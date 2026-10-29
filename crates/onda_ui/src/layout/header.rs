use crate::{
    icons::Icon,
    model::{FileDropTarget, RunUi},
    scene::{Palette, Rect, Scene, Target, TextStyle},
    theme::{blend, CONTROL_RADIUS, SECTION_RADIUS},
};
use serde_json::json;

fn icon_button(
    ui: &RunUi,
    s: &mut Scene,
    p: Palette,
    rect: [f32; 4],
    (icon, description): (Icon, &'static str),
    target: Target,
    active: bool,
) {
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
pub(super) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, y, w, _] = rect;
    let mut actions = Vec::new();
    if ui.state["supportsDeviceSelection"] == true
        || ui.state["supportsMidiInputSelection"] == true
        || ui.state["supportsRunSettings"] == true
    {
        actions.push(((Icon::Settings, "Settings"), Target::Devices));
    }
    if ui.state["supportsProjectExport"] == true && ui.state["canExportProject"] == true {
        actions.push((
            (Icon::Save, "Save as Project"),
            Target::Command(json!({"type":"saveProjectAs"})),
        ));
    }
    if ui.state["supportsSourceSelection"] == true {
        actions.push((
            (Icon::Unload, "Unload"),
            Target::Command(json!({"type":"unload"})),
        ));
    }
    let transport = ui.state["supportsTransport"] != false;
    let action_width = (actions.len() as f32 * 40.0 - 8.0).max(0.0);
    let controls_width = if transport { 72.0 } else { 0.0 }
        + action_width
        + if transport && !actions.is_empty() {
            25.0
        } else {
            0.0
        };
    let compact = w - 100.0 - controls_width < 120.0;
    let height = if compact { 118.0 } else { 72.0 };
    s.panel([x, y, w, height], p.panel, p.border, SECTION_RADIUS);
    let separator_x = x + 62.0;
    let brand_left = x + 1.0;
    let brand_width = separator_x - brand_left;
    let brand_center = brand_left + brand_width * 0.5;
    let brand_top = y + 12.0;
    s.logo([brand_center - 16.0, brand_top, 32.0, 32.0], p.accent);
    s.styled(
        ui.state["ondaVersion"]
            .as_str()
            .unwrap_or(env!("CARGO_PKG_VERSION")),
        [brand_left, brand_top + 36.0, brand_width, 14.0],
        10.0,
        p.muted,
        TextStyle {
            mono: true,
            centered: true,
            ..Default::default()
        },
    );
    if !compact {
        s.shape([separator_x, y + 20.0, 1.0, 32.0], p.border, 0.0);
    }
    let text_width = if compact {
        w - 28.0 - if transport { 84.0 } else { 0.0 }
    } else {
        w - 100.0 - controls_width
    }
    .max(1.0);
    let path = ui.state["path"].as_str().unwrap_or("Onda");
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let source_x = x + if compact { 14.0 } else { 80.0 };
    let source_y = y + if compact { 64.0 } else { 13.0 };
    s.tooltip([source_x, source_y, text_width, 24.0], path.into());
    s.styled(
        name,
        [source_x, source_y, text_width, 24.0],
        15.0,
        p.ink,
        TextStyle {
            mono: true,
            bold: true,
            ..Default::default()
        },
    );
    let status = ui.state["status"]
        .as_str()
        .unwrap_or(if ui.state["running"] == true {
            "Running"
        } else {
            "Stopped"
        });
    // Hosts own the status line; native states supply rate/block metadata separately.
    let details = if status.contains('·') || status.contains('—') {
        status.to_owned()
    } else {
        let mut details = status.to_owned();
        if let Some(rate) = ui.state["sampleRateHz"].as_f64() {
            details.push_str(&format!(" — {} kHz", rate / 1000.0));
        }
        if let Some(block) = ui.state["blockSamples"].as_u64() {
            details.push_str(&format!(" · {block} samples"));
        }
        if ui.state["fastMath"] == true {
            details.push_str(" · Fast Math");
        }
        details
    };
    s.tooltip(
        [source_x, source_y + 26.0, text_width, 20.0],
        details.clone(),
    );
    s.styled(
        details,
        [source_x, source_y + 26.0, text_width, 20.0],
        12.0,
        p.muted,
        TextStyle::default(),
    );
    let mut bx = x + w - if compact { 72.0 } else { controls_width } - 12.0;
    let by = y + if compact { 66.0 } else { 14.0 };
    if transport {
        let running = ui.state["running"] == true;
        let (label, icon, color) = if running {
            ("Stop", Icon::Stop, p.danger)
        } else {
            ("Start", Icon::Play, p.success)
        };
        let button = [bx, by, 72.0, 44.0];
        let hovered = s.hovered(ui, button);
        let fill = if running {
            blend(color, p.panel, if hovered { 0.2 } else { 0.1 })
        } else if hovered {
            blend(color, p.ink_color(), 0.85)
        } else {
            color
        };
        s.panel(
            button,
            fill,
            if hovered || ui.focused(&Target::Transport) || !running {
                color
            } else {
                blend(color, p.panel, 0.6)
            },
            CONTROL_RADIUS,
        );
        let ink = if running { color } else { p.background };
        let icon_width = if running { 12.0 } else { 10.0 };
        let label_width = crate::fonts::heading_width(label, 12.0);
        let left = bx + (72.0 - icon_width - 6.0 - label_width) * 0.5;
        s.icon(
            icon,
            [left + if running { 6.0 } else { 4.0 }, by + 22.0],
            ink,
        );
        let text = std::array::from_fn(|i| (ink[i] * 255.0).round() as u8);
        s.styled(
            label,
            [left + icon_width + 6.0, by, label_width + 2.0, 44.0],
            12.0,
            text,
            TextStyle {
                bold: true,
                ..Default::default()
            },
        );
        s.hit(button, Target::Transport);
        bx += 72.0;
    }
    if !actions.is_empty() {
        if compact {
            bx = x + w - action_width - 12.0;
        } else if transport {
            s.shape([bx + 12.0, by + 6.0, 1.0, 32.0], p.border, 0.0);
            bx += 25.0;
        }
        for (action, target) in actions {
            let active = matches!(target, Target::Devices) && ui.interaction.settings_open();
            icon_button(ui, s, p, [bx, y + 20.0, 32.0, 32.0], action, target, active);
            bx += 40.0;
        }
    }
    height
}

fn picker_settings_height(ui: &RunUi, width: f32) -> f32 {
    match (
        ui.state["supportsRunSettings"] == true,
        ui.state["supportsFastMath"] == true,
    ) {
        (true, true) if width < 420.0 => 318.0,
        (true, _) => 254.0,
        (false, true) => 248.0,
        (false, false) => 181.0,
    }
}
pub(super) fn picker_content_height(ui: &RunUi, width: f32) -> f32 {
    picker_settings_height(ui, width)
        + if ui.state["error"].as_str().is_some_and(|e| !e.is_empty()) {
            12.0 + ui.diagnostic_height
        } else {
            0.0
        }
}
pub(super) fn picker(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) {
    use crate::interaction::SelectId;
    let [x, y, w, h] = rect;
    let center = x + w * 0.5;
    let settings = ui.state["supportsRunSettings"] == true;
    let fast_math = ui.state["supportsFastMath"] == true;
    let narrow = w < 420.0;
    let content_height = picker_content_height(ui, w);
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
                top + picker_settings_height(ui, w) + 12.0,
                w - 24.0,
                ui.diagnostic_height,
            ],
            crate::theme::text(if ui.light { 0xb53333 } else { 0xff7e7e }),
        );
    }
    if settings || fast_math {
        let width = ((w - 42.0) * 0.5).clamp(1.0, 140.0);
        let group_width = if settings { width * 2.0 + 10.0 } else { 0.0 }
            + if fast_math && (!narrow || !settings) {
                80.0
            } else {
                0.0
            };
        let left = center - group_width * 0.5;
        for (index, label, id) in [
            (0, "SAMPLE RATE", SelectId::SampleRate),
            (1, "BLOCK SIZE", SelectId::BlockSize),
        ] {
            if !settings {
                continue;
            }
            let x = left + index as f32 * (width + 10.0);
            s.styled(
                label,
                [x, top + 194.0, width, 16.0],
                10.0,
                p.muted,
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
            s.select(ui, id, [x, top + 218.0, width, 36.0], p);
        }
        if fast_math {
            let x = if narrow || !settings {
                center - 35.0
            } else {
                left + group_width - 70.0
            };
            let y = top + if narrow && settings { 264.0 } else { 194.0 };
            s.styled(
                "FAST MATH",
                [x, y, 70.0, 16.0],
                10.0,
                p.muted,
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
            s.toggle(
                ui,
                Target::FastMath,
                [x + 23.0, y + 30.0, 24.0, 24.0],
                ui.state["fastMath"] == true,
                true,
                p,
            );
        }
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
                assert_eq!(header[3], 72.0);
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
                ui.prepare_scene_at(ui.repaint_deadline().unwrap());
                assert_eq!(ui.scene.labels[ui.scene.tooltip_label.unwrap()].text, path);
                assert_eq!(ui.scene.shapes[0].rect, header);
            }
        }
    }
}
