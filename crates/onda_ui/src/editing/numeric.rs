//! Numeric interaction policy shared by parameter, event, and settings fields.
use crate::{
    editing::{Field, Setting},
    model::{number, RunUi},
};
use onda_mir::{ParamDomain, ParamScale, ScalarType};
use serde_json::Value;

/// Compact float presentation never changes the stored value or exact integers.
pub(crate) fn number_text(value: &Value, decimals: usize) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(n) if n.is_f64() => {
            let n = n.as_f64().unwrap();
            let text = if n != 0.0
                && (n.abs() < 0.0001_f64.max(0.5 * 10.0_f64.powi(-(decimals as i32)))
                    || n.abs() >= 1e9)
            {
                format!("{n:.decimals$e}")
            } else {
                format!("{n:.decimals$}")
            };
            let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, ""));
            let mantissa = if mantissa.contains('.') {
                mantissa.trim_end_matches('0').trim_end_matches('.')
            } else {
                mantissa
            };
            if exponent.is_empty() {
                mantissa.into()
            } else {
                format!("{mantissa}e{exponent}")
            }
        }
        value => value.to_string(),
    }
}

pub(crate) struct NumberDrag {
    pub field: Field,
    pub active: bool,
    start: f32,
    previous: f32,
    initial: Value,
    position: f64,
}
impl NumberDrag {
    pub fn new(field: Field, x: f32) -> Self {
        Self {
            field,
            active: false,
            start: x,
            previous: x,
            initial: Value::Null,
            position: 0.0,
        }
    }
    pub fn move_to(&mut self, ui: &RunUi, x: f32, fine: bool) -> Option<Value> {
        if !self.active {
            if (x - self.start).abs() < 3.0 {
                return None;
            }
            self.initial = ui.field_value(&self.field);
            number(&self.initial)?;
            self.position = ui
                .numeric_domain(&self.field)
                .map(|d| d.plain_to_normalized(number(&self.initial).unwrap()))
                .unwrap_or(0.0);
            self.active = true;
        }
        let delta = f64::from(x - self.previous) * if fine { 0.1 } else { 1.0 };
        self.previous = x;
        if let Some(domain) = ui.numeric_domain(&self.field) {
            self.position = (self.position + delta / 375.0).clamp(0.0, 1.0);
            ui.numeric_value(&self.field, domain.normalized_to_plain(self.position))
        } else {
            self.position += delta * ui.field_drag_speed(&self.field);
            if let Field::Setting(setting) = &self.field {
                let initial = number(&self.initial)?;
                let (min, max, _) = ui.setting_bounds(*setting);
                // Clamp the accumulator too, so reversal responds at the bound.
                self.position = (initial + self.position).clamp(min, max) - initial;
            }
            if ui.field_integer(&self.field) {
                let start = self
                    .initial
                    .as_i64()
                    .or_else(|| self.initial.as_str()?.parse().ok())?;
                let (min, max) = ui.integer_bounds(&self.field);
                self.position = self.position.clamp(
                    (min - i128::from(start)) as f64,
                    (max - i128::from(start)) as f64,
                );
                let next = (i128::from(start) + self.position.round() as i128).clamp(min, max);
                ui.parse_field(&self.field, &next.to_string())
            } else {
                ui.numeric_value(&self.field, number(&self.initial)? + self.position)
            }
        }
    }
}
impl RunUi {
    pub(crate) fn field_text(&self, field: &Field) -> String {
        let step = format!("{:e}", self.field_step(field));
        let (coefficient, exponent) = step.split_once('e').unwrap();
        let fraction = coefficient
            .split_once('.')
            .map_or(0, |(_, part)| part.len());
        let decimals = (fraction as i32 - exponent.parse::<i32>().unwrap()).clamp(0, 16);
        number_text(&self.field_value(field), decimals as usize)
    }

    fn integer_bounds(&self, field: &Field) -> (i128, i128) {
        let i32 = match field {
            Field::Control(i) => self.controls[*i].metadata["type"] == "i32",
            Field::Event(f) => f.scalar == "i32",
            Field::Setting(_) => false,
        };
        if i32 {
            (i128::from(i32::MIN), i128::from(i32::MAX))
        } else {
            (i128::from(i64::MIN), i128::from(i64::MAX))
        }
    }
    fn field_drag_speed(&self, field: &Field) -> f64 {
        match field {
            Field::Setting(Setting::ParamSmoothing | Setting::InputSmoothing) => 1.0,
            Field::Setting(Setting::Octave) => 0.1,
            _ if self.field_integer(field) => 0.25,
            _ => 0.01,
        }
    }
    pub(crate) fn field_enabled(&self, field: &Field) -> bool {
        match field {
            Field::Control(i) => self.controls.get(*i).is_some_and(|c| c.enabled()),
            Field::Event(f) => self
                .events
                .get(f.event)
                .and_then(|e| e.args.get(f.arg))
                .is_some(),
            Field::Setting(s) => match s {
                Setting::SampleRate => {
                    self.state["supportsRunSettings"] == true
                        && (!self.loaded() || self.state["canSetSampleRate"] == true)
                        && self.state.get("hostBlockSamples").is_none()
                }
                Setting::BlockSize => {
                    self.state["supportsRunSettings"] == true
                        && if self.state.get("hostBlockSamples").is_some() {
                            self.state["canSetBlockSize"] == true
                        } else {
                            !self.loaded() || self.state["canSetBlockSize"] == true
                        }
                }
                Setting::ParamSmoothing => self.state["supportsParamSmoothing"] == true,
                Setting::InputSmoothing => self.state["supportsInputControlSmoothing"] == true,
                Setting::Octave | Setting::Velocity => self.midi_interactive(),
            },
        }
    }
    pub(crate) fn loaded(&self) -> bool {
        self.state["path"].as_str().is_some_and(|s| !s.is_empty())
            || !self.controls.is_empty()
            || !self.events.is_empty()
    }
    pub(crate) fn field_integer(&self, field: &Field) -> bool {
        match field {
            Field::Control(i) => matches!(
                self.controls[*i].metadata["type"].as_str(),
                Some("i32" | "i64")
            ),
            Field::Event(f) => matches!(f.scalar.as_str(), "i32" | "i64"),
            Field::Setting(s) => matches!(
                s,
                Setting::SampleRate | Setting::BlockSize | Setting::Octave
            ),
        }
    }
    pub(crate) fn field_numeric(&self, field: &Field) -> bool {
        self.field_enabled(field)
            && !matches!(field,Field::Event(f) if f.scalar=="json")
            && !self.field_value(field).is_boolean()
    }
    pub(crate) fn numeric_domain(&self, field: &Field) -> Option<ParamDomain<'static>> {
        match field {
            Field::Control(i) => self.controls[*i].domain,
            Field::Setting(Setting::Velocity) => ParamDomain::new(
                ScalarType::F64,
                0.0,
                1.0,
                ParamScale::Linear,
                None,
                None,
                None,
                None,
            ),
            _ => None,
        }
    }
    pub(crate) fn numeric_value(&self, field: &Field, n: f64) -> Option<Value> {
        let text = if self.field_integer(field) {
            (n.round() as i64).to_string()
        } else {
            n.to_string()
        };
        self.parse_field(field, &text)
    }
    pub(crate) fn field_step(&self, field: &Field) -> f64 {
        match field {
            Field::Setting(Setting::ParamSmoothing | Setting::InputSmoothing) => return 1.0,
            Field::Setting(Setting::Velocity) => return 0.01,
            _ => {}
        }
        if let Some(step) = self.numeric_domain(field).and_then(ParamDomain::step) {
            return step;
        }
        if self.field_integer(field) {
            return 1.0;
        }
        if let Some(d) = self.numeric_domain(field) {
            let step = ((d.maximum() - d.minimum()) / 2000.0).max(0.0001);
            return 10.0_f64.powf(step.log10().floor()).min(0.1);
        }
        0.001
    }
    pub(crate) fn step_field(&mut self, up: bool) {
        let Some(d) = &self.editor.draft else { return };
        let field = d.target.clone();
        if !self.field_numeric(&field) {
            return;
        }
        let step = self.field_step(&field) * if up { 1.0 } else { -1.0 };
        let current = if d.text == d.original {
            self.field_value(&field)
        } else {
            Value::String(d.text.clone())
        };
        let value = if self.field_integer(&field) {
            current
                .as_i64()
                .or_else(|| current.as_str()?.parse::<i64>().ok())
                .and_then(|n| n.checked_add(step as i64))
                .and_then(|n| self.parse_field(&field, &n.to_string()))
        } else {
            number(&current).and_then(|n| self.numeric_value(&field, n + step))
        };
        if let Some(value) = value {
            let d = self.editor.draft.as_mut().unwrap();
            d.text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            d.cursor = d.text.len();
            d.anchor = 0;
            d.invalid = false;
        }
    }
    pub(crate) fn move_number(&mut self, x: f32, fine: bool) {
        let Some(mut drag) = self.editor.number_drag.take() else {
            return;
        };
        if let Some(value) = drag.move_to(self, x, fine) {
            self.set_field(&drag.field, value, false);
        }
        self.editor.number_drag = Some(drag);
    }
    pub(crate) fn finish_number(&mut self) {
        let Some(drag) = self.editor.number_drag.take() else {
            return;
        };
        if drag.active {
            let value = self.field_value(&drag.field);
            self.set_field(&drag.field, value, true);
        } else {
            self.begin_field(drag.field);
        }
    }
    pub(crate) fn cancel_number(&mut self) {
        if let Some(field) = self.editor.cancel_drag() {
            self.end_edit_gesture(field);
        }
    }
}
