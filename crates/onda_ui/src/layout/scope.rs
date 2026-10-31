//! The same channel viewport is used by the pinned section and transient overlay.
use crate::{
    icons::Icon,
    model::RunUi,
    scene::{Palette, Rect, Scene, Target, TextStyle},
};

pub(super) fn pin(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    super::header::icon_button(
        ui,
        s,
        p,
        rect,
        (
            Icon::Pin(ui.presentation.scope_pinned),
            if ui.presentation.scope_pinned {
                "Unpin scope"
            } else {
                "Pin scope"
            },
            Target::PinScope,
        ),
        false,
    );
}
pub(super) fn plot(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, y, w, h] = rect;
    let channels = ui.output_channels();
    let content_height = h.max(channels as f32 * 40.0);
    let scroll = ui
        .interaction
        .scope_scroll
        .min((content_height - h).max(0.0));
    s.scope_scroll_max = (content_height - h).max(0.0);
    s.shape(rect, p.scope, 6.0);
    s.scope_clip = Rect(rect).intersect(Rect(s.clip)).0;
    s.scope_rect = [x, y - scroll, w, content_height];
    let clip = s.clip;
    s.clip = s.scope_clip;
    for channel in 0..channels {
        let row = content_height / channels as f32;
        let cy = y - scroll + channel as f32 * row;
        s.shape(
            [x, cy + row * 0.5, w, 1.0],
            crate::theme::blend(p.accent, p.scope, 0.14),
            0.0,
        );
        s.styled(
            if channels == 2 {
                if channel == 0 {
                    "L".into()
                } else {
                    "R".into()
                }
            } else {
                format!("Ch {}", channel + 1)
            },
            [x + 5.0, cy + 2.0, (w - 10.0).max(1.0), 16.0],
            11.0,
            p.muted,
            TextStyle {
                mono: true,
                ..Default::default()
            },
        );
    }
    s.scope_shape_end = s.shapes.len();
    s.scrollbar(
        ui,
        p,
        crate::scrollbar::ScrollRegion::Scope,
        rect,
        content_height,
        scroll,
    );
    s.clip = clip;
    h
}
pub(super) fn overlay(ui: &RunUi, s: &mut Scene, p: Palette) {
    if ui.presentation.scope_pinned || !ui.scope_open() {
        return;
    }
    let [hx, hy, hw, hh] = s.header_rect;
    let w = hw.min(320.0);
    let top = (hy + hh + 6.0).clamp(4.0, (s.viewport_height - 72.0).max(4.0));
    let h = 208.0_f32.min((s.viewport_height - top - 8.0).max(64.0));
    let r = [hx + hw - w, top, w, h];
    s.scope_panel = r;
    s.scope_overlay = true;
    s.panel(r, p.panel, p.border, 10.0);
    s.hit(r, Target::Overlay);
    s.styled(
        "Scope",
        [r[0] + 9.0, top + 3.0, (w - 70.0).max(1.0), 28.0],
        14.0,
        p.ink,
        TextStyle {
            bold: true,
            ..Default::default()
        },
    );
    pin(ui, s, p, [r[0] + w - 34.0, top + 3.0, 28.0, 28.0]);
    plot(ui, s, p, [r[0] + 8.0, top + 34.0, w - 16.0, h - 42.0]);
}
