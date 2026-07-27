use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use onda_run::{
    ParamDomain, ParamScalarType, ParamScale, RunController, RunHostOptions, RunThemeMode,
};
use serde_json::{Number, Value};

const LOGO_DARK_URI: &str = "bytes://onda-logo-dark-rect.svg";
const LOGO_LIGHT_URI: &str = "bytes://onda-logo-rect.svg";
const LOGO_DARK_BYTES: &[u8] = include_bytes!("../../../assets/svg/onda-logo-dark-rect.svg");
const LOGO_LIGHT_BYTES: &[u8] = include_bytes!("../../../assets/svg/onda-logo-rect.svg");
const APP_ICON_DARK_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo-dark.png");
const APP_ICON_LIGHT_PNG: &[u8] = include_bytes!("../../../assets/png/onda-logo.png");
const RUN_APP_ID: &str = "onda-run";
const PARAM_LAYOUT_STORAGE_KEY: &str = "onda.run-view.param-layout.v1";
const FLOAT_CONTROL_TARGET_STEPS: f64 = 2_000.0;
const FLOAT_CONTROL_MIN_STEP: f64 = 0.0001;
const FLOAT_CONTROL_MAX_STEP: f64 = 0.1;
const EVENT_ARRAY_VISIBLE_LIMIT: usize = 16;
const EVENT_ARRAY_CELL_WIDTH: f32 = 147.0;
const EVENT_ARRAY_CELL_GAP: f32 = 7.0;
const RUN_SAMPLE_RATE_CHOICES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];
const RUN_BLOCK_SIZE_CHOICES: [usize; 6] = [64, 128, 256, 512, 1_024, 2_048];

pub fn run_run_egui(onda_path: Option<&Path>, options: RunHostOptions) -> Result<(), String> {
    let theme_mode = options.theme;
    let startup_icon_dark = startup_icon_is_dark(theme_mode);

    let controller = onda_path
        .map(|path| RunController::new(path, options.clone()))
        .transpose()?;
    let title = run_window_title(onda_path);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(RUN_APP_ID)
            .with_inner_size([680.0, 820.0])
            .with_icon(load_app_icon(startup_icon_dark)?),
        persist_window: false,
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    let native_options = {
        let mut native_options = native_options;
        prefer_x11_for_file_drop(&mut native_options);
        native_options
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
            Ok(Box::new(RunApp::new(
                controller,
                options,
                Some(initial_icon_dark),
                ParamLayout::load(cc.storage),
            )))
        }),
    )
    .map_err(|err| format!("failed to start egui run: {err}"))
}

#[cfg(target_os = "linux")]
fn prefer_x11_for_file_drop(options: &mut eframe::NativeOptions) {
    use winit::platform::x11::EventLoopBuilderExtX11 as _;

    let native_wayland_session = environment_variable_is_set("WAYLAND_DISPLAY")
        || environment_variable_is_set("WAYLAND_SOCKET");
    let xwayland_available = environment_variable_is_set("DISPLAY");
    if native_wayland_session && xwayland_available {
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_x11();
        }));
    }
}

#[cfg(target_os = "linux")]
fn environment_variable_is_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn run_window_title(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|name| format!("Onda - {}", name.to_string_lossy()))
        .unwrap_or_else(|| "Onda".to_owned())
}

fn is_onda_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("onda"))
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
    match theme_mode {
        RunThemeMode::Dark => true,
        RunThemeMode::Light => false,
        RunThemeMode::Auto => true,
    }
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

#[derive(Debug, PartialEq)]
struct EventArgSignature {
    name: Option<String>,
    type_repr: String,
    default: Option<Value>,
}

struct RunApp {
    controller: Option<RunController>,
    options: RunHostOptions,
    load_error: Option<String>,
    event_inputs: HashMap<String, Vec<Value>>,
    event_input_signatures: HashMap<String, Vec<EventArgSignature>>,
    number_drafts: HashMap<String, f64>,
    current_icon_dark: Option<bool>,
    param_layout: ParamLayout,
}

impl RunApp {
    fn new(
        controller: Option<RunController>,
        options: RunHostOptions,
        current_icon_dark: Option<bool>,
        param_layout: ParamLayout,
    ) -> Self {
        let mut app = Self {
            controller,
            options,
            load_error: None,
            event_inputs: HashMap::new(),
            event_input_signatures: HashMap::new(),
            number_drafts: HashMap::new(),
            current_icon_dark,
            param_layout,
        };
        app.sync_event_inputs();
        app
    }

    fn sync_event_inputs(&mut self) {
        let Some(controller) = self.controller.as_ref() else {
            self.event_inputs.clear();
            self.event_input_signatures.clear();
            self.number_drafts.clear();
            return;
        };
        let events = controller.state().events.clone();
        for event in events {
            let Some(name) = event_name(&event).map(str::to_owned) else {
                continue;
            };
            let args = event_args(&event);
            let signature = event_arg_signature(&args);
            let values = args
                .iter()
                .map(|arg| arg_value(arg).unwrap_or_else(|| default_value_for_type(arg_type(arg))))
                .collect::<Vec<_>>();
            let preserves_inputs = self
                .event_input_signatures
                .get(&name)
                .is_some_and(|existing| existing == &signature)
                && self
                    .event_inputs
                    .get(&name)
                    .is_some_and(|existing| existing.len() == values.len());
            if !preserves_inputs {
                self.event_inputs.insert(name.clone(), values);
            }
            self.event_input_signatures.insert(name, signature);
        }
        let valid_names = self
            .controller
            .as_ref()
            .expect("loaded run controller")
            .state()
            .events
            .iter()
            .filter_map(|event| event_name(event).map(str::to_owned))
            .collect::<Vec<_>>();
        self.event_inputs
            .retain(|name, _| valid_names.iter().any(|valid| valid == name));
        self.event_input_signatures
            .retain(|name, _| valid_names.iter().any(|valid| valid == name));
        let valid_params = self
            .controller
            .as_ref()
            .expect("loaded run controller")
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
        self.event_input_signatures.clear();
        self.sync_event_inputs();
    }

    fn load_path(&mut self, ctx: &egui::Context, path: &Path) {
        if !is_onda_path(path) {
            self.load_error = Some("Choose an .onda file".to_owned());
            return;
        }
        match RunController::new(path, self.options.clone()) {
            Ok(controller) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(run_window_title(Some(
                    controller.path(),
                ))));
                self.controller = Some(controller);
                self.load_error = None;
                self.sync_event_inputs();
            }
            Err(error) => self.load_error = Some(error),
        }
    }

    fn unload(&mut self, ctx: &egui::Context) {
        if let Some(controller) = self.controller.take() {
            self.options = controller.options().clone();
        }
        self.load_error = None;
        self.event_inputs.clear();
        self.event_input_signatures.clear();
        self.number_drafts.clear();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(run_window_title(None)));
    }

    fn render_file_picker(&mut self, ui: &mut egui::Ui, theme: &RunTheme, file_hovered: bool) {
        let available = ui.available_size();
        let panel_size = egui::vec2(available.x.min(520.0), available.y.min(440.0));
        let panel_rect = egui::Rect::from_center_size(ui.max_rect().center(), panel_size);
        let response = ui.interact(
            panel_rect,
            ui.make_persistent_id("onda-file-picker"),
            egui::Sense::click(),
        );
        ui.painter().rect(
            panel_rect,
            14.0,
            ui.visuals().panel_fill,
            if file_hovered {
                egui::Stroke::new(2.0_f32, theme.accent)
            } else {
                egui::Stroke::new(1.0_f32, ui.visuals().widgets.active.bg_stroke.color)
            },
            egui::StrokeKind::Inside,
        );

        let content_height = if self.load_error.is_some() {
            248.0
        } else {
            220.0
        };
        let content_rect = egui::Rect::from_center_size(
            panel_rect.center(),
            egui::vec2((panel_rect.width() - 48.0).max(0.0), content_height),
        );
        let mut content_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(content_rect)
                .layout(egui::Layout::top_down(egui::Align::Center)),
        );
        let logo = if theme.is_dark {
            egui::Image::from_bytes(LOGO_DARK_URI, LOGO_DARK_BYTES)
        } else {
            egui::Image::from_bytes(LOGO_LIGHT_URI, LOGO_LIGHT_BYTES)
        };
        content_ui.add(
            logo.fit_to_exact_size(egui::vec2(72.0, 72.0))
                .maintain_aspect_ratio(true)
                .texture_options(egui::TextureOptions::LINEAR),
        );
        content_ui.add_space(14.0);
        content_ui.label(
            egui::RichText::new(if file_hovered {
                "Drop to open"
            } else {
                "Open an Onda file"
            })
            .strong()
            .size(20.0),
        );
        content_ui.add_space(6.0);
        content_ui.label(
            egui::RichText::new(if file_hovered {
                "Release the .onda file anywhere in this window"
            } else {
                "Drag an .onda file here, or click to choose one"
            })
            .weak()
            .size(13.0),
        );
        content_ui.add_space(14.0);
        let settings_response = content_ui
            .allocate_ui(egui::vec2(380.0, 62.0), |ui| {
                ui.spacing_mut().interact_size.y = 30.0;
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.add_sized(
                            [184.0, 16.0],
                            egui::Label::new(egui::RichText::new("Sample rate").weak().size(11.0)),
                        );
                        ui.add_sized(
                            [184.0, 16.0],
                            egui::Label::new(egui::RichText::new("Block size").weak().size(11.0)),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        ui.allocate_ui(egui::vec2(184.0, 30.0), |ui| {
                            egui::ComboBox::from_id_salt("run-sample-rate")
                                .width(ui.available_width())
                                .selected_text(format_sample_rate(
                                    self.options.sample_rate_hz as f64,
                                ))
                                .show_ui(ui, |ui| {
                                    for sample_rate in RUN_SAMPLE_RATE_CHOICES {
                                        ui.selectable_value(
                                            &mut self.options.sample_rate_hz,
                                            sample_rate,
                                            format_sample_rate(sample_rate as f64),
                                        );
                                    }
                                });
                        });
                        ui.allocate_ui(egui::vec2(184.0, 30.0), |ui| {
                            egui::ComboBox::from_id_salt("run-block-size")
                                .width(ui.available_width())
                                .selected_text(format!("{} frames", self.options.block_frames))
                                .show_ui(ui, |ui| {
                                    for block_frames in RUN_BLOCK_SIZE_CHOICES {
                                        ui.selectable_value(
                                            &mut self.options.block_frames,
                                            block_frames,
                                            format!("{block_frames} frames"),
                                        );
                                    }
                                });
                        });
                    });
                });
            })
            .response;
        if let Some(error) = &self.load_error {
            content_ui.add_space(12.0);
            content_ui.colored_label(theme.error, error);
        }

        let pointer_over_settings = ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|position| settings_response.rect.contains(position));
        if response.clicked() && !pointer_over_settings {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Onda source", &["onda"])
                .set_title("Open an Onda file")
                .pick_file()
            {
                self.load_path(ui.ctx(), &path);
            }
        }
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

    fn render_buffers(&mut self, ui: &mut egui::Ui, buffers: &[Value]) {
        for buffer in buffers {
            let name = buffer_name(buffer).unwrap_or("buffer");
            let loaded_path = buffer.get("loadedPath").and_then(Value::as_str);
            let loaded_summary = buffer_loaded_summary(buffer);
            egui::Frame::group(ui.style())
                .fill(ui.visuals().panel_fill)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(9.0)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        ui.label(egui::RichText::new(name).strong().monospace());
                        ui.label(
                            egui::RichText::new(buffer_type_summary(buffer))
                                .size(11.0)
                                .weak()
                                .monospace(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let clear_clicked = loaded_path.is_some()
                                && ui
                                    .add(
                                        egui::Button::new("Clear").min_size(egui::vec2(48.0, 26.0)),
                                    )
                                    .clicked();
                            let bind_label = if loaded_path.is_some() {
                                "Replace"
                            } else {
                                "Bind"
                            };
                            let bind_clicked = ui
                                .add(egui::Button::new(bind_label).min_size(egui::vec2(56.0, 26.0)))
                                .on_hover_text("Bind a WAV file to this buffer")
                                .clicked();

                            if clear_clicked {
                                self.controller
                                    .as_mut()
                                    .expect("loaded run controller")
                                    .clear_buffer(name);
                            }
                            if bind_clicked {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Wave Audio", &["wav"])
                                    .set_title(format!("Bind '{name}' buffer"))
                                    .pick_file()
                                {
                                    if let Some(file_path) = path.to_str() {
                                        self.controller
                                            .as_mut()
                                            .expect("loaded run controller")
                                            .bind_buffer_file(name, file_path);
                                    }
                                }
                            }
                        });
                    });
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let file_name = match loaded_path {
                            Some(path) => buffer_file_name(path),
                            None => "No file bound".to_owned(),
                        };
                        if let Some(summary) = loaded_summary.as_deref() {
                            let summary_width = ui
                                .painter()
                                .layout_no_wrap(
                                    summary.to_owned(),
                                    egui::FontId::monospace(11.0),
                                    ui.visuals().weak_text_color(),
                                )
                                .size()
                                .x;
                            let file_width = (ui.available_width() - summary_width - 6.0).max(24.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(file_width, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(file_name)
                                                .size(11.0)
                                                .weak()
                                                .monospace(),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(summary).size(11.0).weak().monospace(),
                                    );
                                },
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(file_name).size(11.0).weak().monospace(),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            ui.add_space(3.0);
        }
    }

    fn render_events(&mut self, ui: &mut egui::Ui, events: &[Value], connected: bool) {
        for event in events {
            let Some(name) = event_name(event) else {
                continue;
            };
            let args = event_args(event);
            let values = self.event_inputs.entry(name.to_owned()).or_insert_with(|| {
                args.iter()
                    .map(|arg| {
                        arg_value(arg).unwrap_or_else(|| default_value_for_type(arg_type(arg)))
                    })
                    .collect()
            });

            egui::Frame::group(ui.style())
                .fill(ui.visuals().panel_fill)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(9.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(name).strong().size(14.0).monospace())
                                .on_hover_text(name);
                            let summary = match args.len() {
                                0 => "No arguments".to_owned(),
                                1 => "1 argument".to_owned(),
                                count => format!("{count} arguments"),
                            };
                            ui.label(egui::RichText::new(summary).size(11.0).weak());
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    connected,
                                    egui::Button::new(egui::RichText::new("Trigger").strong())
                                        .min_size(egui::vec2(72.0, 30.0)),
                                )
                                .clicked()
                            {
                                self.controller
                                    .as_mut()
                                    .expect("loaded run controller")
                                    .trigger_event(name, values.clone());
                            }
                        });
                    });

                    if !args.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);
                        for (index, arg) in args.iter().enumerate() {
                            render_event_arg_editor(
                                ui,
                                arg_name(arg).unwrap_or("arg"),
                                arg_type(arg),
                                &mut values[index],
                                connected,
                            );
                            if index + 1 < args.len() {
                                ui.add_space(1.0);
                            }
                        }
                    }
                });
            ui.add_space(2.0);
        }
    }

    fn render_params(&mut self, ui: &mut egui::Ui, params: Vec<Value>) {
        let gap = 8.0;
        let columns = param_grid_columns(ui.available_width(), self.param_layout);
        let card_width =
            (ui.available_width() - gap * columns.saturating_sub(1) as f32) / columns as f32;
        let card_height = match self.param_layout {
            ParamLayout::Sliders => 80.0,
            ParamLayout::Knobs => 140.0,
        };
        let mut params = params.into_iter();
        let mut row_index = 0;
        loop {
            let row = params.by_ref().take(columns).collect::<Vec<_>>();
            if row.is_empty() {
                break;
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (column_index, param) in row.into_iter().enumerate() {
                    let (card_rect, _) = ui.allocate_exact_size(
                        egui::vec2(card_width, card_height),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect(
                        card_rect,
                        10.0,
                        ui.visuals().faint_bg_color,
                        ui.visuals().widgets.noninteractive.bg_stroke,
                        egui::StrokeKind::Inside,
                    );

                    let mut card_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("param-card", row_index, column_index))
                            .max_rect(card_rect.shrink2(egui::vec2(9.0, 8.0)))
                            .layout(egui::Layout::top_down(egui::Align::Center)),
                    );
                    self.render_param(&mut card_ui, param, self.param_layout == ParamLayout::Knobs);
                }
            });
            if params.len() > 0 {
                ui.add_space(gap);
            }
            row_index += 1;
        }
    }

    fn render_param(&mut self, ui: &mut egui::Ui, mut param: Value, compact: bool) {
        let Some(name) = param_name(&param).map(str::to_owned) else {
            return;
        };
        let ty = param_type(&param);
        let min = param.get("rangeMin").and_then(Value::as_f64);
        let max = param.get("rangeMax").and_then(Value::as_f64);
        let default = param.get("default").and_then(Value::as_f64);
        let step = param.get("step").and_then(Value::as_f64);
        let step_count = param
            .get("stepCount")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok());
        let scale = match param.get("scale").and_then(Value::as_str) {
            Some("log") => ParamScale::Log,
            _ => ParamScale::Linear,
        };
        let scalar = match param.get("scalar").and_then(Value::as_str) {
            Some("f32") => Some(ParamScalarType::F32),
            Some("f64") => Some(ParamScalarType::F64),
            Some("i32") => Some(ParamScalarType::I32),
            Some("i64") => Some(ParamScalarType::I64),
            _ => None,
        };
        let curve = param.get("curve").and_then(Value::as_f64);
        let unit = param.get("unit").and_then(Value::as_str);
        let domain = min
            .zip(max)
            .zip(scalar)
            .and_then(|((minimum, maximum), scalar)| {
                ParamDomain::new(
                    scalar, minimum, maximum, scale, curve, unit, step, step_count,
                )
            });
        let display_name = unit
            .filter(|unit| !unit.is_empty())
            .map(|unit| format!("{name} ({unit})"))
            .unwrap_or_else(|| name.clone());
        let value = param
            .get("value")
            .cloned()
            .unwrap_or_else(|| default_value_for_type(ty));
        let number_draft = self.number_drafts.get(&name).copied();
        let spec = ParamControlSpec {
            label: &display_name,
            ty,
            default,
            domain,
        };
        let outcome = if compact {
            render_compact_param_value_editor(ui, spec, &value, number_draft)
        } else {
            render_param_value_editor(ui, spec, &value, number_draft)
        };
        match outcome {
            ParamEditOutcome::None => {}
            ParamEditOutcome::NumberDraft(next_value) => {
                self.number_drafts.insert(name, next_value);
            }
            ParamEditOutcome::Commit(next_value) => {
                let next_value = next_value
                    .as_f64()
                    .map(|value| json_number(spec.constrain_plain(value)))
                    .unwrap_or(next_value);
                self.number_drafts.remove(&name);
                set_param_value(&mut param, next_value.clone());
                self.controller
                    .as_mut()
                    .expect("loaded run controller")
                    .set_param(&name, next_value);
            }
        }
    }
}

impl eframe::App for RunApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let file_hovered =
            self.controller.is_none() && ctx.input(|input| !input.raw.hovered_files.is_empty());
        if self.controller.is_none() {
            let (dropped_path, dropped_file) = ctx.input(|input| {
                let mut paths = input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.as_deref());
                (
                    paths
                        .clone()
                        .find(|path| is_onda_path(path))
                        .map(Path::to_owned),
                    paths.next().is_some(),
                )
            });
            if let Some(path) = dropped_path {
                self.load_path(ctx, &path);
            } else if dropped_file {
                self.load_error = Some("Choose an .onda file".to_owned());
            }
        }
        if let Some(controller) = self.controller.as_mut() {
            let poll = controller.poll();
            if poll.state_changed {
                self.sync_event_inputs();
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        let theme = RunTheme::from_dark_mode(ctx.style().visuals.dark_mode);
        self.sync_window_icon(ctx, theme.is_dark);
        apply_run_theme(ctx, &theme);
        if self.controller.is_none() {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::default()
                        .fill(theme.app_fill)
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ctx, |ui| self.render_file_picker(ui, &theme, file_hovered));
            return;
        }
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
                        let state = self
                            .controller
                            .as_ref()
                            .expect("loaded run controller")
                            .state()
                            .clone();

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
                                        egui::RichText::new(format_run_status(
                                            &state.status,
                                            self.options.sample_rate_hz,
                                            self.options.block_frames,
                                        ))
                                        .size(13.0),
                                    );
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), 30.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let button_width = 104.0;
                                            let button_gap = 8.0;
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
                                                let _ = self
                                                    .controller
                                                    .as_mut()
                                                    .expect("loaded run controller")
                                                    .start();
                                            }
                                            if ui
                                                .add_enabled(
                                                    state.running,
                                                    egui::Button::new("Stop")
                                                        .min_size(egui::vec2(button_width, 30.0)),
                                                )
                                                .clicked()
                                            {
                                                self.controller
                                                    .as_mut()
                                                    .expect("loaded run controller")
                                                    .stop();
                                            }
                                            if ui
                                                .add_sized(button_size, egui::Button::new("Reset"))
                                                .clicked()
                                            {
                                                self.controller
                                                    .as_mut()
                                                    .expect("loaded run controller")
                                                    .reset();
                                                self.reset_event_inputs();
                                            }
                                            if ui
                                                .add_sized(button_size, egui::Button::new("Unload"))
                                                .clicked()
                                            {
                                                self.unload(ctx);
                                            }
                                        },
                                    );
                                    if self.controller.is_none() {
                                        return;
                                    }
                                    ui.add_space(12.0);
                                    let combo_width = 140.0;
                                    let refresh_width = 128.0;
                                    let control_gap = 12.0;
                                    let combo_count =
                                        (!state.input_devices.is_empty() as usize)
                                            + (!state.output_devices.is_empty() as usize);
                                    let control_count = combo_count + 1;
                                    let total_width = combo_width * combo_count as f32
                                        + refresh_width
                                        + control_gap
                                            * control_count.saturating_sub(1) as f32;
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Center),
                                        |ui| {
                                            ui.allocate_ui(
                                                egui::vec2(total_width, 56.0),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.x = control_gap;
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
                                                                            .as_mut()
                                                                            .expect(
                                                                                "loaded run controller",
                                                                            )
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
                                                                            .as_mut()
                                                                            .expect(
                                                                                "loaded run controller",
                                                                            )
                                                                            .set_output_device(
                                                                                selection,
                                                                            );
                                                                    },
                                                                    );
                                                                },
                                                            );
                                                        }
                                                        ui.allocate_ui(
                                                            egui::vec2(refresh_width, 56.0),
                                                            |ui| {
                                                                let selector_offset = ui
                                                                    .text_style_height(
                                                                        &egui::TextStyle::Body,
                                                                    )
                                                                    + ui
                                                                        .spacing()
                                                                        .item_spacing
                                                                        .y;
                                                                let control_height =
                                                                    ui.spacing().interact_size.y;
                                                                ui.with_layout(
                                                                    egui::Layout::top_down(
                                                                        egui::Align::Center,
                                                                    ),
                                                                    |ui| {
                                                                        ui.add_space(
                                                                            selector_offset,
                                                                        );
                                                                        if ui
                                                                            .add_sized(
                                                                                [
                                                                                    refresh_width,
                                                                                    control_height,
                                                                                ],
                                                                                egui::Button::new(
                                                                                    "Refresh Devices",
                                                                                ),
                                                                            )
                                                                            .clicked()
                                                                        {
                                                                            self.controller
                                                                                .as_mut()
                                                                                .expect(
                                                                                    "loaded run controller",
                                                                                )
                                                                                .refresh_devices();
                                                                        }
                                                                    },
                                                                );
                                                            },
                                                        );
                                                    });
                                                },
                                            );
                                        },
                                    );
                                    if let Some(error) = state.error {
                                        ui.add_space(8.0);
                                        ui.colored_label(theme.error, error);
                                    }
                                },
                            );
                        });

                        if self.controller.is_none() {
                            return;
                        }

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
                                        self.render_buffers(ui, &state.buffers);
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
                                        self.render_events(ui, &state.events, state.connected);
                                    });
                            });
                        }

                        if !state.params.is_empty() {
                            ui.add_space(12.0);
                            section_box(ui, "", |ui| {
                                let mut param_layout = self.param_layout;
                                let mut params_state =
                                    egui::collapsing_header::CollapsingState::load_with_default_open(
                                        ui.ctx(),
                                        ui.make_persistent_id("params-section"),
                                        true,
                                    );
                                ui.horizontal(|ui| {
                                    params_state.show_toggle_button(
                                        ui,
                                        egui::collapsing_header::paint_default_icon,
                                    );
                                    ui.label(egui::RichText::new("Params").strong());
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.selectable_value(
                                                &mut param_layout,
                                                ParamLayout::Knobs,
                                                "Knobs",
                                            );
                                            ui.selectable_value(
                                                &mut param_layout,
                                                ParamLayout::Sliders,
                                                "Sliders",
                                            );
                                        },
                                    );
                                });
                                self.param_layout = param_layout;
                                params_state.show_body_unindented(ui, |ui| {
                                    ui.add_space(8.0);
                                    self.render_params(ui, state.params);
                                });
                            });
                        }
                    });
            });
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(
            PARAM_LAYOUT_STORAGE_KEY,
            self.param_layout.as_str().to_owned(),
        );
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ParamLayout {
    #[default]
    Sliders,
    Knobs,
}

impl ParamLayout {
    fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        match storage.and_then(|storage| storage.get_string(PARAM_LAYOUT_STORAGE_KEY)) {
            Some(value) if value == Self::Knobs.as_str() => Self::Knobs,
            _ => Self::Sliders,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Sliders => "sliders",
            Self::Knobs => "knobs",
        }
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

fn render_event_arg_editor(
    ui: &mut egui::Ui,
    label: &str,
    ty: &str,
    value: &mut Value,
    connected: bool,
) {
    if is_array_type(ty) {
        render_event_array_editor(ui, label, ty, value, connected);
        return;
    }

    ui.horizontal(|ui| {
        ui.set_min_height(26.0);
        ui.allocate_ui_with_layout(
            egui::vec2(150.0, 26.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(egui::RichText::new(label).strong().size(12.0).monospace());
                ui.label(egui::RichText::new(ty).size(11.0).weak().monospace());
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            render_event_scalar_editor(ui, ty, value, connected, 112.0)
        });
    });
}

fn render_event_array_editor(
    ui: &mut egui::Ui,
    label: &str,
    ty: &str,
    value: &mut Value,
    connected: bool,
) {
    let scalar_ty = event_array_scalar_type(ty);
    let fixed_len = event_array_len(ty).flatten();
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    let values = value.as_array_mut().expect("event array initialized above");
    if let Some(length) = fixed_len {
        values.resize_with(length, || default_value_for_type(scalar_ty));
        values.truncate(length);
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.label(egui::RichText::new(label).strong().size(12.0).monospace());
        ui.label(egui::RichText::new(ty).size(11.0).weak().monospace());
        let count_label = match values.len() {
            1 => "1 element".to_owned(),
            count => format!("{count} elements"),
        };
        ui.label(egui::RichText::new(count_label).size(11.0).weak());

        if fixed_len.is_none() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        connected && values.len() < EVENT_ARRAY_VISIBLE_LIMIT,
                        egui::Button::new("+").small(),
                    )
                    .on_hover_text("Add element")
                    .clicked()
                {
                    values.push(default_value_for_type(scalar_ty));
                }
                if ui
                    .add_enabled(
                        connected && !values.is_empty(),
                        egui::Button::new("−").small(),
                    )
                    .on_hover_text("Remove last element")
                    .clicked()
                {
                    values.pop();
                }
            });
        }
    });

    ui.add_space(5.0);
    egui::Frame::NONE
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(7.0)
        .inner_margin(egui::Margin::symmetric(9, 8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if values.is_empty() {
                ui.label(
                    egui::RichText::new("No elements. Add one to build the event payload.")
                        .size(11.0)
                        .weak(),
                );
                return;
            }

            let visible_len = values.len().min(EVENT_ARRAY_VISIBLE_LIMIT);
            let columns = event_array_grid_columns(ui.available_width());
            for (row_index, row) in values[..visible_len].chunks_mut(columns).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = EVENT_ARRAY_CELL_GAP;
                    for (column_index, element) in row.iter_mut().enumerate() {
                        let index = row_index * columns + column_index;
                        egui::Frame::NONE
                            .fill(ui.visuals().extreme_bg_color)
                            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                            .corner_radius(5.0)
                            .inner_margin(egui::Margin::symmetric(7, 5))
                            .show(ui, |ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(133.0, 24.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 5.0;
                                        ui.add_sized(
                                            [24.0, 24.0],
                                            egui::Label::new(
                                                egui::RichText::new(format!("[{index}]"))
                                                    .monospace()
                                                    .size(10.0)
                                                    .weak(),
                                            ),
                                        );
                                        render_event_scalar_editor(
                                            ui, scalar_ty, element, connected, 104.0,
                                        );
                                    },
                                );
                            });
                    }
                });
                if (row_index + 1) * columns < visible_len {
                    ui.add_space(EVENT_ARRAY_CELL_GAP);
                }
            }

            if values.len() > EVENT_ARRAY_VISIBLE_LIMIT {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{} additional elements are retained but not shown",
                        values.len() - EVENT_ARRAY_VISIBLE_LIMIT
                    ))
                    .size(11.0)
                    .weak(),
                );
            }
        });
}

fn render_event_scalar_editor(
    ui: &mut egui::Ui,
    ty: &str,
    value: &mut Value,
    connected: bool,
    width: f32,
) {
    if ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        let label = if checked { "On" } else { "Off" };
        let changed = ui
            .add_enabled_ui(connected, |ui| {
                ui.add_sized([width, 24.0], egui::Checkbox::new(&mut checked, label))
                    .changed()
            })
            .inner;
        if changed {
            *value = Value::Bool(checked);
        }
        return;
    }

    let mut number = value.as_f64().unwrap_or(0.0);
    let changed = if is_integer_type(ty) {
        let mut integer = number.round() as i64;
        let changed = ui
            .add_enabled_ui(connected, |ui| {
                ui.add_sized(
                    [width, 24.0],
                    egui::DragValue::new(&mut integer)
                        .speed(0.25)
                        .range(i64::MIN..=i64::MAX),
                )
                .changed()
            })
            .inner;
        number = integer as f64;
        changed
    } else {
        let step = scalar_step(ty, None, None);
        ui.add_enabled_ui(connected, |ui| {
            ui.add_sized(
                [width, 24.0],
                egui::DragValue::new(&mut number)
                    .speed(step / 4.0)
                    .max_decimals(slider_decimals(step)),
            )
            .changed()
        })
        .inner
    };

    if changed {
        *value = json_number(number);
    }
}

enum ParamEditOutcome {
    None,
    NumberDraft(f64),
    Commit(Value),
}

#[derive(Clone, Copy)]
struct ParamControlSpec<'a> {
    label: &'a str,
    ty: &'a str,
    default: Option<f64>,
    domain: Option<ParamDomain<'a>>,
}

impl ParamControlSpec<'_> {
    fn bounds(self) -> Option<(f64, f64)> {
        self.domain
            .map(|domain| (domain.minimum(), domain.maximum()))
    }

    fn effective_step(self) -> f64 {
        let (min, max) = self.bounds().unzip();
        self.domain
            .and_then(ParamDomain::step)
            .unwrap_or_else(|| scalar_step(self.ty, min, max))
    }

    fn constrain_plain(self, value: f64) -> f64 {
        self.domain
            .map_or(value, |domain| domain.constrain_plain(value))
    }
}

fn render_param_value_editor(
    ui: &mut egui::Ui,
    spec: ParamControlSpec<'_>,
    value: &Value,
    number_draft: Option<f64>,
) -> ParamEditOutcome {
    let ParamControlSpec { label, ty, .. } = spec;
    let (min, max) = spec.bounds().unzip();
    if ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        if ui
            .add(egui::Checkbox::new(
                &mut checked,
                egui::RichText::new(label).monospace(),
            ))
            .changed()
        {
            return ParamEditOutcome::Commit(Value::Bool(checked));
        }
        return ParamEditOutcome::None;
    }

    let committed_number = value.as_f64().unwrap_or(0.0);
    let displayed_number = number_draft.unwrap_or(committed_number);
    let is_integer = is_integer_type(ty);
    let step = spec.effective_step();
    let mut outcome = ParamEditOutcome::None;

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let heading_width = (ui.available_width() - 112.0).max(36.0);
            render_param_heading(ui, label, ty, heading_width, egui::Align::Min);
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
                            .speed(scalar_drag_speed(ty, Some(min), Some(max)))
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
                    let domain = spec
                        .domain
                        .expect("ranged parameter controls must have a prepared domain");
                    let slider = if domain.curve().is_some() || domain.scale() == ParamScale::Log {
                        let mut normalized = domain.plain_to_normalized(slider_value);
                        let response = ui.add_sized(
                            [ui.available_width(), 24.0],
                            egui::Slider::new(&mut normalized, 0.0..=1.0)
                                .show_value(false)
                                .trailing_fill(true),
                        );
                        slider_value = domain.normalized_to_plain(normalized);
                        return response;
                    } else if is_integer {
                        egui::Slider::new(&mut slider_value, min..=max)
                            .integer()
                            .step_by(step)
                            .show_value(false)
                            .trailing_fill(true)
                    } else {
                        egui::Slider::new(&mut slider_value, min..=max)
                            .step_by(step)
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

fn render_compact_param_value_editor(
    ui: &mut egui::Ui,
    spec: ParamControlSpec<'_>,
    value: &Value,
    number_draft: Option<f64>,
) -> ParamEditOutcome {
    let mut outcome = ParamEditOutcome::None;
    render_param_heading(
        ui,
        spec.label,
        spec.ty,
        ui.available_width(),
        egui::Align::Center,
    );
    ui.add_space(4.0);

    if spec.ty == "bool" {
        let mut checked = value.as_bool().unwrap_or(false);
        if ui.checkbox(&mut checked, "").changed() {
            outcome = ParamEditOutcome::Commit(Value::Bool(checked));
        }
        return outcome;
    }

    let committed_number = value.as_f64().unwrap_or(0.0);
    let mut displayed_number = number_draft.unwrap_or(committed_number);

    if let Some((min, max)) = spec.bounds() {
        let mut knob_value = committed_number;
        if render_param_knob(ui, &mut knob_value, min, max, spec).changed() {
            displayed_number = knob_value;
            outcome = ParamEditOutcome::Commit(json_number(knob_value));
        }
        ui.add_space(4.0);
    } else {
        ui.add_space(68.0);
    }

    let next = render_compact_number_input(
        ui,
        spec.ty,
        spec.bounds().map(|(min, _)| min),
        spec.bounds().map(|(_, max)| max),
        spec.effective_step(),
        displayed_number,
        number_draft.is_some(),
    );
    if !matches!(next, ParamEditOutcome::None) {
        outcome = next;
    }
    outcome
}

fn render_param_heading(ui: &mut egui::Ui, label: &str, ty: &str, width: f32, align: egui::Align) {
    let name_font = egui::FontId::monospace(egui::TextStyle::Body.resolve(ui.style()).size);
    let type_font = egui::FontId::monospace(12.0);
    let name_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), name_font, ui.visuals().text_color())
        .size()
        .x;
    let type_width = ui
        .painter()
        .layout_no_wrap(ty.to_owned(), type_font, ui.visuals().weak_text_color())
        .size()
        .x;
    let spacing = 5.0;
    let visible_name_width = name_width.min((width - type_width - spacing).max(0.0));

    ui.allocate_ui_with_layout(
        egui::vec2(width, 20.0),
        egui::Layout::left_to_right(egui::Align::Center)
            .with_main_wrap(false)
            .with_main_align(align),
        |ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            ui.add_sized(
                [visible_name_width, 20.0],
                egui::Label::new(egui::RichText::new(label).strong().monospace()).truncate(),
            );
            ui.label(egui::RichText::new(ty).size(12.0).weak().monospace());
        },
    );
}

fn render_compact_number_input(
    ui: &mut egui::Ui,
    ty: &str,
    min: Option<f64>,
    max: Option<f64>,
    step: f64,
    displayed_number: f64,
    has_draft: bool,
) -> ParamEditOutcome {
    let mut outcome = ParamEditOutcome::None;
    let is_integer = is_integer_type(ty);
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
        let editing_finished = response.drag_stopped() || response.lost_focus() || enter_pressed;
        if response.changed() {
            let next = integer as f64;
            outcome = if response.dragged() || editing_finished {
                ParamEditOutcome::Commit(json_number(next))
            } else {
                ParamEditOutcome::NumberDraft(next)
            };
        } else if editing_finished && has_draft {
            outcome = ParamEditOutcome::Commit(json_number(displayed_number.round()));
        }
    } else {
        let mut number = displayed_number;
        let drag = if let (Some(min), Some(max)) = (min, max) {
            egui::DragValue::new(&mut number)
                .speed(scalar_drag_speed(ty, Some(min), Some(max)))
                .range(min..=max)
                .max_decimals(control_decimals(step))
        } else {
            egui::DragValue::new(&mut number)
                .speed(0.01)
                .max_decimals(8)
        };
        let response = ui.add_sized([104.0, 26.0], drag);
        let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let editing_finished = response.drag_stopped() || response.lost_focus() || enter_pressed;
        if response.changed() {
            outcome = if response.dragged() || editing_finished {
                ParamEditOutcome::Commit(json_number(number))
            } else {
                ParamEditOutcome::NumberDraft(number)
            };
        } else if editing_finished && has_draft {
            outcome = ParamEditOutcome::Commit(json_number(displayed_number));
        }
    }
    outcome
}

fn render_param_knob(
    ui: &mut egui::Ui,
    value: &mut f64,
    min: f64,
    max: f64,
    spec: ParamControlSpec<'_>,
) -> egui::Response {
    let step = spec.effective_step();
    let domain = spec
        .domain
        .expect("knob controls must have a prepared parameter domain");
    let size = egui::vec2(64.0, 64.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value, spec.label));
    if response.clicked() {
        response.request_focus();
    }

    let mut next_value = *value;
    if response.drag_started() {
        ui.data_mut(|data| {
            data.insert_temp(response.id, KnobDragState::new(domain, next_value));
        });
    }
    if response.dragged() {
        let delta_y = response.drag_delta().y as f64;
        next_value = ui.data_mut(|data| {
            data.get_temp_mut_or_insert_with(response.id, || KnobDragState::new(domain, next_value))
                .drag(domain, delta_y)
        });
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<KnobDragState>(response.id));
    }
    if response.has_focus() {
        ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowDown) {
                next_value -= step;
            }
            if input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::ArrowUp) {
                next_value += step;
            }
            if input.key_pressed(egui::Key::Home) {
                next_value = min;
            }
            if input.key_pressed(egui::Key::End) {
                next_value = max;
            }
        });
    }
    if response.double_clicked() {
        next_value = spec.default.unwrap_or(min);
    }

    next_value = spec.constrain_plain(next_value);
    if next_value != *value {
        *value = next_value;
        response.mark_changed();
    }

    let visuals = ui.style().interact(&response);
    let center = rect.center();
    let radius = 27.0;
    ui.painter()
        .circle_filled(center, radius, ui.visuals().extreme_bg_color);
    ui.painter()
        .circle_stroke(center, radius, visuals.bg_stroke);

    let ratio = domain.plain_to_normalized(*value) as f32;
    let start = 135.0_f32.to_radians();
    let sweep = 270.0_f32.to_radians();
    paint_knob_arc(
        ui.painter(),
        center,
        radius - 4.0,
        start,
        start + sweep,
        egui::Stroke::new(3.0_f32, ui.visuals().weak_text_color()),
    );
    paint_knob_arc(
        ui.painter(),
        center,
        radius - 4.0,
        start,
        start + sweep * ratio,
        egui::Stroke::new(3.0_f32, ui.visuals().selection.stroke.color),
    );
    let indicator_angle = start + sweep * ratio;
    let direction = egui::vec2(indicator_angle.cos(), indicator_angle.sin());
    ui.painter().line_segment(
        [
            center + direction * 8.0,
            center + direction * (radius - 8.0),
        ],
        egui::Stroke::new(3.0_f32, visuals.fg_stroke.color),
    );

    response.on_hover_text("Drag vertically to adjust; double-click to reset")
}

const KNOB_DRAG_POINTS_PER_RANGE: f64 = 160.0;

#[derive(Clone, Copy)]
struct KnobDragState {
    start_normalized: f64,
    delta_y: f64,
}

impl KnobDragState {
    fn new(domain: ParamDomain<'_>, plain: f64) -> Self {
        Self {
            start_normalized: domain.plain_to_normalized(plain),
            delta_y: 0.0,
        }
    }

    fn drag(&mut self, domain: ParamDomain<'_>, delta_y: f64) -> f64 {
        let minimum_delta = (self.start_normalized - 1.0) * KNOB_DRAG_POINTS_PER_RANGE;
        let maximum_delta = self.start_normalized * KNOB_DRAG_POINTS_PER_RANGE;
        self.delta_y = (self.delta_y + delta_y).clamp(minimum_delta, maximum_delta);
        domain
            .normalized_to_plain(self.start_normalized - self.delta_y / KNOB_DRAG_POINTS_PER_RANGE)
    }
}

fn paint_knob_arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    start: f32,
    end: f32,
    stroke: egui::Stroke,
) {
    let segments = 40;
    let points = (0..=segments)
        .map(|index| {
            let angle = egui::lerp(start..=end, index as f32 / segments as f32);
            center + egui::vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
}

fn param_grid_columns(available_width: f32, layout: ParamLayout) -> usize {
    let minimum_card_width = match layout {
        ParamLayout::Sliders => 200.0,
        ParamLayout::Knobs => 140.0,
    };
    (((available_width + 8.0) / (minimum_card_width + 8.0)).floor() as usize).max(1)
}

fn scalar_step(ty: &str, min: Option<f64>, max: Option<f64>) -> f64 {
    // Keep this host policy aligned with ui/run/run.html::scalarStepForParam.
    if is_integer_type(ty) {
        return 1.0;
    }
    if let (Some(min), Some(max)) = (min, max) {
        let span = (max - min).abs();
        if span.is_finite() && span > 0.0 {
            let raw_step = (span / FLOAT_CONTROL_TARGET_STEPS).max(FLOAT_CONTROL_MIN_STEP);
            return 10.0_f64
                .powf(raw_step.log10().floor())
                .min(FLOAT_CONTROL_MAX_STEP);
        }
    }
    0.001
}

fn scalar_drag_speed(ty: &str, min: Option<f64>, max: Option<f64>) -> f64 {
    if is_integer_type(ty) {
        return 0.25;
    }
    if let (Some(min), Some(max)) = (min, max) {
        let span = (max - min).abs();
        if span.is_finite() && span > 0.0 {
            return (span / 2000.0).max(0.0001) / 16.0;
        }
    }
    0.01
}

fn slider_decimals(step: f64) -> usize {
    if step >= 1.0 {
        return 0;
    }
    let decimals = (-step.log10()).ceil() as usize + 1;
    decimals.min(6)
}

fn control_decimals(step: f64) -> usize {
    if step >= 1.0 {
        return 0;
    }
    (-step.log10()).ceil().max(0.0) as usize
}

fn is_integer_type(ty: &str) -> bool {
    matches!(ty, "i32" | "i64")
}

fn is_array_type(ty: &str) -> bool {
    ty.ends_with(']')
}

fn event_array_len(ty: &str) -> Option<Option<usize>> {
    let (_, suffix) = ty.rsplit_once('[')?;
    let length = suffix.strip_suffix(']')?;
    if length.is_empty() {
        Some(None)
    } else {
        length.parse().ok().map(Some)
    }
}

fn event_array_scalar_type(ty: &str) -> &str {
    ty.split_once('[').map_or(ty, |(scalar, _)| scalar)
}

fn event_array_grid_columns(available_width: f32) -> usize {
    (((available_width + EVENT_ARRAY_CELL_GAP) / (EVENT_ARRAY_CELL_WIDTH + EVENT_ARRAY_CELL_GAP))
        .floor() as usize)
        .max(1)
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
    buffer
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("buffer")
        .to_owned()
}

fn buffer_file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn buffer_loaded_summary(buffer: &Value) -> Option<String> {
    let frames = buffer.get("loadedFrames").and_then(Value::as_u64)?;
    let channels = buffer.get("loadedChannels").and_then(Value::as_u64)?;
    let sample_rate = buffer.get("loadedSampleRate").and_then(Value::as_f64)?;
    Some(format!(
        "{} frames · {channels} ch · {}",
        format_grouped_count(frames),
        format_sample_rate(sample_rate)
    ))
}

fn format_grouped_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

fn format_sample_rate(sample_rate: f64) -> String {
    if sample_rate >= 1_000.0 {
        let khz = sample_rate / 1_000.0;
        if (khz - khz.round()).abs() < 0.000_1 {
            format!("{khz:.0} kHz")
        } else {
            format!("{khz:.1} kHz")
        }
    } else {
        format!("{sample_rate:.0} Hz")
    }
}

fn format_run_status(status: &str, sample_rate_hz: u32, block_frames: usize) -> String {
    format!(
        "{status} — {} · {block_frames} frames",
        format_sample_rate(sample_rate_hz as f64)
    )
}

fn event_args(event: &Value) -> Vec<Value> {
    event
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn event_arg_signature(args: &[Value]) -> Vec<EventArgSignature> {
    args.iter()
        .map(|arg| EventArgSignature {
            name: arg_name(arg).map(str::to_owned),
            type_repr: arg_type(arg).to_owned(),
            default: arg.get("default").cloned(),
        })
        .collect()
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
    if is_array_type(ty) {
        Value::Array(Vec::new())
    } else if ty == "bool" {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use eframe::{egui, Storage};

    use super::{
        buffer_loaded_summary, control_decimals, event_arg_signature, event_array_grid_columns,
        event_array_len, event_array_scalar_type, format_run_status, param_grid_columns,
        render_compact_param_value_editor, scalar_drag_speed, scalar_step, KnobDragState,
        ParamControlSpec, ParamDomain, ParamLayout, ParamScalarType, ParamScale,
        PARAM_LAYOUT_STORAGE_KEY,
    };
    #[derive(Default)]
    struct TestStorage(HashMap<String, String>);

    impl eframe::Storage for TestStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn flush(&mut self) {}
    }

    #[test]
    fn wide_float_ranges_keep_fractional_control_precision() {
        assert_eq!(scalar_step("f32", Some(40.0), Some(12_000.0)), 0.1);
        assert_eq!(
            scalar_step("f64", Some(-1_000_000.0), Some(1_000_000.0)),
            0.1
        );
        assert_eq!(scalar_step("f32", Some(0.0), Some(0.02)), 0.0001);
        assert_eq!(scalar_step("f64", Some(-1.0), Some(1.0)), 0.001);
        assert_eq!(scalar_step("f32", None, None), 0.001);
        assert_eq!(scalar_step("i32", Some(40.0), Some(12_000.0)), 1.0);
    }

    #[test]
    fn float_drag_speed_remains_scaled_to_the_range() {
        let speed = scalar_drag_speed("f32", Some(40.0), Some(12_000.0));
        assert!((speed - 0.37375).abs() < f64::EPSILON);
    }

    #[test]
    fn knob_values_follow_the_shared_scalar_step() {
        let fine = ParamDomain::new(
            ParamScalarType::F64,
            40.0,
            12_000.0,
            ParamScale::Linear,
            None,
            None,
            Some(0.1),
            Some(119_600),
        )
        .expect("valid fine domain");
        let coarse = ParamDomain::new(
            ParamScalarType::F64,
            40.0,
            12_000.0,
            ParamScale::Linear,
            None,
            None,
            Some(1.0),
            Some(11_960),
        )
        .expect("valid coarse domain");
        assert_eq!(fine.constrain_plain(705.06), 705.1);
        assert_eq!(coarse.constrain_plain(705.6), 706.0);
    }

    #[test]
    fn stepped_knob_drag_accumulates_sub_step_motion() {
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            0.0,
            800.0,
            ParamScale::Linear,
            None,
            None,
            Some(100.0),
            Some(8),
        )
        .expect("valid stepped domain");
        let mut drag = KnobDragState::new(domain, 0.0);

        for _ in 0..9 {
            assert_eq!(drag.drag(domain, -1.0), 0.0);
        }
        assert_eq!(drag.drag(domain, -1.0), 100.0);
    }

    #[test]
    fn logarithmic_controls_map_the_geometric_midpoint_to_the_center() {
        let min = 20.0;
        let max = 20_000.0;
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            min,
            max,
            ParamScale::Log,
            None,
            None,
            None,
            None,
        )
        .expect("valid logarithmic domain");
        let midpoint = domain.normalized_to_plain(0.5);

        assert!((midpoint - (min * max).sqrt()).abs() < 1e-10);
        assert!((domain.plain_to_normalized(midpoint) - 0.5).abs() < 1e-12);
        assert_eq!(domain.normalized_to_plain(0.0), min);
        assert_eq!(domain.normalized_to_plain(1.0), max);
        assert_eq!(domain.plain_to_normalized(min), 0.0);
        assert_eq!(domain.plain_to_normalized(max), 1.0);

        let wide = ParamDomain::new(
            ParamScalarType::F64,
            1.0e-300,
            1.0e300,
            ParamScale::Log,
            None,
            None,
            None,
            None,
        )
        .expect("valid wide logarithmic domain");
        let wide_midpoint = wide.normalized_to_plain(0.5);
        assert!((wide_midpoint - 1.0).abs() < 1.0e-12);
        assert!((wide.plain_to_normalized(1.0) - 0.5).abs() < 1.0e-12);
    }

    #[test]
    fn curved_controls_follow_supercollider_lincurve_shape() {
        let inverse = ParamDomain::new(
            ParamScalarType::F64,
            0.0,
            1.0,
            ParamScale::Linear,
            Some(-4.0),
            None,
            None,
            None,
        )
        .expect("valid inverse curve");
        let inverse_midpoint = inverse.normalized_to_plain(0.5);
        let expected = (-2.0_f64).exp_m1() / (-4.0_f64).exp_m1();
        assert!((inverse_midpoint - expected).abs() < 1.0e-12);
        assert!((inverse.plain_to_normalized(inverse_midpoint) - 0.5).abs() < 1.0e-12);
        let forward = ParamDomain::new(
            ParamScalarType::F64,
            0.0,
            1.0,
            ParamScale::Linear,
            Some(4.0),
            None,
            None,
            None,
        )
        .expect("valid forward curve");
        assert!((forward.normalized_to_plain(0.5) + inverse_midpoint - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn parameter_layout_restores_from_native_storage() {
        let mut storage = TestStorage::default();
        assert_eq!(ParamLayout::load(Some(&storage)), ParamLayout::Sliders);
        storage.set_string(PARAM_LAYOUT_STORAGE_KEY, "knobs".to_owned());
        assert_eq!(ParamLayout::load(Some(&storage)), ParamLayout::Knobs);
    }

    #[test]
    fn parameter_grid_adapts_each_layout_to_available_width() {
        assert_eq!(param_grid_columns(620.0, ParamLayout::Sliders), 3);
        assert_eq!(param_grid_columns(620.0, ParamLayout::Knobs), 4);
        assert_eq!(param_grid_columns(650.0, ParamLayout::Sliders), 3);
        assert_eq!(param_grid_columns(650.0, ParamLayout::Knobs), 4);
        assert_eq!(param_grid_columns(120.0, ParamLayout::Knobs), 1);
    }

    #[test]
    fn compact_float_inputs_show_only_the_step_precision() {
        assert_eq!(control_decimals(1.0), 0);
        assert_eq!(control_decimals(0.1), 1);
        assert_eq!(control_decimals(0.001), 3);
        let spec = ParamControlSpec {
            label: "fine",
            ty: "f64",
            default: Some(0.0),
            domain: ParamDomain::new(
                ParamScalarType::F64,
                0.0,
                0.000_001,
                ParamScale::Linear,
                None,
                None,
                Some(0.000_000_1),
                Some(10),
            ),
        };
        assert_eq!(control_decimals(spec.effective_step()), 7);
    }

    #[test]
    fn event_array_types_expose_shape_and_scalar_type() {
        assert_eq!(event_array_len("f32[2]"), Some(Some(2)));
        assert_eq!(event_array_len("bool[]"), Some(None));
        assert_eq!(event_array_scalar_type("i32[8]"), "i32");
    }

    #[test]
    fn event_input_signatures_track_argument_types_and_defaults() {
        let scalar = serde_json::json!([{
            "name": "samples",
            "type": "f32",
            "default": 1.0,
        }]);
        let array = serde_json::json!([{
            "name": "samples",
            "type": "f32[2]",
            "default": [1.0, 2.0],
        }]);
        let changed_default = serde_json::json!([{
            "name": "samples",
            "type": "f32",
            "default": 2.0,
        }]);

        assert_ne!(
            event_arg_signature(scalar.as_array().expect("scalar args")),
            event_arg_signature(array.as_array().expect("array args"))
        );
        assert_ne!(
            event_arg_signature(scalar.as_array().expect("scalar args")),
            event_arg_signature(changed_default.as_array().expect("changed-default args"))
        );
    }

    #[test]
    fn event_array_grid_only_uses_columns_that_fit() {
        assert_eq!(event_array_grid_columns(640.0), 4);
        assert_eq!(event_array_grid_columns(455.0), 3);
        assert_eq!(event_array_grid_columns(300.0), 1);
        assert_eq!(event_array_grid_columns(120.0), 1);
    }

    #[test]
    fn loaded_buffer_summary_is_compact_and_complete() {
        assert_eq!(
            buffer_loaded_summary(&serde_json::json!({
                "loadedFrames": 96_000,
                "loadedChannels": 2,
                "loadedSampleRate": 44_100,
            }))
            .as_deref(),
            Some("96,000 frames · 2 ch · 44.1 kHz")
        );
    }

    #[test]
    fn run_status_includes_active_compile_settings() {
        assert_eq!(
            format_run_status("Running", 48_000, 256),
            "Running — 48 kHz · 256 frames"
        );
    }

    #[test]
    fn compact_knob_contents_stay_inside_the_card() {
        egui::__run_test_ui(|ui| {
            let card_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(140.0, 124.0));
            let mut card_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(card_rect)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            render_compact_param_value_editor(
                &mut card_ui,
                ParamControlSpec {
                    label: "a_very_long_parameter_name",
                    ty: "f32",
                    default: Some(880.0),
                    domain: ParamDomain::new(
                        ParamScalarType::F64,
                        20.0,
                        20_000.0,
                        ParamScale::Log,
                        None,
                        None,
                        None,
                        None,
                    ),
                },
                &serde_json::json!(880.0),
                None,
            );

            assert!(
                card_ui.min_rect().max.y <= card_rect.max.y,
                "compact contents overflowed by {} px",
                card_ui.min_rect().max.y - card_rect.max.y
            );
        });
    }
}
