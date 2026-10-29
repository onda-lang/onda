//! Measure complete log lines once; retain widths across appended output and scrolling.
use crate::{panels::log, RunUi};
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics as TextMetrics, Shaping, Wrap};
use std::collections::HashMap;

pub(super) struct Metrics {
    buffer: Buffer,
    widths: HashMap<String, (f32, u64)>,
    generation: u64,
    key: Option<(usize, u64, f32)>,
}

impl Metrics {
    pub fn new(fonts: &mut FontSystem) -> Self {
        let mut buffer = Buffer::new(fonts, TextMetrics::new(12.0, 16.8));
        buffer.set_wrap(Wrap::None);
        Self {
            buffer,
            widths: HashMap::new(),
            generation: 0,
            key: None,
        }
    }

    pub fn clear(&mut self) {
        self.key = None;
        self.widths.clear();
    }

    pub fn prepare(&mut self, ui: &mut RunUi, fonts: &mut FontSystem) {
        let width = ui.scene.log_width;
        if width <= 0.0 {
            return;
        }
        let key = (ui.id, ui.log_revision, width);
        let mut changed = false;
        if self.key != Some(key) {
            if self.key.is_some_and(|key| key.0 != ui.id) {
                self.widths.clear();
            }
            self.generation = self.generation.wrapping_add(1);
            let mut maximum = 0.0_f32;
            for (index, line) in ui.state["logText"]
                .as_str()
                .unwrap_or("")
                .lines()
                .enumerate()
            {
                let line_width = if let Some((width, seen)) = self.widths.get_mut(line) {
                    *seen = self.generation;
                    *width
                } else {
                    self.buffer.set_text(
                        line,
                        &Attrs::new().family(Family::Monospace),
                        Shaping::Advanced,
                        None,
                    );
                    self.buffer.shape_until_scroll(fonts, false);
                    let width = self
                        .buffer
                        .layout_runs()
                        .map(|run| run.line_w)
                        .fold(0.0, f32::max)
                        .ceil();
                    self.widths
                        .insert(line.to_owned(), (width, self.generation));
                    width
                };
                let context = log::context(&ui.state["logEntries"][index]);
                let reserved = if context.is_empty() {
                    0.0
                } else {
                    log::context_width(&context, width) + 8.0
                };
                maximum = maximum.max(line_width - (width - reserved).max(0.0));
            }
            self.widths.retain(|_, (_, seen)| *seen == self.generation);
            changed = ui.log_max_scroll_x != maximum;
            ui.log_max_scroll_x = maximum;
            self.key = Some(key);
        }
        let scroll = ui.presentation.log_x.clamp(0.0, ui.log_max_scroll_x);
        if ui.presentation.log_x != scroll {
            ui.presentation.log_x = scroll;
            ui.presentation.scrolled();
            changed = true;
        }
        if changed {
            ui.invalidate();
        }
    }
}
