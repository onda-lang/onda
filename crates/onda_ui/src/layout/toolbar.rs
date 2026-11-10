//! Anchored, scrollable overflow menus shared by single-row panel headers.
use crate::{
    icons::Icon,
    interaction::{FocusLayer, ToolbarMenu},
    model::RunUi,
    scene::{Palette, Rect, Scene, Target},
};

pub(crate) fn button(
    ui: &RunUi,
    s: &mut Scene,
    p: Palette,
    rect: [f32; 4],
    menu: ToolbarMenu,
    description: &str,
) {
    let selected = ui.interaction.toolbar_menu.as_ref() == Some(&menu);
    super::header::icon_button(
        ui,
        s,
        p,
        rect,
        (Icon::More, description, Target::ToolbarMenu(menu)),
        selected,
    );
}

pub(super) fn build(ui: &RunUi, s: &mut Scene, p: Palette) {
    match &ui.interaction.toolbar_menu {
        Some(ToolbarMenu::Header) => super::header::menu(ui, s, p),
        Some(ToolbarMenu::Bank(input)) => super::bank::menu(ui, s, p, *input),
        Some(ToolbarMenu::Log) => crate::panels::log::menu(ui, s, p),
        Some(ToolbarMenu::Midi) => crate::panels::midi_menu(ui, s, p),
        Some(ToolbarMenu::Buffer(name)) => crate::panels::buffer_menu(ui, s, p, name),
        None => {}
    }
}

pub(crate) fn menu(
    ui: &RunUi,
    s: &mut Scene,
    p: Palette,
    kind: ToolbarMenu,
    content_height: f32,
    build: impl FnOnce(&mut Scene, [f32; 4]),
) {
    let Some(hit) = s
        .hits
        .iter()
        .find(|h| matches!(&h.target, Target::ToolbarMenu(menu) if menu == &kind))
    else {
        return;
    };
    let anchor = if kind == ToolbarMenu::Header {
        s.header_rect
    } else {
        hit.rect.0
    };
    let gap = if kind == ToolbarMenu::Header {
        6.0
    } else {
        4.0
    };
    let below = (ui.size[1] - anchor[1] - anchor[3] - gap - 12.0).max(1.0);
    let above = (anchor[1] - gap - 12.0).max(1.0);
    let down = below >= content_height || below >= above;
    let height = content_height.min(if down { below } else { above });
    let width = (ui.size[0] - 24.0).clamp(1.0, 240.0);
    let rect = [
        (anchor[0] + anchor[2] - width).clamp(12.0, (ui.size[0] - width - 12.0).max(12.0)),
        if down {
            anchor[1] + anchor[3] + gap
        } else {
            anchor[1] - gap - height
        },
        width,
        height,
    ];
    s.toolbar_menu_rect = rect;
    s.toolbar_scroll_max = (content_height - height).max(0.0);
    s.focus_layer = FocusLayer::Toolbar;
    s.panel(rect, p.panel, p.border, 7.0);
    s.hit(rect, Target::Overlay);
    let clip = s.clip;
    s.clip = Rect(rect).intersect(Rect(clip)).0;
    let mut content = rect;
    content[1] -= ui.interaction.toolbar_scroll.min(s.toolbar_scroll_max);
    build(s, content);
    s.scrollbar(
        ui,
        p,
        crate::scrollbar::ScrollRegion::Toolbar,
        rect,
        content_height,
        ui.interaction.toolbar_scroll,
    );
    s.clip = clip;
}
