//! Measurement-only adapter around the unchanged production run panels.
use crate::{apply_run_theme, ParamLayout, RunApp, RunTheme};
use eframe::egui;
use onda_run::{RunHostOptions, RunMidiCapabilities, RunState};
use serde_json::Value;

pub struct Scene {
    context: egui::Context,
    app: RunApp,
    state: RunState,
}
impl Scene {
    pub fn new(params: Vec<Value>) -> Self {
        let context = egui::Context::default();
        egui_extras::install_image_loaders(&context);
        apply_run_theme(&context, &RunTheme::from_dark_mode(true));
        Self {
            context,
            app: RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs),
            state: RunState {
                running: true,
                connected: true,
                path: "electric.onda".into(),
                status: "Playing".into(),
                error: None,
                output_channels: 2,
                buffers: vec![],
                events: vec![],
                midi: RunMidiCapabilities::default(),
                delegates: vec![],
                log_text: String::new(),
                log_entries: vec![],
                log_revealed: false,
                print_overflow_count: 0,
                print_transport_drop_count: 0,
                delegate_overflow_count: 0,
                delegate_transport_drop_count: 0,
                params,
                input_devices: vec![],
                output_devices: vec![],
                midi_input_devices: vec![],
                current_input_device: None,
                current_output_device: None,
                current_midi_input_device: None,
                scope_channels: 2,
                scope_samples: vec![],
            },
        }
    }
    pub fn render(
        &mut self,
        samples: &[f32],
        values: Option<&[Value]>,
        frame: u32,
    ) -> (egui::FullOutput, Vec<egui::ClippedPrimitive>) {
        self.state.scope_samples.clear();
        self.state.scope_samples.extend_from_slice(samples);
        if let Some(values) = values {
            for (param, value) in self.state.params.iter_mut().zip(values) {
                param["value"] = value.clone();
            }
        }
        let mut output = self.context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(768.0, 840.0),
                )),
                time: Some(f64::from(frame) / 60.0),
                ..Default::default()
            },
            |ctx| {
                self.app.render_run_panels(
                    ctx,
                    self.state.clone(),
                    &RunTheme::from_dark_mode(true),
                );
            },
        );
        let primitives = self
            .context
            .tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        (output, primitives)
    }
}
