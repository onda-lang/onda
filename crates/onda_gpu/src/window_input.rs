//! Native pointer, keyboard and IME translation; shared editing remains in onda_ui.
use super::*;
use winit::{
    dpi::PhysicalPosition,
    window::{ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData},
};

impl App {
    pub(super) fn keyboard_input(&mut self, event: winit::event::KeyEvent) {
        if event.state.is_pressed()
            && self.composing
            && !self.modifiers.control_key()
            && !self.modifiers.meta_key()
        {
            return;
        }
        if event.state == ElementState::Pressed
            && (self.modifiers.control_key() || self.modifiers.meta_key())
            && matches!(&event.logical_key, WinitKey::Character(c) if c.eq_ignore_ascii_case("v"))
        {
            if self.ui.editing() {
                match self
                    .clipboard()
                    .and_then(|clipboard| clipboard.get_text().map_err(|e| e.to_string()))
                {
                    Ok(text) => self.input(Input::Text { text }),
                    Err(error) => {
                        self.action_error(format!("Could not paste clipboard text: {error}"))
                    }
                }
            }
            self.redraw_if_dirty();
            return;
        }
        if let Some(c) = physical_letter(event.physical_key) {
            if event.state == ElementState::Released
                || (!self.modifiers.control_key() && !self.modifiers.meta_key() && !event.repeat)
            {
                self.input(Input::ComputerKey {
                    key: c.into(),
                    pressed: event.state == ElementState::Pressed,
                });
            }
        }
        if event.state == ElementState::Released {
            return;
        }
        if self.transport_shortcut(&event.logical_key, event.repeat) {
            return;
        }

        let key = match &event.logical_key {
            WinitKey::Named(n) => match n {
                NamedKey::Enter => Some(Key::Enter),
                NamedKey::Escape => Some(Key::Escape),
                NamedKey::Tab => Some(Key::Tab),
                NamedKey::Backspace => Some(Key::Backspace),
                NamedKey::Delete => Some(Key::Delete),
                NamedKey::ArrowLeft => Some(Key::ArrowLeft),
                NamedKey::ArrowRight => Some(Key::ArrowRight),
                NamedKey::ArrowUp => Some(Key::ArrowUp),
                NamedKey::ArrowDown => Some(Key::ArrowDown),
                NamedKey::Home => Some(Key::Home),
                NamedKey::End => Some(Key::End),
                _ => None,
            },
            WinitKey::Character(c)
                if (self.modifiers.control_key() || self.modifiers.meta_key())
                    && c.eq_ignore_ascii_case("a") =>
            {
                Some(Key::SelectAll)
            }
            WinitKey::Character(c)
                if (self.modifiers.control_key() || self.modifiers.meta_key())
                    && (c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("x")) =>
            {
                Some(if c.eq_ignore_ascii_case("c") {
                    Key::Copy
                } else {
                    Key::Cut
                })
            }
            _ => None,
        };
        if let Some(key) = key {
            self.input(Input::Key {
                key,
                shift: self.modifiers.shift_key(),
            });
        } else if !self.modifiers.control_key() && !self.modifiers.meta_key() {
            if let Some(text) = event.text {
                self.input(Input::Text {
                    text: text.to_string(),
                });
            }
        }
    }

    pub(super) fn pointer_moved(
        &mut self,
        position: PhysicalPosition<f64>,
        source: PointerSource,
        primary: bool,
        scale: f32,
    ) {
        let x = position.x as f32 / scale;
        let y = position.y as f32 / scale;
        if let PointerSource::Touch { finger_id, .. } = source {
            self.input(Input::TouchMove {
                id: finger_id.into_raw() as u64,
                x,
                y,
            });
        } else if primary {
            self.pointer = [x, y];
            self.input(Input::PointerMove {
                x,
                y,
                fine: self.modifiers.shift_key(),
            });
        }
    }

    pub(super) fn pointer_button(
        &mut self,
        position: PhysicalPosition<f64>,
        button: ButtonSource,
        state: ElementState,
        primary: bool,
        scale: f32,
    ) {
        let x = position.x as f32 / scale;
        let y = position.y as f32 / scale;
        if let ButtonSource::Touch { finger_id, .. } = button {
            let id = finger_id.into_raw() as u64;
            self.input(if state.is_pressed() {
                Input::TouchStart { id, x, y }
            } else {
                Input::TouchEnd { id }
            });
        } else if primary && button.mouse_button() == Some(MouseButton::Left) {
            self.pointer = [x, y];
            if state.is_pressed() {
                let now = Instant::now();
                let double = self.last_click.is_some_and(|(t, p)| {
                    now.duration_since(t) < Duration::from_millis(350)
                        && (p[0] - x).abs() < 4.0
                        && (p[1] - y).abs() < 4.0
                });
                self.last_click = if double {
                    None
                } else {
                    Some((now, self.pointer))
                };
                self.input(Input::PointerDown {
                    x,
                    y,
                    clicks: if double { 2 } else { 1 },
                });
            } else {
                self.input(Input::PointerUp);
            }
        }
    }

    pub(super) fn sync_ime(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let editing = self.ui.editing();
        let rect = self
            .ui
            .text_edit()
            .and_then(|edit| {
                edit["rect"]
                    .as_array()
                    .map(|r| std::array::from_fn::<_, 4, _>(|i| r[i].as_f64().unwrap_or(0.0)))
            })
            .unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let data = ImeRequestData::default().with_cursor_area(
            winit::dpi::LogicalPosition::new(rect[0], rect[1]).into(),
            winit::dpi::LogicalSize::new(rect[2], rect[3]).into(),
        );
        let request = if editing && !self.ime_enabled {
            let capabilities = ImeCapabilities::new().with_cursor_area();
            ImeRequest::Enable(
                ImeEnableRequest::new(capabilities, data).expect("cursor area supplied"),
            )
        } else if editing {
            ImeRequest::Update(data)
        } else if self.ime_enabled {
            ImeRequest::Disable
        } else {
            return;
        };
        self.ime_enabled = editing;
        let _ = window.request_ime_update(request);
    }
}
