//! The run screen scrolls as one document; the MIDI keyboard stays docked.
mod header;
mod sections;

use crate::{
    model::RunUi,
    panels,
    presentation::Section,
    scene::{Palette, Rect, Scene, Target},
    theme::{GAP, GUTTER},
};

pub(crate) fn midi_visible(ui: &RunUi) -> bool {
    ui.state["midi"]["noteOn"] == true
}
pub(crate) fn dock_height(ui: &RunUi) -> f32 {
    if !midi_visible(ui) {
        0.0
    } else if ui.presentation.open(Section::Midi) {
        panels::midi_height(ui)
    } else {
        46.0
    }
}
pub(crate) fn build(ui: &RunUi) -> Scene {
    let mut s = Scene::default();
    let p = Palette::new(ui.light);
    let [w, h] = ui.size;
    let viewport = (h - dock_height(ui)).max(1.0);
    s.clip = [0.0, 0.0, w, viewport];
    s.viewport_height = viewport;
    let width = (w - GUTTER * 2.0 - 4.0).max(1.0);
    let mut y = GUTTER - ui.scroll;
    let mut header_bottom = y;
    let loaded = ui.loaded();
    if loaded {
        header_bottom = y + header::build(ui, &mut s, p, [GUTTER, y, width, 0.0]);
        y = header_bottom + GAP;
        if let Some(error) = ui.state["error"].as_str().filter(|s| !s.is_empty()) {
            let height = ui.diagnostic_height + 20.0;
            let rect = [GUTTER, y, width, height];
            s.panel(rect, p.card, p.danger, 9.0);
            s.paragraph(
                error,
                [GUTTER + 10.0, y + 10.0, width - 20.0, ui.diagnostic_height],
                crate::theme::text(if ui.light { 0xb53333 } else { 0xff7e7e }),
            );
            y += height + GAP;
        }
        y = sections::build(ui, &mut s, p, [GUTTER, y, width, 0.0]);
    } else if ui.state["supportsSourceSelection"] == true {
        let height = (viewport - 24.0)
            .max(header::picker_content_height(ui, width) + 24.0)
            .max(300.0);
        header::picker(ui, &mut s, p, [GUTTER, y, width, height]);
        y += height + GAP;
        s.scope_shape_end = s.shapes.len();
    }
    s.content_height = y + ui.scroll;
    s.scrollbar = crate::scrollbar::Scrollbar::new(w, viewport, s.content_height, ui.scroll);
    if let Some(bar) = s.scrollbar {
        let color = if s.hovered(ui, bar.track.0) || ui.interaction.dragging_scrollbar() {
            p.scrollbar_hover
        } else {
            p.scrollbar
        };
        s.shape(bar.thumb.0, color, 3.5);
        s.hit(bar.track.0, Target::Scrollbar);
    }
    s.clip = [0.0, 0.0, w, h];
    s.focus_layer = crate::interaction::FocusLayer::Dock;
    if midi_visible(ui) {
        panels::midi(ui, &mut s, p, [0.0, viewport, w, h - viewport]);
    }
    s.overlay_shapes = s.shapes.len();
    s.overlay_labels = s.labels.len();
    s.overlay_hits = s.hits.len();
    if ui.interaction.settings_open() {
        s.focus_layer = crate::interaction::FocusLayer::Popup;
        // Keep popup painting and hit testing above the entire document and dock.
        let popup_width = width.min(256.0);
        let popup_top = header_bottom.clamp(GUTTER, (h - GUTTER - 60.0).max(GUTTER));
        let rect = [
            GUTTER + width - popup_width,
            popup_top,
            popup_width,
            (h - popup_top - GUTTER).clamp(60.0, 600.0),
        ];
        let panel = s.shapes.len();
        s.popup_rect = rect;
        s.panel(rect, p.panel, p.border, 12.0);
        s.clip = Rect([rect[0] + 1.0, rect[1] + 1.0, rect[2] - 2.0, rect[3] - 2.0])
            .intersect(Rect([0.0, 0.0, w, h]))
            .0;
        s.popup_height = panels::settings(
            ui,
            &mut s,
            p,
            [
                rect[0] + 10.0,
                rect[1] + 10.0 - ui.interaction.popup_scroll(),
                rect[2] - 20.0,
                0.0,
            ],
        ) + 20.0;
        let height = rect[3].min(s.popup_height);
        s.popup_rect[3] = height;
        s.shapes[panel].rect[3] = height;
        s.shapes[panel + 1].rect[3] = (height - 2.0).max(0.0);
    }
    s.menu_shapes = s.shapes.len();
    s.menu_labels = s.labels.len();
    s.menu_hits = s.hits.len();
    crate::interaction::build_menu(ui, &mut s, p);
    crate::interaction::build_tooltip(ui, &mut s, p);
    s
}
