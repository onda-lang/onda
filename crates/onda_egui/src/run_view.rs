use std::path::Path;

use eframe::egui;
use onda_run::RunState;

use super::{
    compact_button, draw_scope, error_text, format_run_status, render_param_header,
    render_section_header, run_logo, section_box, section_frame, RunApp, RunTheme,
    COMPACT_FONT_SIZE, PARAM_HEADER_CONTROL_HEIGHT, SMALL_FONT_SIZE,
};

#[cfg(test)]
mod tests;

const TRANSPORT_BUTTON_SIZE: egui::Vec2 = egui::vec2(72.0, 44.0);
const HEADER_CONTROL_SIZE: f32 = 32.0;
const HEADER_LOGO_SIZE: f32 = 32.0;
const HEADER_GAP: f32 = 12.0;
const HEADER_IDENTITY_GAP: f32 = 16.0;
const HEADER_ACTION_GAP: f32 = 8.0;
const HEADER_DIVIDER_WIDTH: f32 = 1.0;
const HEADER_ACTIONS_WIDTH: f32 = 3.0 * HEADER_CONTROL_SIZE + 2.0 * HEADER_ACTION_GAP;

impl RunApp {
    pub(super) fn render_run_panels(
        &mut self,
        ctx: &egui::Context,
        state: RunState,
        theme: &RunTheme,
    ) {
        if self.midi_running && !state.running {
            self.release_virtual_notes();
        }
        self.midi_running = state.running;
        if state.midi.note_on {
            egui::TopBottomPanel::bottom("midi-keyboard")
                .resizable(false)
                .frame(
                    egui::Frame::default()
                        .fill(theme.panel_fill)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .stroke(egui::Stroke::new(1.0_f32, theme.border)),
                )
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    self.render_midi_keyboard(ui, theme);
                });
        } else {
            self.release_virtual_notes();
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(theme.app_fill)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 12,
                        top: 12,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.render_run_view(ui, ctx, state, theme);
                        ui.add_space(12.0);
                    });
            });
    }

    pub(super) fn handle_run_shortcuts(&mut self, ctx: &egui::Context) {
        if self.controller.is_none() {
            return;
        }
        if let Some(playing) = take_transport_shortcut(ctx) {
            self.set_playing(playing);
        }
    }

    fn set_playing(&mut self, playing: bool) {
        if !playing {
            self.release_virtual_notes();
        }
        let Some(controller) = self.controller.as_mut() else {
            return;
        };
        if controller.state().running == playing {
            return;
        }
        if playing {
            let _ = controller.start();
        } else {
            controller.stop();
        }
    }

    pub(super) fn render_run_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        state: RunState,
        theme: &RunTheme,
    ) {
        // Section and body gaps are explicit so widget spacing cannot enlarge them.
        ui.spacing_mut().item_spacing.y = 0.0;
        if self.render_run_header(ui, ctx, &state, theme) {
            return;
        }

        ui.add_space(12.0);
        section_box(ui, |ui| {
            let mut scope_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                ui.make_persistent_id("scope-section"),
                true,
            );
            render_section_header(ui, &mut scope_state, "Scope", |_| {});
            scope_state.show_body_unindented(ui, |ui| {
                ui.add_space(8.0);
                draw_scope(
                    ui,
                    if state.scope_channels == 0 {
                        state.output_channels
                    } else {
                        state.scope_channels
                    },
                    if state.running {
                        &state.scope_samples
                    } else {
                        &[]
                    },
                    egui::vec2(
                        ui.available_width(),
                        if ctx.screen_rect().height() <= 720.0 {
                            96.0
                        } else {
                            160.0
                        },
                    ),
                    theme,
                );
            });
        });

        if state.log_revealed {
            ui.add_space(12.0);
            section_box(ui, |ui| {
                let mut log_state =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("log-section"),
                        true,
                    );
                let mut clear_log = false;
                render_section_header(ui, &mut log_state, "Log", |ui| {
                    clear_log = ui
                        .add(compact_button("Clear").min_size(egui::vec2(48.0, 26.0)))
                        .clicked();
                });
                if clear_log {
                    self.controller
                        .as_mut()
                        .expect("loaded run controller")
                        .clear_log();
                }
                log_state.show_body_unindented(ui, |ui| {
                    ui.add_space(8.0);
                    self.render_log(ui, &state);
                });
            });
        }

        if !state.buffers.is_empty() {
            ui.add_space(12.0);
            section_box(ui, |ui| {
                let mut buffers_state =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("buffers-section"),
                        true,
                    );
                render_section_header(ui, &mut buffers_state, "Buffers", |_| {});
                buffers_state.show_body_unindented(ui, |ui| {
                    ui.add_space(8.0);
                    self.render_buffers(ui, &state.buffers);
                });
            });
        }

        if !state.events.is_empty() {
            ui.add_space(12.0);
            section_box(ui, |ui| {
                let mut events_state =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("events-section"),
                        true,
                    );
                render_section_header(ui, &mut events_state, "Events", |ui| {
                    if reset_icon_button(ui, "Reset event arguments to defaults").clicked() {
                        self.controller
                            .as_mut()
                            .expect("loaded run controller")
                            .reset_event_arguments();
                        self.reset_event_inputs();
                    }
                });
                events_state.show_body_unindented(ui, |ui| {
                    ui.add_space(8.0);
                    self.render_events(ui, &state.events, state.connected);
                });
            });
        }

        if !state.params.is_empty() {
            ui.add_space(12.0);
            section_box(ui, |ui| {
                let mut param_layout = self.param_layout;
                let mut params_state =
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("params-section"),
                        true,
                    );
                let mut seconds = self.options.param_smoothing_seconds;
                let actions = render_param_header(
                    ui,
                    &mut params_state,
                    &mut param_layout,
                    &mut seconds,
                    self.default_param_smoothing_seconds,
                );
                if actions.reset {
                    self.controller
                        .as_mut()
                        .expect("loaded run controller")
                        .reset_params();
                }
                if actions.smoothing_changed {
                    match self
                        .controller
                        .as_mut()
                        .expect("loaded run controller")
                        .set_param_smoothing_seconds(seconds)
                    {
                        Ok(()) => {
                            self.options.param_smoothing_seconds = seconds;
                            self.load_error = None;
                        }
                        Err(error) => self.load_error = Some(error),
                    }
                }
                self.param_layout = param_layout;
                params_state.show_body_unindented(ui, |ui| {
                    ui.add_space(8.0);
                    self.render_params(ui, state.params);
                });
            });
        }
    }
    fn render_run_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        state: &RunState,
        theme: &RunTheme,
    ) -> bool {
        let mut unload = false;
        let transport_id = ui.make_persistent_id("run-transport");
        let actions_id = ui.make_persistent_id("run-actions");
        let frame = section_frame(ui).inner_margin(egui::Margin::symmetric(12, 8));
        frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(HEADER_IDENTITY_GAP, 2.0);
            ui.spacing_mut().interact_size.y = HEADER_CONTROL_SIZE;
            let version = ui.painter().layout_no_wrap(
                env!("CARGO_PKG_VERSION").to_owned(),
                egui::FontId::monospace(SMALL_FONT_SIZE),
                theme.muted_text,
            );
            let brand_width = version.size().x.max(HEADER_LOGO_SIZE);
            let controls_width = TRANSPORT_BUTTON_SIZE.x
                + 2.0 * HEADER_GAP
                + HEADER_DIVIDER_WIDTH
                + HEADER_ACTIONS_WIDTH;
            let brand_panel_width = brand_width + HEADER_IDENTITY_GAP + HEADER_DIVIDER_WIDTH;
            let source_lines = [
                Path::new(&state.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&state.path)
                    .to_owned(),
                format_run_status(
                    &state.status,
                    self.options.sample_rate_hz,
                    self.options.block_samples,
                ),
            ];
            let source_allowance = source_lines
                .iter()
                .zip([
                    egui::FontId::monospace(15.0),
                    egui::FontId::proportional(COMPACT_FONT_SIZE),
                ])
                .map(|(text, font)| {
                    ui.painter()
                        .layout_no_wrap(text.clone(), font, theme.muted_text)
                        .size()
                        .x
                })
                .fold(120.0_f32, f32::max)
                .min(200.0);
            let source_space = ui.available_width()
                - brand_panel_width
                - 2.0 * HEADER_IDENTITY_GAP
                - controls_width;
            let narrow = source_space < source_allowance;
            let stacked =
                ui.available_width() < brand_width + HEADER_IDENTITY_GAP + HEADER_ACTIONS_WIDTH;
            let source_width = source_space.max(0.0);
            let identity_height = HEADER_LOGO_SIZE + 4.0 + version.size().y;
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), identity_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_height(identity_height);
                    let brand = ui.allocate_ui_with_layout(
                        egui::vec2(
                            if narrow {
                                brand_width
                            } else {
                                brand_panel_width
                            },
                            identity_height,
                        ),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_height(identity_height);
                            ui.set_min_width(if narrow {
                                brand_width
                            } else {
                                brand_panel_width
                            });
                            ui.allocate_ui_with_layout(
                                egui::vec2(brand_width, identity_height),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.y = 4.0;
                                    ui.add(run_logo(ctx, theme.is_dark, HEADER_LOGO_SIZE));
                                    ui.label(version);
                                },
                            );
                        },
                    );
                    if !narrow {
                        let rect = brand.response.rect;
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.right(), rect.top() + 4.0),
                                egui::pos2(rect.right(), rect.bottom() - 4.0),
                            ],
                            egui::Stroke::new(HEADER_DIVIDER_WIDTH, theme.border),
                        );
                        render_run_source(ui, &state.path, &source_lines, theme, source_width);
                    }
                    if !narrow {
                        ui.allocate_ui_with_layout(
                            egui::vec2(controls_width, identity_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = HEADER_GAP;
                                self.render_run_transport(ui, transport_id, state, theme);
                                render_header_divider(ui, theme);
                                unload = self.render_run_actions(ui, actions_id, state, theme);
                            },
                        );
                    } else if !stacked {
                        ui.add_space((ui.available_width() - HEADER_ACTIONS_WIDTH).max(0.0));
                        unload = self.render_run_actions(ui, actions_id, state, theme);
                    }
                },
            );
            if narrow {
                ui.add_space(HEADER_GAP);
                let source_width =
                    (ui.available_width() - TRANSPORT_BUTTON_SIZE.x - HEADER_GAP).max(0.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), identity_height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = HEADER_GAP;
                        render_run_source(ui, &state.path, &source_lines, theme, source_width);
                        self.render_run_transport(ui, transport_id, state, theme);
                    },
                );
            }
            if stacked {
                ui.add_space(HEADER_GAP);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), HEADER_CONTROL_SIZE),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space((ui.available_width() - HEADER_ACTIONS_WIDTH).max(0.0));
                        unload = self.render_run_actions(ui, actions_id, state, theme);
                    },
                );
            }
            if unload {
                return;
            }
            if let Some(notice) = &self.project_notice {
                ui.add_space(6.0);
                match notice {
                    Ok(message) => {
                        ui.label(egui::RichText::new(message).size(SMALL_FONT_SIZE));
                    }
                    Err(error) => {
                        ui.colored_label(theme.error, error_text(error));
                    }
                }
            }
            if let Some(error) = self.load_error.as_ref().or(state.error.as_ref()) {
                ui.add_space(6.0);
                ui.colored_label(theme.error, error_text(error));
            }
        });
        if unload {
            self.release_virtual_notes();
            self.unload(ctx);
        }
        unload
    }

    fn render_run_transport(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        state: &RunState,
        theme: &RunTheme,
    ) {
        if transport_icon_button(ui, id, state.running, theme).clicked() {
            self.set_playing(!state.running);
        }
    }

    fn render_run_actions(
        &mut self,
        ui: &mut egui::Ui,
        id: egui::Id,
        state: &RunState,
        theme: &RunTheme,
    ) -> bool {
        let mut unload = false;
        ui.allocate_ui_with_layout(
            egui::vec2(
                ui.available_width().min(HEADER_ACTIONS_WIDTH),
                HEADER_CONTROL_SIZE,
            ),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            |ui| {
                ui.set_min_height(HEADER_CONTROL_SIZE);
                ui.spacing_mut().item_spacing = egui::Vec2::splat(HEADER_ACTION_GAP);
                let inactive = &mut ui.visuals_mut().widgets.inactive;
                inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                inactive.bg_stroke = egui::Stroke::NONE;
                inactive.fg_stroke.color = theme.muted_text;
                let mut devices_open = self.devices_open;
                let settings =
                    device_settings_button(ui, id.with("devices"), &mut devices_open, |ui| {
                        self.render_run_devices(ui, state, theme);
                    });
                self.devices_open = devices_open;
                HeaderIcon::Settings.decorate(ui, settings.response);
                let response = run_header_button(
                    ui,
                    id.with("save"),
                    egui::Vec2::splat(HEADER_CONTROL_SIZE),
                    false,
                );
                if HeaderIcon::Save.decorate(ui, response).clicked() {
                    self.devices_open = false;
                    ui.memory_mut(|memory| memory.close_popup());
                    self.save_as_project();
                }
                let response = run_header_button(
                    ui,
                    id.with("unload"),
                    egui::Vec2::splat(HEADER_CONTROL_SIZE),
                    false,
                );
                unload = HeaderIcon::Unload.decorate(ui, response).clicked();
            },
        );
        unload
    }

    fn render_run_devices(&mut self, ui: &mut egui::Ui, state: &RunState, theme: &RunTheme) {
        let fields = [
            (!state.input_devices.is_empty()).then_some((
                DeviceKind::Input,
                "Input Device",
                Some("Default"),
                &state.input_devices,
                state.current_input_device.as_deref(),
            )),
            (!state.output_devices.is_empty()).then_some((
                DeviceKind::Output,
                "Output Device",
                Some("Default"),
                &state.output_devices,
                state.current_output_device.as_deref(),
            )),
            state.midi.available.then_some((
                DeviceKind::Midi,
                "MIDI Input",
                None,
                &state.midi_input_devices,
                state.current_midi_input_device.as_deref(),
            )),
        ];
        for (kind, label, default, devices, current) in fields.into_iter().flatten() {
            render_device_combo(
                ui,
                label,
                theme.muted_text,
                default,
                devices,
                current,
                |selection| {
                    if matches!(kind, DeviceKind::Midi) {
                        self.release_virtual_notes();
                    }
                    let controller = self.controller.as_mut().expect("loaded run controller");
                    let _ = match kind {
                        DeviceKind::Input => controller.set_input_device(selection),
                        DeviceKind::Output => controller.set_output_device(selection),
                        DeviceKind::Midi => controller.set_midi_input_device(selection),
                    };
                },
            );
            ui.add_space(8.0);
        }
        if ui
            .add_sized(
                [ui.available_width(), 32.0],
                compact_button("Refresh Devices"),
            )
            .clicked()
        {
            self.controller
                .as_mut()
                .expect("loaded run controller")
                .refresh_devices();
        }
    }
}

fn render_run_source(
    ui: &mut egui::Ui,
    path: &str,
    lines: &[String; 2],
    theme: &RunTheme,
    width: f32,
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 40.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            let [title, subtitle] = render_header_lines(
                ui,
                width,
                [
                    egui::RichText::new(&lines[0])
                        .monospace()
                        .strong()
                        .size(15.0),
                    egui::RichText::new(&lines[1])
                        .size(COMPACT_FONT_SIZE)
                        .color(theme.muted_text),
                ],
            );
            title.on_hover_text(path);
            subtitle.on_hover_text(&lines[1]);
        },
    );
}

enum DeviceKind {
    Input,
    Output,
    Midi,
}

fn take_transport_shortcut(ctx: &egui::Context) -> Option<bool> {
    ctx.input_mut(|input| {
        if !input.focused {
            return None;
        }
        let mut playing = None;
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if modifiers.alt || !(modifiers.ctrl || modifiers.command) {
                return true;
            }
            let requested = match key {
                egui::Key::S | egui::Key::Enter => true,
                egui::Key::Period => false,
                _ => return true,
            };
            if !repeat {
                playing = Some(requested);
            }
            false
        });
        playing
    })
}

fn run_header_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    size: egui::Vec2,
    selected: bool,
) -> egui::Response {
    // Keep interaction identity independent of the header's responsive layout.
    let (_, rect) = ui.allocate_space(size);
    let mut response = ui.interact(rect, id, egui::Sense::click());
    response.intrinsic_size = Some(size);
    let visuals = ui.style().interact(&response);
    let (fill, stroke) = if selected {
        (
            ui.visuals().selection.bg_fill,
            ui.visuals().selection.stroke,
        )
    } else {
        (visuals.weak_bg_fill, visuals.bg_stroke)
    };
    ui.painter().rect(
        rect.expand(visuals.expansion),
        super::CONTROL_CORNER_RADIUS,
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    if response.hovered() {
        if let Some(cursor) = ui.visuals().interact_cursor {
            ui.ctx().set_cursor_icon(cursor);
        }
    }
    response
}

fn device_settings_button<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    open: &mut bool,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let response = run_header_button(ui, id, egui::Vec2::splat(HEADER_CONTROL_SIZE), *open);
    if response.clicked() {
        *open = !*open;
        if !*open {
            ui.memory_mut(|memory| memory.close_popup());
        }
    }
    let inner = if *open {
        // ComboBox owns egui's popup slot. Keep Devices outside that slot and
        // defer dismissal while its dropdown handles a click or Escape.
        let dropdown_open = ui.memory(|memory| memory.any_popup_open());
        let frame = egui::Frame::menu(ui.style());
        let bounds = ui.ctx().screen_rect().shrink(12.0);
        let width = (bounds.width() - frame.total_margin().sum().x).clamp(0.0, 240.0);
        let height = (bounds.height() - frame.total_margin().sum().y).max(0.0);
        let popup = egui::Area::new(response.id.with("devices"))
            .kind(egui::UiKind::Popup)
            .order(egui::Order::Foreground)
            .fixed_pos(response.rect.right_bottom() + egui::vec2(0.0, 8.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .constrain_to(bounds)
            .show(ui.ctx(), |ui| {
                frame
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().interact_size.y = 32.0;
                        egui::ScrollArea::vertical()
                            .max_height(height)
                            .auto_shrink([false, true])
                            .show(ui, add_contents)
                            .inner
                    })
                    .inner
            });
        if !dropdown_open {
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                *open = false;
                response.request_focus();
            } else if response.clicked_elsewhere() && popup.response.clicked_elsewhere() {
                *open = false;
            }
        }
        Some(popup.inner)
    } else {
        None
    };
    egui::InnerResponse { inner, response }
}

pub(super) fn reset_icon_button(ui: &mut egui::Ui, label: &'static str) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = PARAM_HEADER_CONTROL_HEIGHT;
        let response =
            ui.add(compact_button("").min_size(egui::Vec2::splat(PARAM_HEADER_CONTROL_HEIGHT)));
        HeaderIcon::Reset(label).decorate(ui, response)
    })
    .inner
}

fn render_header_lines(
    ui: &mut egui::Ui,
    width: f32,
    lines: [egui::RichText; 2],
) -> [egui::Response; 2] {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 39.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let [title, subtitle] = lines;
            [(title, 20.0), (subtitle, 17.0)].map(|(text, height)| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(egui::vec2(width, height));
                        ui.add(egui::Label::new(text).truncate())
                    },
                )
                .inner
            })
        },
    )
    .inner
}

fn render_header_divider(ui: &mut egui::Ui, theme: &RunTheme) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(HEADER_DIVIDER_WIDTH, HEADER_CONTROL_SIZE),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        egui::Stroke::new(HEADER_DIVIDER_WIDTH, theme.border),
    );
}

fn transport_icon_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    running: bool,
    theme: &RunTheme,
) -> egui::Response {
    let (icon, color) = if running {
        (HeaderIcon::Stop, theme.error)
    } else {
        (HeaderIcon::Play, theme.success)
    };
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = TRANSPORT_BUTTON_SIZE.y;
        let widgets = &mut ui.visuals_mut().widgets;
        for (visuals, fill_opacity, border_opacity) in [
            (&mut widgets.inactive, 0.1, 0.6),
            (&mut widgets.hovered, 0.2, 1.0),
            (&mut widgets.active, 0.3, 1.0),
        ] {
            visuals.weak_bg_fill = if running {
                color.gamma_multiply(fill_opacity)
            } else {
                color
            };
            visuals.bg_stroke = egui::Stroke::new(1.0_f32, color.gamma_multiply(border_opacity));
            visuals.fg_stroke.color = if running { color } else { theme.app_fill };
        }
        let response = run_header_button(ui, id, TRANSPORT_BUTTON_SIZE, false);
        icon.decorate(ui, response)
    })
    .inner
}

enum HeaderIcon {
    Play,
    Stop,
    Settings,
    Save,
    Unload,
    Reset(&'static str),
}

impl HeaderIcon {
    fn decorate(self, ui: &egui::Ui, response: egui::Response) -> egui::Response {
        let label = match self {
            Self::Play => "Play",
            Self::Stop => "Stop",
            Self::Settings => "Devices",
            Self::Save => "Save as project",
            Self::Unload => "Unload",
            Self::Reset(label) => label,
        };
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = ui.style().interact(&response).fg_stroke.color;
        let center = if matches!(self, Self::Play | Self::Stop) {
            let text = ui.painter().layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(COMPACT_FONT_SIZE),
                color,
            );
            let (icon_left, icon_right) = if matches!(self, Self::Play) {
                (-4.0, 6.0)
            } else {
                (-6.0, 6.0)
            };
            let icon_width = icon_right - icon_left;
            let gap = 6.0;
            let left = response.rect.center().x - (icon_width + gap + text.size().x) / 2.0;
            let center = egui::pos2(left - icon_left, response.rect.center().y);
            ui.painter().galley(
                center + egui::vec2(icon_right + gap, -text.size().y / 2.0),
                text,
                color,
            );
            center
        } else {
            response.rect.center()
        };
        let point = |x, y| center + egui::vec2(x, y);
        let painter = ui.painter();
        match self {
            Self::Play => {
                painter.add(egui::Shape::convex_polygon(
                    vec![point(-4.0, -6.0), point(6.0, 0.0), point(-4.0, 6.0)],
                    color,
                    egui::Stroke::NONE,
                ));
            }
            Self::Stop => {
                painter.rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(12.0, 12.0)),
                    1,
                    color,
                );
            }
            Self::Settings => {
                let stroke = egui::Stroke::new(1.5_f32, color);
                for (y, knob) in [(-5.0, -3.0), (0.0, 3.0), (5.0, -1.0)] {
                    painter.line_segment([point(-7.0, y), point(knob - 2.0, y)], stroke);
                    painter.line_segment([point(knob + 2.0, y), point(7.0, y)], stroke);
                    painter.circle_stroke(point(knob, y), 2.0, stroke);
                }
            }
            Self::Save => {
                let stroke = egui::Stroke::new(1.5_f32, color);
                painter.add(egui::Shape::closed_line(
                    vec![
                        point(-6.0, -7.0),
                        point(3.0, -7.0),
                        point(6.0, -4.0),
                        point(6.0, 7.0),
                        point(-6.0, 7.0),
                    ],
                    stroke,
                ));
                for (edge, inner) in [(-7.0, -2.0), (7.0, 2.0)] {
                    painter.add(egui::Shape::line(
                        vec![
                            point(-3.0, edge),
                            point(-3.0, inner),
                            point(3.0, inner),
                            point(3.0, edge),
                        ],
                        stroke,
                    ));
                }
            }
            Self::Unload => {
                painter.add(egui::Shape::convex_polygon(
                    vec![point(0.0, -6.0), point(6.0, 2.0), point(-6.0, 2.0)],
                    color,
                    egui::Stroke::NONE,
                ));
                painter.rect_filled(
                    egui::Rect::from_center_size(point(0.0, 5.0), egui::vec2(12.0, 2.0)),
                    1,
                    color,
                );
            }
            Self::Reset(_) => {
                let stroke = egui::Stroke::new(1.5_f32, color);
                let mut points = (0..=32)
                    .map(|step| {
                        let angle =
                            std::f32::consts::PI - step as f32 / 32.0 * 1.75 * std::f32::consts::PI;
                        point(5.5 * angle.cos(), 5.5 * angle.sin())
                    })
                    .collect::<Vec<_>>();
                points.push(point(-5.5, -2.5));
                painter.add(egui::Shape::line(points, stroke));
                painter.add(egui::Shape::line(
                    vec![point(-5.5, -6.0), point(-5.5, -2.5), point(-2.0, -2.5)],
                    stroke,
                ));
            }
        }
        response.on_hover_text(match self {
            Self::Play => "Play (Ctrl+S / Ctrl+Enter)",
            Self::Stop => "Stop (Ctrl+.)",
            _ => label,
        })
    }
}

fn render_device_combo(
    ui: &mut egui::Ui,
    label: &str,
    label_color: egui::Color32,
    default_label: Option<&str>,
    devices: &[String],
    current: Option<&str>,
    mut on_change: impl FnMut(Option<&str>),
) -> egui::Response {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(SMALL_FONT_SIZE)
                .color(label_color),
        );
        let selected = current
            .or_else(|| devices.first().map(String::as_str))
            .or(default_label)
            .unwrap_or("");
        egui::ComboBox::from_id_salt(label)
            .width(ui.available_width())
            .selected_text(egui::RichText::new(selected).size(COMPACT_FONT_SIZE))
            .truncate()
            .show_ui(ui, |ui| {
                if let Some(default_label) = default_label {
                    if ui
                        .selectable_label(current.is_none(), default_label)
                        .clicked()
                    {
                        on_change(None);
                    }
                }
                for device in devices {
                    if ui
                        .selectable_label(current == Some(device.as_str()), device)
                        .clicked()
                    {
                        on_change(Some(device));
                    }
                }
            })
            .response
            .on_hover_text(selected)
    })
    .inner
}
