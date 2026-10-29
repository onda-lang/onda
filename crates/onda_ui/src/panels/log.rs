use crate::{
    model::RunUi,
    scene::{Palette, Rect, Scene, TextStyle},
};

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, y, w, _] = rect;
    let dropped = ui.state["printTransportDropCount"].as_u64().unwrap_or(0)
        + ui.state["delegateTransportDropCount"].as_u64().unwrap_or(0);
    let overflow = ui.state["printOverflowCount"].as_u64().unwrap_or(0)
        + ui.state["delegateOverflowCount"].as_u64().unwrap_or(0);
    s.text_row(
        format!("{dropped} dropped · {overflow} overflows · Shift + scroll to pan"),
        [x, y, w, 22.0],
        p,
    );
    let plot = [x, y + 26.0, w, 140.0];
    s.panel(plot, p.field, p.border, 5.0);
    s.log_rect = Rect(plot).intersect(Rect(s.clip)).0;
    s.log_width = (w - 12.0).max(0.0);
    let old = s.clip;
    s.clip = Rect([x + 6.0, y + 30.0, w - 12.0, 132.0])
        .intersect(Rect(old))
        .0;
    let text = ui.state["logText"].as_str().unwrap_or("");
    let count = text.lines().count();
    let max = (count as f32 * 18.0 - 132.0).max(0.0);
    s.log_max_scroll = max;
    let scroll = if ui.presentation.log_follow {
        max
    } else {
        ui.presentation.log_scroll.min(max)
    };
    let first = (scroll / 18.0).floor() as usize;
    for (i, line) in text.lines().enumerate().skip(first).take(9) {
        let y = plot[1] + 4.0 + i as f32 * 18.0 - scroll;
        if s.visible(y, 18.0) {
            let context = context(&ui.state["logEntries"][i]);
            let context_width = context_width(&context, s.log_width);
            let message_clip = s.clip;
            if !context.is_empty() {
                s.clip[2] = (s.clip[2] - context_width - 8.0).max(0.0);
            }
            s.styled(
                line,
                [
                    x + 6.0 - ui.presentation.log_x,
                    y,
                    s.log_width + ui.log_max_scroll_x,
                    18.0,
                ],
                12.0,
                p.ink,
                TextStyle {
                    mono: true,
                    ..Default::default()
                },
            );
            s.clip = message_clip;
            if !context.is_empty() {
                let bounds = [x + w - 6.0 - context_width, y, context_width, 18.0];
                s.tooltip(bounds, context.clone());
                s.styled(
                    context,
                    bounds,
                    10.0,
                    p.muted,
                    TextStyle {
                        mono: true,
                        ..Default::default()
                    },
                );
            }
        }
    }
    if text.is_empty() {
        s.text_row(
            "Output from print and delegates appears here.",
            [x + 8.0, plot[1] + 8.0, w - 16.0, 24.0],
            p,
        );
    }
    s.clip = old;
    166.0
}

pub(crate) fn context_width(context: &str, width: f32) -> f32 {
    (context.chars().count() as f32 * 6.0).min(width * 0.4)
}

pub(crate) fn context(entry: &serde_json::Value) -> String {
    let file = entry["source"]["file"].as_str().unwrap_or("");
    let line = entry["source"]["line"].as_u64();
    let owner = entry["lexicalOwner"].as_str().unwrap_or("");
    let location = if let Some(line) = line.filter(|_| !file.is_empty()) {
        format!("{file}:{line}")
    } else {
        String::new()
    };
    match (location.is_empty(), owner.is_empty()) {
        (false, false) => format!("{location} · {owner}"),
        (false, true) => location,
        _ => owner.into(),
    }
}
