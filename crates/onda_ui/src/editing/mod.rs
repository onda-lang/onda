mod numeric;
mod text;

use crate::{
    events::{self, EventField},
    model::RunUi,
};
pub(crate) use numeric::number_text;
use serde_json::{json, Value};
pub(crate) use text::{byte_offset, Draft};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Setting {
    SampleRate,
    BlockSize,
    ParamSmoothing,
    InputSmoothing,
    Octave,
    Velocity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Field {
    Control(usize),
    Event(EventField),
    Setting(Setting),
}

/// Owns drafts, text selection, composition, and pending numeric drags.
/// Field bindings remain in RunUi; rendering only borrows the active draft.
#[derive(Default)]
pub(crate) struct Editor {
    draft: Option<Draft>,
    number_drag: Option<numeric::NumberDrag>,
    selecting_text: bool,
}

impl Editor {
    pub(crate) fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }
    pub(crate) fn editing(&self) -> bool {
        self.draft.is_some()
    }
    pub(crate) fn selecting_text(&self) -> bool {
        self.selecting_text
    }
    pub(crate) fn begin_selection(&mut self, field: &Field) -> bool {
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft.target == *field)
        {
            self.selecting_text = true;
            true
        } else {
            false
        }
    }
    pub(crate) fn finish_selection(&mut self) {
        self.selecting_text = false;
    }
    pub(crate) fn pointer_captured(&self) -> bool {
        self.number_drag.is_some() || self.selecting_text
    }
    pub(crate) fn has_number_drag(&self) -> bool {
        self.number_drag.is_some()
    }
    pub(crate) fn dragging_field(&self, field: &Field) -> bool {
        self.number_drag
            .as_ref()
            .is_some_and(|drag| drag.field == *field)
    }
    pub(crate) fn active_control(&self) -> Option<usize> {
        self.number_drag
            .as_ref()
            .filter(|drag| drag.active)
            .and_then(|drag| match drag.field {
                Field::Control(index) => Some(index),
                _ => None,
            })
    }
    pub(crate) fn begin_drag(&mut self, field: Field, x: f32) {
        self.number_drag = Some(numeric::NumberDrag::new(field, x));
    }
    fn begin(&mut self, target: Field, text: String) {
        self.draft = Some(Draft {
            target,
            cursor: text.len(),
            original: text.clone(),
            text,
            anchor: 0,
            invalid: false,
            preedit: None,
            mark: None,
        });
    }
    fn cancel_drag(&mut self) -> Option<Field> {
        self.number_drag
            .take()
            .filter(|drag| drag.active)
            .map(|drag| drag.field)
    }
    fn cancel(&mut self) -> Option<Field> {
        self.cancel_draft();
        self.cancel_drag()
    }
    pub(crate) fn cancel_draft(&mut self) {
        self.draft = None;
        self.selecting_text = false;
    }
}

impl RunUi {
    pub(crate) fn setting_bounds(&self, setting: Setting) -> (f64, f64, bool) {
        match setting {
            Setting::SampleRate => (1.0, 768000.0, true),
            Setting::BlockSize => (
                if self.state.get("hostBlockSamples").is_some() {
                    0.0
                } else {
                    1.0
                },
                65536.0,
                true,
            ),
            Setting::Octave => (-1.0, 7.0, true),
            Setting::Velocity => (0.0, 1.0, false),
            Setting::ParamSmoothing | Setting::InputSmoothing => (0.0, f64::MAX, false),
        }
    }
    pub(crate) fn field_value(&self, field: &Field) -> Value {
        match field {
            Field::Control(i) => self.controls[*i].value.clone(),
            Field::Event(f) => self
                .events
                .get(f.event)
                .and_then(|e| e.args.get(f.arg))
                .and_then(|a| a.value.pointer(&f.path))
                .cloned()
                .unwrap_or(Value::Null),
            Field::Setting(s) => match s {
                Setting::SampleRate => self.state["sampleRateHz"].clone(),
                Setting::BlockSize => self
                    .state
                    .get("preferredBlockSamples")
                    .cloned()
                    .unwrap_or_else(|| self.state["blockSamples"].clone()),
                Setting::ParamSmoothing => self.state["paramSmoothingMs"].clone(),
                Setting::InputSmoothing => self.state["inputControlSmoothingMs"].clone(),
                Setting::Octave => json!(self.interaction.midi().octave),
                Setting::Velocity => json!(self.interaction.midi().velocity),
            },
        }
    }
    pub(crate) fn field_default(&self, field: &Field) -> Option<Value> {
        match field {
            Field::Control(i) => self.controls[*i].metadata.get("default").cloned(),
            Field::Event(f) => self
                .events
                .get(f.event)?
                .args
                .get(f.arg)?
                .default
                .pointer(&f.path)
                .cloned(),
            Field::Setting(s) => Some(match s {
                Setting::ParamSmoothing => self.state["paramSmoothingDefaultMs"].clone(),
                Setting::InputSmoothing => self.state["inputControlSmoothingDefaultMs"].clone(),
                Setting::SampleRate => json!(48000),
                Setting::BlockSize => json!(512),
                Setting::Octave => json!(4),
                Setting::Velocity => json!(crate::interaction::DEFAULT_VELOCITY),
            }),
        }
    }
    pub(crate) fn parse_field(&self, field: &Field, text: &str) -> Option<Value> {
        match field {
            Field::Control(i) => self.controls[*i].parse(text),
            Field::Event(f) => {
                if f.scalar == "json" {
                    let value = serde_json::from_str(text).ok()?;
                    let arg = self.events.get(f.event)?.args.get(f.arg)?;
                    events::canonical(&arg.shape, &value)
                } else {
                    events::scalar_value(&f.scalar, text)
                }
            }
            Field::Setting(s) => {
                let n = text.trim().parse::<f64>().ok().filter(|n| n.is_finite())?;
                let (min, max, integer) = self.setting_bounds(*s);
                (n >= min && n <= max && (!integer || n.fract() == 0.0)).then(|| {
                    if integer {
                        json!(n as i64)
                    } else {
                        json!(match s {
                            Setting::ParamSmoothing | Setting::InputSmoothing => n.round(),
                            Setting::Velocity => (n * 100.0).round() / 100.0,
                            _ => n,
                        })
                    }
                })
            }
        }
    }
    pub(crate) fn set_field(&mut self, field: &Field, value: Value, commit: bool) {
        if !self.field_enabled(field) {
            return;
        }
        if self.field_value(field) == value && (!commit || !matches!(field, Field::Control(_))) {
            return;
        }
        match field {
            Field::Control(i) => {
                self.controls[*i].value = value;
                self.commands.push(self.controls[*i].command(commit));
            }
            Field::Event(f) => {
                if f.scalar == "json" {
                    self.event_layout.get_mut().take();
                }
                if let Some(target) = self
                    .events
                    .get_mut(f.event)
                    .and_then(|e| e.args.get_mut(f.arg))
                    .and_then(|a| a.value.pointer_mut(&f.path))
                {
                    *target = value;
                }
                self.presentation.changed();
            }
            Field::Setting(s) => match s {
                Setting::SampleRate | Setting::BlockSize => {
                    let key = if *s == Setting::SampleRate {
                        "sampleRateHz"
                    } else if self.state.get("preferredBlockSamples").is_some() {
                        "preferredBlockSamples"
                    } else {
                        "blockSamples"
                    };
                    self.state[key] = value;
                    self.commands.push(json!({"type":"setRunSettings","sampleRateHz":self.state["sampleRateHz"],"blockSamples":self.field_value(&Field::Setting(Setting::BlockSize))}));
                }
                Setting::ParamSmoothing | Setting::InputSmoothing => {
                    self.state[if *s == Setting::ParamSmoothing {
                        "paramSmoothingMs"
                    } else {
                        "inputControlSmoothingMs"
                    }] = value.clone();
                    self.commands.push(json!({"type":if *s == Setting::ParamSmoothing {"setParamSmoothing"} else {"setInputControlSmoothing"},"milliseconds":value}));
                }
                Setting::Octave => {
                    self.commands.extend(
                        self.interaction
                            .set_octave(value.as_i64().unwrap_or(4) as i32),
                    );
                    self.presentation.changed();
                }
                Setting::Velocity => {
                    self.interaction.set_velocity(value.as_f64().unwrap_or(0.0));
                    self.presentation.changed();
                }
            },
        }
        self.invalidate();
    }
    pub(crate) fn begin_field(&mut self, target: Field) {
        if !self.field_enabled(&target) {
            return;
        }
        self.interaction.focus_field(target.clone());
        let value = self.field_value(&target);
        let text = if self.field_numeric(&target) {
            crate::editing::number_text(&value, 5)
        } else {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        };
        self.editor.begin(target, text);
        self.invalidate();
    }
    pub(crate) fn commit_draft(&mut self) -> bool {
        let Some(d) = &self.editor.draft else {
            return true;
        };
        if d.preedit.is_some() || d.mark.is_some() {
            return false;
        }
        if d.text == d.original {
            self.editor.draft = None;
            self.invalidate();
            return true;
        }
        let Some(value) = self.parse_field(&d.target, &d.text) else {
            self.editor.draft.as_mut().unwrap().invalid = true;
            self.invalidate();
            return false;
        };
        let field = d.target.clone();
        if self.field_value(&field) != value {
            self.set_field(&field, value, true);
        }
        self.editor.draft = None;
        self.invalidate();
        true
    }

    pub(crate) fn cancel_editing(&mut self) {
        if let Some(field) = self.editor.cancel() {
            self.end_edit_gesture(field);
        }
    }

    fn end_edit_gesture(&mut self, field: Field) {
        if let Field::Control(index) = field {
            self.commands.push(self.controls[index].end_command());
        }
    }
}
