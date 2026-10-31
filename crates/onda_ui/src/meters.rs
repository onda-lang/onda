//! Meter ballistics and geometry, shared by compact and horizontal layouts.
use crate::{
    model::RunUi,
    scene::{Scene, Shape, TextStyle},
    theme::{blend, Palette},
};
use onda_scope::MAX_CHANNELS;
use web_time::{Duration, Instant};

pub(crate) const HORIZONTAL_WIDTH: f32 = 128.0;

pub(crate) struct Meters {
    pub channels: usize,
    pub peaks: [f32; MAX_CHANNELS],
    held: [f32; MAX_CHANNELS],
    hold_until: [Instant; MAX_CHANNELS],
    clip_until: [Option<Instant>; MAX_CHANNELS],
}
impl Default for Meters {
    fn default() -> Self {
        Self {
            channels: 0,
            peaks: [0.0; MAX_CHANNELS],
            held: [0.0; MAX_CHANNELS],
            hold_until: [Instant::now(); MAX_CHANNELS],
            clip_until: [None; MAX_CHANNELS],
        }
    }
}
impl Meters {
    pub fn update(&mut self, channels: usize, peaks: &[f32], now: Instant) -> bool {
        let channels = channels.min(MAX_CHANNELS);
        let mut changed = self.channels != channels;
        self.channels = channels;
        for i in 0..MAX_CHANNELS {
            let peak = if i < channels {
                peaks.get(i).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
            changed |= self.peaks[i] != peak;
            self.peaks[i] = peak;
            if i >= channels || peak >= self.held[i] || now >= self.hold_until[i] {
                changed |= self.held[i] != peak;
                self.held[i] = peak;
                self.hold_until[i] = now + Duration::from_millis(500);
            }
            let clipped = self.clip_until[i].is_some();
            if peak >= 1.0 {
                self.clip_until[i] = Some(now + Duration::from_secs(1));
            } else if i >= channels || self.clip_until[i].is_some_and(|end| now >= end) {
                self.clip_until[i] = None;
            }
            changed |= clipped != self.clip_until[i].is_some();
        }
        changed
    }
}

fn level(peak: f32) -> f32 {
    if peak > 0.0 {
        ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub(crate) fn horizontal(
    s: &mut Scene,
    p: Palette,
    rect: [f32; 4],
    channels: usize,
    running: bool,
) {
    let [x, y, width, height] = rect;
    let left = x + 17.0;
    let length = width - 25.0;
    let row_height = ((height - 6.0 - (channels - 1) as f32 * 2.0) / channels as f32).min(10.0);
    let top = y + (height - (row_height * channels as f32 + (channels - 1) as f32 * 2.0)) * 0.5;
    for channel in 0..channels {
        let row = top + channel as f32 * (row_height + 2.0);
        let label = if channels == 2 {
            ["L", "R"][channel].to_owned()
        } else {
            (channel + 1).to_string()
        };
        s.styled(
            label,
            [x + 6.0, row - 2.0, 8.0, row_height + 4.0],
            8.0,
            p.muted,
            TextStyle {
                mono: true,
                centered: true,
                ..Default::default()
            },
        );
        let track = [left, row, length, row_height];
        s.shape(
            track,
            if running {
                blend(p.field, p.meter_track, 0.75)
            } else {
                p.meter_stopped
            },
            1.0,
        );
        for db in [-48, -36, -24, -12, -6] {
            s.shape(
                [
                    left + (db + 60) as f32 / 60.0 * length,
                    row,
                    1.0,
                    row_height,
                ],
                p.border,
                0.0,
            );
        }
        s.meter_rects.push(track);
    }
}

fn interval(r: [f32; 4], start: f32, end: f32) -> [f32; 4] {
    if r[2] > r[3] {
        [r[0] + r[2] * start, r[1], r[2] * (end - start), r[3]]
    } else {
        [r[0], r[1] + r[3] * (1.0 - end), r[2], r[3] * (end - start)]
    }
}

pub(crate) fn shapes(ui: &RunUi, out: &mut Vec<Shape>) {
    out.clear();
    if ui.state["running"] != true {
        return;
    }
    let p = Palette::new(ui.light);
    let meters = &ui.meters;
    for (i, &rect) in ui.scene.meter_rects.iter().enumerate() {
        let range = if ui.scene.meter_rects.len() == 1 {
            0..meters.channels
        } else {
            i..i + 1
        };
        let peak = meters.peaks[range.clone()]
            .iter()
            .copied()
            .fold(0.0, f32::max);
        let held = meters.held[range.clone()]
            .iter()
            .copied()
            .fold(0.0, f32::max);
        let clipped = meters.clip_until[range].iter().any(Option::is_some);
        let level = level(peak);
        let mut push = |start, end, color| {
            out.push(Shape {
                rect: interval(rect, start, end),
                color,
                params: [0.5, 0.0, 0.0, 0.0],
                clip: ui.scene.meter_clip,
            })
        };
        for (start, end, color) in [
            (0.0, 0.8, p.meter_active),
            (0.8, 0.95, p.meter_warning),
            (0.95, 1.0, p.danger),
        ] {
            if level > start {
                push(start, level.min(end), color);
            }
        }
        let held = self::level(held);
        let pixel = 1.0 / rect[2].max(rect[3]);
        if held > level {
            push((held - pixel).max(0.0), held, p.ink_color());
        }
        if clipped {
            push((1.0 - 2.0 * pixel).max(0.0), 1.0, p.danger);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_hold_transients_and_clipping_then_expire_on_the_shared_stream() {
        let mut meters = Meters::default();
        let now = Instant::now();
        meters.update(2, &[1.2, 0.5], now);
        meters.update(2, &[0.1, 0.0], now + Duration::from_millis(100));
        assert_eq!(&meters.peaks[..2], &[0.1, 0.0]);
        assert_eq!(&meters.held[..2], &[1.2, 0.5]);
        assert!(meters.clip_until[0].is_some() && meters.clip_until[1].is_none());
        assert!(meters.update(2, &[0.1, 0.0], now + Duration::from_millis(600)));
        assert_eq!(&meters.held[..2], &[0.1, 0.0]);
        assert!(meters.clip_until[0].is_some());
        assert!(meters.update(2, &[0.1, 0.0], now + Duration::from_millis(1100)));
        assert!(meters.clip_until[0].is_none());
        assert!(!meters.update(2, &[0.1, 0.0], now + Duration::from_millis(1200)));
        meters.update(1, &[f32::NAN], now + Duration::from_secs(2));
        assert!(meters.peaks.iter().all(|&peak| peak == 0.0));
        assert!(meters.clip_until.iter().all(Option::is_none));
    }
}
