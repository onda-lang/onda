use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use onda_daemon::{
    RunBufferChannels as DaemonRunBufferChannels, RunBuildError, RunEventInfo, RunEventParamInfo,
    RunEventValue, RunParamInfo,
};
use onda_frontend::Diagnostic;
use serde::Deserialize;
use serde_json::{json, Value};

mod playback;

pub use playback::{
    append_interleaved_block, format_run_param_info, play_run_realtime, PlaybackLaunch,
};

const SCOPE_MAX_FRAMES: usize = 1024;
const SCOPE_POLL_INTERVAL_MS: u64 = 50;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum RunThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Debug)]
pub struct RunHostOptions {
    pub sample_rate_hz: u32,
    pub block_frames: usize,
    pub opt_level: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub fast_math: bool,
    pub show_meta: bool,
    pub theme: RunThemeMode,
    pub onda_bin: String,
}

impl Default for RunHostOptions {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            block_frames: 128,
            opt_level: "3".to_owned(),
            input_device: None,
            output_device: None,
            fast_math: false,
            show_meta: false,
            theme: RunThemeMode::Auto,
            onda_bin: "onda".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunState {
    pub running: bool,
    pub connected: bool,
    pub path: String,
    pub status: String,
    pub error: Option<String>,
    pub output_channels: usize,
    pub buffers: Vec<Value>,
    pub events: Vec<Value>,
    pub params: Vec<Value>,
    pub input_devices: Vec<String>,
    pub output_devices: Vec<String>,
    pub current_input_device: Option<String>,
    pub current_output_device: Option<String>,
    pub scope_channels: usize,
    pub scope_samples: Vec<f32>,
}

impl RunState {
    fn new(path: &Path, options: &RunHostOptions) -> Self {
        Self {
            running: false,
            connected: false,
            path: display_path(path),
            status: "Stopped".to_owned(),
            error: None,
            output_channels: 0,
            buffers: Vec::new(),
            events: Vec::new(),
            params: Vec::new(),
            input_devices: list_input_devices(),
            output_devices: list_output_devices(),
            current_input_device: options.input_device.clone(),
            current_output_device: options.output_device.clone(),
            scope_channels: 0,
            scope_samples: Vec::new(),
        }
    }
}

pub fn available_audio_devices() -> (Vec<String>, Vec<String>) {
    onda_cpal::available_audio_devices()
}

#[derive(Debug)]
enum ControllerEvent {
    ChildReady(ReadyEvent),
    TcpResponse(String),
    FileChanged,
}

#[derive(Debug, Clone)]
struct ReadyEvent {
    path: String,
    port: u16,
    params: Vec<Value>,
    buffers: Vec<Value>,
    events: Vec<Value>,
    output_channels: usize,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    current_input_device: Option<String>,
    current_output_device: Option<String>,
}

#[derive(Deserialize)]
struct RawReadyEvent {
    event: String,
    path: Option<String>,
    port: Option<u16>,
    params: Option<Vec<Value>>,
    buffers: Option<Vec<Value>>,
    events: Option<Vec<Value>>,
    #[serde(rename = "outputChannels")]
    output_channels: Option<usize>,
    #[serde(rename = "inputDevices")]
    input_devices: Option<Vec<String>>,
    #[serde(rename = "outputDevices")]
    output_devices: Option<Vec<String>>,
    #[serde(rename = "currentInputDevice")]
    current_input_device: Option<String>,
    #[serde(rename = "currentOutputDevice")]
    current_output_device: Option<String>,
}

pub struct RunController {
    onda_path: PathBuf,
    options: RunHostOptions,
    state: RunState,
    events_rx: Receiver<ControllerEvent>,
    events_tx: Sender<ControllerEvent>,
    bridge: IpcBridge,
    child: ChildSession,
    _watcher: Option<FileWatcher>,
    preserved_params: Vec<(String, Value)>,
    preserved_buffers: Vec<(String, String)>,
    preserved_events: Vec<(String, Vec<Value>)>,
    scope_polling_active: bool,
    scope_polling_in_flight: bool,
    last_scope_poll: Instant,
    compiled_source: Option<Vec<u8>>,
    pending_source: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PollResult {
    pub state_changed: bool,
    pub scope_changed: bool,
}

impl RunController {
    pub fn new(onda_path: &Path, options: RunHostOptions) -> Result<Self, String> {
        let onda_path = std::fs::canonicalize(onda_path)
            .map_err(|e| format!("cannot resolve path {}: {e}", onda_path.display()))?;
        let (events_tx, events_rx) = mpsc::channel();
        let bridge = IpcBridge::new();
        let state = RunState::new(&onda_path, &options);
        let pending_source = fs::read(&onda_path).ok();
        let child = ChildSession::spawn(&onda_path, &options, events_tx.clone())
            .map_err(|e| format!("failed to start run subprocess: {e}"))?;
        let watcher_tx = events_tx.clone();
        let watcher = FileWatcher::watch(&onda_path, move || {
            let _ = watcher_tx.send(ControllerEvent::FileChanged);
        });

        let mut controller = Self {
            onda_path,
            options,
            state,
            events_rx,
            events_tx,
            bridge,
            child,
            _watcher: watcher,
            preserved_params: Vec::new(),
            preserved_buffers: Vec::new(),
            preserved_events: Vec::new(),
            scope_polling_active: false,
            scope_polling_in_flight: false,
            last_scope_poll: Instant::now(),
            compiled_source: None,
            pending_source,
        };
        controller.state.running = true;
        controller.state.status = "Starting...".to_owned();
        Ok(controller)
    }

    pub fn state(&self) -> &RunState {
        &self.state
    }

    pub fn path(&self) -> &Path {
        &self.onda_path
    }

    pub fn poll(&mut self) -> PollResult {
        let mut result = PollResult::default();

        if let Some((code, error)) = self.child.try_take_exit() {
            self.handle_child_exited(code, error);
            result.state_changed = true;
        }

        if self.scope_polling_active
            && !self.scope_polling_in_flight
            && self.last_scope_poll.elapsed() >= Duration::from_millis(SCOPE_POLL_INTERVAL_MS)
        {
            self.bridge
                .send_command("getScopeData", &json!({ "maxFrames": SCOPE_MAX_FRAMES }));
            self.scope_polling_in_flight = true;
            self.last_scope_poll = Instant::now();
        }

        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                ControllerEvent::ChildReady(ready) => {
                    self.handle_child_ready(ready);
                    result.state_changed = true;
                }
                ControllerEvent::TcpResponse(line) => {
                    if self.handle_tcp_response(&line) {
                        result.scope_changed = true;
                    }
                }
                ControllerEvent::FileChanged => {
                    if self.source_requires_recompile() {
                        if self.state.running {
                            let _ = self.restart_with_status("Restarting...");
                        } else {
                            self.invalidate_compiled_child();
                        }
                        result.state_changed = true;
                    }
                }
            }
        }

        result
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.can_reuse_compiled_child() {
            self.bridge.send_command("play", &json!({}));
            self.scope_polling_active = true;
            self.scope_polling_in_flight = false;
            self.last_scope_poll = Instant::now();
            self.state.running = true;
            self.state.connected = true;
            self.state.status = "Running".to_owned();
            self.state.error = None;
            self.state.scope_channels = 0;
            self.state.scope_samples.clear();
            return Ok(());
        }
        self.restart_with_status("Starting...")
    }

    pub fn stop(&mut self) {
        if self.state.connected && self.child.is_active() {
            self.bridge.send_command("pause", &json!({}));
        } else {
            self.child.kill();
            self.bridge.disconnect();
            self.pending_source = None;
        }
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.state.running = false;
        self.state.connected = false;
        self.state.status = "Stopped".to_owned();
        self.state.error = None;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();
    }

    pub fn refresh_devices(&mut self) {
        let (input_devices, output_devices) = available_audio_devices();
        self.state.input_devices = input_devices;
        self.state.output_devices = output_devices;
        self.state.error = None;
    }

    pub fn reset(&mut self) {
        self.preserved_params.clear();
        self.preserved_events.clear();
        for param in &mut self.state.params {
            let Some(name) = param_name(param).map(str::to_owned) else {
                continue;
            };
            let Some(default_value) = param_default_value(param) else {
                continue;
            };
            set_param_value(param, default_value.clone());
            self.bridge.send_command_notification(
                "setParam",
                &json!({ "name": name, "value": default_value }),
            );
        }
        reset_event_values(&mut self.state.events);
        self.state.error = None;
    }

    pub fn set_param(&mut self, name: &str, value: Value) {
        self.bridge
            .send_command_notification("setParam", &json!({ "name": name, "value": value }));
        if let Some(entry) = self.preserved_params.iter_mut().find(|(n, _)| n == name) {
            entry.1 = value.clone();
        } else {
            self.preserved_params.push((name.to_owned(), value.clone()));
        }
        update_param_value(&mut self.state.params, name, value);
        self.state.error = None;
    }

    pub fn trigger_event(&mut self, name: &str, values: Vec<Value>) {
        self.bridge
            .send_command("triggerEvent", &json!({ "name": name, "values": values }));
        if let Some(entry) = self.preserved_events.iter_mut().find(|(n, _)| n == name) {
            entry.1 = values.clone();
        } else {
            self.preserved_events
                .push((name.to_owned(), values.clone()));
        }
        update_event_values(&mut self.state.events, name, &values);
        self.state.error = None;
    }

    pub fn bind_buffer_file(&mut self, name: &str, file_path: &str) {
        self.bridge
            .send_command("bindBufferWav", &json!({ "name": name, "path": file_path }));
        if let Some(entry) = self.preserved_buffers.iter_mut().find(|(n, _)| n == name) {
            entry.1 = file_path.to_owned();
        } else {
            self.preserved_buffers
                .push((name.to_owned(), file_path.to_owned()));
        }
        update_buffer_loaded_path(&mut self.state.buffers, name, Some(file_path));
        self.state.error = None;
    }

    pub fn clear_buffer(&mut self, name: &str) {
        self.bridge
            .send_command("clearBuffer", &json!({ "name": name }));
        self.preserved_buffers.retain(|(n, _)| n != name);
        update_buffer_loaded_path(&mut self.state.buffers, name, None);
        self.state.error = None;
    }

    pub fn set_input_device(&mut self, name: Option<&str>) -> Result<(), String> {
        let next = match validated_device(name, &self.state.input_devices) {
            Ok(next) => next,
            Err(err) => {
                self.state.error = Some(err.clone());
                return Err(err);
            }
        };
        self.options.input_device = next;
        self.state.current_input_device = self.options.input_device.clone();
        self.restart_with_status("Restarting...")
    }

    pub fn set_output_device(&mut self, name: Option<&str>) -> Result<(), String> {
        let next = match validated_device(name, &self.state.output_devices) {
            Ok(next) => next,
            Err(err) => {
                self.state.error = Some(err.clone());
                return Err(err);
            }
        };
        self.options.output_device = next;
        self.state.current_output_device = self.options.output_device.clone();
        self.restart_with_status("Restarting...")
    }

    fn restart_with_status(&mut self, status: &str) -> Result<(), String> {
        self.child.kill();
        self.bridge.disconnect();
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.state.running = false;
        self.state.connected = false;
        self.state.status = status.to_owned();
        self.state.error = None;
        self.state.output_channels = 0;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();

        self.pending_source = fs::read(&self.onda_path).ok();
        match ChildSession::spawn(&self.onda_path, &self.options, self.events_tx.clone()) {
            Ok(child) => {
                self.child = child;
                self.state.running = true;
                Ok(())
            }
            Err(e) => {
                self.pending_source = None;
                self.state.status = "Failed to start".to_owned();
                self.state.error = Some(e.clone());
                Err(e)
            }
        }
    }

    fn can_reuse_compiled_child(&self) -> bool {
        self.bridge.is_connected()
            && self.child.is_active()
            && self.compiled_source.is_some()
            && self.compiled_source == fs::read(&self.onda_path).ok()
    }

    fn source_requires_recompile(&self) -> bool {
        let current = fs::read(&self.onda_path).ok();
        source_differs_from_cached(
            current.as_deref(),
            self.compiled_source.as_deref(),
            self.pending_source.as_deref(),
        )
    }

    fn invalidate_compiled_child(&mut self) {
        self.child.kill();
        self.bridge.disconnect();
        self.compiled_source = None;
        self.pending_source = None;
        self.state.connected = false;
        self.state.status = "Stopped".to_owned();
        self.state.error = None;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();
    }

    fn handle_child_ready(&mut self, ready: ReadyEvent) {
        self.compiled_source = self
            .pending_source
            .take()
            .or_else(|| fs::read(&self.onda_path).ok());
        let bridge_error = self
            .bridge
            .connect(ready.port, self.events_tx.clone())
            .err();

        reconcile_preserved_params(
            &mut self.preserved_params,
            &self.state.params,
            &ready.params,
        );
        self.state.params = ready.params;
        apply_preserved_param_state(&mut self.state.params, &self.preserved_params);
        self.state.buffers = ready.buffers;
        apply_preserved_buffer_state(&mut self.state.buffers, &self.preserved_buffers);
        self.state.events = ready.events;
        apply_preserved_event_state(&mut self.state.events, &self.preserved_events);
        self.state.path = ready.path;
        self.state.output_channels = ready.output_channels;
        if !ready.input_devices.is_empty() {
            self.state.input_devices = ready.input_devices;
        } else if self.state.input_devices.is_empty() {
            self.state.input_devices = list_input_devices();
        }
        if !ready.output_devices.is_empty() {
            self.state.output_devices = ready.output_devices;
        } else if self.state.output_devices.is_empty() {
            self.state.output_devices = list_output_devices();
        }
        self.state.current_input_device = ready.current_input_device;
        self.state.current_output_device = ready.current_output_device;
        self.state.running = true;
        self.state.connected = self.bridge.is_connected();
        self.state.status = "Running".to_owned();
        self.state.error = bridge_error;

        for (name, value) in &self.preserved_params {
            self.bridge
                .send_command_notification("setParam", &json!({ "name": name, "value": value }));
        }
        for (name, path) in &self.preserved_buffers {
            self.bridge
                .send_command("bindBufferWav", &json!({ "name": name, "path": path }));
        }

        self.scope_polling_active = true;
        self.scope_polling_in_flight = false;
        self.last_scope_poll = Instant::now();
    }

    fn handle_child_exited(&mut self, code: Option<i32>, error: Option<String>) {
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.bridge.disconnect();
        self.state.running = false;
        self.state.connected = false;
        self.state.status = "Stopped".to_owned();
        self.state.error =
            error.or_else(|| code.filter(|c| *c != 0).map(|c| format!("exit code {c}")));
        self.state.output_channels = 0;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();
    }

    fn handle_tcp_response(&mut self, line: &str) -> bool {
        if let Ok(resp) = serde_json::from_str::<Value>(line) {
            if let Some(result) = resp.get("result") {
                let channels = result.get("channels").and_then(Value::as_u64);
                let samples = result.get("samples").and_then(Value::as_array);
                if let (Some(channels), Some(samples)) = (channels, samples) {
                    self.scope_polling_in_flight = false;
                    self.state.scope_channels = channels as usize;
                    self.state.scope_samples = samples
                        .iter()
                        .filter_map(Value::as_f64)
                        .map(|value| value as f32)
                        .collect();
                    return true;
                }
            }
        }
        false
    }
}

fn source_differs_from_cached(
    current: Option<&[u8]>,
    compiled: Option<&[u8]>,
    pending: Option<&[u8]>,
) -> bool {
    current != compiled && pending.is_none_or(|pending| current != Some(pending))
}

impl Drop for RunController {
    fn drop(&mut self) {
        self.child.kill();
        self.bridge.disconnect();
    }
}

struct ChildSession {
    child: Option<Child>,
    stderr_buffer: Arc<Mutex<String>>,
}

fn format_run_build_error(prefix: &str, err: &RunBuildError) -> String {
    match err {
        RunBuildError::Diagnostics(diags) => {
            if let Some(first) = diags.first() {
                format_single_diagnostic(prefix, first)
            } else {
                format!("{prefix}: run diagnostics")
            }
        }
        RunBuildError::Runtime(diag) => format_single_diagnostic(prefix, diag),
    }
}

fn format_single_diagnostic(prefix: &str, diag: &Diagnostic) -> String {
    let location = if let Some(file) = diag.file.as_ref().filter(|file: &&String| !file.is_empty())
    {
        format!("{}:{}:{}", file, diag.line, diag.column)
    } else {
        format!("{}:{}", diag.line, diag.column)
    };
    format!("{prefix}: {} ({location})", diag.message)
}

fn run_param_json(param: &RunParamInfo) -> Value {
    json!({
        "index": param.index,
        "name": param.name,
        "type": param.type_repr,
        "value": param.value,
        "default": param.default,
        "rangeMin": param.range_min,
        "rangeMax": param.range_max,
        "scalar": param.scalar,
    })
}

fn run_buffer_json(buffer: &onda_daemon::RunBufferInfo) -> Value {
    let (channels_kind, channels_static) = match buffer.channels {
        DaemonRunBufferChannels::Mono => ("mono", None),
        DaemonRunBufferChannels::Static(channels) => ("static", Some(channels)),
        DaemonRunBufferChannels::Dynamic => ("dynamic", None),
    };
    json!({
        "index": buffer.index,
        "name": buffer.name,
        "type": buffer.type_repr,
        "channelsKind": channels_kind,
        "channelsStatic": channels_static,
        "loadedPath": buffer.loaded_path,
    })
}

fn run_event_json(event: &RunEventInfo) -> Value {
    json!({
        "index": event.index,
        "name": event.name,
        "args": event.params.iter().map(run_event_param_json).collect::<Vec<_>>(),
    })
}

fn run_event_param_json(param: &RunEventParamInfo) -> Value {
    json!({
        "index": param.index,
        "name": param.name,
        "type": param.type_repr,
        "default": run_event_value_json(&param.value),
        "value": run_event_value_json(&param.value),
    })
}

fn run_event_value_json(value: &RunEventValue) -> Value {
    match value {
        RunEventValue::Bool(value) => Value::Bool(*value),
        RunEventValue::Number(value) => json!(value),
    }
}

impl ChildSession {
    fn is_active(&self) -> bool {
        self.child.is_some()
    }

    fn spawn(
        onda_path: &Path,
        options: &RunHostOptions,
        event_tx: Sender<ControllerEvent>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(&options.onda_bin);
        cmd.arg("run")
            .arg("play")
            .arg(onda_path)
            .arg("--forever")
            .arg("--control-json")
            .arg("--sample-rate")
            .arg(options.sample_rate_hz.to_string())
            .arg("--block-size")
            .arg(options.block_frames.to_string())
            .arg("--opt-level")
            .arg(options.opt_level.as_str());

        if let Some(input_device) = &options.input_device {
            cmd.arg("--input-device").arg(input_device);
        }
        if let Some(output_device) = &options.output_device {
            cmd.arg("--output-device").arg(output_device);
        }
        if options.fast_math {
            cmd.arg("--fast-math");
        }
        if options.show_meta {
            cmd.arg("--meta");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn onda run: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture subprocess stdout".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "failed to capture subprocess stderr".to_owned())?;
        let stderr_buffer = Arc::new(Mutex::new(String::new()));

        let stderr_sink = Arc::clone(&stderr_buffer);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        eprintln!("[onda run] {line}");
                        if let Ok(mut slot) = stderr_sink.lock() {
                            if !slot.is_empty() {
                                slot.push('\n');
                            }
                            slot.push_str(&line);
                            if slot.len() > 4000 {
                                let keep_from = slot.len().saturating_sub(4000);
                                *slot = slot[keep_from..].to_owned();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(_) => break,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(raw) = serde_json::from_str::<RawReadyEvent>(trimmed) {
                    if raw.event == "ready" {
                        let ready = ReadyEvent {
                            path: raw.path.unwrap_or_default(),
                            port: raw.port.unwrap_or(0),
                            params: raw.params.unwrap_or_default(),
                            buffers: raw.buffers.unwrap_or_default(),
                            events: raw.events.unwrap_or_default(),
                            output_channels: raw.output_channels.unwrap_or(0),
                            input_devices: raw.input_devices.unwrap_or_default(),
                            output_devices: raw.output_devices.unwrap_or_default(),
                            current_input_device: raw.current_input_device,
                            current_output_device: raw.current_output_device,
                        };
                        let _ = event_tx.send(ControllerEvent::ChildReady(ready));
                        continue;
                    }
                }
                eprintln!("[onda run stdout] {trimmed}");
            }
        });

        Ok(Self {
            child: Some(child),
            stderr_buffer,
        })
    }

    fn try_take_exit(&mut self) -> Option<(Option<i32>, Option<String>)> {
        let child = self.child.as_mut()?;
        let status = child.try_wait().ok()??;
        self.child = None;
        let error = self.stderr_buffer.lock().ok().and_then(|slot| {
            let trimmed = slot.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        });
        Some((status.code(), error))
    }

    fn kill(&mut self) {
        if let Some(ref mut child) = self.child {
            terminate_run_child(child);
            let _ = child.wait();
        }
        self.child = None;
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(target_os = "windows")]
fn terminate_run_child(child: &mut Child) {
    let pid = child.id().to_string();
    let mut cmd = Command::new("taskkill");
    cmd.arg("/PID").arg(pid).arg("/T").arg("/F");
    cmd.creation_flags(CREATE_NO_WINDOW);
    if cmd.status().map(|status| status.success()).unwrap_or(false) {
        return;
    }
    let _ = child.kill();
}

#[cfg(not(target_os = "windows"))]
fn terminate_run_child(child: &mut Child) {
    let _ = child.kill();
}

#[derive(Clone)]
struct IpcBridge {
    writer: Arc<Mutex<Option<BufWriter<TcpStream>>>>,
    request_id: Arc<AtomicU64>,
}

impl IpcBridge {
    fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(None)),
            request_id: Arc::new(AtomicU64::new(0)),
        }
    }

    fn connect(&self, port: u16, event_tx: Sender<ControllerEvent>) -> Result<(), String> {
        self.disconnect();

        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("TCP connect failed: {e}"))?;
        let reader_stream = stream
            .try_clone()
            .map_err(|e| format!("TCP clone failed: {e}"))?;

        if let Ok(mut writer) = self.writer.lock() {
            *writer = Some(BufWriter::new(stream));
        }

        thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let trimmed = line.trim().to_owned();
                        if !trimmed.is_empty() {
                            let _ = event_tx.send(ControllerEvent::TcpResponse(trimmed));
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(())
    }

    fn disconnect(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            if let Some(writer) = writer.as_mut() {
                let _ = writer.get_ref().shutdown(std::net::Shutdown::Both);
            }
            *writer = None;
        }
    }

    fn send_command(&self, command: &str, payload: &Value) {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        self.send_command_inner(Some(id), command, payload);
    }

    fn send_command_notification(&self, command: &str, payload: &Value) {
        self.send_command_inner(None, command, payload);
    }

    fn send_command_inner(&self, id: Option<u64>, command: &str, payload: &Value) {
        let mut request = json!({ "command": command });
        if let Some(id) = id {
            if let Value::Object(ref mut req_map) = request {
                req_map.insert("id".to_owned(), Value::from(id));
            }
        }

        if let Value::Object(map) = payload {
            if let Value::Object(ref mut req_map) = request {
                for (key, value) in map {
                    req_map.insert(key.clone(), value.clone());
                }
            }
        }

        if let Ok(mut writer) = self.writer.lock() {
            let Some(writer) = writer.as_mut() else {
                return;
            };
            let line = serde_json::to_string(&request).unwrap_or_default();
            let _ = writer.write_all(line.as_bytes());
            let _ = writer.write_all(b"\n");
            let _ = writer.flush();
        }
    }

    fn is_connected(&self) -> bool {
        self.writer
            .lock()
            .map(|writer| writer.is_some())
            .unwrap_or(false)
    }
}

struct FileWatcher {
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

impl FileWatcher {
    fn watch(path: &Path, on_change: impl Fn() + Send + 'static) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_millis(200), tx).ok()?;
        let watch_root = path.parent().unwrap_or_else(|| Path::new("."));
        debouncer
            .watcher()
            .watch(watch_root, notify::RecursiveMode::NonRecursive)
            .ok()?;
        let watched_path = path.to_path_buf();
        let mut last_stamp = file_stamp(&watched_path);

        thread::spawn(move || {
            while let Ok(Ok(events)) = rx.recv() {
                if !events
                    .iter()
                    .any(|event| event.kind == DebouncedEventKind::Any)
                {
                    continue;
                }
                let next_stamp = file_stamp(&watched_path);
                if next_stamp != last_stamp {
                    last_stamp = next_stamp;
                    on_change();
                }
            }
        });

        Some(Self {
            _debouncer: debouncer,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    exists: bool,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn file_stamp(path: &Path) -> FileStamp {
    match fs::metadata(path) {
        Ok(metadata) => FileStamp {
            exists: true,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        },
        Err(_) => FileStamp {
            exists: false,
            len: 0,
            modified: None,
        },
    }
}

fn list_input_devices() -> Vec<String> {
    onda_cpal::input_audio_devices()
}

fn list_output_devices() -> Vec<String> {
    onda_cpal::output_audio_devices()
}

fn validated_device(name: Option<&str>, devices: &[String]) -> Result<Option<String>, String> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    if devices.iter().any(|device| device == name) {
        Ok(Some(name.to_owned()))
    } else {
        Err(format!("unknown device '{name}'"))
    }
}

fn apply_preserved_param_state(params: &mut [Value], preserved_params: &[(String, Value)]) {
    for param in params {
        let Some(name) = param_name(param).map(str::to_owned) else {
            continue;
        };
        if let Some((_, value)) = preserved_params
            .iter()
            .find(|(param_name, _)| param_name == &name)
        {
            set_param_value(param, value.clone());
        }
    }
}

fn reconcile_preserved_params(
    preserved_params: &mut Vec<(String, Value)>,
    old_params: &[Value],
    new_params: &[Value],
) {
    preserved_params.retain(|(name, _)| {
        let Some(old_param) = old_params
            .iter()
            .find(|param| param_name(param) == Some(name))
        else {
            return false;
        };
        let Some(new_param) = new_params
            .iter()
            .find(|param| param_name(param) == Some(name))
        else {
            return false;
        };
        params_are_compatible_for_preservation(old_param, new_param)
    });
}

fn params_are_compatible_for_preservation(old_param: &Value, new_param: &Value) -> bool {
    param_name(old_param) == param_name(new_param)
        && old_param.get("type") == new_param.get("type")
        && old_param.get("default") == new_param.get("default")
        && old_param.get("rangeMin") == new_param.get("rangeMin")
        && old_param.get("rangeMax") == new_param.get("rangeMax")
        && old_param.get("scalar") == new_param.get("scalar")
}

fn apply_preserved_buffer_state(buffers: &mut [Value], preserved_buffers: &[(String, String)]) {
    for buffer in buffers {
        let Some(name) = buffer_name(buffer).map(str::to_owned) else {
            continue;
        };
        let path = preserved_buffers
            .iter()
            .find(|(buffer_name, _)| buffer_name == &name)
            .map(|(_, path)| path.as_str());
        update_buffer_loaded_path(std::slice::from_mut(buffer), &name, path);
    }
}

fn apply_preserved_event_state(events: &mut [Value], preserved_events: &[(String, Vec<Value>)]) {
    for event in events {
        let Some(name) = event_name(event).map(str::to_owned) else {
            continue;
        };
        if let Some((_, values)) = preserved_events
            .iter()
            .find(|(event_name, _)| event_name == &name)
        {
            set_event_values(event, values);
        }
    }
}

fn update_param_value(params: &mut [Value], name: &str, value: Value) {
    for param in params {
        if param_name(param) == Some(name) {
            set_param_value(param, value.clone());
            break;
        }
    }
}

fn update_event_values(events: &mut [Value], name: &str, values: &[Value]) {
    for event in events {
        if event_name(event) == Some(name) {
            set_event_values(event, values);
            break;
        }
    }
}

fn update_buffer_loaded_path(buffers: &mut [Value], name: &str, loaded_path: Option<&str>) {
    for buffer in buffers {
        if buffer_name(buffer) != Some(name) {
            continue;
        }
        if let Some(obj) = buffer.as_object_mut() {
            obj.insert(
                "loadedPath".to_owned(),
                loaded_path.map_or(Value::Null, |path| Value::String(path.to_owned())),
            );
        }
        break;
    }
}

fn param_name(param: &Value) -> Option<&str> {
    param.get("name").and_then(Value::as_str)
}

fn buffer_name(buffer: &Value) -> Option<&str> {
    buffer.get("name").and_then(Value::as_str)
}

fn event_name(event: &Value) -> Option<&str> {
    event.get("name").and_then(Value::as_str)
}

fn set_param_value(param: &mut Value, value: Value) {
    if let Some(obj) = param.as_object_mut() {
        obj.insert("value".to_owned(), value);
    }
}

fn set_event_values(event: &mut Value, values: &[Value]) {
    let Some(args) = event.get_mut("args").and_then(Value::as_array_mut) else {
        return;
    };
    for (arg, value) in args.iter_mut().zip(values.iter()) {
        if let Some(obj) = arg.as_object_mut() {
            obj.insert("value".to_owned(), value.clone());
        }
    }
}

fn reset_event_values(events: &mut [Value]) {
    for event in events {
        let Some(args) = event.get_mut("args").and_then(Value::as_array_mut) else {
            continue;
        };
        for arg in args {
            let Some(default_value) = event_arg_default_value(arg) else {
                continue;
            };
            if let Some(obj) = arg.as_object_mut() {
                obj.insert("value".to_owned(), default_value);
            }
        }
    }
}

fn param_default_value(param: &Value) -> Option<Value> {
    if let Some(default) = param.get("default").filter(|value| !value.is_null()) {
        return Some(default.clone());
    }
    if let Some(range_min) = param.get("rangeMin").filter(|value| !value.is_null()) {
        return Some(range_min.clone());
    }
    Some(match param.get("type").and_then(Value::as_str) {
        Some("bool") => Value::Bool(false),
        _ => Value::Number(serde_json::Number::from(0)),
    })
}

fn event_arg_default_value(arg: &Value) -> Option<Value> {
    if let Some(default) = arg.get("default").filter(|value| !value.is_null()) {
        return Some(default.clone());
    }
    Some(match arg.get("type").and_then(Value::as_str) {
        Some("bool") => Value::Bool(false),
        _ => Value::Number(serde_json::Number::from(0)),
    })
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        params_are_compatible_for_preservation, reconcile_preserved_params,
        source_differs_from_cached, FileWatcher,
    };
    use serde_json::{json, Value};
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn file_watcher_survives_repeated_atomic_replaces() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let watched = temp_root.join("run.onda");
        fs::write(&watched, "outs:\n  out1\nsample:\n  out1 = 0.0\n").expect("write initial file");

        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::watch(&watched, move || {
            let _ = tx.send(());
        })
        .expect("watcher should start");

        replace_file(&watched, "outs:\n  out1\nsample:\n  out1 = 1.0\n");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("first replace should trigger");

        replace_file(&watched, "outs:\n  out1\nsample:\n  out1 = 2.0\n");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("second replace should trigger");

        let _ = fs::remove_dir_all(temp_root);
    }

    fn replace_file(path: &Path, contents: &str) {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, contents).expect("write temp file");
        #[cfg(target_os = "windows")]
        if path.exists() {
            fs::remove_file(path).expect("remove old file");
        }
        fs::rename(&tmp, path).expect("replace watched file");
    }

    #[test]
    fn preserved_params_are_dropped_when_param_signature_changes() {
        let old_params = vec![run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), true)];
        let new_params = vec![run_param("gain", "f32", 0.5, Some(0.0), Some(2.0), true)];
        let mut preserved = vec![("gain".to_owned(), json!(1.25))];

        reconcile_preserved_params(&mut preserved, &old_params, &new_params);

        assert!(
            preserved.is_empty(),
            "changed default should reset preserved param"
        );
    }

    #[test]
    fn source_cache_only_invalidates_for_different_contents() {
        assert!(!source_differs_from_cached(
            Some(b"same"),
            Some(b"same"),
            None,
        ));
        assert!(!source_differs_from_cached(
            Some(b"pending"),
            Some(b"old"),
            Some(b"pending"),
        ));
        assert!(source_differs_from_cached(
            Some(b"changed"),
            Some(b"old"),
            None,
        ));
        assert!(source_differs_from_cached(None, Some(b"old"), None));
    }

    #[test]
    fn preserved_params_are_kept_when_param_signature_matches() {
        let old_params = vec![run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), true)];
        let new_params = vec![run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), true)];
        let mut preserved = vec![("gain".to_owned(), json!(1.25))];

        reconcile_preserved_params(&mut preserved, &old_params, &new_params);

        assert_eq!(
            preserved.len(),
            1,
            "unchanged param should keep preserved value"
        );
    }

    #[test]
    fn param_preservation_compatibility_checks_type_default_range_and_shape() {
        let base = run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), true);
        assert!(params_are_compatible_for_preservation(
            &base,
            &run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), true)
        ));
        assert!(!params_are_compatible_for_preservation(
            &base,
            &run_param("gain", "f64", 1.0, Some(0.0), Some(2.0), true)
        ));
        assert!(!params_are_compatible_for_preservation(
            &base,
            &run_param("gain", "f32", 0.5, Some(0.0), Some(2.0), true)
        ));
        assert!(!params_are_compatible_for_preservation(
            &base,
            &run_param("gain", "f32", 1.0, Some(-1.0), Some(2.0), true)
        ));
        assert!(!params_are_compatible_for_preservation(
            &base,
            &run_param("gain", "f32", 1.0, Some(0.0), Some(2.0), false)
        ));
    }

    fn run_param(
        name: &str,
        ty: &str,
        default: f64,
        range_min: Option<f64>,
        range_max: Option<f64>,
        scalar: bool,
    ) -> Value {
        json!({
            "name": name,
            "type": ty,
            "value": default,
            "default": default,
            "rangeMin": range_min,
            "rangeMax": range_max,
            "scalar": scalar,
        })
    }
}
