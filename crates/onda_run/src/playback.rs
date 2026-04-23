use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use onda_codegen_llvm::TargetOptLevel;
use onda_daemon::{
    DaemonConfig, DaemonSession, RunBufferInfo, RunEventInfo, RunEventValue, RunOptions,
    RunParamInfo,
};
use onda_semantics::AnalysisOptions;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    available_audio_devices, display_path, format_run_build_error, format_single_diagnostic,
    run_buffer_json, run_event_json, run_param_json,
};

const MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK: usize = 64;
const SCOPE_CAPACITY_FRAMES: usize = 4096;

#[cfg(unix)]
static RUN_TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct PlaybackLaunch {
    pub input: PathBuf,
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
}

struct PlaybackStartup {
    path: PathBuf,
    input_channels: usize,
    output_channels: usize,
    params: Vec<RunParamInfo>,
    buffers: Vec<RunBufferInfo>,
    events: Vec<RunEventInfo>,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    current_input_device: Option<String>,
    current_output_device: Option<String>,
}

enum PlaybackControlCommand {
    GetParams {
        reply: mpsc::Sender<Result<Vec<RunParamInfo>, String>>,
    },
    GetBuffers {
        reply: mpsc::Sender<Result<Vec<RunBufferInfo>, String>>,
    },
    GetEvents {
        reply: mpsc::Sender<Result<Vec<RunEventInfo>, String>>,
    },
    GetDevices {
        reply: mpsc::Sender<Result<(Vec<String>, Vec<String>), String>>,
    },
    SetParam {
        name: String,
        value: f64,
        reply: Option<mpsc::Sender<Result<(), String>>>,
    },
    TriggerEvent {
        name: String,
        values: Vec<RunEventValue>,
        reply: Option<mpsc::Sender<Result<(), String>>>,
    },
    BindBufferWav {
        name: String,
        path: PathBuf,
        reply: mpsc::Sender<Result<(), String>>,
    },
    ClearBuffer {
        name: String,
        reply: mpsc::Sender<Result<(), String>>,
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
    replies: Vec<mpsc::Sender<Result<(), String>>>,
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

pub fn play_run_realtime(launch: PlaybackLaunch) -> Result<(), String> {
    let _signal_guard = install_run_signal_handlers();
    let host = cpal::default_host();
    let output_device = find_output_device(&host, launch.output_device.as_deref())?;
    let default_output_config = output_device
        .default_output_config()
        .map_err(|err| format!("failed to query default output config: {err}"))?;

    let output_device_channels = usize::from(default_output_config.channels());
    let mut output_config: cpal::StreamConfig = default_output_config.config();
    output_config.channels = default_output_config.channels();
    output_config.sample_rate = cpal::SampleRate(launch.sample_rate_hz);
    output_config.buffer_size = cpal::BufferSize::Fixed(launch.block_frames as u32);

    let queue_capacity = (launch.block_frames * output_device_channels.max(2) * 16)
        .next_power_of_two()
        .max(1024);
    let sample_queue = Arc::new(SpscSampleRing::new(queue_capacity));
    let input_queue = Arc::new(SpscSampleRing::new(queue_capacity));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let render_error = Arc::new(Mutex::new(None::<String>));
    let error_state = Arc::new(Mutex::new(None::<String>));
    let (startup_tx, startup_rx) = mpsc::channel();
    let (control_tx, control_rx) = if launch.control_json {
        let (tx, rx) = mpsc::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let scope_ring = Arc::new(Mutex::new(ScopeRing::new(0, 0)));
    let render_thread = spawn_run_render_thread(
        launch.clone(),
        Arc::clone(&sample_queue),
        Arc::clone(&input_queue),
        Arc::clone(&scope_ring),
        Arc::clone(&stop_flag),
        Arc::clone(&render_error),
        startup_tx,
        control_rx,
    );
    let startup = startup_rx
        .recv()
        .map_err(|_| "run render thread exited before startup completed".to_owned())??;

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
            control_tx,
            Arc::clone(&scope_ring),
            Arc::clone(&stop_flag),
        ))
    } else {
        None
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
    if startup.output_channels == 0 {
        stop_flag.store(true, Ordering::Release);
        let _ = render_thread.join();
        drop(control_server);
        return Err("daemon play requires at least one output channel".to_owned());
    }

    let input_stream = if startup.input_channels > 0 {
        let input_device = find_input_device(&host, launch.input_device.as_deref())?;
        let default_input_config = input_device
            .default_input_config()
            .map_err(|err| format!("failed to query default input config: {err}"))?;
        let mut input_config: cpal::StreamConfig = default_input_config.config();
        input_config.channels = default_input_config.channels();
        input_config.sample_rate = cpal::SampleRate(launch.sample_rate_hz);
        input_config.buffer_size = cpal::BufferSize::Fixed(launch.block_frames as u32);
        Some(match default_input_config.sample_format() {
            cpal::SampleFormat::F32 => build_input_stream::<f32>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            cpal::SampleFormat::I16 => build_input_stream::<i16>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            cpal::SampleFormat::U16 => build_input_stream::<u16>(
                &input_device,
                &input_config,
                startup.input_channels,
                Arc::clone(&input_queue),
                make_input_stream_error_handler(Arc::clone(&error_state)),
            )?,
            other => {
                stop_flag.store(true, Ordering::Release);
                let _ = render_thread.join();
                drop(control_server);
                return Err(format!(
                    "unsupported input sample format from audio device: {other:?}"
                ));
            }
        })
    } else {
        None
    };

    wait_for_prefill(
        &sample_queue,
        startup.output_channels * launch.block_frames,
        &stop_flag,
        &render_error,
    )?;
    if stop_flag.load(Ordering::Acquire) {
        let _ = render_thread.join();
        drop(control_server);
        return Ok(());
    }

    let stream = match default_output_config.sample_format() {
        cpal::SampleFormat::F32 => build_output_stream::<f32>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        cpal::SampleFormat::I16 => build_output_stream::<i16>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        cpal::SampleFormat::U16 => build_output_stream::<u16>(
            &output_device,
            &output_config,
            output_device_channels,
            startup.output_channels,
            Arc::clone(&sample_queue),
            make_stream_error_handler(Arc::clone(&error_state)),
        )?,
        other => {
            stop_flag.store(true, Ordering::Release);
            let _ = render_thread.join();
            return Err(format!(
                "unsupported output sample format from audio device: {other:?}"
            ));
        }
    };

    if let Some(input_stream) = input_stream.as_ref() {
        input_stream
            .play()
            .map_err(|err| format!("failed to start audio input stream: {err}"))?;
    }
    stream
        .play()
        .map_err(|err| format!("failed to start audio output stream: {err}"))?;
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

    wait_for_playback_completion(launch.dur_seconds, &stop_flag, &render_error, &error_state)?;

    stop_flag.store(true, Ordering::Release);
    drop(input_stream);
    drop(stream);
    let _ = render_thread.join();
    drop(control_server);

    if let Some(err) = error_state
        .lock()
        .map_err(|_| "failed to read audio stream error state".to_owned())?
        .clone()
    {
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
    error_state: &Arc<Mutex<Option<String>>>,
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
        if let Some(err) = error_state
            .lock()
            .map_err(|_| "failed to read audio stream error state".to_owned())?
            .clone()
        {
            return Err(err);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    device_channels: usize,
    source_channels: usize,
    sample_queue: Arc<SpscSampleRing>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                ensure_realtime_thread_fp_mode();
                write_output_data::<T>(data, device_channels, source_channels, &sample_queue)
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build audio output stream: {err}"))
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    target_channels: usize,
    input_queue: Arc<SpscSampleRing>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let device_channels = usize::from(config.channels);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                ensure_realtime_thread_fp_mode();
                write_input_data::<T>(data, device_channels, target_channels, &input_queue)
            },
            err_fn,
            None,
        )
        .map_err(|err| format!("failed to build audio input stream: {err}"))
}

fn make_stream_error_handler(
    error_state: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        if let Ok(mut slot) = error_state.lock() {
            *slot = Some(format!("audio output stream error: {err}"));
        }
    }
}

fn make_input_stream_error_handler(
    error_state: Arc<Mutex<Option<String>>>,
) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err| {
        if let Ok(mut slot) = error_state.lock() {
            *slot = Some(format!("audio input stream error: {err}"));
        }
    }
}

thread_local! {
    static REALTIME_FP_MODE_CONFIGURED: Cell<bool> = const { Cell::new(false) };
}

fn ensure_realtime_thread_fp_mode() {
    REALTIME_FP_MODE_CONFIGURED.with(|configured| {
        if configured.get() {
            return;
        }
        configure_realtime_thread_fp_mode();
        configured.set(true);
    });
}

#[cfg(target_arch = "x86_64")]
fn configure_realtime_thread_fp_mode() {
    // Flush denormals to zero on realtime threads to avoid severe x86 stalls when
    // tiny float values appear during parameter smoothing or feedback decay.
    unsafe {
        let mut csr = 0_u32;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        let desired = csr | (1 << 15) | (1 << 6);
        if desired != csr {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &desired, options(nostack, preserves_flags));
        }
    }
}

#[cfg(target_arch = "x86")]
fn configure_realtime_thread_fp_mode() {
    unsafe {
        let mut csr = 0_u32;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        let desired = csr | (1 << 15) | (1 << 6);
        if desired != csr {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &desired, options(nostack, preserves_flags));
        }
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn configure_realtime_thread_fp_mode() {}

fn write_output_data<T>(
    data: &mut [T],
    device_channels: usize,
    source_channels: usize,
    sample_queue: &Arc<SpscSampleRing>,
) where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    for frame in data.chunks_mut(device_channels) {
        if source_channels == 1 {
            let sample = sample_queue.pop_one().unwrap_or(0.0);
            for out in frame.iter_mut() {
                *out = T::from_sample(sample);
            }
            continue;
        }

        for (channel_index, sample) in frame.iter_mut().enumerate() {
            let value = if channel_index < source_channels {
                sample_queue.pop_one().unwrap_or(0.0)
            } else {
                0.0
            };
            *sample = T::from_sample(value);
        }
        for _ in device_channels..source_channels {
            let _ = sample_queue.pop_one();
        }
    }
}

fn write_input_data<T>(
    data: &[T],
    device_channels: usize,
    target_channels: usize,
    input_queue: &Arc<SpscSampleRing>,
) where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    if device_channels == 0 || target_channels == 0 {
        return;
    }
    for frame in data.chunks(device_channels) {
        for sample in frame.iter().take(target_channels).copied() {
            if !input_queue.push_one(f32::from_sample(sample)) {
                return;
            }
        }
    }
}

fn find_output_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .output_devices()
            .map_err(|err| format!("failed to enumerate output devices: {err}"))?
            .find(|device| {
                device
                    .name()
                    .map(|device_name| device_name == name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("output device '{name}' was not found")),
        None => host
            .default_output_device()
            .ok_or_else(|| "no default output audio device available".to_owned()),
    }
}

fn find_input_device(
    host: &cpal::Host,
    requested_name: Option<&str>,
) -> Result<cpal::Device, String> {
    match requested_name {
        Some(name) => host
            .input_devices()
            .map_err(|err| format!("failed to enumerate input devices: {err}"))?
            .find(|device| {
                device
                    .name()
                    .map(|device_name| device_name == name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("input device '{name}' was not found")),
        None => host
            .default_input_device()
            .ok_or_else(|| "no default input audio device available".to_owned()),
    }
}

fn spawn_run_render_thread(
    launch: PlaybackLaunch,
    sample_queue: Arc<SpscSampleRing>,
    input_queue: Arc<SpscSampleRing>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
    render_error: Arc<Mutex<Option<String>>>,
    startup_tx: mpsc::Sender<Result<PlaybackStartup, String>>,
    control_rx: Option<mpsc::Receiver<PlaybackControlCommand>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        ensure_realtime_thread_fp_mode();
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

        let startup = (|| -> Result<PlaybackStartup, String> {
            session
                .start_run(&launch.input)
                .map_err(|err| format_run_build_error("daemon play start failed", &err))?;

            let run = session
                .run(&launch.input)
                .expect("run should be active after successful start");
            let params = if launch.show_meta || launch.control_json {
                run.param_info()
            } else {
                Vec::new()
            };
            let buffers = if launch.control_json {
                run.buffer_info()
            } else {
                Vec::new()
            };
            let events = if launch.show_meta || launch.control_json {
                run.event_info()
            } else {
                Vec::new()
            };
            let input_devices = Vec::new();
            let output_devices = Vec::new();
            let input_channels = run.input_channel_count();
            let output_channels = run.output_channel_count();
            let path = run.path().to_path_buf();

            for (name, value) in &launch.param_sets {
                session
                    .run_mut(&launch.input)
                    .expect("run should be active while applying params")
                    .set_param_f64(name, *value)
                    .map_err(|diag| format_single_diagnostic("daemon play param failed", &diag))?;
            }

            Ok(PlaybackStartup {
                path,
                input_channels,
                output_channels,
                params,
                buffers,
                events,
                input_devices,
                output_devices,
                current_input_device: launch.input_device.clone(),
                current_output_device: launch.output_device.clone(),
            })
        })();

        let render_input_channels = match startup {
            Ok(ref startup) => {
                {
                    let mut ring = scope_ring.lock().unwrap();
                    *ring = ScopeRing::new(SCOPE_CAPACITY_FRAMES, startup.output_channels);
                }
                if startup_tx
                    .send(Ok(PlaybackStartup {
                        path: startup.path.clone(),
                        input_channels: startup.input_channels,
                        output_channels: startup.output_channels,
                        params: startup.params.clone(),
                        buffers: startup.buffers.clone(),
                        events: startup.events.clone(),
                        input_devices: startup.input_devices.clone(),
                        output_devices: startup.output_devices.clone(),
                        current_input_device: startup.current_input_device.clone(),
                        current_output_device: startup.current_output_device.clone(),
                    }))
                    .is_err()
                {
                    return;
                }
                startup.input_channels
            }
            Err(err) => {
                let _ = startup_tx.send(Err(err));
                return;
            }
        };

        while !stop_flag.load(Ordering::Acquire) {
            if let Some(control_rx) = &control_rx {
                let mut pending_param_updates = HashMap::<String, PendingParamUpdate>::new();
                for _ in 0..MAX_CONTROL_COMMANDS_PER_RENDER_BLOCK {
                    let Ok(command) = control_rx.try_recv() else {
                        break;
                    };
                    match command {
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
                                    run.trigger_event(&name, &values).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play trigger event failed",
                                            &diag,
                                        )
                                    })
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
                                    run.bind_buffer_wav_path(&name, &path).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play bind buffer failed",
                                            &diag,
                                        )
                                    })
                                });
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
                                    run.clear_buffer(&name).map_err(|diag| {
                                        format_single_diagnostic(
                                            "daemon play clear buffer failed",
                                            &diag,
                                        )
                                    })
                                });
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

            if render_input_channels > 0 {
                let input_channels = render_input_channels;
                let input_samples = launch.block_frames * input_channels;
                let mut captured = vec![0.0_f32; input_samples];
                for sample in &mut captured {
                    *sample = input_queue.pop_one().unwrap_or(0.0);
                }
                if let Some(run) = session.run_mut(&launch.input) {
                    run.set_input_block(&captured, input_channels);
                }
            }

            let block = match session.render_run_block(&launch.input) {
                Ok(block) => block,
                Err(diag) => {
                    store_thread_error(
                        &render_error,
                        format_single_diagnostic("daemon play render failed", &diag),
                    );
                    stop_flag.store(true, Ordering::Release);
                    break;
                }
            };

            let mut interleaved = Vec::with_capacity(
                block.len() * block.first().map(Vec::len).unwrap_or(launch.block_frames),
            );
            append_interleaved_block(&mut interleaved, &block);

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

fn flush_pending_param_updates(
    pending: &mut HashMap<String, PendingParamUpdate>,
    session: &mut DaemonSession,
    input: &Path,
) {
    for (name, update) in std::mem::take(pending) {
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
    sample_queue: &Arc<SpscSampleRing>,
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

fn spawn_run_control_server(
    listener: TcpListener,
    control_tx: mpsc::Sender<PlaybackControlCommand>,
    scope_ring: Arc<Mutex<ScopeRing>>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }

        while !stop_flag.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(err) =
                        handle_run_control_client(stream, &control_tx, &scope_ring, &stop_flag)
                    {
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
    control_tx: &mpsc::Sender<PlaybackControlCommand>,
    scope_ring: &Arc<Mutex<ScopeRing>>,
    stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|err| format!("failed to set control socket read timeout: {err}"))?;
    let reader_stream = stream
        .try_clone()
        .map_err(|err| format!("failed to clone control socket: {err}"))?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = BufWriter::new(stream);

    while !stop_flag.load(Ordering::Acquire) {
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
        let response = run_control_response(request, control_tx, scope_ring);
        if let Some(response) = response {
            write_json_line(&mut writer, &response)
                .map_err(|err| format!("failed to write control response: {err}"))?;
        }
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
        "setParam" => {
            let result = (|| -> Result<Option<Value>, String> {
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
            })();
            result
        }
        "triggerEvent" => {
            let result = (|| -> Result<Option<Value>, String> {
                let name = request
                    .name
                    .ok_or_else(|| "triggerEvent requires 'name'".to_owned())?;
                let raw_values = request.values.unwrap_or_default();
                let values = raw_values
                    .into_iter()
                    .map(|value| match value {
                        Value::Bool(value) => Ok(RunEventValue::Bool(value)),
                        Value::Number(value) => value
                            .as_f64()
                            .map(RunEventValue::Number)
                            .ok_or_else(|| "triggerEvent values must be numeric".to_owned()),
                        _ => Err("triggerEvent values must be numbers or booleans".to_owned()),
                    })
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
            })();
            result
        }
        "bindBufferWav" => {
            let result = (|| -> Result<Option<Value>, String> {
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
            })();
            result
        }
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
        "clearBuffer" => {
            let result = (|| -> Result<Option<Value>, String> {
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
            })();
            result
        }
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

struct SpscSampleRing {
    capacity: usize,
    mask: usize,
    slots: Box<[UnsafeCell<f32>]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

unsafe impl Send for SpscSampleRing {}
unsafe impl Sync for SpscSampleRing {}

impl SpscSampleRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        let slots = std::iter::repeat_with(|| UnsafeCell::new(0.0))
            .take(capacity)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            capacity,
            mask: capacity - 1,
            slots,
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        let write = self.write_index.load(Ordering::Acquire);
        let read = self.read_index.load(Ordering::Acquire);
        write.saturating_sub(read)
    }

    fn push_slice(&self, input: &[f32]) -> usize {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        let available = self.capacity.saturating_sub(write.saturating_sub(read));
        let count = input.len().min(available);
        for (offset, sample) in input.iter().copied().take(count).enumerate() {
            let index = (write + offset) & self.mask;
            // SAFETY: producer is single-writer and only writes slots not yet published via write_index.
            unsafe { *self.slots[index].get() = sample };
        }
        if count != 0 {
            self.write_index.store(write + count, Ordering::Release);
        }
        count
    }

    fn push_one(&self, sample: f32) -> bool {
        let write = self.write_index.load(Ordering::Relaxed);
        let read = self.read_index.load(Ordering::Acquire);
        if self.capacity.saturating_sub(write.saturating_sub(read)) == 0 {
            return false;
        }
        let index = write & self.mask;
        // SAFETY: producer is single-writer and only writes slots not yet published via write_index.
        unsafe { *self.slots[index].get() = sample };
        self.write_index.store(write + 1, Ordering::Release);
        true
    }

    fn pop_one(&self) -> Option<f32> {
        let read = self.read_index.load(Ordering::Relaxed);
        let write = self.write_index.load(Ordering::Acquire);
        if read == write {
            return None;
        }
        let index = read & self.mask;
        // SAFETY: consumer is single-reader and only reads slots published via write_index.
        let sample = unsafe { *self.slots[index].get() };
        self.read_index.store(read + 1, Ordering::Release);
        Some(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::{run_control_response, PlaybackControlCommand, PlaybackControlRequest, ScopeRing};
    use onda_daemon::RunEventValue;
    use serde_json::Value;
    use std::sync::{mpsc, Arc, Mutex};

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
                values: Some(vec![Value::from(60), Value::from(0.75), Value::Bool(true)]),
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
                    ]
                );
                assert!(reply.is_none());
            }
            _ => panic!("expected triggerEvent command"),
        }
    }
}
