use crate::{
    editing::Field,
    model::RunUi,
    scene::{Palette, Scene, Target, TextStyle},
    theme::{blend, CARD_RADIUS},
};

pub(crate) const SLIDER_INSET: f32 = 11.0;

mod layout;
pub(crate) use layout::Layout;
use layout::{field_rect, Item};

pub(crate) fn build(ui: &RunUi, s: &mut Scene, p: Palette, rect: [f32; 4], input: bool) -> f32 {
    let knobs = ui.bank_knobs(input);
    let mut cached = ui.control_layouts[input as usize].borrow_mut();
    if cached
        .as_ref()
        .is_none_or(|layout| !layout.matches(ui, rect[2], input))
    {
        *cached = Some(Layout::new(ui, rect[2], input));
    }
    let layout = cached.as_ref().unwrap();
    let row = layout.row;
    let card_width = layout.card_width;
    let top = s.clip[1] - rect[1];
    let focus_start = s.focus_targets.len();
    for item in layout.visible(top, top + s.clip[3]) {
        let (index, col, offset) = match item {
            Item::Group { name, y, folded } => {
                let y = rect[1] + *y;
                if s.visible(y, 28.0) {
                    let heading = [rect[0] + 18.0, y, rect[2] - 18.0, 24.0];
                    s.icon(
                        crate::icons::Icon::Chevron(!folded),
                        [rect[0] + 7.0, y + 12.0],
                        p.ink_color(),
                    );
                    s.tooltip(heading, name.clone());
                    s.styled(
                        name,
                        heading,
                        13.0,
                        p.ink,
                        TextStyle {
                            mono: true,
                            bold: true,
                            ..Default::default()
                        },
                    );
                }
                s.hit(
                    [rect[0], y, rect[2], 26.0],
                    Target::Fold(format!("bank/{input}/{name}")),
                );
                continue;
            }
            Item::Control { index, col, y } => (*index, *col, *y),
        };
        let c = &ui.controls[index];
        let x = rect[0] + col as f32 * (card_width + 8.0);
        let y = rect[1] + offset;
        let field = field_rect(c, knobs, [x, y, card_width, row]);
        s.panel(
            [x, y, card_width, row],
            p.card,
            if input {
                p.border
            } else {
                blend(p.border, p.card, 0.55)
            },
            CARD_RADIUS,
        );
        let name = c.display_name();
        let ty = c.metadata["type"].as_str().unwrap_or("");
        let head_width = card_width - 18.0;
        let centered_heading = knobs || c.boolean();
        let heading_size = if centered_heading && ui.size[0] <= 400.0 {
            12.0
        } else {
            14.0
        };
        let heading_y = y + if centered_heading { 9.0 } else { 11.0 };
        let name_width = (name.chars().count() as f32 * heading_size * 0.6)
            .min((head_width - ty.len() as f32 * 7.2 - 6.0).max(12.0));
        let value_width = 104.0_f32.min(card_width - 18.0);
        let title_width = if centered_heading {
            head_width
        } else {
            (head_width - value_width - 8.0).max(20.0)
        };
        let start = if centered_heading {
            x + (card_width - (name_width + 6.0 + ty.len() as f32 * 7.2).min(title_width)) * 0.5
        } else {
            x + 10.0
        };
        s.tooltip(
            [
                if centered_heading { x + 9.0 } else { start },
                heading_y,
                title_width,
                20.0,
            ],
            format!("{name} ({ty})"),
        );
        s.styled(
            name,
            [start, heading_y, name_width.min(title_width), 20.0],
            heading_size,
            p.ink,
            TextStyle {
                mono: true,
                bold: true,
                ..Default::default()
            },
        );
        if name_width + 6.0 + ty.len() as f32 * 7.2 <= title_width {
            s.styled(
                ty,
                [
                    start + name_width + 6.0,
                    heading_y,
                    ty.len() as f32 * 7.2,
                    20.0,
                ],
                12.0,
                p.muted,
                TextStyle {
                    mono: true,
                    ..Default::default()
                },
            );
        }
        if c.boolean() {
            s.checkbox(ui, Field::Control(index), field, c.value == true, p);
            continue;
        }
        let n = c.normalized() as f32;
        if c.metadata["enabled"] == false {
            s.styled(
                "External input",
                [
                    x + 9.0,
                    y + if knobs { 58.0 } else { 43.0 },
                    card_width - 18.0,
                    24.0,
                ],
                12.0,
                p.muted,
                TextStyle {
                    centered: true,
                    ..Default::default()
                },
            );
        }
        if knobs {
            let dial = [x + (card_width - 54.0) * 0.5, y + 38.0, 54.0, 54.0];
            if c.domain.is_some() && c.enabled() {
                let target = [dial[0] - 5.0, dial[1] - 5.0, 64.0, 64.0];
                let center = [dial[0] + 27.0, dial[1] + 27.0];
                let hot = s.hovered(ui, target) || ui.interaction.dragging_control(index);
                if hot {
                    s.ring(center, 32.0, 1.0, p.accent);
                }
                s.shape(dial, p.field, 27.0);
                s.primitive(
                    dial,
                    blend(p.ink_color(), p.field, 0.14),
                    [3.0, 1.0, 1.0, 1.0],
                );
                s.primitive(dial, p.accent, [3.0, 1.0, n, 0.0]);
                let a = std::f32::consts::PI * 0.75 + n * std::f32::consts::PI * 1.5;
                s.line(
                    [
                        center[0] + a.cos() * (9.0 * 54.0 / 58.0),
                        center[1] + a.sin() * (9.0 * 54.0 / 58.0),
                    ],
                    [
                        center[0] + a.cos() * (20.0 * 54.0 / 58.0),
                        center[1] + a.sin() * (20.0 * 54.0 / 58.0),
                    ],
                    3.0,
                    if hot { p.accent_strong } else { p.ink_color() },
                );
                s.hit(target, Target::Control(index));
            }
            s.field(
                ui,
                Field::Control(index),
                field,
                ui.field_text(&Field::Control(index)),
                p,
            );
        } else {
            s.field(
                ui,
                Field::Control(index),
                field,
                ui.field_text(&Field::Control(index)),
                p,
            );
            s.labels.last_mut().unwrap().style.centered = false;
            if c.domain.is_none() || !c.enabled() {
                continue;
            }
            let slider = [x + 10.0, y + 44.0, card_width - 20.0, 22.0];
            let hot = s.hovered(ui, slider) || ui.interaction.dragging_control(index);
            s.panel(slider, p.field, if hot { p.accent } else { p.border }, 11.0);
            let track = [
                slider[0] + SLIDER_INSET,
                slider[1] + 9.0,
                (slider[2] - SLIDER_INSET * 2.0).max(1.0),
                4.0,
            ];
            s.shape(track, blend(p.ink_color(), p.field, 0.10), 2.0);
            s.primitive(
                [track[0], track[1], track[2] * n, 4.0],
                p.accent,
                [2.0, 6.0, 0.0, 0.0],
            );
            let mut shadow = p.panel;
            shadow[3] = 0.78;
            s.shape(
                [track[0] + track[2] * n - 9.0, slider[1] + 2.0, 18.0, 18.0],
                shadow,
                9.0,
            );
            s.shape(
                [track[0] + track[2] * n - 7.0, slider[1] + 4.0, 14.0, 14.0],
                p.ink_color(),
                7.0,
            );
            if c.enabled() {
                s.hit(slider, Target::Control(index));
            }
        }
    }
    s.focus_targets.truncate(focus_start);
    s.focus_targets.push(crate::interaction::FocusEntry::Group {
        targets: layout.focus.clone(),
        offset: [rect[0], rect[1]],
    });
    layout.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Input, Key};
    use serde_json::json;
    use std::sync::Arc;

    fn cached_focus(ui: &RunUi, input: bool) -> Arc<[crate::interaction::FocusTarget]> {
        ui.control_layouts[input as usize]
            .borrow()
            .as_ref()
            .unwrap()
            .focus
            .clone()
    }

    #[test]
    fn large_banks_share_layout_across_values_hover_scroll_and_offscreen_edits() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"large.onda","supportsScope":false,
            "params":(0..4096).map(|i|json!({"name":format!("p{i}"),"type":"f64","value":0.5})).collect::<Vec<_>>()
        }}));
        ui.prepare_scene();
        let cached = cached_focus(&ui, false);
        assert_eq!(cached.len(), 4096);
        for x in [0.0, 300.0, 0.0] {
            ui.input(Input::PointerMove {
                x,
                y: 200.0,
                fine: false,
            });
            ui.message(
                json!({"type":"parameterValues","state":{"params":[{"name":"p0","value":x}]}}),
            );
            ui.prepare_scene();
            assert!(Arc::ptr_eq(&cached, &cached_focus(&ui, false)));
            assert!(ui.scene.labels.len() < 100);
            assert!(ui.scene.focus_targets.len() < 20);
        }
        ui.interaction.focus_field(Field::Control(4094));
        ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        assert_eq!(
            ui.interaction.focus(),
            Some(&Target::Field(Field::Control(4095)))
        );
        assert!(ui.scroll > 100_000.0);
        ui.input(Input::Text {
            text: "0.25".into(),
        });
        ui.input(Input::Key {
            key: Key::Enter,
            shift: false,
        });
        ui.prepare_scene();
        assert_eq!(ui.controls[4095].value, 0.25);
        assert!(Arc::ptr_eq(&cached, &cached_focus(&ui, false)));
        ui.message(json!({"type":"state","state":{"params":ui.state["params"]}}));
        ui.prepare_scene();
        assert!(Arc::ptr_eq(&cached, &cached_focus(&ui, false)));
    }

    #[test]
    fn cached_layout_tracks_bank_indices_editability_folds_resize_and_layout() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{
            "path":"banks.onda","supportsScope":false,
            "inputControls":[{"name":"input","type":"f64","value":0}],
            "params":[{"name":"p0","type":"bool","value":false,"array":{"name":"p"}},
                {"name":"p1","type":"f64","value":0,"array":{"name":"p"}}]
        }}));
        ui.prepare_scene();
        let mut cached = cached_focus(&ui, false);
        for message in [
            json!({"type":"state","state":{"inputControls":[]}}),
            json!({"type":"state","state":{"paramLayout":"sliders"}}),
            json!({"type":"state","state":{"viewState":{"readyId":1,"folded":["bank/false/p"]}}}),
            json!({"type":"state","state":{"viewState":{"readyId":2,"reset":true}}}),
            json!({"type":"state","state":{"params":[{"name":"disabled","type":"f64","value":0,"enabled":false}]}}),
        ] {
            ui.message(message);
            ui.prepare_scene();
            let next = cached_focus(&ui, false);
            assert!(!Arc::ptr_eq(&cached, &next));
            cached = next;
        }
        assert!(
            cached.is_empty(),
            "disabled controls must leave the focus order"
        );
        ui.resize(360.0, 840.0, 1.0);
        ui.prepare_scene();
        assert!(!Arc::ptr_eq(&cached, &cached_focus(&ui, false)));
    }

    #[test]
    fn grouping_preserves_first_appearance_order_and_separates_banks() {
        let mut ui = RunUi::default();
        let control = |name: &str, array: Option<&str>| {
            json!({
                "name":name,"type":"f64","value":0,
                "array":array.map(|name| json!({"name":name,"length":2}))
            })
        };
        ui.message(json!({"type":"state","state":{
            "inputControls":[control("in",Some("a"))],
            "params":[control("a0",Some("a")),control("scalar",None),
                control("b0",Some("b")),control("a1",Some("a")),control("b1",Some("b"))]
        }}));
        let names = |ui: &RunUi, input| {
            Layout::new(ui, 600.0, input)
                .items
                .into_iter()
                .filter_map(|item| {
                    if let Item::Control { index, .. } = item {
                        Some(ui.controls[index].name().to_owned())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&ui, false), ["a0", "a1", "scalar", "b0", "b1"]);
        assert_eq!(names(&ui, true), ["in"]);
        ui.presentation.folded.insert("bank/false/a".into());
        assert_eq!(names(&ui, false), ["scalar", "b0", "b1"]);
        assert_eq!(names(&ui, true), ["in"]);
    }

    #[test]
    fn folded_groups_reserve_their_complete_hit_region() {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{"params":[
            {"name":"voice[0]","type":"f32","value":0,"array":{"name":"voice","length":1}}
        ]}}));
        ui.presentation.folded.insert("bank/false/voice".into());
        let mut scene = Scene {
            clip: [0.0, 0.0, 600.0, 840.0],
            ..Default::default()
        };
        let height = build(
            &ui,
            &mut scene,
            Palette::new(false),
            [0.0, 0.0, 600.0, 0.0],
            false,
        );
        for hit in scene.hits {
            assert!(
                hit.rect.0[1] + hit.rect.0[3] <= height,
                "a folded group must fit inside the body allocated by its parent"
            );
        }
    }
}
