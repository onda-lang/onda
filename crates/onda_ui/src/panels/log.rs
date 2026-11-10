use crate::{
    fonts::READING_SIZE,
    interaction::ToolbarMenu,
    model::RunUi,
    scene::{Palette, Rect, Scene, Target, TextStyle},
    scrollbar::ScrollRegion,
};

const ROW_HEIGHT: f32 = 20.0;

pub(crate) fn header_width(width: f32) -> f32 {
    if width >= crate::fonts::heading_width("Log", 14.0) + 43.0 + 110.0 {
        110.0
    } else {
        28.0
    }
}

pub(crate) fn header(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    let [x, y, w, _] = rect;
    if header_width(w) == 28.0 {
        crate::layout::toolbar::button(
            ui,
            s,
            p,
            [x + w - 37.0, y + 6.0, 28.0, 28.0],
            ToolbarMenu::Log,
            "Log actions",
        );
    } else {
        for (offset, width, label, target) in actions() {
            s.button(
                ui,
                label,
                [x + w - 9.0 - 110.0 + offset, y + 7.0, width, 26.0],
                target,
                p,
            );
        }
    }
}

fn actions() -> [(f32, f32, &'static str, Target); 2] {
    [
        (0.0, 48.0, "Copy", Target::CopyLog),
        (
            54.0,
            56.0,
            "Clear",
            Target::Command(serde_json::json!({"type":"clearLog"})),
        ),
    ]
}

pub(crate) fn menu(ui: &RunUi, s: &mut Scene, p: Palette) {
    crate::layout::toolbar::menu(ui, s, p, ToolbarMenu::Log, 68.0, |s, r| {
        for (i, (_, _, label, target)) in actions().into_iter().enumerate() {
            s.button(
                ui,
                label,
                [r[0] + 4.0, r[1] + 4.0 + i as f32 * 30.0, r[2] - 8.0, 28.0],
                target,
                p,
            );
        }
    });
}

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
    s.log_width = (w - 20.0).max(0.0);
    let old = s.clip;
    s.clip = Rect([x + 6.0, y + 30.0, (w - 20.0).max(0.0), 124.0])
        .intersect(Rect(old))
        .0;
    let text = ui.state["logText"].as_str().unwrap_or("");
    let count = text.lines().count();
    let max = (count as f32 * ROW_HEIGHT - 124.0).max(0.0);
    s.log_max_scroll = max;
    let scroll = if ui.presentation.log_follow {
        max
    } else {
        ui.presentation.log_scroll.min(max)
    };
    let first = (scroll / ROW_HEIGHT).floor() as usize;
    for (i, line) in text.lines().enumerate().skip(first).take(9) {
        let y = plot[1] + 4.0 + i as f32 * ROW_HEIGHT - scroll;
        if s.visible(y, ROW_HEIGHT) {
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
                    ROW_HEIGHT,
                ],
                READING_SIZE,
                p.ink,
                TextStyle {
                    mono: true,
                    ellipsis: false,
                    ..Default::default()
                },
            );
            s.clip = message_clip;
            if !context.is_empty() {
                let bounds = [
                    x + 6.0 + s.log_width - context_width,
                    y,
                    context_width,
                    ROW_HEIGHT,
                ];
                s.text_tooltip(bounds, context.clone());
                s.styled(
                    context,
                    bounds,
                    11.0,
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
    s.clip = Rect(plot).intersect(Rect(old)).0;
    s.scrollbar(
        ui,
        p,
        ScrollRegion::Log,
        [x, plot[1], w, 132.0],
        count as f32 * ROW_HEIGHT + 8.0,
        scroll,
    );
    s.scrollbar(
        ui,
        p,
        ScrollRegion::LogHorizontal,
        [x, plot[1], (w - 8.0).max(0.0), plot[3]],
        (w - 8.0).max(0.0) + ui.log_max_scroll_x,
        ui.presentation.log_x,
    );
    s.clip = old;
    166.0
}

pub(crate) fn context_width(context: &str, width: f32) -> f32 {
    (context.chars().count() as f32 * 6.6).min(width * 0.4)
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
