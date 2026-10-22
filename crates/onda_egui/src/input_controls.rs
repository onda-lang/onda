//! Optional host-owned input controls. Stock Onda does not supply these inputs.
//! Metadata uses the same scalar fields as the shared web run view, including
//! `enabled: false` for an input supplied by an external bus.

use eframe::egui;
use serde_json::Value;

use crate::{
    default_value_for_type, json_number, param_name, param_type, prepared_param_domain,
    render_compact_param_value_editor, render_control_header, render_param_grid,
    render_param_value_editor, section_box, ParamControlSpec, ParamEditOutcome, ParamLayout,
};

#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq)]
pub enum InputControlAction {
    SetValue { name: String, value: Value },
    SetLayout(ParamLayout),
    SetSmoothing(f64),
    Reset,
}

/// Retain one section per host instance. The host owns values, smoothing, and
/// persistence, and applies the returned actions to its input-control bank.
pub struct InputControlSection {
    pub layout: ParamLayout,
    pub smoothing_seconds: f64,
    pub default_smoothing_seconds: f64,
}

impl InputControlSection {
    pub fn new(default_smoothing_seconds: f64) -> Self {
        Self {
            layout: ParamLayout::Knobs,
            smoothing_seconds: default_smoothing_seconds,
            default_smoothing_seconds,
        }
    }

    /// An empty list hides the section. Names identify controls within the bank;
    /// the caller's egui ID scope identifies the host instance.
    pub fn show(&mut self, ui: &mut egui::Ui, controls: &[Value]) -> Vec<InputControlAction> {
        let mut actions = Vec::new();
        if controls.is_empty() {
            return actions;
        }
        ui.push_id("input-controls", |ui| {
            section_box(ui, |ui| {
                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    ui.make_persistent_id("section"),
                    true,
                );
                let previous_layout = self.layout;
                let header = render_control_header(
                    ui,
                    &mut state,
                    "Input Controls",
                    &mut self.layout,
                    &mut self.smoothing_seconds,
                    self.default_smoothing_seconds,
                );
                if header.reset {
                    actions.push(InputControlAction::Reset);
                }
                if header.smoothing_changed {
                    actions.push(InputControlAction::SetSmoothing(self.smoothing_seconds));
                }
                if previous_layout != self.layout {
                    actions.push(InputControlAction::SetLayout(self.layout));
                }
                state.show_body_unindented(ui, |ui| {
                    ui.add_space(8.0);
                    actions.extend(
                        render_control_values(ui, controls, self.layout)
                            .into_iter()
                            .map(|(name, value)| InputControlAction::SetValue { name, value }),
                    );
                });
            });
        });
        actions
    }
}

pub(crate) fn render_control_values(
    ui: &mut egui::Ui,
    controls: &[Value],
    layout: ParamLayout,
) -> Vec<(String, Value)> {
    let mut edits = Vec::new();
    for (index, group) in controls
        .chunk_by(|a, b| a.pointer("/array/name") == b.pointer("/array/name"))
        .enumerate()
    {
        if index > 0 {
            ui.add_space(8.0);
        }
        ui.push_id(param_name(&group[0]), |ui| {
            let mut grid = |ui: &mut egui::Ui| {
                render_param_grid(ui, group, layout, |ui, control| {
                    if let Some(edit) =
                        render_control_value(ui, control, layout == ParamLayout::Knobs)
                    {
                        edits.push(edit);
                    }
                });
            };
            if let Some(name) = group[0].pointer("/array/name").and_then(Value::as_str) {
                egui::CollapsingHeader::new(format!("{name} [{}]", group.len()))
                    .default_open(true)
                    .show(ui, grid);
            } else {
                grid(ui);
            }
        });
    }
    edits
}

fn render_control_value(
    ui: &mut egui::Ui,
    control: &Value,
    compact: bool,
) -> Option<(String, Value)> {
    let name = param_name(control)?;
    let ty = param_type(control);
    let spec = ParamControlSpec {
        label: control.get("label").and_then(Value::as_str).unwrap_or(name),
        unit: control.get("unit").and_then(Value::as_str),
        ty,
        default: control
            .get("default")
            .map(|value| crate::NumberValue::from_json(ty, Some(value))),
        domain: prepared_param_domain(control),
    };
    let value = control
        .get("value")
        .cloned()
        .unwrap_or_else(|| default_value_for_type(ty));
    let external = control.get("enabled").and_then(Value::as_bool) == Some(false);
    let fixed = control
        .get("rangeMin")
        .and_then(Value::as_f64)
        .is_some_and(|minimum| control.get("rangeMax").and_then(Value::as_f64) == Some(minimum));
    let outcome = ui
        .add_enabled_ui(!external && !fixed, |ui| {
            if external {
                ui.label("External input");
            }
            if compact {
                render_compact_param_value_editor(ui, spec, &value)
            } else {
                render_param_value_editor(ui, spec, &value)
            }
        })
        .inner;
    if external || fixed {
        return None;
    }
    match outcome {
        ParamEditOutcome::None => None,
        ParamEditOutcome::Commit(value) => {
            let value = value
                .as_f64()
                .map(|value| json_number(spec.constrain_plain(value)))
                .unwrap_or(value);
            Some((name.to_owned(), value))
        }
    }
}
