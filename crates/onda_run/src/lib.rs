//! Shared native run controller for the egui and webview frontends.
//!
//! The controller owns source/project filesystem watching, targeted snapshot
//! invalidation, and disk-validation fallback when watcher coverage is partial.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub use onda_codegen_llvm::{ParamDomain, ParamScalarType, ParamScale};
use onda_daemon::{
    RunBufferChannels as DaemonRunBufferChannels, RunBuildError, RunEventInfo, RunEventParamInfo,
    RunEventValue, RunParamInfo,
};
use onda_frontend::{load_program_file, Diagnostic};
use onda_host_protocol::{event_by_name, signature_matches, HostEventFamily};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod midi;
mod playback;
mod project_io;

pub use playback::{
    append_interleaved_block, format_run_param_info, play_run_realtime, PlaybackLaunch,
    ProjectBufferBinding,
};
pub use project_io::{
    create_empty_project, package_project_from_files, package_project_plan,
    project_buffer_declarations, publish_project_plan,
};

const SCOPE_MAX_FRAMES: usize = 1024;
const MAX_VISIBLE_LOG_ENTRIES: usize = 4096;
const SOURCE_WATCH_DEBOUNCE: Duration = Duration::from_millis(200);
const SOURCE_WATCH_FALLBACK_INTERVAL: Duration = Duration::from_millis(500);
/// Periodic controller polling interval used while a native run frontend is loaded.
pub const CONTROLLER_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const DEFAULT_REALTIME_BLOCK_FRAMES: usize = 256;
pub const COMPUTER_KEYBOARD_MIDI_INPUT: &str = "Computer Keyboard";
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
    pub midi_input_device: Option<String>,
    pub fast_math: bool,
    pub show_meta: bool,
    pub theme: RunThemeMode,
    pub onda_bin: String,
}

impl Default for RunHostOptions {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            block_frames: DEFAULT_REALTIME_BLOCK_FRAMES,
            opt_level: "3".to_owned(),
            input_device: None,
            output_device: None,
            midi_input_device: None,
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
    pub midi: RunMidiCapabilities,
    pub delegates: Vec<Value>,
    pub log_text: String,
    pub log_entries: Vec<Value>,
    pub log_revealed: bool,
    pub print_overflow_count: u64,
    pub print_transport_drop_count: u64,
    pub delegate_overflow_count: u64,
    pub delegate_transport_drop_count: u64,
    pub params: Vec<Value>,
    pub input_devices: Vec<String>,
    pub output_devices: Vec<String>,
    pub midi_input_devices: Vec<String>,
    pub current_input_device: Option<String>,
    pub current_output_device: Option<String>,
    pub current_midi_input_device: Option<String>,
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
            midi: RunMidiCapabilities::default(),
            delegates: Vec::new(),
            log_text: String::new(),
            log_entries: Vec::new(),
            log_revealed: false,
            print_overflow_count: 0,
            print_transport_drop_count: 0,
            delegate_overflow_count: 0,
            delegate_transport_drop_count: 0,
            params: Vec::new(),
            input_devices: list_input_devices(),
            output_devices: list_output_devices(),
            midi_input_devices: list_midi_input_devices(),
            current_input_device: options.input_device.clone(),
            current_output_device: options.output_device.clone(),
            current_midi_input_device: options.midi_input_device.clone(),
            scope_channels: 0,
            scope_samples: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMidiCapabilities {
    pub available: bool,
    pub note_on: bool,
    pub note_off: bool,
}

pub fn available_audio_devices() -> (Vec<String>, Vec<String>) {
    onda_cpal::available_audio_devices()
}

#[derive(Debug)]
enum ControllerEvent {
    ChildReady {
        generation: u64,
        ready: Box<ReadyEvent>,
    },
    TcpResponse {
        generation: u64,
        line: String,
    },
    SourcesMayHaveChanged {
        paths: Vec<PathBuf>,
        watch_batch: SourceWatchBatch,
    },
}

impl ControllerEvent {
    fn is_current_for(&self, child_generation: u64) -> bool {
        let event_generation = match self {
            Self::ChildReady { generation, .. } | Self::TcpResponse { generation, .. } => {
                Some(*generation)
            }
            Self::SourcesMayHaveChanged { .. } => None,
        };
        event_generation.is_none_or(|generation| generation == child_generation)
    }
}

#[derive(Debug)]
enum PendingCommand {
    BindBuffer { name: String, path: String },
    ClearBuffer { name: String },
    Play,
    ResetParams,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum PreservedBufferBinding {
    File(String),
    Cleared,
}

impl PreservedBufferBinding {
    fn replay_request(&self, name: &str) -> (&'static str, Value, PendingCommand) {
        match self {
            Self::File(path) => (
                "bindBufferWav",
                json!({ "name": name, "path": path }),
                PendingCommand::BindBuffer {
                    name: name.to_owned(),
                    path: path.clone(),
                },
            ),
            Self::Cleared => (
                "clearBuffer",
                json!({ "name": name }),
                PendingCommand::ClearBuffer {
                    name: name.to_owned(),
                },
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct ReadyEvent {
    path: String,
    port: u16,
    params: Vec<Value>,
    buffers: Vec<Value>,
    events: Vec<Value>,
    midi: RunMidiCapabilities,
    delegates: Vec<Value>,
    output_channels: usize,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    midi_input_devices: Vec<String>,
    current_input_device: Option<String>,
    current_output_device: Option<String>,
    current_midi_input_device: Option<String>,
}

#[derive(Deserialize)]
struct RawReadyEvent {
    event: String,
    path: Option<String>,
    port: Option<u16>,
    params: Option<Vec<RunParamWire>>,
    buffers: Option<Vec<Value>>,
    events: Option<Vec<Value>>,
    #[serde(default)]
    midi: RunMidiCapabilities,
    delegates: Option<Vec<Value>>,
    #[serde(rename = "outputChannels")]
    output_channels: Option<usize>,
    #[serde(rename = "inputDevices")]
    input_devices: Option<Vec<String>>,
    #[serde(rename = "outputDevices")]
    output_devices: Option<Vec<String>>,
    #[serde(rename = "midiInputDevices")]
    midi_input_devices: Option<Vec<String>>,
    #[serde(rename = "currentInputDevice")]
    current_input_device: Option<String>,
    #[serde(rename = "currentOutputDevice")]
    current_output_device: Option<String>,
    #[serde(rename = "currentMidiInputDevice")]
    current_midi_input_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunParamWire {
    index: usize,
    name: String,
    #[serde(rename = "type")]
    type_repr: String,
    value_repr: Option<String>,
    default_repr: Option<String>,
    range_min_repr: Option<String>,
    range_max_repr: Option<String>,
    scale: Option<String>,
    curve_repr: Option<String>,
    unit: Option<String>,
    step_repr: Option<String>,
    step_count: Option<u32>,
    scalar: bool,
}

pub struct RunController {
    launch_path: PathBuf,
    onda_path: PathBuf,
    project_watch_paths: Option<onda_project::ProjectWatchPaths>,
    options: RunHostOptions,
    state: RunState,
    events_rx: Receiver<ControllerEvent>,
    events_tx: Sender<ControllerEvent>,
    bridge: IpcBridge,
    child: ChildSession,
    child_generation: u64,
    watcher: Option<FileWatcher>,
    watched_sources: Vec<PathBuf>,
    watched_paths: Vec<PathBuf>,
    watched_roots: HashMap<PathBuf, notify::RecursiveMode>,
    source_watch_revision: SourceWatchRevision,
    compiled_watch_revision: Option<u64>,
    last_source_fallback_validation: Instant,
    preserved_params: Vec<(String, Value)>,
    preserved_buffers: BTreeMap<String, PreservedBufferBinding>,
    preserved_events: Vec<(String, Vec<Value>)>,
    pending_commands: HashMap<u64, PendingCommand>,
    processing_requested: bool,
    scope_polling_active: bool,
    scope_polling_in_flight: bool,
    last_scope_poll: Instant,
    source_compilation: SourceCompilationState,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PollResult {
    pub state_changed: bool,
    pub scope_changed: bool,
}

impl RunController {
    pub fn new(onda_path: &Path, mut options: RunHostOptions) -> Result<Self, String> {
        if options.midi_input_device.is_none() {
            options.midi_input_device = Some(COMPUTER_KEYBOARD_MIDI_INPUT.to_owned());
        }
        onda_frontend::ensure_no_symlink_components(onda_path).map_err(|error| {
            format!("cannot watch Onda input '{}': {error}", onda_path.display())
        })?;
        let input_path = project_io::absolute_lexical_path(onda_path)?;
        let launch_path = std::fs::canonicalize(&input_path)
            .map_err(|e| format!("cannot resolve path {}: {e}", onda_path.display()))?;
        let (onda_path, project_watch_paths) = resolve_run_input_paths(&launch_path)?;
        let (events_tx, events_rx) = mpsc::channel();
        let bridge = IpcBridge::new();
        let state = RunState::new(&launch_path, &options);
        let source_watch_revision = SourceWatchRevision::default();
        let compiled_watch_revision = source_watch_revision.current();
        let pending_sources = source_snapshot_from_paths(&onda_path, &[]);
        let pending_sources = pending_sources.with_project(project_watch_paths.as_ref());
        let watched_sources = pending_sources.paths();
        let watched_paths = pending_sources.watch_paths();
        let (watcher, watched_roots) = start_source_watcher(
            &watched_paths,
            source_watch_revision.clone(),
            events_tx.clone(),
        );
        if !pending_sources.matches_disk_changes(&watched_paths) {
            source_watch_revision.observe_change();
        }
        let child_generation = 1;
        let child =
            ChildSession::spawn(&launch_path, &options, child_generation, events_tx.clone())
                .map_err(|e| format!("failed to start run subprocess: {e}"))?;

        let mut controller = Self {
            launch_path,
            onda_path,
            project_watch_paths,
            options,
            state,
            events_rx,
            events_tx,
            bridge,
            child,
            child_generation,
            watcher,
            watched_sources,
            watched_paths,
            watched_roots,
            source_watch_revision,
            compiled_watch_revision: Some(compiled_watch_revision),
            last_source_fallback_validation: Instant::now(),
            preserved_params: Vec::new(),
            preserved_buffers: BTreeMap::new(),
            preserved_events: Vec::new(),
            pending_commands: HashMap::new(),
            processing_requested: true,
            scope_polling_active: false,
            scope_polling_in_flight: false,
            last_scope_poll: Instant::now(),
            source_compilation: SourceCompilationState::Compiling(pending_sources),
        };
        controller.state.running = true;
        controller.state.status = "Starting...".to_owned();
        Ok(controller)
    }

    pub fn state(&self) -> &RunState {
        &self.state
    }

    pub fn options(&self) -> &RunHostOptions {
        &self.options
    }

    pub fn path(&self) -> &Path {
        &self.launch_path
    }

    pub fn save_as_project(&self, destination: &Path) -> Result<(), String> {
        let input = onda_project::resolve_project_input(
            &self.launch_path,
            onda_project::ProjectLimits::default(),
        )
        .map_err(|error| error.to_string())?;
        let source_root = input.project().map(|project| project.root.as_path());
        let constants = input
            .project()
            .map(|project| project.manifest.constants.clone())
            .unwrap_or_default();
        let mut assets = std::collections::BTreeMap::new();
        let mut asset_file_names = std::collections::BTreeMap::new();
        if let Some(project) = input.project() {
            for (name, (asset, path)) in project
                .load_buffer_assets(onda_project::ProjectLimits::default())
                .map_err(|error| error.to_string())?
            {
                if let Some(file_name) = path
                    .as_deref()
                    .and_then(Path::file_name)
                    .and_then(|file_name| file_name.to_str())
                {
                    asset_file_names.insert(name.clone(), file_name.to_owned());
                }
                assets.insert(name, asset);
            }
        }

        for buffer in &self.state.buffers {
            let Some(name) = buffer.get("name").and_then(Value::as_str) else {
                continue;
            };
            if buffer.get("loadedFrames").and_then(Value::as_u64).is_none() {
                assets.remove(name);
                asset_file_names.remove(name);
                continue;
            }
            if let Some(path) = buffer.get("loadedPath").and_then(Value::as_str) {
                let asset =
                    onda_project::load_buffer_file(path, onda_project::ProjectLimits::default())
                        .map_err(|error| {
                            format!("failed to capture buffer '{name}' from '{path}': {error}")
                        })?;
                assets.insert(name.to_owned(), asset);
                if let Some(file_name) = Path::new(path)
                    .file_name()
                    .and_then(|file_name| file_name.to_str())
                {
                    asset_file_names.insert(name.to_owned(), file_name.to_owned());
                }
            }
        }

        let project_file_name = project_io::project_file_name_from_target(destination)?;
        let plan = project_io::package_project_plan(
            &self.onda_path,
            source_root,
            constants,
            assets,
            &project_file_name,
            &asset_file_names,
        )?;
        project_io::publish_project_plan(destination, &plan)
    }

    pub fn poll(&mut self) -> PollResult {
        let mut result = PollResult::default();

        if self.scope_polling_active
            && !self.scope_polling_in_flight
            && self.last_scope_poll.elapsed() >= CONTROLLER_POLL_INTERVAL
        {
            let _ = self
                .bridge
                .send_command("getScopeData", &json!({ "maxFrames": SCOPE_MAX_FRAMES }));
            self.scope_polling_in_flight = true;
            self.last_scope_poll = Instant::now();
        }

        while let Ok(event) = self.events_rx.try_recv() {
            if !event.is_current_for(self.child_generation) {
                continue;
            }
            match event {
                ControllerEvent::ChildReady { ready, .. } => {
                    self.handle_child_ready(*ready);
                    result.state_changed = true;
                }
                ControllerEvent::TcpResponse { line, .. } => {
                    let response = self.handle_tcp_response(&line);
                    result.state_changed |= response.state_changed;
                    result.scope_changed |= response.scope_changed;
                }
                ControllerEvent::SourcesMayHaveChanged {
                    paths: changed_paths,
                    watch_batch,
                } => {
                    if self.sources_require_recompile(&changed_paths) {
                        self.recompile_after_source_change(&changed_paths);
                        result.state_changed = true;
                    } else if self.watched_sources.iter().any(|path| !path.exists()) {
                        // A parent of an unresolved nested candidate may have
                        // just appeared. Retarget the watch as the path becomes
                        // reachable without recompiling for directory-only
                        // changes.
                        let refreshed = self.refresh_source_watcher(&changed_paths);
                        if !self.source_compilation.matches(&refreshed) {
                            self.recompile_after_source_change(&changed_paths);
                            result.state_changed = true;
                        } else {
                            self.acknowledge_source_watch_revision(watch_batch);
                        }
                    } else {
                        self.refresh_kqueue_file_watches(&changed_paths);
                        self.acknowledge_source_watch_revision(watch_batch);
                    }
                }
            }
        }

        if self.validate_degraded_source_watch() {
            result.state_changed = true;
        }

        if let Some((code, error)) = self.child.try_take_exit() {
            self.handle_child_exited(code, error);
            result.state_changed = true;
        }

        result
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.processing_requested = true;
        if self.can_reuse_compiled_child() {
            if let Some(id) = self.bridge.send_command("play", &json!({})) {
                self.pending_commands.insert(id, PendingCommand::Play);
            }
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
        self.processing_requested = false;
        if self.state.connected && self.child.is_active() {
            let _ = self.bridge.send_command("pause", &json!({}));
        } else {
            self.advance_child_generation();
            self.child.kill();
            self.bridge.disconnect();
            self.source_compilation = SourceCompilationState::None;
            self.compiled_watch_revision = None;
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
        self.state.midi_input_devices = list_midi_input_devices();
        self.state.error = None;
    }

    pub fn reset_params(&mut self) {
        self.preserved_params.clear();
        for param in &mut self.state.params {
            let Some(default_value) = param_default_value(param) else {
                continue;
            };
            set_param_value(param, default_value);
        }
        if let Some(id) = self.bridge.send_command("resetParams", &json!({})) {
            self.pending_commands
                .insert(id, PendingCommand::ResetParams);
        }
        self.state.error = None;
    }

    pub fn reset_event_arguments(&mut self) {
        self.preserved_events.clear();
        reset_event_values(&mut self.state.events);
        self.state.error = None;
    }

    pub fn clear_log(&mut self) {
        self.state.log_text.clear();
        self.state.log_entries.clear();
        self.state.print_overflow_count = 0;
        self.state.print_transport_drop_count = 0;
        self.state.delegate_overflow_count = 0;
        self.state.delegate_transport_drop_count = 0;
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
        let _ = self
            .bridge
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

    pub fn trigger_midi_note(&mut self, key: i32, velocity: f32, pressed: bool) {
        let name = if pressed {
            if !self.state.midi.note_on {
                return;
            }
            "note_on"
        } else if self.state.midi.note_off {
            "note_off"
        } else {
            return;
        };
        self.bridge.send_command_notification(
            "triggerEvent",
            &json!({
                "name": name,
                "values": [-1, 0, key.clamp(0, 127), velocity.clamp(0.0, 1.0)],
            }),
        );
    }

    pub fn bind_buffer_file(&mut self, name: &str, file_path: &str) {
        if let Some(id) = self
            .bridge
            .send_command("bindBufferWav", &json!({ "name": name, "path": file_path }))
        {
            self.pending_commands.insert(
                id,
                PendingCommand::BindBuffer {
                    name: name.to_owned(),
                    path: file_path.to_owned(),
                },
            );
        }
        self.state.error = None;
    }

    pub fn clear_buffer(&mut self, name: &str) {
        if let Some(id) = self
            .bridge
            .send_command("clearBuffer", &json!({ "name": name }))
        {
            self.pending_commands.insert(
                id,
                PendingCommand::ClearBuffer {
                    name: name.to_owned(),
                },
            );
        }
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

    pub fn set_midi_input_device(&mut self, name: Option<&str>) -> Result<(), String> {
        let next = match validated_device(
            name.or(Some(COMPUTER_KEYBOARD_MIDI_INPUT)),
            &self.state.midi_input_devices,
        ) {
            Ok(next) => next,
            Err(err) => {
                self.state.error = Some(err.clone());
                return Err(err);
            }
        };
        self.options.midi_input_device = next;
        self.state.current_midi_input_device = self.options.midi_input_device.clone();
        self.restart_with_status("Restarting...")
    }

    fn restart_with_status(&mut self, status: &str) -> Result<(), String> {
        self.restart_with_status_after_changes(status, &[])
    }

    fn restart_with_status_after_changes(
        &mut self,
        status: &str,
        changed_paths: &[PathBuf],
    ) -> Result<(), String> {
        let compiled_watch_revision = self.source_watch_revision.current();
        let pending_sources = self.refresh_source_watcher(changed_paths);
        self.advance_child_generation();
        self.child.kill();
        self.bridge.disconnect();
        self.source_compilation = SourceCompilationState::None;
        self.compiled_watch_revision = None;
        self.pending_commands.clear();
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.state.running = false;
        self.state.connected = false;
        self.state.status = status.to_owned();
        self.state.error = None;
        self.state.output_channels = 0;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();

        match ChildSession::spawn(
            &self.launch_path,
            &self.options,
            self.child_generation,
            self.events_tx.clone(),
        ) {
            Ok(child) => {
                self.child = child;
                self.source_compilation = SourceCompilationState::Compiling(pending_sources);
                self.compiled_watch_revision = Some(compiled_watch_revision);
                self.state.running = true;
                Ok(())
            }
            Err(e) => {
                self.source_compilation = SourceCompilationState::Failed(pending_sources);
                self.compiled_watch_revision = None;
                self.state.status = "Failed to start".to_owned();
                self.state.error = Some(e.clone());
                Err(e)
            }
        }
    }

    fn can_reuse_compiled_child(&self) -> bool {
        self.bridge.is_connected()
            && self.child.is_active()
            && self
                .source_compilation
                .ready()
                .is_some_and(|snapshot| self.compiled_sources_are_current(snapshot))
    }

    fn sources_require_recompile(&self, changed_paths: &[PathBuf]) -> bool {
        !self
            .source_compilation
            .snapshot()
            .is_some_and(|snapshot| snapshot.matches_disk_changes(changed_paths))
    }

    fn validate_degraded_source_watch(&mut self) -> bool {
        if self.source_compilation.snapshot().is_none()
            || self.last_source_fallback_validation.elapsed() < SOURCE_WATCH_FALLBACK_INTERVAL
        {
            return false;
        }
        self.last_source_fallback_validation = Instant::now();

        let validation_paths = self.source_watch_fallback_paths().to_vec();
        if validation_paths.is_empty() {
            return false;
        }
        if self.sources_require_recompile(&validation_paths) {
            self.recompile_after_source_change(&validation_paths);
            return true;
        }

        // A missing directory or rejected symlink may have been replaced
        // without an event from a partially covered watcher. Move the watch
        // inward as soon as the reachable, non-symlink topology changes.
        if source_watch_root_map(&self.watched_paths) != self.watched_roots {
            let refreshed = self.refresh_source_watcher(&[]);
            if !self.source_compilation.matches(&refreshed) {
                self.recompile_after_source_change(&validation_paths);
                return true;
            }
        }
        false
    }

    fn source_watch_fallback_paths(&self) -> &[PathBuf] {
        self.watcher
            .as_ref()
            .map(FileWatcher::fallback_validation_paths)
            .unwrap_or(&self.watched_paths)
    }

    fn recompile_after_source_change(&mut self, changed_paths: &[PathBuf]) {
        if self.processing_requested {
            let _ = self.restart_with_status_after_changes("Restarting...", changed_paths);
        } else {
            self.invalidate_compiled_child();
        }
    }

    fn invalidate_compiled_child(&mut self) {
        self.advance_child_generation();
        self.child.kill();
        self.bridge.disconnect();
        self.pending_commands.clear();
        self.source_compilation = SourceCompilationState::None;
        self.compiled_watch_revision = None;
        self.state.connected = false;
        self.state.status = "Stopped".to_owned();
        self.state.error = None;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();
    }

    fn handle_child_ready(&mut self, ready: ReadyEvent) {
        let compilation_is_current = match &self.source_compilation {
            SourceCompilationState::Compiling(pending) => {
                self.compiled_sources_are_current(pending)
            }
            SourceCompilationState::None
            | SourceCompilationState::Ready(_)
            | SourceCompilationState::Failed(_) => false,
        };
        if !compilation_is_current {
            let _ = self.restart_with_status("Restarting...");
            return;
        }
        let SourceCompilationState::Compiling(compiled_sources) =
            std::mem::take(&mut self.source_compilation)
        else {
            unreachable!("matching source compilation must be pending");
        };
        self.source_compilation = SourceCompilationState::Ready(compiled_sources);
        let bridge_error = self
            .bridge
            .connect(ready.port, self.child_generation, self.events_tx.clone())
            .err();

        reconcile_preserved_params(
            &mut self.preserved_params,
            &self.state.params,
            &ready.params,
        );
        self.state.params = ready.params;
        apply_preserved_param_state(&mut self.state.params, &self.preserved_params);
        self.state.buffers = ready.buffers;
        reconcile_preserved_events(
            &mut self.preserved_events,
            &self.state.events,
            &ready.events,
        );
        self.state.events = ready.events;
        self.state.midi = ready.midi;
        self.state.delegates = ready.delegates;
        apply_preserved_event_state(&mut self.state.events, &self.preserved_events);
        self.state.log_text.clear();
        self.state.log_entries.clear();
        self.state.log_revealed = false;
        self.state.print_overflow_count = 0;
        self.state.print_transport_drop_count = 0;
        self.state.delegate_overflow_count = 0;
        self.state.delegate_transport_drop_count = 0;
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
        if !ready.midi_input_devices.is_empty() {
            self.state.midi_input_devices = with_computer_keyboard(ready.midi_input_devices);
        } else if self.state.midi_input_devices.is_empty() {
            self.state.midi_input_devices = list_midi_input_devices();
        }
        self.state.current_input_device = ready.current_input_device;
        self.state.current_output_device = ready.current_output_device;
        self.state.current_midi_input_device =
            if self.options.midi_input_device.as_deref() == Some(COMPUTER_KEYBOARD_MIDI_INPUT) {
                self.options.midi_input_device.clone()
            } else {
                ready.current_midi_input_device
            };
        self.state.connected = self.bridge.is_connected();
        self.state.error = bridge_error;

        // The run UI's compact Log is an explicit delegate consumer. Other
        // control clients remain unsubscribed until they request collection.
        self.bridge
            .send_command_notification("subscribeDelegates", &json!({}));

        for (name, value) in &self.preserved_params {
            self.bridge
                .send_command_notification("setParam", &json!({ "name": name, "value": value }));
        }
        for (name, binding) in &self.preserved_buffers {
            let (command, payload, pending) = binding.replay_request(name);
            if let Some(id) = self.bridge.send_command(command, &payload) {
                self.pending_commands.insert(id, pending);
            }
        }

        self.refresh_processing_state();
        self.scope_polling_active = self.state.running;
        self.scope_polling_in_flight = false;
        self.last_scope_poll = Instant::now();
    }

    fn refresh_source_watcher(&mut self, changed_paths: &[PathBuf]) -> SourceSnapshot {
        let previous_assets = self
            .source_compilation
            .snapshot()
            .map(|snapshot| snapshot.assets.clone())
            .unwrap_or_default();
        let mut project_input_resolved = false;
        let mut project_file = None;
        if let Ok(project_input) = onda_project::resolve_project_input(
            &self.launch_path,
            onda_project::ProjectLimits::default(),
        ) {
            project_input_resolved = true;
            project_file = project_input.project().cloned();
            if let Ok(entry) = std::fs::canonicalize(project_input.entry_path()) {
                self.onda_path = entry;
            }
        } else if let Ok(paths) = onda_project::resolve_project_watch_paths(
            &self.launch_path,
            onda_project::ProjectLimits::default(),
        ) {
            self.onda_path = paths.entry.clone();
            self.project_watch_paths = Some(paths);
        }
        let snapshot = source_snapshot_from_paths(&self.onda_path, &self.watched_sources);
        if let Some(project) = project_file {
            if let Ok(paths) = project.watch_paths() {
                self.project_watch_paths = Some(paths);
            }
        } else if project_input_resolved {
            self.project_watch_paths = None;
        }
        let snapshot = snapshot.with_project_reusing(
            self.project_watch_paths.as_ref(),
            &previous_assets,
            changed_paths,
        );
        self.watched_sources = snapshot.paths();
        let watched_paths = snapshot.watch_paths();
        let previous_fallback_paths = self.source_watch_fallback_paths().to_vec();
        let gap_validation_paths = watcher_gap_validation_paths(
            &watched_paths,
            &self.watched_paths,
            &previous_fallback_paths,
        );
        let (watcher, watched_roots) = start_source_watcher(
            &watched_paths,
            self.source_watch_revision.clone(),
            self.events_tx.clone(),
        );
        if !snapshot.matches_disk_changes(&gap_validation_paths) {
            // Validate the discovery gap for imports/assets that were not part
            // of a complete previous watch. Force the in-flight compilation
            // generation stale if one changed before registration.
            self.source_watch_revision.observe_change();
        }
        self.watcher = watcher;
        self.watched_roots = watched_roots;
        self.watched_paths = watched_paths;
        self.last_source_fallback_validation = Instant::now();
        snapshot
    }

    fn compiled_sources_are_current(&self, snapshot: &SourceSnapshot) -> bool {
        let fallback_paths = self.source_watch_fallback_paths();
        compiled_snapshot_is_current(
            snapshot,
            self.compiled_watch_revision,
            self.source_watch_revision.current(),
            fallback_paths,
        )
    }

    fn acknowledge_source_watch_revision(&mut self, watch_batch: SourceWatchBatch) {
        // While the child is compiling, matching bytes only prove what is on
        // disk now. The child may have parsed transient contents which were
        // changed back before this batch was delivered.
        if !self.source_compilation.can_acknowledge_watch_revision() {
            return;
        }
        if let Some(compiled_revision) = &mut self.compiled_watch_revision {
            if let Some(acknowledged) = watch_batch.advance(*compiled_revision) {
                *compiled_revision = acknowledged;
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn refresh_kqueue_file_watches(&mut self, changed_paths: &[PathBuf]) {
        let Some(paths) = self
            .source_compilation
            .snapshot()
            .map(SourceSnapshot::watch_paths)
        else {
            return;
        };
        let affected = paths
            .into_iter()
            .filter(|path| {
                changed_paths
                    .iter()
                    .any(|changed| paths_overlap(path, changed))
            })
            .collect::<Vec<_>>();
        if let Some(watcher) = self.watcher.as_mut() {
            watcher.reattach_existing_paths(&affected);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn refresh_kqueue_file_watches(&mut self, _changed_paths: &[PathBuf]) {}

    fn advance_child_generation(&mut self) {
        self.child_generation = self
            .child_generation
            .checked_add(1)
            .expect("run child generation exhausted");
    }

    fn handle_child_exited(&mut self, code: Option<i32>, error: Option<String>) {
        self.scope_polling_active = false;
        self.scope_polling_in_flight = false;
        self.bridge.disconnect();
        self.pending_commands.clear();
        self.source_compilation.mark_failed();
        self.compiled_watch_revision = None;
        self.state.running = false;
        self.state.connected = false;
        self.state.status = "Runtime error".to_owned();
        self.state.error = Some(error.unwrap_or_else(|| child_exit_error(code)));
        self.state.output_channels = 0;
        self.state.scope_channels = 0;
        self.state.scope_samples.clear();
    }

    fn handle_tcp_response(&mut self, line: &str) -> PollResult {
        let mut poll = PollResult::default();
        if let Ok(resp) = serde_json::from_str::<Value>(line) {
            if resp.get("event").and_then(Value::as_str) == Some("print") {
                let entries = resp
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let text = resp.get("text").and_then(Value::as_str).unwrap_or_default();
                let overflow_count = resp
                    .get("overflowCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let transport_drop_count = resp
                    .get("transportDropCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if record_notification_is_visible(
                    !text.is_empty() || !entries.is_empty(),
                    overflow_count,
                    transport_drop_count,
                ) {
                    self.state.log_revealed = true;
                }
                if !text.is_empty() {
                    for (index, line) in text.lines().enumerate() {
                        self.state.log_text.push_str(line);
                        self.state.log_text.push('\n');
                        self.state
                            .log_entries
                            .push(entries.get(index).cloned().unwrap_or(Value::Null));
                    }
                    trim_log_history(&mut self.state);
                }
                self.state.print_overflow_count = self
                    .state
                    .print_overflow_count
                    .saturating_add(overflow_count);
                self.state.print_transport_drop_count = self
                    .state
                    .print_transport_drop_count
                    .saturating_add(transport_drop_count);
                poll.state_changed = true;
                return poll;
            }
            if resp.get("event").and_then(Value::as_str) == Some("delegates") {
                let overflow_count = resp
                    .get("overflowCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let transport_drop_count = resp
                    .get("transportDropCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let has_occurrences = resp
                    .get("occurrences")
                    .and_then(Value::as_array)
                    .is_some_and(|occurrences| !occurrences.is_empty());
                if record_notification_is_visible(
                    has_occurrences,
                    overflow_count,
                    transport_drop_count,
                ) {
                    self.state.log_revealed = true;
                }
                if let Some(occurrences) = resp.get("occurrences").and_then(Value::as_array) {
                    for occurrence in occurrences {
                        self.state
                            .log_text
                            .push_str(&format_delegate_log_line(occurrence, &self.state.delegates));
                        self.state.log_text.push('\n');
                        self.state.log_entries.push(json!({
                            "kind": "delegate",
                            "delegate": occurrence,
                        }));
                    }
                    trim_log_history(&mut self.state);
                }
                self.state.delegate_overflow_count = self
                    .state
                    .delegate_overflow_count
                    .saturating_add(overflow_count);
                self.state.delegate_transport_drop_count = self
                    .state
                    .delegate_transport_drop_count
                    .saturating_add(transport_drop_count);
                poll.state_changed = true;
                return poll;
            }
            let id = resp.get("id").and_then(Value::as_u64);
            let succeeded = resp.get("ok").and_then(Value::as_bool).unwrap_or(true);
            if let Some(command) = id.and_then(|id| self.pending_commands.remove(&id)) {
                if succeeded {
                    match command {
                        PendingCommand::BindBuffer { name, path } => {
                            self.preserved_buffers
                                .insert(name.clone(), PreservedBufferBinding::File(path.clone()));
                            update_buffer_loaded_path(&mut self.state.buffers, &name, Some(&path));
                            let _ = self.bridge.send_command("getBuffers", &json!({}));
                            self.refresh_processing_state();
                        }
                        PendingCommand::ClearBuffer { name } => {
                            self.preserved_buffers
                                .insert(name.clone(), PreservedBufferBinding::Cleared);
                            update_buffer_loaded_path(&mut self.state.buffers, &name, None);
                            let _ = self.bridge.send_command("getBuffers", &json!({}));
                            self.refresh_processing_state();
                        }
                        PendingCommand::Play => {
                            self.state.running = true;
                            self.state.status = "Running".to_owned();
                            self.scope_polling_active = true;
                        }
                        PendingCommand::ResetParams => {}
                    }
                } else {
                    match command {
                        PendingCommand::BindBuffer { .. } => self.refresh_processing_state(),
                        PendingCommand::ClearBuffer { .. } => {}
                        PendingCommand::Play => {
                            self.state.running = false;
                            self.scope_polling_active = false;
                            self.scope_polling_in_flight = false;
                            self.state.status = "Runtime error".to_owned();
                        }
                        PendingCommand::ResetParams => {}
                    }
                }
                poll.state_changed = true;
            }
            if !succeeded {
                if let Some(error) = resp.get("error").and_then(Value::as_str) {
                    self.state.error = Some(error.to_owned());
                    poll.state_changed = true;
                }
            }
            if let Some(result) = resp.get("result") {
                if let Some(buffers) = result.get("buffers").and_then(Value::as_array) {
                    self.state.buffers = buffers.clone();
                    self.refresh_processing_state();
                    poll.state_changed = true;
                }
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
                    poll.scope_changed = true;
                }
            }
        }
        poll
    }

    fn refresh_processing_state(&mut self) {
        self.state.running = self.processing_requested && self.state.connected;
        self.state.status = if self.state.running {
            "Running".to_owned()
        } else {
            "Stopped".to_owned()
        };
        self.scope_polling_active = self.state.running;
        if !self.state.running {
            self.scope_polling_in_flight = false;
        }
    }
}

fn record_notification_is_visible(
    has_records: bool,
    overflow_count: u64,
    transport_drop_count: u64,
) -> bool {
    has_records || overflow_count != 0 || transport_drop_count != 0
}

fn trim_log_history(state: &mut RunState) {
    let excess = state
        .log_entries
        .len()
        .saturating_sub(MAX_VISIBLE_LOG_ENTRIES);
    if excess == 0 {
        return;
    }
    state.log_entries.drain(..excess);
    state.log_text = state
        .log_text
        .lines()
        .skip(excess)
        .map(|line| format!("{line}\n"))
        .collect();
}

fn format_delegate_log_line(occurrence: &Value, delegates: &[Value]) -> String {
    let name = occurrence
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("delegate");
    let values = occurrence.get("values").and_then(Value::as_object);
    let params = occurrence
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|index| delegates.get(index as usize))
        .and_then(|delegate| delegate.get("params"))
        .and_then(Value::as_array);
    let values = match (values, params) {
        (Some(values), Some(params)) => params
            .iter()
            .filter_map(|param| {
                let name = param.get("name")?.as_str()?;
                let value = values.get(name)?;
                let type_repr = param.get("type").and_then(Value::as_str);
                Some(format!("{name}={}", format_log_value(value, type_repr)))
            })
            .collect::<Vec<_>>()
            .join(" "),
        (Some(values), None) => values
            .iter()
            .map(|(name, value)| format!("{name}={}", format_log_value(value, None)))
            .collect::<Vec<_>>()
            .join(" "),
        (None, _) => String::new(),
    };
    if values.is_empty() {
        format!("delegate {name}")
    } else {
        format!("delegate {name}: {values}")
    }
}

fn format_log_value(value: &Value, type_repr: Option<&str>) -> String {
    match value {
        // Delegate i64 values cross JSON as decimal strings to retain their
        // full width. Present them as integers rather than quoted text.
        Value::String(value) => value.clone(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| format_log_value(value, type_repr))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Number(value)
            if type_repr.is_some_and(|ty| matches!(ty.split('[').next(), Some("i32" | "i64"))) =>
        {
            value
                .as_i64()
                .map(|value| value.to_string())
                .or_else(|| {
                    value
                        .as_f64()
                        .filter(|value| value.is_finite() && value.fract() == 0.0)
                        .map(|value| format!("{value:.0}"))
                })
                .unwrap_or_else(|| value.to_string())
        }
        _ => value.to_string(),
    }
}

fn resolve_run_input_paths(
    launch_path: &Path,
) -> Result<(PathBuf, Option<onda_project::ProjectWatchPaths>), String> {
    let limits = onda_project::ProjectLimits::default();
    match onda_project::resolve_project_input(launch_path, limits) {
        Ok(input) => {
            let entry = input.entry_path().to_path_buf();
            let project_watch_paths = match input.project() {
                Some(project) => match project.watch_paths() {
                    Ok(paths) => Some(paths),
                    Err(strict_error) => Some(
                        onda_project::resolve_project_watch_paths(launch_path, limits)
                            .map_err(|_| strict_error.to_string())?,
                    ),
                },
                None => None,
            };
            Ok((entry, project_watch_paths))
        }
        Err(strict_error) if onda_project::is_project_file_path(launch_path) => {
            onda_project::resolve_project_watch_paths(launch_path, limits)
                .map(|paths| (paths.entry.clone(), Some(paths)))
                .map_err(|_| strict_error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
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
    stderr_reader: Option<thread::JoinHandle<()>>,
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

fn run_param_json(param: &RunParamInfo) -> RunParamWire {
    RunParamWire::from(param)
}

impl From<&RunParamInfo> for RunParamWire {
    fn from(param: &RunParamInfo) -> Self {
        let scalar_repr = |value| run_param_scalar_repr(&param.type_repr, value);
        Self {
            index: param.index,
            name: param.name.clone(),
            type_repr: param.type_repr.clone(),
            value_repr: param.value.map(scalar_repr),
            default_repr: param.default.map(scalar_repr),
            range_min_repr: param.range_min.map(scalar_repr),
            range_max_repr: param.range_max.map(scalar_repr),
            scale: param.scale.clone(),
            curve_repr: param.curve.map(|curve| curve.to_string()),
            unit: param.unit.clone(),
            step_repr: param.step.map(scalar_repr),
            step_count: param.step_count,
            scalar: param.scalar,
        }
    }
}

impl RunParamWire {
    fn into_host_value(self) -> Result<Value, String> {
        let value = decode_run_param_scalar_repr(&self.type_repr, self.value_repr, "valueRepr")?;
        let default =
            decode_run_param_scalar_repr(&self.type_repr, self.default_repr, "defaultRepr")?;
        let range_min =
            decode_run_param_scalar_repr(&self.type_repr, self.range_min_repr, "rangeMinRepr")?;
        let range_max =
            decode_run_param_scalar_repr(&self.type_repr, self.range_max_repr, "rangeMaxRepr")?;
        let curve = decode_run_param_scalar_repr("f64", self.curve_repr, "curveRepr")?;
        let step = decode_run_param_scalar_repr(&self.type_repr, self.step_repr, "stepRepr")?;
        Ok(json!({
            "index": self.index,
            "name": self.name,
            "type": self.type_repr,
            "value": value,
            "default": default,
            "rangeMin": range_min,
            "rangeMax": range_max,
            "scale": self.scale,
            "curve": curve,
            "unit": self.unit,
            "step": step,
            "stepCount": self.step_count,
            "scalar": self.scalar,
        }))
    }
}

fn run_param_scalar_repr(ty: &str, value: f64) -> String {
    match ty {
        "f32" => (value as f32).to_string(),
        "i32" => (value as i32).to_string(),
        "i64" => (value as i64).to_string(),
        "bool" => (value != 0.0).to_string(),
        _ => value.to_string(),
    }
}

fn decode_run_param_scalar_repr(
    ty: &str,
    repr: Option<String>,
    field: &str,
) -> Result<Value, String> {
    let Some(repr) = repr else {
        return Ok(Value::Null);
    };
    let value = match ty {
        "f32" => repr
            .parse::<f32>()
            .ok()
            .map(f64::from)
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        "f64" => repr
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        "i32" => repr.parse::<i32>().ok().map(Value::from),
        "i64" => repr.parse::<i64>().ok().map(Value::from),
        "bool" => repr.parse::<bool>().ok().map(Value::Bool),
        _ => None,
    };
    value.ok_or_else(|| format!("run parameter has invalid {ty} '{field}' value '{repr}'"))
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
        "loadedFrames": buffer.loaded_frames,
        "loadedChannels": buffer.loaded_channels,
        "loadedSampleRate": buffer.loaded_sample_rate_hz,
        "waveform": buffer.waveform.as_ref().map(|waveform| json!({
            "minValue": waveform.min_value,
            "maxValue": waveform.max_value,
            "minimums": waveform.minimums,
            "maximums": waveform.maximums,
        })),
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
        RunEventValue::I64(value) => Value::String(value.to_string()),
        RunEventValue::Array(values) => {
            Value::Array(values.iter().map(run_event_value_json).collect())
        }
    }
}

impl ChildSession {
    fn is_active(&self) -> bool {
        self.child.is_some()
    }

    fn spawn(
        onda_path: &Path,
        options: &RunHostOptions,
        generation: u64,
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
        if let Some(midi_input_device) = options
            .midi_input_device
            .as_deref()
            .filter(|name| *name != COMPUTER_KEYBOARD_MIDI_INPUT)
        {
            cmd.arg("--midi-input-device").arg(midi_input_device);
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
        let stderr_reader = thread::spawn(move || {
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
                        let params = match raw
                            .params
                            .unwrap_or_default()
                            .into_iter()
                            .map(RunParamWire::into_host_value)
                            .collect::<Result<Vec<_>, _>>()
                        {
                            Ok(params) => params,
                            Err(error) => {
                                eprintln!("[onda run stdout] invalid ready event: {error}");
                                continue;
                            }
                        };
                        let ready = ReadyEvent {
                            path: raw.path.unwrap_or_default(),
                            port: raw.port.unwrap_or(0),
                            params,
                            buffers: raw.buffers.unwrap_or_default(),
                            events: raw.events.unwrap_or_default(),
                            midi: raw.midi,
                            delegates: raw.delegates.unwrap_or_default(),
                            output_channels: raw.output_channels.unwrap_or(0),
                            input_devices: raw.input_devices.unwrap_or_default(),
                            output_devices: raw.output_devices.unwrap_or_default(),
                            midi_input_devices: raw.midi_input_devices.unwrap_or_default(),
                            current_input_device: raw.current_input_device,
                            current_output_device: raw.current_output_device,
                            current_midi_input_device: raw.current_midi_input_device,
                        };
                        let _ = event_tx.send(ControllerEvent::ChildReady {
                            generation,
                            ready: Box::new(ready),
                        });
                        continue;
                    }
                }
                eprintln!("[onda run stdout] {trimmed}");
            }
        });

        Ok(Self {
            child: Some(child),
            stderr_buffer,
            stderr_reader: Some(stderr_reader),
        })
    }

    fn try_take_exit(&mut self) -> Option<(Option<i32>, Option<String>)> {
        let child = self.child.as_mut()?;
        let status = child.try_wait().ok()??;
        let status_error = (!status.success()).then(|| format_exit_status(&status));
        self.child = None;
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        let stderr_error = self.stderr_buffer.lock().ok().and_then(|slot| {
            let trimmed = slot.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        });
        let error = match (stderr_error, status_error) {
            (Some(stderr), Some(status)) => Some(format!("{stderr}\n{status}")),
            (Some(stderr), None) => Some(stderr),
            (None, status) => status,
        };
        Some((status.code(), error))
    }

    fn kill(&mut self) {
        if let Some(ref mut child) = self.child {
            terminate_run_child(child);
            let _ = child.wait();
        }
        self.child = None;
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
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

    fn connect(
        &self,
        port: u16,
        generation: u64,
        event_tx: Sender<ControllerEvent>,
    ) -> Result<(), String> {
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
                            let _ = event_tx.send(ControllerEvent::TcpResponse {
                                generation,
                                line: trimmed,
                            });
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

    fn send_command(&self, command: &str, payload: &Value) -> Option<u64> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        self.send_command_inner(Some(id), command, payload)
            .then_some(id)
    }

    fn send_command_notification(&self, command: &str, payload: &Value) {
        let _ = self.send_command_inner(None, command, payload);
    }

    fn send_command_inner(&self, id: Option<u64>, command: &str, payload: &Value) -> bool {
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
                return false;
            };
            let line = serde_json::to_string(&request).unwrap_or_default();
            return writer.write_all(line.as_bytes()).is_ok()
                && writer.write_all(b"\n").is_ok()
                && writer.flush().is_ok();
        }
        false
    }

    fn is_connected(&self) -> bool {
        self.writer
            .lock()
            .map(|writer| writer.is_some())
            .unwrap_or(false)
    }
}

struct FileWatcher {
    _watcher: notify::RecommendedWatcher,
    coverage: SourceWatchCoverage,
}

struct SourceWatchCoverage {
    watched_paths: Vec<PathBuf>,
    initially_uncovered_paths: Vec<PathBuf>,
    coverage_lost: Arc<AtomicBool>,
}

struct SourceWatchPlan {
    roots: HashMap<PathBuf, notify::RecursiveMode>,
    path_requirements: Vec<(PathBuf, Vec<PathBuf>)>,
}

impl SourceWatchCoverage {
    fn fallback_validation_paths(&self) -> &[PathBuf] {
        if self.coverage_lost.load(Ordering::Acquire) {
            &self.watched_paths
        } else {
            &self.initially_uncovered_paths
        }
    }

    #[cfg(test)]
    fn covers_all_paths(&self) -> bool {
        self.fallback_validation_paths().is_empty()
    }
}

#[derive(Clone, Default)]
struct SourceWatchRevision(Arc<AtomicU64>);

#[derive(Clone, Copy, Debug)]
struct SourceWatchBatch {
    first_revision: u64,
    last_revision: u64,
    contiguous: bool,
}

impl SourceWatchBatch {
    fn advance(self, current: u64) -> Option<u64> {
        if self.last_revision <= current {
            return Some(current);
        }
        (self.contiguous && self.first_revision == current.wrapping_add(1))
            .then_some(self.last_revision)
    }
}

impl SourceWatchRevision {
    fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    fn observe_change(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Release).wrapping_add(1)
    }
}

impl FileWatcher {
    #[cfg(test)]
    fn watch(
        paths: &[PathBuf],
        revision: SourceWatchRevision,
        on_change: impl Fn(Vec<PathBuf>, SourceWatchBatch) + Send + 'static,
    ) -> Option<Self> {
        Self::watch_plan(source_watch_plan(paths), revision, on_change)
    }

    fn watch_plan(
        plan: SourceWatchPlan,
        revision: SourceWatchRevision,
        on_change: impl Fn(Vec<PathBuf>, SourceWatchBatch) + Send + 'static,
    ) -> Option<Self> {
        if plan.path_requirements.is_empty() {
            return None;
        }
        let relevant_paths = plan
            .path_requirements
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        Self::watch_roots_filtered(
            plan.roots,
            plan.path_requirements,
            revision,
            move |changed| {
                relevant_paths
                    .iter()
                    .any(|watched| paths_overlap(changed, watched))
            },
            on_change,
        )
    }

    #[cfg(test)]
    fn watch_roots(
        watch_roots: HashMap<PathBuf, notify::RecursiveMode>,
        revision: SourceWatchRevision,
        on_change: impl Fn(Vec<PathBuf>, SourceWatchBatch) + Send + 'static,
    ) -> Option<Self> {
        let path_requirements = watch_roots
            .keys()
            .map(|root| (root.clone(), vec![root.clone()]))
            .collect();
        Self::watch_roots_filtered(
            watch_roots,
            path_requirements,
            revision,
            |_| true,
            on_change,
        )
    }

    fn watch_roots_filtered(
        watch_roots: HashMap<PathBuf, notify::RecursiveMode>,
        path_requirements: Vec<(PathBuf, Vec<PathBuf>)>,
        revision: SourceWatchRevision,
        is_relevant: impl Fn(&Path) -> bool + Send + 'static,
        on_change: impl Fn(Vec<PathBuf>, SourceWatchBatch) + Send + 'static,
    ) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx).ok()?;
        let mut registered_root = false;
        let mut registered_roots = HashSet::new();
        let rescan_paths = watch_roots.keys().cloned().collect::<Vec<_>>();
        for (root, mode) in watch_roots {
            let registered = notify::Watcher::watch(&mut watcher, &root, mode).is_ok();
            registered_root |= registered;
            if registered {
                registered_roots.insert(root);
            }
        }
        if !registered_root {
            return None;
        }

        let mut watched_paths = Vec::with_capacity(path_requirements.len());
        let mut initially_uncovered_paths = Vec::new();
        for (path, required_roots) in path_requirements {
            if required_roots
                .iter()
                .any(|root| !registered_roots.contains(root))
            {
                initially_uncovered_paths.push(path.clone());
            }
            watched_paths.push(path);
        }
        let coverage_lost = Arc::new(AtomicBool::new(false));
        let callback_coverage_lost = coverage_lost.clone();
        thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                // Keep unrelated activity outside the sliding debounce window,
                // otherwise a busy sibling can indefinitely postpone reloads.
                let Some(mut changed) = relevant_source_change_paths(
                    event,
                    &rescan_paths,
                    &is_relevant,
                    &callback_coverage_lost,
                ) else {
                    continue;
                };
                let first_revision = revision.observe_change();
                let mut last_revision = first_revision;
                let mut revisions_are_contiguous = true;

                let mut deadline = Instant::now() + SOURCE_WATCH_DEBOUNCE;
                loop {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(event) => {
                            if let Some(next) = relevant_source_change_paths(
                                event,
                                &rescan_paths,
                                &is_relevant,
                                &callback_coverage_lost,
                            ) {
                                let next_revision = revision.observe_change();
                                revisions_are_contiguous &=
                                    next_revision == last_revision.wrapping_add(1);
                                last_revision = next_revision;
                                changed.extend(next);
                                deadline = Instant::now() + SOURCE_WATCH_DEBOUNCE;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }

                changed.sort();
                changed.dedup();
                on_change(
                    changed,
                    SourceWatchBatch {
                        first_revision,
                        last_revision,
                        contiguous: revisions_are_contiguous,
                    },
                );
            }
        });

        Some(Self {
            _watcher: watcher,
            coverage: SourceWatchCoverage {
                watched_paths,
                initially_uncovered_paths,
                coverage_lost,
            },
        })
    }

    #[cfg(test)]
    fn covers_all_paths(&self) -> bool {
        self.coverage.covers_all_paths()
    }

    fn fallback_validation_paths(&self) -> &[PathBuf] {
        self.coverage.fallback_validation_paths()
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn reattach_existing_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    // Filesystem-backed Onda paths reject symlinks. Keep only
                    // the parent watch so replacing the alias remains visible.
                    continue;
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    // The parent subscription covers creation of a missing path.
                    continue;
                }
                Err(_) => {
                    self.coverage.coverage_lost.store(true, Ordering::Release);
                    continue;
                }
            }
            let _ = notify::Watcher::unwatch(&mut self._watcher, path);
            if notify::Watcher::watch(
                &mut self._watcher,
                path,
                notify::RecursiveMode::NonRecursive,
            )
            .is_err()
            {
                // Kqueue parent watches detect replacement but not subsequent
                // in-place writes to the new inode. Fall back to disk validation.
                self.coverage.coverage_lost.store(true, Ordering::Release);
            }
        }
    }
}

fn watcher_gap_validation_paths(
    watched_paths: &[PathBuf],
    previous_paths: &[PathBuf],
    previous_fallback_paths: &[PathBuf],
) -> Vec<PathBuf> {
    watched_paths
        .iter()
        .filter(|path| !previous_paths.contains(path) || previous_fallback_paths.contains(path))
        .cloned()
        .collect()
}

fn relevant_source_change_paths(
    event: notify::Result<notify::Event>,
    rescan_paths: &[PathBuf],
    is_relevant: &impl Fn(&Path) -> bool,
    coverage_lost: &AtomicBool,
) -> Option<Vec<PathBuf>> {
    if event.is_err() {
        // A backend error can mean that an existing subscription was lost
        // (for example, because the platform watch limit was exhausted).
        // This watcher instance must use disk validation from this point on.
        coverage_lost.store(true, Ordering::Release);
    }
    let mut changed = source_change_paths(event)?;
    if changed.is_empty() {
        changed.extend_from_slice(rescan_paths);
    }
    changed.retain(|path| is_relevant(path));
    (!changed.is_empty()).then_some(changed)
}

fn source_change_paths(event: notify::Result<notify::Event>) -> Option<Vec<PathBuf>> {
    let Ok(event) = event else {
        return Some(Vec::new());
    };
    if event.need_rescan() {
        return Some(event.paths);
    }
    // Snapshotting reads every source. Treating those reads as changes creates
    // a watcher -> parse -> watcher feedback loop on backends that report access.
    // A close-after-write is also represented as an access event, however, and
    // is the only mutation some filesystems report after an editor save.
    let changes_source = matches!(
        event.kind,
        notify::EventKind::Access(notify::event::AccessKind::Close(
            notify::event::AccessMode::Write
        ))
    ) || !matches!(
        event.kind,
        notify::EventKind::Access(_)
            | notify::EventKind::Modify(notify::event::ModifyKind::Metadata(
                notify::event::MetadataKind::AccessTime
            ))
    );
    if changes_source {
        Some(event.paths)
    } else {
        None
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn source_watch_root(path: &Path) -> (PathBuf, notify::RecursiveMode) {
    let desired = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut current = PathBuf::new();
    let mut deepest_directory = None;
    for component in desired.components() {
        current.push(component.as_os_str());
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => break,
            Ok(metadata) if metadata.is_dir() => deepest_directory = Some(current.clone()),
            Ok(_) | Err(_) => break,
        }
    }

    let existing = deepest_directory.unwrap_or_else(|| PathBuf::from("."));
    if existing == desired {
        return (existing, notify::RecursiveMode::NonRecursive);
    }
    let mode = if existing.parent().is_some() {
        notify::RecursiveMode::Recursive
    } else {
        // Never recursively subscribe to an entire filesystem while waiting
        // for the first component of an absolute include path.
        notify::RecursiveMode::NonRecursive
    };
    (existing, mode)
}

fn source_watch_roots(path: &Path) -> Vec<(PathBuf, notify::RecursiveMode)> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut roots = vec![source_watch_root(path)];
    // Kqueue does not report writes to a directory's children through a
    // non-recursive directory watch. Watch the current inode as well, while
    // retaining the parent subscription for atomic replacement.
    #[cfg(target_os = "macos")]
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_symlink()) {
        roots.push((path.to_path_buf(), notify::RecursiveMode::NonRecursive));
    }
    roots
}

fn source_watch_plan(paths: &[PathBuf]) -> SourceWatchPlan {
    let mut watch_roots = HashMap::<PathBuf, notify::RecursiveMode>::new();
    let mut requirements = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for path in paths {
        for (root, mode) in source_watch_roots(path) {
            watch_roots
                .entry(root.clone())
                .and_modify(|existing| {
                    if mode == notify::RecursiveMode::Recursive {
                        *existing = mode;
                    }
                })
                .or_insert(mode);
            requirements.entry(path.clone()).or_default().push(root);
        }
    }
    for required_roots in requirements.values_mut() {
        required_roots.sort();
        required_roots.dedup();
    }
    SourceWatchPlan {
        roots: watch_roots,
        path_requirements: requirements.into_iter().collect(),
    }
}

fn source_watch_root_map(paths: &[PathBuf]) -> HashMap<PathBuf, notify::RecursiveMode> {
    source_watch_plan(paths).roots
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceSnapshot {
    load_succeeded: bool,
    sources: Vec<SourceFileSnapshot>,
    project_manifest: Option<SourceFileSnapshot>,
    assets: Vec<AssetFileSnapshot>,
}

impl SourceSnapshot {
    fn paths(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .map(|source| source.path.clone())
            .collect()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .map(|source| source.path.clone())
            .chain(
                self.project_manifest
                    .iter()
                    .map(|manifest| manifest.path.clone()),
            )
            .chain(self.assets.iter().map(|asset| asset.path.clone()))
            .collect()
    }

    #[cfg(test)]
    fn matches_disk(&self) -> bool {
        self.sources.iter().all(SourceFileSnapshot::matches_disk)
            && self
                .project_manifest
                .as_ref()
                .is_none_or(SourceFileSnapshot::matches_disk)
            && self.assets.iter().all(AssetFileSnapshot::matches_disk)
    }

    fn matches_disk_changes(&self, changed_paths: &[PathBuf]) -> bool {
        let may_have_changed = |path: &Path| {
            changed_paths
                .iter()
                .any(|changed| paths_overlap(path, changed))
        };
        self.sources
            .iter()
            .filter(|source| may_have_changed(&source.path))
            .all(SourceFileSnapshot::matches_disk)
            && self
                .project_manifest
                .iter()
                .filter(|manifest| may_have_changed(&manifest.path))
                .all(SourceFileSnapshot::matches_disk)
            && self
                .assets
                .iter()
                .filter(|asset| may_have_changed(&asset.path))
                .all(AssetFileSnapshot::matches_disk)
    }

    fn with_project(self, project: Option<&onda_project::ProjectWatchPaths>) -> Self {
        self.with_project_reusing(project, &[], &[])
    }

    fn with_project_reusing(
        mut self,
        project: Option<&onda_project::ProjectWatchPaths>,
        previous_assets: &[AssetFileSnapshot],
        changed_paths: &[PathBuf],
    ) -> Self {
        self.project_manifest = project.map(|project| SourceFileSnapshot {
            contents: read_file_without_symlinks(&project.manifest),
            path: project.manifest.clone(),
        });
        let previous_assets = previous_assets
            .iter()
            .map(|asset| (asset.path.as_path(), asset.digest))
            .collect::<HashMap<_, _>>();
        self.assets = project
            .into_iter()
            .flat_map(|project| &project.assets)
            .map(|path| {
                let changed = changed_paths
                    .iter()
                    .any(|changed| paths_overlap(path, changed));
                let digest = if changed {
                    digest_file(path)
                } else {
                    previous_assets
                        .get(path.as_path())
                        .copied()
                        .unwrap_or_else(|| digest_file(path))
                };
                AssetFileSnapshot {
                    digest,
                    path: path.clone(),
                }
            })
            .collect();
        self
    }
}

fn compiled_snapshot_is_current(
    snapshot: &SourceSnapshot,
    compiled_watch_revision: Option<u64>,
    current_watch_revision: u64,
    fallback_validation_paths: &[PathBuf],
) -> bool {
    compiled_watch_revision == Some(current_watch_revision)
        && snapshot.matches_disk_changes(fallback_validation_paths)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceFileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl SourceFileSnapshot {
    fn matches_disk(&self) -> bool {
        self.contents == read_file_without_symlinks(&self.path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssetFileSnapshot {
    path: PathBuf,
    digest: Option<[u8; 32]>,
}

impl AssetFileSnapshot {
    fn matches_disk(&self) -> bool {
        self.digest == digest_file(&self.path)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum SourceCompilationState {
    #[default]
    None,
    Compiling(SourceSnapshot),
    Ready(SourceSnapshot),
    Failed(SourceSnapshot),
}

impl SourceCompilationState {
    fn snapshot(&self) -> Option<&SourceSnapshot> {
        match self {
            Self::None => None,
            Self::Compiling(snapshot) | Self::Ready(snapshot) | Self::Failed(snapshot) => {
                Some(snapshot)
            }
        }
    }

    fn ready(&self) -> Option<&SourceSnapshot> {
        match self {
            Self::Ready(snapshot) => Some(snapshot),
            Self::None | Self::Compiling(_) | Self::Failed(_) => None,
        }
    }

    fn matches(&self, snapshot: &SourceSnapshot) -> bool {
        self.snapshot() == Some(snapshot)
    }

    fn can_acknowledge_watch_revision(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    fn mark_failed(&mut self) {
        let previous = std::mem::take(self);
        *self = match previous {
            Self::None => Self::None,
            Self::Compiling(snapshot) | Self::Ready(snapshot) | Self::Failed(snapshot) => {
                Self::Failed(snapshot)
            }
        };
    }
}

#[cfg(test)]
fn source_snapshot(entry: &Path, previous: &[PathBuf]) -> SourceSnapshot {
    source_snapshot_with_project(entry, previous, None)
}

#[cfg(test)]
fn source_snapshot_with_project(
    entry: &Path,
    previous: &[PathBuf],
    project: Option<&onda_project::ProjectWatchPaths>,
) -> SourceSnapshot {
    source_snapshot_from_paths(entry, previous).with_project(project)
}

fn source_snapshot_from_paths(entry: &Path, previous: &[PathBuf]) -> SourceSnapshot {
    // Resolve the exact source graph first. Project metadata and assets must
    // never participate in source loading or failed-load source retention.
    let loaded = load_program_file(entry);
    let (manifest, load_succeeded) = match loaded {
        Ok(loaded) => (loaded.sources, true),
        Err(error) => (error.sources, false),
    };
    let mut document_contents = manifest
        .documents
        .into_vec()
        .into_iter()
        .map(|document| (document.path, document.contents.into_bytes()))
        .collect::<HashMap<_, _>>();
    let mut paths = manifest.files;
    if !paths.iter().any(|path| path == entry) {
        paths.insert(0, entry.to_path_buf());
    }
    for path in manifest.unresolved_files {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if !load_succeeded {
        for path in previous {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }
    let sources = paths
        .into_iter()
        .map(|path| SourceFileSnapshot {
            contents: document_contents
                .remove(&path)
                .or_else(|| read_file_without_symlinks(&path)),
            path,
        })
        .collect();
    SourceSnapshot {
        load_succeeded,
        sources,
        project_manifest: None,
        assets: Vec::new(),
    }
}

fn read_file_without_symlinks(path: &Path) -> Option<Vec<u8>> {
    onda_frontend::ensure_no_symlink_components(path).ok()?;
    fs::read(path).ok()
}

fn digest_file(path: &Path) -> Option<[u8; 32]> {
    onda_frontend::ensure_no_symlink_components(path).ok()?;
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let bytes = file.read(&mut chunk).ok()?;
        if bytes == 0 {
            break;
        }
        hasher.update(&chunk[..bytes]);
    }
    Some(hasher.finalize().into())
}

fn start_source_watcher(
    paths: &[PathBuf],
    revision: SourceWatchRevision,
    events_tx: Sender<ControllerEvent>,
) -> (Option<FileWatcher>, HashMap<PathBuf, notify::RecursiveMode>) {
    let plan = source_watch_plan(paths);
    let watched_roots = plan.roots.clone();
    let watcher = FileWatcher::watch_plan(plan, revision, move |paths, watch_batch| {
        let _ = events_tx.send(ControllerEvent::SourcesMayHaveChanged { paths, watch_batch });
    });
    (watcher, watched_roots)
}

fn list_input_devices() -> Vec<String> {
    onda_cpal::input_audio_devices()
}

fn list_output_devices() -> Vec<String> {
    onda_cpal::output_audio_devices()
}

fn list_midi_input_devices() -> Vec<String> {
    with_computer_keyboard(midi::input_devices())
}

fn with_computer_keyboard(mut devices: Vec<String>) -> Vec<String> {
    devices.insert(0, COMPUTER_KEYBOARD_MIDI_INPUT.to_owned());
    devices
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

fn classify_host_events(
    events: Vec<RunEventInfo>,
) -> Result<(Vec<RunEventInfo>, RunMidiCapabilities), String> {
    let mut visible = Vec::with_capacity(events.len());
    let mut midi = RunMidiCapabilities::default();
    for event in events {
        let Some(expected) = event_by_name(&event.name) else {
            visible.push(event);
            continue;
        };
        if !signature_matches(
            expected,
            event
                .params
                .iter()
                .map(|param| (param.name.as_str(), param.type_repr.as_str())),
        ) {
            return Err(format!(
                "canonical host event '{}' must use the exact signature ({})",
                expected.name, expected.signature
            ));
        }
        if expected.family == HostEventFamily::Midi {
            midi.available = true;
            midi.note_on |= expected.name == "note_on";
            midi.note_off |= expected.name == "note_off";
        }
    }
    Ok((visible, midi))
}

fn child_exit_error(code: Option<i32>) -> String {
    code.map_or_else(
        || "Runtime error: audio processing terminated unexpectedly".to_owned(),
        |code| format!("Runtime error: audio processing exited with code {code}"),
    )
}

#[cfg(unix)]
fn format_exit_status(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    status.signal().map_or_else(
        || child_exit_error(status.code()),
        |signal| format!("Runtime error: audio processing terminated by signal {signal}"),
    )
}

#[cfg(not(unix))]
fn format_exit_status(status: &std::process::ExitStatus) -> String {
    child_exit_error(status.code())
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
        && old_param.get("scale") == new_param.get("scale")
        && old_param.get("curve") == new_param.get("curve")
        && old_param.get("unit") == new_param.get("unit")
        && old_param.get("step") == new_param.get("step")
        && old_param.get("stepCount") == new_param.get("stepCount")
        && old_param.get("scalar") == new_param.get("scalar")
}

fn reconcile_preserved_events(
    preserved_events: &mut Vec<(String, Vec<Value>)>,
    old_events: &[Value],
    new_events: &[Value],
) {
    preserved_events.retain(|(name, _)| {
        let Some(old_event) = old_events
            .iter()
            .find(|event| event_name(event) == Some(name))
        else {
            return false;
        };
        let Some(new_event) = new_events
            .iter()
            .find(|event| event_name(event) == Some(name))
        else {
            return false;
        };
        events_are_compatible_for_preservation(old_event, new_event)
    });
}

fn events_are_compatible_for_preservation(old_event: &Value, new_event: &Value) -> bool {
    if event_name(old_event) != event_name(new_event) {
        return false;
    }
    let (Some(old_args), Some(new_args)) = (
        old_event.get("args").and_then(Value::as_array),
        new_event.get("args").and_then(Value::as_array),
    ) else {
        return false;
    };
    old_args.len() == new_args.len()
        && old_args.iter().zip(new_args).all(|(old_arg, new_arg)| {
            old_arg.get("name") == new_arg.get("name")
                && old_arg.get("type") == new_arg.get("type")
                && old_arg.get("default") == new_arg.get("default")
        })
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
        classify_host_events, compiled_snapshot_is_current, events_are_compatible_for_preservation,
        format_delegate_log_line, params_are_compatible_for_preservation,
        reconcile_preserved_events, reconcile_preserved_params, record_notification_is_visible,
        relevant_source_change_paths, run_param_json, source_change_paths, source_snapshot,
        source_snapshot_with_project, source_watch_root, watcher_gap_validation_paths,
        ControllerEvent, FileWatcher, ParamDomain, ParamScalarType, ParamScale, PendingCommand,
        PreservedBufferBinding, RunEventInfo, RunEventParamInfo, RunEventValue, RunHostOptions,
        RunParamInfo, RunParamWire, SourceCompilationState, SourceWatchRevision,
    };
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn realtime_host_defaults_to_256_frame_blocks() {
        assert_eq!(RunHostOptions::default().block_frames, 256);
    }

    #[test]
    fn canonical_host_events_are_hidden_and_enable_midi_capabilities() {
        let events = vec![
            host_event(
                "note_on",
                &[
                    ("id", "i32"),
                    ("channel", "i32"),
                    ("key", "i32"),
                    ("velocity", "f32"),
                ],
            ),
            host_event("tempo", &[("bpm", "f64")]),
            host_event("gate", &[("enabled", "bool")]),
        ];
        let (visible, midi) = classify_host_events(events).expect("valid host event signatures");

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "gate");
        assert!(midi.available);
        assert!(midi.note_on);
        assert!(!midi.note_off);
    }

    #[test]
    fn canonical_host_event_names_reject_incompatible_signatures() {
        let error = classify_host_events(vec![host_event(
            "note_on",
            &[
                ("id", "i32"),
                ("channel", "i32"),
                ("note", "i32"),
                ("velocity", "f32"),
            ],
        )])
        .expect_err("reserved signature must be exact");

        assert!(error.contains("exact signature"));
    }

    fn host_event(name: &str, params: &[(&str, &str)]) -> RunEventInfo {
        RunEventInfo {
            index: 0,
            name: name.to_owned(),
            params: params
                .iter()
                .enumerate()
                .map(|(index, (name, type_repr))| RunEventParamInfo {
                    index,
                    name: (*name).to_owned(),
                    type_repr: (*type_repr).to_owned(),
                    value: RunEventValue::Number(0.0),
                })
                .collect(),
        }
    }

    #[test]
    fn delegate_log_lines_follow_declared_order_and_types() {
        let delegates = [json!({
            "name": "meter",
            "params": [
                { "name": "voice", "type": "i32" },
                { "name": "bins", "type": "f32[]" },
                { "name": "frame", "type": "i64" },
            ],
        })];
        assert_eq!(
            format_delegate_log_line(
                &json!({
                    "index": 0,
                    "name": "meter",
                    "values": {
                        "voice": 3.0,
                        "bins": [0.25, 0.5],
                        "frame": "9007199254740993",
                    },
                }),
                &delegates
            ),
            "delegate meter: voice=3 bins=[0.25, 0.5] frame=9007199254740993"
        );
        assert_eq!(
            format_delegate_log_line(&json!({ "name": "done", "values": {} }), &[],),
            "delegate done"
        );
    }

    #[test]
    fn record_loss_notifications_reveal_the_log_without_records() {
        assert!(record_notification_is_visible(false, 1, 0));
        assert!(record_notification_is_visible(false, 0, 1));
        assert!(!record_notification_is_visible(false, 0, 0));
    }

    #[test]
    fn control_json_dispatches_typed_parameter_values() {
        let coupling = RunParamInfo {
            index: 0,
            name: "coupling".to_owned(),
            type_repr: "f32".to_owned(),
            value: Some(f64::from(0.72_f32)),
            default: Some(f64::from(0.72_f32)),
            range_min: Some(0.0),
            range_max: Some(f64::from(0.98_f32)),
            scale: Some("linear".to_owned()),
            curve: Some(-4.000000000000001),
            unit: None,
            step: None,
            step_count: None,
            scalar: true,
        };
        let stepped = RunParamInfo {
            index: 1,
            name: "stepped".to_owned(),
            type_repr: "f32".to_owned(),
            value: Some(f64::from(0.2_f32)),
            default: Some(f64::from(0.1_f32)),
            range_min: Some(0.0),
            range_max: Some(f64::from(0.3_f32)),
            scale: Some("linear".to_owned()),
            curve: None,
            unit: None,
            step: Some(f64::from(0.1_f32)),
            step_count: Some(3),
            scalar: true,
        };
        let gate = RunParamInfo {
            index: 2,
            name: "gate".to_owned(),
            type_repr: "bool".to_owned(),
            value: Some(1.0),
            default: Some(1.0),
            range_min: None,
            range_max: None,
            scale: None,
            curve: None,
            unit: None,
            step: None,
            step_count: None,
            scalar: true,
        };
        let wire_params = vec![
            run_param_json(&coupling),
            run_param_json(&stepped),
            run_param_json(&gate),
        ];
        assert_eq!(wire_params[0].range_max_repr.as_deref(), Some("0.98"));
        assert_eq!(wire_params[1].step_repr.as_deref(), Some("0.1"));
        assert_eq!(wire_params[2].value_repr.as_deref(), Some("true"));

        let encoded = serde_json::to_string(&wire_params).expect("serialize run parameters");
        let parsed: Vec<RunParamWire> =
            serde_json::from_str(&encoded).expect("deserialize run parameters");
        let decoded = parsed
            .into_iter()
            .map(RunParamWire::into_host_value)
            .collect::<Result<Vec<_>, _>>()
            .expect("decode typed run parameters");

        assert_eq!(
            decoded[0]["rangeMax"]
                .as_f64()
                .expect("numeric range maximum")
                .to_bits(),
            f64::from(0.98_f32).to_bits()
        );
        assert_eq!(
            decoded[1]["step"]
                .as_f64()
                .expect("numeric parameter step")
                .to_bits(),
            f64::from(0.1_f32).to_bits()
        );
        assert_eq!(
            decoded[0]["curve"]
                .as_f64()
                .expect("numeric parameter curve")
                .to_bits(),
            (-4.000000000000001_f64).to_bits()
        );
        assert_eq!(decoded[2]["value"], true);
        assert_eq!(decoded[2]["default"], true);
        ParamDomain::new(
            ParamScalarType::F32,
            decoded[0]["rangeMin"].as_f64().expect("range minimum"),
            decoded[0]["rangeMax"].as_f64().expect("range maximum"),
            ParamScale::Linear,
            decoded[0]["curve"].as_f64(),
            None,
            None,
            None,
        )
        .expect("transported parameter domain");
    }

    #[test]
    fn child_event_generation_filter_rejects_stale_events() {
        let event = ControllerEvent::TcpResponse {
            generation: 41,
            line: String::new(),
        };
        assert!(event.is_current_for(41));
        assert!(!event.is_current_for(42));
        assert!(ControllerEvent::SourcesMayHaveChanged {
            paths: Vec::new(),
            watch_batch: super::SourceWatchBatch {
                first_revision: 1,
                last_revision: 1,
                contiguous: true,
            },
        }
        .is_current_for(42));
    }

    #[test]
    fn source_watch_batches_only_acknowledge_contiguous_revisions() {
        let contiguous = super::SourceWatchBatch {
            first_revision: 5,
            last_revision: 7,
            contiguous: true,
        };
        assert_eq!(contiguous.advance(4), Some(7));

        let starts_after_gap = super::SourceWatchBatch {
            first_revision: 6,
            ..contiguous
        };
        assert_eq!(starts_after_gap.advance(4), None);

        let interleaved = super::SourceWatchBatch {
            contiguous: false,
            ..contiguous
        };
        assert_eq!(interleaved.advance(4), None);
        assert_eq!(contiguous.advance(7), Some(7));
    }

    #[test]
    fn compiling_sources_never_acknowledge_transient_mutations() {
        let snapshot = super::SourceSnapshot {
            load_succeeded: false,
            sources: Vec::new(),
            project_manifest: None,
            assets: Vec::new(),
        };
        assert!(
            !SourceCompilationState::Compiling(snapshot.clone()).can_acknowledge_watch_revision()
        );
        assert!(SourceCompilationState::Ready(snapshot).can_acknowledge_watch_revision());
    }

    #[test]
    fn disk_fallback_never_replaces_the_watcher_revision_guard() {
        let snapshot = super::SourceSnapshot {
            load_succeeded: false,
            sources: Vec::new(),
            project_manifest: None,
            assets: Vec::new(),
        };

        assert!(compiled_snapshot_is_current(&snapshot, Some(7), 7, &[]));
        assert!(
            !compiled_snapshot_is_current(&snapshot, Some(7), 8, &[]),
            "matching disk state cannot make a stale compilation revision current"
        );
    }

    #[test]
    fn watcher_recreation_validates_uncovered_and_new_paths() {
        let first = PathBuf::from("first.onda");
        let second = PathBuf::from("second.onda");
        let watched = vec![first.clone(), second.clone()];

        assert_eq!(
            watcher_gap_validation_paths(&watched, &watched, &watched),
            watched,
            "every uncovered path must be checked across watcher recreation"
        );
        assert_eq!(
            watcher_gap_validation_paths(
                &[first.clone(), second.clone()],
                std::slice::from_ref(&first),
                &[],
            ),
            vec![second.clone()],
            "new paths must be checked even after complete previous coverage"
        );
        assert_eq!(
            watcher_gap_validation_paths(&watched, &watched, std::slice::from_ref(&first)),
            vec![first],
            "covered paths must rely on revisions instead of redundant disk reads"
        );
    }

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
        let _watcher = FileWatcher::watch(
            std::slice::from_ref(&watched),
            SourceWatchRevision::default(),
            move |_, _| {
                let _ = tx.send(());
            },
        )
        .expect("watcher should start");

        replace_file(&watched, "outs:\n  out1\nsample:\n  out1 = 1.0\n");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("first replace should trigger");

        replace_file(&watched, "outs:\n  out1\nsample:\n  out1 = 2.0\n");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("second replace should trigger");

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn file_watcher_reports_changes_to_transitive_sources() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_dependency_watch_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let dependency = temp_root.join("dependency.onda");
        fs::write(&entry, "import dependency\n").expect("write entry");
        fs::write(&dependency, "const value = 1.0\n").expect("write dependency");

        let paths = vec![entry, dependency.clone()];
        let (tx, rx) = mpsc::channel();
        let revision = SourceWatchRevision::default();
        let _watcher = FileWatcher::watch(&paths, revision.clone(), move |paths, _| {
            let _ = tx.send(paths);
        })
        .expect("watcher should start");

        replace_file(&dependency, "const value = 22.0\n");
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("dependency replace should trigger")
                .contains(&dependency),
            "dependency event should include the replaced path"
        );
        assert_ne!(revision.current(), 0, "a mutation must stale the snapshot");

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn file_watcher_reports_project_entry_changes() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_project_entry_watch_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        let source_dir = temp_root.join("code");
        fs::create_dir_all(&source_dir).expect("create source directory");
        let manifest = temp_root.join("project.ondaproject");
        let entry = source_dir.join("main.onda");
        fs::write(&manifest, "{\"entry\":\"code/main.onda\"}\n").expect("write manifest");
        fs::write(&entry, "outs:\n  out1\nsample:\n  out1 = 0.0\n").expect("write entry");

        let project = onda_project::resolve_project_watch_paths(
            &manifest,
            onda_project::ProjectLimits::default(),
        )
        .expect("resolve project watch paths");
        let snapshot = source_snapshot_with_project(&project.entry, &[], Some(&project));
        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::watch(
            &snapshot.watch_paths(),
            SourceWatchRevision::default(),
            move |paths, _| {
                let _ = tx.send(paths);
            },
        )
        .expect("project watcher should start");

        replace_file(&entry, "outs:\n  out1\nsample:\n  out1 = 1.0\n");
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .expect("project entry replacement should trigger")
                .contains(&entry),
            "project entry event should include the changed source"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn file_watcher_ignores_read_only_accesses() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_access_watch_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let watched = temp_root.join("run.onda");
        fs::write(&watched, "outs:\n  out1\nsample:\n  out1 = 0.0\n").expect("write file");

        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::watch(
            std::slice::from_ref(&watched),
            SourceWatchRevision::default(),
            move |_, _| {
                let _ = tx.send(());
            },
        )
        .expect("watcher should start");

        fs::read(&watched).expect("read watched file");
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "reading a source must not schedule a source rescan"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn source_change_filter_keeps_mutations_only() {
        use notify::event::{AccessKind, AccessMode, DataChange, Flag, MetadataKind, ModifyKind};
        use notify::EventKind;

        let path = PathBuf::from("source.onda");
        let event = |kind| Ok(notify::Event::new(kind).add_path(path.clone()));

        assert_eq!(
            source_change_paths(event(EventKind::Access(AccessKind::Read))),
            None
        );
        assert_eq!(
            source_change_paths(event(EventKind::Access(AccessKind::Close(
                AccessMode::Read
            )))),
            None
        );
        assert_eq!(
            source_change_paths(event(EventKind::Access(AccessKind::Close(
                AccessMode::Write
            )))),
            Some(vec![path.clone()])
        );
        assert!(
            source_change_paths(event(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::AccessTime
            ))))
            .is_none()
        );
        assert_eq!(
            source_change_paths(event(EventKind::Other)),
            Some(vec![path.clone()])
        );
        assert_eq!(
            source_change_paths(Ok(notify::Event::new(EventKind::Other)
                .add_path(path.clone())
                .set_flag(Flag::Rescan))),
            Some(vec![path.clone()])
        );
        assert_eq!(
            source_change_paths(event(EventKind::Modify(ModifyKind::Data(
                DataChange::Content
            )))),
            Some(vec![path])
        );
    }

    #[test]
    fn source_change_filter_drops_unrelated_mutations_before_debouncing() {
        use notify::event::{DataChange, ModifyKind};
        use notify::EventKind;

        let watched = PathBuf::from("sources/main.onda");
        let sibling = PathBuf::from("sources/build.log");
        let relevant = |path: &Path| super::paths_overlap(path, &watched);
        let event = Ok(notify::Event::new(EventKind::Modify(ModifyKind::Data(
            DataChange::Content,
        )))
        .add_path(sibling));
        let coverage_lost = std::sync::atomic::AtomicBool::new(false);

        assert_eq!(
            relevant_source_change_paths(event, &[], &relevant, &coverage_lost),
            None,
            "unrelated sibling activity must not extend the source debounce window"
        );
        assert!(!coverage_lost.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn watcher_errors_permanently_downgrade_coverage() {
        let root = PathBuf::from("sources");
        let coverage_lost = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            relevant_source_change_paths(
                Err(notify::Error::generic("watch subscription was lost")),
                std::slice::from_ref(&root),
                &|_| true,
                &coverage_lost,
            ),
            Some(vec![root])
        );
        assert!(
            coverage_lost.load(std::sync::atomic::Ordering::Acquire),
            "a backend error must keep the watcher on disk-validation fallback"
        );
    }

    #[test]
    fn file_watcher_keeps_valid_roots_when_another_root_cannot_be_watched() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_partial_watch_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let missing_root = temp_root.join("missing");
        let roots = HashMap::from([
            (temp_root.clone(), notify::RecursiveMode::NonRecursive),
            (missing_root.clone(), notify::RecursiveMode::NonRecursive),
        ]);

        let watcher = FileWatcher::watch_roots(roots, SourceWatchRevision::default(), |_, _| {})
            .expect("one invalid root must not disable a valid watch root");
        assert!(
            !watcher.covers_all_paths(),
            "partial watcher coverage must retain disk-validation fallback"
        );
        assert_eq!(watcher.fallback_validation_paths(), [missing_root]);

        drop(watcher);
        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(unix)]
    #[test]
    fn source_watch_root_stops_before_a_symlink_directory() {
        use std::os::unix::fs::symlink;

        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_symlink_watch_root_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        let assets = temp_root.join("assets");
        let target = temp_root.join("target");
        fs::create_dir_all(&assets).expect("create assets directory");
        fs::create_dir_all(&target).expect("create target directory");
        let alias = assets.join("linked");
        symlink(&target, &alias).expect("create directory symlink");

        let watched = alias.join("sample.wav");
        assert_eq!(
            source_watch_root(&watched),
            (assets.clone(), notify::RecursiveMode::Recursive),
            "the watcher must observe replacement of the rejected alias"
        );

        fs::remove_file(&alias).expect("remove symlink");
        fs::create_dir(&alias).expect("replace symlink with a real directory");
        assert_eq!(
            source_watch_root(&watched),
            (alias, notify::RecursiveMode::NonRecursive),
            "the watcher may move inward after the path becomes symlink-free"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn source_snapshot_replaces_on_success_and_unions_on_failure() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_source_manifest_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let dependency = temp_root.join("dependency.onda");
        fs::write(&entry, "import dependency\n").expect("write entry");
        fs::write(&dependency, "const value = 1.0\n").expect("write dependency");
        let entry = fs::canonicalize(entry).expect("canonical entry");
        let dependency = fs::canonicalize(dependency).expect("canonical dependency");

        let initial = source_snapshot(&entry, &[]);
        assert_eq!(initial.paths(), vec![entry.clone(), dependency.clone()]);

        fs::write(&entry, "this is not valid onda\nimport dependency\n").expect("break entry");
        let failed = source_snapshot(&entry, &initial.paths());
        assert_eq!(
            failed.paths(),
            initial.paths(),
            "failed loads should retain previous sources"
        );

        fs::write(&entry, "outs 1\nsample:\n  out1 = 0.0\n").expect("remove dependency");
        let recovered = source_snapshot(&entry, &failed.paths());
        assert_eq!(
            recovered.paths(),
            vec![entry.clone()],
            "successful loads should replace the watch set"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn source_snapshot_detects_dependency_changes_during_compilation() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_compile_generation_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let dependency = temp_root.join("dependency.onda");
        fs::write(&entry, "import dependency\n").expect("write entry");
        fs::write(&dependency, "const value = 1.0\n").expect("write dependency");
        let entry = fs::canonicalize(entry).expect("canonical entry");

        let launched = source_snapshot(&entry, &[]);
        assert!(launched.matches_disk());
        fs::write(&dependency, "const value = 2.0\n").expect("change dependency");
        assert!(
            !launched.matches_disk(),
            "known-source comparison should reject a stale child without reparsing"
        );
        let ready = source_snapshot(&entry, &launched.paths());

        assert_ne!(
            ready, launched,
            "a child compiled from the launched snapshot must not be accepted"
        );
        assert!(!SourceCompilationState::Compiling(launched).matches(&ready));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[cfg(unix)]
    #[test]
    fn source_snapshot_rejects_identical_symlink_replacements() {
        use std::os::unix::fs::symlink;

        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_symlink_replacement_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let target = temp_root.join("target.onda");
        let source = "outs 1\nsample:\n  out1 = 0.0\n";
        fs::write(&entry, source).expect("write entry");
        fs::write(&target, source).expect("write identical target");
        let entry = fs::canonicalize(entry).expect("canonical entry");
        let snapshot = source_snapshot(&entry, &[]);

        fs::remove_file(&entry).expect("remove regular entry");
        symlink(&target, &entry).expect("replace entry with symlink");

        assert!(
            !snapshot.matches_disk_changes(std::slice::from_ref(&entry)),
            "a symlink replacement must invalidate even when its target has identical contents"
        );
        assert!(
            super::read_file_without_symlinks(&entry).is_none(),
            "snapshot recovery must not read through the replacement symlink"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn project_assets_are_fingerprinted_without_joining_the_source_graph() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_project_asset_snapshot_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let manifest = temp_root.join("project.ondaproject");
        let asset = temp_root.join("sample.ondabuffer");
        fs::write(&entry, "outs 1\nsample:\n  out1 = 0.0\n").expect("write entry");
        fs::write(&manifest, "{\"entry\":\"main.onda\"}\n").expect("write manifest");
        fs::write(&asset, [1_u8, 2, 3]).expect("write asset");
        let entry = fs::canonicalize(entry).expect("canonical entry");
        let project = onda_project::ProjectWatchPaths {
            manifest: fs::canonicalize(manifest).expect("canonical manifest"),
            entry: entry.clone(),
            assets: vec![fs::canonicalize(&asset).expect("canonical asset")],
        };

        let initial = source_snapshot_with_project(&entry, &[], Some(&project));
        assert_eq!(initial.paths(), vec![entry.clone()]);
        assert_eq!(
            initial.watch_paths(),
            vec![
                entry.clone(),
                project.manifest.clone(),
                project.assets[0].clone(),
            ]
        );
        assert_eq!(initial.assets.len(), 1);
        assert!(initial.assets[0].digest.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let target = temp_root.join("identical.ondabuffer");
            fs::write(&target, [1_u8, 2, 3]).expect("write identical asset target");
            fs::remove_file(&asset).expect("remove regular asset");
            symlink(&target, &asset).expect("replace asset with symlink");
            assert!(
                super::digest_file(&project.assets[0]).is_none(),
                "asset recovery must not hash through a replacement symlink"
            );
            assert!(
                !initial.matches_disk_changes(std::slice::from_ref(&project.assets[0])),
                "an asset symlink replacement must invalidate even with an identical digest"
            );
            fs::remove_file(&asset).expect("remove asset symlink");
            fs::write(&asset, [1_u8, 2, 3]).expect("restore regular asset");
        }

        fs::write(&asset, [3_u8, 2, 1]).expect("change asset without changing its length");
        assert!(
            initial.matches_disk_changes(std::slice::from_ref(&entry)),
            "a source-only event should not hash unrelated project assets"
        );
        assert!(
            !initial.matches_disk_changes(std::slice::from_ref(&project.assets[0])),
            "an asset event must validate the changed asset"
        );
        let source_only_refresh = super::source_snapshot_from_paths(&entry, &initial.paths())
            .with_project_reusing(
                Some(&project),
                &initial.assets,
                std::slice::from_ref(&entry),
            );
        assert_eq!(
            source_only_refresh.assets, initial.assets,
            "a source event must reuse unrelated project asset fingerprints"
        );
        let targeted_asset_refresh = super::source_snapshot_from_paths(&entry, &initial.paths())
            .with_project_reusing(
                Some(&project),
                &initial.assets,
                std::slice::from_ref(&project.assets[0]),
            );
        assert_ne!(
            targeted_asset_refresh.assets, initial.assets,
            "an asset event must refresh the affected fingerprint"
        );
        let changed = source_snapshot_with_project(&entry, &initial.paths(), Some(&project));
        assert_ne!(
            changed, initial,
            "asset content changes must invalidate the run"
        );
        assert_eq!(changed.paths(), vec![entry]);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn missing_project_entry_creation_invalidates_the_snapshot() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_missing_project_entry_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(temp_root.join("code")).expect("create source directory");
        let manifest = temp_root.join("project.ondaproject");
        fs::write(&manifest, "{\"entry\":\"code/main.onda\"}\n").expect("write manifest");
        let project = onda_project::resolve_project_watch_paths(
            &manifest,
            onda_project::ProjectLimits::default(),
        )
        .expect("missing project entry should remain watchable");
        let launch_path = fs::canonicalize(&manifest).expect("canonical manifest");
        let (run_entry, run_project) = super::resolve_run_input_paths(&launch_path)
            .expect("run initialization must retain a missing project entry");
        assert_eq!(run_entry, project.entry);
        assert_eq!(run_project, Some(project.clone()));

        let initial = source_snapshot_with_project(&project.entry, &[], Some(&project));
        assert!(!initial.load_succeeded);
        assert!(initial.paths().contains(&project.entry));
        assert!(initial.matches_disk());

        fs::write(&project.entry, "outs 1\nsample:\n  out1 = 0.0\n").expect("create project entry");
        assert!(
            !initial.matches_disk_changes(std::slice::from_ref(&project.entry)),
            "creating the missing entry must invalidate the failed snapshot"
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn missing_nested_dependency_creation_invalidates_the_snapshot() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_missing_dependency_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        let dependency = temp_root.join("dsp/filter.onda");
        fs::write(&entry, "import dsp/filter\n").expect("write entry");
        let entry = fs::canonicalize(entry).expect("canonical entry");

        let failed = source_snapshot(&entry, &[]);
        assert!(!failed.load_succeeded);
        assert!(
            failed.paths().contains(&dependency),
            "the unresolved .onda candidate should be watched"
        );

        let (tx, rx) = mpsc::channel();
        let _watcher = FileWatcher::watch(
            &failed.paths(),
            SourceWatchRevision::default(),
            move |paths, _| {
                let _ = tx.send(paths);
            },
        )
        .expect("watcher should start");

        fs::create_dir_all(dependency.parent().expect("dependency parent"))
            .expect("create dependency directory");
        fs::write(&dependency, "const value = 1.0\n").expect("create dependency");
        rx.recv_timeout(Duration::from_secs(5))
            .expect("nested dependency creation should trigger a rescan");

        let recovered = source_snapshot(&entry, &failed.paths());
        assert!(recovered.load_succeeded);
        assert!(!SourceCompilationState::Compiling(failed).matches(&recovered));

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
    fn cleared_buffers_replay_as_clear_commands_after_restart() {
        let binding = PreservedBufferBinding::Cleared;
        let (command, payload, pending) = binding.replay_request("sample");

        assert_eq!(command, "clearBuffer");
        assert_eq!(payload, json!({ "name": "sample" }));
        assert!(matches!(
            pending,
            PendingCommand::ClearBuffer { name } if name == "sample"
        ));
    }

    #[test]
    fn source_compilation_state_tracks_only_the_current_child() {
        let temp_root = std::env::temp_dir().join(format!(
            "onda_run_source_cache_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should advance")
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let entry = temp_root.join("main.onda");
        fs::write(&entry, "sample:\n  out1 = 0.0\n").expect("write entry");
        let entry = fs::canonicalize(entry).expect("canonical entry");

        let compiled = source_snapshot(&entry, &[]);
        let mut state = SourceCompilationState::Ready(compiled.clone());
        assert!(state.matches(&compiled));

        fs::write(&entry, "sample:\n  out1 = 1.0\n").expect("change entry");
        let pending = source_snapshot(&entry, &compiled.paths());
        state = SourceCompilationState::Compiling(pending.clone());
        assert!(state.matches(&pending));
        assert!(
            !state.matches(&compiled),
            "starting a new child must forget the source state of the killed child"
        );
        state.mark_failed();
        assert!(
            state.matches(&pending),
            "a failed child must retain its source baseline"
        );
        assert!(
            state.ready().is_none(),
            "a failed child must never be reused"
        );

        fs::write(&entry, "sample:\n  out1 = 0.0\n").expect("revert entry");
        let reverted = source_snapshot(&entry, &pending.paths());
        assert_eq!(reverted, compiled);
        assert!(
            !state.matches(&reverted),
            "reverting must replace the in-flight child after the old child was killed"
        );

        let _ = fs::remove_dir_all(temp_root);
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
        let mut curved = base.clone();
        curved["curve"] = json!(-4.0);
        assert!(
            !params_are_compatible_for_preservation(&base, &curved),
            "changed curve should reset preserved param"
        );
    }

    #[test]
    fn preserved_events_are_dropped_when_an_argument_signature_changes() {
        let old_events = vec![run_event(
            "load",
            vec![run_event_arg("samples", "f32", json!(1.0))],
        )];
        let new_events = vec![run_event(
            "load",
            vec![run_event_arg("samples", "f32[2]", json!([1.0, 2.0]))],
        )];
        let mut preserved = vec![("load".to_owned(), vec![json!(0.5)])];

        reconcile_preserved_events(&mut preserved, &old_events, &new_events);

        assert!(
            preserved.is_empty(),
            "changed event argument shape should reset preserved values"
        );
    }

    #[test]
    fn event_preservation_requires_the_same_ordered_argument_signature() {
        let base = run_event(
            "load",
            vec![
                run_event_arg("gain", "f32", json!(1.0)),
                run_event_arg("enabled", "bool", json!(true)),
            ],
        );
        assert!(events_are_compatible_for_preservation(&base, &base));
        assert!(!events_are_compatible_for_preservation(
            &base,
            &run_event(
                "load",
                vec![
                    run_event_arg("enabled", "bool", json!(true)),
                    run_event_arg("gain", "f32", json!(1.0)),
                ],
            )
        ));
        assert!(!events_are_compatible_for_preservation(
            &base,
            &run_event(
                "load",
                vec![
                    run_event_arg("gain", "f32", json!(0.5)),
                    run_event_arg("enabled", "bool", json!(true)),
                ],
            )
        ));
    }

    fn run_event(name: &str, args: Vec<Value>) -> Value {
        json!({
            "name": name,
            "args": args,
        })
    }

    fn run_event_arg(name: &str, ty: &str, default: Value) -> Value {
        json!({
            "name": name,
            "type": ty,
            "default": default,
            "value": default,
        })
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
            "scale": null,
            "curve": null,
            "unit": null,
            "step": null,
            "stepCount": null,
            "scalar": scalar,
        })
    }
}
