use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use onda_run::{RunController, RunHostOptions, RunThemeMode};
use serde_json::{Number, Value};

const LOGO_DARK_URI: &str = "bytes://onda-logo-dark-rect.svg";
const LOGO_LIGHT_URI: &str = "bytes://onda-logo-rect.svg";
const LOGO_DARK_BYTES: &[u8] = include_bytes!("../../../assets/svg/onda-logo-dark-rect.svg");
const LOGO_LIGHT_BYTES: &[u8] = include_bytes!("../../../assets/svg/onda-logo-rect.svg");
const APP_ICON_DARK_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo-dark-rect.png");
const APP_ICON_LIGHT_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo-rect.png");

pub fn run_run_egui(onda_path: &Path, options: RunHostOptions) -> Result<(), String> {
    let theme_mode = options.theme;
    let controller = RunController::new(onda_path, options)?;
    let title = format!(
        "Onda - {}",
        controller
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Run".to_owned())
    );

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 820.0])
            .with_icon(load_app_icon(startup_icon_is_dark(theme_mode))?),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        native_options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            match theme_mode {
                RunThemeMode::Auto => {}
                RunThemeMode::Dark => cc.egui_ctx.set_visuals(egui::Visuals::dark()),
                RunThemeMode::Light => cc.egui_ctx.set_visuals(egui::Visuals::light()),
            }
            let initial_icon_dark = resolved_theme_is_dark(&cc.egui_ctx, theme_mode);
            if let Ok(icon) = load_app_icon(initial_icon_dark) {
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(icon))));
            }
            Ok(Box::new(RunApp::new(controller, Some(initial_icon_dark))))
        }),
    )
    .map_err(|err| format!("failed to start egui run: {err}"))
}

fn load_app_icon(is_dark: bool) -> Result<egui::IconData, String> {
    let png_bytes = if is_dark {
        APP_ICON_DARK_PNG
    } else {
        APP_ICON_LIGHT_PNG
    };
    eframe::icon_data::from_png_bytes(png_bytes)
        .map_err(|err| format!("failed to decode run app icon: {err}"))
}

fn startup_icon_is_dark(theme_mode: RunThemeMode) -> bool {
    !matches!(theme_mode, RunThemeMode::Light)
}

fn resolved_theme_is_dark(ctx: &egui::Context, theme_mode: RunThemeMode) -> bool {
    match theme_mode {
        RunThemeMode::Dark => true,
        RunThemeMode::Light => false,
        RunThemeMode::Auto => {
            ctx.system_theme().unwrap_or_else(|| ctx.theme()) == egui::Theme::Dark
        }
    }
}

struct RunApp {
    controller: RunController,
    event_inputs: HashMap<String, Vec<Value>>,
    number_drafts: HashMap<String, f64>,
    current_icon_dark: Option<bool>,
}

impl RunApp {
    fn new(controller: RunController, current_icon_dark: Option<bool>) -> Self {
        let mut app = Self {
            controller,
            event_inputs: HashMap::new(),
            number_drafts: HashMap::new(),
            current_icon_dark,
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

    fn sync_window_icon(&mut self, ctx: &egui::Context, is_dark: bool) {
        if self.current_icon_dark == Some(is_dark) {
            return;
        }
        if let Ok(icon) = load_app_icon(is_dark) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(icon))));
            self.current_icon_dark = Some(is_dark);
        }
    }
}

impl eframe::App for RunApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let poll = self.controller.poll();
        if poll.state_changed {
            self.sync_event_inputs();
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let theme = RunTheme::from_dark_mode(ctx.style().visuals.dark_mode);
        self.sync_window_icon(ctx, theme.is_dark);
        apply_run_theme(ctx, &theme);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme.app_fill)
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let state = self.controller.state().clone();

                        section_box(ui, "", |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), 236.0),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    let logo = if theme.is_dark {
                                        egui::Image::from_bytes(LOGO_DARK_URI, LOGO_DARK_BYTES)
                                    } else {
                                        egui::Image::from_bytes(LOGO_LIGHT_URI, LOGO_LIGHT_BYTES)
                                    };
                                    ui.add(
                                        logo.fit_to_exact_size(egui::vec2(68.0, 68.0))
                                            .maintain_aspect_ratio(true)
                                            .texture_options(egui::TextureOptions::LINEAR),
                                    );
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(&state.path).monospace().size(14.0),
                                    );
                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(format!("Status: {}", state.status))
                                            .size(13.0),
                                    );
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), 30.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let button_width = 120.0;
                                            let button_gap = 10.0;
                                            let total_width = button_width * 4.0 + button_gap * 3.0;
                                            let leading_space =
                                                ((ui.available_width() - total_width) * 0.5)
                                                    .max(0.0);
                                            ui.add_space(leading_space);
                                            ui.spacing_mut().item_spacing.x = button_gap;
                                            let button_size = [button_width, 30.0];
                                            if ui
                                                .add_enabled(
                                                    !state.running
                                                        && state.buffers.iter().all(|buffer| {
                                                            buffer
                                                                .get("loadedPath")
                                                                .and_then(Value::as_str)
                                                                .is_some()
                                                        }),
                                                    egui::Button::new("Play")
                                                        .min_size(egui::vec2(button_width, 30.0)),
                                                )
                                                .clicked()
                                            {
                                                let _ = self.controller.start();
                                            }
                                            if ui
                                                .add_enabled(
                                                    state.running,
                                                    egui::Button::new("Stop")
                                                        .min_size(egui::vec2(button_width, 30.0)),
                                                )
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
                                                .add_sized(
                                                    button_size,
                                                    egui::Button::new("Refresh Devices"),
                                                )
                                                .clicked()
                                            {
                                                self.controller.refresh_devices();
                                            }
                                        },
                                    );
                                    if !state.input_devices.is_empty()
                                        || !state.output_devices.is_empty()
                                    {
                                        ui.add_space(12.0);
                                        let combo_width = 140.0;
                                        let combo_gap = 12.0;
                                        let combo_count = (!state.input_devices.is_empty()
                                            as usize)
                                            + (!state.output_devices.is_empty() as usize);
                                        let total_width = combo_width * combo_count as f32
                                            + combo_gap * combo_count.saturating_sub(1) as f32;
                                        ui.with_layout(
                                            egui::Layout::top_down(egui::Align::Center),
                                            |ui| {
                                                ui.allocate_ui(
                                                    egui::vec2(total_width, 56.0),
                                                    |ui| {
                                                        ui.spacing_mut().item_spacing.x = combo_gap;
                                                        ui.horizontal(|ui| {
                                                            if !state.input_devices.is_empty() {
                                                                ui.allocate_ui(
                                                                    egui::vec2(combo_width, 56.0),
                                                                    |ui| {
                                                                        render_device_combo(
                                                                    ui,
                                                                    "Input Device",
                                                                    &state.input_devices,
                                                                    state
                                                                        .current_input_device
                                                                        .as_deref(),
                                                                    |selection| {
                                                                        let _ = self
                                                                            .controller
                                                                            .set_input_device(
                                                                                selection,
                                                                            );
                                                                    },
                                                                );
                                                                    },
                                                                );
                                                            }
                                                            if !state.output_devices.is_empty() {
                                                                ui.allocate_ui(
                                                                    egui::vec2(combo_width, 56.0),
                                                                    |ui| {
                                                                        render_device_combo(
                                                                    ui,
                                                                    "Output Device",
                                                                    &state.output_devices,
                                                                    state
                                                                        .current_output_device
                                                                        .as_deref(),
                                                                    |selection| {
                                                                        let _ = self
                                                                            .controller
                                                                            .set_output_device(
                                                                                selection,
                                                                            );
                                                                    },
                                                                );
                                                                    },
                                                                );
                                                            }
                                                        });
                                                    },
                                                );
                                            },
                                        );
                                    }
                                    if let Some(error) = state.error {
                                        ui.add_space(8.0);
                                        ui.colored_label(theme.error, error);
                                    }
                                },
                            );
                        });

                        if state.running {
                            ui.add_space(12.0);
                        section_box(ui, "", |ui| {
                            egui::CollapsingHeader::new("Scope")
                                .default_open(true)
                                .show_unindented(ui, |ui| {
                                    ui.add_space(8.0);
                                    draw_scope(
                                        ui,
                                        state.scope_channels,
                                        &state.scope_samples,
                                            egui::vec2(ui.available_width(), 140.0),
                                            &theme,
                                        );
                                    });
                            });
                        }

                        if !state.buffers.is_empty() {
                            ui.add_space(12.0);
                            section_box(ui, "", |ui| {
                                egui::CollapsingHeader::new("Buffers")
                                    .default_open(true)
                                    .show_unindented(ui, |ui| {
                                        ui.add_space(8.0);
                                        for (index, buffer) in state.buffers.iter().enumerate() {
                                            let name = buffer_name(buffer).unwrap_or("buffer");
                                            let loaded_path =
                                                buffer.get("loadedPath").and_then(Value::as_str);
                                            egui::Frame::group(ui.style())
                                                .fill(ui.visuals().panel_fill)
                                                .stroke(
                                                    ui.visuals().widgets.noninteractive.bg_stroke,
                                                )
                                                .corner_radius(12.0)
                                                .inner_margin(egui::Margin::same(12))
                                                .show(ui, |ui| {
                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(ui.available_width(), 36.0),
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.label(
                                                                    egui::RichText::new(name)
                                                                        .strong()
                                                                        .size(15.0),
                                                                );
                                                                ui.label(
                                                                    egui::RichText::new(
                                                                        buffer_type_summary(buffer),
                                                                    )
                                                                    .size(12.0)
                                                                    .weak(),
                                                                );
                                                            });
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    let clear_clicked = ui
                                                                        .add_enabled(
                                                                            loaded_path.is_some(),
                                                                            egui::Button::new(
                                                                                "Clear",
                                                                            )
                                                                            .min_size(egui::vec2(
                                                                                104.0, 36.0,
                                                                            )),
                                                                        )
                                                                        .clicked();
                                                                    let choose_clicked = ui
                                                                        .add(
                                                                            egui::Button::new(
                                                                                "Choose File",
                                                                            )
                                                                            .min_size(egui::vec2(
                                                                                104.0, 36.0,
                                                                            )),
                                                                        )
                                                                        .on_hover_text(
                                                                            "Bind a WAV file to this buffer",
                                                                        )
                                                                        .clicked();

                                                                    if clear_clicked {
                                                                        self.controller
                                                                            .clear_buffer(name);
                                                                    }
                                                                    if choose_clicked {
                                                                        if let Some(path) =
                                                                            rfd::FileDialog::new()
                                                                                .add_filter(
                                                                                    "Wave Audio",
                                                                                    &["wav"],
                                                                                )
                                                                                .set_title(format!(
                                                                                    "Bind '{name}' buffer"
                                                                                ))
                                                                                .pick_file()
                                                                        {
                                                                            if let Some(file_path) =
                                                                                path.to_str()
                                                                            {
                                                                                self.controller
                                                                                    .bind_buffer_file(
                                                                                        name,
                                                                                        file_path,
                                                                                    );
                                                                            }
                                                                        }
                                                                    }
                                                                },
                                                            );
                                                        },
                                                    );

                                                    ui.add_space(8.0);
                                                    ui.label(
                                                        egui::RichText::new("Loaded file")
                                                            .size(12.0)
                                                            .weak(),
                                                    );
                                                    ui.add_space(4.0);
                                                    egui::Frame::group(ui.style())
                                                        .fill(ui.visuals().faint_bg_color)
                                                        .stroke(
                                                            ui.visuals()
                                                                .widgets
                                                                .noninteractive
                                                                .bg_stroke,
                                                        )
                                                        .corner_radius(10.0)
                                                        .inner_margin(egui::Margin::same(10))
                                                        .show(ui, |ui| {
                                                            ui.set_min_width(ui.available_width());
                                                            let text = loaded_path
                                                                .unwrap_or("No file loaded");
                                                            let rich_text =
                                                                if loaded_path.is_some() {
                                                                    egui::RichText::new(text)
                                                                        .monospace()
                                                                } else {
                                                                    egui::RichText::new(text)
                                                                        .weak()
                                                                };
                                                            ui.label(rich_text);
                                                        });

                                                });
                                            if index + 1 < state.buffers.len() {
                                                ui.add_space(6.0);
                                            }
                                        }
                                    });
                            });
                        }

                        if !state.events.is_empty() {
                            ui.add_space(12.0);
                            section_box(ui, "", |ui| {
                                egui::CollapsingHeader::new("Events")
                                    .default_open(true)
                                    .show_unindented(ui, |ui| {
                                        ui.add_space(8.0);
                                        for (event_index, event) in state.events.iter().enumerate()
                                        {
                                            let Some(name) = event_name(event) else {
                                                continue;
                                            };
                                            let args = event_args(event);
                                            let values = self
                                                .event_inputs
                                                .entry(name.to_owned())
                                                .or_insert_with(|| {
                                                    args.iter()
                                                        .map(|arg| {
                                                            arg_value(arg).unwrap_or_else(|| {
                                                                default_value_for_type(arg_type(
                                                                    arg,
                                                                ))
                                                            })
                                                        })
                                                        .collect()
                                                });

                                            egui::Frame::group(ui.style())
                                                .fill(ui.visuals().panel_fill)
                                                .stroke(
                                                    ui.visuals().widgets.noninteractive.bg_stroke,
                                                )
                                                .corner_radius(12.0)
                                                .inner_margin(egui::Margin::same(12))
                                                .show(ui, |ui| {
                                                    ui.allocate_ui_with_layout(
                                                        egui::vec2(ui.available_width(), 36.0),
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                egui::RichText::new(name)
                                                                    .strong()
                                                                    .size(15.0),
                                                            );
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    let trigger_button =
                                                                        egui::Button::new(
                                                                            egui::RichText::new(
                                                                                "Trigger",
                                                                            )
                                                                            .strong()
                                                                            .size(14.0),
                                                                        )
                                                                        .min_size(egui::vec2(
                                                                            104.0, 36.0,
                                                                        ));
                                                                    if ui
                                                                        .add_enabled(
                                                                            state.connected,
                                                                            trigger_button,
                                                                        )
                                                                        .clicked()
                                                                    {
                                                                        self.controller
                                                                            .trigger_event(
                                                                                name,
                                                                                values.clone(),
                                                                            );
                                                                    }
                                                                },
                                                            );
                                                        },
                                                    );

                                                    if !args.is_empty() {
                                                        ui.add_space(8.0);
                                                        for (index, arg) in args.iter().enumerate()
                                                        {
                                                            render_event_arg_editor(
                                                                ui,
                                                                arg_name(arg).unwrap_or("arg"),
                                                                arg_type(arg),
                                                                &mut values[index],
                                                            );
                                                        }
                                                    }
                                                });
                                            if event_index + 1 < state.events.len() {
                                                ui.add_space(6.0);
                                            }
                                        }
                                    });
                            });
                        }

                        if !state.params.is_empty() {
                            ui.add_space(12.0);
                            section_box(ui, "", |ui| {
                                egui::CollapsingHeader::new("Params")
                                    .default_open(true)
                                    .show_unindented(ui, |ui| {
                                        ui.add_space(8.0);
                                        let param_count = state.params.len();
                                        for (index, mut param) in
                                            state.params.into_iter().enumerate()
                                        {
                                            let Some(name) = param_name(&param).map(str::to_owned)
                                            else {
                                                continue;
                                            };
                                            let ty = param_type(&param);
                                            let min = param.get("rangeMin").and_then(Value::as_f64);
                                            let max = param.get("rangeMax").and_then(Value::as_f64);
                                            let value = param
                                                .get("value")
                                                .cloned()
                                                .unwrap_or_else(|| default_value_for_type(ty));
                                            let number_draft =
                                                self.number_drafts.get(&name).copied();
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
                                                    self.number_drafts
                                                        .insert(name.clone(), next_value);
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
                            });
                        }
                    });
            });
    }
}

fn section_box(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().panel_fill)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(8, 10))
        .show(ui, |ui| {
            if !title.is_empty() {
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .size(15.0)
                        .color(ui.visuals().strong_text_color()),
                );
                ui.add_space(8.0);
            }
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
    ui.vertical_centered(|ui| {
        ui.label(label);
        let selected = current.unwrap_or("Default");
        egui::ComboBox::from_id_salt(label)
            .width(ui.available_width())
            .selected_text(ellipsize_middle(selected, 18))
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

fn ellipsize_middle(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars || max_chars <= 3 {
        return text.to_owned();
    }

    let head_len = (max_chars - 3) / 2;
    let tail_len = max_chars - 3 - head_len;
    let head = chars[..head_len].iter().collect::<String>();
    let tail = chars[chars.len() - tail_len..].iter().collect::<String>();
    format!("{head}...{tail}")
}

fn render_event_arg_editor(ui: &mut egui::Ui, label: &str, ty: &str, value: &mut Value) -> bool {
    let row_height = 26.0;
    if ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        let changed = ui
            .horizontal(|ui| {
                ui.set_min_height(row_height);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(label).strong());
                    ui.label(egui::RichText::new(ty).size(12.0).weak());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut checked, "")
                })
                .inner
                .changed()
            })
            .inner;
        if changed {
            *value = Value::Bool(checked);
        }
        return changed;
    }

    let is_integer = is_integer_type(ty);
    let mut number = value.as_f64().unwrap_or(0.0);
    let step = scalar_step(ty, None, None);
    let changed = ui
        .horizontal(|ui| {
            ui.set_min_height(row_height);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).strong());
                ui.label(egui::RichText::new(ty).size(12.0).weak());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_integer {
                    let mut integer = number.round() as i64;
                    let changed = ui
                        .add_sized(
                            [104.0, row_height],
                            egui::DragValue::new(&mut integer).speed(0.25),
                        )
                        .changed();
                    number = integer as f64;
                    changed
                } else {
                    ui.add_sized(
                        [104.0, row_height],
                        egui::DragValue::new(&mut number)
                            .speed(step / 4.0)
                            .max_decimals(slider_decimals(step)),
                    )
                    .changed()
                }
            })
            .inner
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
    let is_integer = is_integer_type(ty);
    let step = scalar_step(ty, min, max);
    let mut outcome = ParamEditOutcome::None;

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(label).strong());
                ui.label(egui::RichText::new(ty).size(12.0).weak());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_integer {
                    let mut integer = displayed_number.round() as i64;
                    let drag = if let (Some(min), Some(max)) = (min, max) {
                        egui::DragValue::new(&mut integer)
                            .speed(0.25)
                            .range((min.ceil() as i64)..=(max.floor() as i64))
                    } else {
                        egui::DragValue::new(&mut integer).speed(0.25)
                    };
                    let response = ui.add_sized([104.0, 26.0], drag);
                    let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let editing_finished =
                        response.drag_stopped() || response.lost_focus() || enter_pressed;
                    let number_changed = response.changed();
                    let dragging = response.dragged();

                    if number_changed {
                        let next = integer as f64;
                        if dragging || editing_finished {
                            outcome = ParamEditOutcome::Commit(json_number(next));
                        } else {
                            outcome = ParamEditOutcome::NumberDraft(next);
                        }
                    } else if editing_finished && number_draft.is_some() {
                        outcome = ParamEditOutcome::Commit(json_number(displayed_number.round()));
                    }
                } else {
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
                    let response = ui.add_sized([104.0, 26.0], drag);
                    let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let editing_finished =
                        response.drag_stopped() || response.lost_focus() || enter_pressed;
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
                    let slider = if is_integer {
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
    if is_integer_type(ty) {
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

fn is_integer_type(ty: &str) -> bool {
    matches!(ty, "i32" | "i64")
}

fn draw_scope(
    ui: &mut egui::Ui,
    channels: usize,
    samples: &[f32],
    size: egui::Vec2,
    theme: &RunTheme,
) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, theme.scope_background);
    if channels == 0 || samples.is_empty() {
        return;
    }

    let stroke_color = theme.scope_strokes[0];
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
            egui::Stroke::new(1.0_f32, theme.scope_grid),
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
            egui::Stroke::new(1.5_f32, stroke_color),
        ));
    }
}

#[derive(Clone, Copy)]
struct RunTheme {
    is_dark: bool,
    app_fill: egui::Color32,
    accent: egui::Color32,
    accent_soft: egui::Color32,
    accent_hover: egui::Color32,
    border: egui::Color32,
    panel_fill: egui::Color32,
    panel_tint: egui::Color32,
    menu_fill: egui::Color32,
    error: egui::Color32,
    scope_background: egui::Color32,
    scope_grid: egui::Color32,
    scope_strokes: [egui::Color32; 4],
}

impl RunTheme {
    fn from_dark_mode(is_dark: bool) -> Self {
        if is_dark {
            Self {
                is_dark: true,
                app_fill: egui::Color32::from_rgb(13, 25, 41),
                accent: egui::Color32::from_rgb(201, 219, 242),
                accent_soft: egui::Color32::from_rgb(35, 61, 92),
                accent_hover: egui::Color32::from_rgb(224, 235, 250),
                border: egui::Color32::from_rgb(53, 78, 109),
                panel_fill: egui::Color32::from_rgb(12, 30, 51),
                panel_tint: egui::Color32::from_rgb(18, 31, 49),
                menu_fill: egui::Color32::from_rgb(13, 24, 39),
                error: egui::Color32::from_rgb(255, 126, 126),
                scope_background: egui::Color32::from_rgb(10, 22, 36),
                scope_grid: egui::Color32::from_rgba_unmultiplied(201, 219, 242, 36),
                scope_strokes: [
                    egui::Color32::from_rgb(201, 219, 242),
                    egui::Color32::from_rgb(123, 173, 230),
                    egui::Color32::from_rgb(122, 215, 186),
                    egui::Color32::from_rgb(243, 181, 123),
                ],
            }
        } else {
            Self {
                is_dark: false,
                app_fill: egui::Color32::from_rgb(219, 232, 247),
                accent: egui::Color32::from_rgb(20, 58, 99),
                accent_soft: egui::Color32::from_rgb(219, 232, 247),
                accent_hover: egui::Color32::from_rgb(10, 33, 59),
                border: egui::Color32::from_rgb(190, 209, 229),
                panel_fill: egui::Color32::from_rgb(245, 249, 255),
                panel_tint: egui::Color32::from_rgb(244, 249, 255),
                menu_fill: egui::Color32::from_rgb(246, 250, 255),
                error: egui::Color32::from_rgb(181, 51, 51),
                scope_background: egui::Color32::from_rgb(236, 244, 253),
                scope_grid: egui::Color32::from_rgba_unmultiplied(20, 58, 99, 28),
                scope_strokes: [
                    egui::Color32::from_rgb(20, 58, 99),
                    egui::Color32::from_rgb(47, 118, 197),
                    egui::Color32::from_rgb(33, 149, 110),
                    egui::Color32::from_rgb(191, 121, 37),
                ],
            }
        }
    }
}

fn apply_run_theme(ctx: &egui::Context, theme: &RunTheme) {
    let mut style = (*ctx.style()).clone();
    let visuals = &mut style.visuals;

    visuals.panel_fill = theme.panel_fill;
    visuals.window_fill = theme.app_fill;
    visuals.faint_bg_color = theme.panel_tint;
    visuals.extreme_bg_color = theme.menu_fill;
    visuals.code_bg_color = theme.menu_fill;
    visuals.selection.bg_fill = theme.accent_soft;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, theme.accent);
    visuals.widgets.noninteractive.bg_fill = theme.panel_fill;
    visuals.widgets.inactive.bg_fill = theme.accent_soft;
    visuals.widgets.inactive.weak_bg_fill = theme.panel_tint;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, theme.border);
    visuals.widgets.hovered.bg_fill = theme.accent;
    visuals.widgets.hovered.weak_bg_fill = theme.accent_soft;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, theme.accent_hover);
    visuals.widgets.active.bg_fill = theme.accent_soft;
    visuals.widgets.active.weak_bg_fill = theme.accent_soft;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, theme.accent);
    visuals.widgets.open.bg_fill = theme.menu_fill;
    visuals.widgets.open.weak_bg_fill = theme.menu_fill;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0_f32, theme.border);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, theme.border);
    visuals.widgets.noninteractive.weak_bg_fill = theme.panel_tint;
    visuals.window_corner_radius = 14.0.into();
    visuals.menu_corner_radius = 12.0.into();
    visuals.widgets.noninteractive.corner_radius = 10.0.into();
    visuals.widgets.inactive.corner_radius = 10.0.into();
    visuals.widgets.hovered.corner_radius = 10.0.into();
    visuals.widgets.active.corner_radius = 10.0.into();
    visuals.widgets.open.corner_radius = 10.0.into();
    style.spacing.slider_width = 220.0;

    ctx.set_style(style);
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
