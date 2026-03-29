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

use cpal::traits::{DeviceTrait, HostTrait};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use serde::Deserialize;
use serde_json::{json, Value};

const SCOPE_MAX_FRAMES: usize = 1024;
const SCOPE_POLL_INTERVAL_MS: u64 = 50;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct PreviewHostOptions {
    pub sample_rate_hz: u32,
    pub block_frames: usize,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub fast_math: bool,
    pub omni_bin: String,
}

impl Default for PreviewHostOptions {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            block_frames: 128,
            input_device: None,
            output_device: None,
            fast_math: false,
            omni_bin: "omni".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreviewState {
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

impl PreviewState {
    fn new(path: &Path, options: &PreviewHostOptions) -> Self {
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
    (list_input_devices(), list_output_devices())
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

pub struct PreviewController {
    omni_path: PathBuf,
    options: PreviewHostOptions,
    state: PreviewState,
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
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PollResult {
    pub state_changed: bool,
    pub scope_changed: bool,
}

impl PreviewController {
    pub fn new(omni_path: &Path, options: PreviewHostOptions) -> Result<Self, String> {
        let omni_path = std::fs::canonicalize(omni_path)
            .map_err(|e| format!("cannot resolve path {}: {e}", omni_path.display()))?;
        let (events_tx, events_rx) = mpsc::channel();
        let bridge = IpcBridge::new();
        let state = PreviewState::new(&omni_path, &options);
        let child = ChildSession::spawn(&omni_path, &options, events_tx.clone())
            .map_err(|e| format!("failed to start preview subprocess: {e}"))?;
        let watcher_tx = events_tx.clone();
        let watcher = FileWatcher::watch(&omni_path, move || {
            let _ = watcher_tx.send(ControllerEvent::FileChanged);
        });

        let mut controller = Self {
            omni_path,
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
        };
        controller.state.running = true;
        controller.state.status = "Starting...".to_owned();
        Ok(controller)
    }

    pub fn state(&self) -> &PreviewState {
        &self.state
    }

    pub fn path(&self) -> &Path {
        &self.omni_path
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
                    let _ = self.restart_with_status("Restarting...");
                    result.state_changed = true;
                }
            }
        }

        result
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.restart_with_status("Starting...")
    }

    pub fn stop(&mut self) {
        self.child.kill();
        self.bridge.disconnect();
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.state.running = false;
        self.state.connected = false;
        self.state.status = "Stopped".to_owned();
        self.state.error = None;
        self.state.output_channels = 0;
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

        match ChildSession::spawn(&self.omni_path, &self.options, self.events_tx.clone()) {
            Ok(child) => {
                self.child = child;
                self.state.running = true;
                Ok(())
            }
            Err(e) => {
                self.state.status = "Failed to start".to_owned();
                self.state.error = Some(e.clone());
                Err(e)
            }
        }
    }

    fn handle_child_ready(&mut self, ready: ReadyEvent) {
        let bridge_error = self
            .bridge
            .connect(ready.port, self.events_tx.clone())
            .err();

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

impl Drop for PreviewController {
    fn drop(&mut self) {
        self.child.kill();
        self.bridge.disconnect();
    }
}

struct ChildSession {
    child: Option<Child>,
    stderr_buffer: Arc<Mutex<String>>,
}

impl ChildSession {
    fn spawn(
        omni_path: &Path,
        options: &PreviewHostOptions,
        event_tx: Sender<ControllerEvent>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(&options.omni_bin);
        cmd.arg("preview")
            .arg("play")
            .arg(omni_path)
            .arg("--forever")
            .arg("--control-json")
            .arg("--sample-rate")
            .arg(options.sample_rate_hz.to_string())
            .arg("--block")
            .arg(options.block_frames.to_string());

        if let Some(input_device) = &options.input_device {
            cmd.arg("--input-device").arg(input_device);
        }
        if let Some(output_device) = &options.output_device {
            cmd.arg("--output-device").arg(output_device);
        }
        if options.fast_math {
            cmd.arg("--fast-math");
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn omni preview: {e}"))?;

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
                        eprintln!("[omni preview] {line}");
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
                eprintln!("[omni preview stdout] {trimmed}");
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
            let _ = child.kill();
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
        debouncer
            .watcher()
            .watch(path, notify::RecursiveMode::NonRecursive)
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
    enumerate_devices_silenced(|host| {
        host.input_devices()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|device| device.name().ok())
            .collect()
    })
}

fn list_output_devices() -> Vec<String> {
    enumerate_devices_silenced(|host| {
        host.output_devices()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|device| device.name().ok())
            .collect()
    })
}

fn enumerate_devices_silenced<T>(f: impl FnOnce(&cpal::Host) -> T) -> T {
    #[cfg(target_os = "linux")]
    {
        static STDERR_GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = STDERR_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("stderr guard mutex should not be poisoned");
        let host = cpal::default_host();
        with_stderr_silenced(|| f(&host))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let host = cpal::default_host();
        f(&host)
    }
}

#[cfg(target_os = "linux")]
fn with_stderr_silenced<T>(f: impl FnOnce() -> T) -> T {
    let stderr = std::io::stderr();
    let stderr_fd = stderr.as_raw_fd();
    let null = match std::fs::OpenOptions::new().write(true).open("/dev/null") {
        Ok(file) => file,
        Err(_) => return f(),
    };
    let null_fd = null.as_raw_fd();

    // SAFETY:
    // We temporarily redirect the process stderr to /dev/null while CPAL/ALSA/JACK
    // enumerate devices because some backends print directly to stderr. Access is
    // serialized with a global mutex and the original fd is restored afterwards.
    unsafe {
        let saved = libc::dup(stderr_fd);
        if saved < 0 {
            return f();
        }
        if libc::dup2(null_fd, stderr_fd) < 0 {
            let _ = libc::close(saved);
            return f();
        }
        let result = f();
        let _ = libc::dup2(saved, stderr_fd);
        let _ = libc::close(saved);
        result
    }
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
    path.display().to_string()
}
