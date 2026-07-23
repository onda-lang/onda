#[cfg(target_os = "linux")]
use std::path::Path;

use onda_run::RunHostOptions;

#[cfg(any(target_os = "windows", target_os = "macos", test))]
fn update_run_settings(message: &serde_json::Value, options: &mut RunHostOptions) -> bool {
    let sample_rate_hz = message
        .get("sampleRateHz")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0);
    let block_frames = message
        .get("blockFrames")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0);
    if let (Some(sample_rate_hz), Some(block_frames)) = (sample_rate_hz, block_frames) {
        options.sample_rate_hz = sample_rate_hz;
        options.block_frames = block_frames;
        true
    } else {
        false
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod platform {
    use super::update_run_settings;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use image::ImageReader;
    use onda_run::{RunController, RunHostOptions, RunState, RunThemeMode};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder};
    use tao::window::{Icon, WindowBuilder};
    use wry::{DragDropEvent, Rect, WebViewBuilder};

    const RUN_HTML: &str = include_str!("../../../ui/run/run.html");
    const APP_ICON_DARK_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo-dark.png");
    const APP_ICON_LIGHT_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo.png");

    #[derive(Debug)]
    enum UserEvent {
        WebviewMessage(String),
        BufferFileDialogResult {
            buffer_name: String,
            path: Option<PathBuf>,
        },
        OndaFileDialogResult(Option<PathBuf>),
        OndaFileDropped(PathBuf),
    }

    pub fn run_run_window(
        onda_path: Option<&Path>,
        mut options: RunHostOptions,
    ) -> Result<(), String> {
        let event_loop: EventLoop<UserEvent> = EventLoopBuilder::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let run_theme = options.theme;
        let mut controller = onda_path
            .map(|path| RunController::new(path, options.clone()))
            .transpose()?;

        let window = WindowBuilder::new()
            .with_title(run_window_title(onda_path))
            .with_inner_size(tao::dpi::LogicalSize::new(480.0_f64, 720.0))
            .with_window_icon(Some(load_app_icon(startup_window_icon_is_dark(run_theme))?))
            .build(&event_loop)
            .map_err(|e| format!("failed to create window: {e}"))?;
        set_app_icon(&window, resolved_window_icon_is_dark(&window, run_theme));

        let theme_mode = web_theme_mode(run_theme);
        let init_script = format!(
            r#"
            window.__hostBridge = {{ mode: "wry", theme: "{theme_mode}" }};
            window.__ondaForcedTheme = "{theme_mode}";
            if ("{theme_mode}" !== "auto" && document && document.documentElement) {{
                document.documentElement.dataset.theme = "{theme_mode}";
            }}
            "#
        );
        let ipc_proxy = proxy.clone();
        let drop_proxy = proxy.clone();
        let webview = WebViewBuilder::new()
            .with_html(RUN_HTML)
            .with_initialization_script(&init_script)
            .with_ipc_handler(move |msg| {
                let _ = ipc_proxy.send_event(UserEvent::WebviewMessage(msg.body().to_owned()));
            })
            .with_drag_drop_handler(move |event| {
                if let DragDropEvent::Drop { paths, .. } = event {
                    if let Some(path) = paths.into_iter().find(|path| is_onda_path(path)) {
                        let _ = drop_proxy.send_event(UserEvent::OndaFileDropped(path));
                    }
                }
                true
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

        let mut load_error = None;
        let mut pending_state_sync = true;
        let mut pending_scope_sync = true;

        event_loop.run(move |event, _target, control_flow| {
            *control_flow =
                ControlFlow::WaitUntil(std::time::Instant::now() + Duration::from_millis(16));

            if let Some(controller) = controller.as_mut() {
                let poll = controller.poll();
                if poll.state_changed {
                    pending_state_sync = true;
                }
                if poll.scope_changed {
                    pending_scope_sync = true;
                }
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
                Event::WindowEvent {
                    event: WindowEvent::ThemeChanged(theme),
                    ..
                } => {
                    if matches!(run_theme, RunThemeMode::Auto) {
                        set_app_icon(&window, matches!(theme, tao::window::Theme::Dark));
                    }
                }
                Event::UserEvent(UserEvent::WebviewMessage(raw)) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if handle_webview_message(
                            &msg,
                            &webview,
                            &mut controller,
                            &mut load_error,
                            &mut options,
                            &proxy,
                            &window,
                            theme_mode,
                        ) {
                            pending_state_sync = true;
                        }
                    }
                }
                Event::UserEvent(UserEvent::BufferFileDialogResult { buffer_name, path }) => {
                    if let (Some(controller), Some(path)) = (controller.as_mut(), path) {
                        let file_path = path.display().to_string();
                        controller.bind_buffer_file(&buffer_name, &file_path);
                        pending_state_sync = true;
                    }
                }
                Event::UserEvent(UserEvent::OndaFileDialogResult(path)) => {
                    if let Some(path) = path {
                        load_run_controller(
                            &path,
                            &options,
                            &window,
                            &mut controller,
                            &mut load_error,
                        );
                        pending_state_sync = true;
                    }
                }
                Event::UserEvent(UserEvent::OndaFileDropped(path)) => {
                    if controller.is_none() {
                        load_run_controller(
                            &path,
                            &options,
                            &window,
                            &mut controller,
                            &mut load_error,
                        );
                        pending_state_sync = true;
                    }
                }
                _ => {}
            }

            if pending_state_sync {
                sync_host_state(
                    &webview,
                    controller.as_ref(),
                    load_error.as_deref(),
                    &options,
                    theme_mode,
                );
                pending_state_sync = false;
                pending_scope_sync = false;
            } else if pending_scope_sync {
                if let Some(controller) = controller.as_ref() {
                    sync_scope_state(&webview, controller.state());
                }
                pending_scope_sync = false;
            }
        });
    }

    fn run_window_title(path: Option<&Path>) -> String {
        path.and_then(Path::file_name)
            .map(|name| format!("Onda - {}", name.to_string_lossy()))
            .unwrap_or_else(|| "Onda".to_owned())
    }

    fn is_onda_path(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("onda"))
    }

    fn load_run_controller(
        path: &Path,
        options: &RunHostOptions,
        window: &tao::window::Window,
        controller: &mut Option<RunController>,
        load_error: &mut Option<String>,
    ) {
        if !is_onda_path(path) {
            *load_error = Some("Choose an .onda file".to_owned());
            return;
        }
        match RunController::new(path, options.clone()) {
            Ok(next_controller) => {
                window.set_title(&run_window_title(Some(next_controller.path())));
                *controller = Some(next_controller);
                *load_error = None;
            }
            Err(error) => *load_error = Some(error),
        }
    }

    fn handle_webview_message(
        msg: &serde_json::Value,
        webview: &wry::WebView,
        controller: &mut Option<RunController>,
        load_error: &mut Option<String>,
        options: &mut RunHostOptions,
        proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
        window: &tao::window::Window,
        theme_mode: &str,
    ) -> bool {
        let msg_type = msg
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match msg_type {
            "webviewReady" => {
                sync_host_state(
                    webview,
                    controller.as_ref(),
                    load_error.as_deref(),
                    options,
                    theme_mode,
                );
                false
            }
            "chooseOndaFile" => {
                let dialog_proxy = proxy.clone();
                std::thread::spawn(move || {
                    let path = rfd::FileDialog::new()
                        .add_filter("Onda source", &["onda"])
                        .set_title("Open an Onda file")
                        .pick_file();
                    let _ = dialog_proxy.send_event(UserEvent::OndaFileDialogResult(path));
                });
                false
            }
            "unload" => {
                if let Some(current) = controller.take() {
                    *options = current.options().clone();
                }
                *load_error = None;
                window.set_title(&run_window_title(None));
                true
            }
            "setRunSettings" => {
                if controller.is_some() {
                    return false;
                }
                update_run_settings(msg, options)
            }
            "start" => {
                if let Some(controller) = controller.as_mut() {
                    let _ = controller.start();
                }
                true
            }
            "stop" => {
                if let Some(controller) = controller.as_mut() {
                    controller.stop();
                }
                true
            }
            "reset" => {
                if let Some(controller) = controller.as_mut() {
                    controller.reset();
                }
                true
            }
            "refreshDevices" => {
                if let Some(controller) = controller.as_mut() {
                    controller.refresh_devices();
                }
                true
            }
            "setParam" => {
                if let (Some(name), Some(value)) = (
                    msg.get("name").and_then(|value| value.as_str()),
                    msg.get("value"),
                ) {
                    if let Some(controller) = controller.as_mut() {
                        controller.set_param(name, value.clone());
                    }
                }
                false
            }
            "triggerEvent" => {
                if let Some(name) = msg.get("name").and_then(|value| value.as_str()) {
                    let values = msg
                        .get("values")
                        .and_then(|value| value.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if let Some(controller) = controller.as_mut() {
                        controller.trigger_event(name, values);
                    }
                }
                false
            }
            "setInputDevice" => {
                if let Some(controller) = controller.as_mut() {
                    let _ = controller
                        .set_input_device(msg.get("name").and_then(|value| value.as_str()));
                }
                true
            }
            "setOutputDevice" => {
                if let Some(controller) = controller.as_mut() {
                    let _ = controller
                        .set_output_device(msg.get("name").and_then(|value| value.as_str()));
                }
                true
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
                            .send_event(UserEvent::BufferFileDialogResult { buffer_name, path });
                    });
                }
                false
            }
            "bindBufferFile" => {
                if let (Some(name), Some(file_path)) = (
                    msg.get("name").and_then(|value| value.as_str()),
                    msg.get("filePath").and_then(|value| value.as_str()),
                ) {
                    if let Some(controller) = controller.as_mut() {
                        controller.bind_buffer_file(name, file_path);
                    }
                }
                true
            }
            "clearBuffer" => {
                if let Some(name) = msg.get("name").and_then(|value| value.as_str()) {
                    if let Some(controller) = controller.as_mut() {
                        controller.clear_buffer(name);
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn sync_host_state(
        webview: &wry::WebView,
        controller: Option<&RunController>,
        load_error: Option<&str>,
        options: &RunHostOptions,
        theme_mode: &str,
    ) {
        if let Some(controller) = controller {
            sync_panel_state(webview, controller.state(), options, theme_mode);
            return;
        }
        let panel_state = serde_json::json!({
            "running": false,
            "connected": false,
            "path": "",
            "status": "No file selected",
            "error": load_error,
            "outputChannels": 0,
            "buffers": [],
            "events": [],
            "params": [],
            "inputDevices": [],
            "outputDevices": [],
            "currentInputDevice": null,
            "currentOutputDevice": null,
            "sampleRateHz": options.sample_rate_hz,
            "blockFrames": options.block_frames,
            "themeMode": theme_mode,
        });
        send_to_webview(webview, "state", &panel_state);
        send_empty_scope_state(webview);
    }

    fn sync_panel_state(
        webview: &wry::WebView,
        state: &RunState,
        options: &RunHostOptions,
        theme_mode: &str,
    ) {
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
            "sampleRateHz": options.sample_rate_hz,
            "blockFrames": options.block_frames,
            "themeMode": theme_mode,
        });
        send_to_webview(webview, "state", &panel_state);

        sync_scope_state(webview, state);
    }

    fn sync_scope_state(webview: &wry::WebView, state: &RunState) {
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

    fn send_empty_scope_state(webview: &wry::WebView) {
        let scope_message = serde_json::json!({
            "type": "scopeData",
            "channels": 0,
            "samples": [],
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

    fn load_app_icon(is_dark: bool) -> Result<Icon, String> {
        let image = ImageReader::new(std::io::Cursor::new(app_icon_png(is_dark)))
            .with_guessed_format()
            .map_err(|err| format!("failed to detect webview app icon format: {err}"))?
            .decode()
            .map_err(|err| format!("failed to decode webview app icon: {err}"))?
            .into_rgba8();
        let width = image.width();
        let height = image.height();
        Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|err| format!("failed to build webview app icon: {err}"))
    }

    fn app_icon_png(is_dark: bool) -> &'static [u8] {
        if is_dark {
            APP_ICON_DARK_PNG
        } else {
            APP_ICON_LIGHT_PNG
        }
    }

    fn set_app_icon(window: &tao::window::Window, is_dark: bool) {
        if let Ok(icon) = load_app_icon(is_dark) {
            window.set_window_icon(Some(icon));
        }
        #[cfg(target_os = "macos")]
        set_macos_app_icon(app_icon_png(is_dark));
    }

    #[cfg(target_os = "macos")]
    fn set_macos_app_icon(png_bytes: &[u8]) {
        use objc2::AnyThread;
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::NSData;

        extern "C" {
            static NSApp: Option<&'static NSApplication>;
        }

        let data = NSData::with_bytes(png_bytes);
        let image = NSImage::initWithData(NSImage::alloc(), &data);
        unsafe {
            if let Some(app) = NSApp {
                app.setApplicationIconImage(image.as_deref());
            }
        }
    }

    fn startup_window_icon_is_dark(theme: RunThemeMode) -> bool {
        match theme {
            RunThemeMode::Dark => true,
            RunThemeMode::Light => false,
            RunThemeMode::Auto => true,
        }
    }

    fn resolved_window_icon_is_dark(window: &tao::window::Window, theme: RunThemeMode) -> bool {
        match theme {
            RunThemeMode::Dark => true,
            RunThemeMode::Light => false,
            RunThemeMode::Auto => matches!(window.theme(), tao::window::Theme::Dark),
        }
    }

    fn web_theme_mode(theme: RunThemeMode) -> &'static str {
        match theme {
            RunThemeMode::Auto => "auto",
            RunThemeMode::Dark => "dark",
            RunThemeMode::Light => "light",
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use platform::run_run_window;

#[cfg(target_os = "linux")]
pub fn run_run_window(_onda_path: Option<&Path>, _options: RunHostOptions) -> Result<(), String> {
    Err("webview run host is unavailable on this platform/build".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{update_run_settings, RunHostOptions};

    #[test]
    fn run_settings_update_together_and_reject_invalid_values() {
        let mut options = RunHostOptions::default();
        assert!(update_run_settings(
            &serde_json::json!({
                "sampleRateHz": 96_000,
                "blockFrames": 512,
            }),
            &mut options,
        ));
        assert_eq!(options.sample_rate_hz, 96_000);
        assert_eq!(options.block_frames, 512);

        assert!(!update_run_settings(
            &serde_json::json!({
                "sampleRateHz": 0,
                "blockFrames": 128,
            }),
            &mut options,
        ));
        assert_eq!(options.sample_rate_hz, 96_000);
        assert_eq!(options.block_frames, 512);
    }
}
