//! Dropdowns keep their option list and scrolling separate from the parent panel.
use crate::{
    editing::{Field, Setting},
    icons::Icon,
    model::RunUi,
    scene::{Palette, Rect, Scene, Target, TextStyle},
    Key,
};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectId {
    SampleRate,
    BlockSize,
    AudioInput,
    AudioOutput,
    MidiInput,
}
pub(crate) struct Choice {
    pub label: String,
    pub value: Value,
}
pub(crate) struct SelectSpec {
    pub choices: Vec<Choice>,
    pub value: Value,
    pub label: String,
    pub enabled: bool,
}
pub(crate) struct Menu {
    pub id: SelectId,
    pub highlighted: usize,
    pub scroll: f32,
    search: String,
}
impl SelectId {
    pub fn spec(self, ui: &RunUi) -> SelectSpec {
        let (choices, value, enabled) = match self {
            Self::SampleRate | Self::BlockSize => {
                let rate = self == Self::SampleRate;
                let field = Field::Setting(if rate {
                    Setting::SampleRate
                } else {
                    Setting::BlockSize
                });
                let host = if rate {
                    None
                } else {
                    ui.state.get("hostBlockSamples")
                };
                let offered = ui.state[if rate {
                    "sampleRateOptions"
                } else {
                    "blockSizeOptions"
                }]
                .as_array();
                let values = if let Some(offered) = offered {
                    offered
                        .iter()
                        .filter_map(Value::as_u64)
                        .filter(|n| *n > 0)
                        .collect()
                } else if rate {
                    vec![44100, 48000, 88200, 96000, 192000]
                } else if let Some(host) = host {
                    let maximum = host.as_u64().unwrap_or(0);
                    let mut values: Vec<_> = [1, 2, 4, 8]
                        .map(|divisor| maximum / divisor)
                        .into_iter()
                        .filter(|n| *n > 0)
                        .collect();
                    values.dedup();
                    values
                } else {
                    vec![64, 128, 256, 512, 1024, 2048]
                };
                let value = ui.field_value(&field);
                let mut choices: Vec<_> = values
                    .into_iter()
                    .map(|n| Choice {
                        label: if rate {
                            format!("{} kHz", n as f64 / 1000.0)
                        } else {
                            format!("{n} samples")
                        },
                        value: json!(n),
                    })
                    .collect();
                if host.is_some() {
                    choices.insert(
                        0,
                        Choice {
                            label: "Follow host".into(),
                            value: json!(0),
                        },
                    );
                }
                if !choices.iter().any(|c| c.value == value)
                    && (host.is_none()
                        || value
                            .as_u64()
                            .zip(host.and_then(Value::as_u64))
                            .is_some_and(|(value, maximum)| value <= maximum))
                {
                    choices.push(Choice {
                        label: format!("{} {}", value, if rate { "Hz" } else { "samples" }),
                        value: value.clone(),
                    });
                }
                (
                    choices,
                    value,
                    ui.field_enabled(&field)
                        && host.is_none_or(|n| n.as_u64().is_some_and(|n| n > 0)),
                )
            }
            _ => {
                let (bank, current, default, enabled) = match self {
                    Self::AudioInput => (
                        "inputDevices",
                        "currentInputDevice",
                        "Default input",
                        ui.state["supportsDeviceSelection"] == true,
                    ),
                    Self::AudioOutput => (
                        "outputDevices",
                        "currentOutputDevice",
                        "Default output",
                        ui.state["supportsDeviceSelection"] == true,
                    ),
                    _ => (
                        "midiInputDevices",
                        "currentMidiInputDevice",
                        "Computer Keyboard",
                        ui.state["supportsMidiInputSelection"] == true,
                    ),
                };
                let default_value = if self == Self::MidiInput {
                    json!(default)
                } else {
                    Value::Null
                };
                let mut choices = vec![Choice {
                    label: default.into(),
                    value: default_value.clone(),
                }];
                for name in ui.state[bank]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !choices.iter().any(|c| c.value == name) {
                        choices.push(Choice {
                            label: name.into(),
                            value: json!(name),
                        });
                    }
                }
                let value = ui
                    .state
                    .get(current)
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or(default_value);
                if !value.is_null() && !choices.iter().any(|c| c.value == value) {
                    choices.push(Choice {
                        label: value.as_str().unwrap_or(default).into(),
                        value: value.clone(),
                    });
                }
                (choices, value, enabled)
            }
        };
        let label = choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| format!("{value} samples (using {})", ui.state["hostBlockSamples"]));
        SelectSpec {
            choices,
            value,
            label,
            enabled,
        }
    }
}
impl Scene {
    pub fn select(&mut self, ui: &RunUi, id: SelectId, rect: [f32; 4], p: Palette) {
        let spec = id.spec(ui);
        let active = ui.interaction.chooser.as_ref().is_some_and(|m| m.id == id)
            || ui.focused(&Target::Choose(id));
        self.panel(
            rect,
            p.field,
            if spec.enabled && (active || self.hovered(ui, rect)) {
                p.accent
            } else {
                p.border
            },
            5.0,
        );
        self.styled(
            spec.label,
            [rect[0] + 9.0, rect[1], rect[2] - 32.0, rect[3]],
            12.0,
            if spec.enabled { p.ink } else { p.muted },
            TextStyle::default(),
        );
        self.icon(
            Icon::Chevron(true),
            [rect[0] + rect[2] - 13.0, rect[1] + rect[3] * 0.5],
            p.ink_color(),
        );
        if spec.enabled {
            self.selects.push((id, Rect(rect)));
            self.hit(rect, Target::Choose(id));
        }
    }
}
impl RunUi {
    pub(crate) fn open_select(&mut self, id: SelectId) {
        let spec = id.spec(self);
        if !spec.enabled {
            return;
        }
        self.cancel_number();
        self.interaction.focus = Some(Target::Choose(id));
        self.interaction.chooser = Some(Menu {
            id,
            highlighted: spec
                .choices
                .iter()
                .position(|c| c.value == spec.value)
                .unwrap_or(0),
            scroll: 0.0,
            search: String::new(),
        });
        self.ensure_choice_visible();
        self.invalidate();
    }
    pub(crate) fn ensure_choice_visible(&mut self) {
        let Some(menu) = &self.interaction.chooser else {
            return;
        };
        let count = menu.id.spec(self).choices.len();
        let Some((_, anchor)) = self.scene.selects.iter().find(|(id, _)| *id == menu.id) else {
            return;
        };
        let height = (menu_bounds(self, *anchor, count)[3] - 8.0).max(1.0);
        let menu = self.interaction.chooser.as_mut().unwrap();
        let y = menu.highlighted as f32 * 28.0;
        let max = (count as f32 * 28.0 - height).max(0.0);
        menu.scroll = menu
            .scroll
            .min(y)
            .max((y + 28.0 - height).max(0.0))
            .clamp(0.0, max);
    }
    pub(crate) fn choose_option(&mut self, index: usize) {
        let Some(menu) = self.interaction.chooser.take() else {
            return;
        };
        let id = menu.id;
        let spec = id.spec(self);
        if !spec.enabled {
            return;
        }
        if let Some(choice) = spec.choices.get(index) {
            match id {
                SelectId::SampleRate | SelectId::BlockSize => self.set_field(
                    &Field::Setting(if id == SelectId::SampleRate {
                        Setting::SampleRate
                    } else {
                        Setting::BlockSize
                    }),
                    choice.value.clone(),
                    true,
                ),
                _ => {
                    let (current, command) = match id {
                        SelectId::AudioInput => ("currentInputDevice", "setInputDevice"),
                        SelectId::AudioOutput => ("currentOutputDevice", "setOutputDevice"),
                        _ => ("currentMidiInputDevice", "setMidiInputDevice"),
                    };
                    if id == SelectId::MidiInput && self.state[current] != choice.value {
                        self.commands.extend(self.interaction.midi.release());
                        self.interaction.midi.monitored.fill(false);
                    }
                    self.state[current] = choice.value.clone();
                    self.commands
                        .push(json!({"type":command,"name":choice.value}));
                }
            }
        }
        self.interaction.focus = Some(Target::Choose(id));
        self.invalidate();
    }
    pub(crate) fn select_key(&mut self, key: &Key) -> bool {
        if self.interaction.chooser.is_none() {
            if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::Enter) {
                if let Some(Target::Choose(id)) = self.interaction.focus {
                    self.open_select(id);
                    return true;
                }
            }
            return false;
        }
        let menu = self.interaction.chooser.as_ref().unwrap();
        let count = menu.id.spec(self).choices.len();
        let index = menu.highlighted;
        match key {
            Key::Enter => self.choose_option(index),
            Key::Escape | Key::Tab => {
                self.interaction.chooser = None;
                self.invalidate();
                return !matches!(key, Key::Tab);
            }
            Key::ArrowUp => {
                self.interaction.chooser.as_mut().unwrap().highlighted = index.saturating_sub(1)
            }
            Key::ArrowDown => {
                self.interaction.chooser.as_mut().unwrap().highlighted =
                    (index + 1).min(count.saturating_sub(1))
            }
            Key::Home => self.interaction.chooser.as_mut().unwrap().highlighted = 0,
            Key::End => {
                self.interaction.chooser.as_mut().unwrap().highlighted = count.saturating_sub(1)
            }
            _ => return false,
        }
        self.ensure_choice_visible();
        self.invalidate();
        true
    }
    pub(crate) fn select_text(&mut self, text: &str) {
        let Some(menu) = self.interaction.chooser.as_ref() else {
            return;
        };
        let choices = menu.id.spec(self).choices;
        let menu = self.interaction.chooser.as_mut().unwrap();
        menu.search.push_str(&text.to_lowercase());
        let found = choices
            .iter()
            .position(|c| c.label.to_lowercase().starts_with(&menu.search));
        if let Some(index) = found {
            menu.highlighted = index;
        } else {
            menu.search = text.to_lowercase();
            if let Some(index) = choices
                .iter()
                .position(|c| c.label.to_lowercase().starts_with(&menu.search))
            {
                menu.highlighted = index;
            }
        }
        self.ensure_choice_visible();
        self.invalidate();
    }
}
fn menu_bounds(ui: &RunUi, anchor: Rect, count: usize) -> [f32; 4] {
    let [x, y, w, h] = anchor.0;
    let below = (ui.size[1] - y - h - 8.0).max(0.0);
    let above = (y - 8.0).max(0.0);
    let down = below >= above || below >= count as f32 * 28.0 + 8.0;
    let height = (count as f32 * 28.0 + 8.0)
        .min(320.0)
        .min(if down { below } else { above })
        .max(1.0);
    [
        x.clamp(4.0, (ui.size[0] - w - 4.0).max(4.0)),
        if down { y + h + 4.0 } else { y - height - 4.0 },
        w,
        height,
    ]
}
pub(crate) fn build_menu(ui: &RunUi, s: &mut Scene, p: Palette) {
    let Some(menu) = &ui.interaction.chooser else {
        return;
    };
    let Some((_, anchor)) = s.selects.iter().find(|(id, _)| *id == menu.id) else {
        return;
    };
    let spec = menu.id.spec(ui);
    let rect = menu_bounds(ui, *anchor, spec.choices.len());
    let height = rect[3];
    s.focus_layer = crate::interaction::FocusLayer::Menu;
    s.clip = [0.0, 0.0, ui.size[0], ui.size[1]];
    s.menu_rect = rect;
    s.panel(rect, p.panel, p.border, 5.0);
    s.clip = [rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, rect[3] - 2.0];
    let max = (spec.choices.len() as f32 * 28.0 + 8.0 - height).max(0.0);
    let scroll = menu.scroll.clamp(0.0, max);
    for (index, choice) in spec.choices.iter().enumerate() {
        let r = [
            rect[0] + 4.0,
            rect[1] + 4.0 + index as f32 * 28.0 - scroll,
            rect[2] - 8.0,
            28.0,
        ];
        if !s.visible(r[1], r[3]) {
            continue;
        }
        if index == menu.highlighted || s.hovered(ui, r) {
            s.shape(r, p.accent_soft, 3.0);
        }
        if choice.value == spec.value {
            s.shape([r[0] + 3.0, r[1] + 11.0, 5.0, 5.0], p.accent, 2.5);
        }
        s.styled(
            &choice.label,
            [r[0] + 15.0, r[1], r[2] - 22.0, r[3]],
            12.0,
            p.ink,
            TextStyle::default(),
        );
        s.hit(r, Target::Option(index));
    }
    if max > 0.0 {
        let thumb =
            ((height - 8.0) * (height - 8.0) / (spec.choices.len() as f32 * 28.0)).max(16.0);
        s.shape(
            [
                rect[0] + rect[2] - 6.0,
                rect[1] + 4.0 + (height - 8.0 - thumb) * scroll / max,
                3.0,
                thumb,
            ],
            p.scrollbar,
            1.5,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_choices_default_to_computer_keyboard_without_a_null_option() {
        let mut ui = RunUi::default();
        for current in [Value::Null, json!("Computer Keyboard"), json!("USB MIDI")] {
            ui.message(json!({"type":"state","state":{
                "supportsMidiInputSelection":true,
                "midiInputDevices":["Computer Keyboard","USB MIDI"],
                "currentMidiInputDevice":current
            }}));
            let spec = SelectId::MidiInput.spec(&ui);
            assert_eq!(spec.choices.len(), 2);
            assert!(spec.choices.iter().all(|choice| !choice.value.is_null()));
            assert_eq!(spec.choices[0].value, "Computer Keyboard");
            assert_eq!(spec.label, current.as_str().unwrap_or("Computer Keyboard"));
            ui.open_select(SelectId::MidiInput);
            ui.choose_option(0);
            assert!(ui.take_commands().iter().any(|command| {
                command["type"] == "setMidiInputDevice" && command["name"] == "Computer Keyboard"
            }));
        }
        for id in [SelectId::AudioInput, SelectId::AudioOutput] {
            assert!(id.spec(&ui).choices[0].value.is_null());
        }
    }

    #[test]
    fn pointer_movement_within_an_option_repaints_a_keyboard_changed_highlight() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{"supportsSourceSelection":true,"supportsRunSettings":true}}));
        ui.open_select(SelectId::SampleRate);
        ui.prepare_scene();
        let r = ui
            .scene
            .hits
            .iter()
            .find(|hit| hit.target == Target::Option(0))
            .unwrap()
            .rect
            .0;
        let x = r[0] + 2.0;
        let y = r[1] + 2.0;
        ui.input(crate::Input::PointerMove { x, y, fine: false });
        ui.input(crate::Input::Key {
            key: Key::ArrowDown,
            shift: false,
        });
        ui.prepare_scene();
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 1);
        let revision = ui.revisions();
        ui.input(crate::Input::PointerMove {
            x: x + 1.0,
            y,
            fine: false,
        });
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 0);
        assert_ne!(
            ui.revisions(),
            revision,
            "the visible highlight must follow the pending selection"
        );
    }

    #[test]
    fn plugin_block_choices_follow_host_capacity_and_subdivisions() {
        for (host, expected) in [
            (128, vec![0, 128, 64, 32, 16]),
            (32, vec![0, 32, 16, 8, 4]),
            (1, vec![0, 1]),
            (0, vec![0]),
            (300, vec![0, 300, 150, 75, 37]),
        ] {
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{
                "hostBlockSamples":host,"preferredBlockSamples":0,"canSetBlockSize":true,
                "supportsRunSettings":true
            }}));
            let spec = SelectId::BlockSize.spec(&ui);
            assert_eq!(
                spec.choices
                    .iter()
                    .map(|c| c.value.as_i64().unwrap())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(spec.enabled, host > 0);
        }
    }

    #[test]
    fn plugin_keeps_valid_custom_sizes_and_only_displays_oversized_preferences() {
        let mut ui = RunUi::default();
        for (preferred, offered) in [(24, true), (512, false)] {
            ui.message(json!({"type":"state","state":{
                "hostBlockSamples":128,"preferredBlockSamples":preferred,
                "canSetBlockSize":true,"supportsRunSettings":true
            }}));
            let spec = SelectId::BlockSize.spec(&ui);
            assert_eq!(spec.choices.iter().any(|c| c.value == preferred), offered);
            assert!(spec.label.starts_with(&preferred.to_string()));
            if !offered {
                assert_eq!(spec.label, "512 samples (using 128)");
            }
        }
        ui.message(json!({"type":"state","state":{"canSetBlockSize":false}}));
        assert!(!SelectId::BlockSize.spec(&ui).enabled);
    }
}
