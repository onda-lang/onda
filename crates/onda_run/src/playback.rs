use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use onda_codegen_llvm::TargetOptLevel;
use onda_cpal::{
    configure_current_thread_fp_mode, sample_ring, AudioHost, InputEndpoint, OutputEndpoint,
    SampleConsumer, SampleProducer, StreamErrorState,
};
use onda_daemon::{
    DaemonConfig, DaemonSession, InitialBufferBinding, RunBufferInfo, RunBuildError,
    RunDelegateBatch, RunDelegateInfo, RunDelegateOccurrence, RunEventInfo, RunEventValue,
    RunOptions, RunParamInfo, RunPrintBatch, RunPrintEntry, RunSession,
};
use onda_project::{BufferAsset, ProjectLimits};
use onda_semantics::AnalysisOptions;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    available_audio_devices, display_path, format_run_build_error, format_single_diagnostic,
    run_buffer_json, run_event_json, run_event_value_json, run_param_json,
};

const MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK: usize = 64;
const SCOPE_CAPACITY_FRAMES: usize = 4096;
const DELEGATE_NOTIFICATION_CAPACITY: usize = 32;
const PRINT_NOTIFICATION_CAPACITY: usize = 32;

#[cfg(unix)]
static RUN_TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct PlaybackLaunch {
    pub input: PathBuf,
    pub compile_inputs: onda_semantics::CompileInputs,
    pub dur_seconds: Option<u32>,
    pub sample_rate_hz: u32,
    pub block_frames: usize,
    pub opt_level: TargetOptLevel,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub fast_math: bool,
    pub show_meta: bool,
    pub control_json: bool,
    pub param_sets: Vec<(String, f64)>,
    pub buffer_bindings: Vec<(String, PathBuf)>,
    pub project_buffer_bindings: Vec<ProjectBufferBinding>,
}

#[derive(Clone)]
pub struct ProjectBufferBinding {
    pub name: String,
    pub asset: BufferAsset,
    pub loaded_path: Option<PathBuf>,
}

struct PlaybackStartup {
    path: PathBuf,
    input_channels: usize,
    output_channels: usize,
    params: Vec<RunParamInfo>,
    buffers: Vec<RunBufferInfo>,
    events: Vec<RunEventInfo>,
    delegates: Vec<RunDelegateInfo>,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    current_input_device: Option<String>,
    current_output_device: Option<String>,
}

struct PlaybackStartupFailure {
    message: String,
    print_batch: Option<RunPrintBatch>,
}

impl From<String> for PlaybackStartupFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            print_batch: None,
        }
    }
}

type PlaybackReply<T> = mpsc::Sender<Result<T, String>>;
type AudioDeviceLists = (Vec<String>, Vec<String>);

struct RenderThreadContext {
    sample_queue: SampleProducer,
    input_queue: SampleConsumer,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
    render_error: Arc<Mutex<Option<String>>>,
    startup_tx: mpsc::Sender<Result<PlaybackStartup, PlaybackStartupFailure>>,
    control_rx: Option<mpsc::Receiver<PlaybackControlCommand>>,
    delegate_transport: Option<DelegateTransport>,
    print_transport: PrintTransport,
}

#[derive(Clone)]
struct DelegateTransport {
    sender: mpsc::SyncSender<RunDelegateBatch>,
    dropped_occurrences: Arc<AtomicU32>,
}

#[derive(Clone)]
struct PrintTransport {
    sender: mpsc::SyncSender<RunPrintBatch>,
    dropped_occurrences: Arc<AtomicU32>,
    pending_overflow: Arc<AtomicU32>,
}

enum PlaybackControlCommand {
    Pause {
        reply: PlaybackReply<()>,
    },
    Play {
        reply: PlaybackReply<()>,
    },
    ResetParams {
        reply: PlaybackReply<()>,
    },
    GetParams {
        reply: PlaybackReply<Vec<RunParamInfo>>,
    },
    GetBuffers {
        reply: PlaybackReply<Vec<RunBufferInfo>>,
    },
    GetEvents {
        reply: PlaybackReply<Vec<RunEventInfo>>,
    },
    GetDevices {
        reply: PlaybackReply<AudioDeviceLists>,
    },
    SetDelegateCollection {
        enabled: bool,
        reply: Option<PlaybackReply<()>>,
    },
    SetParam {
        name: String,
        value: f64,
        reply: Option<PlaybackReply<()>>,
    },
    TriggerEvent {
        name: String,
        values: Vec<RunEventValue>,
        reply: Option<PlaybackReply<()>>,
    },
    BindBufferWav {
        name: String,
        path: PathBuf,
        reply: PlaybackReply<()>,
    },
    ClearBuffer {
        name: String,
        reply: PlaybackReply<()>,
    },
}

struct ScopeSnapshot {
    channels: usize,
    samples: Vec<f32>,
}

struct ScopeRing {
    buffer: Vec<f32>,
    channels: usize,
    write_pos: usize,
    frames_written: usize,
}

struct PendingParamUpdate {
    value: f64,
    replies: Vec<PlaybackReply<()>>,
}

impl ScopeRing {
    fn new(capacity_frames: usize, channels: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity_frames * channels],
            channels,
            write_pos: 0,
            frames_written: 0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buffer.len();
        if cap == 0 {
            return;
        }
        for (i, &sample) in samples.iter().enumerate() {
            self.buffer[(self.write_pos + i) % cap] = sample;
        }
        self.write_pos = (self.write_pos + samples.len()) % cap;
        self.frames_written += samples.len() / self.channels.max(1);
    }

    fn snapshot(&self, max_frames: usize) -> ScopeSnapshot {
        let total_frames = self.buffer.len() / self.channels.max(1);
        let available = total_frames.min(self.frames_written);
        let frames = max_frames.min(available);
        let sample_count = frames * self.channels;
        let cap = self.buffer.len();
        let start = (self.write_pos + cap - sample_count) % cap;
        let mut samples = Vec::with_capacity(sample_count);
        for i in 0..sample_count {
            samples.push(self.buffer[(start + i) % cap]);
        }
        ScopeSnapshot {
            channels: self.channels,
            samples,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PlaybackControlRequest {
    #[serde(default)]
    id: Option<Value>,
    command: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    values: Option<Vec<Value>>,
    #[serde(default, rename = "maxFrames")]
    max_frames: Option<usize>,
}

struct DelegateSubscriptionGuard {
    control_tx: mpsc::Sender<PlaybackControlCommand>,
    enabled: bool,
}

impl Drop for DelegateSubscriptionGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = self
                .control_tx
                .send(PlaybackControlCommand::SetDelegateCollection {
                    enabled: false,
                    reply: None,
                });
        }
    }
}

pub fn play_run_realtime(launch: PlaybackLaunch) -> Result<(), String> {
    let _signal_guard = install_run_signal_handlers();
    let audio_host = AudioHost::default();
    let output_endpoint = OutputEndpoint::open(
        &audio_host,
        launch.output_device.as_deref(),
        launch.sample_rate_hz,
        launch.block_frames,
    )?;
    let output_device_channels = output_endpoint.channels();

    let queue_capacity = (launch.block_frames * output_device_channels.max(2) * 16)
        .next_power_of_two()
        .max(1024);
    let (sample_producer, sample_consumer) = sample_ring(queue_capacity);
    let (input_producer, input_consumer) = sample_ring(queue_capacity);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let render_error = Arc::new(Mutex::new(None::<String>));
    let error_state = StreamErrorState::default();
    let (startup_tx, startup_rx) = mpsc::channel();
    let (control_tx, control_rx) = if launch.control_json {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let (delegate_transport, delegate_rx, dropped_delegate_occurrences) = if launch.control_json {
        let (sender, receiver) = mpsc::sync_channel(DELEGATE_NOTIFICATION_CAPACITY);
        let dropped = Arc::new(AtomicU32::new(0));
        (
            Some(DelegateTransport {
                sender,
                dropped_occurrences: Arc::clone(&dropped),
            }),
            Some(receiver),
            Some(dropped),
        )
    } else {
        (None, None, None)
    };
    let (print_sender, print_receiver) = mpsc::sync_channel(PRINT_NOTIFICATION_CAPACITY);
    let mut print_rx = Some(print_receiver);
    let dropped_print_occurrences = Arc::new(AtomicU32::new(0));
    let pending_print_overflow = Arc::new(AtomicU32::new(0));
    let print_transport = PrintTransport {
        sender: print_sender,
        dropped_occurrences: Arc::clone(&dropped_print_occurrences),
        pending_overflow: Arc::clone(&pending_print_overflow),
    };

    let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
    let render_thread = spawn_run_render_thread(
        launch.clone(),
        RenderThreadContext {
            sample_queue: sample_producer,
            input_queue: input_consumer,
            scope_ring: Arc::clone(&scope_ring),
            stop_flag: Arc::clone(&stop_flag),
            render_error: Arc::clone(&render_error),
            startup_tx,
            control_rx,
            delegate_transport,
            print_transport,
        },
    );
    let startup = startup_rx
        .recv()
        .map_err(|_| "run render thread exited before startup completed".to_owned())?;
    let startup = match startup {
        Ok(startup) => startup,
        Err(failure) => {
            let _ = render_thread.join();
            let message = failure.message;
            let print_batch = failure.print_batch;
            if launch.control_json {
                let batch = print_batch.unwrap_or(RunPrintBatch {
                    text: String::new(),
                    entries: Vec::new(),
                    overflow_count: 0,
                    transport_drop_count: 0,
                });
                write_json_line(
                    &mut BufWriter::new(std::io::stdout().lock()),
                    &json!({
                        "event": "startupFailed",
                        "error": &message,
                        "text": batch.text,
                        "entries": batch.entries.iter().map(run_print_entry_json).collect::<Vec<_>>(),
                        "overflowCount": batch.overflow_count,
                        "transportDropCount": batch.transport_drop_count,
                    }),
                )
                .map_err(|error| format!("failed to write run startup failure event: {error}"))?;
            } else if let Some(batch) = print_batch {
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                stdout
                    .write_all(batch.text.as_bytes())
                    .and_then(|()| stdout.flush())
                    .map_err(|error| {
                        format!("failed to write run startup print output: {error}")
                    })?;
                if batch.overflow_count != 0 || batch.transport_drop_count != 0 {
                    eprintln!(
                        "onda print delivery: {} generated record(s) overflowed, {} record(s) were dropped in transport",
                        batch.overflow_count, batch.transport_drop_count
                    );
                }
            }
            return Err(message);
        }
    };

    let control_server = if launch.control_json {
        let Some(control_tx) = control_tx else {
            unreachable!("control channel should exist when control json is enabled");
        };
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|err| format!("failed to bind run control socket: {err}"))?;
        let port = listener
            .local_addr()
            .map_err(|err| format!("failed to query run control socket: {err}"))?
            .port();
        let startup_message = json!({
            "event": "ready",
            "path": display_path(&startup.path),
            "port": port,
            "params": startup.params.iter().map(run_param_json).collect::<Vec<_>>(),
            "buffers": startup.buffers.iter().map(run_buffer_json).collect::<Vec<_>>(),
            "events": startup.events.iter().map(run_event_json).collect::<Vec<_>>(),
            "delegates": startup.delegates.iter().map(run_delegate_json).collect::<Vec<_>>(),
            "outputChannels": startup.output_channels,
            "inputDevices": startup.input_devices,
            "outputDevices": startup.output_devices,
            "currentInputDevice": startup.current_input_device,
            "currentOutputDevice": startup.current_output_device,
        });
        write_json_line(
            &mut BufWriter::new(std::io::stdout().lock()),
            &startup_message,
        )
        .map_err(|err| format!("failed to write run control startup event: {err}"))?;
        Some(spawn_run_control_server(
            listener,
            RunControlServerContext {
                control_tx,
                scope_ring: Arc::clone(&scope_ring),
                stop_flag: Arc::clone(&stop_flag),
                delegate_rx: delegate_rx.expect("delegate receiver should exist"),
                dropped_delegate_occurrences: dropped_delegate_occurrences
                    .expect("delegate drop counter should exist"),
                print_rx: print_rx.take().expect("print receiver should be available"),
                dropped_print_occurrences: Arc::clone(&dropped_print_occurrences),
                pending_print_overflow: Arc::clone(&pending_print_overflow),
            },
        ))
    } else {
        None
    };
    let print_output = if launch.control_json {
        None
    } else {
        Some(spawn_run_print_stdout(
            print_rx.take().expect("print receiver should be available"),
            Arc::clone(&dropped_print_occurrences),
            Arc::clone(&pending_print_overflow),
        ))
    };

    if launch.show_meta && !startup.params.is_empty() {
        if launch.control_json {
            eprintln!("{}", format_run_param_info(&startup.params));
        } else {
            println!("{}", format_run_param_info(&startup.params));
        }
    }
    if launch.show_meta && !startup.events.is_empty() {
        if launch.control_json {
            eprintln!("{}", format_run_event_info(&startup.events));
        } else {
            println!("{}", format_run_event_info(&startup.events));
        }
    }
    if launch.show_meta && !startup.delegates.is_empty() {
        if launch.control_json {
            eprintln!("{}", format_run_delegate_info(&startup.delegates));
        } else {
            println!("{}", format_run_delegate_info(&startup.delegates));
        }
    }
    if startup.output_channels == 0 {
        stop_flag.store(true, Ordering::Release);
        let _ = render_thread.join();
        if let Some(server) = control_server {
            let _ = server.join();
        }
        if let Some(output) = print_output {
            let _ = output.join();
        }
        return Err("daemon play requires at least one output channel".to_owned());
    }

    let input_stream = if startup.input_channels > 0 {
        let input_endpoint = InputEndpoint::open(
            &audio_host,
            launch.input_device.as_deref(),
            launch.sample_rate_hz,
            launch.block_frames,
        )?;
        Some(input_endpoint.build_stream(
            startup.input_channels,
            input_producer,
            error_state.clone(),
        )?)
    } else {
        None
    };

    wait_for_prefill(
        &sample_consumer,
        startup.output_channels * launch.block_frames,
        &stop_flag,
        &render_error,
    )?;
    if stop_flag.load(Ordering::Acquire) {
        let _ = render_thread.join();
        if let Some(server) = control_server {
            let _ = server.join();
        }
        if let Some(output) = print_output {
            let _ = output.join();
        }
        return Ok(());
    }

    let stream = output_endpoint.build_stream(
        startup.output_channels,
        sample_consumer,
        error_state.clone(),
    )?;

    if let Some(input_stream) = input_stream.as_ref() {
        input_stream.play()?;
    }
    stream.play()?;
    if launch.control_json {
        eprintln!(
            "{}",
            playback_status_message(&startup.path, launch.dur_seconds)
        );
    } else {
        println!(
            "{}",
            playback_status_message(&startup.path, launch.dur_seconds)
        );
    }

    let playback_result =
        wait_for_playback_completion(launch.dur_seconds, &stop_flag, &render_error, &error_state);

    stop_flag.store(true, Ordering::Release);
    drop(input_stream);
    drop(stream);
    let _ = render_thread.join();
    if let Some(server) = control_server {
        let _ = server.join();
    }
    if let Some(output) = print_output {
        let _ = output.join();
    }

    playback_result?;

    if let Some(err) = error_state.message() {
        return Err(err);
    }
    if let Some(err) = render_error
        .lock()
        .map_err(|_| "failed to read render thread error state".to_owned())?
        .clone()
    {
        return Err(err);
    }
    if run_termination_requested() {
        return Ok(());
    }
    Ok(())
}

pub fn append_interleaved_block(rendered: &mut Vec<f32>, block: &[Vec<f32>]) {
    if block.is_empty() {
        return;
    }
    let frames = block[0].len();
    for frame in 0..frames {
        for channel in block {
            rendered.push(channel[frame]);
        }
    }
}

pub fn format_run_param_info(params: &[RunParamInfo]) -> String {
    let mut lines = Vec::with_capacity(params.len() + 1);
    lines.push("Run params:".to_owned());
    for param in params {
        let range = match (param.range_min, param.range_max) {
            (Some(min), Some(max)) => format!(" [{min}, {max}]"),
            (None, Some(max)) => format!(" [.., {max}]"),
            _ => String::new(),
        };
        let default = param
            .default
            .map(|value| format!(" = {value}"))
            .unwrap_or_default();
        let scalar = if param.scalar { "" } else { " (non-scalar)" };
        lines.push(format!(
            "  {}: {}{}{}{}",
            param.name, param.type_repr, default, range, scalar
        ));
    }
    lines.join("\n")
}

fn format_run_event_info(events: &[RunEventInfo]) -> String {
    let mut lines = Vec::with_capacity(events.len() + 1);
    lines.push("Run events:".to_owned());
    for event in events {
        let signature = if event.params.is_empty() {
            "()".to_owned()
        } else {
            format!(
                "({})",
                event
                    .params
                    .iter()
                    .map(|param| format!("{}: {}", param.name, param.type_repr))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        lines.push(format!("  {}{}", event.name, signature));
    }
    lines.join("\n")
}

fn format_run_delegate_info(delegates: &[RunDelegateInfo]) -> String {
    let mut lines = Vec::with_capacity(delegates.len() + 1);
    lines.push("Run delegates:".to_owned());
    for delegate in delegates {
        let params = delegate
            .params
            .iter()
            .map(|param| format!("{}: {}", param.name, param.type_repr))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("  {}({params})", delegate.name));
    }
    lines.join("\n")
}

fn run_delegate_json(delegate: &RunDelegateInfo) -> Value {
    json!({
        "index": delegate.index,
        "name": delegate.name,
        "params": delegate.params.iter().map(|param| json!({
            "index": param.index,
            "name": param.name,
            "type": param.type_repr,
        })).collect::<Vec<_>>(),
    })
}

fn run_delegate_occurrence_json(occurrence: &RunDelegateOccurrence) -> Value {
    let values = occurrence
        .values
        .iter()
        .map(|entry| (entry.name.clone(), run_event_value_json(&entry.value)))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "index": occurrence.index,
        "name": occurrence.name,
        "values": values,
    })
}

fn playback_status_message(path: &Path, dur_seconds: Option<u32>) -> String {
    match dur_seconds {
        Some(dur_seconds) => format!("Playing {} for {} seconds", display_path(path), dur_seconds),
        None => format!("Playing {} until stopped", display_path(path)),
    }
}

fn wait_for_playback_completion(
    dur_seconds: Option<u32>,
    stop_flag: &Arc<AtomicBool>,
    render_error: &Arc<Mutex<Option<String>>>,
    error_state: &StreamErrorState,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        if run_termination_requested() {
            stop_flag.store(true, Ordering::Release);
            break;
        }
        if stop_flag.load(Ordering::Acquire) {
            break;
        }
        if let Some(limit) = dur_seconds {
            if start.elapsed() >= Duration::from_secs(u64::from(limit)) {
                break;
            }
        }
        if let Some(err) = render_error
            .lock()
            .map_err(|_| "failed to read render thread error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        if let Some(err) = error_state.message() {
            return Err(err);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn spawn_run_render_thread(
    mut launch: PlaybackLaunch,
    context: RenderThreadContext,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let RenderThreadContext {
            sample_queue,
            input_queue,
            scope_ring,
            stop_flag,
            render_error,
            startup_tx,
            control_rx,
            delegate_transport,
            print_transport,
        } = context;
        configure_current_thread_fp_mode();
        let control_rx = control_rx;
        let mut session = DaemonSession::new(DaemonConfig {
            analysis: AnalysisOptions {
                sample_rate: launch.sample_rate_hz as f32,
                block_size: launch.block_frames,
            },
            run: RunOptions {
                sample_rate: launch.sample_rate_hz as f32,
                block_size: launch.block_frames,
                fast_math: launch.fast_math,
                opt_level: launch.opt_level,
                ..RunOptions::default()
            },
        });

        let startup = (|| -> Result<PlaybackStartup, PlaybackStartupFailure> {
            let mut initial_buffers = Vec::with_capacity(
                launch.project_buffer_bindings.len() + launch.buffer_bindings.len(),
            );
            initial_buffers.extend(
                std::mem::take(&mut launch.project_buffer_bindings)
                    .into_iter()
                    .map(|binding| {
                        InitialBufferBinding::from_asset(
                            binding.name,
                            binding.asset,
                            binding.loaded_path,
                        )
                    }),
            );
            for (name, path) in &launch.buffer_bindings {
                initial_buffers.push(
                    InitialBufferBinding::load_file(name.clone(), path, ProjectLimits::default())
                        .map_err(|error| {
                        format!("failed to load buffer asset '{}': {error}", path.display())
                    })?,
                );
            }
            session
                .start_run_with_options_inputs_and_initial_buffers(
                    &launch.input,
                    RunOptions {
                        sample_rate: launch.sample_rate_hz as f32,
                        block_size: launch.block_frames,
                        fast_math: launch.fast_math,
                        opt_level: launch.opt_level,
                        ..RunOptions::default()
                    },
                    &launch.compile_inputs,
                    initial_buffers,
                )
                .map_err(|error| {
                    let print_batch = match &error {
                        RunBuildError::Initialization { print_batch, .. } => {
                            print_batch.as_deref().cloned()
                        }
                        _ => None,
                    };
                    PlaybackStartupFailure {
                        message: format_run_build_error("daemon play start failed", &error),
                        print_batch,
                    }
                })?;

            for (name, value) in &launch.param_sets {
                session
                    .run_mut(&launch.input)
                    .expect("run should be active while applying params")
                    .set_param_f64(name, *value)
                    .map_err(|diag| format_single_diagnostic("daemon play param failed", &diag))?;
            }

            publish_run_print_batch(
                session
                    .run_mut(&launch.input)
                    .expect("run should be active after successful start"),
                &print_transport,
            )?;

            let run = session
                .run(&launch.input)
                .expect("run should be active after successful start");
            let params = if launch.show_meta || launch.control_json {
                run.param_info()
            } else {
                Vec::new()
            };
            let buffers = run.buffer_info();
            let events = if launch.show_meta || launch.control_json {
                run.event_info()
            } else {
                Vec::new()
            };
            let delegates = if launch.show_meta || launch.control_json {
                run.delegate_info()
            } else {
                Vec::new()
            };
            let input_devices = Vec::new();
            let output_devices = Vec::new();
            let input_channels = run.input_channel_count();
            let output_channels = run.output_channel_count();
            let path = run.path().to_path_buf();

            Ok(PlaybackStartup {
                path,
                input_channels,
                output_channels,
                params,
                buffers,
                events,
                delegates,
                input_devices,
                output_devices,
                current_input_device: launch.input_device.clone(),
                current_output_device: launch.output_device.clone(),
            })
        })();

        let (render_input_channels, render_output_channels) = match startup {
            Ok(startup) => {
                {
                    let mut ring = scope_ring.lock().unwrap();
                    *ring = ScopeRing::new(SCOPE_CAPACITY_FRAMES, startup.output_channels);
                }
                let channel_counts = (startup.input_channels, startup.output_channels);
                if startup_tx.send(Ok(startup)).is_err() {
                    return;
                }
                channel_counts
            }
            Err(err) => {
                let _ = startup_tx.send(Err(err));
                return;
            }
        };

        let mut pending_param_updates = HashMap::<String, PendingParamUpdate>::with_capacity(8);
        let mut play_requested = true;
        let mut playing = session.run(&launch.input).is_some();
        let mut captured = vec![0.0_f32; launch.block_frames.saturating_mul(render_input_channels)];
        let mut interleaved =
            vec![0.0_f32; launch.block_frames.saturating_mul(render_output_channels)];

        while !stop_flag.load(Ordering::Acquire) {
            if let Some(control_rx) = &control_rx {
                for _ in 0..MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK {
                    let Ok(command) = control_rx.try_recv() else {
                        break;
                    };
                    match command {
                        PlaybackControlCommand::Pause { reply } => {
                            play_requested = false;
                            playing = false;
                            let _ = reply.send(Ok(()));
                        }
                        PlaybackControlCommand::Play { reply } => {
                            play_requested = true;
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let drain_deadline = Instant::now() + Duration::from_millis(75);
                            while !sample_queue.is_empty() && Instant::now() < drain_deadline {
                                thread::sleep(Duration::from_millis(1));
                            }
                            input_queue.discard_buffered();
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .and_then(|run| {
                                    let execution = run.restart().map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play restart failed",
                                            &diag,
                                        )
                                    });
                                    publish_run_print_batch(run, &print_transport)?;
                                    execution
                                });
                            playing = play_requested && result.is_ok();
                            if playing {
                                if let Ok(mut ring) = scope_ring.try_lock() {
                                    *ring = ScopeRing::new(
                                        SCOPE_CAPACITY_FRAMES,
                                        render_output_channels,
                                    );
                                }
                            }
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::ResetParams { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .and_then(|run| {
                                    run.reset_params().map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon run parameter reset failed",
                                            &diag,
                                        )
                                    })
                                });
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::GetParams { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run(&launch.input)
                                .map(|run| run.param_info())
                                .ok_or_else(|| "run is not active".to_owned());
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::GetBuffers { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run(&launch.input)
                                .map(|run| run.buffer_info())
                                .ok_or_else(|| "run is not active".to_owned());
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::GetEvents { reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run(&launch.input)
                                .map(|run| run.event_info())
                                .ok_or_else(|| "run is not active".to_owned());
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::GetDevices { reply } => {
                            let _ = reply.send(Ok(available_audio_devices()));
                        }
                        PlaybackControlCommand::SetDelegateCollection { enabled, reply } => {
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .map(|run| run.set_delegate_collection_enabled(enabled));
                            if let Some(reply) = reply {
                                let _ = reply.send(result);
                            }
                        }
                        PlaybackControlCommand::SetParam { name, value, reply } => {
                            let entry = pending_param_updates.entry(name).or_insert_with(|| {
                                PendingParamUpdate {
                                    value,
                                    replies: Vec::new(),
                                }
                            });
                            entry.value = value;
                            if let Some(reply) = reply {
                                entry.replies.push(reply);
                            }
                        }
                        PlaybackControlCommand::TriggerEvent {
                            name,
                            values,
                            reply,
                        } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .and_then(|run| {
                                    let execution =
                                        run.trigger_event(&name, &values).map_err(|diag| {
                                            format_single_diagnostic(
                                                "daemon play trigger event failed",
                                                &diag,
                                            )
                                        });
                                    publish_run_print_batch(run, &print_transport)?;
                                    publish_run_delegate_batch(run, delegate_transport.as_ref())?;
                                    execution
                                });
                            if let Some(reply) = reply {
                                let _ = reply.send(result);
                            }
                        }
                        PlaybackControlCommand::BindBufferWav { name, path, reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .and_then(|run| {
                                    let result =
                                        run.bind_buffer_wav_path(&name, &path).map_err(|diag| {
                                            format_single_diagnostic(
                                                "daemon play bind buffer failed",
                                                &diag,
                                            )
                                        });
                                    publish_run_print_batch(run, &print_transport)?;
                                    result
                                });
                            if result.is_ok() {
                                playing = play_requested;
                            }
                            let _ = reply.send(result);
                        }
                        PlaybackControlCommand::ClearBuffer { name, reply } => {
                            flush_pending_param_updates(
                                &mut pending_param_updates,
                                &mut session,
                                &launch.input,
                            );
                            let result = session
                                .run_mut(&launch.input)
                                .ok_or_else(|| "run is not active".to_owned())
                                .and_then(|run| {
                                    let result = run.clear_buffer(&name).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play clear buffer failed",
                                            &diag,
                                        )
                                    });
                                    publish_run_print_batch(run, &print_transport)?;
                                    result
                                });
                            if result.is_ok() {
                                playing = play_requested;
                            }
                            let _ = reply.send(result);
                        }
                    }
                }
                flush_pending_param_updates(
                    &mut pending_param_updates,
                    &mut session,
                    &launch.input,
                );
            }

            if !playing {
                thread::sleep(Duration::from_millis(1));
                continue;
            }

            if render_input_channels > 0 {
                let input_channels = render_input_channels;
                input_queue.pop_slice_aligned(&mut captured, input_channels);
                if let Some(run) = session.run_mut(&launch.input) {
                    run.set_input_block(&captured, input_channels);
                }
            }

            let execution = session.render_run_block_interleaved(&launch.input, &mut interleaved);
            let output_result = session
                .run_mut(&launch.input)
                .ok_or_else(|| "run is not active".to_owned())
                .and_then(|run| {
                    publish_run_print_batch(run, &print_transport)?;
                    publish_run_delegate_batch(run, delegate_transport.as_ref())
                });
            if let Err(error) = output_result {
                store_thread_error(&render_error, error);
                stop_flag.store(true, Ordering::Release);
                break;
            }
            if let Err(diag) = execution {
                store_thread_error(
                    &render_error,
                    format_single_diagnostic("daemon play render failed", &diag),
                );
                stop_flag.store(true, Ordering::Release);
                break;
            }

            if let Ok(mut ring) = scope_ring.try_lock() {
                ring.push_interleaved(&interleaved);
            }

            let mut offset = 0;
            while offset < interleaved.len() && !stop_flag.load(Ordering::Acquire) {
                let written = sample_queue.push_slice(&interleaved[offset..]);
                if written == 0 {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                offset += written;
            }
        }
    })
}

fn publish_run_delegate_batch(
    run: &mut RunSession,
    transport: Option<&DelegateTransport>,
) -> Result<(), String> {
    let batch = run.take_delegate_batch().map_err(|diagnostic| {
        format_single_diagnostic("daemon play delegate decoding failed", &diagnostic)
    })?;
    if batch.occurrences.is_empty() && batch.overflow_count == 0 {
        return Ok(());
    }
    let Some(transport) = transport else {
        return Ok(());
    };
    if let Err(mpsc::TrySendError::Full(batch)) = transport.sender.try_send(batch) {
        let dropped = u32::try_from(batch.occurrences.len())
            .unwrap_or(u32::MAX)
            .saturating_add(batch.overflow_count);
        let _ = transport.dropped_occurrences.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_add(dropped)),
        );
    }
    Ok(())
}

fn publish_run_print_batch(run: &mut RunSession, transport: &PrintTransport) -> Result<(), String> {
    let batch = run.take_print_batch().map_err(|diagnostic| {
        format_single_diagnostic("daemon play print decoding failed", &diagnostic)
    })?;
    if batch.entries.is_empty() && batch.overflow_count == 0 {
        return Ok(());
    }
    if let Err(mpsc::TrySendError::Full(batch)) = transport.sender.try_send(batch) {
        atomic_saturating_add(
            &transport.dropped_occurrences,
            u32::try_from(batch.entries.len()).unwrap_or(u32::MAX),
        );
        atomic_saturating_add(&transport.pending_overflow, batch.overflow_count);
    }
    Ok(())
}

fn atomic_saturating_add(counter: &AtomicU32, value: u32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

fn run_print_entry_json(entry: &RunPrintEntry) -> Value {
    json!({
        "siteIndex": entry.site_index,
        "label": entry.label,
        "source": {
            "file": entry.source_file,
            "line": entry.line,
            "column": entry.column,
            "endLine": entry.end_line,
            "endColumn": entry.end_column,
        },
        "lexicalOwner": entry.lexical_owner,
        "declaration": entry.declaration,
        "values": entry.values.iter().map(|value| json!({
            "type": value.type_repr,
            "value": run_event_value_json(&value.value),
        })).collect::<Vec<_>>(),
    })
}

fn spawn_run_print_stdout(
    receiver: mpsc::Receiver<RunPrintBatch>,
    dropped_occurrences: Arc<AtomicU32>,
    pending_overflow: Arc<AtomicU32>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(batch) = receiver.recv() {
            print!("{}", batch.text);
            let generated = batch
                .overflow_count
                .saturating_add(pending_overflow.swap(0, Ordering::Relaxed));
            let transported = batch
                .transport_drop_count
                .saturating_add(dropped_occurrences.swap(0, Ordering::Relaxed));
            if generated != 0 || transported != 0 {
                eprintln!(
                    "onda print delivery: {generated} generated record(s) overflowed, {transported} record(s) were dropped in transport"
                );
            }
        }
        let generated = pending_overflow.swap(0, Ordering::Relaxed);
        let transported = dropped_occurrences.swap(0, Ordering::Relaxed);
        if generated != 0 || transported != 0 {
            eprintln!(
                "onda print delivery: {generated} generated record(s) overflowed, {transported} record(s) were dropped in transport"
            );
        }
        let _ = std::io::stdout().flush();
    })
}

fn flush_pending_param_updates(
    pending: &mut HashMap<String, PendingParamUpdate>,
    session: &mut DaemonSession,
    input: &Path,
) {
    for (name, update) in pending.drain() {
        let result = session
            .run_mut(input)
            .ok_or_else(|| "run is not active".to_owned())
            .and_then(|run| {
                run.set_param_f64(&name, update.value)
                    .map_err(|diag| format_single_diagnostic("daemon play param failed", &diag))
            });
        for reply in update.replies {
            let _ = reply.send(result.clone());
        }
    }
}

fn wait_for_prefill(
    sample_queue: &SampleConsumer,
    min_samples: usize,
    stop_flag: &Arc<AtomicBool>,
    render_error: &Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    while sample_queue.len() < min_samples && !stop_flag.load(Ordering::Acquire) {
        if run_termination_requested() {
            stop_flag.store(true, Ordering::Release);
            break;
        }
        if let Some(err) = render_error
            .lock()
            .map_err(|_| "failed to read render thread error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn store_thread_error(slot: &Arc<Mutex<Option<String>>>, message: String) {
    if let Ok(mut slot) = slot.lock() {
        if slot.is_none() {
            *slot = Some(message);
        }
    }
}

#[cfg(unix)]
extern "C" fn run_termination_signal_handler(_sig: libc::c_int) {
    RUN_TERMINATION_REQUESTED.store(true, Ordering::Release);
}

#[cfg(unix)]
struct RunSignalGuard {
    previous_sigint: libc::sighandler_t,
    previous_sigterm: libc::sighandler_t,
}

#[cfg(unix)]
impl Drop for RunSignalGuard {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.previous_sigint);
            libc::signal(libc::SIGTERM, self.previous_sigterm);
        }
        RUN_TERMINATION_REQUESTED.store(false, Ordering::Release);
    }
}

#[cfg(unix)]
fn install_run_signal_handlers() -> RunSignalGuard {
    RUN_TERMINATION_REQUESTED.store(false, Ordering::Release);
    let handler = run_termination_signal_handler as *const () as libc::sighandler_t;
    let previous_sigint = unsafe { libc::signal(libc::SIGINT, handler) };
    let previous_sigterm = unsafe { libc::signal(libc::SIGTERM, handler) };
    RunSignalGuard {
        previous_sigint,
        previous_sigterm,
    }
}

#[cfg(not(unix))]
struct RunSignalGuard;

#[cfg(not(unix))]
fn install_run_signal_handlers() -> RunSignalGuard {
    RunSignalGuard
}

#[cfg(unix)]
fn run_termination_requested() -> bool {
    RUN_TERMINATION_REQUESTED.load(Ordering::Acquire)
}

#[cfg(not(unix))]
fn run_termination_requested() -> bool {
    false
}

fn write_json_line(writer: &mut impl Write, value: &Value) -> Result<(), std::io::Error> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn run_event_value_from_json(value: Value) -> Result<RunEventValue, String> {
    match value {
        Value::Bool(value) => Ok(RunEventValue::Bool(value)),
        Value::Number(value) => value
            .as_f64()
            .map(RunEventValue::Number)
            .ok_or_else(|| "triggerEvent values must be numeric".to_owned()),
        Value::String(value) => value
            .parse::<i64>()
            .map(RunEventValue::I64)
            .map_err(|_| "triggerEvent string values must be decimal i64 integers".to_owned()),
        Value::Array(values) => values
            .into_iter()
            .map(run_event_value_from_json)
            .collect::<Result<Vec<_>, _>>()
            .map(RunEventValue::Array),
        _ => Err(
            "triggerEvent values must be numbers, decimal i64 strings, booleans, or arrays"
                .to_owned(),
        ),
    }
}

struct RunControlServerContext {
    control_tx: mpsc::Sender<PlaybackControlCommand>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
    delegate_rx: mpsc::Receiver<RunDelegateBatch>,
    dropped_delegate_occurrences: Arc<AtomicU32>,
    print_rx: mpsc::Receiver<RunPrintBatch>,
    dropped_print_occurrences: Arc<AtomicU32>,
    pending_print_overflow: Arc<AtomicU32>,
}

fn spawn_run_control_server(
    listener: TcpListener,
    context: RunControlServerContext,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }

        while !context.stop_flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) = handle_run_control_client(stream, &context) {
                        eprintln!("run control client error: {err}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    eprintln!("run control accept error: {err}");
                    break;
                }
            }
        }
    })
}

fn handle_run_control_client(
    stream: TcpStream,
    context: &RunControlServerContext,
) -> Result<(), String> {
    let mut delegate_subscription = DelegateSubscriptionGuard {
        control_tx: context.control_tx.clone(),
        enabled: false,
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|err| format!("failed to set control socket read timeout: {err}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|err| format!("failed to clone control socket: {err}"))?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = BufWriter::new(stream);

    while !context.stop_flag.load(Ordering::Acquire) {
        write_pending_delegate_batch(
            &mut writer,
            &context.delegate_rx,
            &context.dropped_delegate_occurrences,
        )?;
        write_pending_print_batches(
            &mut writer,
            &context.print_rx,
            &context.dropped_print_occurrences,
            &context.pending_print_overflow,
        )?;
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(err) => return Err(format!("failed to read control request: {err}")),
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request: PlaybackControlRequest = serde_json::from_str(trimmed)
            .map_err(|err| format!("invalid control request json: {err}"))?;
        let delegate_subscription_request = match request.command.as_str() {
            "subscribeDelegates" => Some(true),
            "unsubscribeDelegates" => Some(false),
            _ => None,
        };
        let response = run_control_response(request, &context.control_tx, &context.scope_ring);
        let request_succeeded = response
            .as_ref()
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if request_succeeded {
            if let Some(enabled) = delegate_subscription_request {
                delegate_subscription.enabled = enabled;
            }
        }
        if let Some(response) = response {
            write_json_line(&mut writer, &response)
                .map_err(|err| format!("failed to write control response: {err}"))?;
        }
    }

    // The render producer may publish its last batch or loss counters while
    // shutdown is being observed. Flush once after the stop transition so a
    // connected control consumer sees terminal output even without a later
    // authored print occurrence.
    write_pending_delegate_batch(
        &mut writer,
        &context.delegate_rx,
        &context.dropped_delegate_occurrences,
    )?;
    write_pending_print_batches(
        &mut writer,
        &context.print_rx,
        &context.dropped_print_occurrences,
        &context.pending_print_overflow,
    )?;

    Ok(())
}

fn write_pending_print_batches(
    writer: &mut impl Write,
    receiver: &mpsc::Receiver<RunPrintBatch>,
    dropped_occurrences: &AtomicU32,
    pending_overflow: &AtomicU32,
) -> Result<(), String> {
    let mut wrote_batch = false;
    while let Ok(batch) = receiver.try_recv() {
        wrote_batch = true;
        let notification = json!({
            "event": "print",
            "text": batch.text,
            "entries": batch.entries.iter().map(run_print_entry_json).collect::<Vec<_>>(),
            "overflowCount": batch.overflow_count.saturating_add(
                pending_overflow.swap(0, Ordering::Relaxed)
            ),
            "transportDropCount": batch.transport_drop_count.saturating_add(
                dropped_occurrences.swap(0, Ordering::Relaxed)
            ),
        });
        write_json_line(writer, &notification)
            .map_err(|error| format!("failed to write print notification: {error}"))?;
    }
    if !wrote_batch {
        let overflow_count = pending_overflow.swap(0, Ordering::Relaxed);
        let transport_drop_count = dropped_occurrences.swap(0, Ordering::Relaxed);
        if overflow_count != 0 || transport_drop_count != 0 {
            write_json_line(
                writer,
                &json!({
                    "event": "print",
                    "text": "",
                    "entries": [],
                    "overflowCount": overflow_count,
                    "transportDropCount": transport_drop_count,
                }),
            )
            .map_err(|error| format!("failed to write print loss notification: {error}"))?;
        }
    }
    Ok(())
}

fn write_pending_delegate_batch(
    writer: &mut impl Write,
    receiver: &mpsc::Receiver<RunDelegateBatch>,
    dropped_delegate_occurrences: &AtomicU32,
) -> Result<(), String> {
    while let Ok(batch) = receiver.try_recv() {
        let transport_drop_count = dropped_delegate_occurrences.swap(0, Ordering::Relaxed);
        let notification = json!({
            "event": "delegates",
            "occurrences": batch.occurrences.iter().map(run_delegate_occurrence_json).collect::<Vec<_>>(),
            "overflowCount": batch.overflow_count,
            "transportDropCount": transport_drop_count,
        });
        write_json_line(writer, &notification)
            .map_err(|error| format!("failed to write delegate notification: {error}"))?;
    }
    Ok(())
}

fn run_control_response(
    request: PlaybackControlRequest,
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
) -> Option<Value> {
    let request_id = request.id;
    let result = match request.command.as_str() {
        "pause" | "play" | "resetParams" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            let command = match request.command.as_str() {
                "pause" => PlaybackControlCommand::Pause { reply: reply_tx },
                "play" => PlaybackControlCommand::Play { reply: reply_tx },
                "resetParams" => PlaybackControlCommand::ResetParams { reply: reply_tx },
                _ => unreachable!("matched playback transport command"),
            };
            control_tx
                .send(command)
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    request_id.clone().map(|id| match result {
                        Ok(()) => json!({ "id": id, "ok": true }),
                        Err(err) => json!({ "id": id, "ok": false, "error": err }),
                    })
                })
        }
        "getParams" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetParams { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(params) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "params": params.iter().map(run_param_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getBuffers" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetBuffers { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(buffers) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "buffers": buffers.iter().map(run_buffer_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getEvents" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetEvents { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok(events) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "events": events.iter().map(run_event_json).collect::<Vec<_>>(),
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "getDevices" => {
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::GetDevices { reply: reply_tx })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    Some(match result {
                        Ok((input_devices, output_devices)) => json!({
                            "id": request_id,
                            "ok": true,
                            "result": {
                                "inputDevices": input_devices,
                                "outputDevices": output_devices,
                            }
                        }),
                        Err(err) => json!({
                            "id": request_id,
                            "ok": false,
                            "error": err,
                        }),
                    })
                })
        }
        "subscribeDelegates" | "unsubscribeDelegates" => {
            let enabled = request.command == "subscribeDelegates";
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::SetDelegateCollection {
                    enabled,
                    reply: Some(reply_tx),
                })
                .map_err(|_| "run control channel closed".to_owned())
                .and_then(|_| {
                    reply_rx
                        .recv()
                        .map_err(|_| "run control reply channel closed".to_owned())
                })
                .map(|result| {
                    request_id.clone().map(|id| match result {
                        Ok(()) => json!({ "id": id, "ok": true }),
                        Err(err) => json!({ "id": id, "ok": false, "error": err }),
                    })
                })
        }
        "setParam" => (|| -> Result<Option<Value>, String> {
            let name = request
                .name
                .ok_or_else(|| "setParam requires 'name'".to_owned())?;
            let raw_value = request
                .value
                .ok_or_else(|| "setParam requires 'value'".to_owned())?;
            let value = match raw_value {
                Value::Bool(value) => {
                    if value {
                        1.0
                    } else {
                        0.0
                    }
                }
                Value::Number(value) => value
                    .as_f64()
                    .ok_or_else(|| "setParam value must be numeric".to_owned())?,
                _ => return Err("setParam value must be number or boolean".to_owned()),
            };
            if request_id.is_none() {
                control_tx
                    .send(PlaybackControlCommand::SetParam {
                        name,
                        value,
                        reply: None,
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                return Ok(None);
            }
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::SetParam {
                    name,
                    value,
                    reply: Some(reply_tx),
                })
                .map_err(|_| "run control channel closed".to_owned())?;
            match reply_rx
                .recv()
                .map_err(|_| "run control reply channel closed".to_owned())?
            {
                Ok(()) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": true,
                    })
                })),
                Err(err) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    })
                })),
            }
        })(),
        "triggerEvent" => (|| -> Result<Option<Value>, String> {
            let name = request
                .name
                .ok_or_else(|| "triggerEvent requires 'name'".to_owned())?;
            let raw_values = request.values.unwrap_or_default();
            let values = raw_values
                .into_iter()
                .map(run_event_value_from_json)
                .collect::<Result<Vec<_>, _>>()?;
            if request_id.is_none() {
                control_tx
                    .send(PlaybackControlCommand::TriggerEvent {
                        name,
                        values,
                        reply: None,
                    })
                    .map_err(|_| "run control channel closed".to_owned())?;
                return Ok(None);
            }
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::TriggerEvent {
                    name,
                    values,
                    reply: Some(reply_tx),
                })
                .map_err(|_| "run control channel closed".to_owned())?;
            match reply_rx
                .recv()
                .map_err(|_| "run control reply channel closed".to_owned())?
            {
                Ok(()) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": true,
                    })
                })),
                Err(err) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    })
                })),
            }
        })(),
        "bindBufferWav" => (|| -> Result<Option<Value>, String> {
            let name = request
                .name
                .ok_or_else(|| "bindBufferWav requires 'name'".to_owned())?;
            let path = request
                .path
                .ok_or_else(|| "bindBufferWav requires 'path'".to_owned())?;
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::BindBufferWav {
                    name,
                    path: PathBuf::from(path),
                    reply: reply_tx,
                })
                .map_err(|_| "run control channel closed".to_owned())?;
            match reply_rx
                .recv()
                .map_err(|_| "run control reply channel closed".to_owned())?
            {
                Ok(()) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": true,
                    })
                })),
                Err(err) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    })
                })),
            }
        })(),
        "getScopeData" => scope_ring
            .lock()
            .map_err(|_| "failed to lock scope ring".to_owned())
            .map(|ring| {
                let snapshot = ring.snapshot(request.max_frames.unwrap_or(2048));
                Some(json!({
                    "id": request_id,
                    "ok": true,
                    "result": {
                        "channels": snapshot.channels,
                        "samples": snapshot.samples,
                    }
                }))
            }),
        "clearBuffer" => (|| -> Result<Option<Value>, String> {
            let name = request
                .name
                .ok_or_else(|| "clearBuffer requires 'name'".to_owned())?;
            let (reply_tx, reply_rx) = mpsc::channel();
            control_tx
                .send(PlaybackControlCommand::ClearBuffer {
                    name,
                    reply: reply_tx,
                })
                .map_err(|_| "run control channel closed".to_owned())?;
            match reply_rx
                .recv()
                .map_err(|_| "run control reply channel closed".to_owned())?
            {
                Ok(()) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": true,
                    })
                })),
                Err(err) => Ok(request_id.clone().map(|id| {
                    json!({
                        "id": id,
                        "ok": false,
                        "error": err,
                    })
                })),
            }
        })(),
        other => Err(format!("unknown command '{other}'")),
    };

    match result {
        Ok(value) => value,
        Err(err) => request_id.map(|id| {
            json!({
                "id": id,
                "ok": false,
                "error": err,
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        run_control_response, write_pending_delegate_batch, write_pending_print_batches,
        DelegateSubscriptionGuard, PlaybackControlCommand, PlaybackControlRequest, ScopeRing,
    };
    use onda_daemon::{
        RunDelegateBatch, RunDelegateOccurrence, RunDelegateValue, RunEventValue, RunPrintBatch,
        RunPrintEntry, RunPrintValue,
    };
    use serde_json::Value;
    use std::sync::{atomic::AtomicU32, mpsc, Arc, Mutex};

    #[test]
    fn delegate_collection_is_disabled_until_explicitly_subscribed() {
        let (sender, receiver) = mpsc::channel();
        drop(DelegateSubscriptionGuard {
            control_tx: sender.clone(),
            enabled: false,
        });
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        drop(DelegateSubscriptionGuard {
            control_tx: sender,
            enabled: true,
        });
        assert!(matches!(
            receiver.recv().expect("unsubscribe command"),
            PlaybackControlCommand::SetDelegateCollection {
                enabled: false,
                reply: None,
            }
        ));
    }

    #[test]
    fn run_delegate_notifications_preserve_payloads_and_drop_counts() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RunDelegateBatch {
                occurrences: vec![RunDelegateOccurrence {
                    index: 2,
                    name: "meter".to_owned(),
                    values: vec![
                        RunDelegateValue {
                            name: "level".to_owned(),
                            value: RunEventValue::Number(0.75),
                        },
                        RunDelegateValue {
                            name: "bins".to_owned(),
                            value: RunEventValue::Array(vec![
                                RunEventValue::Number(0.25),
                                RunEventValue::Number(0.5),
                            ]),
                        },
                    ],
                }],
                overflow_count: 3,
            })
            .expect("delegate batch should be queued");

        let dropped = AtomicU32::new(4);
        let mut bytes = Vec::new();
        write_pending_delegate_batch(&mut bytes, &receiver, &dropped)
            .expect("delegate batch should be serialized");

        let notification: Value =
            serde_json::from_slice(&bytes).expect("delegate batch should be valid JSON");
        assert_eq!(
            notification,
            serde_json::json!({
                "event": "delegates",
                "occurrences": [{
                    "index": 2,
                    "name": "meter",
                    "values": {
                        "level": 0.75,
                        "bins": [0.25, 0.5],
                    },
                }],
                "overflowCount": 3,
                "transportDropCount": 4,
            })
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn run_delegate_notifications_encode_i64_as_decimal_strings() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RunDelegateBatch {
                occurrences: vec![RunDelegateOccurrence {
                    index: 0,
                    name: "wide".to_owned(),
                    values: vec![RunDelegateValue {
                        name: "value".to_owned(),
                        value: RunEventValue::I64(9_007_199_254_740_993),
                    }],
                }],
                overflow_count: 0,
            })
            .expect("delegate batch should be queued");

        let mut bytes = Vec::new();
        write_pending_delegate_batch(&mut bytes, &receiver, &AtomicU32::new(0))
            .expect("delegate batch should serialize");
        let notification: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(
            notification["occurrences"][0]["values"]["value"],
            Value::String("9007199254740993".to_owned())
        );
    }

    #[test]
    fn run_print_notifications_preserve_text_source_values_and_loss_counts() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RunPrintBatch {
                text: "wide: 9007199254740993\n".to_owned(),
                entries: vec![RunPrintEntry {
                    site_index: 3,
                    label: Some("wide".to_owned()),
                    source_file: Some("voice.onda".to_owned()),
                    line: 7,
                    column: 3,
                    end_line: 7,
                    end_column: 24,
                    lexical_owner: "Voice".to_owned(),
                    declaration: Some("sample".to_owned()),
                    values: vec![RunPrintValue {
                        type_repr: "i64".to_owned(),
                        value: RunEventValue::I64(9_007_199_254_740_993),
                    }],
                }],
                overflow_count: 2,
                transport_drop_count: 1,
            })
            .expect("print batch should be queued");

        let dropped = AtomicU32::new(4);
        let pending_overflow = AtomicU32::new(8);
        let mut bytes = Vec::new();
        write_pending_print_batches(&mut bytes, &receiver, &dropped, &pending_overflow)
            .expect("print batch should serialize");

        let notification: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(notification["event"], "print");
        assert_eq!(notification["text"], "wide: 9007199254740993\n");
        assert_eq!(notification["overflowCount"], 10);
        assert_eq!(notification["transportDropCount"], 5);
        assert_eq!(notification["entries"][0]["source"]["file"], "voice.onda");
        assert_eq!(notification["entries"][0]["lexicalOwner"], "Voice");
        assert_eq!(
            notification["entries"][0]["values"][0]["value"],
            "9007199254740993"
        );
    }

    #[test]
    fn run_print_transport_reports_terminal_loss_without_a_later_batch() {
        let (_sender, receiver) = mpsc::channel();
        let dropped = AtomicU32::new(6);
        let pending_overflow = AtomicU32::new(9);
        let mut bytes = Vec::new();

        write_pending_print_batches(&mut bytes, &receiver, &dropped, &pending_overflow)
            .expect("loss-only print notification should serialize");

        let notification: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(
            notification,
            serde_json::json!({
                "event": "print",
                "text": "",
                "entries": [],
                "overflowCount": 9,
                "transportDropCount": 6,
            })
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            pending_overflow.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn run_set_param_notification_enqueues_without_waiting_for_reply() {
        let (control_tx, control_rx) = mpsc::channel();
        let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
        let response = run_control_response(
            PlaybackControlRequest {
                id: None,
                command: "setParam".to_owned(),
                name: Some("gain".to_owned()),
                path: None,
                value: Some(Value::from(0.5)),
                values: None,
                max_frames: None,
            },
            &control_tx,
            &scope_ring,
        );

        assert!(response.is_none());
        match control_rx.try_recv().expect("setParam should be queued") {
            PlaybackControlCommand::SetParam { name, value, reply } => {
                assert_eq!(name, "gain");
                assert_eq!(value, 0.5);
                assert!(reply.is_none());
            }
            _ => panic!("expected setParam command"),
        }
    }

    #[test]
    fn run_trigger_event_notification_enqueues_full_payload() {
        let (control_tx, control_rx) = mpsc::channel();
        let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
        let response = run_control_response(
            PlaybackControlRequest {
                id: None,
                command: "triggerEvent".to_owned(),
                name: Some("note_on".to_owned()),
                path: None,
                value: None,
                values: Some(vec![
                    Value::from(60),
                    Value::from(0.75),
                    Value::Bool(true),
                    serde_json::json!([0.25, 0.5]),
                ]),
                max_frames: None,
            },
            &control_tx,
            &scope_ring,
        );

        assert!(response.is_none());
        match control_rx
            .try_recv()
            .expect("triggerEvent should be queued")
        {
            PlaybackControlCommand::TriggerEvent {
                name,
                values,
                reply,
            } => {
                assert_eq!(name, "note_on");
                assert_eq!(
                    values,
                    vec![
                        RunEventValue::Number(60.0),
                        RunEventValue::Number(0.75),
                        RunEventValue::Bool(true),
                        RunEventValue::Array(vec![
                            RunEventValue::Number(0.25),
                            RunEventValue::Number(0.5),
                        ]),
                    ]
                );
                assert!(reply.is_none());
            }
            _ => panic!("expected triggerEvent command"),
        }
    }

    #[test]
    fn run_play_command_waits_for_runtime_restart() {
        let (control_tx, control_rx) = mpsc::channel();
        let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
        let worker =
            std::thread::spawn(
                move || match control_rx.recv().expect("play should be queued") {
                    PlaybackControlCommand::Play { reply } => {
                        reply.send(Ok(())).expect("play reply should be received");
                    }
                    _ => panic!("expected play command"),
                },
            );

        let response = run_control_response(
            PlaybackControlRequest {
                id: Some(Value::from(7)),
                command: "play".to_owned(),
                name: None,
                path: None,
                value: None,
                values: None,
                max_frames: None,
            },
            &control_tx,
            &scope_ring,
        )
        .expect("play request should return a response");

        worker.join().expect("play worker should finish");
        assert_eq!(response.get("id"), Some(&Value::from(7)));
        assert_eq!(response.get("ok"), Some(&Value::Bool(true)));
    }

    #[test]
    fn run_reset_params_command_waits_for_runtime_reset() {
        let (control_tx, control_rx) = mpsc::channel();
        let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
        let worker = std::thread::spawn(move || {
            match control_rx.recv().expect("resetParams should be queued") {
                PlaybackControlCommand::ResetParams { reply } => {
                    reply
                        .send(Ok(()))
                        .expect("resetParams reply should be received");
                }
                _ => panic!("expected resetParams command"),
            }
        });

        let response = run_control_response(
            PlaybackControlRequest {
                id: Some(Value::from(8)),
                command: "resetParams".to_owned(),
                name: None,
                path: None,
                value: None,
                values: None,
                max_frames: None,
            },
            &control_tx,
            &scope_ring,
        )
        .expect("resetParams request should return a response");

        worker.join().expect("resetParams worker should finish");
        assert_eq!(response.get("id"), Some(&Value::from(8)));
        assert_eq!(response.get("ok"), Some(&Value::Bool(true)));
    }
}
