mod ipc;
mod process;
mod watcher;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::{Rect, WebViewBuilder};

use ipc::IpcBridge;
use process::{ChildSession, ReadyEvent};
use watcher::FileWatcher;

const PREVIEW_HTML: &str = include_str!("../../../editors/shared/preview.html");
const SCOPE_MAX_FRAMES: usize = 1024;
const SCOPE_POLL_INTERVAL_MS: u64 = 50;

/// Options for the preview window.
#[derive(Clone, Debug)]
pub struct PreviewWindowOptions {
    pub sample_rate_hz: u32,
    pub block_frames: usize,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub fast_math: bool,
    pub omni_bin: String,
}

impl Default for PreviewWindowOptions {
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

/// Custom events sent to the tao event loop from background threads.
#[derive(Debug)]
enum UserEvent {
    /// The child process emitted a ready event.
    ChildReady(ReadyEvent),
    /// The child process exited.
    ChildExited {
        code: Option<i32>,
        error: Option<String>,
    },
    /// A message arrived from the webview JS.
    WebviewMessage(String),
    /// The watched file changed — trigger a restart.
    FileChanged,
    /// A TCP response line arrived from the control socket.
    TcpResponse(String),
    /// A native file dialog completed.
    FileDialogResult {
        buffer_name: String,
        path: Option<PathBuf>,
    },
}

/// Launch the standalone preview window for the given `.omni` file.
///
/// This function blocks until the window is closed.
pub fn run_preview_window(omni_path: &Path, options: PreviewWindowOptions) -> Result<(), String> {
    let omni_path = std::fs::canonicalize(omni_path)
        .map_err(|e| format!("cannot resolve path {}: {e}", omni_path.display()))?;

    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title(format!(
            "Omni \u{2013} {}",
            omni_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ))
        .with_inner_size(tao::dpi::LogicalSize::new(480.0_f64, 720.0))
        .build(&event_loop)
        .map_err(|e| format!("failed to create window: {e}"))?;

    // Build the webview. We inject the host bridge config before the page script runs.
    let init_script = r#"window.__hostBridge = { mode: "wry" };"#;

    let ipc_proxy = proxy.clone();
    let ipc_bridge = IpcBridge::new();
    let webview_bridge = ipc_bridge.clone();
    let webview = WebViewBuilder::new()
        .with_html(PREVIEW_HTML)
        .with_initialization_script(init_script)
        .with_ipc_handler(move |msg| {
            let raw = msg.body().to_owned();
            let handled_directly = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|value| {
                    let ty = value.get("type").and_then(|item| item.as_str())?;
                    if ty != "setParam" {
                        return Some(false);
                    }
                    let commit = value
                        .get("commit")
                        .and_then(|item| item.as_bool())
                        .unwrap_or(false);
                    if commit {
                        return Some(false);
                    }
                    let name = value.get("name").and_then(|item| item.as_str())?;
                    let payload = serde_json::json!({
                        "name": name,
                        "value": value.get("value").cloned().unwrap_or(serde_json::Value::Null),
                    });
                    webview_bridge.send_command_notification("setParam", &payload);
                    Some(true)
                })
                .unwrap_or(false);
            if !handled_directly {
                let _ = ipc_proxy.send_event(UserEvent::WebviewMessage(raw));
            }
        })
        .build(&window)
        .map_err(|e| format!("failed to create webview: {e}"))?;

    let initial_logical_size = window.inner_size().to_logical::<u32>(window.scale_factor());
    let _ = webview.set_bounds(Rect {
        position: tao::dpi::LogicalPosition::new(0, 0).into(),
        size: tao::dpi::LogicalSize::new(initial_logical_size.width, initial_logical_size.height)
            .into(),
    });

    let input_devices = list_input_devices();
    let output_devices = list_output_devices();
    let mut current_options = options.clone();

    // Spawn the child process.
    let child_proxy = proxy.clone();
    let mut child = ChildSession::spawn(&omni_path, &current_options, child_proxy.clone())
        .map_err(|e| format!("failed to start preview subprocess: {e}"))?;

    // Set up the file watcher.
    let watcher_proxy = proxy.clone();
    let _watcher = FileWatcher::watch(&omni_path, move || {
        let _ = watcher_proxy.send_event(UserEvent::FileChanged);
    });

    // IPC bridge state.
    let bridge = ipc_bridge;

    // Preserved state for restarts.
    let mut preserved_params: Vec<(String, serde_json::Value)> = Vec::new();
    let mut preserved_buffers: Vec<(String, String)> = Vec::new();
    let mut current_params: Vec<serde_json::Value> = Vec::new();
    let mut current_buffers: Vec<serde_json::Value> = Vec::new();
    let mut current_output_channels = 0usize;
    let omni_path_clone = omni_path.clone();

    // Scope polling timer state.
    let mut scope_polling_active = false;
    let mut scope_polling_in_flight = false;
    let mut last_scope_poll = std::time::Instant::now();
    let scope_interval = Duration::from_millis(SCOPE_POLL_INTERVAL_MS);

    event_loop.run(move |event, _target, control_flow| {
        // Default: poll with a short sleep to drive scope updates.
        *control_flow = ControlFlow::WaitUntil(
            std::time::Instant::now() + Duration::from_millis(16),
        );

        if scope_polling_active
            && !scope_polling_in_flight
            && last_scope_poll.elapsed() >= scope_interval
        {
            bridge.send_command(
                "getScopeData",
                &serde_json::json!({ "maxFrames": SCOPE_MAX_FRAMES }),
            );
            scope_polling_in_flight = true;
            last_scope_poll = std::time::Instant::now();
        }

        match event {
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let logical_size = size.to_logical::<u32>(window.scale_factor());
                let _ = webview.set_bounds(Rect {
                    position: tao::dpi::LogicalPosition::new(0, 0).into(),
                    size: tao::dpi::LogicalSize::new(logical_size.width, logical_size.height).into(),
                });
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                child.kill();
                *control_flow = ControlFlow::Exit;
            }

            Event::UserEvent(user_event) => match user_event {
                UserEvent::ChildReady(ready) => {
                    // Connect to the TCP control socket.
                    let tcp_proxy = proxy.clone();
                    if let Err(e) = bridge.connect(ready.port, tcp_proxy) {
                        eprintln!("failed to connect to control socket: {e}");
                    }

                    current_params = ready.params;
                    apply_preserved_param_state(&mut current_params, &preserved_params);
                    current_buffers = ready.buffers;
                    apply_preserved_buffer_state(&mut current_buffers, &preserved_buffers);
                    current_output_channels = ready.output_channels;

                    sync_panel_state(
                        &webview,
                        true,
                        true,
                        &ready.path,
                        "Running",
                        None,
                        current_output_channels,
                        &current_buffers,
                        &current_params,
                        &input_devices,
                        &output_devices,
                        current_options.input_device.as_deref(),
                        current_options.output_device.as_deref(),
                    );

                    // Reapply preserved params.
                    for (name, value) in &preserved_params {
                        bridge.send_command_notification("setParam", &serde_json::json!({
                            "name": name,
                            "value": value,
                        }));
                    }
                    // Reapply preserved buffers.
                    for (name, path) in &preserved_buffers {
                        bridge.send_command("bindBufferWav", &serde_json::json!({
                            "name": name,
                            "path": path,
                        }));
                    }

                    scope_polling_active = true;
                    scope_polling_in_flight = false;
                    last_scope_poll = std::time::Instant::now();
                }

                UserEvent::ChildExited { code, error } => {
                    scope_polling_active = false;
                    scope_polling_in_flight = false;
                    bridge.disconnect();
                    sync_panel_state(
                        &webview,
                        false,
                        false,
                        &omni_path_clone.display().to_string(),
                        "Stopped",
                        error.or_else(|| code.filter(|c| *c != 0).map(|c| format!("exit code {c}"))),
                        0,
                        &[],
                        &[],
                        &input_devices,
                        &output_devices,
                        current_options.input_device.as_deref(),
                        current_options.output_device.as_deref(),
                    );
                }

                UserEvent::WebviewMessage(raw) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&raw) {
                        handle_webview_message(
                            &msg,
                            &webview,
                            &bridge,
                            &mut preserved_params,
                            &mut preserved_buffers,
                            &mut current_params,
                            &mut current_buffers,
                            current_output_channels,
                            &omni_path_clone,
                            &mut current_options,
                            &input_devices,
                            &output_devices,
                            &mut child,
                            &proxy,
                        );
                    }
                }

                UserEvent::FileChanged => {
                    // Restart the child process, preserving current state.
                    child.kill();
                    bridge.disconnect();
                    scope_polling_active = false;
                    scope_polling_in_flight = false;

                    sync_panel_state(
                        &webview,
                        false,
                        false,
                        &omni_path_clone.display().to_string(),
                        "Restarting...",
                        None,
                        0,
                        &[],
                        &[],
                        &input_devices,
                        &output_devices,
                        current_options.input_device.as_deref(),
                        current_options.output_device.as_deref(),
                    );

                    match ChildSession::spawn(
                        &omni_path_clone,
                        &current_options,
                        proxy.clone(),
                    ) {
                        Ok(new_child) => {
                            child = new_child;
                        }
                        Err(e) => {
                            eprintln!("failed to restart preview: {e}");
                            sync_panel_state(
                                &webview,
                                false,
                                false,
                                &omni_path_clone.display().to_string(),
                                "Failed to restart",
                                Some(e),
                                0,
                                &[],
                                &[],
                                &input_devices,
                                &output_devices,
                                current_options.input_device.as_deref(),
                                current_options.output_device.as_deref(),
                            );
                        }
                    }
                }

                UserEvent::TcpResponse(line) => {
                    // Forward scope data to webview if it's a scope response.
                    if let Ok(resp) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let Some(result) = resp.get("result") {
                            if result.get("channels").is_some() && result.get("samples").is_some()
                            {
                                scope_polling_in_flight = false;
                                let scope_msg = serde_json::json!({
                                    "type": "scopeData",
                                    "channels": result["channels"],
                                    "samples": result["samples"],
                                });
                                eval_js(
                                    &webview,
                                    &format!(
                                        "if(window._onHostMessage)window._onHostMessage({scope_msg})"
                                    ),
                                );
                            }
                        }
                    }
                }

                UserEvent::FileDialogResult { buffer_name, path } => {
                    if let Some(path) = path {
                        let path_str = path.display().to_string();
                        bridge.send_command("bindBufferWav", &serde_json::json!({
                            "name": buffer_name,
                            "path": path_str,
                        }));
                        // Update preserved buffers.
                        if let Some(entry) = preserved_buffers.iter_mut().find(|(n, _)| n == &buffer_name) {
                            entry.1 = path_str;
                        } else {
                            preserved_buffers.push((buffer_name, path_str));
                        }
                        apply_preserved_buffer_state(&mut current_buffers, &preserved_buffers);
                        sync_panel_state(
                            &webview,
                            true,
                            bridge.is_connected(),
                            &omni_path_clone.display().to_string(),
                            "Running",
                            None,
                            current_output_channels,
                            &current_buffers,
                            &current_params,
                            &input_devices,
                            &output_devices,
                            current_options.input_device.as_deref(),
                            current_options.output_device.as_deref(),
                        );
                    }
                }
            },

            _ => {}
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn handle_webview_message(
    msg: &serde_json::Value,
    webview: &wry::WebView,
    bridge: &IpcBridge,
    preserved_params: &mut Vec<(String, serde_json::Value)>,
    preserved_buffers: &mut Vec<(String, String)>,
    current_params: &mut Vec<serde_json::Value>,
    current_buffers: &mut Vec<serde_json::Value>,
    current_output_channels: usize,
    omni_path: &Path,
    options: &mut PreviewWindowOptions,
    input_devices: &[String],
    output_devices: &[String],
    child: &mut ChildSession,
    proxy: &EventLoopProxy<UserEvent>,
) {
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "webviewReady" => {
            if !current_params.is_empty()
                || !current_buffers.is_empty()
                || current_output_channels > 0
            {
                sync_panel_state(
                    webview,
                    true,
                    bridge.is_connected(),
                    &omni_path.display().to_string(),
                    "Running",
                    None,
                    current_output_channels,
                    current_buffers,
                    current_params,
                    input_devices,
                    output_devices,
                    options.input_device.as_deref(),
                    options.output_device.as_deref(),
                );
            } else {
                sync_panel_state(
                    webview,
                    false,
                    false,
                    &omni_path.display().to_string(),
                    "Starting...",
                    None,
                    0,
                    &[],
                    &[],
                    input_devices,
                    output_devices,
                    options.input_device.as_deref(),
                    options.output_device.as_deref(),
                );
            }
        }

        "start" => {
            // Restart the subprocess.
            child.kill();
            bridge.disconnect();
            match ChildSession::spawn(omni_path, options, proxy.clone()) {
                Ok(new_child) => {
                    *child = new_child;
                }
                Err(e) => {
                    eprintln!("failed to start preview: {e}");
                }
            }
        }

        "setInputDevice" => {
            let next = msg
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|name| input_devices.iter().any(|device| device == name))
                .map(str::to_owned);
            options.input_device = next;
            child.kill();
            bridge.disconnect();
            match ChildSession::spawn(omni_path, options, proxy.clone()) {
                Ok(new_child) => {
                    *child = new_child;
                }
                Err(e) => {
                    eprintln!("failed to restart preview after input device change: {e}");
                }
            }
        }

        "setOutputDevice" => {
            let next = msg
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|name| output_devices.iter().any(|device| device == name))
                .map(str::to_owned);
            options.output_device = next;
            child.kill();
            bridge.disconnect();
            match ChildSession::spawn(omni_path, options, proxy.clone()) {
                Ok(new_child) => {
                    *child = new_child;
                }
                Err(e) => {
                    eprintln!("failed to restart preview after output device change: {e}");
                }
            }
        }

        "stop" => {
            child.kill();
            bridge.disconnect();
            let _ = proxy.send_event(UserEvent::ChildExited {
                code: Some(0),
                error: None,
            });
        }

        "reset" => {
            preserved_params.clear();
            for param in current_params.iter_mut() {
                let Some(name) = param_name(param).map(str::to_owned) else {
                    continue;
                };
                let Some(default_value) = param_default_value(param) else {
                    continue;
                };
                set_param_value(param, default_value.clone());
                bridge.send_command_notification(
                    "setParam",
                    &serde_json::json!({ "name": name, "value": default_value }),
                );
            }
            sync_panel_state(
                webview,
                true,
                bridge.is_connected(),
                &omni_path.display().to_string(),
                "Running",
                None,
                current_output_channels,
                current_buffers,
                current_params,
                input_devices,
                output_devices,
                options.input_device.as_deref(),
                options.output_device.as_deref(),
            );
        }

        "setParam" => {
            if let (Some(name), Some(value)) =
                (msg.get("name").and_then(|v| v.as_str()), msg.get("value"))
            {
                bridge.send_command_notification(
                    "setParam",
                    &serde_json::json!({ "name": name, "value": value }),
                );
                // Preserve for restart.
                if let Some(entry) = preserved_params.iter_mut().find(|(n, _)| n == name) {
                    entry.1 = value.clone();
                } else {
                    preserved_params.push((name.to_owned(), value.clone()));
                }
                update_param_value(current_params, name, value.clone());
            }
        }

        "chooseBufferFile" => {
            if let Some(name) = msg.get("name").and_then(|v| v.as_str()) {
                let name = name.to_owned();
                let dialog_proxy = proxy.clone();
                std::thread::spawn(move || {
                    let path = rfd::FileDialog::new()
                        .add_filter("Wave Audio", &["wav"])
                        .set_title(format!("Bind '{name}' buffer"))
                        .pick_file();
                    let _ = dialog_proxy.send_event(UserEvent::FileDialogResult {
                        buffer_name: name,
                        path,
                    });
                });
            }
        }

        "bindBufferFile" => {
            if let (Some(name), Some(file_path)) = (
                msg.get("name").and_then(|v| v.as_str()),
                msg.get("filePath").and_then(|v| v.as_str()),
            ) {
                bridge.send_command(
                    "bindBufferWav",
                    &serde_json::json!({ "name": name, "path": file_path }),
                );
                if let Some(entry) = preserved_buffers.iter_mut().find(|(n, _)| n == name) {
                    entry.1 = file_path.to_owned();
                } else {
                    preserved_buffers.push((name.to_owned(), file_path.to_owned()));
                }
                update_buffer_loaded_path(current_buffers, name, Some(file_path));
                sync_panel_state(
                    webview,
                    true,
                    bridge.is_connected(),
                    &omni_path.display().to_string(),
                    "Running",
                    None,
                    current_output_channels,
                    current_buffers,
                    current_params,
                    input_devices,
                    output_devices,
                    options.input_device.as_deref(),
                    options.output_device.as_deref(),
                );
            }
        }

        "clearBuffer" => {
            if let Some(name) = msg.get("name").and_then(|v| v.as_str()) {
                bridge.send_command("clearBuffer", &serde_json::json!({ "name": name }));
                preserved_buffers.retain(|(n, _)| n != name);
                update_buffer_loaded_path(current_buffers, name, None);
                sync_panel_state(
                    webview,
                    true,
                    bridge.is_connected(),
                    &omni_path.display().to_string(),
                    "Running",
                    None,
                    current_output_channels,
                    current_buffers,
                    current_params,
                    input_devices,
                    output_devices,
                    options.input_device.as_deref(),
                    options.output_device.as_deref(),
                );
            }
        }

        _ => {}
    }
}

fn sync_panel_state(
    webview: &wry::WebView,
    running: bool,
    connected: bool,
    path: &str,
    status: &str,
    error: Option<String>,
    output_channels: usize,
    buffers: &[serde_json::Value],
    params: &[serde_json::Value],
    input_devices: &[String],
    output_devices: &[String],
    current_input_device: Option<&str>,
    current_output_device: Option<&str>,
) {
    let state = serde_json::json!({
        "running": running,
        "connected": connected,
        "path": path,
        "status": status,
        "error": error,
        "outputChannels": output_channels,
        "buffers": buffers,
        "params": params,
        "inputDevices": input_devices,
        "outputDevices": output_devices,
        "currentInputDevice": current_input_device,
        "currentOutputDevice": current_output_device,
    });
    send_to_webview(webview, "state", &state);
}

fn apply_preserved_param_state(
    params: &mut [serde_json::Value],
    preserved_params: &[(String, serde_json::Value)],
) {
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

fn apply_preserved_buffer_state(
    buffers: &mut [serde_json::Value],
    preserved_buffers: &[(String, String)],
) {
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

fn update_param_value(params: &mut [serde_json::Value], name: &str, value: serde_json::Value) {
    for param in params {
        if param_name(param) == Some(name) {
            set_param_value(param, value.clone());
            break;
        }
    }
}

fn update_buffer_loaded_path(
    buffers: &mut [serde_json::Value],
    name: &str,
    loaded_path: Option<&str>,
) {
    for buffer in buffers {
        if buffer_name(buffer) != Some(name) {
            continue;
        }
        if let Some(obj) = buffer.as_object_mut() {
            obj.insert(
                "loadedPath".to_owned(),
                loaded_path.map_or(serde_json::Value::Null, |path| {
                    serde_json::Value::String(path.to_owned())
                }),
            );
        }
        break;
    }
}

fn param_name(param: &serde_json::Value) -> Option<&str> {
    param.get("name").and_then(|value| value.as_str())
}

fn buffer_name(buffer: &serde_json::Value) -> Option<&str> {
    buffer.get("name").and_then(|value| value.as_str())
}

fn set_param_value(param: &mut serde_json::Value, value: serde_json::Value) {
    if let Some(obj) = param.as_object_mut() {
        obj.insert("value".to_owned(), value);
    }
}

fn param_default_value(param: &serde_json::Value) -> Option<serde_json::Value> {
    let default = param.get("default");
    if let Some(default) = default.filter(|value| !value.is_null()) {
        return Some(default.clone());
    }
    let range_min = param.get("rangeMin");
    if let Some(range_min) = range_min.filter(|value| !value.is_null()) {
        return Some(range_min.clone());
    }
    Some(match param.get("type").and_then(|value| value.as_str()) {
        Some("bool") => serde_json::Value::Bool(false),
        _ => serde_json::Value::Number(serde_json::Number::from(0)),
    })
}

fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.name().ok())
        .collect()
}

fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.name().ok())
        .collect()
}

fn send_to_webview(webview: &wry::WebView, msg_type: &str, payload: &serde_json::Value) {
    let msg = serde_json::json!({
        "type": msg_type,
        "state": payload,
    });
    eval_js(
        webview,
        &format!("if(window._onHostMessage)window._onHostMessage({msg})"),
    );
}

fn eval_js(webview: &wry::WebView, script: &str) {
    let _ = webview.evaluate_script(script);
}
