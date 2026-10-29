//! Native event-loop adapter for the shared GPU run screen.

use onda_run::{RunController, RunHostOptions, RunThemeMode};
use onda_ui::{Input, Key, RunUi, SurfaceRenderer};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
use winit::{
    application::ApplicationHandler,
    event::{
        ButtonSource, ElementState, Ime, MouseButton, MouseScrollDelta, PointerKind, PointerSource,
        WindowEvent,
    },
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as WinitKey, KeyCode, NamedKey, PhysicalKey},
    window::{Window, WindowAttributes, WindowId},
};

mod commands;
mod file_drop;
#[cfg(test)]
mod tests;
mod window_input;

const SCOPE_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

pub fn run(path: Option<&Path>, options: RunHostOptions) -> Result<(), String> {
    let mut event_loop = event_loop_builder().build().map_err(|e| e.to_string())?;
    let mut app = App::new(path, options);
    app.proxy = Some(event_loop.create_proxy());
    event_loop
        .run_app_on_demand(&mut app)
        .map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

fn event_loop_builder() -> winit::event_loop::EventLoopBuilder {
    EventLoop::builder()
}

struct App {
    // Drop the surface before its owning window.
    gpu: Option<SurfaceRenderer>,
    file_drop: file_drop::FileDrop,
    window: Option<Arc<dyn Window>>,
    clipboard: Option<arboard::Clipboard>,
    ui: RunUi,
    controller: Option<RunController>,
    path: Option<PathBuf>,
    options: RunHostOptions,
    default_param_smoothing_seconds: f64,
    error: Option<String>,
    controller_woken: bool,
    proxy: Option<winit::event_loop::EventLoopProxy>,
    rendered: Option<(u64, u64)>,
    surface_size: [u32; 2],
    redraw_retry: Option<Instant>,
    pointer: [f32; 2],
    modifiers: winit::keyboard::ModifiersState,
    occluded: bool,
    last_click: Option<(Instant, [f32; 2])>,
    views: std::collections::BTreeMap<String, serde_json::Value>,
    view_restore_id: u64,
    composing: bool,
    ime_enabled: bool,
}
impl App {
    fn new(path: Option<&Path>, options: RunHostOptions) -> Self {
        let mut app = Self {
            window: None,
            gpu: None,
            file_drop: Default::default(),
            clipboard: None,
            ui: RunUi::default(),
            controller: None,
            path: path.map(Path::to_path_buf),
            default_param_smoothing_seconds: options.param_smoothing_seconds,
            options,
            error: None,
            controller_woken: true,
            proxy: None,
            rendered: None,
            surface_size: [0; 2],
            redraw_retry: None,
            pointer: [0.0; 2],
            modifiers: Default::default(),
            occluded: false,
            last_click: None,
            views: Default::default(),
            view_restore_id: 0,
            composing: false,
            ime_enabled: false,
        };
        app.publish();
        app
    }
    fn theme(&self) -> &'static str {
        match self.options.theme {
            RunThemeMode::Light => "light",
            RunThemeMode::Dark => "dark",
            RunThemeMode::Auto => {
                if self.window.as_ref().and_then(|w| w.theme()) == Some(winit::window::Theme::Light)
                {
                    "light"
                } else {
                    "dark"
                }
            }
        }
    }
    fn panel_state(&self) -> serde_json::Value {
        let theme = self.theme();
        let mut state = onda_run::host_panel_state(
            self.controller.as_ref(),
            None,
            &self.options,
            theme,
            self.default_param_smoothing_seconds,
        );
        state["supportsViewState"] = json!(true);
        let path = state["path"].as_str().unwrap_or("");
        if let Some(view) = self.views.get(path) {
            state["viewState"] = view.clone();
            state["viewState"]["readyId"] = json!(self.view_restore_id);
        } else {
            state["paramLayout"] = json!("knobs");
        }
        state
    }
    fn publish(&mut self) {
        let state = self.panel_state();
        self.ui.message(json!({"type":"state","state":state}));
        self.publish_scope();
    }
    fn publish_scope(&mut self) {
        if let Some(controller) = &self.controller {
            if let Some(envelope) = controller.scope_envelope() {
                self.ui.set_scope_envelope(envelope);
            } else {
                let state = controller.state();
                self.ui
                    .set_scope_interleaved(state.scope_channels, &state.scope_samples);
            }
        } else {
            self.ui.set_scope_interleaved(0, &[]);
        }
    }
    fn publish_changes(&mut self, changes: onda_run::PollResult) {
        if changes.schema_changed {
            self.publish();
            return;
        }
        if let Some(controller) = &self.controller {
            if changes.state_changed {
                self.ui.message(json!({"type":"state",
                    "state":onda_run::runtime_panel_state(controller.state())}));
            }
            if changes.log_changed {
                self.ui.message(json!({"type":"state",
                    "state":onda_run::log_panel_state(controller.state())}));
            }
        }
        if changes.scope_changed || changes.state_changed {
            self.publish_scope();
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.file_drop.cancel(&mut self.ui);
        if let Some(controller) = &self.controller {
            self.options = controller.options().clone();
        }
        match RunController::new(&path, self.options.clone()) {
            Ok(mut controller) => {
                if let Some(proxy) = self.proxy.clone() {
                    controller.set_wakeup(move || {
                        proxy.wake_up();
                    });
                }
                self.controller_woken = true;
                controller.set_scope_poll_interval(SCOPE_INTERVAL);
                controller.set_scope_columns(self.ui.scope_columns());
                controller.set_scope_visible(self.surface_visible() && self.ui.scope_enabled());
                self.controller = Some(controller);
                self.path = Some(path);
                // A new controller must restore even when its source path is unchanged.
                self.view_restore_id = self.view_restore_id.wrapping_add(1);
                self.publish();
            }
            Err(error) => self.action_error(error),
        }
    }
    fn action_error(&mut self, error: String) {
        self.ui
            .message(json!({"type":"state","state":{"error":error}}));
    }
    fn transport_shortcut(&mut self, key: &WinitKey, repeat: bool) -> bool {
        if !self.modifiers.control_key() && !self.modifiers.meta_key() {
            return false;
        }
        let key = match key {
            WinitKey::Named(NamedKey::Enter) => Key::Play,
            WinitKey::Character(c) if c.eq_ignore_ascii_case("s") => Key::Play,
            WinitKey::Character(c) if c == "." => Key::Stop,
            _ => return false,
        };
        if !repeat {
            self.input(Input::Key {
                key,
                shift: self.modifiers.shift_key(),
            });
        }
        true
    }
    fn input(&mut self, input: Input) {
        if let Some(gpu) = &mut self.gpu {
            gpu.renderer.input(&mut self.ui, input);
        } else {
            self.ui.input(input);
        }
        self.commands();
        let cursor = match self.ui.cursor() {
            "ew-resize" => winit::cursor::CursorIcon::EwResize,
            "ns-resize" => winit::cursor::CursorIcon::NsResize,
            "text" => winit::cursor::CursorIcon::Text,
            "pointer" => winit::cursor::CursorIcon::Pointer,
            _ => winit::cursor::CursorIcon::Default,
        };
        if let Some(window) = &self.window {
            window.set_cursor(cursor.into());
        }
        self.sync_ime();
        self.sync_scope_visibility();
        self.redraw_if_dirty();
    }
    fn surface_visible(&self) -> bool {
        !self.occluded && !self.surface_size.contains(&0)
    }
    fn sync_scope_visibility(&mut self) {
        let visible = self.surface_visible() && self.ui.scope_enabled();
        if let Some(c) = &mut self.controller {
            c.set_scope_visible(visible);
        }
    }
    fn resize_surface(&mut self, width: u32, height: u32, scale: f32) {
        self.surface_size = [width, height];
        self.redraw_retry = None;
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(width, height);
        }
        self.ui
            .resize(width as f32 / scale, height as f32 / scale, scale);
        if let Some(c) = &mut self.controller {
            c.set_scope_columns(self.ui.scope_columns());
        }
        self.sync_scope_visibility();
        self.rendered = None;
    }
    fn redraw_ready(&self, now: Instant) -> bool {
        self.surface_visible() && self.redraw_retry.is_none_or(|deadline| now >= deadline)
    }
    fn needs_redraw(&self, now: Instant) -> bool {
        self.redraw_ready(now) && self.rendered != Some(self.ui.revisions())
    }
    fn next_wake(&self) -> Option<Instant> {
        let poll = self
            .controller
            .as_ref()
            .map(RunController::next_poll_deadline);
        let retry = self.redraw_retry.filter(|_| self.surface_visible());
        poll.into_iter()
            .chain(retry)
            .chain(self.ui.view_state_deadline())
            .chain(self.ui.repaint_deadline())
            .min()
    }
    fn redraw_if_dirty(&self) {
        if self.needs_redraw(Instant::now()) {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
    }
}
impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let result = (|| {
            let window: Arc<dyn Window> = Arc::from(
                event_loop
                    .create_window(
                        WindowAttributes::default()
                            .with_title("Onda Run")
                            .with_surface_size(winit::dpi::LogicalSize::new(720.0, 840.0))
                            .with_min_surface_size(winit::dpi::LogicalSize::new(360.0, 400.0)),
                    )
                    .map_err(|e| e.to_string())?,
            );
            let size = window.surface_size();
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
                Box::new(event_loop.owned_display_handle()),
            ));
            let surface = instance
                .create_surface(window.clone())
                .map_err(|e| e.to_string())?;
            self.gpu = Some(pollster::block_on(SurfaceRenderer::new(
                instance,
                surface,
                size.width,
                size.height,
            ))?);
            let scale = window.scale_factor() as f32;
            self.resize_surface(size.width, size.height, scale);
            self.window = Some(window);
            self.publish();
            if let Some(path) = self.path.clone() {
                self.open(path);
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            self.error = Some(error);
            event_loop.exit();
        }
    }
    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let scale = window.scale_factor() as f32;
        if self.file_drop_event(event_loop, &event, scale) {
            self.redraw_if_dirty();
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::SurfaceResized(size) => {
                self.resize_surface(size.width, size.height, scale);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.surface_size();
                self.resize_surface(size.width, size.height, scale);
            }
            WindowEvent::RedrawRequested => {
                if !self.redraw_ready(Instant::now()) {
                    return;
                }
                // Exposure can require presentation even when the UI is clean.
                self.rendered = None;
                if let Some(g) = &mut self.gpu {
                    match g.render(&mut self.ui, scale) {
                        Ok(true) => {
                            self.rendered = Some(self.ui.revisions());
                            self.redraw_retry = None;
                        }
                        Ok(false) => self.redraw_retry = Some(Instant::now() + SCOPE_INTERVAL),
                        Err(error) => {
                            self.error = Some(error);
                            event_loop.exit();
                        }
                    }
                }
            }
            WindowEvent::PointerMoved {
                position,
                source,
                primary,
                ..
            } => self.pointer_moved(position, source, primary, scale),
            WindowEvent::PointerButton {
                position,
                button,
                state,
                primary,
                ..
            } => self.pointer_button(position, button, state, primary, scale),
            WindowEvent::PointerLeft {
                kind: PointerKind::Touch(id),
                ..
            } => self.input(Input::TouchCancel {
                id: id.into_raw() as u64,
            }),
            WindowEvent::PointerLeft { primary: true, .. } => self.input(Input::PointerLeave),
            WindowEvent::MouseWheel { delta, .. } => {
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * 40.0, -y * 40.0),
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32 / scale, -p.y as f32 / scale),
                    _ => return,
                };
                if self.modifiers.shift_key() || x.abs() > y.abs() {
                    self.input(Input::HorizontalScroll {
                        delta: if self.modifiers.shift_key() { y } else { x },
                    });
                } else {
                    self.input(Input::Scroll { delta: y });
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Focused(false) => {
                self.composing = false;
                self.input(Input::FocusLost);
            }
            WindowEvent::Ime(Ime::Preedit(text, selection)) => {
                self.composing = !text.is_empty();
                let (a, b) = selection.unwrap_or((text.len(), text.len()));
                let utf16 = |i: usize| text.get(..i).unwrap_or("").encode_utf16().count();
                self.input(Input::Preedit {
                    anchor: utf16(a),
                    cursor: utf16(b),
                    text,
                });
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.composing = false;
                self.input(Input::Text { text });
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.composing = false;
                self.input(Input::Preedit {
                    text: String::new(),
                    anchor: 0,
                    cursor: 0,
                });
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.sync_scope_visibility();
                if !occluded {
                    self.rendered = None;
                    self.redraw_retry = None;
                }
            }
            WindowEvent::ThemeChanged(_) => self.publish(),
            WindowEvent::KeyboardInput { event, .. } => self.keyboard_input(event),
            _ => {}
        }
        self.redraw_if_dirty();
    }
    fn proxy_wake_up(&mut self, _event_loop: &dyn ActiveEventLoop) {
        self.controller_woken = true;
    }
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.sync_scope_visibility();
        if std::mem::take(&mut self.controller_woken)
            || self
                .controller
                .as_ref()
                .is_some_and(|c| Instant::now() >= c.next_poll_deadline())
        {
            if let Some(c) = &mut self.controller {
                let poll = c.poll();
                self.publish_changes(poll);
            }
        }
        self.commands();
        self.ui.tick();
        self.redraw_if_dirty();
        event_loop.set_control_flow(
            self.next_wake()
                .map_or(ControlFlow::Wait, ControlFlow::WaitUntil),
        );
    }
}

fn physical_letter(key: PhysicalKey) -> Option<&'static str> {
    Some(match key {
        PhysicalKey::Code(KeyCode::KeyA) => "a",
        PhysicalKey::Code(KeyCode::KeyW) => "w",
        PhysicalKey::Code(KeyCode::KeyS) => "s",
        PhysicalKey::Code(KeyCode::KeyE) => "e",
        PhysicalKey::Code(KeyCode::KeyD) => "d",
        PhysicalKey::Code(KeyCode::KeyF) => "f",
        PhysicalKey::Code(KeyCode::KeyT) => "t",
        PhysicalKey::Code(KeyCode::KeyG) => "g",
        PhysicalKey::Code(KeyCode::KeyY) => "y",
        PhysicalKey::Code(KeyCode::KeyH) => "h",
        PhysicalKey::Code(KeyCode::KeyU) => "u",
        PhysicalKey::Code(KeyCode::KeyJ) => "j",
        PhysicalKey::Code(KeyCode::KeyK) => "k",
        PhysicalKey::Code(KeyCode::KeyO) => "o",
        PhysicalKey::Code(KeyCode::KeyL) => "l",
        PhysicalKey::Code(KeyCode::KeyP) => "p",
        PhysicalKey::Code(KeyCode::KeyZ) => "z",
        PhysicalKey::Code(KeyCode::KeyX) => "x",
        _ => return None,
    })
}
