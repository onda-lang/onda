use eframe::egui;
use onda_run::{RunHostOptions, RunMidiCapabilities, RunState};
use serde_json::json;

use crate::{apply_run_theme, ParamLayout, RunApp, RunTheme};

#[test]
fn section_tab_order_starts_with_disclosure_then_actions() {
    for width in [240.0, 720.0] {
        let ctx = egui::Context::default();
        apply_run_theme(&ctx, &RunTheme::from_dark_mode(true));
        let mut reset_rect = egui::Rect::NOTHING;
        for frame in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 300.0),
                    )),
                    events: if frame == 0 {
                        Vec::new()
                    } else {
                        [true, false]
                            .map(|pressed| egui::Event::Key {
                                key: egui::Key::Tab,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            })
                            .to_vec()
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ctx,
                                ui.make_persistent_id("events"),
                                true,
                            );
                        crate::render_section_header(ui, &mut state, "Events", |ui| {
                            reset_rect = super::reset_icon_button(ui, "Reset event arguments").rect;
                        });
                    });
                },
            );
            if frame > 0 {
                let focused = ctx
                    .memory(|memory| memory.focused())
                    .expect("focused section control");
                let rect = ctx.read_response(focused).unwrap().rect;
                if frame == 1 {
                    assert!(
                        rect.right() < reset_rect.left(),
                        "disclosure must precede Reset"
                    );
                } else {
                    assert_eq!(rect, reset_rect);
                }
            }
        }
    }
}

#[test]
fn midi_labels_and_tab_order_follow_their_number_fields_when_wrapping() {
    for width in [160.0, 180.0, 200.0, 240.0, 280.0, 300.0, 400.0, 720.0] {
        let ctx = egui::Context::default();
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        for frame in 0..5 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 840.0),
                    )),
                    events: if frame < 2 {
                        Vec::new()
                    } else {
                        [true, false]
                            .map(|pressed| egui::Event::Key {
                                key: egui::Key::Tab,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            })
                            .to_vec()
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::TopBottomPanel::bottom("midi")
                        .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(12, 10)))
                        .show(ctx, |ui| app.render_midi_keyboard(ui, &theme));
                },
            );
            // Bottom panels settle their content height after the first frame.
            if frame == 0 {
                continue;
            }
            let fields: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if shape.rect.size() == egui::vec2(56.0, 26.0) => {
                        Some(shape.rect)
                    }
                    egui::Shape::Rect(shape) if shape.rect.size() == egui::vec2(58.0, 28.0) => {
                        Some(shape.rect.shrink(1.0))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(fields.len(), 2, "width {width}, frame {frame}");
            for (index, label) in ["Octave", "Velocity"].into_iter().enumerate() {
                let rect = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.job.text == label => {
                            Some(text.galley.rect.translate(text.pos.to_vec2()))
                        }
                        _ => None,
                    })
                    .expect("MIDI label");
                let field = fields.iter().find(|field| {
                    (field.center().y - rect.center().y).abs() < 1.0
                        && (field.left() - rect.right() - 6.0).abs() < 1.0
                });
                assert!(
                    field.is_some(),
                    "{label} must stay beside its field at width {width}: {rect:?}, {fields:?}"
                );
                if frame == index + 3 {
                    let focused = ctx
                        .memory(|memory| memory.focused())
                        .expect("focused MIDI field");
                    assert_eq!(
                        ctx.read_response(focused).unwrap().rect,
                        *field.unwrap(),
                        "Tab must visit {label} at width {width}"
                    );
                }
            }
        }
    }
}

#[test]
fn stopped_midi_piano_clicks_still_highlight_keys() {
    let ctx = egui::Context::default();
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
    let mut pos = egui::Pos2::ZERO;
    for frame in 0..4 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(720.0, 840.0),
                )),
                events: if frame < 2 {
                    Vec::new()
                } else {
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: frame == 2,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                },
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("midi")
                    .show(ctx, |ui| app.render_midi_keyboard(ui, &theme));
            },
        );
        if frame < 2 {
            let piano = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if shape.fill == theme.piano.bed => Some(shape.rect),
                    _ => None,
                })
                .unwrap();
            pos = egui::pos2(piano.left() + 5.0, piano.bottom() - 5.0);
        } else if frame == 2 {
            assert_eq!(app.pointer_note, Some(60));
            assert!(output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::Shape::Rect(shape) if shape.fill == theme.piano.white_active)));
        } else {
            assert_eq!(app.pointer_note, None);
        }
    }
}

#[test]
fn midi_piano_adds_octaves_and_keeps_proportions_when_resized() {
    let ctx = egui::Context::default();
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
    let render = |app: &mut RunApp, width, height, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, height),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::bottom("midi")
                    .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(12, 10)))
                    .show(ctx, |ui| app.render_midi_keyboard(ui, &theme));
            },
        )
    };
    app.computer_notes.insert(egui::Key::A, 60);
    for (width, height, octave, octaves) in [
        (320.0, 840.0, 4, 2),
        (720.0, 840.0, 4, 2),
        (960.0, 840.0, 4, 2),
        (1040.0, 840.0, 4, 3),
        (1440.0, 840.0, 4, 4),
        (1760.0, 840.0, 4, 5),
        (2560.0, 840.0, 4, 5),
        (4000.0, 840.0, 4, 5),
        (720.0, 840.0, 4, 2),
        (1760.0, 400.0, 4, 5),
        (4000.0, 840.0, 7, 2),
        (4000.0, 840.0, -1, 10),
    ] {
        app.keyboard_octave = octave;
        for _ in 0..2 {
            let _ = render(&mut app, width, height, Vec::new());
        }
        let output = render(&mut app, width, height, Vec::new());
        let rectangles = |colors: &[egui::Color32]| {
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if colors.contains(&shape.fill) => Some(shape.rect),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let bed = rectangles(&[theme.piano.bed])[0];
        let white = rectangles(&[theme.piano.white, theme.piano.white_active]);
        let black = rectangles(&[theme.piano.black, theme.piano.black_active]);
        assert_eq!(white.len(), octaves * 7, "width {width}, octave {octave}");
        assert_eq!(black.len(), octaves * 5);
        assert!((104.0..=156.0).contains(&bed.height()));
        assert!(white.iter().all(|key| key.width() <= 72.01));
        assert!((bed.center().x - width * 0.5).abs() < 1.0);
        assert!(bed.left() >= 12.0 && bed.right() <= width - 12.0);
        assert!(bed.top() > 100.0 && bed.bottom() <= height);
        if width == 720.0 || height == 400.0 {
            assert_eq!(bed.height(), 104.0);
        }
        for (index, key) in black.iter().enumerate() {
            let boundary = index / 5 * 7 + [1, 2, 4, 5, 6][index % 5];
            assert!((key.center().x - white[boundary - 1].right()).abs() < 0.01);
        }
        assert_eq!(app.computer_notes.get(&egui::Key::A), Some(&60));
        let pos = egui::pos2(white.last().unwrap().center().x, bed.bottom() - 5.0);
        let pointer = |pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = render(&mut app, width, height, pointer(true));
        assert_eq!(
            app.pointer_note,
            Some((octave + 1) * 12 + octaves as i32 * 12 - 1)
        );
        let _ = render(&mut app, width, height, pointer(false));
        assert_eq!(app.pointer_note, None);
        // Clear hover before measuring the next layout's idle key fills.
        let _ = render(&mut app, width, height, vec![egui::Event::PointerGone]);
    }

    app.keyboard_octave = 4;
    let piano_rect = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.fill == theme.piano.bed => Some(shape.rect),
                _ => None,
            })
            .unwrap()
    };
    for _ in 0..2 {
        let _ = render(&mut app, 720.0, 840.0, Vec::new());
    }
    let bed = piano_rect(&render(&mut app, 720.0, 840.0, Vec::new()));
    let pos = egui::pos2(bed.left() + 5.0, bed.bottom() - 5.0);
    let _ = render(
        &mut app,
        720.0,
        840.0,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(app.pointer_note, Some(60));
    for width in [1760.0, 2560.0] {
        for _ in 0..3 {
            let _ = render(&mut app, width, 840.0, Vec::new());
            assert_eq!(
                app.pointer_note,
                Some(60),
                "resizing must preserve held pointer notes"
            );
        }
    }
    let bed = piano_rect(&render(&mut app, 2560.0, 840.0, Vec::new()));
    let pos = egui::pos2(bed.left() + bed.width() / 35.0 * 28.5, bed.bottom() - 5.0);
    let _ = render(
        &mut app,
        2560.0,
        840.0,
        vec![egui::Event::PointerMoved(pos)],
    );
    assert_eq!(
        app.pointer_note,
        Some(108),
        "dragging still moves between keys"
    );
    for _ in 0..3 {
        let _ = render(&mut app, 720.0, 840.0, Vec::new());
        assert_eq!(
            app.pointer_note, None,
            "shrinking releases keys that disappear"
        );
    }
}

fn stereo_state(midi_available: bool) -> RunState {
    RunState {
        running: true,
        connected: true,
        path: "/a/long/project/directory/orbit_flanger.onda".to_owned(),
        status: "Running".to_owned(),
        error: None,
        output_channels: 2,
        buffers: Vec::new(),
        events: Vec::new(),
        midi: RunMidiCapabilities {
            available: midi_available,
            note_on: midi_available,
            ..Default::default()
        },
        delegates: Vec::new(),
        log_text: String::new(),
        log_entries: Vec::new(),
        log_revealed: false,
        print_overflow_count: 0,
        print_transport_drop_count: 0,
        delegate_overflow_count: 0,
        delegate_transport_drop_count: 0,
        params: (0..8)
            .map(|index| {
                json!({
                    "name": format!("parameter_{index}"), "type": "f32", "scalar": true, "value": 0.5,
                "default": 0.5, "rangeMin": 0.0, "rangeMax": 1.0, "scale": "linear",
                })
            })
            .collect(),
        input_devices: vec!["Default input".to_owned()],
        output_devices: vec!["Default output".to_owned()],
        midi_input_devices: vec!["Computer Keyboard".to_owned()],
        current_input_device: None,
        current_output_device: None,
        current_midi_input_device: None,
        scope_channels: 2,
        scope_samples: vec![0.0; 512],
    }
}

#[test]
fn stopped_scope_keeps_gray_guides_without_a_waveform_or_silent_trace() {
    for dark in [false, true] {
        for channels in [1, 2] {
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            let theme = RunTheme::from_dark_mode(dark);
            apply_run_theme(&ctx, &theme);
            let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
            for (running, samples) in [
                (true, vec![0.25, -0.5, 0.75, -0.25]),
                (false, vec![0.25, -0.5, 0.75, -0.25]),
                (false, vec![0.0; 4]),
                (false, Vec::new()),
            ] {
                let mut state = stereo_state(false);
                state.running = running;
                state.output_channels = channels;
                state.scope_channels = if samples.is_empty() { 0 } else { channels };
                state.scope_samples = samples;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(720.0, 840.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            app.render_run_view(ui, ctx, state.clone(), &theme);
                        });
                    },
                );
                let scope = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Rect(rect) if rect.fill == theme.scope_background => {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("scope remains visible");
                assert_eq!(scope.height(), 160.0);
                let mut guides = Vec::new();
                let mut traces = 0;
                for clipped in &output.shapes {
                    match &clipped.shape {
                        egui::Shape::LineSegment { points, stroke }
                            if scope.contains(points[0].lerp(points[1], 0.5)) =>
                        {
                            if stroke.color == theme.scope_grid {
                                assert_eq!(stroke.width, 1.0);
                                guides.push(*points);
                            } else {
                                traces += 1;
                            }
                        }
                        egui::Shape::Path(path)
                            if scope.contains(path.visual_bounding_rect().center()) =>
                        {
                            traces += 1;
                        }
                        _ => {}
                    }
                }
                assert_eq!(guides.len(), channels);
                for (channel, points) in guides.iter().enumerate() {
                    let y = scope.top() + scope.height() / channels as f32 * (channel as f32 + 0.5);
                    assert_eq!(
                        *points,
                        [egui::pos2(scope.left(), y), egui::pos2(scope.right(), y)]
                    );
                }
                assert_eq!(traces, if running { channels } else { 0 });
            }
        }
    }
}

#[test]
fn stereo_scope_and_two_knob_rows_fit_in_the_default_window() {
    for dark in [false, true] {
        for midi_available in [false, true] {
            let ctx = egui::Context::default();
            egui_extras::install_image_loaders(&ctx);
            let theme = RunTheme::from_dark_mode(dark);
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            apply_run_theme(&ctx, &theme);
            let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
            for frame in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(720.0, 840.0),
                        )),
                        time: Some(frame as f64),
                        ..Default::default()
                    },
                    |ctx| app.render_run_panels(ctx, stereo_state(midi_available), &theme),
                );
                let labels: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|clipped| {
                        let egui::Shape::Text(text) = &clipped.shape else {
                            return None;
                        };
                        text.galley.job.text.starts_with("parameter_").then_some((
                            text.galley.rect.translate(text.pos.to_vec2()),
                            clipped.clip_rect,
                        ))
                    })
                    .collect();
                assert_eq!(labels.len(), 8);
                let rows: std::collections::HashSet<_> = labels
                    .iter()
                    .map(|(rect, _)| rect.top().round() as i32)
                    .collect();
                assert_eq!(rows.len(), 2);
                let mut cards = 0;
                for clipped in &output.shapes {
                    if let egui::Shape::Rect(shape) = &clipped.shape {
                        if (shape.rect.height() - 140.0).abs() < 0.1 {
                            cards += 1;
                            assert!(
                                clipped.clip_rect.contains_rect(shape.rect),
                                "clipped knob card {:?}, clip {:?}, MIDI: {midi_available}",
                                shape.rect,
                                clipped.clip_rect
                            );
                        }
                    }
                }
                assert_eq!(cards, 8, "all knob cards must be checked for clipping");
                for clipped in &output.shapes {
                    if let egui::Shape::Rect(shape) = &clipped.shape {
                        if shape.corner_radius == crate::SECTION_CORNER_RADIUS.into() {
                            assert!(
                                clipped.clip_rect.contains_rect(shape.rect),
                                "clipped section {:?}, clip {:?}, MIDI: {midi_available}",
                                shape.rect,
                                clipped.clip_rect
                            );
                        }
                    }
                }
                assert_eq!(
                    output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                    egui::Shape::Text(text) if text.galley.job.text == "MIDI Keyboard")),
                    midi_available
                );
            }
        }
    }
}

#[test]
fn parameter_grids_grow_to_eight_columns_and_reflow_when_resized() {
    for layout in [ParamLayout::Knobs, ParamLayout::Sliders] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, layout);
        let mut state = stereo_state(false);
        state.params = (0..17)
            .map(|index| {
                let mut param = state.params[index % 8].clone();
                param["name"] = json!(format!("parameter_{index}"));
                param
            })
            .collect();
        let array = state
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let mut param = param.clone();
                param["name"] = json!(format!("levels[{index}]"));
                param["array"] = json!({ "name": "levels", "length": 17, "index": index });
                param
            })
            .collect::<Vec<_>>();
        state.params.extend(array);
        let (height, minimum_width) = match layout {
            ParamLayout::Knobs => (140.0, 140.0),
            ParamLayout::Sliders => (80.0, 200.0),
        };
        for (width, knobs, sliders) in [
            (640.0, 4, 2),
            (800.0, 5, 3),
            (960.0, 6, 4),
            (1120.0, 7, 5),
            (1280.0, 8, 5),
            (1440.0, 8, 6),
            (1600.0, 8, 7),
            (1760.0, 8, 8),
            (3200.0, 8, 8),
            (1280.0, 8, 5),
            (640.0, 4, 2),
        ] {
            for _ in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 2400.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| app.render_run_panels(ctx, state.clone(), &theme),
                );
                let cards: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|clipped| match &clipped.shape {
                        egui::Shape::Rect(shape)
                            if shape.corner_radius == crate::CARD_CORNER_RADIUS.into()
                                && shape.rect.height() == height =>
                        {
                            Some(shape.rect)
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(cards.len(), 34);
                let expected = if layout == ParamLayout::Knobs {
                    knobs
                } else {
                    sliders
                };
                let columns = cards[..17]
                    .iter()
                    .filter(|rect| rect.top() == cards[0].top())
                    .count();
                assert_eq!(columns, expected, "width {width}, layout {layout:?}");
                let array_columns = cards[17..]
                    .iter()
                    .filter(|rect| rect.top() == cards[17].top())
                    .count();
                assert!(
                    array_columns <= expected,
                    "array grids respect the same column limit"
                );
                if width >= 1760.0 {
                    assert_eq!(array_columns, 8);
                }
                assert!(cards.iter().all(|rect| rect.width() >= minimum_width));
                for clipped in &output.shapes {
                    let rect = match &clipped.shape {
                        egui::Shape::Rect(shape) if shape.rect.height() <= height => shape.rect,
                        egui::Shape::Text(text) => text.galley.rect.translate(text.pos.to_vec2()),
                        _ => continue,
                    };
                    if let Some(card) = cards.iter().find(|card| card.contains(rect.center())) {
                        assert!(
                            card.contains_rect(rect),
                            "width {width}: {rect:?} outside {card:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn narrow_knob_grids_keep_two_columns_and_contain_their_controls() {
    for width in [240.0, 280.0, 300.0, 400.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        for _ in 0..2 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1400.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.render_run_panels(ctx, stereo_state(false), &theme),
            );
            let cards: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape)
                        if shape.corner_radius == crate::CARD_CORNER_RADIUS.into()
                            && shape.rect.height() == 140.0 =>
                    {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(cards.len(), 8);
            let rows: std::collections::HashSet<_> =
                cards.iter().map(|rect| rect.top().round() as i32).collect();
            assert_eq!(rows.len(), 4, "width {width}: two knobs per row");
            for clipped in &output.shapes {
                let rect = match &clipped.shape {
                    egui::Shape::Rect(shape) => shape.rect,
                    egui::Shape::Text(text) => text.galley.rect.translate(text.pos.to_vec2()),
                    _ => continue,
                };
                if let Some(card) = cards.iter().find(|card| card.contains(rect.center())) {
                    assert!(
                        card.contains_rect(rect),
                        "width {width}: {rect:?} outside {card:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn collapsed_section_headings_stay_centered_when_actions_wrap() {
    let sections = ["Scope", "Log", "Buffers", "Events", "Params"];
    for width in [240.0, 300.0, 400.0, 720.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let mut state = stereo_state(false);
        state.log_revealed = true;
        state.buffers = vec![json!({ "name": "buffer", "type": "f32[]" })];
        state.events = vec![json!({ "name": "ping", "args": [] })];
        let mut frame = 0;
        let mut render = |events| {
            frame += 1;
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 3000.0),
                    )),
                    time: Some(f64::from(frame)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_run_view(ui, ctx, state.clone(), &theme);
                    });
                },
            )
        };
        let text_rect = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing heading {label}"))
        };
        let mut output = render(Vec::new());
        for label in sections {
            let title = text_rect(&output, label);
            let arrow = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Path(path)
                        if path.closed
                            && path.points.len() == 3
                            && (path.visual_bounding_rect().center().y - title.center().y)
                                .abs()
                                < 1.0 =>
                    {
                        Some(path.visual_bounding_rect())
                    }
                    _ => None,
                })
                .expect("section disclosure arrow");
            let pos = arrow.center();
            let _ = render(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            output = render(Vec::new());
        }
        for label in sections {
            let title = text_rect(&output, label);
            let panel = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape)
                        if shape.corner_radius == crate::SECTION_CORNER_RADIUS.into()
                            && shape.rect.contains(title.center()) =>
                    {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .expect("collapsed section panel");
            let heading_center = if label == "Params" && width < 400.0 {
                panel.top() + 11.0 + crate::PARAM_HEADER_CONTROL_HEIGHT / 2.0
            } else {
                panel.center().y
            };
            assert!(
                (title.center().y - heading_center).abs() < 1.0,
                "{label}, width {width}: title {title:?}, panel {panel:?}"
            );
            let arrow = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Path(path)
                        if path.closed
                            && path.points.len() == 3
                            && panel.contains(path.visual_bounding_rect().center()) =>
                    {
                        Some(path.visual_bounding_rect())
                    }
                    _ => None,
                })
                .expect("collapsed disclosure arrow");
            assert!(
                (arrow.center().y - heading_center).abs() < 1.0,
                "{label}, width {width}: arrow {arrow:?}, panel {panel:?}"
            );
        }
    }
}

#[test]
fn header_tab_order_matches_the_toolbar_and_escape_restores_devices_focus() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
    let state = stereo_state(true);
    let mut render = |key: Option<egui::Key>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(720.0, 840.0),
                )),
                events: key
                    .into_iter()
                    .flat_map(|key| {
                        [true, false].map(|pressed| egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        })
                    })
                    .collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_run_header(ui, ctx, &state, &theme);
                });
            },
        )
    };
    let output = render(None);
    let mut controls: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| {
            let egui::Shape::Rect(shape) = &clipped.shape else {
                return None;
            };
            (shape.brush.is_none()
                && (shape.rect.size() == super::TRANSPORT_BUTTON_SIZE
                    || shape.rect.size() == egui::vec2(32.0, 32.0)))
            .then_some(shape.rect)
        })
        .collect();
    controls.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(controls.len(), 4);
    let focused_rect = || {
        let id = ctx
            .memory(|memory| memory.focused())
            .expect("focused toolbar control");
        ctx.read_response(id)
            .expect("rendered focused control")
            .rect
    };
    let mut devices_id = None;
    for (index, rect) in controls.iter().enumerate() {
        let _ = render(Some(egui::Key::Tab));
        assert_eq!(focused_rect(), *rect);
        if index == 1 {
            devices_id = ctx.memory(|memory| memory.focused());
        }
    }
    ctx.memory_mut(|memory| memory.request_focus(devices_id.expect("Devices focus ID")));
    let _ = render(None);
    assert_eq!(focused_rect(), controls[1]);
    let _ = render(Some(egui::Key::Enter));
    let _ = render(None);
    let _ = render(Some(egui::Key::Tab));
    assert_ne!(focused_rect(), controls[1]);
    let _ = render(Some(egui::Key::Escape));
    let _ = render(None);
    assert_eq!(focused_rect(), controls[1]);
    let _ = render(Some(egui::Key::Tab));
    assert_eq!(focused_rect(), controls[2]);
}

#[test]
fn devices_dropdown_choices_outside_the_settings_panel_remain_selectable() {
    for (size, field, target) in [
        (egui::vec2(720.0, 840.0), 0, "Device 8"),
        (egui::vec2(720.0, 840.0), 1, "Device 8"),
        (egui::vec2(240.0, 840.0), 2, "Device 8"),
        (egui::vec2(720.0, 300.0), 2, "Device 1"),
    ] {
        let ctx = egui::Context::default();
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let devices: Vec<_> = (0..20).map(|index| format!("Device {index}")).collect();
        let mut selected = devices[0].clone();
        let mut open = false;
        let mut frame = 0;
        let mut render = |events, selected: &mut String, open: &mut bool| {
            frame += 1;
            let mut button = egui::Rect::NOTHING;
            let mut selectors = Vec::new();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    time: Some(f64::from(frame) / 60.0),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.spacing_mut().interact_size.y = 32.0;
                        let settings = super::device_settings_button(
                            ui,
                            egui::Id::new("test-devices"),
                            open,
                            |ui| {
                                for (index, label) in
                                    ["Input Device", "Output Device", "MIDI Input"]
                                        .into_iter()
                                        .enumerate()
                                {
                                    let current = selected.clone();
                                    selectors.push(
                                        super::render_device_combo(
                                            ui,
                                            label,
                                            theme.muted_text,
                                            (index < 2).then_some("Default"),
                                            &devices,
                                            Some(&current),
                                            |selection| *selected = selection.unwrap().to_owned(),
                                        )
                                        .rect,
                                    );
                                    ui.add_space(8.0);
                                }
                                ui.add_sized(
                                    [ui.available_width(), 32.0],
                                    egui::Button::new("Refresh Devices"),
                                );
                            },
                        );
                        button = settings.response.rect;
                    });
                },
            );
            (output, button, selectors)
        };
        let pointer = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let (_, button, _) = render(Vec::new(), &mut selected, &mut open);
        render(pointer(button.center(), true), &mut selected, &mut open);
        render(pointer(button.center(), false), &mut selected, &mut open);
        let (_, _, selectors) = render(Vec::new(), &mut selected, &mut open);
        let pos = selectors[field].center();
        render(pointer(pos, true), &mut selected, &mut open);
        render(pointer(pos, false), &mut selected, &mut open);
        let (output, _, _) = render(Vec::new(), &mut selected, &mut open);
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == target => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                }
                _ => None,
            })
            .expect("dropdown option outside the settings panel");
        render(pointer(pos, true), &mut selected, &mut open);
        render(pointer(pos, false), &mut selected, &mut open);
        assert_eq!(selected, target, "field {field}, viewport {size:?}");
        assert!(open, "selecting a device keeps Settings open");
        // Escape dismisses a nested dropdown first, then Settings itself.
        let (_, _, selectors) = render(Vec::new(), &mut selected, &mut open);
        let pos = selectors[field].center();
        render(pointer(pos, true), &mut selected, &mut open);
        render(pointer(pos, false), &mut selected, &mut open);
        assert!(ctx.memory(|memory| memory.any_popup_open()));
        let escape = || {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        render(escape(), &mut selected, &mut open);
        assert!(open);
        assert!(!ctx.memory(|memory| memory.any_popup_open()));
        render(Vec::new(), &mut selected, &mut open);
        render(escape(), &mut selected, &mut open);
        assert!(!open);
        render(Vec::new(), &mut selected, &mut open);
        render(pointer(button.center(), true), &mut selected, &mut open);
        render(pointer(button.center(), false), &mut selected, &mut open);
        assert!(open);
        // Clicking outside the whole popup still dismisses Settings.
        let outside = egui::pos2(size.x - 4.0, size.y - 4.0);
        render(pointer(outside, true), &mut selected, &mut open);
        render(pointer(outside, false), &mut selected, &mut open);
        assert!(!open);
    }
}

#[test]
fn midi_keyboard_and_scope_keep_their_positions_after_stop_and_restart() {
    for width in [240.0, 720.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let mut initial = None;
        for frame in 0..6 {
            let mut state = stereo_state(true);
            state.running = !(2..4).contains(&frame);
            state.connected = state.running;
            if frame == 2 {
                app.computer_notes.insert(egui::Key::A, 60);
                app.pointer_note = Some(64);
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 840.0),
                    )),
                    time: Some(f64::from(frame)),
                    ..Default::default()
                },
                |ctx| app.render_run_panels(ctx, state.clone(), &theme),
            );
            let plot = |fill, height| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.fill == fill && rect.rect.height() == height =>
                        {
                            Some(rect.rect)
                        }
                        _ => None,
                    })
                    .expect("scope and piano remain visible")
            };
            let geometry = (
                plot(theme.scope_background, 160.0),
                plot(theme.piano.bed, 104.0),
            );
            if frame == 1 {
                initial = Some(geometry);
            } else if frame > 1 {
                assert_eq!(geometry, initial.unwrap());
            }
            if frame == 2 {
                assert!(app.computer_notes.is_empty());
                assert!(app.pointer_note.is_none());
            }
        }
    }
}

#[test]
fn transport_keeps_focus_and_keyboard_activation_when_the_header_wraps() {
    let ctx = egui::Context::default();
    egui_extras::install_image_loaders(&ctx);
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
    let mut render = |width, running, key: Option<egui::Key>| {
        let mut state = stereo_state(true);
        state.running = running;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 840.0),
                )),
                events: key
                    .into_iter()
                    .flat_map(|key| {
                        [true, false].map(|pressed| egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        })
                    })
                    .collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_run_header(ui, ctx, &state, &theme);
                });
            },
        )
    };
    let _ = render(720.0, true, None);
    let _ = render(720.0, true, Some(egui::Key::Tab));
    let transport_id = ctx
        .memory(|memory| memory.focused())
        .expect("Tab focuses the transport");
    for running in [true, false] {
        for width in [720.0, 400.0, 240.0, 160.0, 400.0, 720.0] {
            let _ = render(width, running, None);
            assert_eq!(
                ctx.memory(|memory| memory.focused()),
                Some(transport_id),
                "transport focus at width {width}, running {running}"
            );
            // egui exposes the prior pass's response geometry between frames.
            let output = render(width, running, None);
            let transport_rect = ctx.read_response(transport_id).unwrap().rect;
            assert_eq!(transport_rect.size(), super::TRANSPORT_BUTTON_SIZE);
            let label_visible = output.shapes.iter().any(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.job.text == if running { "Stop" } else { "Play" } =>
                {
                    transport_rect.contains_rect(text.galley.rect.translate(text.pos.to_vec2()))
                }
                _ => false,
            });
            assert!(
                label_visible,
                "focused rect {transport_rect:?} must contain its label at width {width}, running {running}"
            );
            let _ = render(width, running, Some(egui::Key::Enter));
            assert!(
                ctx.read_response(transport_id).unwrap().clicked(),
                "Enter activates the transport at width {width}, running {running}"
            );
        }
    }
}

#[test]
fn devices_popup_fits_narrow_viewports() {
    for width in [200.0, 240.0, 300.0, 720.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let state = stereo_state(true);
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 840.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_run_header(ui, ctx, &state, &theme);
                    });
                },
            )
        };
        let output = render(Vec::new());
        let devices = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let egui::Shape::Rect(shape) = &clipped.shape else {
                    return None;
                };
                (shape.brush.is_none() && shape.rect.size() == egui::vec2(32.0, 32.0))
                    .then_some(shape.rect)
            })
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .expect("Devices button");
        let pos = devices.center();
        let _ = render(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let output = render(Vec::new());
        for label in [
            "Input Device",
            "Output Device",
            "MIDI Input",
            "Refresh Devices",
        ] {
            let rect = output
                .shapes
                .iter()
                .find_map(|clipped| {
                    let egui::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    (text.galley.job.text == label)
                        .then_some(text.galley.rect.translate(text.pos.to_vec2()))
                })
                .unwrap_or_else(|| panic!("missing popup field {label} at width {width}"));
            assert!(
                rect.left() >= 12.0 && rect.right() <= width - 12.0,
                "{label}: {rect:?} at width {width}"
            );
        }
    }
}

#[test]
fn header_wraps_controls_and_truncates_long_names_without_overflow() {
    for width in [300.0, 400.0, 720.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let mut state = stereo_state(true);
        state.path = format!("/project/{}.onda", "long_name_".repeat(100));
        state.input_devices = vec!["Wide device name WWWWWWWWWWWWWWWWWWWWWW".to_owned()];
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 840.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let bounds = ui.max_rect();
                    let header = ui.scope(|ui| app.render_run_header(ui, ctx, &state, &theme));
                    assert!(
                        ui.min_rect().right() <= bounds.right() + 1.0,
                        "width {width}, content {:?}, bounds {bounds:?}",
                        ui.min_rect()
                    );
                    assert!(
                        header.response.rect.height() < 420.0,
                        "width {width}: {:?}",
                        ui.min_rect()
                    );
                });
            },
        );
    }
}

#[test]
fn stacked_header_utilities_wrap_inside_the_gutters() {
    for (width, rows) in [(128.0, 2), (144.0, 2), (160.0, 1)] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        for _ in 0..2 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 840.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_run_header(ui, ctx, &stereo_state(true), &theme);
                    });
                },
            );
            let header = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape)
                        if shape.corner_radius == crate::SECTION_CORNER_RADIUS.into() =>
                    {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .expect("header panel");
            let controls: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape)
                        if shape.brush.is_none() && shape.rect.size() == egui::vec2(32.0, 32.0) =>
                    {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(controls.len(), 3);
            let content = header.shrink2(egui::vec2(13.0, 9.0));
            assert!(
                controls.iter().all(|rect| content.contains_rect(*rect)),
                "width {width}: {controls:?} outside {content:?}"
            );
            assert_eq!(
                controls
                    .iter()
                    .map(|rect| rect.top().round() as i32)
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                rows,
                "width {width}: {controls:?}"
            );
        }
    }
}

#[test]
fn header_controls_have_consistent_sizes_and_alignment() {
    for (midi_available, audio_available) in [(false, true), (true, true), (true, false)] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let mut state = stereo_state(midi_available);
        if !audio_available {
            state.input_devices.clear();
            state.output_devices.clear();
        }
        let status = super::format_run_status(
            &state.status,
            app.options.sample_rate_hz,
            app.options.block_samples,
        );
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(720.0, 840.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_run_header(ui, ctx, &state, &theme);
                    });
                },
            )
        };
        let output = render(Vec::new());
        let text_rect = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing header text: {label}"))
        };
        let header = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape)
                    if shape.corner_radius == crate::SECTION_CORNER_RADIUS.into() =>
                {
                    Some(shape.rect)
                }
                _ => None,
            })
            .expect("header panel");
        assert!(header.height() < 90.0, "{header:?}");
        let mut controls: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape)
                    if shape.brush.is_none()
                        && (shape.rect.width() - 32.0).abs() < 1.0
                        && (shape.rect.height() - 32.0).abs() < 1.0 =>
                {
                    Some(shape.rect)
                }
                _ => None,
            })
            .collect();
        controls.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert_eq!(controls.len(), 3, "{controls:?}");
        let transport = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape)
                    if (shape.rect.width() - 72.0).abs() < 1.0
                        && (shape.rect.height() - 44.0).abs() < 1.0 =>
                {
                    Some(shape.rect)
                }
                _ => None,
            })
            .expect("labeled transport button");
        for rect in &controls {
            assert!(
                (rect.center().y - transport.center().y).abs() < 1.0,
                "{rect:?}"
            );
        }
        assert!(transport.right() + 24.0 <= controls[0].left() + 1.0);
        assert!(transport.contains(text_rect(&output, "Stop").center()));
        let version = text_rect(&output, env!("CARGO_PKG_VERSION"));
        let logo = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(shape) if shape.brush.is_some() => Some(shape.rect),
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds()),
                _ => None,
            })
            .expect("the loaded header logo is rendered");
        assert!(
            (logo.left().min(version.left()) - header.left() - 13.0).abs() < 1.0
                && (version.top() - logo.bottom() - 4.0).abs() < 1.0
                && (version.center().x - logo.center().x).abs() < 1.0
                && ((logo.top() + version.bottom()) / 2.0 - controls[0].center().y).abs() < 1.0,
            "logo {logo:?}, version {version:?}"
        );
        assert!(
            (controls[2].right() - header.right() + 13.0).abs() < 1.0,
            "toolbar {:?}, header {header:?}",
            controls[2]
        );
        for label in ["orbit_flanger.onda", &status] {
            let rect = text_rect(&output, label);
            assert!(
                rect.left() >= version.right().max(logo.right()) + 32.0
                    && rect.right() <= transport.left() - 16.0 + 1.0,
                "{label}: {rect:?}, header {header:?}"
            );
        }
        let filename = text_rect(&output, "orbit_flanger.onda");
        let status = text_rect(&output, &status);
        assert!(
            (filename.left() - status.left()).abs() < 1.0
                && ((filename.top() + status.bottom()) / 2.0 - transport.center().y).abs() < 2.0,
            "filename {filename:?}, status {status:?}, transport {:?}",
            transport
        );
        let click = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = render(click(controls[0].center()));
        let settings = render(Vec::new());
        assert!(!settings.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.job.text == "Audio & MIDI"
        )));
        text_rect(&settings, "Refresh Devices");
        if audio_available {
            text_rect(&settings, "Default input");
            text_rect(&settings, "Default output");
        }
        if midi_available {
            text_rect(&settings, "Computer Keyboard");
        }
        if audio_available {
            let center = text_rect(&settings, "Default input").center();
            let combo = settings
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if shape.rect.contains(center) => Some(shape.rect),
                    _ => None,
                })
                .min_by(|a, b| a.area().total_cmp(&b.area()))
                .expect("input selector");
            let _ = render(click(combo.center()));
            let options = render(Vec::new());
            text_rect(&options, "Default");
        }
    }
}

#[test]
fn devices_popup_scrolls_in_short_windows_and_uses_matching_control_heights() {
    let ctx = egui::Context::default();
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let viewport = egui::vec2(720.0, 180.0);
    let mut open = true;
    let devices = vec!["Device".to_owned()];
    let mut selectors = Vec::new();
    let mut refresh = egui::Rect::NOTHING;
    let mut render = |events| {
        selectors.clear();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    super::device_settings_button(
                        ui,
                        egui::Id::new("test-devices"),
                        &mut open,
                        |ui| {
                            for label in ["Input Device", "Output Device", "MIDI Input"] {
                                selectors.push(
                                    super::render_device_combo(
                                        ui,
                                        label,
                                        theme.muted_text,
                                        None,
                                        &devices,
                                        Some("Device"),
                                        |_| {},
                                    )
                                    .rect,
                                );
                                ui.add_space(8.0);
                            }
                            refresh = ui
                                .add_sized(
                                    [ui.available_width(), 32.0],
                                    egui::Button::new("Refresh Devices"),
                                )
                                .rect;
                        },
                    );
                });
            },
        );
        (output, selectors.clone(), refresh)
    };
    let _ = render(Vec::new());
    let (output, selectors, refresh) = render(Vec::new());
    assert!(selectors.iter().all(|rect| rect.height() == 32.0));
    assert_eq!(refresh.height(), 32.0);
    fn popup_frame(shape: &egui::Shape) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Rect(shape)
                if shape.corner_radius == crate::SECTION_CORNER_RADIUS.into()
                    && shape.stroke.width > 0.0 =>
            {
                Some(shape.rect)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(popup_frame),
            _ => None,
        }
    }
    let panel = output
        .shapes
        .iter()
        .find_map(|clipped| popup_frame(&clipped.shape))
        .expect("device popup frame");
    assert!(
        egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)
            .shrink(12.0)
            .contains_rect(panel),
        "{panel:?}"
    );
    let mut output = render(vec![
        egui::Event::PointerMoved(panel.center()),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -500.0),
            modifiers: egui::Modifiers::NONE,
        },
    ])
    .0;
    for _ in 0..10 {
        output = render(Vec::new()).0;
    }
    let refresh_visible = output.shapes.iter().any(|clipped| match &clipped.shape {
        egui::Shape::Text(text) if text.galley.job.text == "Refresh Devices" => clipped
            .clip_rect
            .contains_rect(text.galley.rect.translate(text.pos.to_vec2())),
        _ => false,
    });
    assert!(refresh_visible, "scrolling must reveal Refresh completely");
}

#[test]
fn transport_shortcuts_require_focus_and_modifiers_and_consume_repeats() {
    use egui::{Key, Modifiers};
    for (key, modifiers, repeat, focused, expected, consumed) in [
        (Key::S, Modifiers::CTRL, false, true, Some(true), true),
        (Key::Enter, Modifiers::CTRL, false, true, Some(true), true),
        (Key::Period, Modifiers::CTRL, false, true, Some(false), true),
        (Key::S, Modifiers::COMMAND, false, true, Some(true), true),
        (
            Key::Enter,
            Modifiers::COMMAND,
            false,
            true,
            Some(true),
            true,
        ),
        (
            Key::Period,
            Modifiers::COMMAND,
            false,
            true,
            Some(false),
            true,
        ),
        (
            Key::S,
            Modifiers::CTRL | Modifiers::SHIFT,
            false,
            true,
            Some(true),
            true,
        ),
        (Key::S, Modifiers::NONE, false, true, None, false),
        (Key::Enter, Modifiers::NONE, false, true, None, false),
        (Key::Period, Modifiers::NONE, false, true, None, false),
        (
            Key::S,
            Modifiers::CTRL | Modifiers::ALT,
            false,
            true,
            None,
            false,
        ),
        (Key::S, Modifiers::CTRL, false, false, None, false),
        (Key::S, Modifiers::CTRL, true, true, None, true),
        (Key::Period, Modifiers::CTRL, true, true, None, true),
    ] {
        let ctx = egui::Context::default();
        if repeat {
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |_| {},
            );
        }
        let _ = ctx.run(
            egui::RawInput {
                focused,
                modifiers,
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                assert_eq!(
                    super::take_transport_shortcut(ctx),
                    expected,
                    "{key:?}, {modifiers:?}, repeat {repeat}, focused {focused}"
                );
                assert_eq!(
                    ctx.input(|input| {
                        input.events.iter().any(|event|
                matches!(event, egui::Event::Key { key: found, pressed: true, .. } if *found == key)
            )
                    }),
                    !consumed
                );
            },
        );
    }
}

#[test]
fn svg_logos_rasterize_at_the_display_size_after_unloading_and_scaling() {
    for dark in [false, true] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(dark);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        for density in [1.0, 1.5, 2.0, 1.0] {
            ctx.set_pixels_per_point(density);
            for loaded in [true, false, true] {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(720.0, 840.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            if loaded {
                                app.render_run_header(ui, ctx, &stereo_state(false), &theme);
                            } else {
                                app.render_file_picker(ui, &theme, false);
                            }
                        });
                    },
                );
                let texture_id = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Mesh(mesh) => Some(mesh.texture_id),
                        egui::Shape::Rect(shape) => {
                            shape.brush.as_ref().map(|brush| brush.fill_texture_id)
                        }
                        _ => None,
                    })
                    .expect("the run logo is rendered");
                let manager = ctx.tex_manager();
                let manager = manager.read();
                let texture = manager.meta(texture_id).expect("logo texture");
                let expected = ((if loaded { 32.0 } else { 72.0 }) * density).round() as usize;
                assert_eq!(
                    texture.size,
                    [expected, expected],
                    "loaded: {loaded}, display scale: {density}, texture: {}",
                    texture.name
                );
            }
        }
    }
}

#[test]
fn utility_focus_survives_header_wrapping() {
    for tab_count in [2, 3, 4] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        let state = stereo_state(true);
        let mut render = |width, tab| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 840.0),
                    )),
                    events: if tab {
                        [true, false]
                            .map(|pressed| egui::Event::Key {
                                key: egui::Key::Tab,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            })
                            .to_vec()
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_run_header(ui, ctx, &state, &theme);
                    });
                },
            )
        };
        let _ = render(720.0, false);
        for _ in 0..tab_count {
            let _ = render(720.0, true);
        }
        let focused = ctx.memory(|m| m.focused()).unwrap();

        for width in [400.0, 240.0, 160.0, 400.0, 720.0] {
            for _ in 0..2 {
                let _ = render(width, false);
            }
            assert_eq!(
                ctx.memory(|m| m.focused()),
                Some(focused),
                "action {tab_count} must retain focus at width {width}"
            );
            assert!(
                ctx.read_response(focused).is_some(),
                "action {tab_count} must retain its widget identity at width {width}"
            );
        }
    }
}

#[test]
fn short_windows_keep_a_full_knob_row_visible_with_the_compact_scope() {
    for width in [540.0, 720.0] {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let theme = RunTheme::from_dark_mode(true);
        apply_run_theme(&ctx, &theme);
        let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
        for _ in 0..3 {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 620.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.render_run_panels(ctx, stereo_state(true), &theme),
            );
            let scope = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if shape.fill == theme.scope_background => {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(scope.height(), 96.0);
            let cards: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(shape) if (shape.rect.height() - 140.0).abs() < 0.1 => {
                        Some((shape.rect, clipped.clip_rect))
                    }
                    _ => None,
                })
                .collect();
            let top = cards[0].0.top();
            for (rect, clip) in cards.into_iter().filter(|(rect, _)| rect.top() == top) {
                assert!(
                    clip.contains_rect(rect),
                    "first knob row clipped at width {width}: {rect:?}, {clip:?}"
                );
            }
        }
    }
}

#[test]
fn folding_midi_releases_holds_and_preserves_computer_playback_while_folded() {
    let ctx = egui::Context::default();
    let theme = RunTheme::from_dark_mode(true);
    apply_run_theme(&ctx, &theme);
    let mut app = RunApp::new(None, RunHostOptions::default(), None, ParamLayout::Knobs);
    let mut toggle = egui::Pos2::ZERO;
    let mut expanded_height = 0.0;
    for frame in 0..7 {
        let events = match frame {
            2 => [true, false]
                .map(|pressed| egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .to_vec(),
            3 | 4 => vec![
                egui::Event::PointerMoved(toggle),
                egui::Event::PointerButton {
                    pos: toggle,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            _ => Vec::new(),
        };
        if frame == 4 || frame == 6 {
            app.computer_notes.insert(egui::Key::A, 60);
        }
        let mut dock_height = 0.0;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(720.0, 620.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                dock_height = egui::TopBottomPanel::bottom("midi")
                    .frame(egui::Frame::default().inner_margin(egui::Margin::symmetric(12, 10)))
                    .show(ctx, |ui| app.render_midi_keyboard(ui, &theme))
                    .response
                    .rect
                    .height();
            },
        );
        if frame == 2 {
            let focused = ctx.memory(|memory| memory.focused()).unwrap();
            toggle = ctx.read_response(focused).unwrap().rect.center();
            expanded_height = dock_height;
        }
        if frame >= 4 {
            assert!(!output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::Shape::Text(text) if text.galley.job.text == "Octave")));
            if frame == 4 {
                assert!(app.computer_notes.is_empty());
            }
            if frame >= 5 {
                assert!(dock_height < expanded_height - 100.0);
            }
            if frame == 6 {
                assert_eq!(app.computer_notes.get(&egui::Key::A), Some(&60));
            }
        }
    }
}
