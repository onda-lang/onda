//! Full-label disclosure shares the renderer across native and browser hosts.
use crate::{
    model::RunUi,
    scene::{Palette, Rect, Scene},
};
use web_time::{Duration, Instant};

const HOVER_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, PartialEq)]
pub(crate) struct Tooltip {
    rect: Rect,
    text: String,
}

pub(crate) struct Hover {
    tooltip: Tooltip,
    deadline: Instant,
    visible: bool,
}

impl RunUi {
    /// Advance timed UI feedback before checking whether a frame is needed.
    pub fn tick(&mut self) {
        self.prepare_scene();
    }

    /// Next timed redraw. Hosts may sleep until this deadline while idle.
    pub fn repaint_deadline(&self) -> Option<Instant> {
        self.interaction
            .hover
            .as_ref()
            .filter(|h| !h.visible)
            .map(|h| h.deadline)
    }

    pub(crate) fn update_hover(&mut self, now: Instant) -> bool {
        let candidate = if self.pointer_captured()
            || self.editor.editing()
            || self.interaction.devices_open
            || self.interaction.chooser.is_some()
            || self.interaction.touch.is_some()
            || self.interaction.midi.pointer_down
        {
            None
        } else {
            self.scene
                .tooltip_at(self.interaction.pointer[0], self.interaction.pointer[1])
                .map(|i| &self.scene.tooltips[i])
        };
        if self.interaction.hover.as_ref().map(|h| &h.tooltip) != candidate {
            self.interaction.hover = candidate.cloned().map(|tooltip| Hover {
                tooltip,
                deadline: now + HOVER_DELAY,
                visible: false,
            });
        }
        if let Some(hover) = &mut self.interaction.hover {
            hover.visible |= now >= hover.deadline;
        }
        self.scene.tooltip_label.is_some()
            != self.interaction.hover.as_ref().is_some_and(|h| h.visible)
    }
}

impl Scene {
    pub fn tooltip(&mut self, rect: [f32; 4], text: String) {
        let rect = Rect(rect).intersect(Rect(self.clip));
        if rect.0[2] > 0.0 && rect.0[3] > 0.0 {
            self.tooltips.push(Tooltip { rect, text });
        }
    }

    pub fn tooltip_at(&self, x: f32, y: f32) -> Option<usize> {
        self.tooltips.iter().rposition(|t| t.rect.contains(x, y))
    }
}

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette) {
    let Some(hover) = ui.interaction.hover.as_ref().filter(|h| h.visible) else {
        return;
    };
    let Some(index) = s.tooltip_at(ui.interaction.pointer[0], ui.interaction.pointer[1]) else {
        return;
    };
    let Tooltip {
        rect: Rect(anchor),
        text,
    } = &s.tooltips[index];
    if s.tooltips[index] != hover.tooltip {
        return;
    }
    let text = text.clone();
    let [w, h] = ui.size;
    let width = (ui.tooltip_size[0] + 20.0).min((w - 24.0).max(1.0));
    let height = ui.tooltip_size[1] + 16.0;
    let x = (anchor[0] + (anchor[2] - width) * 0.5)
        .max(12.0)
        .min((w - width - 12.0).max(0.0));
    let y = if anchor[1] >= height + 6.0 {
        anchor[1] - height - 6.0
    } else {
        (anchor[1] + anchor[3] + 6.0).min((h - height).max(0.0))
    };
    s.clip = [0.0, 0.0, w, h];
    s.panel([x, y, width, height], p.field, p.accent, 5.0);
    s.tooltip_label = Some(s.labels.len());
    s.paragraph(
        &text,
        [
            x + 10.0,
            y + 8.0,
            (width - 20.0).max(1.0),
            ui.tooltip_size[1],
        ],
        p.ink,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Input;
    use serde_json::json;

    #[test]
    fn centered_heading_tooltips_do_not_extend_past_their_control_card() {
        for ty in ["f64", "bool"] {
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{
                "path":"hover.onda","supportsScope":false,"paramLayout":"knobs",
                "params":[{"name":"gain","type":ty,"value":0.5}]
            }}));
            ui.prepare_scene();
            let r = ui
                .scene
                .shapes
                .iter()
                .find(|s| s.rect[3] == 140.0)
                .unwrap()
                .rect;
            assert!(ui
                .scene
                .tooltip_at(r[0] + r[2] * 0.5, r[1] + 16.0)
                .is_some());
            assert!(
                ui.scene
                    .tooltip_at(r[0] + r[2] + 2.0, r[1] + 16.0)
                    .is_none(),
                "hovering the gap beside a card must not reveal its label"
            );
        }
    }

    #[test]
    fn hovering_clipped_names_reveals_the_full_name_and_type_without_changing_layout() {
        let mut ui = RunUi::default();
        let params = [
            "filter_envelope_attack",
            "filter_envelope_release",
            "filter_envelope_decay",
        ]
        .map(|name| json!({"name":name,"type":"f64","value":0.5}));
        ui.message(json!({"type":"state","state":{
            "path":"names.onda","supportsScope":false,"paramLayout":"sliders",
            "params":params
        }}));
        ui.prepare_scene();
        let height = ui.scene.content_height;
        for i in 0..3 {
            let name = ui.controls[i].name().to_owned();
            let r = ui
                .scene
                .tooltips
                .iter()
                .find(|t| t.text == format!("{name} (f64)"))
                .unwrap()
                .rect
                .0;
            let revision = ui.revisions();
            ui.input(Input::PointerMove {
                x: r[0] + 1.0,
                y: r[1] + 1.0,
                fine: false,
            });
            assert_ne!(ui.revisions(), revision);
            ui.prepare_scene();
            assert!(ui.scene.tooltip_label.is_none());
            ui.prepare_scene_at(ui.repaint_deadline().unwrap());
            let label = &ui.scene.labels[ui.scene.tooltip_label.expect("full-label tooltip")];
            assert_eq!(label.text, format!("{name} (f64)"));
            assert!(label.style.wrap);
            assert_eq!(ui.scene.content_height, height);
        }
        ui.input(Input::PointerLeave);
        ui.prepare_scene();
        assert!(ui.scene.tooltip_label.is_none());
    }

    #[test]
    fn header_and_log_disclose_full_text_without_changing_the_layout() {
        let mut ui = RunUi::default();
        ui.resize(360.0, 840.0, 1.0);
        let path = "/home/user/projects/synth/voices/main.onda";
        let context = format!("{path}:123 · Synth.Voice.oscillator");
        ui.message(json!({"type":"state","state":{
            "path":path,"supportsScope":false,"supportsDeviceSelection":true,
            "supportsSourceSelection":true,"supportsProjectExport":true,"canExportProject":true,
            "logText":"Output\n","logEntries":[{"source":{"file":path,"line":123},"lexicalOwner":"Synth.Voice.oscillator"}]
        }}));
        ui.prepare_scene();
        let height = ui.scene.content_height;
        for text in [
            path,
            "Settings",
            "Save as Project",
            "Unload",
            context.as_str(),
        ] {
            let r = ui
                .scene
                .tooltips
                .iter()
                .find(|t| t.text == text)
                .unwrap_or_else(|| panic!("missing tooltip: {text}"))
                .rect
                .0;
            ui.input(Input::PointerMove {
                x: r[0] + 1.0,
                y: r[1] + 1.0,
                fine: false,
            });
            ui.prepare_scene();
            assert!(ui.scene.tooltip_label.is_none());
            ui.prepare_scene_at(ui.repaint_deadline().unwrap());
            assert_eq!(ui.scene.labels[ui.scene.tooltip_label.unwrap()].text, text);
            assert_eq!(ui.scene.content_height, height);
        }
    }

    #[test]
    fn tooltips_wait_half_a_second_retain_the_deadline_within_a_label_and_cancel_on_exit() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"hover.onda","supportsScope":false,
            "params":[{"name":"gain","type":"f64","value":0.5}]
        }}));
        ui.prepare_scene();
        let r = ui
            .scene
            .tooltips
            .iter()
            .find(|t| t.text == "gain (f64)")
            .unwrap()
            .rect
            .0;
        ui.input(Input::PointerMove {
            x: r[0] + 1.0,
            y: r[1] + 1.0,
            fine: false,
        });
        ui.prepare_scene();
        let deadline = ui.repaint_deadline().unwrap();
        let revision = ui.revisions();
        ui.prepare_scene_at(deadline - Duration::from_millis(1));
        assert!(ui.scene.tooltip_label.is_none());
        assert_eq!(ui.revisions(), revision);
        ui.input(Input::PointerMove {
            x: r[0] + 2.0,
            y: r[1] + 2.0,
            fine: false,
        });
        ui.prepare_scene();
        assert_eq!(ui.repaint_deadline(), Some(deadline));
        ui.prepare_scene_at(deadline);
        assert!(ui.scene.tooltip_label.is_some());
        assert!(
            ui.repaint_deadline().is_none(),
            "visible tooltips need no periodic frames"
        );
        assert_ne!(ui.revisions(), revision);

        ui.input(Input::PointerLeave);
        ui.prepare_scene();
        assert!(ui.scene.tooltip_label.is_none());
        assert!(ui.repaint_deadline().is_none());
        ui.input(Input::PointerMove {
            x: r[0] + 1.0,
            y: r[1] + 1.0,
            fine: false,
        });
        ui.prepare_scene();
        assert!(
            ui.scene.tooltip_label.is_none(),
            "returning starts a fresh delay"
        );
        assert!(ui.repaint_deadline().is_some());
        ui.input(Input::FocusLost);
        ui.prepare_scene();
        assert!(
            ui.repaint_deadline().is_none(),
            "unfocused views must not open tooltips"
        );
    }
}
