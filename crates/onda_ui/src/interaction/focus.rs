//! Keyboard traversal follows logical widget order, including culled rows.
use crate::{
    model::RunUi,
    scene::{Scene, Target},
};

#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum FocusLayer {
    #[default]
    Document,
    Dock,
    Popup,
    Menu,
    Toolbar,
}

pub(crate) struct FocusTarget {
    pub target: Target,
    pub rect: [f32; 4],
    pub layer: FocusLayer,
}

pub(crate) enum FocusEntry {
    Target(FocusTarget),
    Group {
        targets: std::sync::Arc<[FocusTarget]>,
        offset: [f32; 2],
    },
}
impl FocusEntry {
    fn targets(&self) -> impl Iterator<Item = (&FocusTarget, [f32; 4])> {
        let (targets, offset) = match self {
            Self::Target(target) => (std::slice::from_ref(target), [0.0, 0.0]),
            Self::Group { targets, offset } => (targets.as_ref(), *offset),
        };
        targets.iter().map(move |target| {
            let [x, y, w, h] = target.rect;
            (target, [x + offset[0], y + offset[1], w, h])
        })
    }
}

impl Scene {
    pub fn focus_target(&mut self, rect: [f32; 4], target: &Target) {
        if matches!(
            target,
            Target::Control(_)
                | Target::Note(_)
                | Target::Option(_)
                | Target::Scrollbar(_)
                | Target::Overlay
        ) {
            return;
        }
        self.focus_targets.push(FocusEntry::Target(FocusTarget {
            target: target.clone(),
            rect,
            layer: self.focus_layer,
        }));
    }
}

impl RunUi {
    pub(crate) fn focused(&self, target: &Target) -> bool {
        self.interaction.focus_visible && self.interaction.focus() == Some(target)
    }

    pub(crate) fn finish_hidden_edit(&mut self) {
        if self.editor.editing() && self.edited_field_rect().is_none() {
            // A field leaving the visible scene loses focus just like a click outside.
            self.commit_draft();
            self.cancel_editing();
            self.interaction.focus = None;
            self.invalidate();
        }
    }

    pub(crate) fn tab(&mut self, backwards: bool) -> bool {
        let current = self.interaction.focus.clone();
        if !self.commit_draft() {
            return true;
        }
        self.prepare_scene();
        let targets: Vec<_> = self
            .scene
            .focus_targets
            .iter()
            .flat_map(FocusEntry::targets)
            .filter(|(entry, _)| {
                if self.interaction.toolbar_menu.is_some() && self.scene.toolbar_menu_rect[3] > 0.0
                {
                    entry.layer == FocusLayer::Toolbar
                } else {
                    (entry.layer == FocusLayer::Popup) == self.interaction.devices_open
                }
            })
            .collect();
        if targets.is_empty() {
            return false;
        }
        let current = current.as_ref().and_then(|target| {
            targets
                .iter()
                .position(|(entry, _)| entry.target == *target)
        });
        let index = if backwards {
            current.unwrap_or(0) + targets.len() - 1
        } else {
            current.map_or(0, |index| index + 1)
        } % targets.len();
        let (entry, rect) = targets[index];
        let target = entry.target.clone();
        let [_, top, _, height] = rect;
        match entry.layer {
            FocusLayer::Document => {
                let top = top + self.scroll;
                self.scroll = self
                    .scroll
                    .min((top - 12.0).max(0.0))
                    .max((top + height - self.scene.viewport_height + 12.0).max(0.0));
                self.presentation.changed();
            }
            FocusLayer::Popup => {
                let bounds = self.scene.popup_rect;
                let top = top + self.interaction.popup_scroll;
                self.interaction.popup_scroll = self
                    .interaction
                    .popup_scroll
                    .min((top - bounds[1] - 10.0).max(0.0))
                    .max((top + height - bounds[1] - bounds[3] + 10.0).max(0.0));
            }
            FocusLayer::Toolbar => {
                let bounds = self.scene.toolbar_menu_rect;
                let top = top + self.interaction.toolbar_scroll;
                self.interaction.toolbar_scroll = self
                    .interaction
                    .toolbar_scroll
                    .min((top - bounds[1] - 4.0).max(0.0))
                    .max((top + height - bounds[1] - bounds[3] + 4.0).max(0.0));
            }
            FocusLayer::Dock | FocusLayer::Menu => {}
        }
        self.interaction.focus = Some(target.clone());
        if let Target::Field(field) = target {
            if !self.field_value(&field).is_boolean() {
                self.begin_field(field);
            }
        }
        self.invalidate();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editing::{Field, Setting},
        events::EventField,
        presentation::Section,
        Input, Key,
    };
    use serde_json::json;

    fn fixture() -> RunUi {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"focus.onda","supportsScope":false,"supportsParamSmoothing":true,
            "supportsDeviceSelection":true,"supportsMidiInputSelection":true,
            "midiKeyboardInteractive":true,"midi":{"noteOn":true},"paramSmoothingMs":30,
            "sampleRateHz":48000,"blockSamples":512,
            "params":[{"name":"enabled","type":"bool","value":false},
                {"name":"gain","type":"f64","value":0.5},
                {"name":"external","type":"f64","enabled":false,"value":0.0}],
            "events":[{"name":"play","args":[{"name":"level","type":"f64","value":0.5}]}]
        }}));
        ui
    }
    fn tab(ui: &mut RunUi, shift: bool) {
        ui.input(Input::Key {
            key: Key::Tab,
            shift,
        });
    }
    #[test]
    fn traversal_reveals_every_buffer_action_in_both_directions() {
        for backwards in [false, true] {
            let mut ui = RunUi::default();
            ui.resize(720.0, 400.0, 1.0);
            ui.message(json!({"type":"state","state":{
                "path":"buffers.onda","supportsScope":false,"supportsTransport":false,
                "buffers":(0..10).map(|i| json!({"name":format!("buffer_{i}")})).collect::<Vec<_>>()
            }}));
            let mut visited = std::collections::BTreeSet::new();
            for _ in 0..24 {
                tab(&mut ui, backwards);
                let Some(Target::Command(command)) = ui.interaction.focus.clone() else {
                    continue;
                };
                if !matches!(
                    command["type"].as_str(),
                    Some("chooseBufferFile" | "clearBuffer")
                ) {
                    continue;
                }
                ui.prepare_scene();
                let hit = ui
                    .scene
                    .hits
                    .iter()
                    .find(|h| h.target == Target::Command(command.clone()))
                    .unwrap();
                assert!(hit.rect.0[2] > 0.0 && hit.rect.0[3] > 0.0);
                ui.input(Input::Key {
                    key: Key::Enter,
                    shift: false,
                });
                assert!(ui.take_commands().contains(&command));
                visited.insert((
                    command["name"].as_str().unwrap().to_owned(),
                    command["type"].as_str().unwrap().to_owned(),
                ));
            }
            assert_eq!(
                visited.len(),
                20,
                "Tab must reach Bind and Clear on all ten buffers"
            );
        }
    }
    #[test]
    fn traversal_reaches_header_events_booleans_smoothing_and_the_dock() {
        let mut ui = fixture();
        let mut visited = Vec::new();
        for _ in 0..32 {
            tab(&mut ui, false);
            visited.push(ui.interaction.focus.clone().unwrap());
        }
        for target in [
            Target::Devices,
            Target::Field(Field::Control(0)),
            Target::Field(Field::Control(1)),
            Target::Field(Field::Setting(Setting::ParamSmoothing)),
            Target::Field(Field::Setting(Setting::Octave)),
            Target::Field(Field::Event(EventField {
                event: 0,
                arg: 0,
                path: String::new(),
                scalar: "f64".into(),
            })),
        ] {
            assert!(visited.contains(&target), "missing focus stop {target:?}");
        }
        assert!(!visited.contains(&Target::Field(Field::Control(2))));
        ui.begin_field(Field::Control(1));
        ui.input(Input::Text {
            text: "0.75".into(),
        });
        tab(&mut ui, false);
        assert_eq!(ui.interaction.focus, Some(Target::Section(Section::Midi)));
        assert_eq!(ui.controls[1].value, 0.75);
        tab(&mut ui, true);
        assert_eq!(ui.interaction.focus, Some(Target::Field(Field::Control(1))));
        ui.editor.cancel_draft();
        ui.interaction.focus = Some(Target::Field(Field::Control(0)));
        ui.input(Input::Key {
            key: Key::Enter,
            shift: false,
        });
        assert_eq!(ui.controls[0].value, true);
    }
    #[test]
    fn popup_focus_stays_inside_and_reveals_scrolled_settings() {
        let mut ui = fixture();
        ui.resize(360.0, 400.0, 1.0);
        ui.interaction.devices_open = true;
        ui.invalidate();
        for _ in 0..8 {
            tab(&mut ui, false);
            let target = ui.interaction.focus.as_ref().unwrap();
            assert!(matches!(
                target,
                Target::Theme | Target::Choose(_) | Target::Command(_)
            ));
        }
        assert!(ui.interaction.popup_scroll > 0.0);
        assert_eq!(ui.scroll, 0.0);
    }
}
