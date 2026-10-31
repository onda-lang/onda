use crate::{
    controls,
    icons::Icon,
    model::RunUi,
    panels,
    presentation::Section,
    scene::{Palette, Scene, Target, TextStyle},
    theme::{GAP, SECTION_HEADER_HEIGHT, SECTION_INSET, SECTION_RADIUS},
};
use serde_json::json;

pub(super) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let [x, mut y, w, _] = rect;
    s.scope_shape_end = s.shapes.len();
    for section in [
        Section::Scope,
        Section::Log,
        Section::Buffers,
        Section::Events,
        Section::InputControls,
        Section::Params,
    ] {
        let present = match section {
            Section::Scope => ui.presentation.scope_pinned && ui.scope_available(),
            Section::Log => ui.log_present(),
            Section::Buffers => ui.state["buffers"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
            Section::Events => !ui.events.is_empty(),
            Section::InputControls => ui.has_controls(true),
            Section::Params => ui.has_controls(false),
            Section::Midi => false,
        };
        if !present {
            continue;
        }
        let open = ui.presentation.open(section);
        let start = s.shapes.len();
        s.panel([x, y, w, 0.0], p.panel, p.border, SECTION_RADIUS);
        let bank = matches!(section, Section::Params | Section::InputControls);
        let reset = match section {
            Section::Events => Some(("resetEventArguments", "Reset event arguments")),
            Section::Params => Some(("resetParams", "Reset parameters")),
            Section::InputControls => Some(("resetInputControls", "Reset input controls")),
            _ => None,
        };
        let input = section == Section::InputControls;
        let bank_header = bank.then(|| super::bank::Header::new(ui, input, w));
        let reset = reset.filter(|_| bank_header.as_ref().is_none_or(|h| h.reset_inline));
        let title = bank_header.as_ref().map_or(section.label(), |h| h.title);
        let font_size = bank_header.as_ref().map_or(14.0, |h| h.title_size);
        let tools_width = if let Some(header) = &bank_header {
            header.tools_width
        } else if section == Section::Log {
            panels::log::header_width(w)
        } else if section == Section::Scope {
            28.0
        } else {
            0.0
        };
        let title_x: f32 = if bank && w < 128.0 { 14.0 } else { 26.0 };
        let title_inset = if reset.is_some() {
            75.0
        } else {
            title_x + if bank && w < 128.0 { 9.0 } else { 17.0 }
        };
        let title_size = crate::fonts::heading_width(title, font_size) + 2.0;
        let header_height = SECTION_HEADER_HEIGHT;
        let title_space = (w - tools_width - title_inset).max(1.0);
        let title_width = if reset.is_some() {
            title_size.min(title_space)
        } else {
            title_space
        };
        s.icon(
            Icon::Chevron(open),
            [x + (title_x - 12.0).max(6.0), y + 20.0],
            if ui.focused(&Target::Section(section)) {
                p.accent
            } else {
                p.ink_color()
            },
        );
        s.styled(
            title,
            [x + title_x, y + 6.0, title_width, 28.0],
            font_size,
            p.ink,
            TextStyle {
                bold: true,
                ..Default::default()
            },
        );
        s.text_tooltip(
            [x + title_x, y + 6.0, title_width, 28.0],
            section.label().into(),
        );
        s.hit(
            [
                x + 6.0,
                y + 4.0,
                if tools_width > 0.0 {
                    (w - tools_width - 15.0).max(0.0)
                } else {
                    w - 12.0
                },
                30.0,
            ],
            Target::Section(section),
        );
        if let Some((command, tooltip)) = reset {
            let button = [x + 32.0 + title_width, y + 7.0, 26.0, 26.0];
            s.button(ui, "", button, Target::Command(json!({"type":command})), p);
            s.icon(
                Icon::Reset,
                [button[0] + 13.0, button[1] + 13.0],
                p.ink_color(),
            );
            s.tooltip(button, tooltip.into());
        }
        if section == Section::Scope {
            super::scope::pin(ui, s, p, [x + w - 37.0, y + 6.0, 28.0, 28.0]);
        }
        if let Some(header) = &bank_header {
            header.build(ui, s, p, [x, y, w, SECTION_HEADER_HEIGHT], input);
        }
        if section == Section::Log {
            panels::log::header(ui, s, p, [x, y, w, header_height]);
        }
        let body = [
            x + SECTION_INSET,
            y + header_height,
            w - 2.0 * SECTION_INSET,
            0.0,
        ];
        let body_height = if !open {
            0.0
        } else {
            match section {
                Section::Scope => super::scope::plot(ui, s, p, [body[0], body[1], body[2], 194.0]),
                Section::Log => panels::log(ui, s, p, body),
                Section::Buffers => panels::buffers(ui, s, p, body),
                Section::Events => panels::events(ui, s, p, body),
                Section::InputControls | Section::Params => {
                    let input = section == Section::InputControls;
                    controls::build(ui, s, p, body, input)
                }
                Section::Midi => 0.0,
            }
        };
        let height = header_height
            + if open {
                body_height + SECTION_INSET
            } else {
                0.0
            };
        s.shapes[start].rect[3] = height;
        s.shapes[start + 1].rect[3] = (height - 2.0).max(0.0);
        if section == Section::Scope && !open {
            s.scope_shape_end = s.shapes.len();
        }
        y += height + GAP;
    }
    y
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        editing::{Field, Setting},
        scene::Rect,
    };

    #[test]
    fn bank_headers_keep_headings_readable_and_actions_separate() {
        for width in [
            128.0, 144.0, 160.0, 195.0, 240.0, 320.0, 360.0, 400.0, 420.0, 428.0, 440.0, 500.0,
            720.0,
        ] {
            for smoothing in [false, true] {
                let mut ui = RunUi::default();
                ui.resize(width, 840.0, 1.0);
                ui.message(json!({"type":"state","state":{
                    "path":"banks.onda","supportsScope":false,"supportsTransport":false,
                    "supportsParamSmoothing":smoothing,"supportsInputControlSmoothing":smoothing,
                    "params":[{"name":"gain","type":"f64","value":0.5}],
                    "inputControls":[{"name":"input","type":"f64","value":0.5}]
                }}));
                for open in [true, false] {
                    ui.presentation.sections.fill(open);
                    ui.invalidate();
                    ui.prepare_scene();
                    for input in [false, true] {
                        let section = if input {
                            Section::InputControls
                        } else {
                            Section::Params
                        };
                        let reset = if input {
                            "resetInputControls"
                        } else {
                            "resetParams"
                        };
                        let setting = if input {
                            Setting::InputSmoothing
                        } else {
                            Setting::ParamSmoothing
                        };
                        let targets = [
                            Target::Section(section),
                            Target::ToolbarMenu(crate::interaction::ToolbarMenu::Bank(input)),
                            Target::Command(json!({"type":reset})),
                            Target::Layout {
                                input,
                                knobs: false,
                            },
                            Target::Layout { input, knobs: true },
                            Target::Field(Field::Setting(setting)),
                        ];
                        let hits: Vec<_> = ui
                            .scene
                            .hits
                            .iter()
                            .filter(|hit| targets.contains(&hit.target))
                            .collect();
                        let compact_menu = hits.iter().any(|h| {
                            h.target
                                == Target::ToolbarMenu(crate::interaction::ToolbarMenu::Bank(input))
                        });
                        let inline_reset = hits
                            .iter()
                            .any(|h| h.target == Target::Command(json!({"type":reset})));
                        assert_eq!(
                            hits.len(),
                            if compact_menu {
                                2 + usize::from(inline_reset)
                            } else if smoothing {
                                5
                            } else {
                                4
                            }
                        );
                        let label = ui
                            .scene
                            .labels
                            .iter()
                            .find(|l| l.text == section.label() || (input && l.text == "Inputs"))
                            .unwrap();
                        assert!(
                            label.width >= crate::fonts::heading_width(&label.text, label.size),
                            "clipped heading at viewport {width}, smoothing {smoothing}: {} needs {}, has {}",
                            label.text,
                            crate::fonts::heading_width(&label.text, label.size),
                            label.width,
                        );
                        let section_y = hits
                            .iter()
                            .find(|h| h.target == Target::Section(section))
                            .unwrap()
                            .rect
                            .0[1];
                        for hit in &hits {
                            assert!(
                                (hit.rect.0[1] - section_y).abs() <= 5.0,
                                "bank toolbar wrapped at {width}"
                            );
                            assert!(
                                hit.rect.0[0] >= 12.0
                                    && hit.rect.0[0] + hit.rect.0[2] <= width - 16.0,
                                "action outside the section at {width}: {:?}",
                                hit.target
                            );
                        }
                        let actions: Vec<_> = hits
                            .iter()
                            .filter(|hit| !matches!(hit.target, Target::Section(_)))
                            .collect();
                        for (i, a) in actions.iter().enumerate() {
                            let [x, y, w, h] = a.rect.0;
                            assert_eq!(ui.target(x + w * 0.5, y + h * 0.5), Some(a.target.clone()));
                            for b in &actions[i + 1..] {
                                let Rect(r) = a.rect.intersect(b.rect);
                                assert!(
                                    r[2] == 0.0 || r[3] == 0.0,
                                    "overlap at viewport {width}, smoothing {smoothing}, open {open}: {:?} {:?}",
                                    a.target,
                                    b.target
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn header_middle_collapses_sections_and_reset_keeps_its_own_action() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"headers.onda","supportsScope":false,
            "supportsParamSmoothing":true,"supportsInputControlSmoothing":true,
            "params":[{"name":"gain","type":"f64","value":0.5}],
            "inputControls":[{"name":"wet","type":"f64","value":0.5}],
            "events":[{"name":"fire","args":[]}]
        }}));
        for (section, command) in [
            (Section::Params, "resetParams"),
            (Section::InputControls, "resetInputControls"),
            (Section::Events, "resetEventArguments"),
        ] {
            ui.prepare_scene();
            let reset = Target::Command(json!({"type":command}));
            let r = ui
                .scene
                .hits
                .iter()
                .find(|hit| hit.target == reset)
                .unwrap()
                .rect
                .0;
            let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
            assert_eq!(ui.target(x, y), Some(reset));
            ui.input(crate::Input::PointerDown { x, y, clicks: 1 });
            ui.input(crate::Input::PointerUp);
            assert!(ui.presentation.open(section));
            assert!(ui.take_commands().iter().any(|c| c["type"] == command));

            ui.prepare_scene();
            let r = ui
                .scene
                .hits
                .iter()
                .find(|hit| hit.target == Target::Section(section))
                .unwrap()
                .rect
                .0;
            let (x, y) = (r[0] + r[2] * 0.75, r[1] + r[3] * 0.5);
            assert_eq!(ui.target(x, y), Some(Target::Section(section)));
            ui.input(crate::Input::PointerMove { x, y, fine: false });
            assert_eq!(ui.cursor(), "pointer");
            ui.input(crate::Input::PointerDown { x, y, clicks: 1 });
            ui.input(crate::Input::PointerUp);
            assert!(!ui.presentation.open(section));
            ui.input(crate::Input::PointerDown { x, y, clicks: 1 });
            ui.input(crate::Input::PointerUp);
            assert!(ui.presentation.open(section));
        }
    }
}
