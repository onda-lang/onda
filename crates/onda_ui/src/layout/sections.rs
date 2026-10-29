use crate::{
    controls,
    editing::{Field, Setting},
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
            Section::Scope => ui.state["supportsScope"] != false,
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
        let tools_width = if bank {
            bank_tools_width(smoothing_supported(ui, input))
        } else if section == Section::Log {
            192.0
        } else {
            0.0
        };
        let title_inset = if reset.is_some() { 75.0 } else { 43.0 };
        let title_size = crate::fonts::heading_width(section.label(), 14.0) + 2.0;
        let wrap_tools = tools_width > 0.0 && w < title_size + title_inset + tools_width;
        let split_tools = bank && wrap_tools && tools_width > w - 2.0 * SECTION_INSET;
        let tools_y = y + if wrap_tools {
            SECTION_HEADER_HEIGHT
        } else {
            0.0
        };
        let header_height = SECTION_HEADER_HEIGHT
            + if wrap_tools {
                SECTION_HEADER_HEIGHT
            } else {
                0.0
            }
            + if split_tools {
                SECTION_HEADER_HEIGHT
            } else {
                0.0
            };
        let title_space = (w - if wrap_tools { 0.0 } else { tools_width } - title_inset).max(1.0);
        let title_width = if reset.is_some() {
            title_size.min(title_space)
        } else {
            title_space
        };
        s.icon(
            Icon::Chevron(open),
            [x + 14.0, y + 20.0],
            if ui.focused(&Target::Section(section)) {
                p.accent
            } else {
                p.ink_color()
            },
        );
        s.styled(
            section.label(),
            [x + 26.0, y + 6.0, title_width, 28.0],
            14.0,
            p.ink,
            TextStyle {
                bold: true,
                ..Default::default()
            },
        );
        s.tooltip(
            [x + 26.0, y + 6.0, title_width, 28.0],
            section.label().into(),
        );
        s.hit(
            [
                x + 6.0,
                y + 4.0,
                if reset.is_some() {
                    title_width + 20.0
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
        if bank {
            bank_toolbar(
                ui,
                s,
                p,
                [x, tools_y, w, header_height - (tools_y - y)],
                section == Section::InputControls,
            );
        }
        if section == Section::Log {
            s.button(
                ui,
                if ui.presentation.log_follow {
                    "Following"
                } else {
                    "Follow"
                },
                [x + w - SECTION_INSET - 192.0, tools_y + 7.0, 76.0, 26.0],
                Target::Follow,
                p,
            );
            s.button(
                ui,
                "Copy",
                [x + w - SECTION_INSET - 110.0, tools_y + 7.0, 48.0, 26.0],
                Target::CopyLog,
                p,
            );
            s.button(
                ui,
                "Clear",
                [x + w - SECTION_INSET - 56.0, tools_y + 7.0, 56.0, 26.0],
                Target::Command(json!({"type":"clearLog"})),
                p,
            );
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
                Section::Scope => scope(ui, s, p, body),
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
        if section == Section::Scope {
            s.scope_shape_end = s.shapes.len();
        }
        y += height + GAP;
    }
    y
}
fn scope(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4]) -> f32 {
    let height = if ui.size[1] <= 720.0 { 96.0 } else { 160.0 };
    let plot = [rect[0], rect[1], rect[2], height];
    s.shape(plot, p.scope, 9.0);
    s.scope_rect = plot;
    s.scope_clip = crate::scene::Rect(plot)
        .intersect(crate::scene::Rect(s.clip))
        .0;
    let channels = ui
        .scope
        .channels()
        .max(ui.state["outputChannels"].as_u64().unwrap_or(1) as usize)
        .clamp(1, 64);
    let grid = crate::theme::blend(p.accent, p.scope, 0.14);
    for channel in 0..channels {
        let y = plot[1] + height / channels as f32 * (channel as f32 + 0.5);
        s.shape([plot[0], y, plot[2], 1.0], grid, 0.0);
    }
    height
}
const SMOOTHING_WIDTH: f32 = 112.0;
fn smoothing_supported(ui: &RunUi, input: bool) -> bool {
    ui.state[if input {
        "supportsInputControlSmoothing"
    } else {
        "supportsParamSmoothing"
    }] == true
}
fn bank_tools_width(smoothing: bool) -> f32 {
    if smoothing {
        SMOOTHING_WIDTH + 6.0 + 110.0
    } else {
        110.0
    }
}
fn bank_toolbar(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4], input: bool) {
    let [x, y, w, h] = rect;
    let mut by = y + 7.0;
    let split = h > SECTION_HEADER_HEIGHT;
    let supported = smoothing_supported(ui, input);
    let setting = if input {
        Setting::InputSmoothing
    } else {
        Setting::ParamSmoothing
    };
    let tools_width = bank_tools_width(supported);
    let mut bx = x + w - if split { SMOOTHING_WIDTH } else { tools_width } - SECTION_INSET;
    if supported {
        s.field(
            ui,
            Field::Setting(setting),
            [bx, by, SMOOTHING_WIDTH, 26.0],
            format!(
                "Smooth {:.0} ms",
                ui.field_value(&Field::Setting(setting))
                    .as_f64()
                    .unwrap_or(0.0)
            ),
            p,
        );
        bx += SMOOTHING_WIDTH + 6.0;
    }
    if split {
        bx = x + w - 110.0 - SECTION_INSET;
        by += SECTION_HEADER_HEIGHT;
    }
    let knobs = ui.bank_knobs(input);
    s.panel([bx, by, 110.0, 26.0], p.field, p.border, 5.0);
    bx += 2.0;
    for (label, selected) in [("Sliders", false), ("Knobs", true)] {
        let b = [bx, by + 2.0, 52.0, 22.0];
        let target = Target::Layout {
            input,
            knobs: selected,
        };
        let hover = s.hovered(ui, b) || ui.focused(&target);
        let fill = if knobs == selected {
            p.accent_soft
        } else {
            p.field
        };
        if hover {
            s.panel(b, fill, p.accent, 3.0);
        } else {
            s.shape(b, fill, 3.0);
        }
        s.styled(
            label,
            b,
            12.0,
            if knobs == selected || hover {
                p.ink
            } else {
                p.muted
            },
            TextStyle {
                centered: true,
                ..Default::default()
            },
        );
        s.hit(b, target);
        bx += 54.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Rect;

    #[test]
    fn bank_headers_keep_headings_readable_and_actions_separate() {
        for width in [
            240.0, 320.0, 360.0, 400.0, 420.0, 428.0, 440.0, 500.0, 720.0,
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
                        assert_eq!(hits.len(), if smoothing { 5 } else { 4 });
                        let label = ui
                            .scene
                            .labels
                            .iter()
                            .find(|l| l.text == section.label())
                            .unwrap();
                        assert!(
                            label.width >= crate::fonts::heading_width(section.label(), 14.0),
                            "clipped heading at viewport {width}, smoothing {smoothing}"
                        );
                        for hit in &hits {
                            assert!(
                                hit.rect.0[0] >= 12.0
                                    && hit.rect.0[0] + hit.rect.0[2] <= width - 16.0,
                                "action outside the section at {width}: {:?}",
                                hit.target
                            );
                        }
                        for (i, a) in hits.iter().enumerate() {
                            for b in &hits[i + 1..] {
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
}
