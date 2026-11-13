use crate::{
    icons::{logo_outer_radius, Icon},
    model::{FileDropTarget, RunUi},
    scene::{Palette, Rect, Scene, Target, TextStyle},
    theme::{blend, CONTROL_RADIUS},
};
use serde_json::json;

const SEPARATOR_GAP: f32 = 10.0;
const SEPARATOR_WIDTH: f32 = 1.0;

pub(super) fn icon_button(
    ui: &RunUi,
    s: &mut Scene,
    p: Palette,
    rect: [f32; 4],
    button: (Icon, &str, Target),
    active: bool,
) {
    let (icon, description, target) = button;
    if active || s.hovered(ui, rect) || ui.focused(&target) {
        s.panel(rect, p.accent_soft, p.accent, CONTROL_RADIUS);
    }
    s.icon(
        icon,
        [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5],
        p.ink_color(),
    );
    s.tooltip(rect, description.into());
    s.hit(rect, target);
}
fn actions(ui: &RunUi) -> Vec<(Icon, &'static str, Target)> {
    let mut actions = vec![(Icon::Settings, "Settings", Target::Devices)];
    if ui.state["supportsProjectExport"] == true && ui.state["canExportProject"] == true {
        actions.push((
            Icon::Save,
            "Save as Project",
            Target::Command(json!({"type":"saveProjectAs"})),
        ));
    }
    if ui.state["supportsSourceSelection"] == true {
        actions.push((
            Icon::Unload,
            "Unload",
            Target::Command(json!({"type":"unload"})),
        ));
    }
    actions
}
fn source(ui: &RunUi, compact: bool) -> (&str, String) {
    let path = ui.state["path"].as_str().unwrap_or("Onda");
    let status = ui.state["status"]
        .as_str()
        .unwrap_or(if ui.state["running"] == true {
            "Running"
        } else {
            "Stopped"
        });
    let mut details = status.to_owned();
    if !status.contains('·') && !status.contains('—') {
        if let Some(rate) = ui.state["sampleRateHz"].as_f64() {
            let unit = if compact { "k" } else { " kHz" };
            details.push_str(&format!(" · {}{unit}", rate / 1000.0));
        }
        if let Some(block) = ui.state["blockSamples"].as_u64() {
            let unit = if compact { "" } else { " samples" };
            details.push_str(&format!(" · {block}{unit}"));
        }
        if ui.state["fastMath"] == true {
            details.push_str(if compact { " · FM" } else { " · Fast Math" });
        }
    }
    (path, details)
}
pub(super) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, y, w, _] = rect;
    let dense = w < 452.0;
    let minimal = w < 182.0;
    let height = if minimal { 44.0 } else { 56.0 };
    s.header_rect = [x, y, w, height];
    s.panel(s.header_rect, p.panel, p.border, 10.0);
    let actions = actions(ui);
    let overflow = dense && actions.len() > 1;
    let scope = ui.scope_available();
    let scope_icon = !minimal && !ui.presentation.scope_pinned;
    let channels = ui.output_channels();
    let transport = ui.state["supportsTransport"] != false;
    let transport_width = if dense { 28.0 } else { 56.0 };
    let utility_width = if overflow {
        28.0
    } else {
        (actions.len() as f32 * 32.0 - 4.0).max(0.0)
    };
    let gap = if (scope || transport) && utility_width > 0.0 {
        if dense {
            6.0
        } else {
            2.0 * SEPARATOR_GAP + SEPARATOR_WIDTH - actions[0].0.header_inset()
        }
    } else {
        0.0
    };
    let padding = if minimal { 4.0 } else { 10.0 };
    let brand = w >= 252.0;
    // Preserve room for the source label before expanding the per-channel meters.
    let source_width = if minimal {
        padding
    } else if brand {
        132.0
    } else {
        90.0
    };
    let audio_gap = if scope && transport { 4.0 } else { 0.0 };
    let available = w
        - padding
        - source_width
        - 8.0
        - gap
        - utility_width
        - if transport { transport_width } else { 0.0 }
        - audio_gap;
    let full_meter_width =
        ((if scope_icon { 26.0 } else { 12.0 }) + channels as f32 * 6.0).max(32.0);
    let horizontal = scope && channels <= 4 && available >= crate::meters::HORIZONTAL_WIDTH;
    let combined = channels > 1 && full_meter_width > available;
    let meters = if combined { 1 } else { channels };
    let meter_width = if horizontal {
        crate::meters::HORIZONTAL_WIDTH
    } else if combined {
        if scope_icon {
            46.0
        } else {
            32.0
        }
    } else {
        full_meter_width
    };
    let audio_width = if scope { meter_width } else { 0.0 }
        + if transport { transport_width } else { 0.0 }
        + audio_gap;
    let controls_width = audio_width + gap + utility_width;
    let mut bx = x + w - padding - controls_width;
    let by = y + (height - 28.0) * 0.5;
    let mut sx = x + 10.0;
    if brand {
        let brand_top = y + (height - 42.0) * 0.5;
        let version = ui.state["ondaVersion"]
            .as_str()
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        let version_rect = [x + 6.0, brand_top + 26.0, 40.0, 16.0];
        let brand_right = (x + 26.0 + logo_outer_radius(24.0)).max(
            version_rect[0]
                + crate::fonts::centered_monospace_right(version, 11.0, version_rect[2]),
        );
        let separator_x = brand_right + SEPARATOR_GAP;
        s.shape(
            [separator_x, y + 10.0, SEPARATOR_WIDTH, height - 20.0],
            p.border,
            0.0,
        );
        s.logo([x + 14.0, brand_top, 24.0, 24.0], p.accent);
        sx = separator_x + SEPARATOR_WIDTH + SEPARATOR_GAP;
        s.styled(
            version,
            version_rect,
            11.0,
            p.muted,
            TextStyle {
                mono: true,
                centered: true,
                ..Default::default()
            },
        );
    }
    let sw = (bx - sx - 8.0).max(0.0);
    if !minimal && sw > 0.0 {
        let (path, details) = source(ui, false);
        let label = if crate::fonts::proportional_width(&details, 11.0, false) > sw {
            source(ui, true).1
        } else {
            details.clone()
        };
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        s.styled(
            name,
            [sx, y + 9.0, sw, 21.0],
            14.0,
            p.ink,
            TextStyle {
                mono: true,
                bold: true,
                ..Default::default()
            },
        );
        s.styled(
            label,
            [sx, y + 30.0, sw, 16.0],
            11.0,
            p.muted,
            TextStyle::default(),
        );
        s.text_tooltip([sx, y + 9.0, sw, 21.0], path.into());
        s.text_tooltip([sx, y + 30.0, sw, 16.0], details);
    }
    if scope {
        let running = ui.state["running"] == true;
        let pinned = ui.presentation.scope_pinned;
        let active = !pinned && ui.scope_open();
        let r = [bx, by, meter_width, 28.0];
        s.panel(
            r,
            if active { p.accent_soft } else { p.field },
            if !pinned && (active || s.hovered(ui, r) || ui.focused(&Target::Scope)) {
                p.accent
            } else {
                p.border
            },
            5.0,
        );
        let mut mx = bx + 6.0;
        if scope_icon {
            s.icon(
                Icon::Scope,
                [mx + 6.0, by + 14.0],
                if running {
                    p.ink_color()
                } else {
                    p.muted_color()
                },
            );
            mx += 17.0;
        } else {
            mx = bx
                + if combined {
                    6.0
                } else {
                    (meter_width - meters as f32 * 6.0 + 3.0) * 0.5
                };
        }
        s.meter_clip = s.clip;
        if horizontal {
            let inset = if scope_icon { 17.0 } else { 0.0 };
            crate::meters::horizontal(
                s,
                p,
                [bx + inset, by, meter_width - inset, 28.0],
                channels,
                running,
            );
        } else {
            for _ in 0..meters {
                let r = [mx, by + 6.0, 3.0, 16.0];
                s.shape(
                    r,
                    if running {
                        p.meter_track
                    } else {
                        p.meter_stopped
                    },
                    1.0,
                );
                s.meter_rects.push(r);
                mx += 6.0;
            }
        }
        if combined {
            s.styled(
                channels.to_string(),
                [mx, by, 14.0, 28.0],
                11.0,
                p.muted,
                TextStyle {
                    mono: true,
                    ..Default::default()
                },
            );
        }
        if !pinned {
            s.tooltip(
                r,
                if combined {
                    format!("Scope · peak across all {channels} channels")
                } else {
                    "Scope".into()
                },
            );
            s.hit(r, Target::Scope);
        }
        bx += meter_width + if transport { 4.0 } else { 0.0 };
    }
    if transport {
        let running = ui.state["running"] == true;
        let color = if running { p.danger } else { p.success };
        let button = [bx, by, transport_width, 28.0];
        s.panel(
            button,
            if running {
                blend(color, p.panel, 0.1)
            } else {
                color
            },
            color,
            5.0,
        );
        let ink = if running { color } else { p.background };
        let label = if running { "Stop" } else { "Play" };
        let label_size = 12.0;
        let label_width = crate::fonts::heading_width(label, label_size);
        let content_x = if dense {
            bx + 8.0
        } else {
            bx + (transport_width - 18.0 - label_width) * 0.5
        };
        s.icon(
            if running { Icon::Stop } else { Icon::Play },
            [content_x + 6.0, by + 14.0],
            ink,
        );
        if !dense {
            s.styled(
                label,
                [content_x + 18.0, by, label_width + 1.0, 28.0],
                label_size,
                std::array::from_fn(|i| (ink[i] * 255.0).round() as u8),
                TextStyle {
                    bold: true,
                    ..Default::default()
                },
            );
        }
        s.tooltip(button, label.into());
        s.hit(button, Target::Transport);
        bx += transport_width;
    }
    if gap > 0.0 && !dense {
        s.shape(
            [bx + SEPARATOR_GAP, y + 10.0, SEPARATOR_WIDTH, height - 20.0],
            p.border,
            0.0,
        );
    }
    bx += gap;
    if overflow {
        icon_button(
            ui,
            s,
            p,
            [bx, by, 28.0, 28.0],
            (
                Icon::More,
                "Actions",
                Target::ToolbarMenu(crate::interaction::ToolbarMenu::Header),
            ),
            ui.interaction.toolbar_menu == Some(crate::interaction::ToolbarMenu::Header),
        );
    } else {
        for (icon, label, target) in actions {
            let active = target == Target::Devices && ui.interaction.settings_open();
            icon_button(
                ui,
                s,
                p,
                [bx, by, 28.0, 28.0],
                (icon, label, target),
                active,
            );
            bx += 32.0;
        }
    }
    height
}
pub(super) fn menu(ui: &RunUi, s: &mut Scene, p: Palette) {
    let actions = actions(ui);
    let content_height = 8.0 + actions.len() as f32 * 30.0;
    super::toolbar::menu(
        ui,
        s,
        p,
        crate::interaction::ToolbarMenu::Header,
        content_height,
        |s, r| {
            let [_, top, w, _] = r;
            for (i, (icon, label, target)) in actions.into_iter().enumerate() {
                let button = [r[0] + 4.0, top + 4.0 + i as f32 * 30.0, w - 8.0, 28.0];
                s.button(ui, "", button, target, p);
                s.icon(icon, [button[0] + 14.0, button[1] + 14.0], p.ink_color());
                s.styled(
                    label,
                    [button[0] + 28.0, button[1], (w - 40.0).max(1.0), 28.0],
                    12.0,
                    p.ink,
                    TextStyle::default(),
                );
            }
        },
    );
}

fn picker_settings_height(ui: &RunUi) -> f32 {
    match (
        !picker_selects(ui).is_empty(),
        ui.state["supportsFastMath"] == true,
    ) {
        (true, _) => 254.0,
        (false, true) => 248.0,
        (false, false) => 181.0,
    }
}
fn picker_selects(ui: &RunUi) -> Vec<(&'static str, crate::interaction::SelectId)> {
    use crate::{
        editing::{Field, Setting},
        interaction::SelectId,
    };
    [
        ("SAMPLE RATE", SelectId::SampleRate, Setting::SampleRate),
        ("BLOCK SIZE", SelectId::BlockSize, Setting::BlockSize),
    ]
    .into_iter()
    .filter(|(_, _, setting)| ui.field_enabled(&Field::Setting(*setting)))
    .map(|(label, id, _)| (label, id))
    .collect()
}
pub(super) fn picker_content_height(ui: &RunUi) -> f32 {
    picker_settings_height(ui)
        + if ui.state["error"].as_str().is_some_and(|e| !e.is_empty()) {
            12.0 + ui.diagnostic_height
        } else {
            0.0
        }
}
pub(super) fn picker(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    let [x, y, w, h] = rect;
    let center = x + w * 0.5;
    let selects = picker_selects(ui);
    let fast_math = ui.state["supportsFastMath"] == true;
    let content_height = picker_content_height(ui);
    let top = y + (h - content_height).max(0.0) * 0.5;
    s.source_picker = Some(Rect(rect).intersect(Rect(s.clip)));
    let hover = ui.drop_target == Some(FileDropTarget::Source);
    s.panel(
        rect,
        if hover { p.accent_soft } else { p.panel },
        if hover { p.accent } else { p.border },
        14.0,
    );
    s.logo([center - 38.0, top, 76.0, 76.0], p.accent);
    s.styled(
        ui.state["ondaVersion"]
            .as_str()
            .unwrap_or(env!("CARGO_PKG_VERSION")),
        [x + 12.0, top + 80.0, w - 24.0, 18.0],
        11.0,
        p.muted,
        TextStyle {
            mono: true,
            centered: true,
            ..Default::default()
        },
    );
    let width = (w - 32.0).min(272.0);
    let button = [center - width * 0.5, top + 112.0, width, 36.0];
    s.panel(
        button,
        if s.hovered(ui, button) {
            blend(p.accent, p.ink_color(), 0.7)
        } else {
            p.accent
        },
        p.accent,
        5.0,
    );
    let text = std::array::from_fn(|i| (p.background[i] * 255.0).round() as u8);
    s.styled(
        "Open Onda source or project",
        button,
        13.0,
        text,
        TextStyle {
            bold: true,
            centered: true,
            ..Default::default()
        },
    );
    s.hit(button, Target::Command(json!({"type":"chooseOndaFile"})));
    s.styled(
        "or drop an .onda or .ondaproject file here",
        [x + 12.0, top + 155.0, w - 24.0, 26.0],
        12.0,
        p.muted,
        TextStyle {
            centered: true,
            ..Default::default()
        },
    );
    if let Some(error) = ui.state["error"].as_str().filter(|e| !e.is_empty()) {
        s.paragraph(
            error,
            [
                x + 12.0,
                top + picker_settings_height(ui) + 12.0,
                w - 24.0,
                ui.diagnostic_height,
            ],
            crate::fonts::READING_SIZE,
            crate::theme::text(if ui.light { 0xb53333 } else { 0xff7e7e }),
        )
        .style
        .centered = true;
    }
    if !selects.is_empty() || fast_math {
        let fast_width = if fast_math {
            70.0_f32.min((w - 24.0) * 0.3)
        } else {
            0.0
        };
        let count = selects.len() as f32;
        let gaps = (count + f32::from(fast_math) - 1.0).max(0.0) * 10.0;
        let width = ((w - 24.0 - fast_width - gaps) / count.max(1.0)).clamp(1.0, 140.0);
        let group_width = count * width + fast_width + gaps;
        let left = center - group_width * 0.5;
        for (index, (label, id)) in selects.into_iter().enumerate() {
            let x = left + index as f32 * (width + 10.0);
            s.styled(
                label,
                [x, top + 194.0, width, 16.0],
                11.0,
                p.muted,
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
            s.select(ui, id, [x, top + 218.0, width, 36.0], p);
        }
        if fast_math {
            let x = left + group_width - fast_width;
            let y = top + 194.0;
            s.styled(
                "FAST MATH",
                [x, y, fast_width, 16.0],
                11.0,
                p.muted,
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
            s.toggle(
                ui,
                Target::FastMath,
                [x + (fast_width - 24.0) * 0.5, y + 30.0, 24.0, 24.0],
                ui.state["fastMath"] == true,
                true,
                p,
            );
        }
    }
    empty_actions(ui, s, p, rect);
}

pub(super) fn empty_actions(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    let [x, y, w, _] = rect;
    if ui.state["supportsThemeSelection"] != false {
        icon_button(
            ui,
            s,
            p,
            [x + w - 40.0, y + 12.0, 28.0, 28.0],
            (Icon::Theme, "Toggle dark / light theme", Target::Theme),
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_headers_keep_actions_on_one_row_and_disclose_clipped_filenames() {
        let path = "/projects/a_long_project_directory/a_very_long_patch_filename.onda";
        for width in [480.0, 570.0, 600.0, 720.0] {
            for transport in [false, true] {
                let mut ui = RunUi::default();
                ui.resize(width, 840.0, 1.0);
                ui.message(json!({"type":"state","state":{
                    "path":path,"supportsScope":false,"supportsTransport":transport,
                    "supportsDeviceSelection":true,"supportsSourceSelection":true,
                    "supportsProjectExport":true,"canExportProject":true
                }}));
                ui.prepare_scene();
                let header = ui.scene.shapes[0].rect;
                assert_eq!(header[3], 56.0);
                let name = ui
                    .scene
                    .labels
                    .iter()
                    .find(|l| l.text.ends_with("filename.onda"))
                    .unwrap();
                assert!(!name.style.wrap);
                assert!(name.x + name.width <= ui.scene.hits[0].rect.0[0]);
                let center_y = ui.scene.hits[0].rect.0[1] + ui.scene.hits[0].rect.0[3] * 0.5;
                for hit in &ui.scene.hits {
                    assert_eq!(hit.rect.0[1] + hit.rect.0[3] * 0.5, center_y);
                    assert!(hit.rect.0[0] >= header[0]);
                    assert!(hit.rect.0[0] + hit.rect.0[2] <= header[0] + header[2]);
                }
                let pointer = [name.x + 1.0, name.y + 1.0];
                ui.input(crate::Input::PointerMove {
                    x: pointer[0],
                    y: pointer[1],
                    fine: false,
                });
                ui.prepare_scene();
                ui.update_at(ui.repaint_deadline().unwrap());
                assert_eq!(ui.scene.labels[ui.scene.tooltip_label.unwrap()].text, path);
                assert_eq!(ui.scene.shapes[0].rect, header);
            }
        }
    }
}
