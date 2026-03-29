use std::path::Path;

use omni_preview::PreviewHostOptions;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod platform {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use omni_preview::{PreviewController, PreviewHostOptions, PreviewState};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{Rect, WebViewBuilder};

    const PREVIEW_HTML: &str = include_str!("../../../editors/shared/preview.html");

    #[derive(Debug)]
    enum UserEvent {
        WebviewMessage(String),
        FileDialogResult {
            buffer_name: String,
            path: Option<PathBuf>,
        },
    }

    pub fn run_preview_window(omni_path: &Path, options: PreviewHostOptions) -> Result<(), String> {
        let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
        let proxy = event_loop.create_proxy();

        let window = WindowBuilder::new()
            .with_title(format!(
                "Omni - {}",
                omni_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ))
            .with_inner_size(tao::dpi::LogicalSize::new(480.0_f64, 720.0))
            .build(&event_loop)
            .map_err(|e| format!("failed to create window: {e}"))?;

        let init_script = r#"window.__hostBridge = { mode: "wry" };"#;
        let ipc_proxy = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_html(PREVIEW_HTML)
            .with_initialization_script(init_script)
            .with_ipc_handler(move |msg| {
                let _ = ipc_proxy.send_event(UserEvent::WebviewMessage(msg.body().to_owned()));
            })
            .build(&window)
            .map_err(|e| format!("failed to create webview: {e}"))?;

        let initial_logical_size = window.inner_size().to_logical::<u32>(window.scale_factor());
        let _ = webview.set_bounds(Rect {
            position: tao::dpi::LogicalPosition::new(0, 0).into(),
            size: tao::dpi::LogicalSize::new(
                initial_logical_size.width,
                initial_logical_size.height,
            )
            .into(),
        });

        let mut controller = PreviewController::new(omni_path, options)?;
        let mut pending_state_sync = true;

        event_loop.run(move |event, _target, control_flow| {
            *control_flow =
                ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(16));

            if controller.poll() {
                pending_state_sync = true;
            }

            match event {
                Event::WindowEvent {
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    let logical_size = size.to_logical::<u32>(window.scale_factor());
                    let _ = webview.set_bounds(Rect {
                        position: tao::dpi::LogicalPosition::new(0, 0).into(),
                        size: tao::dpi::LogicalSize::new(logical_size.width, logical_size.height)
                            .into(),
                    });
                }
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    *control_flow = ControlFlow::Exit;
                }
                Event::UserEvent(UserEvent::WebviewMessage(raw)) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&raw) {
                        handle_webview_message(&msg, &webview, &mut controller, &proxy);
                        pending_state_sync = true;
                    }
                }
                Event::UserEvent(UserEvent::FileDialogResult { buffer_name, path }) => {
                    if let Some(path) = path {
                        let file_path = path.display().to_string();
                        controller.bind_buffer_file(&buffer_name, &file_path);
                        pending_state_sync = true;
                    }
                }
                _ => {}
            }

            if pending_state_sync {
                sync_panel_state(&webview, controller.state());
                pending_state_sync = false;
            }
        });
    }

    fn handle_webview_message(
        msg: &serde_json::Value,
        webview: &wry::WebView,
        controller: &mut PreviewController,
        proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
    ) {
        let msg_type = msg
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match msg_type {
            "webviewReady" => sync_panel_state(webview, controller.state()),
            "start" => {
                let _ = controller.start();
            }
            "stop" => controller.stop(),
            "reset" => controller.reset(),
            "refreshDevices" => controller.refresh_devices(),
            "setParam" => {
                if let (Some(name), Some(value)) = (
                    msg.get("name").and_then(|value| value.as_str()),
                    msg.get("value"),
                ) {
                    controller.set_param(name, value.clone());
                }
            }
            "triggerEvent" => {
                if let Some(name) = msg.get("name").and_then(|value| value.as_str()) {
                    let values = msg
                        .get("values")
                        .and_then(|value| value.as_array())
                        .cloned()
                        .unwrap_or_default();
                    controller.trigger_event(name, values);
                }
            }
            "setInputDevice" => {
                let _ =
                    controller.set_input_device(msg.get("name").and_then(|value| value.as_str()));
            }
            "setOutputDevice" => {
                let _ =
                    controller.set_output_device(msg.get("name").and_then(|value| value.as_str()));
            }
            "chooseBufferFile" => {
                if let Some(name) = msg.get("name").and_then(|value| value.as_str()) {
                    let buffer_name = name.to_owned();
                    let dialog_proxy = proxy.clone();
                    std::thread::spawn(move || {
                        let path = rfd::FileDialog::new()
                            .add_filter("Wave Audio", &["wav"])
                            .set_title(format!("Bind '{buffer_name}' buffer"))
                            .pick_file();
                        let _ = dialog_proxy
                            .send_event(UserEvent::FileDialogResult { buffer_name, path });
                    });
                }
            }
            "bindBufferFile" => {
                if let (Some(name), Some(file_path)) = (
                    msg.get("name").and_then(|value| value.as_str()),
                    msg.get("filePath").and_then(|value| value.as_str()),
                ) {
                    controller.bind_buffer_file(name, file_path);
                }
            }
            "clearBuffer" => {
                if let Some(name) = msg.get("name").and_then(|value| value.as_str()) {
                    controller.clear_buffer(name);
                }
            }
            _ => {}
        }
    }

    fn sync_panel_state(webview: &wry::WebView, state: &PreviewState) {
        let panel_state = serde_json::json!({
            "running": state.running,
            "connected": state.connected,
            "path": state.path,
            "status": state.status,
            "error": state.error,
            "outputChannels": state.output_channels,
            "buffers": state.buffers,
            "events": state.events,
            "params": state.params,
            "inputDevices": state.input_devices,
            "outputDevices": state.output_devices,
            "currentInputDevice": state.current_input_device,
            "currentOutputDevice": state.current_output_device,
        });
        send_to_webview(webview, "state", &panel_state);

        let scope_message = serde_json::json!({
            "type": "scopeData",
            "channels": state.scope_channels,
            "samples": state.scope_samples,
        });
        eval_js(
            webview,
            &format!("if(window._onHostMessage)window._onHostMessage({scope_message})"),
        );
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
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use platform::run_preview_window;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn run_preview_window(_omni_path: &Path, _options: PreviewHostOptions) -> Result<(), String> {
    Err("webview preview host is unavailable on this platform/build".to_owned())
}
