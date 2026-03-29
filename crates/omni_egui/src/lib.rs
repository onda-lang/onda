use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use eframe::egui;
use omni_preview::{PreviewController, PreviewHostOptions};
use serde_json::{Number, Value};

pub fn run_preview_egui(omni_path: &Path, options: PreviewHostOptions) -> Result<(), String> {
    let controller = PreviewController::new(omni_path, options)?;
    let title = format!(
        "Omni - {}",
        controller
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Preview".to_owned())
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([680.0, 820.0]),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        native_options,
        Box::new(move |_cc| Ok(Box::new(PreviewApp::new(controller)))),
    )
    .map_err(|err| format!("failed to start egui preview: {err}"))
}

struct PreviewApp {
    controller: PreviewController,
    event_inputs: HashMap<String, Vec<Value>>,
    number_drafts: HashMap<String, f64>,
}

impl PreviewApp {
    fn new(controller: PreviewController) -> Self {
        let mut app = Self {
            controller,
            event_inputs: HashMap::new(),
            number_drafts: HashMap::new(),
        };
        app.sync_event_inputs();
        app
    }

    fn sync_event_inputs(&mut self) {
        let events = self.controller.state().events.clone();
        for event in events {
            let Some(name) = event_name(&event).map(str::to_owned) else {
                continue;
            };
            let args = event_args(&event);
            let values = args
                .iter()
                .map(|arg| arg_value(arg).unwrap_or_else(|| default_value_for_type(arg_type(arg))))
                .collect::<Vec<_>>();
            match self.event_inputs.get(&name) {
                Some(existing) if existing.len() == values.len() => {}
                _ => {
                    self.event_inputs.insert(name, values);
                }
            }
        }
        let valid_names = self
            .controller
            .state()
            .events
            .iter()
            .filter_map(|event| event_name(event).map(str::to_owned))
            .collect::<Vec<_>>();
        self.event_inputs
            .retain(|name, _| valid_names.iter().any(|valid| valid == name));
        let valid_params = self
            .controller
            .state()
            .params
            .iter()
            .filter_map(|param| param_name(param).map(str::to_owned))
            .collect::<Vec<_>>();
        self.number_drafts
            .retain(|name, _| valid_params.iter().any(|valid| valid == name));
    }

    fn reset_event_inputs(&mut self) {
        self.event_inputs.clear();
        self.sync_event_inputs();
    }
}

impl eframe::App for PreviewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let poll = self.controller.poll();
        if poll.state_changed {
            self.sync_event_inputs();
        }
        ctx.request_repaint_after(Duration::from_millis(16));

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            let state = self.controller.state().clone();
            ui.heading("Omni Preview");
            ui.label(egui::RichText::new(state.path).monospace().size(14.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("Status: {}", state.status)).size(13.0));
                let button_size = [120.0, 30.0];
                if ui
                    .add_sized(button_size, egui::Button::new("Start"))
                    .clicked()
                {
                    let _ = self.controller.start();
                }
                if ui
                    .add_sized(button_size, egui::Button::new("Stop"))
                    .clicked()
                {
                    self.controller.stop();
                }
                if ui
                    .add_sized(button_size, egui::Button::new("Reset"))
                    .clicked()
                {
                    self.controller.reset();
                    self.reset_event_inputs();
                }
                if ui
                    .add_sized([138.0, 30.0], egui::Button::new("Refresh Devices"))
                    .clicked()
                {
                    self.controller.refresh_devices();
                }
            });
            if let Some(error) = state.error {
                ui.colored_label(egui::Color32::from_rgb(220, 96, 96), error);
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let state = self.controller.state().clone();

                    if !state.input_devices.is_empty() || !state.output_devices.is_empty() {
                        section_box(ui, "Devices", |ui| {
                            ui.horizontal(|ui| {
                                render_device_combo(
                                    ui,
                                    "Input Device",
                                    &state.input_devices,
                                    state.current_input_device.as_deref(),
                                    |selection| {
                                        let _ = self.controller.set_input_device(selection);
                                    },
                                );
                                render_device_combo(
                                    ui,
                                    "Output Device",
                                    &state.output_devices,
                                    state.current_output_device.as_deref(),
                                    |selection| {
                                        let _ = self.controller.set_output_device(selection);
                                    },
                                );
                            });
                        });
                    }

                    ui.add_space(8.0);
                    section_box(ui, "Scope", |ui| {
                        draw_scope(
                            ui,
                            state.scope_channels,
                            &state.scope_samples,
                            egui::vec2(ui.available_width(), 140.0),
                        );
                    });

                    if !state.buffers.is_empty() {
                        ui.add_space(12.0);
                        section_box(ui, "Buffers", |ui| {
                            for (index, buffer) in state.buffers.iter().enumerate() {
                                let name = buffer_name(buffer).unwrap_or("buffer");
                                let loaded_path = buffer.get("loadedPath").and_then(Value::as_str);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(name).strong());
                                    ui.label(buffer_type_summary(buffer));
                                });
                                ui.label(loaded_path.unwrap_or("No file loaded"));
                                ui.horizontal(|ui| {
                                    if ui.button("Choose File").clicked() {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .add_filter("Wave Audio", &["wav"])
                                            .set_title(format!("Bind '{name}' buffer"))
                                            .pick_file()
                                        {
                                            if let Some(file_path) = path.to_str() {
                                                self.controller.bind_buffer_file(name, file_path);
                                            }
                                        }
                                    }
                                    if ui.button("Clear").clicked() {
                                        self.controller.clear_buffer(name);
                                    }
                                });
                                if index + 1 < state.buffers.len() {
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                }
                            }
                        });
                    }

                    if !state.events.is_empty() {
                        ui.add_space(12.0);
                        section_box(ui, "Events", |ui| {
                            for (event_index, event) in state.events.iter().enumerate() {
                                let Some(name) = event_name(event) else {
                                    continue;
                                };
                                let args = event_args(event);
                                let values =
                                    self.event_inputs.entry(name.to_owned()).or_insert_with(|| {
                                        args.iter()
                                            .map(|arg| {
                                                arg_value(arg).unwrap_or_else(|| {
                                                    default_value_for_type(arg_type(arg))
                                                })
                                            })
                                            .collect()
                                    });

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(name).strong());
                                    if ui.button("Trigger").clicked() {
                                        self.controller.trigger_event(name, values.clone());
                                    }
                                });
                                for (index, arg) in args.iter().enumerate() {
                                    render_scalar_value_editor(
                                        ui,
                                        arg_name(arg).unwrap_or("arg"),
                                        arg_type(arg),
                                        None,
                                        None,
                                        &mut values[index],
                                    );
                                }
                                if event_index + 1 < state.events.len() {
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                }
                            }
                        });
                    }

                    if !state.params.is_empty() {
                        ui.add_space(12.0);
                        section_box(ui, "Params", |ui| {
                            let param_count = state.params.len();
                            for (index, mut param) in state.params.into_iter().enumerate() {
                                let Some(name) = param_name(&param).map(str::to_owned) else {
                                    continue;
                                };
                                let ty = param_type(&param);
                                let min = param.get("rangeMin").and_then(Value::as_f64);
                                let max = param.get("rangeMax").and_then(Value::as_f64);
                                let value = param
                                    .get("value")
                                    .cloned()
                                    .unwrap_or_else(|| default_value_for_type(ty));
                                let number_draft = self.number_drafts.get(&name).copied();
                                match render_param_value_editor(
                                    ui,
                                    &name,
                                    ty,
                                    min,
                                    max,
                                    &value,
                                    number_draft,
                                ) {
                                    ParamEditOutcome::None => {}
                                    ParamEditOutcome::NumberDraft(next_value) => {
                                        self.number_drafts.insert(name.clone(), next_value);
                                    }
                                    ParamEditOutcome::Commit(next_value) => {
                                        self.number_drafts.remove(&name);
                                        set_param_value(&mut param, next_value.clone());
                                        self.controller.set_param(&name, next_value);
                                    }
                                }
                                if index + 1 < param_count {
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                }
                            }
                        });
                    }
                });
        });
    }
}

fn section_box(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(title).strong().size(15.0));
            ui.add_space(8.0);
            add_contents(ui);
        });
}

fn render_device_combo(
    ui: &mut egui::Ui,
    label: &str,
    devices: &[String],
    current: Option<&str>,
    mut on_change: impl FnMut(Option<&str>),
) {
    ui.vertical(|ui| {
        ui.label(label);
        let selected = current.unwrap_or("Default");
        egui::ComboBox::from_id_salt(label)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "Default").clicked() {
                    on_change(None);
                }
                for device in devices {
                    if ui
                        .selectable_label(current == Some(device.as_str()), device)
                        .clicked()
                    {
                        on_change(Some(device));
                    }
                }
            });
    });
}

fn render_scalar_value_editor(
    ui: &mut egui::Ui,
    label: &str,
    ty: &str,
    min: Option<f64>,
    max: Option<f64>,
    value: &mut Value,
) -> bool {
    if ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        let changed = ui.checkbox(&mut checked, label).changed();
        if changed {
            *value = Value::Bool(checked);
        }
        return changed;
    }

    let mut number = value.as_f64().unwrap_or(0.0);
    let step = scalar_step(ty, min, max);
    let changed = ui
        .vertical(|ui| {
            let header_drag_changed = ui
                .horizontal(|ui| {
                    ui.label(egui::RichText::new(label).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let drag = if let (Some(min), Some(max)) = (min, max) {
                            egui::DragValue::new(&mut number)
                                .speed(step / 4.0)
                                .range(min..=max)
                                .max_decimals(slider_decimals(step))
                        } else {
                            egui::DragValue::new(&mut number)
                                .speed(step / 4.0)
                                .max_decimals(slider_decimals(step))
                        };
                        ui.add_sized([120.0, 26.0], drag).changed()
                    })
                    .inner
                })
                .inner;
            ui.add_space(2.0);
            if let (Some(min), Some(max)) = (min, max) {
                let slider_height = 24.0;
                let slider_changed = ui
                    .scope(|ui| {
                        ui.spacing_mut().interact_size.y = slider_height;
                        ui.spacing_mut().slider_width = ui.available_width();
                        let slider = egui::Slider::new(&mut number, min..=max)
                            .step_by(step)
                            .show_value(false)
                            .trailing_fill(true);
                        ui.add_sized([ui.available_width(), slider_height], slider)
                            .changed()
                    })
                    .inner;
                slider_changed || header_drag_changed
            } else {
                header_drag_changed
            }
        })
        .inner;
    if changed {
        *value = json_number(number);
    }
    changed
}

enum ParamEditOutcome {
    None,
    NumberDraft(f64),
    Commit(Value),
}

fn render_param_value_editor(
    ui: &mut egui::Ui,
    label: &str,
    ty: &str,
    min: Option<f64>,
    max: Option<f64>,
    value: &Value,
    number_draft: Option<f64>,
) -> ParamEditOutcome {
    if ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        if ui.checkbox(&mut checked, label).changed() {
            return ParamEditOutcome::Commit(Value::Bool(checked));
        }
        return ParamEditOutcome::None;
    }

    let committed_number = value.as_f64().unwrap_or(0.0);
    let displayed_number = number_draft.unwrap_or(committed_number);
    let step = scalar_step(ty, min, max);
    let mut outcome = ParamEditOutcome::None;

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut number = displayed_number;
                let drag = if let (Some(min), Some(max)) = (min, max) {
                    egui::DragValue::new(&mut number)
                        .speed(step / 16.0)
                        .range(min..=max)
                        .max_decimals(slider_decimals(step).max(6))
                } else {
                    egui::DragValue::new(&mut number)
                        .speed(0.01)
                        .max_decimals(8)
                };
                let response = ui.add_sized([120.0, 26.0], drag);
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let editing_finished = response.drag_stopped() || response.lost_focus() || enter_pressed;
                let number_changed = response.changed();
                let dragging = response.dragged();

                if number_changed {
                    if dragging || editing_finished {
                        outcome = ParamEditOutcome::Commit(json_number(number));
                    } else {
                        outcome = ParamEditOutcome::NumberDraft(number);
                    }
                } else if editing_finished && number_draft.is_some() {
                    outcome = ParamEditOutcome::Commit(json_number(displayed_number));
                }
            });
        });

        if let (Some(min), Some(max)) = (min, max) {
            ui.add_space(2.0);
            let mut slider_value = committed_number;
            let slider_response = ui
                .scope(|ui| {
                    ui.spacing_mut().interact_size.y = 24.0;
                    ui.spacing_mut().slider_width = ui.available_width();
                    let slider = if matches!(ty, "i32" | "i64") {
                        egui::Slider::new(&mut slider_value, min..=max)
                            .integer()
                            .step_by(1.0)
                            .show_value(false)
                            .trailing_fill(true)
                    } else {
                        egui::Slider::new(&mut slider_value, min..=max)
                            .show_value(false)
                            .trailing_fill(true)
                    };
                    ui.add_sized([ui.available_width(), 24.0], slider)
                })
                .inner;

            if slider_response.changed() {
                outcome = ParamEditOutcome::Commit(json_number(slider_value));
            }
        }
    });

    outcome
}

fn scalar_step(ty: &str, min: Option<f64>, max: Option<f64>) -> f64 {
    if matches!(ty, "i32" | "i64") {
        return 1.0;
    }
    if let (Some(min), Some(max)) = (min, max) {
        let span = (max - min).abs();
        if span.is_finite() && span > 0.0 {
            return (span / 2000.0).max(0.0001);
        }
    }
    0.001
}

fn slider_decimals(step: f64) -> usize {
    if step >= 1.0 {
        return 0;
    }
    let decimals = (-step.log10()).ceil() as usize + 1;
    decimals.min(6)
}

fn draw_scope(ui: &mut egui::Ui, channels: usize, samples: &[f32], size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(18, 18, 20));
    if channels == 0 || samples.is_empty() {
        return;
    }

    let colors = [
        egui::Color32::from_rgb(34, 197, 94),
        egui::Color32::from_rgb(59, 130, 246),
        egui::Color32::from_rgb(245, 158, 11),
        egui::Color32::from_rgb(239, 68, 68),
    ];
    let frames = samples.len() / channels;
    if frames == 0 {
        return;
    }

    let ch_height = rect.height() / channels as f32;
    for ch in 0..channels {
        let center_y = rect.top() + ch_height * ch as f32 + ch_height * 0.5;
        painter.line_segment(
            [
                egui::pos2(rect.left(), center_y),
                egui::pos2(rect.right(), center_y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(24)),
        );

        let mut points = Vec::with_capacity(frames);
        for x in 0..frames {
            let sample = samples[x * channels + ch].clamp(-1.0, 1.0);
            let xpos =
                rect.left() + rect.width() * (x as f32 / (frames.saturating_sub(1).max(1) as f32));
            let ypos = center_y - sample * (ch_height * 0.42);
            points.push(egui::pos2(xpos, ypos));
        }
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.5, colors[ch % colors.len()]),
        ));
    }
}

fn event_name(event: &Value) -> Option<&str> {
    event.get("name").and_then(Value::as_str)
}

fn param_name(param: &Value) -> Option<&str> {
    param.get("name").and_then(Value::as_str)
}

fn param_type(param: &Value) -> &str {
    param.get("type").and_then(Value::as_str).unwrap_or("f32")
}

fn buffer_name(buffer: &Value) -> Option<&str> {
    buffer.get("name").and_then(Value::as_str)
}

fn buffer_type_summary(buffer: &Value) -> String {
    let ty = buffer
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("buffer");
    let channels_kind = buffer
        .get("channelsKind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match channels_kind {
        "static" => format!(
            "{ty} · {} ch",
            buffer
                .get("channelsStatic")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        "mono" => format!("{ty} · mono"),
        "dynamic" => format!("{ty} · dynamic"),
        _ => ty.to_owned(),
    }
}

fn event_args(event: &Value) -> Vec<Value> {
    event
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn arg_name(arg: &Value) -> Option<&str> {
    arg.get("name").and_then(Value::as_str)
}

fn arg_type(arg: &Value) -> &str {
    arg.get("type").and_then(Value::as_str).unwrap_or("f32")
}

fn arg_value(arg: &Value) -> Option<Value> {
    arg.get("value").cloned()
}

fn default_value_for_type(ty: &str) -> Value {
    if ty == "bool" {
        Value::Bool(false)
    } else {
        Value::Number(Number::from(0))
    }
}

fn json_number(value: f64) -> Value {
    Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or_else(|| Value::Number(Number::from(0)))
}

fn set_param_value(param: &mut Value, value: Value) {
    if let Some(obj) = param.as_object_mut() {
        obj.insert("value".to_owned(), value);
    }
}
