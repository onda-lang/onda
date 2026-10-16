use eframe::egui;
use serde_json::{json, Value};

use super::{InputControlAction, InputControlSection};
use crate::{apply_run_theme, ParamLayout, RunTheme};

fn frame(
    ctx: &egui::Context,
    section: &mut InputControlSection,
    controls: &[Value],
    width: f32,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Vec<InputControlAction>) {
    let mut actions = Vec::new();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                actions = section.show(ui, controls);
            });
        },
    );
    (output, actions)
}

fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.galley.rect.translate(text.pos.to_vec2()).center())
            }
            _ => None,
        })
        .expect("rendered section text")
}

#[test]
fn empty_host_controls_hide_the_section() {
    let ctx = egui::Context::default();
    let mut section = InputControlSection::new(0.03);
    let (output, actions) = frame(&ctx, &mut section, &[], 720.0, vec![]);
    assert!(actions.is_empty());
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.job.text == "Input Controls")));
}

#[test]
fn boolean_controls_center_in_their_cards_and_commit_without_value_labels() {
    for layout in [ParamLayout::Sliders, ParamLayout::Knobs] {
        for enabled in [false, true] {
            let ctx = egui::Context::default();
            apply_run_theme(&ctx, &RunTheme::from_dark_mode(true));
            let controls = [json!({"name":"enabled", "type":"bool", "scalar":true,
                "value":false, "default":false, "enabled":enabled})];
            let render = |events| {
                let mut edits = Vec::new();
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(360.0, 300.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            edits = super::render_control_values(ui, &controls, layout);
                        });
                    },
                );
                (output, edits)
            };
            let _ = render(vec![]);
            let (output, edits) = render(vec![]);
            assert!(edits.is_empty());
            let rectangles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) => Some(rect.rect),
                    _ => None,
                })
                .collect();
            let card = rectangles
                .iter()
                .find(|rect| rect.height() >= 80.0 && rect.height() < 300.0)
                .expect("boolean card");
            let checkbox = rectangles
                .iter()
                .find(|rect| rect.size() == egui::Vec2::splat(24.0))
                .expect("bare checkbox");
            assert!(
                (checkbox.center().x - card.center().x).abs() < 1.0,
                "{layout:?}: {checkbox:?} in {card:?}"
            );
            assert!(
                checkbox.center().y > card.center().y && checkbox.bottom() <= card.bottom() - 8.0,
                "{layout:?}: the checkbox centers below the heading: {checkbox:?} in {card:?}"
            );
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if matches!(text.galley.job.text.as_str(), "On" | "Off"))));
            text_center(&output, "enabled");
            let pos = checkbox.center();
            let _ = render(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            let (_, edits) = render(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            assert_eq!(
                edits,
                if enabled {
                    vec![("enabled".into(), json!(true))]
                } else {
                    vec![]
                }
            );
        }
    }
}

#[test]
fn input_layout_actions_and_read_only_sources_work_in_both_layouts() {
    let controls = [json!({ "name": "cv", "label": "cutoff CV", "type": "f32",
        "scalar": true, "value": 0.5, "default": 0.25,
        "rangeMin": 0.0, "rangeMax": 1.0, "scale": "linear" })];
    for width in [240.0, 360.0, 720.0] {
        let ctx = egui::Context::default();
        apply_run_theme(&ctx, &RunTheme::from_dark_mode(true));
        let mut section = InputControlSection::new(0.03);
        let (output, _) = frame(&ctx, &mut section, &controls, width, vec![]);
        let position = text_center(&output, "Sliders");
        let click = [true, false]
            .into_iter()
            .flat_map(|pressed| {
                [
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            })
            .collect();
        let (_, actions) = frame(&ctx, &mut section, &controls, width, click);
        assert_eq!(section.layout, ParamLayout::Sliders);
        assert!(actions.contains(&InputControlAction::SetLayout(ParamLayout::Sliders)));
        assert_eq!(section.smoothing_seconds, 0.03);
        for layout in [ParamLayout::Knobs, ParamLayout::Sliders] {
            section.layout = layout;
            let mut external = controls[0].clone();
            external["enabled"] = json!(false);
            let (output, actions) = frame(&ctx, &mut section, &[external], width, vec![]);
            text_center(&output, "External input");
            assert!(actions.is_empty());
            let mut fixed = controls[0].clone();
            fixed["rangeMin"] = json!(2.0);
            fixed["rangeMax"] = json!(2.0);
            fixed["value"] = json!(2.0);
            let (_, actions) = frame(&ctx, &mut section, &[fixed], width, vec![]);
            assert!(actions.is_empty());
        }
    }
}

#[test]
fn external_input_editors_stay_inside_their_cards() {
    for dark in [false, true] {
        for width in [240.0, 360.0, 720.0] {
            for layout in [ParamLayout::Knobs, ParamLayout::Sliders] {
                for ranged in [false, true] {
                    let ctx = egui::Context::default();
                    apply_run_theme(&ctx, &RunTheme::from_dark_mode(dark));
                    let controls: Vec<_> = (0..6)
                        .map(|index| {
                            let mut control = json!({
                                "name": format!("cv_{index}"),
                                "label": "cutoff_control_with_a_long_name",
                                "type": "f32", "scalar": true, "unit": "Hz",
                                "value": 0.5, "enabled": index % 2 != 0,
                            });
                            if ranged {
                                control["rangeMin"] = json!(0.0);
                                control["rangeMax"] = json!(1.0);
                            }
                            control
                        })
                        .collect();
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 1000.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                assert!(
                                    super::render_control_values(ui, &controls, layout).is_empty()
                                );
                            });
                        },
                    );
                    let cards: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.rect.height() >= 80.0 && rect.rect.height() < 1000.0 =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(cards.len(), controls.len());
                    let numbers: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect) if rect.rect.height() == 26.0 => {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(numbers.len(), controls.len());
                    text_center(&output, "External input");
                    for ((card, number), control) in cards.iter().zip(numbers).zip(&controls) {
                        assert!(
                            card.shrink2(egui::vec2(0.0, 8.0)).contains_rect(number),
                            "{layout:?}, width={width}, dark={dark}, ranged={ranged}: \
                             number {number:?} overflows card {card:?}"
                        );
                        assert!(
                            card.left() >= 0.0 && card.right() <= width,
                            "card exceeds viewport"
                        );
                        if control["enabled"] == true {
                            assert_eq!(
                                card.height(),
                                match layout {
                                    ParamLayout::Knobs => 140.0,
                                    ParamLayout::Sliders => 80.0,
                                }
                            );
                        }
                    }
                    for pair in cards.windows(2) {
                        assert!(!pair[0].intersects(pair[1]), "cards overlap");
                    }
                }
            }
        }
    }
}
