use crate::{
    model::{FileDropTarget, RunUi},
    scene::{Palette, Rect, Scene, Target, TextStyle},
    theme::CARD_INSET,
};
use serde_json::{json, Value};

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let Some(buffers) = ui.state["buffers"].as_array() else {
        return 0.0;
    };
    let [x, top, w, _] = rect;
    let mut y = top;
    for b in buffers {
        let name = b["name"].as_str().unwrap_or("");
        let path = b["loadedPath"].as_str().filter(|s| !s.is_empty());
        let loaded = path.is_some() || b["loadedFrames"].as_u64().is_some_and(|n| n > 0);
        let summary = loaded_summary(b);
        let summary_width = summary
            .as_ref()
            .map_or(0.0, |s| s.chars().count() as f32 * 6.6);
        let stacked = summary.is_some() && w - 2.0 * CARD_INSET < summary_width + 102.0;
        let preview = b["waveform"]["minimums"].is_array() && b["waveform"]["maximums"].is_array();
        let details_height = if stacked { 36.0 } else { 18.0 };
        let height = 40.0 + details_height + if preview { 74.0 } else { 8.0 };
        let actions = [
            (
                if loaded { "Replace" } else { "Bind file" },
                [x + w - CARD_INSET - 136.0, y + 8.0, 76.0, 28.0],
                Target::Command(json!({"type":"chooseBufferFile","name":name})),
            ),
            (
                "Clear",
                [x + w - CARD_INSET - 54.0, y + 8.0, 54.0, 28.0],
                Target::Command(json!({"type":"clearBuffer","name":name})),
            ),
        ];
        if s.visible(y, height) {
            let hover =
                matches!(&ui.drop_target, Some(FileDropTarget::Buffer(target)) if target == name);
            s.panel(
                [x, y, w, height],
                if hover { p.accent_soft } else { p.card },
                if hover { p.accent } else { p.border },
                9.0,
            );
            s.buffers
                .push((Rect([x, y, w, height]).intersect(Rect(s.clip)), name.into()));
            let ty = b["type"].as_str().unwrap_or("audio");
            let available = (w - 2.0 * CARD_INSET - 146.0).max(1.0);
            let type_width = ty.chars().count() as f32 * 6.6;
            let name_width =
                (name.chars().count() as f32 * 8.4).min((available - type_width - 7.0).max(1.0));
            s.tooltip(
                [x + CARD_INSET, y + 8.0, available, 28.0],
                format!("{name} ({ty})"),
            );
            s.styled(
                name,
                [x + CARD_INSET, y + 8.0, name_width, 28.0],
                14.0,
                p.ink,
                TextStyle {
                    mono: true,
                    bold: true,
                    ..Default::default()
                },
            );
            if name_width + type_width + 7.0 <= available {
                s.styled(
                    ty,
                    [x + CARD_INSET + 7.0 + name_width, y + 8.0, type_width, 28.0],
                    11.0,
                    p.muted,
                    TextStyle {
                        mono: true,
                        ..Default::default()
                    },
                );
            }
            let file = path
                .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
                .unwrap_or(if loaded {
                    "Embedded project asset"
                } else {
                    "Drop an audio file here"
                });
            let file_width = if stacked || summary.is_none() {
                w - 2.0 * CARD_INSET
            } else {
                w - 2.0 * CARD_INSET - 6.0 - summary_width
            };
            s.tooltip(
                [x + CARD_INSET, y + 40.0, file_width, 18.0],
                path.unwrap_or(file).into(),
            );
            s.styled(
                file,
                [x + CARD_INSET, y + 40.0, file_width, 18.0],
                12.0,
                p.muted,
                TextStyle {
                    mono: true,
                    ..Default::default()
                },
            );
            if let Some(summary) = summary {
                s.styled(
                    summary,
                    [
                        if stacked {
                            x + CARD_INSET
                        } else {
                            x + w - CARD_INSET - summary_width
                        },
                        y + 40.0 + if stacked { 18.0 } else { 0.0 },
                        (w - 2.0 * CARD_INSET).min(summary_width),
                        18.0,
                    ],
                    11.0,
                    p.muted,
                    TextStyle {
                        mono: true,
                        ..Default::default()
                    },
                );
            }
            if preview {
                waveform(
                    s,
                    p,
                    b,
                    [
                        x + CARD_INSET,
                        y + 44.0 + details_height,
                        w - 2.0 * CARD_INSET,
                        62.0,
                    ],
                    ui.scale,
                );
            }
            for (label, rect, target) in actions {
                s.button(ui, label, rect, target, p);
            }
        } else {
            for (_, rect, target) in actions {
                s.focus_target(rect, &target);
            }
        }
        y += height + 6.0;
    }
    (y - top - 6.0).max(0.0)
}

fn loaded_summary(buffer: &Value) -> Option<String> {
    let digits = buffer["loadedFrames"].as_u64()?.to_string();
    let mut frames = String::new();
    for (i, c) in digits.char_indices() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            frames.push(',');
        }
        frames.push(c);
    }
    let channels = buffer["loadedChannels"].as_u64()?;
    let rate = buffer["loadedSampleRate"].as_f64()?;
    let rate = if rate >= 1000.0 {
        format!(
            "{} kHz",
            crate::editing::number_text(&json!(rate / 1000.0), 1)
        )
    } else {
        format!("{rate:.0} Hz")
    };
    Some(format!("{frames} frames · {channels} ch · {rate}"))
}

fn waveform(s: &mut Scene, p: Palette, b: &Value, wave: [f32; 4], scale: f32) {
    s.shape(wave, p.scope, 6.0);
    let Some((lo, hi)) = b["waveform"]["minimums"]
        .as_array()
        .zip(b["waveform"]["maximums"].as_array())
    else {
        return;
    };
    let count = lo.len().min(hi.len());
    let cols = count.min(512);
    let extent = b["waveform"]["minValue"]
        .as_f64()
        .unwrap_or(-1.0)
        .abs()
        .max(b["waveform"]["maxValue"].as_f64().unwrap_or(1.0).abs())
        .max(1.0);
    let labels = [extent, 0.0, -extent].map(|v| crate::editing::number_text(&json!(v), 3));
    let label_width = labels.iter().map(|s| s.chars().count()).max().unwrap_or(1) as f32 * 6.0;
    let plot = [
        wave[0] + 10.0 + label_width,
        wave[1] + 5.0,
        (wave[2] - 15.0 - label_width).max(1.0),
        wave[3] - 10.0,
    ];
    let snap = |v: f32| ((v * scale).floor() + 0.5) / scale;
    let zero = snap(plot[1] + plot[3] * 0.5);
    let value_y = |v: f64| snap(zero - (v / extent) as f32 * plot[3] * 0.5);
    let guide = crate::theme::blend(p.accent, p.scope, 0.14);
    s.line(
        [plot[0], zero],
        [plot[0] + plot[2], zero],
        1.0 / scale,
        guide,
    );
    if extent > 1.0 && plot[3] * scale / (2.0 * extent as f32) >= 2.0 {
        for value in [-1.0, 1.0] {
            let y = value_y(value);
            s.line([plot[0], y], [plot[0] + plot[2], y], 1.0 / scale, guide);
        }
    }
    for (label, y) in labels
        .iter()
        .zip([plot[1], zero - 7.0, plot[1] + plot[3] - 14.0])
    {
        s.styled(
            label,
            [wave[0] + 5.0, y, label_width, 14.0],
            10.0,
            p.muted,
            TextStyle {
                mono: true,
                ..Default::default()
            },
        );
    }
    for col in 0..cols {
        let start = col * count / cols;
        let end = ((col + 1) * count / cols).max(start + 1);
        let low = lo[start..end]
            .iter()
            .filter_map(Value::as_f64)
            .fold(f64::INFINITY, f64::min)
            .clamp(-extent, extent);
        let high = hi[start..end]
            .iter()
            .filter_map(Value::as_f64)
            .fold(f64::NEG_INFINITY, f64::max)
            .clamp(-extent, extent);
        if low.is_finite() && high.is_finite() {
            let x = snap(plot[0] + (col as f32 + 0.5) * plot[2] / cols as f32);
            let (top, bottom) = (value_y(high.max(0.0)), value_y(low.min(0.0)));
            s.line([x, top], [x, bottom], 1.25, p.accent);
            for value in [Some(high), (low != high).then_some(low)]
                .into_iter()
                .flatten()
            {
                s.shape([x - 0.75, value_y(value) - 0.75, 1.5, 1.5], p.accent, 0.75);
            }
        }
    }
}
