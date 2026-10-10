use super::*;
use crate::number_input::{NumberInput, NumberValue};
use crate::{param_name, render_event_array_editor, render_param_grid};

#[test]
fn parameter_presses_preserve_host_updates_until_dragging() {
    for ty in ["f64", "i64"] {
        for ranged in [false, true] {
            for drag in [false, true] {
                let ctx = egui::Context::default();
                let spec = ParamControlSpec {
                    label: "value",
                    ty,
                    default: Some(20.0),
                    domain: ranged.then(|| {
                        ParamDomain::new(
                            ParamScalarType::F64,
                            0.0,
                            100.0,
                            ParamScale::Linear,
                            None,
                            None,
                            None,
                            None,
                        )
                        .unwrap()
                    }),
                };
                let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
                    render_param_number_input(ui, spec, 20.0, 6)
                });
                let start = rect.center();
                widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
                    render_param_number_input(ui, spec, 20.0, 6)
                });
                for (time, pixels, value) in [(0.2, 0.0, 60.0), (0.3, 2.0, 70.0)] {
                    let (_, outcome) = widget_frame(
                        &ctx,
                        time,
                        vec![egui::Event::PointerMoved(start + egui::vec2(pixels, 0.0))],
                        |ui| render_param_number_input(ui, spec, value, 6),
                    );
                    assert!(
                        matches!(outcome, ParamEditOutcome::None),
                        "{ty}, ranged={ranged}: a press must preserve the host value {value}"
                    );
                }
                if drag {
                    // The host changes again in the frame where dragging begins.
                    let end = start + egui::vec2(15.0, 0.0);
                    let (_, outcome) =
                        widget_frame(&ctx, 0.4, vec![egui::Event::PointerMoved(end)], |ui| {
                            render_param_number_input(ui, spec, 80.0, 6)
                        });
                    let expected = if ranged || ty == "i64" { 84.0 } else { 80.15 };
                    assert!(
                        matches!(outcome, ParamEditOutcome::Commit(next)
                            if next.as_f64() == Some(expected)),
                        "{ty}, ranged={ranged}: dragging must start from the latest host value"
                    );
                    let (_, outcome) = widget_frame(&ctx, 0.5, pointer_button(end, false), |ui| {
                        render_param_number_input(ui, spec, expected, 6)
                    });
                    assert!(matches!(outcome, ParamEditOutcome::None));
                } else {
                    let (_, outcome) = widget_frame(
                        &ctx,
                        0.4,
                        pointer_button(start + egui::vec2(2.0, 0.0), false),
                        |ui| render_param_number_input(ui, spec, 80.0, 6),
                    );
                    assert!(matches!(outcome, ParamEditOutcome::None));
                    let (_, outcome) = widget_frame(&ctx, 0.5, key_event(egui::Key::Enter), |ui| {
                        render_param_number_input(ui, spec, 80.0, 6)
                    });
                    assert!(matches!(outcome, ParamEditOutcome::None));
                }
            }
        }
    }
}

#[test]
fn smoothing_keeps_its_width_while_displaying_and_editing_large_values() {
    for initial in [0.0, 0.0125, 0.03, 300.0, 12_345.0, 1e50] {
        let ctx = egui::Context::default();
        let mut seconds = initial;
        for (time, events) in [
            (0.0, vec![]),
            (0.1, key_event(egui::Key::Tab)),
            (
                0.2,
                vec![egui::Event::Text("123456789012345678901234567890".into())],
            ),
            (0.3, key_event(egui::Key::Escape)),
            (0.4, vec![]),
        ] {
            let (rect, changed) = widget_frame(&ctx, time, events, |ui| {
                render_param_smoothing(ui, &mut seconds, initial)
            });
            assert_eq!(rect.width(), 112.0, "seconds={initial}, time={time}");
            assert!(!changed);
            assert_eq!(seconds, initial);
        }
    }
}

#[test]
fn number_editors_shorten_float_drafts_without_committing_rounding() {
    for (initial, text) in [
        (1023.095538565032, "1023.09554"),
        (8.0, "8"),
        (-0.0, "0"),
        (0.0000123456789, "1.23457e-5"),
        (-1.23456789e-8, "-1.23457e-8"),
        (1.23456789e20, "1.23457e20"),
        (f64::from_bits(1), "4.94066e-324"),
    ] {
        for ty in ["f32", "f64"] {
            for finish in [Some(egui::Key::Enter), Some(egui::Key::Escape), None] {
                let ctx = egui::Context::default();
                let spec = ParamControlSpec {
                    label: "value",
                    ty,
                    default: Some(initial),
                    domain: None,
                };
                widget_frame(&ctx, 0.0, vec![], |ui| {
                    render_param_number_input(ui, spec, initial, 6)
                });
                widget_frame(&ctx, 0.1, key_event(egui::Key::Tab), |ui| {
                    render_param_number_input(ui, spec, initial, 6)
                });
                let output = frame(&ctx, 0.2, 320.0, vec![], |ui| {
                    ui.scope(|ui| render_param_number_input(ui, spec, initial, 6));
                });
                number_position(&output, text);
                if finish.is_none() {
                    ctx.memory_mut(|memory| {
                        memory.surrender_focus(memory.focused().unwrap());
                    });
                }
                let (_, outcome) =
                    widget_frame(&ctx, 0.3, finish.map(key_event).unwrap_or_default(), |ui| {
                        render_param_number_input(ui, spec, initial, 6)
                    });
                assert!(matches!(outcome, ParamEditOutcome::None), "{ty}, {text}");
            }
        }
    }
}

#[test]
fn rounded_number_drafts_accept_full_precision_and_increment_the_exact_value() {
    let initial = 1023.095538565032;
    let spec = ParamControlSpec {
        label: "value",
        ty: "f64",
        default: Some(initial),
        domain: None,
    };
    for typed in [false, true] {
        let ctx = egui::Context::default();
        widget_frame(&ctx, 0.0, vec![], |ui| {
            render_param_number_input(ui, spec, initial, 6)
        });
        widget_frame(&ctx, 0.1, key_event(egui::Key::Tab), |ui| {
            render_param_number_input(ui, spec, initial, 6)
        });
        let events = if typed {
            vec![egui::Event::Text("1023.123456789".into())]
        } else {
            key_event(egui::Key::ArrowUp)
        };
        widget_frame(&ctx, 0.2, events, |ui| {
            render_param_number_input(ui, spec, initial, 6)
        });
        let (_, outcome) = widget_frame(&ctx, 0.3, key_event(egui::Key::Enter), |ui| {
            render_param_number_input(ui, spec, initial, 6)
        });
        let expected = if typed {
            1023.123456789
        } else {
            initial + spec.effective_step()
        };
        assert!(
            matches!(outcome, ParamEditOutcome::Commit(next) if next.as_f64() == Some(expected))
        );
    }
}

#[test]
fn knobs_apply_shift_to_each_movement_and_reverse_at_bounds() {
    for (scale, curve, step) in [
        (ParamScale::Log, None, None),
        (ParamScale::Linear, Some(-4.0), None),
        (ParamScale::Linear, Some(4.0), None),
        (ParamScale::Linear, None, Some(10.0)),
    ] {
        let ctx = egui::Context::default();
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            80.0,
            12_000.0,
            scale,
            curve,
            None,
            step,
            step.map(|_| 1192),
        )
        .unwrap();
        let spec = ParamControlSpec {
            label: "cutoff",
            ty: "f64",
            default: Some(80.0),
            domain: Some(domain),
        };
        let mut value = 80.0;
        let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
            crate::render_param_knob(ui, &mut value, 80.0, 12_000.0, spec)
        });
        let start = rect.center();
        widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
            crate::render_param_knob(ui, &mut value, 80.0, 12_000.0, spec)
        });
        for (index, (pixels, shift, normalized)) in [
            (25.0, true, 0.01),
            (50.0, false, 0.11),
            (50.0, true, 0.11),
            (75.0, true, 0.12),
            (3000.0, false, 1.0),
            (2997.5, true, 0.999),
            (-3000.0, false, 0.0),
            (-2997.5, true, 0.001),
        ]
        .into_iter()
        .enumerate()
        {
            widget_frame_with_modifiers(
                &ctx,
                0.2 + index as f64 * 0.1,
                vec![egui::Event::PointerMoved(start - egui::vec2(0.0, pixels))],
                egui::Modifiers {
                    shift,
                    ..Default::default()
                },
                |ui| crate::render_param_knob(ui, &mut value, 80.0, 12_000.0, spec),
            );
            let expected = domain.normalized_to_plain(normalized);
            assert!((value / expected - 1.0).abs() < 1e-12,
                "{scale:?}, curve={curve:?}, step={step:?}, Shift={shift}, {pixels}: {value} != {expected}");
        }
        let before_release = value;
        widget_frame_with_modifiers(
            &ctx,
            1.1,
            pointer_button(start + egui::vec2(0.0, 2997.5), false),
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
            |ui| crate::render_param_knob(ui, &mut value, 80.0, 12_000.0, spec),
        );
        assert_eq!(value, before_release);
    }
}

#[test]
fn logarithmic_parameter_numbers_drag_by_equal_ratios() {
    // The poly_saw cutoff domain: 80 Hz to 12 kHz.
    for scalar in [ParamScalarType::F32, ParamScalarType::F64] {
        let domain = ParamDomain::new(
            scalar,
            80.0,
            12_000.0,
            ParamScale::Log,
            None,
            Some("Hz"),
            None,
            None,
        )
        .unwrap();
        let spec = ParamControlSpec {
            label: "cutoff",
            ty: if scalar == ParamScalarType::F32 {
                "f32"
            } else {
                "f64"
            },
            default: Some(800.0),
            domain: Some(domain),
        };
        for initial in [80.0, 800.0, 4_000.0] {
            for shift in [false, true] {
                let ctx = egui::Context::default();
                let mut value = initial;
                let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
                    render_param_number_input(ui, spec, value, 6)
                });
                let start = rect.center();
                widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
                    render_param_number_input(ui, spec, value, 6)
                });
                let modifiers = egui::Modifiers {
                    shift,
                    ..Default::default()
                };
                for (index, pixels) in [37.5, 75.0].into_iter().enumerate() {
                    let position = start + egui::vec2(pixels * if shift { 10.0 } else { 1.0 }, 0.0);
                    let (_, outcome) = widget_frame_with_modifiers(
                        &ctx,
                        0.2 + index as f64 * 0.1,
                        vec![egui::Event::PointerMoved(position)],
                        modifiers,
                        |ui| render_param_number_input(ui, spec, value, 6),
                    );
                    if let ParamEditOutcome::Commit(next) = outcome {
                        value = next.as_f64().unwrap();
                    }
                    let expected = initial * 150.0_f64.powf(f64::from(pixels) / 375.0);
                    assert!(
                        (value / expected - 1.0).abs() < 1.0e-12,
                        "{scalar:?}, initial {initial}, Shift={shift}: {value} != {expected}"
                    );
                    if index == 1 {
                        let (_, outcome) = widget_frame_with_modifiers(
                            &ctx,
                            0.4,
                            pointer_button(position, false),
                            modifiers,
                            |ui| render_param_number_input(ui, spec, value, 6),
                        );
                        if let ParamEditOutcome::Commit(next) = outcome {
                            value = next.as_f64().unwrap();
                        }
                        assert!((value / expected - 1.0).abs() < 1.0e-12);
                    }
                }
            }
        }
    }
}

#[test]
fn scaled_parameter_numbers_follow_knobs_and_reverse_at_bounds() {
    for (scale, curve) in [
        (ParamScale::Log, None),
        (ParamScale::Linear, Some(-4.0)),
        (ParamScale::Linear, Some(4.0)),
    ] {
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            80.0,
            12_000.0,
            scale,
            curve,
            None,
            None,
            None,
        )
        .unwrap();
        let spec = ParamControlSpec {
            label: "cutoff",
            ty: "f64",
            default: Some(80.0),
            domain: Some(domain),
        };
        let ctx = egui::Context::default();
        let mut value = 80.0;
        let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
            render_param_number_input(ui, spec, value, 6)
        });
        let start = rect.center();
        widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
            render_param_number_input(ui, spec, value, 6)
        });
        let mut knob = KnobDragState::new(domain, value);
        let mut previous_pixels = 0.0;
        for (index, pixels) in [187.5, 375.0, 562.5, 525.0, -150.0, -112.5]
            .into_iter()
            .enumerate()
        {
            let (_, outcome) = widget_frame(
                &ctx,
                0.2 + index as f64 * 0.1,
                vec![egui::Event::PointerMoved(start + egui::vec2(pixels, 0.0))],
                |ui| render_param_number_input(ui, spec, value, 6),
            );
            if let ParamEditOutcome::Commit(next) = outcome {
                value = next.as_f64().unwrap();
            }
            let expected = knob.drag(domain, f64::from(previous_pixels - pixels) * 250.0 / 375.0);
            assert!(
                (value / expected - 1.0).abs() < 1.0e-12,
                "{scale:?}, curve {curve:?}, {pixels} points: {value} != {expected}"
            );
            previous_pixels = pixels;
        }
    }
}

#[test]
fn logarithmic_parameter_numbers_commit_typed_plain_values() {
    let ctx = egui::Context::default();
    let spec = ParamControlSpec {
        label: "cutoff",
        ty: "f64",
        default: Some(800.0),
        domain: ParamDomain::new(
            ParamScalarType::F64,
            80.0,
            12_000.0,
            ParamScale::Log,
            None,
            Some("Hz"),
            None,
            None,
        ),
    };
    for (time, events) in [
        (0.0, vec![]),
        (0.1, key_event(egui::Key::Tab)),
        (0.2, vec![egui::Event::Text("440".into())]),
    ] {
        let (_, outcome) = widget_frame(&ctx, time, events, |ui| {
            render_param_number_input(ui, spec, 800.0, 6)
        });
        assert!(matches!(outcome, ParamEditOutcome::None));
    }
    let (_, outcome) = widget_frame(&ctx, 0.3, key_event(egui::Key::Enter), |ui| {
        render_param_number_input(ui, spec, 800.0, 6)
    });
    assert!(matches!(outcome, ParamEditOutcome::Commit(next) if next == serde_json::json!(440.0)));
}

#[test]
fn parameter_number_domain_changes_cancel_active_drags() {
    let domain = |minimum, maximum| {
        ParamDomain::new(
            ParamScalarType::F64,
            minimum,
            maximum,
            ParamScale::Linear,
            None,
            None,
            None,
            None,
        )
        .expect("valid range")
    };
    let curved_domain = ParamDomain::new(
        ParamScalarType::F64,
        0.0,
        1.0,
        ParamScale::Linear,
        Some(4.0),
        None,
        None,
        None,
    )
    .unwrap();
    for next_domain in [
        Some(domain(0.0, 100.0)),
        Some(domain(-1.0, 1.0)),
        Some(curved_domain),
        None,
    ] {
        let ctx = egui::Context::default();
        let spec = ParamControlSpec {
            label: "value",
            ty: "f64",
            default: Some(0.5),
            domain: Some(domain(0.0, 1.0)),
        };
        let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
            render_param_number_input(ui, spec, 0.5, 6)
        });
        let start = rect.center();
        widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
            render_param_number_input(ui, spec, 0.5, 6)
        });
        let (_, outcome) = widget_frame(
            &ctx,
            0.2,
            vec![egui::Event::PointerMoved(start + egui::vec2(10.0, 0.0))],
            |ui| render_param_number_input(ui, spec, 0.5, 6),
        );
        assert!(matches!(outcome, ParamEditOutcome::Commit(value)
                if (value.as_f64().unwrap() - (0.5 + 10.0 / 375.0)).abs() < 1e-15));

        let spec = ParamControlSpec {
            domain: next_domain,
            ..spec
        };
        let end = start + egui::vec2(20.0, 0.0);
        for (time, events) in [
            (0.3, vec![]),
            (0.4, vec![egui::Event::PointerMoved(end)]),
            (0.5, pointer_button(end, false)),
        ] {
            let (_, outcome) = widget_frame(&ctx, time, events, |ui| {
                render_param_number_input(ui, spec, 0.5, 6)
            });
            assert!(matches!(outcome, ParamEditOutcome::None), "stale drag");
        }

        widget_frame(&ctx, 0.7, pointer_button(start, true), |ui| {
            render_param_number_input(ui, spec, 0.5, 6)
        });
        let (_, outcome) = widget_frame(
            &ctx,
            0.8,
            vec![egui::Event::PointerMoved(start + egui::vec2(10.0, 0.0))],
            |ui| render_param_number_input(ui, spec, 0.5, 6),
        );
        let expected = next_domain.map_or(0.6, |domain| {
            domain.normalized_to_plain(domain.plain_to_normalized(0.5) + 10.0 / 375.0)
        });
        let ParamEditOutcome::Commit(value) = outcome else {
            panic!("a new drag must commit using the current domain");
        };
        let actual = value.as_f64().unwrap();
        assert!(
            (actual - expected).abs() < 1.0e-12,
            "a new drag must use the current domain: {actual} != {expected}"
        );
    }
}

#[test]
fn parameter_number_drags_preserve_extreme_range_precision() {
    for (minimum, maximum) in [
        (0.0, 1.0e-321),
        (-1.0e-321, 1.0e-321),
        (0.0, f64::from_bits(1)),
        (-1.0e308, 1.0),
        (1.0e16, 1.0e16 + 100.0),
    ] {
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            minimum,
            maximum,
            ParamScale::Linear,
            None,
            None,
            None,
            None,
        )
        .expect("valid range");
        let spec = ParamControlSpec {
            label: "extreme",
            ty: "f64",
            default: Some(minimum),
            domain: Some(domain),
        };
        for shift in [false, true] {
            for incremental in [false, true] {
                let ctx = egui::Context::default();
                let mut value = minimum;
                let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
                    render_param_number_input(ui, spec, value, 6)
                });
                let start = rect.center();
                widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
                    render_param_number_input(ui, spec, value, 6)
                });
                let modifiers = egui::Modifiers {
                    shift,
                    ..Default::default()
                };
                let distance = if shift { 10.0 } else { 1.0 };
                let moves = if incremental {
                    (6..=750)
                        .map(|points| points as f32 / 2.0)
                        .collect::<Vec<_>>()
                } else {
                    vec![187.5, 375.0]
                };
                for (index, pixels) in moves.into_iter().chain([750.0, 562.5]).enumerate() {
                    let (_, outcome) = widget_frame_with_modifiers(
                        &ctx,
                        0.2 + index as f64 * 0.001,
                        vec![egui::Event::PointerMoved(
                            start + egui::vec2(pixels * distance, 0.0),
                        )],
                        modifiers,
                        |ui| render_param_number_input(ui, spec, value, 6),
                    );
                    if let ParamEditOutcome::Commit(next) = outcome {
                        value = next.as_f64().unwrap();
                    }
                    if pixels == 187.5 || pixels == 562.5 {
                        assert_eq!(value, minimum + (maximum - minimum) * 0.5);
                    } else if pixels == 375.0 || pixels == 750.0 {
                        assert_eq!(value, maximum, "Shift={shift}, incremental={incremental}");
                    }
                }
                let (_, outcome) = widget_frame_with_modifiers(
                    &ctx,
                    2.0,
                    pointer_button(start + egui::vec2(562.5 * distance, 0.0), false),
                    modifiers,
                    |ui| render_param_number_input(ui, spec, value, 6),
                );
                if let ParamEditOutcome::Commit(next) = outcome {
                    value = next.as_f64().unwrap();
                }
                assert_eq!(value, minimum + (maximum - minimum) * 0.5);
            }
        }
    }
}

fn frame(
    ctx: &egui::Context,
    time: f64,
    width: f32,
    events: Vec<egui::Event>,
    mut render: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            time: Some(time),
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render(ui));
        },
    )
}

fn grid_frame(
    ctx: &egui::Context,
    time: f64,
    width: f32,
    events: Vec<egui::Event>,
    params: &[serde_json::Value],
) -> (Vec<egui::Rect>, Vec<(String, f64)>) {
    let mut rects = Vec::new();
    let mut commits = Vec::new();
    let _ = frame(ctx, time, width, events, |ui| {
        render_param_grid(ui, params, ParamLayout::Knobs, |ui, param| {
            let name = param_name(param).unwrap();
            let spec = ParamControlSpec {
                label: name,
                ty: "f64",
                default: Some(0.0),
                domain: None,
            };
            let rendered = ui.scope(|ui| {
                render_param_number_input(ui, spec, param["value"].as_f64().unwrap(), 6)
            });
            rects.push(rendered.response.rect);
            if let ParamEditOutcome::Commit(value) = rendered.inner {
                commits.push((name.to_owned(), value.as_f64().unwrap()));
            }
        });
    });
    (rects, commits)
}

#[test]
fn parameter_drafts_follow_their_owner_when_the_grid_reflows() {
    let ctx = egui::Context::default();
    let params = ["A", "B", "C", "D", "E", "F"]
        .into_iter()
        .enumerate()
        .map(|(value, name)| serde_json::json!({ "name": name, "value": value }))
        .collect::<Vec<_>>();
    let (rects, _) = grid_frame(&ctx, 0.0, 480.0, vec![], &params);
    assert!(rects[2].top() < rects[3].top(), "three columns");
    let position = rects[3].center();
    for (time, events) in [
        (0.1, pointer_button(position, true)),
        (0.15, pointer_button(position, false)),
        (0.2, vec![egui::Event::Text("99".into())]),
    ] {
        assert!(grid_frame(&ctx, time, 480.0, events, &params).1.is_empty());
    }
    let (rects, commits) = grid_frame(&ctx, 0.25, 320.0, vec![], &params);
    assert!(rects[1].top() < rects[2].top(), "two columns");
    assert!(commits.is_empty(), "reflow must preserve the active draft");
    let (_, commits) = grid_frame(&ctx, 0.3, 320.0, key_event(egui::Key::Enter), &params);
    assert_eq!(commits, [("D".into(), 99.0)]);
}

fn event_array_frame(
    ctx: &egui::Context,
    time: f64,
    width: f32,
    events: Vec<egui::Event>,
    values: &mut serde_json::Value,
) -> egui::FullOutput {
    frame(ctx, time, width, events, |ui| {
        render_event_array_editor(ui, "samples", "f64[6]", values, None, true);
    })
}

fn number_position(output: &egui::FullOutput, value: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                if text.galley.text() == value {
                    return Some(text.pos + text.galley.size() / 2.0);
                }
            }
            None
        })
        .expect("rendered number")
}

#[test]
fn event_array_drafts_follow_their_element_when_the_grid_reflows() {
    for (initial_width, next_width) in [(640.0, 320.0), (320.0, 640.0)] {
        for finish in [egui::Key::Enter, egui::Key::Escape] {
            let ctx = egui::Context::default();
            let original = serde_json::json!([0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
            let mut values = original.clone();
            let output = event_array_frame(&ctx, 0.0, initial_width, vec![], &mut values);
            let position = number_position(&output, "3");
            for (time, events) in [
                (0.1, pointer_button(position, true)),
                (0.15, pointer_button(position, false)),
                (0.2, vec![egui::Event::Text("99".into())]),
            ] {
                event_array_frame(&ctx, time, initial_width, events, &mut values);
            }
            let owner = ctx
                .memory(|memory| memory.focused())
                .expect("focused element");
            let mut edited = original.clone();
            edited[3] = serde_json::json!(99.0);
            assert_eq!(values, edited);

            event_array_frame(&ctx, 0.25, next_width, vec![], &mut values);
            assert_eq!(values, edited, "reflow must not change another element");
            assert_eq!(ctx.memory(|memory| memory.focused()), Some(owner));

            event_array_frame(&ctx, 0.3, next_width, key_event(finish), &mut values);
            assert_eq!(
                values,
                if finish == egui::Key::Escape {
                    original
                } else {
                    edited
                }
            );
            assert!(ctx.memory(|memory| memory.focused().is_none()));
        }
    }
}

#[test]
fn event_array_drags_follow_their_element_when_the_grid_reflows() {
    for (initial_width, next_width) in [(640.0, 320.0), (320.0, 640.0)] {
        let ctx = egui::Context::default();
        let mut values = serde_json::json!([0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        let output = event_array_frame(&ctx, 0.0, initial_width, vec![], &mut values);
        let position = number_position(&output, "3");
        event_array_frame(
            &ctx,
            0.1,
            initial_width,
            pointer_button(position, true),
            &mut values,
        );
        event_array_frame(
            &ctx,
            0.2,
            initial_width,
            vec![egui::Event::PointerMoved(position + egui::vec2(100.0, 0.0))],
            &mut values,
        );
        assert_eq!(values, serde_json::json!([0.0, 1.0, 2.0, 4.0, 4.0, 5.0]));

        event_array_frame(&ctx, 0.25, next_width, vec![], &mut values);
        assert_eq!(values, serde_json::json!([0.0, 1.0, 2.0, 4.0, 4.0, 5.0]));
        let end = position + egui::vec2(200.0, 0.0);
        event_array_frame(
            &ctx,
            0.3,
            next_width,
            vec![egui::Event::PointerMoved(end)],
            &mut values,
        );
        event_array_frame(
            &ctx,
            0.4,
            next_width,
            pointer_button(end, false),
            &mut values,
        );
        assert_eq!(values, serde_json::json!([0.0, 1.0, 2.0, 5.0, 4.0, 5.0]));
    }
}

#[test]
fn event_number_interactions_follow_the_argument_schema() {
    for (ty, original, default, display) in [
        (
            "f64",
            serde_json::json!(0.75),
            serde_json::json!(0.25),
            "0.75",
        ),
        (
            "i64",
            serde_json::json!("9007199254740993"),
            serde_json::json!("9007199254740995"),
            "9007199254740993",
        ),
        (
            "f64[2]",
            serde_json::json!([0.75, 1.0]),
            serde_json::json!([0.25, 1.0]),
            "0.75",
        ),
    ] {
        for change in ["refresh", "name", "default", "sibling"] {
            for interaction in ["escape", "enter", "drag"] {
                let ctx = egui::Context::default();
                let mut app =
                    RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
                let mut event = serde_json::json!({ "name": "trigger", "args": [
                    { "name": "level", "type": ty, "value": original, "default": default },
                    { "name": "enabled", "type": "bool", "value": false, "default": false },
                ] });
                app.event_inputs.insert(
                    "trigger".into(),
                    vec![original.clone(), serde_json::json!(false)],
                );
                app.event_input_signatures.insert(
                    "trigger".into(),
                    event_arg_signature(event["args"].as_array().unwrap()),
                );
                let output = event_controls_frame(&ctx, 0.0, vec![], &mut app, &event);
                let position = number_position(&output, display);
                event_controls_frame(&ctx, 0.1, pointer_button(position, true), &mut app, &event);
                if interaction == "drag" {
                    event_controls_frame(
                        &ctx,
                        0.2,
                        vec![egui::Event::PointerMoved(position + egui::vec2(100.0, 0.0))],
                        &mut app,
                        &event,
                    );
                } else {
                    event_controls_frame(
                        &ctx,
                        0.15,
                        pointer_button(position, false),
                        &mut app,
                        &event,
                    );
                    event_controls_frame(
                        &ctx,
                        0.2,
                        vec![egui::Event::Text("99".into())],
                        &mut app,
                        &event,
                    );
                }
                let edited = app.event_inputs["trigger"][0].clone();
                assert_ne!(edited, original, "the interaction must change its argument");
                match change {
                    "name" => event["args"][0]["name"] = serde_json::json!("replacement"),
                    "default" => {
                        event["args"][0]["default"] = match ty {
                            "i64" => serde_json::json!("5"),
                            "f64[2]" => serde_json::json!([0.5, 1.0]),
                            _ => serde_json::json!(0.5),
                        }
                    }
                    "sibling" => event["args"][1]["name"] = serde_json::json!("replacement"),
                    _ => {}
                }
                // Hot reload resets the event values when its argument signature changes.
                let replacement = event["args"][0]["default"].clone();
                if change != "refresh" {
                    app.event_inputs.get_mut("trigger").unwrap()[0] = replacement.clone();
                    app.event_json_drafts.remove("trigger");
                    app.event_input_signatures.insert(
                        "trigger".into(),
                        event_arg_signature(event["args"].as_array().unwrap()),
                    );
                }
                event_controls_frame(&ctx, 0.25, vec![], &mut app, &event);
                if interaction == "drag" {
                    event_controls_frame(
                        &ctx,
                        0.3,
                        vec![egui::Event::PointerMoved(position + egui::vec2(200.0, 0.0))],
                        &mut app,
                        &event,
                    );
                    event_controls_frame(
                        &ctx,
                        0.4,
                        pointer_button(position + egui::vec2(200.0, 0.0), false),
                        &mut app,
                        &event,
                    );
                } else {
                    let key = if interaction == "escape" {
                        egui::Key::Escape
                    } else {
                        egui::Key::Enter
                    };
                    event_controls_frame(&ctx, 0.3, key_event(key), &mut app, &event);
                }
                let value = &app.event_inputs["trigger"][0];
                if change != "refresh" {
                    assert_eq!(value, &replacement, "{ty}, {change}, {interaction}: replacement must discard the old interaction");
                } else if interaction == "drag" {
                    assert_ne!(value, &edited, "unchanged schemas retain the drag");
                } else {
                    assert_eq!(
                        value,
                        if interaction == "escape" {
                            &original
                        } else {
                            &edited
                        },
                        "unchanged schemas retain the draft"
                    );
                }
            }
        }
    }
}

fn event_controls_frame(
    ctx: &egui::Context,
    time: f64,
    input: Vec<egui::Event>,
    app: &mut RunApp,
    event: &serde_json::Value,
) -> egui::FullOutput {
    frame(ctx, time, 640.0, input, |ui| {
        app.render_events(ui, std::slice::from_ref(event), true);
    })
}

#[test]
fn event_number_reset_discards_a_draft_when_the_click_is_in_one_frame() {
    for (ty, original, display, edited) in [
        (
            "f64",
            serde_json::json!(0.25),
            "0.25",
            serde_json::json!(99.0),
        ),
        (
            "i64",
            serde_json::json!("9007199254740993"),
            "9007199254740993",
            serde_json::json!("99"),
        ),
    ] {
        let ctx = egui::Context::default();
        let mut value = original.clone();
        let mut number = egui::Pos2::ZERO;
        let mut reset = egui::Rect::NOTHING;
        for (index, time) in [0.0, 0.1, 0.15, 0.2, 0.6, 0.7].into_iter().enumerate() {
            let events = match index {
                1 => pointer_button(number, true),
                2 => pointer_button(number, false),
                3 => vec![egui::Event::Text("99".into())],
                4 => [
                    pointer_button(reset.center(), true),
                    pointer_button(reset.center(), false),
                ]
                .concat(),
                _ => vec![],
            };
            let output = frame(&ctx, time, 320.0, events, |ui| {
                let response = ui.button("Reset");
                reset = response.rect;
                if response.clicked() {
                    value = original.clone();
                }
                render_event_scalar_editor(ui, ty, &mut value, Some(&original), true, 112.0);
            });
            if index == 0 {
                number = number_position(&output, display);
            } else if index == 3 {
                assert_eq!(value, edited, "typed draft");
            } else if index >= 4 {
                assert_eq!(
                    value, original,
                    "reset must survive the old draft losing focus"
                );
            }
        }
    }
}

#[test]
fn integer_number_drags_reverse_from_fractional_overshoot_at_both_bounds() {
    for (initial, range, speed, moves) in [
        (6, Some((-1.0, 7.0)), 0.1, [(14.0, 7), (8.0, 6)]),
        (7, Some((-1.0, 7.0)), 0.1, [(3.0, 7), (-3.0, 6)]),
        (0, Some((-1.0, 7.0)), 0.1, [(-14.0, -1), (-8.0, 0)]),
        (-1, Some((-1.0, 7.0)), 0.1, [(-3.0, -1), (3.0, 0)]),
        // Rounding onto a bound must preserve fractional motion toward the interior.
        (6, Some((-1.0, 7.0)), 0.1, [(8.0, 7), (4.0, 6)]),
        (0, Some((-1.0, 7.0)), 0.1, [(-8.0, -1), (-4.0, 0)]),
        (
            i64::MAX - 1,
            None,
            0.25,
            [(5.0, i64::MAX), (2.0, i64::MAX - 1)],
        ),
        (
            i64::MIN + 1,
            None,
            0.25,
            [(-5.0, i64::MIN), (-2.0, i64::MIN + 1)],
        ),
    ] {
        for shift in [false, true] {
            let ctx = egui::Context::default();
            let mut value = NumberValue::Integer(initial);
            let render = |ui: &mut egui::Ui, value: &mut NumberValue| {
                let mut input = NumberInput::new(value, NumberValue::Integer(initial), speed);
                if let Some((min, max)) = range {
                    input = input.range(min, max);
                }
                ui.add(input)
            };
            let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| render(ui, &mut value));
            let start = rect.center();
            widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
                render(ui, &mut value)
            });
            let modifiers = egui::Modifiers {
                shift,
                ..Default::default()
            };
            for (index, (pixels, expected)) in moves.into_iter().enumerate() {
                let position = start + egui::vec2(pixels * if shift { 10.0 } else { 1.0 }, 0.0);
                widget_frame_with_modifiers(
                    &ctx,
                    0.2 + index as f64 * 0.1,
                    vec![egui::Event::PointerMoved(position)],
                    modifiers,
                    |ui| render(ui, &mut value),
                );
                assert_eq!(
                    value,
                    NumberValue::Integer(expected),
                    "{initial}, {pixels}, Shift={shift}"
                );
                if index == moves.len() - 1 {
                    widget_frame_with_modifiers(
                        &ctx,
                        0.4,
                        pointer_button(position, false),
                        modifiers,
                        |ui| render(ui, &mut value),
                    );
                    assert_eq!(value, NumberValue::Integer(expected));
                }
            }
        }
    }
}

#[test]
fn integer_parameter_drags_snap_fractional_motion_once() {
    for (ty, scalar) in [("i32", ParamScalarType::I32), ("i64", ParamScalarType::I64)] {
        for step in [2.0, 4.0] {
            let ctx = egui::Context::default();
            let domain = ParamDomain::new(
                scalar,
                0.0,
                step * 5.0,
                ParamScale::Linear,
                None,
                None,
                Some(step),
                Some(5),
            )
            .unwrap();
            let spec = ParamControlSpec {
                label: "stepped",
                ty,
                default: Some(0.0),
                domain: Some(domain),
            };
            let mut value = 0.0;
            let (rect, _) = widget_frame(&ctx, 0.0, vec![], |ui| {
                render_param_number_input(ui, spec, value, 0)
            });
            let start = rect.center();
            widget_frame(&ctx, 0.1, pointer_button(start, true), |ui| {
                render_param_number_input(ui, spec, value, 0)
            });
            for (index, (pixels, expected)) in
                [(18.75, 0.0), (37.0, 0.0), (37.5, step), (37.0, 0.0)]
                    .into_iter()
                    .enumerate()
            {
                let (_, outcome) = widget_frame(
                    &ctx,
                    0.2 + index as f64 * 0.1,
                    vec![egui::Event::PointerMoved(start + egui::vec2(pixels, 0.0))],
                    |ui| render_param_number_input(ui, spec, value, 0),
                );
                if let ParamEditOutcome::Commit(next) = outcome {
                    value = domain.constrain_plain(next.as_f64().unwrap());
                }
                assert_eq!(value, expected, "{ty}, step {step}, {pixels} pixels");
            }
        }
    }
}
