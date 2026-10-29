//! A touch drag captures either its control or its original scrolling region.
use crate::{
    model::RunUi,
    scene::{Rect, Target},
    Input,
};

#[derive(Clone, Copy)]
pub(crate) enum ScrollRegion {
    Document,
    Log,
    Popup,
    Menu,
}

enum Mode {
    Pending,
    Control,
    Pan {
        region: ScrollRegion,
        horizontal: bool,
    },
}
pub(crate) struct Touch {
    id: u64,
    start: [f32; 2],
    previous: [f32; 2],
    region: ScrollRegion,
    numeric: bool,
    mode: Mode,
}
impl RunUi {
    pub(crate) fn scroll_region(&self, x: f32, y: f32) -> ScrollRegion {
        if self.interaction.chooser.is_some() && Rect(self.scene.menu_rect).contains(x, y) {
            ScrollRegion::Menu
        } else if self.interaction.devices_open && Rect(self.scene.popup_rect).contains(x, y) {
            ScrollRegion::Popup
        } else if Rect(self.scene.log_rect).contains(x, y) {
            ScrollRegion::Log
        } else {
            ScrollRegion::Document
        }
    }
    pub(crate) fn scroll_region_by(&mut self, region: ScrollRegion, delta: f32) -> bool {
        if !delta.is_finite() {
            return false;
        }
        let moved = match region {
            ScrollRegion::Menu => {
                let Some(menu) = &self.interaction.chooser else {
                    return false;
                };
                let max = (menu.id.spec(self).choices.len() as f32 * 28.0 + 8.0
                    - self.scene.menu_rect[3])
                    .max(0.0);
                let menu = self.interaction.chooser.as_mut().unwrap();
                let scroll = (menu.scroll + delta).clamp(0.0, max);
                if menu.scroll == scroll {
                    return false;
                }
                menu.scroll = scroll;
                true
            }
            ScrollRegion::Popup => {
                let max = (self.scene.popup_height - self.scene.popup_rect[3]).max(0.0);
                let scroll = (self.interaction.popup_scroll + delta).clamp(0.0, max);
                if self.interaction.popup_scroll == scroll {
                    return false;
                }
                self.interaction.popup_scroll = scroll;
                true
            }
            ScrollRegion::Log => {
                let max = self.scene.log_max_scroll;
                let old = if self.presentation.log_follow {
                    max
                } else {
                    self.presentation.log_scroll
                };
                let scroll = (old + delta).clamp(0.0, max);
                let follow = scroll >= max;
                if scroll == old && follow == self.presentation.log_follow {
                    return false;
                }
                self.presentation.log_scroll = scroll;
                self.presentation.log_follow = follow;
                self.presentation.scrolled();
                scroll != old
            }
            ScrollRegion::Document => {
                let max = (self.scene.content_height - self.scene.viewport_height).max(0.0);
                let scroll = (self.scroll + delta).clamp(0.0, max);
                if self.scroll == scroll {
                    return false;
                }
                self.scroll = scroll;
                self.presentation.scrolled();
                true
            }
        };
        self.invalidate();
        moved
    }
    pub(crate) fn pan_log_by(&mut self, delta: f32) -> bool {
        if !delta.is_finite() {
            return false;
        }
        let scroll = (self.presentation.log_x + delta).clamp(0.0, self.log_max_scroll_x);
        if self.presentation.log_x == scroll {
            return false;
        }
        self.presentation.log_x = scroll;
        self.presentation.scrolled();
        self.invalidate();
        true
    }
    pub(crate) fn touch_start(&mut self, id: u64, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let target = self.target(x, y);
        if let Some(Target::Note(note)) = target {
            self.commands
                .extend(self.interaction.midi.touch_note(id, Some(note), false));
            self.invalidate();
            return;
        }
        // Non-piano gestures have one owner; extra fingers cannot steal a drag.
        if self.interaction.touch.is_some()
            || self.pointer_captured()
            || self.interaction.midi.pointer_down
        {
            return;
        }
        let control = matches!(
            target,
            Some(Target::Control(_) | Target::Note(_) | Target::Scrollbar)
        );
        let numeric = matches!(&target, Some(Target::Field(field)) if self.field_numeric(field));
        let touch = Touch {
            id,
            start: [x, y],
            previous: [x, y],
            region: self.scroll_region(x, y),
            numeric,
            mode: if control {
                Mode::Control
            } else {
                Mode::Pending
            },
        };
        self.input(Input::PointerMove { x, y, fine: false });
        if control {
            self.input(Input::PointerDown { x, y, clicks: 1 });
        }
        self.interaction.touch = Some(touch);
    }
    pub(crate) fn touch_move(&mut self, id: u64, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        if self.interaction.midi.touches.contains_key(&id) {
            let note = match self.target(x, y) {
                Some(Target::Note(n)) => Some(n),
                _ => None,
            };
            self.commands
                .extend(self.interaction.midi.touch_note(id, note, false));
            self.invalidate();
            return;
        }
        if self.interaction.touch.as_ref().is_none_or(|t| t.id != id) {
            return;
        }
        let Some(mut touch) = self.interaction.touch.take() else {
            return;
        };
        let dx = x - touch.start[0];
        let dy = y - touch.start[1];
        if matches!(touch.mode, Mode::Pending) {
            if dx.hypot(dy) < 6.0 {
                self.interaction.touch = Some(touch);
                return;
            }
            if touch.numeric && dx.abs() > dy.abs() {
                self.input(Input::PointerDown {
                    x: touch.start[0],
                    y: touch.start[1],
                    clicks: 1,
                });
                touch.mode = Mode::Control;
            } else {
                touch.mode = Mode::Pan {
                    region: touch.region,
                    horizontal: matches!(touch.region, ScrollRegion::Log) && dx.abs() > dy.abs(),
                };
            }
        }
        match touch.mode {
            Mode::Control => {
                self.input(Input::PointerMove { x, y, fine: false });
            }
            Mode::Pan {
                region,
                horizontal: false,
            } => {
                self.scroll_region_by(region, touch.previous[1] - y);
            }
            Mode::Pan {
                horizontal: true, ..
            } => {
                self.pan_log_by(touch.previous[0] - x);
            }
            Mode::Pending => {}
        }
        touch.previous = [x, y];
        self.interaction.touch = Some(touch);
    }
    pub(crate) fn touch_cancel(&mut self, id: u64) {
        if self.interaction.midi.touches.contains_key(&id) {
            self.commands
                .extend(self.interaction.midi.touch_note(id, None, true));
            self.invalidate();
        }
        if self.interaction.touch.as_ref().is_some_and(|t| t.id == id) {
            self.interaction.touch = None;
            self.cancel_gesture();
            self.cancel_number();
            self.interaction.scrollbar_drag = None;
            self.invalidate();
        }
    }
    pub(crate) fn touch_end(&mut self, id: u64) {
        if self.interaction.midi.touches.contains_key(&id) {
            self.touch_cancel(id);
            return;
        }
        if self.interaction.touch.as_ref().is_none_or(|t| t.id != id) {
            return;
        }
        let Some(touch) = self.interaction.touch.take() else {
            return;
        };
        if matches!(touch.mode, Mode::Pending) {
            self.input(Input::PointerDown {
                x: touch.start[0],
                y: touch.start[1],
                clicks: 1,
            });
        }
        if matches!(touch.mode, Mode::Pending | Mode::Control) {
            self.input(Input::PointerUp);
        }
    }
    pub fn pointer_stops(&mut self, x: f32, y: f32) -> bool {
        self.prepare_scene();
        self.state["running"] == true && self.target(x, y) == Some(Target::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wheel_consumption_tracks_movement_at_both_document_edges() {
        let mut ui = RunUi::default();
        ui.resize(360.0, 400.0, 1.0);
        ui.message(json!({"type":"state","state":{
            "path":"scroll.onda","supportsScope":false
        }}));
        assert!(
            !ui.scroll(200.0, false),
            "a fitting document must yield to its host"
        );
        ui.message(json!({"type":"state","state":{"params":(0..80)
            .map(|i|json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>()}}));
        assert!(!ui.scroll(-200.0, false));
        assert!(ui.scroll(200.0, false));
        assert!(!ui.scroll(0.0, false));
        assert!(!ui.scroll(f32::NAN, false));
        assert!(!ui.scroll(200.0, true));
        assert!(ui.scroll(100_000.0, false));
        assert!(!ui.scroll(200.0, false));
        assert!(ui.scroll(-100_000.0, false));
        assert!(!ui.scroll(-200.0, false));
    }

    #[test]
    fn following_a_short_log_does_not_consume_a_wheel_without_movement() {
        let mut ui = RunUi::default();
        ui.resize(720.0, 840.0, 1.0);
        ui.message(json!({"type":"state","state":{
            "path":"log.onda","supportsScope":false,"logText":"One line","logRevealed":true
        }}));
        ui.presentation.log_follow = false;
        ui.prepare_scene();
        let r = ui.scene.log_rect;
        ui.interaction.pointer = [r[0] + 20.0, r[1] + 20.0];
        assert!(!ui.scroll(200.0, false));
        assert!(ui.presentation.log_follow);
    }
}
