//! Dropdowns keep their option list and scrolling separate from the parent panel.
use crate::{
    editing::{Field, Setting},
    icons::Icon,
    model::{number, RunUi},
    scene::{Palette, Rect, Scene, Target, TextStyle},
    Key,
};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectId {
    Field(Field),
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
    fn display(&self, ui: &RunUi) -> (String, bool) {
        let Self::Field(field) = self else {
            let spec = self.spec(ui);
            return (spec.label, spec.enabled);
        };
        let members = enum_members(ui, field);
        let label = enum_ordinal(ui, field)
            .and_then(|ordinal| members.get(ordinal))
            .map(enum_member_label)
            .unwrap_or("")
            .to_owned();
        (label, ui.field_enabled(field) && members.len() > 1)
    }

    pub fn spec(&self, ui: &RunUi) -> SelectSpec {
        if let Self::Field(field) = self {
            let choices = enum_members(ui, field)
                .iter()
                .enumerate()
                .map(|(ordinal, member)| Choice {
                    label: enum_member_label(member).to_owned(),
                    value: json!(ordinal),
                })
                .collect();
            let value = enum_ordinal(ui, field).map_or(Value::Null, |ordinal| json!(ordinal));
            let (label, enabled) = self.display(ui);
            return SelectSpec {
                choices,
                value,
                label,
                enabled,
            };
        }
        let (choices, value, enabled) = match self {
            Self::SampleRate | Self::BlockSize => {
                let rate = *self == Self::SampleRate;
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
                    let mut values: Vec<_> = [2, 4, 8]
                        .map(|divisor| maximum / divisor)
                        .into_iter()
                        .filter(|n| *n > 0)
                        .collect();
                    values.dedup();
                    values
                } else {
                    vec![64, 128, 256, 512, 1024, 2048]
                };
                let mut value = ui.field_value(&field);
                if host.is_some_and(|host| *host == value) {
                    value = json!(0);
                }
                let mut choices: Vec<_> = values
                    .into_iter()
                    .filter(|n| host.and_then(Value::as_u64) != Some(*n))
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
                            label: host
                                .and_then(Value::as_u64)
                                .filter(|n| *n > 0)
                                .map_or_else(|| "Follow host".into(), |n| format!("{n} (Host)")),
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
                let default_value = if *self == Self::MidiInput {
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

fn enum_members<'a>(ui: &'a RunUi, field: &Field) -> &'a [Value] {
    ui.field_enumeration(field)
        .and_then(|enumeration| enumeration["members"].as_array())
        .map_or(&[], Vec::as_slice)
}

fn enum_ordinal(ui: &RunUi, field: &Field) -> Option<usize> {
    // Hosts may encode an i32 ordinal as either an integer or a float in JSON.
    let value = number(&ui.field_value(field))?;
    (value >= 0.0 && value.fract() == 0.0 && value < enum_members(ui, field).len() as f64)
        .then_some(value as usize)
}

fn enum_member_label(member: &Value) -> &str {
    member["label"]
        .as_str()
        .or_else(|| member["name"].as_str())
        .unwrap_or("")
}

impl Scene {
    pub fn select(&mut self, ui: &RunUi, id: SelectId, rect: [f32; 4], p: Palette) {
        let (label, enabled) = id.display(ui);
        let active = ui.interaction.chooser.as_ref().is_some_and(|m| m.id == id)
            || ui.focused(&Target::Choose(id.clone()));
        self.panel(
            rect,
            p.field,
            if enabled && (active || self.hovered(ui, rect)) {
                p.accent
            } else {
                p.border
            },
            5.0,
        );
        self.styled(
            label,
            [rect[0] + 9.0, rect[1], rect[2] - 32.0, rect[3]],
            12.0,
            if enabled { p.ink } else { p.muted },
            TextStyle::default(),
        );
        self.icon(
            Icon::Chevron(true),
            [rect[0] + rect[2] - 13.0, rect[1] + rect[3] * 0.5],
            p.ink_color(),
        );
        if enabled {
            self.selects.push((id.clone(), Rect(rect)));
            self.hit(rect, Target::Choose(id.clone()));
        }
    }
}
impl RunUi {
    pub(crate) fn highlight_hovered_option(&mut self) -> bool {
        let [x, y] = self.interaction.pointer;
        let Some(Target::Option(index)) = self.target(x, y) else {
            return false;
        };
        let Some(menu) = &mut self.interaction.chooser else {
            return false;
        };
        let changed = menu.highlighted != index;
        menu.highlighted = index;
        changed
    }
    pub(crate) fn open_select(&mut self, id: SelectId) {
        let spec = id.spec(self);
        if !spec.enabled {
            return;
        }
        self.cancel_number();
        self.interaction.focus = Some(Target::Choose(id.clone()));
        self.interaction.chooser = Some(Menu {
            id: id.clone(),
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
            match &id {
                SelectId::Field(field) => {
                    self.set_field(field, choice.value.clone(), true);
                    if let Field::Control(index) = field {
                        self.commands.push(self.controls[*index].end_command());
                    }
                }
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
        self.interaction.focus = Some(Target::Choose(id.clone()));
        self.invalidate();
    }
    pub(crate) fn select_key(&mut self, key: &Key) -> bool {
        if self.interaction.chooser.is_none() {
            if matches!(key, Key::ArrowUp | Key::ArrowDown | Key::Enter) {
                if let Some(Target::Choose(id)) = self.interaction.focus.clone() {
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
        self.interaction.focus_visible = true;
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
        // Hit testing owns the active row; adjacent hover rectangles can share an edge.
        if index == menu.highlighted && (s.hovered(ui, r) || ui.interaction.focus_visible) {
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
    s.scrollbar(
        ui,
        p,
        crate::scrollbar::ScrollRegion::Menu,
        rect,
        spec.choices.len() as f32 * 28.0 + 8.0,
        scroll,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_controls_display_selected_labels_and_keep_menu_ordinals() {
        let mut ui = RunUi::default();
        let mut state = json!({
            "path":"enum.onda", "supportsScope":false,
            "params":[{"name":"mode","type":"i32","value":2,
                "rangeMin":0,"rangeMax":2,"step":1,"stepCount":2,
                "enumeration":{"name":"Mode","members":[
                    {"name":"LowPass","label":"Low Pass"},
                    {"name":"HighPass"},
                    {"name":"BandPass","label":"Band-pass"}
                ]}}]
        });
        ui.message(json!({"type":"state","state":state}));
        let id = SelectId::Field(Field::Control(0));
        for (ordinal, expected) in [(0, "Low Pass"), (1, "HighPass"), (2, "Band-pass")] {
            ui.message(json!({"type":"parameterValues","state":{
                "params":[{"name":"mode","value":ordinal}]
            }}));
            ui.prepare_scene();
            assert!(ui.scene.labels.iter().any(|label| label.text == expected));
            assert!(ui.scene.selects.iter().any(|(select, _)| select == &id));
        }
        ui.open_select(id.clone());
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 2);
        ui.select_text("Low");
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 0);
        ui.select_key(&Key::Enter);
        assert_eq!(ui.controls[0].value, 0);

        state["params"][0]["value"] = json!(2);
        state["params"][0]["enumeration"]["members"][2]["label"] = json!("Updated");
        ui.message(json!({"type":"state","state":state}));
        ui.prepare_scene();
        assert!(ui.scene.labels.iter().any(|label| label.text == "Updated"));
        ui.open_select(id);
        ui.prepare_scene();
        assert!(ui.scene.labels.iter().any(|label| label.text == "Updated"));
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 2);
    }

    #[test]
    fn enum_controls_accept_numeric_ordinals_on_load_and_reset() {
        for value in [json!(1), json!(1.0)] {
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{
                "path":"enum.onda", "supportsScope":false,
                "params":[{"name":"mode","type":"i32","value":value,"default":value,
                    "rangeMin":0,"rangeMax":2,"step":1,"stepCount":2,
                    "enumeration":{"name":"Mode","members":[
                        {"name":"LowPass","label":"Low Pass"},
                        {"name":"HighPass","label":"High Pass"},
                        {"name":"BandPass","label":"Band-pass"}
                    ]}}]
            }}));
            let id = SelectId::Field(Field::Control(0));
            for phase in ["load", "reset"] {
                if phase == "reset" {
                    ui.open_select(id.clone());
                    ui.choose_option(0);
                    ui.take_commands();
                    ui.message(json!({"type":"parameterValues","state":{
                        "params":[{"name":"mode","value":value}]
                    }}));
                }
                ui.prepare_scene();
                assert!(
                    ui.scene
                        .labels
                        .iter()
                        .any(|label| label.text == "High Pass"),
                    "{phase}: {value:?}"
                );
                ui.open_select(id.clone());
                assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 1);
                ui.select_key(&Key::Enter);
                assert!(ui
                    .take_commands()
                    .iter()
                    .any(|command| { command["type"] == "setParam" && command["value"] == 1 }));
            }
        }
    }

    #[test]
    fn singleton_and_readonly_enum_controls_keep_their_labels_without_opening() {
        for (members, maximum, enabled) in [
            (json!([{"name":"Only","label":"Only choice"}]), 0, true),
            (
                json!([{"name":"Only","label":"Only choice"}, {"name":"Other"}]),
                1,
                false,
            ),
        ] {
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{
                "path":"enum.onda", "params":[{"name":"mode","type":"i32","value":0,
                    "rangeMin":0,"rangeMax":maximum,"step":1,"stepCount":maximum,
                    "enabled":enabled,"enumeration":{"name":"Mode","members":members}}]
            }}));
            ui.prepare_scene();
            let id = SelectId::Field(Field::Control(0));
            assert!(ui
                .scene
                .labels
                .iter()
                .any(|label| label.text == "Only choice"));
            assert!(!ui.scene.selects.iter().any(|(select, _)| select == &id));
            ui.open_select(id);
            assert!(ui.interaction.chooser.is_none());
        }
    }

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
    fn dropdown_hover_has_one_row_in_both_directions_and_keyboard_takes_over() {
        for light in [false, true] {
            for id in [
                SelectId::SampleRate,
                SelectId::BlockSize,
                SelectId::AudioInput,
                SelectId::AudioOutput,
                SelectId::MidiInput,
            ] {
                let mut ui = RunUi::default();
                ui.message(json!({"type":"state","state":{
                    "path":"dropdowns.onda",
                    "supportsSourceSelection":true,"supportsRunSettings":true,
                    "sampleRateHz":48000,"blockSamples":512,
                    "canSetSampleRate":true,"canSetBlockSize":true,
                    "supportsDeviceSelection":true,"supportsMidiInputSelection":true,
                    "inputDevices":["Microphone","USB"],
                    "outputDevices":["Speakers","Headphones"],"midiInputDevices":["USB MIDI"],
                    "themeMode":if light { "light" } else { "dark" }
                }}));
                ui.interaction.devices_open = true;
                ui.open_select(id.clone());
                ui.prepare_scene();
                assert_highlight(&mut ui, false);
                let rows: Vec<_> = ui
                    .scene
                    .hits
                    .iter()
                    .filter_map(|hit| matches!(hit.target, Target::Option(_)).then_some(hit.rect.0))
                    .collect();
                let top = rows[0][1] as i32;
                let last = rows.last().unwrap();
                let bottom = (last[1] + last[3]) as i32;
                // Include each shared edge, then continue through the row interior.
                // A cached double highlight used to persist only on the downward pass.
                let down = top..bottom;
                for y in down.clone().chain(down.rev()) {
                    ui.input(crate::Input::PointerMove {
                        x: rows[0][0] + 2.0,
                        y: y as f32,
                        fine: false,
                    });
                    assert_highlight(&mut ui, true);
                }
                ui.input(crate::Input::PointerLeave);
                assert_highlight(&mut ui, false);
                ui.input(crate::Input::PointerMove {
                    x: rows[0][0] + 2.0,
                    y: rows[0][1] + 2.0,
                    fine: false,
                });
                ui.input(crate::Input::Key {
                    key: Key::ArrowDown,
                    shift: false,
                });
                assert_highlight(&mut ui, true);
                assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 1);
            }
        }
    }

    #[test]
    fn dropdown_typeahead_highlights_and_commits_the_match_after_mouse_input() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "supportsSourceSelection":true,"supportsRunSettings":true,"sampleRateHz":48000
        }}));
        ui.open_select(SelectId::SampleRate);
        ui.prepare_scene();
        let row = ui
            .scene
            .hits
            .iter()
            .find(|hit| hit.target == Target::Option(0))
            .unwrap()
            .rect
            .0;
        ui.input(crate::Input::PointerMove {
            x: row[0] + 10.0,
            y: row[1] + 14.0,
            fine: false,
        });
        ui.input(crate::Input::Text { text: "192".into() });
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 4);
        assert_highlight(&mut ui, true);
        ui.input(crate::Input::Key {
            key: Key::Enter,
            shift: false,
        });
        assert_eq!(ui.state["sampleRateHz"], 192000);
    }

    #[test]
    fn dropdown_wheel_scrolling_keeps_the_row_under_the_pointer_active() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"dropdowns.onda", "supportsDeviceSelection":true,
            "outputDevices":(0..40).map(|i| format!("Device {i}")).collect::<Vec<_>>()
        }}));
        ui.interaction.devices_open = true;
        ui.open_select(SelectId::AudioOutput);
        ui.prepare_scene();
        let row = ui
            .scene
            .hits
            .iter()
            .find(|hit| hit.target == Target::Option(0))
            .unwrap()
            .rect
            .0;
        let x = row[0] + 10.0;
        let y = row[1] + 14.0;
        ui.input(crate::Input::PointerMove { x, y, fine: false });
        assert_highlight(&mut ui, true);
        for (delta, index) in [(28.0, 1), (280.0, 11), (-56.0, 9), (-252.0, 0)] {
            assert!(ui.input(crate::Input::Scroll { delta }));
            ui.prepare_scene();
            assert_eq!(ui.target(x, y), Some(Target::Option(index)));
            assert_highlight(&mut ui, true);
            assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, index);
        }
        ui.input(crate::Input::Key {
            key: Key::ArrowDown,
            shift: false,
        });
        assert_highlight(&mut ui, true);
        assert_eq!(ui.interaction.chooser.as_ref().unwrap().highlighted, 1);
    }

    fn assert_highlight(ui: &mut RunUi, visible: bool) {
        ui.prepare_scene();
        let highlights: Vec<_> = ui.scene.shapes[ui.scene.menu_shapes..]
            .iter()
            .filter(|shape| shape.color == Palette::new(ui.light).accent_soft)
            .collect();
        assert_eq!(
            highlights.len(),
            usize::from(visible),
            "only the active row may have a background highlight"
        );
        if visible {
            let index = ui.interaction.chooser.as_ref().unwrap().highlighted;
            let row = ui
                .scene
                .hits
                .iter()
                .find(|hit| hit.target == Target::Option(index))
                .unwrap()
                .rect
                .0;
            assert_eq!(highlights[0].rect, row);
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
            (128, vec![0, 64, 32, 16]),
            (32, vec![0, 16, 8, 4]),
            (1, vec![0]),
            (0, vec![0]),
            (300, vec![0, 150, 75, 37]),
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

    #[test]
    fn plugin_host_size_is_listed_once_even_with_explicit_options_or_preference() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "hostBlockSamples":128,"preferredBlockSamples":128,
            "blockSizeOptions":[128,64,32,16],
            "canSetBlockSize":true,"supportsRunSettings":true
        }}));
        let spec = SelectId::BlockSize.spec(&ui);
        assert_eq!(spec.value, 0);
        assert_eq!(spec.label, "128 (Host)");
        assert_eq!(
            spec.choices
                .iter()
                .map(|c| c.value.clone())
                .collect::<Vec<_>>(),
            vec![json!(0), json!(64), json!(32), json!(16)]
        );
    }
}
