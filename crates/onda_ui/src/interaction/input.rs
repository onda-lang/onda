use crate::{
    editing::Field,
    events,
    interaction::KEYS,
    model::RunUi,
    presentation::Section,
    scene::{Rect, Target},
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Input {
    PointerMove {
        x: f32,
        y: f32,
        #[serde(default)]
        fine: bool,
    },
    PointerDown {
        x: f32,
        y: f32,
        #[serde(default)]
        clicks: u8,
    },
    PointerUp,
    PointerLeave,
    TouchStart {
        #[serde(default)]
        id: u64,
        x: f32,
        y: f32,
    },
    TouchMove {
        #[serde(default)]
        id: u64,
        x: f32,
        y: f32,
    },
    TouchEnd {
        #[serde(default)]
        id: u64,
    },
    TouchCancel {
        #[serde(default)]
        id: u64,
    },
    Scroll {
        delta: f32,
    },
    HorizontalScroll {
        delta: f32,
    },
    Text {
        text: String,
    },
    Preedit {
        text: String,
        anchor: usize,
        cursor: usize,
    },
    TextMark {
        start: usize,
        end: usize,
    },
    EditText {
        text: String,
        anchor: usize,
        cursor: usize,
    },
    SelectText {
        index: usize,
        #[serde(default)]
        extend: bool,
    },
    Key {
        key: Key,
        #[serde(default)]
        shift: bool,
    },
    ComputerKey {
        key: String,
        pressed: bool,
    },
    ReleaseComputerKeys,
    FocusLost,
}
#[derive(Debug, Deserialize)]
pub enum Key {
    Enter,
    Escape,
    Backspace,
    Delete,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    Tab,
    SelectAll,
    Copy,
    Cut,
    Play,
    Stop,
}
pub(crate) struct Gesture {
    pub index: usize,
    previous: [f32; 2],
    normalized: f64,
    slider: Option<Rect>,
}

impl RunUi {
    /// Returns whether the region under the pointer moved, so hosts can chain
    /// unconsumed wheel events to their surrounding document.
    pub fn scroll(&mut self, delta: f32, horizontal: bool) -> bool {
        self.prepare_scene();
        if self.interaction.hover.take().is_some() {
            self.invalidate();
        }
        if horizontal {
            Rect(self.scene.log_rect)
                .contains(self.interaction.pointer[0], self.interaction.pointer[1])
                && self.pan_log_by(delta)
        } else {
            self.scroll_region_by(
                self.scroll_region(self.interaction.pointer[0], self.interaction.pointer[1]),
                delta,
            )
        }
    }
    pub(crate) fn pointer_captured(&self) -> bool {
        self.interaction.gesture.is_some()
            || self.editor.pointer_captured()
            || self.interaction.scrollbar_drag.is_some()
    }
    pub(crate) fn target(&self, x: f32, y: f32) -> Option<Target> {
        let hits =
            if self.interaction.chooser.is_some() && Rect(self.scene.menu_rect).contains(x, y) {
                &self.scene.hits[self.scene.menu_hits..]
            } else if self.interaction.devices_open && Rect(self.scene.popup_rect).contains(x, y) {
                &self.scene.hits[self.scene.overlay_hits..]
            } else {
                &self.scene.hits[..]
            };
        hits.iter()
            .rev()
            .find(|h| h.rect.contains(x, y))
            .map(|h| h.target.clone())
    }
    /// Returns whether the UI consumed the event. Embedded hosts can forward
    /// unused keyboard input to their parent application.
    pub fn input(&mut self, input: Input) -> bool {
        self.prepare_scene();
        let focus_visible = match &input {
            Input::PointerDown { .. } | Input::TouchStart { .. } => Some(false),
            _ => None,
        };
        if let Some(visible) =
            focus_visible.filter(|visible| *visible != self.interaction.focus_visible)
        {
            self.interaction.focus_visible = visible;
            self.invalidate();
        }
        if !matches!(input, Input::PointerMove { .. }) {
            self.interaction.hover = None;
        }
        match input {
            Input::TouchStart { id, x, y } => self.touch_start(id, x, y),
            Input::TouchMove { id, x, y } => self.touch_move(id, x, y),
            Input::TouchEnd { id } => self.touch_end(id),
            Input::TouchCancel { id } => self.touch_cancel(id),
            Input::PointerMove { x, y, fine } => {
                let mut hover_changed = self
                    .target(self.interaction.pointer[0], self.interaction.pointer[1])
                    != self.target(x, y)
                    || self
                        .scene
                        .tooltip_at(self.interaction.pointer[0], self.interaction.pointer[1])
                        != self.scene.tooltip_at(x, y);
                self.interaction.pointer = [x, y];
                if let Some(Target::Option(index)) = self.target(x, y) {
                    if let Some(menu) = &mut self.interaction.chooser {
                        hover_changed |= menu.highlighted != index;
                        menu.highlighted = index;
                    }
                }
                if hover_changed {
                    self.invalidate();
                }
                if self.interaction.scrollbar_drag.is_some() {
                    self.scroll_to_pointer(y);
                }
                if self.interaction.midi.pointer_down {
                    let note = match self.target(x, y) {
                        Some(Target::Note(n)) => Some(n),
                        _ => None,
                    };
                    self.pointer_note(note);
                }
                self.move_number(x, fine);
                if let Some(mut g) = self.interaction.gesture.take() {
                    let delta = if let Some(slider) = g.slider {
                        f64::from(x - g.previous[0])
                            / f64::from(
                                (slider.0[2] - crate::controls::SLIDER_INSET * 2.0).max(1.0),
                            )
                    } else {
                        f64::from(g.previous[1] - y) / 250.0
                    };
                    g.previous = [x, y];
                    g.normalized =
                        (g.normalized + delta * if fine { 0.1 } else { 1.0 }).clamp(0.0, 1.0);
                    if let Some(d) = self.controls[g.index].domain {
                        let field = Field::Control(g.index);
                        if let Some(value) =
                            self.numeric_value(&field, d.normalized_to_plain(g.normalized))
                        {
                            self.set_field(&field, value, false);
                        }
                    }
                    self.interaction.gesture = Some(g);
                }
            }
            Input::PointerDown { x, y, clicks } => {
                self.interaction.pointer = [x, y];
                let target = self.target(x, y);
                if clicks < 2
                    && matches!(&target, Some(Target::Field(field)) if self.editor.begin_selection(field))
                {
                    return true;
                }
                // Transport must remain reachable even while a numeric draft is invalid.
                if target == Some(Target::Transport) {
                    self.cancel_interaction();
                    self.interaction.focus = Some(Target::Transport);
                    self.activate(Target::Transport, x, y, clicks);
                    return true;
                }
                if self.interaction.chooser.is_some() && !Rect(self.scene.menu_rect).contains(x, y)
                {
                    let same = matches!(&target,Some(Target::Choose(id)) if self.interaction.chooser.as_ref().is_some_and(|m|m.id==*id));
                    self.interaction.chooser = None;
                    self.invalidate();
                    if same {
                        return true;
                    }
                }
                if self.interaction.devices_open
                    && !Rect(self.scene.popup_rect).contains(x, y)
                    && !matches!(target, Some(Target::Option(_)))
                    && target != Some(Target::Devices)
                {
                    self.interaction.devices_open = false;
                    self.interaction.chooser = None;
                    self.interaction.focus = None;
                    self.invalidate();
                    return true;
                }
                if !self.commit_draft() {
                    self.editor.cancel_draft();
                    self.invalidate();
                }
                self.cancel_gesture();
                self.cancel_number();
                self.interaction.focus = target.clone();
                if let Some(target) = target {
                    self.activate(target, x, y, clicks);
                }
            }
            Input::PointerLeave => {
                self.interaction.pointer = [-1.0, -1.0];
                self.invalidate();
            }
            Input::PointerUp => {
                let captured = self.pointer_captured();
                self.editor.finish_selection();
                self.interaction.scrollbar_drag = None;
                self.interaction.midi.pointer_down = false;
                self.finish_number();
                if let Some(g) = self.interaction.gesture.take() {
                    self.commands.push(self.controls[g.index].command(true));
                }
                self.pointer_note(None);
                if captured {
                    self.invalidate();
                }
            }
            Input::Scroll { delta } => {
                return self.scroll(delta, false);
            }
            Input::HorizontalScroll { delta } => {
                return self.scroll(delta, true);
            }
            Input::Text { text } if self.interaction.chooser.is_some() => self.select_text(&text),
            input @ (Input::Text { .. }
            | Input::Preedit { .. }
            | Input::TextMark { .. }
            | Input::EditText { .. }
            | Input::SelectText { .. }) => {
                if !self.editor.editing() {
                    return false;
                }
                if self.editor.input(input) {
                    self.invalidate();
                }
            }
            Input::Key { key, shift } => {
                let consumed = self.key(key, shift);
                if consumed && !self.interaction.focus_visible {
                    self.interaction.focus_visible = true;
                    self.invalidate();
                }
                return consumed;
            }
            Input::ComputerKey { key, pressed } => return self.computer_key(key, pressed),
            Input::ReleaseComputerKeys => {
                self.commands
                    .extend(self.interaction.midi.release_keyboard());
                self.invalidate();
            }
            Input::FocusLost => {
                self.interaction.pointer = [-1.0, -1.0];
                self.commit_draft();
                self.presentation.flush();
                self.cancel_interaction();
                self.invalidate();
            }
        }
        true
    }
    fn activate(&mut self, target: Target, x: f32, y: f32, clicks: u8) {
        match target {
            Target::Transport => {
                if self.state["supportsTransport"] != false {
                    self.commands
                        .push(json!({"type":if self.state["running"] == true {
                            "stop"
                        } else {
                            "start"
                        }}));
                }
            }
            Target::Command(command) => {
                match command["type"].as_str() {
                    Some("resetEventArguments") => {
                        self.event_layout.get_mut().take();
                        for e in &mut self.events {
                            e.reset_arguments();
                        }
                        self.presentation.changed();
                    }
                    Some("clearLog") => {
                        self.state["logText"] = json!("");
                        self.state["logEntries"] = json!([]);
                        self.log_revision = self.log_revision.wrapping_add(1);
                        self.log_max_scroll_x = 0.0;
                        self.presentation.log_scroll = 0.0;
                        self.presentation.log_x = 0.0;
                        self.presentation.changed();
                    }
                    _ => {}
                }
                self.commands.push(command);
            }
            Target::Layout { input, knobs } => {
                if input {
                    self.input_knobs = knobs;
                } else {
                    self.knobs = knobs;
                }
                self.state[if input {
                    "inputControlLayout"
                } else {
                    "paramLayout"
                }] = json!(if knobs { "knobs" } else { "sliders" });
                self.presentation.changed();
                self.commands.push(json!({"type":if input {"setInputControlLayout"}else{"setParamLayout"},"layout":if knobs {"knobs"}else{"sliders"}}));
            }
            Target::Devices => self.toggle_settings(),
            Target::FastMath => {
                let enabled = self.state["fastMath"] != true;
                self.state["fastMath"] = json!(enabled);
                self.commands
                    .push(json!({"type":"setFastMath","enabled":enabled}));
            }
            Target::Scrollbar => {
                if let Some(bar) = self.scene.scrollbar {
                    let grabbed = bar.thumb.contains(x, y);
                    self.interaction.scrollbar_drag = Some(if grabbed {
                        y - bar.thumb.0[1]
                    } else {
                        bar.thumb.0[3] * 0.5
                    });
                    if !grabbed {
                        self.scroll_to_pointer(y);
                    }
                }
            }
            Target::Control(index) => {
                let c = &self.controls[index];
                if !c.enabled() {
                    return;
                }
                if clicks >= 2 && !c.metadata["default"].is_null() {
                    let value = c.metadata["default"].clone();
                    self.set_field(&Field::Control(index), value, true);
                } else if c.boolean() {
                    let value = json!(c.value != true);
                    self.set_field(&Field::Control(index), value, true);
                } else if c.domain.is_some() {
                    let slider = if self.bank_knobs(c.input) {
                        None
                    } else {
                        self.scene
                            .hits
                            .iter()
                            .find(|h| h.target == Target::Control(index))
                            .map(|h| h.rect)
                    };
                    let normalized = if let Some(r) = slider {
                        f64::from(
                            (x - r.0[0] - crate::controls::SLIDER_INSET)
                                / (r.0[2] - crate::controls::SLIDER_INSET * 2.0).max(1.0),
                        )
                        .clamp(0.0, 1.0)
                    } else {
                        c.normalized()
                    };
                    self.interaction.gesture = Some(Gesture {
                        index,
                        previous: [x, y],
                        normalized,
                        slider,
                    });
                    let d = self.controls[index].domain.unwrap();
                    if let Some(value) = self
                        .numeric_value(&Field::Control(index), d.normalized_to_plain(normalized))
                    {
                        self.set_field(&Field::Control(index), value, false);
                    }
                }
            }
            Target::Field(field) => {
                if !self.field_enabled(&field) {
                    return;
                }
                let value = self.field_value(&field);
                if clicks >= 2 {
                    if let Some(default) = self.field_default(&field).filter(|v| !v.is_null()) {
                        self.set_field(&field, default, true);
                    }
                } else if value.is_boolean() {
                    self.set_field(&field, json!(value != true), true);
                } else if self.field_numeric(&field) {
                    self.editor.begin_drag(field, x);
                } else {
                    self.begin_field(field);
                }
            }
            Target::Section(section) => {
                self.presentation.toggle(section);
                self.cancel_interaction();
                self.interaction.focus = Some(Target::Section(section));
            }
            Target::Fold(key) => {
                self.event_layout.get_mut().take();
                self.clear_control_layouts();
                if !self.presentation.folded.remove(&key) {
                    self.presentation.folded.insert(key);
                }
                self.presentation.changed();
            }
            Target::Trigger(i) => {
                let event = &self.events[i];
                if event.valid() && self.state["connected"] != false {
                    self.commands.push(json!({"type":"triggerEvent","engineGeneration":self.state["engineGeneration"],"name":event.name,"values":event.args.iter().map(|a|&a.value).collect::<Vec<_>>()}));
                }
            }
            Target::Slice {
                event,
                arg,
                path,
                add,
            } => {
                self.event_layout.get_mut().take();
                let a = &mut self.events[event].args[arg];
                if let Some(value) = a.value.pointer_mut(&path).and_then(Value::as_array_mut) {
                    // Find the corresponding nested shape using the value path.
                    let mut shape = &a.shape;
                    for part in path.split('/').skip(1) {
                        let name = part.replace("~1", "/").replace("~0", "~");
                        shape = match shape["kind"].as_str() {
                            Some("struct") => shape["fields"]
                                .as_array()
                                .and_then(|fs| fs.iter().find(|f| f["name"] == name))
                                .map(|f| &f["ty"])
                                .unwrap_or(&Value::Null),
                            Some("tuple") => name
                                .parse::<usize>()
                                .ok()
                                .and_then(|i| shape["elements"].get(i))
                                .unwrap_or(&Value::Null),
                            _ => &shape["element"],
                        };
                    }
                    if add && value.len() < 4096 {
                        value.push(events::default_value(&shape["element"]));
                    } else if !add {
                        value.pop();
                    }
                    self.presentation.changed();
                }
            }
            Target::Follow => {
                self.presentation.log_follow = !self.presentation.log_follow;
                self.presentation.changed();
            }
            Target::CopyLog => self.commands.push(json!({
                "type":"copyText","text":self.state["logText"].as_str().unwrap_or("")
            })),
            Target::Note(n) => {
                self.interaction.midi.pointer_down = true;
                self.pointer_note(Some(n));
            }
            Target::Choose(id) => self.open_select(id),
            Target::Option(index) => self.choose_option(index),
        }
        self.invalidate();
    }
    fn pointer_note(&mut self, note: Option<u8>) {
        let old = self.interaction.midi.pointer;
        if old == note {
            return;
        }
        let already = note.is_some_and(|n| self.interaction.midi.held(n));
        self.interaction.midi.pointer = note;
        if let Some(old) = old {
            if !self.interaction.midi.held(old) {
                self.commands
                    .push(self.interaction.midi.command(old, false));
            }
        }
        if let Some(note) = note {
            if !already {
                self.commands
                    .push(self.interaction.midi.command(note, true));
            }
        }
        self.invalidate();
    }
    fn computer_key(&mut self, key: String, pressed: bool) -> bool {
        let key = key.to_lowercase();
        if !pressed {
            let consumed = self.interaction.midi.pressed_keys.remove(&key);
            if let Some(note) = self.interaction.midi.keyboard.remove(&key) {
                if !self.interaction.midi.held(note) {
                    self.commands
                        .push(self.interaction.midi.command(note, false));
                }
                self.invalidate();
            }
            return consumed;
        }
        if self.interaction.midi.pressed_keys.contains(&key) {
            return true;
        }
        if self.editor.editing()
            || self.interaction.devices_open
            || self.interaction.chooser.is_some()
            || !self.presentation.open(Section::Midi)
            || !crate::layout::midi_visible(self)
            || !self.midi_interactive()
            || !self.state["currentMidiInputDevice"]
                .as_str()
                .map_or(self.state["supportsMidiInputSelection"] != true, |device| {
                    device == "Computer Keyboard"
                })
        {
            return false;
        }
        if key == "z" || key == "x" {
            if self.interaction.midi.pressed_keys.insert(key.clone()) {
                self.commands
                    .extend(
                        self.interaction
                            .midi
                            .shift_octave(if key == "z" { -1 } else { 1 }),
                    );
                self.presentation.changed();
                self.invalidate();
            }
            return true;
        }
        let Some((_, offset)) = KEYS.iter().find(|(c, _)| key == c.to_string()) else {
            return false;
        };
        self.interaction.midi.pressed_keys.insert(key.clone());
        let note = (((self.interaction.midi.octave + 1) * 12).clamp(0, 108) as u8) + offset;
        let already = self.interaction.midi.held(note);
        self.interaction.midi.keyboard.insert(key, note);
        if !already {
            self.commands
                .push(self.interaction.midi.command(note, true));
        }
        self.invalidate();
        true
    }
    fn key(&mut self, key: Key, shift: bool) -> bool {
        if !matches!(key, Key::Play | Key::Stop) && self.select_key(&key) {
            return true;
        }
        match key {
            Key::Escape => {
                if !self.editor.editing()
                    && !self.pointer_captured()
                    && self.interaction.focus.is_none()
                    && !self.interaction.devices_open
                    && self.interaction.touch.is_none()
                    && !self.interaction.midi.pointer_down
                    && self.interaction.midi.pressed_keys.is_empty()
                    && self.interaction.midi.touches.is_empty()
                {
                    return false;
                }
                self.cancel_interaction();
                self.interaction.devices_open = false;
                self.interaction.chooser = None;
            }
            Key::Enter => {
                if self.editor.editing() {
                    self.commit_draft();
                } else if let Some(target) = self.interaction.focus.clone() {
                    if let Target::Field(field) = target {
                        if self.field_value(&field).is_boolean() {
                            self.set_field(&field, json!(self.field_value(&field) != true), true);
                        } else {
                            self.begin_field(field);
                        }
                    } else {
                        self.activate(
                            target,
                            self.interaction.pointer[0],
                            self.interaction.pointer[1],
                            1,
                        );
                    }
                } else if self.state["supportsTransport"] != false {
                    self.commands.push(json!({"type":"start"}));
                } else {
                    return false;
                }
            }
            Key::Play | Key::Stop => {
                if self.state["supportsTransport"] == false {
                    return false;
                }
                self.cancel_interaction();
                self.commands
                    .push(json!({"type":if matches!(key,Key::Play) {"start"} else {"stop"}}));
            }
            Key::Tab => return self.tab(shift),
            Key::ArrowUp | Key::ArrowDown if self.editor.editing() => {
                self.step_field(matches!(key, Key::ArrowUp))
            }
            key if self.editor.editing() => {
                if let Some(text) = self.editor.text_key(key, shift) {
                    self.commands.push(json!({"type":"copyText", "text":text}));
                }
            }
            _ => return false,
        }
        self.invalidate();
        true
    }
    fn scroll_to_pointer(&mut self, y: f32) {
        let Some(bar) = self.scene.scrollbar else {
            return;
        };
        let Some(grab) = self.interaction.scrollbar_drag else {
            return;
        };
        let scroll = bar.scroll_at(y, grab);
        if self.scroll == scroll {
            return;
        }
        self.scroll = scroll;
        self.presentation.scrolled();
        self.invalidate();
    }
    pub(crate) fn cancel_gesture(&mut self) {
        if let Some(g) = self.interaction.gesture.take() {
            self.commands.push(self.controls[g.index].end_command());
        }
    }
    pub(crate) fn toggle_settings(&mut self) {
        self.interaction.devices_open = !self.interaction.devices_open;
        self.interaction.popup_scroll = 0.0;
        self.interaction.chooser = None;
        self.invalidate();
    }
    /// CSS cursor names are shared by native, browser, and embedded hosts.
    pub fn cursor(&mut self) -> &'static str {
        self.prepare_scene();
        if self.interaction.scrollbar_drag.is_some() {
            return "default";
        }
        if self.editor.has_number_drag() {
            return "ew-resize";
        }
        if let Some(g) = &self.interaction.gesture {
            return if g.slider.is_some() {
                "pointer"
            } else {
                "ns-resize"
            };
        }
        match self.target(self.interaction.pointer[0], self.interaction.pointer[1]) {
            Some(Target::Control(i)) => {
                if self.bank_knobs(self.controls[i].input) {
                    "ns-resize"
                } else {
                    "pointer"
                }
            }
            Some(Target::Field(field)) => {
                if self.editor.draft().is_some_and(|d| d.target == field) {
                    "text"
                } else if self.field_numeric(&field) {
                    "ew-resize"
                } else if self.field_value(&field).is_boolean() {
                    "pointer"
                } else {
                    "text"
                }
            }
            Some(Target::Scrollbar) => "default",
            Some(_) => "pointer",
            None => "default",
        }
    }
    pub fn editing(&self) -> bool {
        self.editor.editing()
    }
}
