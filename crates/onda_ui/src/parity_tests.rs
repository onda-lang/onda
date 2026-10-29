use crate::{
    editing::Field,
    scene::{Palette, Rect, Target},
    Input, Key, RunUi,
};
use serde_json::json;

fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.message(
        json!({"type":"state","state":{"path":"parity.onda","running":true,
        "supportsScope":false,"supportsTransport":true,"supportsDeviceSelection":true,"supportsViewState":true,
        "inputDevices":["Input A","Input B"],"outputDevices":["Output A"],
        "params":[{"name":"gain","type":"f64","value":0.5,"default":0.25,
            "rangeMin":0,"rangeMax":1,"scale":"linear","step":0.01,"stepCount":100}]}}),
    );
    ui.resize(720.0, 840.0, 1.0);
    ui.prepare_scene();
    ui.take_commands();
    ui
}
fn rect(ui: &mut RunUi, predicate: impl Fn(&Target) -> bool) -> [f32; 4] {
    ui.prepare_scene();
    ui.scene
        .hits
        .iter()
        .find(|h| predicate(&h.target) && h.rect.0[2] > 0.0 && h.rect.0[3] > 0.0)
        .expect("visible target")
        .rect
        .0
}
fn down(ui: &mut RunUi, r: [f32; 4]) {
    ui.input(Input::PointerDown {
        x: r[0] + r[2] * 0.5,
        y: r[1] + r[3] * 0.5,
        clicks: 1,
    });
}
fn click(ui: &mut RunUi, predicate: impl Fn(&Target) -> bool) {
    let r = rect(ui, predicate);
    down(ui, r);
    ui.input(Input::PointerUp);
}
fn key(ui: &mut RunUi, key: Key) {
    ui.input(Input::Key { key, shift: false });
}
fn control_field(t: &Target) -> bool {
    matches!(t, Target::Field(Field::Control(0)))
}

#[test]
fn midi_velocity_defaults_and_resets_to_one_without_overriding_saved_values() {
    use crate::editing::Setting;
    for view in [
        None,
        Some(json!({"sections":{"params":false}})),
        Some(json!({"reset":true})),
    ] {
        let mut ui = fixture();
        let mut state = json!({"midi":{"noteOn":true,"noteOff":true}});
        if let Some(view) = view {
            state["viewState"] = view;
        }
        ui.message(json!({"type":"state","state":state}));
        click(&mut ui, |t| *t == Target::Note(60));
        let commands = ui.take_commands();
        assert_eq!(
            commands
                .iter()
                .find(|c| c["type"] == "midiNote" && c["pressed"] == true)
                .unwrap()["velocity"],
            1.0
        );
        ui.set_field(&Field::Setting(Setting::Velocity), json!(0.25), true);
        let r = rect(&mut ui, |t| {
            *t == Target::Field(Field::Setting(Setting::Velocity))
        });
        ui.input(Input::PointerDown {
            x: r[0] + r[2] * 0.5,
            y: r[1] + r[3] * 0.5,
            clicks: 2,
        });
        ui.input(Input::PointerUp);
        assert_eq!(
            ui.interaction.midi().velocity,
            1.0,
            "double-click must restore the same default"
        );
    }
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"viewState":{"velocity":0.0}}}));
    assert_eq!(
        ui.interaction.midi().velocity,
        0.0,
        "an explicit saved zero is a valid preference"
    );
}

#[test]
fn compact_headers_keep_the_source_readable_and_actions_separate() {
    for width in [240.0, 320.0, 360.0, 400.0, 480.0, 720.0] {
        for transport in [false, true] {
            let mut ui = fixture();
            ui.resize(width, 840.0, 1.0);
            ui.message(json!({"type":"state","state":{
                "path":"full-ui.onda","supportsTransport":transport,
                "supportsSourceSelection":true,"supportsProjectExport":true,"canExportProject":true
            }}));
            ui.prepare_scene();
            let header = ui.scene.shapes[0].rect;
            let name = ui
                .scene
                .labels
                .iter()
                .find(|l| l.text == "full-ui.onda")
                .unwrap();
            assert!(name.width >= 100.0, "source disappeared at {width}px");
            let bounds = Rect([name.x, name.y, name.width, name.height.unwrap()]);
            let actions: Vec<_> = ui
                .scene
                .hits
                .iter()
                .filter(|h| {
                    matches!(&h.target, Target::Devices | Target::Transport)
                        || matches!(&h.target,
                Target::Command(c) if c["type"] == "saveProjectAs" || c["type"] == "unload")
                })
                .collect();
            for (i, action) in actions.iter().enumerate() {
                assert!(action.rect.0[0] >= header[0]);
                assert!(action.rect.0[0] + action.rect.0[2] <= header[0] + header[2]);
                assert!(action.rect.0[1] + action.rect.0[3] <= header[1] + header[3]);
                for other in actions[i + 1..].iter().map(|h| h.rect).chain([bounds]) {
                    let Rect(r) = action.rect.intersect(other);
                    assert!(
                        r[2] == 0.0 || r[3] == 0.0,
                        "overlapping header at {width}px"
                    );
                }
            }
        }
    }
}

#[test]
fn event_schema_changes_discard_focused_actions() {
    for replacement in [json!([]), json!([{"name":"other","args":[]}])] {
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{"events":[{"name":"bang","args":[]}]}}));
        click(&mut ui, |t| *t == Target::Trigger(0));
        ui.take_commands();
        ui.message(json!({"type":"state","state":{"events":replacement}}));
        key(&mut ui, Key::Enter);
        assert!(!ui
            .take_commands()
            .iter()
            .any(|c| c["type"] == "triggerEvent"));
    }
}

#[test]
fn integer_drags_clamp_and_reverse_at_each_scalar_boundary() {
    for (ty, maximum) in [("i32", i64::from(i32::MAX)), ("i64", i64::MAX)] {
        for upper in [false, true] {
            let bound = if upper { maximum } else { -maximum - 1 };
            let direction = if upper { 1.0 } else { -1.0 };
            let start = if upper { bound - 1 } else { bound + 1 };
            let mut ui = fixture();
            ui.message(json!({"type":"state","state":{"params":[{"name":"count","type":ty,"value":start.to_string()}]}}));
            let r = rect(&mut ui, control_field);
            down(&mut ui, r);
            let x = r[0] + r[2] * 0.5;
            for (distance, expected) in [(20.0, bound), (16.0, start)] {
                ui.input(Input::PointerMove {
                    x: x + direction * distance,
                    y: r[1],
                    fine: false,
                });
                assert_eq!(
                    ui.controls[0].value.to_string().trim_matches('"'),
                    expected.to_string()
                );
            }
            ui.input(Input::PointerUp);
        }
    }
}

#[test]
fn layouts_survive_status_updates_and_saved_presentation() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"paramLayout":"sliders"}}));
    click(&mut ui, |t| {
        *t == Target::Layout {
            input: false,
            knobs: true,
        }
    });
    let saved = ui
        .take_commands()
        .into_iter()
        .find(|c| c["type"] == "viewState")
        .expect("layout is saved");
    ui.message(json!({"type":"state","state":{"status":"Ready"}}));
    assert!(ui.knobs);
    let mut restored = RunUi::default();
    restored
        .message(json!({"type":"state","state":{"path":"parity.onda","viewState":saved["state"]}}));
    assert!(restored.knobs);
}

#[test]
fn shift_arrows_select_text_for_replacement() {
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"params":[{"name":"count","type":"i64","value":"12345"}]}}),
    );
    ui.begin_field(Field::Control(0));
    key(&mut ui, Key::Home);
    for _ in 0..2 {
        ui.input(Input::Key {
            key: Key::ArrowRight,
            shift: true,
        });
    }
    ui.input(Input::Text { text: "9".into() });
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, "9345");
}

#[test]
fn copying_and_cutting_use_the_current_selection() {
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"params":[{"name":"count","type":"i64","value":"12345"}]}}),
    );
    ui.begin_field(Field::Control(0));
    key(&mut ui, Key::Copy);
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "copyText" && c["text"] == "12345"));
    key(&mut ui, Key::Home);
    for _ in 0..2 {
        ui.input(Input::Key {
            key: Key::ArrowRight,
            shift: true,
        });
    }
    key(&mut ui, Key::Cut);
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "copyText" && c["text"] == "12"));
    assert_eq!(ui.editor.draft().unwrap().text, "345");
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, "345");
}

#[test]
fn touch_pans_the_document_without_editing_a_number() {
    let mut ui = fixture();
    ui.resize(390.0, 700.0, 1.0);
    ui.message(json!({"type":"state","state":{"params":(0..80).map(|i| json!({"name":format!("p{i}"),"type":"i64","value":"0"})).collect::<Vec<_>>()}}));
    let r = rect(&mut ui, control_field);
    let x = r[0] + r[2] * 0.5;
    let y = r[1] + r[3] * 0.5;
    ui.input(Input::TouchStart { id: 0, x, y });
    ui.input(Input::TouchMove {
        id: 0,
        x,
        y: y - 100.0,
        delta_y: None,
    });
    ui.input(Input::TouchEnd { id: 0 });
    assert_eq!(ui.scroll, 100.0);
    assert!(!ui.editing());
    assert!(!ui.take_commands().iter().any(|c| c["type"] == "setParam"));
    ui.input(Input::Scroll { delta: -100.0 });
    ui.input(Input::TouchStart { id: 0, x, y });
    ui.input(Input::TouchEnd { id: 0 });
    assert!(ui.editing(), "a tap still enters the field");
}

#[test]
fn touch_controls_and_log_panning_keep_their_owner() {
    let mut ui = fixture();
    let r = rect(&mut ui, |t| *t == Target::Control(0));
    let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
    ui.input(Input::TouchStart { id: 0, x, y });
    ui.input(Input::TouchMove {
        id: 0,
        x: x + 50.0,
        y,
        delta_y: None,
    });
    ui.input(Input::TouchEnd { id: 0 });
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "setParam" && c["commit"] == true));
    ui.message(json!({"type":"state","state":{"logText":"line\n".repeat(100)}}));
    ui.prepare_scene();
    let [x, y, _, _] = ui.scene.log_rect;
    ui.input(Input::TouchStart {
        id: 0,
        x: x + 40.0,
        y: y + 40.0,
    });
    ui.input(Input::TouchMove {
        id: 0,
        x: x + 40.0,
        y: y + 240.0,
        delta_y: None,
    });
    ui.input(Input::TouchEnd { id: 0 });
    assert!(!ui.presentation.log_follow);
    assert_eq!(
        ui.scroll, 0.0,
        "the gesture stays with the log outside its rectangle"
    );
}

#[test]
fn invalid_draft_cannot_block_stop() {
    let mut ui = fixture();
    click(&mut ui, control_field);
    ui.input(Input::Text { text: "-".into() });
    key(&mut ui, Key::Enter);
    click(&mut ui, |t| *t == Target::Transport);
    assert!(ui.take_commands().iter().any(|v| v["type"] == "stop"));
    assert!(!ui.editing());
}
#[test]
fn transport_is_one_button_that_tracks_playback() {
    let mut ui = fixture();
    for running in [true, false] {
        ui.message(json!({"type":"state","state":{"running":running}}));
        ui.prepare_scene();
        let count = ui
            .scene
            .hits
            .iter()
            .filter(|h| h.target == Target::Transport)
            .count();
        assert_eq!(count, 1);
        click(&mut ui, |t| *t == Target::Transport);
        assert!(ui
            .take_commands()
            .iter()
            .any(|command| { command["type"] == if running { "stop" } else { "start" } }));
    }
}

#[test]
fn focused_transport_tracks_playback_in_both_directions() {
    let mut ui = fixture();
    key(&mut ui, Key::Tab);
    for running in [true, false, true] {
        ui.message(json!({"type":"state","state":{"running":running}}));
        key(&mut ui, Key::Enter);
        assert!(ui
            .take_commands()
            .iter()
            .any(|command| { command["type"] == if running { "stop" } else { "start" } }));
    }
    key(&mut ui, Key::Tab);
    assert_eq!(ui.interaction.focus().cloned(), Some(Target::Devices));
}
#[test]
fn numeric_fields_drag_horizontally_and_click_to_edit() {
    let mut ui = fixture();
    let r = rect(&mut ui, control_field);
    down(&mut ui, r);
    ui.input(Input::PointerMove {
        x: r[0] + r[2] * 0.5 + 75.0,
        y: r[1] + r[3] * 0.5,
        fine: false,
    });
    ui.input(Input::PointerUp);
    assert!((ui.controls[0].value.as_f64().unwrap() - 0.7).abs() < 1e-12);
    assert!(!ui.editing());
    let commands = ui.take_commands();
    assert!(commands
        .iter()
        .any(|v| v["type"] == "setParam" && v["commit"] == false));
    assert!(commands
        .iter()
        .any(|v| v["type"] == "setParam" && v["commit"] == true));
    click(&mut ui, control_field);
    assert!(ui.editing());
}
#[test]
fn dragging_a_range_or_number_does_not_hover_widgets_underneath() {
    for number in [false, true] {
        let mut ui = fixture();
        let source = rect(&mut ui, |t| {
            if number {
                control_field(t)
            } else {
                *t == Target::Control(0)
            }
        });
        let button = rect(&mut ui, |t| *t == Target::Devices);
        down(&mut ui, source);
        ui.input(Input::PointerMove {
            x: button[0] + button[2] * 0.5,
            y: button[1] + button[3] * 0.5,
            fine: false,
        });
        ui.prepare_scene();
        assert!(!ui.scene.hovered(&ui, button), "number={number}");
        ui.input(Input::PointerUp);
        ui.prepare_scene();
        assert!(ui.scene.hovered(&ui, button), "hover resumes after release");
    }
}
#[test]
fn smoothing_drags_match_legacy_speed_precision_and_zero_clamping() {
    use crate::editing::Setting;
    for setting in [Setting::ParamSmoothing, Setting::InputSmoothing] {
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{
            "supportsParamSmoothing":true,"paramSmoothingMs":30.0,
            "supportsInputControlSmoothing":true,"inputControlSmoothingMs":30.0,
            "inputControls":[{"name":"input","type":"f64","value":0.5,"rangeMin":0,"rangeMax":1}]
        }}));
        let field = Field::Setting(setting);
        let r = rect(&mut ui, |t| *t == Target::Field(field.clone()));
        let start = r[0] + r[2] * 0.5;
        down(&mut ui, r);
        for (x, expected) in [
            (start + 50.0, 80.0),
            (start - 200.0, 0.0),
            (start - 199.0, 1.0),
        ] {
            ui.input(Input::PointerMove {
                x,
                y: r[1] + 10.0,
                fine: false,
            });
            assert_eq!(ui.field_value(&field), json!(expected));
        }
        ui.input(Input::PointerUp);
        assert_eq!(
            ui.take_commands()
                .iter()
                .filter(|v| {
                    v["type"] == "setParamSmoothing" || v["type"] == "setInputControlSmoothing"
                })
                .count(),
            3,
            "only changed whole-millisecond values are published"
        );
    }
}
#[test]
fn smoothing_keyboard_edits_use_whole_milliseconds_and_preserve_exact_reset() {
    use crate::editing::Setting;
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"supportsParamSmoothing":true,
        "paramSmoothingMs":30.0,"paramSmoothingDefaultMs":30.4}}),
    );
    let field = Field::Setting(Setting::ParamSmoothing);
    click(&mut ui, |t| *t == Target::Field(field.clone()));
    key(&mut ui, Key::ArrowUp);
    key(&mut ui, Key::Enter);
    assert_eq!(ui.field_value(&field), json!(31.0));
    click(&mut ui, |t| *t == Target::Field(field.clone()));
    ui.input(Input::Text {
        text: "15.5".into(),
    });
    key(&mut ui, Key::Enter);
    assert_eq!(ui.field_value(&field), json!(16.0));
    let r = rect(&mut ui, |t| *t == Target::Field(field.clone()));
    ui.input(Input::PointerDown {
        x: r[0] + 10.0,
        y: r[1] + 10.0,
        clicks: 2,
    });
    ui.input(Input::PointerUp);
    assert_eq!(ui.field_value(&field), json!(30.4));
}
#[test]
fn slider_click_uses_the_track_position() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"paramLayout":"sliders"}}));
    let r = rect(&mut ui, |t| matches!(t, Target::Control(0)));
    ui.input(Input::PointerDown {
        x: r[0] + r[2] - 10.0,
        y: r[1] + r[3] * 0.5,
        clicks: 1,
    });
    ui.input(Input::PointerUp);
    assert_eq!(ui.controls[0].value, json!(1.0));
}
#[test]
fn arrow_keys_use_the_declared_step() {
    let mut ui = fixture();
    click(&mut ui, control_field);
    key(&mut ui, Key::ArrowUp);
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, json!(0.51));
}
#[test]
fn disabled_external_inputs_cannot_be_edited_through_tab() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"params":[],"inputControls":[{"name":"external","enabled":false,"type":"f64","value":0.5,"rangeMin":0,"rangeMax":1}]}}));
    key(&mut ui, Key::Tab);
    ui.input(Input::Text {
        text: "0.75".into(),
    });
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, json!(0.5));
    assert!(!ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setInputControl"));
}
#[test]
fn settings_hover_does_not_highlight_controls_beneath_the_popup() {
    for light in [false, true] {
        let mut ui = fixture();
        ui.resize(420.0, 840.0, 1.0);
        ui.message(json!({"type":"state","state":{
            "themeMode":if light {"light"} else {"dark"},
            "supportsDeviceSelection":false,"supportsMidiInputSelection":true,
            "supportsRunSettings":true,"canSetSampleRate":true,"canSetBlockSize":true,
            "sampleRateHz":48000,"blockSamples":512,
            "midiInputDevices":["Computer Keyboard"],
            "params":(0..3).map(|i|json!({"name":format!("gain_{i}"),"type":"f64","value":0.5})).collect::<Vec<_>>()
        }}));
        let covered = rect(&mut ui, |t| *t == Target::Field(Field::Control(1)));
        click(&mut ui, |t| *t == Target::Devices);
        ui.prepare_scene();
        let document = |ui: &RunUi| {
            ui.scene.shapes[..ui.scene.overlay_shapes]
                .iter()
                .map(|s| (s.rect, s.color, s.params))
                .collect::<Vec<_>>()
        };
        let before = document(&ui);
        let (x, y) = (covered[0] + covered[2] * 0.5, covered[1] + covered[3] * 0.5);
        assert!(Rect(ui.scene.popup_rect).contains(x, y));
        ui.input(Input::PointerMove { x, y, fine: false });
        ui.prepare_scene();
        assert!(
            before == document(&ui),
            "covered controls changed on hover, light={light}"
        );
        assert!(!matches!(
            ui.target(x, y),
            Some(Target::Field(Field::Control(_)))
        ));
        let refresh = rect(
            &mut ui,
            |t| matches!(t, Target::Command(c) if c["type"] == "refreshDevices"),
        );
        ui.input(Input::PointerMove {
            x: refresh[0] + refresh[2] * 0.5,
            y: refresh[1] + refresh[3] * 0.5,
            fine: false,
        });
        ui.prepare_scene();
        assert!(
            ui.scene.shapes[ui.scene.overlay_shapes..].iter().any(|s| {
                s.color == Palette::new(light).accent_soft
                    && Rect(refresh).contains(s.rect[0], s.rect[1])
            }),
            "popup buttons retain their own hover, light={light}"
        );

        click(&mut ui, |t| {
            *t == Target::Choose(crate::interaction::SelectId::SampleRate)
        });
        ui.prepare_scene();
        let parent: Vec<_> = ui.scene.shapes[..ui.scene.menu_shapes]
            .iter()
            .map(|s| (s.rect, s.color, s.params))
            .collect();
        let option = rect(&mut ui, |t| *t == Target::Option(0));
        ui.input(Input::PointerMove {
            x: option[0] + option[2] * 0.5,
            y: option[1] + option[3] * 0.5,
            fine: false,
        });
        ui.prepare_scene();
        assert!(
            parent
                == ui.scene.shapes[..ui.scene.menu_shapes]
                    .iter()
                    .map(|s| (s.rect, s.color, s.params))
                    .collect::<Vec<_>>(),
            "dropdown hover changes its parent popup, light={light}"
        );
        assert_eq!(ui.interaction.menu().unwrap().highlighted, 0);
        key(&mut ui, Key::Escape);
        assert!(ui.interaction.settings_open());
        key(&mut ui, Key::Escape);
        assert!(!ui.interaction.settings_open());
        ui.prepare_scene();
        let normal = document(&ui);
        ui.input(Input::PointerMove { x, y, fine: false });
        ui.prepare_scene();
        assert!(
            normal != document(&ui),
            "hover resumes after closing, light={light}"
        );
    }
}

#[test]
fn popup_tab_never_focuses_background_fields() {
    let mut ui = fixture();
    click(&mut ui, |t| matches!(t, Target::Devices));
    key(&mut ui, Key::Tab);
    assert!(!ui
        .editor
        .draft()
        .is_some_and(|d| matches!(d.target, Field::Control(_))));
}
#[test]
fn settings_popup_aligns_with_header_edges() {
    let mut ui = fixture();
    ui.toggle_settings();
    for width in [240.0, 320.0, 720.0] {
        for height in [180.0, 840.0] {
            ui.resize(width, height, 1.0);
            for scroll in [0.0, 24.0] {
                ui.scroll = scroll;
                let scene = crate::layout::build(&ui);
                let header = scene.shapes[0].rect;
                let popup = scene.popup_rect;
                assert_eq!(popup[0] + popup[2], header[0] + header[2]);
                if header[1] + header[3] + 60.0 <= height - crate::theme::GUTTER {
                    assert_eq!(popup[1], header[1] + header[3]);
                } else {
                    assert!(
                        popup[1] < header[1] + header[3],
                        "short windows move settings up to keep them usable"
                    );
                }
                assert!(popup[0] >= header[0]);
                assert!(popup[1] + popup[3] <= height - crate::theme::GUTTER);
            }
        }
    }
}

#[test]
fn choices_do_not_expand_the_settings_body() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"inputDevices":(0..40).map(|n|format!("Input {n}")).collect::<Vec<_>>()}}));
    click(&mut ui, |t| matches!(t, Target::Devices));
    ui.prepare_scene();
    let height = ui.scene.popup_height;
    click(&mut ui, |t| matches!(t, Target::Choose(_)));
    ui.prepare_scene();
    assert_eq!(ui.scene.popup_height, height);
}
#[test]
fn unloaded_screen_requires_source_selection() {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "supportsSourceSelection":false,"supportsRunSettings":true
    }}));
    ui.prepare_scene();
    assert!(ui.scene.source_picker.is_none());
    assert!(ui.scene.labels.is_empty());
    assert!(ui.scene.hits.is_empty());
    assert!(ui.scene.shapes.is_empty());

    ui.message(json!({"type":"state","state":{"supportsSourceSelection":true}}));
    ui.prepare_scene();
    assert!(ui.scene.source_picker.is_some());
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|label| label.text.contains("drop an .onda")));

    ui.message(json!({"type":"state","state":{
        "supportsSourceSelection":false,"path":"main.onda"
    }}));
    ui.prepare_scene();
    assert!(ui.scene.source_picker.is_none());
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|label| label.text == "main.onda"));
}

#[test]
fn empty_picker_exposes_run_setting_dropdowns_inline() {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"supportsSourceSelection":true,"supportsRunSettings":true,"supportsFastMath":true,"sampleRateHz":48000,"blockSamples":512}}));
    ui.prepare_scene();
    assert!(
        ui.scene
            .hits
            .iter()
            .filter(|h| matches!(h.target, Target::Choose(_)))
            .count()
            >= 2
    );
    assert!(!ui
        .scene
        .hits
        .iter()
        .any(|h| matches!(h.target, Target::Devices)));
}
#[test]
fn unbounded_controls_have_no_knob_or_slider_and_units_are_visible() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"params":[{"name":"frequency","unit":"Hz","type":"f64","value":440.0}],"paramLayout":"knobs"}}));
    ui.prepare_scene();
    assert!(!ui
        .scene
        .hits
        .iter()
        .any(|h| matches!(h.target, Target::Control(_))));
    assert!(ui.scene.labels.iter().any(|l| l.text == "frequency (Hz)"));
}
#[test]
fn range_controls_change_visuals_on_hover() {
    for layout in ["knobs", "sliders"] {
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{"paramLayout":layout}}));
        let r = rect(&mut ui, |t| matches!(t, Target::Control(0)));
        let before: Vec<_> = ui.scene.shapes.iter().map(|s| s.color).collect();
        ui.input(Input::PointerMove {
            x: r[0] + r[2] * 0.5,
            y: r[1] + r[3] * 0.5,
            fine: false,
        });
        ui.prepare_scene();
        assert_ne!(
            before,
            ui.scene.shapes.iter().map(|s| s.color).collect::<Vec<_>>(),
            "{layout}"
        );
    }
}
#[test]
fn embedded_buffers_are_loaded_and_keep_a_symmetric_amplitude_scale() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"params":[],"buffers":[{"name":"clip","type":"f32","loadedFrames":128,"loadedChannels":1,"loadedPath":null,"waveform":{"minimums":[-0.1],"maximums":[0.1],"minValue":-0.1,"maxValue":0.1}}]}}));
    ui.prepare_scene();
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|l| l.text == "Embedded project asset"));
    assert!(ui.scene.labels.iter().any(|l| l.text == "Replace"));
    let p = Palette::new(false);
    let wave = ui
        .scene
        .shapes
        .iter()
        .find(|s| s.color == p.scope)
        .expect("waveform plot")
        .rect;
    let bars: Vec<_> = ui
        .scene
        .shapes
        .iter()
        .filter(|s| {
            s.color == p.accent
                && Rect(wave).contains(s.rect[0] + s.rect[2] * 0.5, s.rect[1] + s.rect[3] * 0.5)
        })
        .collect();
    assert!(!bars.is_empty());
    // Thin stems include their rounded caps and pixel snapping.
    assert!(bars.iter().all(|s| s.rect[3] <= wave[3] * 0.11 + 1.5));
}

#[test]
fn buffer_previews_draw_individual_samples_with_a_separate_scale_gutter() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"params":[],"buffers":[{"name":"clip","type":"f32",
        "waveform":{"minimums":[0.25,0.5,0.75],"maximums":[0.25,0.5,0.75],"minValue":0.25,"maxValue":0.75}}]}}));
    ui.prepare_scene();
    let p = Palette::new(false);
    let lines: Vec<_> = ui
        .scene
        .shapes
        .iter()
        .filter(|s| s.color == p.accent && s.params[1] == 2.0)
        .collect();
    assert_eq!(
        lines.len(),
        3,
        "each preview sample needs its own thin stem"
    );
    assert!(lines.iter().all(|s| s.rect[2] <= 1.5));
    let axis = ui.scene.labels.iter().find(|l| l.text == "-1").unwrap();
    assert!(
        lines.iter().all(|s| s.rect[0] > axis.x + axis.width),
        "samples must not cover scale labels"
    );
    let bottom = lines[0].rect[1] + lines[0].rect[3];
    assert!(
        lines
            .iter()
            .all(|s| (s.rect[1] + s.rect[3] - bottom).abs() < 0.01),
        "positive samples extend to zero"
    );
}

#[test]
fn scrollbar_thumb_keeps_its_grab_position_and_drags_to_both_ends() {
    for count in [30, 4096] {
        let mut ui = fixture();
        ui.resize(720.0, 400.0, 1.0);
        ui.message(json!({"type":"state","state":{"params":(0..count)
            .map(|i|json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>()}}));
        ui.input(Input::Scroll { delta: 250.0 });
        ui.prepare_scene();
        let thumb = ui
            .scene
            .shapes
            .iter()
            .find(|s| s.color == Palette::new(false).scrollbar)
            .unwrap()
            .rect;
        let (x, y) = (thumb[0] + thumb[2] * 0.5, thumb[1] + 3.0);
        let before = ui.scroll;
        ui.input(Input::PointerDown { x, y, clicks: 1 });
        assert_eq!(ui.scroll, before, "grabbing the thumb must not jump");
        assert_eq!(
            ui.cursor(),
            "default",
            "scrollbars must retain the arrow cursor"
        );
        ui.input(Input::PointerMove {
            x,
            y: y + 10.0,
            fine: false,
        });
        assert!(ui.scroll > before);
        ui.input(Input::PointerMove { x, y, fine: false });
        assert!(
            (ui.scroll - before).abs() < 0.01,
            "return to the same grab position"
        );
        ui.input(Input::PointerMove {
            x,
            y: 0.0,
            fine: false,
        });
        assert_eq!(ui.scroll, 0.0);
        assert_eq!(
            ui.cursor(),
            "default",
            "scrollbar drags retain the arrow outside the track"
        );
        ui.input(Input::PointerMove {
            x,
            y: 400.0,
            fine: false,
        });
        assert_eq!(
            ui.scroll,
            ui.scene.content_height - ui.scene.viewport_height
        );
        ui.input(Input::PointerUp);
    }
}

#[test]
fn event_numbers_use_compact_fields_and_readable_float_precision() {
    for width in [360.0, 1236.0] {
        let mut ui = fixture();
        ui.resize(width, 840.0, 1.0);
        ui.message(
            json!({"type":"state","state":{"params":[],"events":[{"name":"play",
            "args":[{"name":"start_frame","type":"f32","value":0.0}]}]}}),
        );
        let r = rect(&mut ui, |t| matches!(t, Target::Field(Field::Event(_))));
        assert!(
            r[2] <= 112.0,
            "scalar fields must not stretch with the window"
        );
        assert_eq!(r[3], 24.0);
        down(&mut ui, r);
        ui.input(Input::PointerMove {
            x: r[0] + r[2] * 0.5 + 13.0,
            y: r[1] + r[3] * 0.5,
            fine: false,
        });
        ui.input(Input::PointerUp);
        ui.prepare_scene();
        assert!(
            ui.scene.labels.iter().any(|l| l.text == "0.13"),
            "hide binary float artifacts"
        );
        assert_eq!(ui.events[0].args[0].value, json!(f64::from(0.13_f32)));
    }
}

#[test]
fn numeric_labels_use_control_steps_without_rounding_stored_values() {
    for (metadata, expected) in [
        (
            json!({"type":"f64","value":369.756134,"rangeMin":80,"rangeMax":9000,"scale":"log"}),
            "369.8",
        ),
        (json!({"type":"f32","value":f64::from(0.13_f32)}), "0.13"),
        (json!({"type":"f64","value":0.123456789}), "0.123"),
        (
            json!({"type":"f64","value":0.25,"rangeMin":0,"rangeMax":1,"step":0.25}),
            "0.25",
        ),
        (json!({"type":"f64","value":0.000123456}), "1.235e-4"),
        (json!({"type":"f64","value":1.23456789e12}), "1.235e12"),
        (
            json!({"type":"i64","value":"9223372036854775807"}),
            "9223372036854775807",
        ),
    ] {
        let value = metadata["value"].clone();
        let mut control = metadata;
        control["name"] = json!("precise");
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{"params":[control]}}));
        ui.prepare_scene();
        assert!(
            ui.scene.labels.iter().any(|l| l.text == expected),
            "expected {expected}"
        );
        assert_eq!(ui.controls[0].value, value);
    }
}

#[test]
fn released_drags_and_committed_edits_keep_full_precision_behind_compact_labels() {
    for bank in ["params", "inputControls"] {
        for layout in ["knobs", "sliders"] {
            for number in [false, true] {
                let mut ui = fixture();
                let mut state = json!({"params":[],"inputControls":[],"paramLayout":layout,"inputControlLayout":layout});
                state[bank] = json!([{"name":"cutoff","type":"f64","value":369.756134,
                    "rangeMin":80,"rangeMax":9000,"scale":"log","hostValueDomain":"plain"}]);
                ui.message(json!({"type":"state","state":state}));
                let source = rect(&mut ui, |t| {
                    if number {
                        *t == Target::Field(Field::Control(0))
                    } else {
                        *t == Target::Control(0)
                    }
                });
                down(&mut ui, source);
                let vertical = !number && layout == "knobs";
                ui.input(Input::PointerMove {
                    x: source[0] + source[2] * 0.5 + if vertical { 0.0 } else { 13.75 },
                    y: source[1] + source[3] * 0.5 - if vertical { 13.75 } else { 0.0 },
                    fine: false,
                });
                let dragged = ui.controls[0].value.clone();
                assert_ne!(dragged, json!(369.756134));
                ui.input(Input::PointerUp);
                assert_eq!(ui.controls[0].value, dragged);
                let command = ui
                    .take_commands()
                    .into_iter()
                    .find(|c| c["commit"] == true)
                    .unwrap();
                assert_eq!(command["value"], dragged);
                ui.prepare_scene();
                let r = rect(&mut ui, control_field);
                let label = ui
                    .scene
                    .labels
                    .iter()
                    .find(|l| Rect(r).contains(l.x, l.y))
                    .unwrap();
                let displayed: f64 = label.text.parse().unwrap();
                assert!((displayed - dragged.as_f64().unwrap()).abs() <= 0.050001);
                assert_ne!(
                    json!(displayed),
                    dragged,
                    "display rounding must not reach the processor"
                );

                click(&mut ui, control_field);
                ui.input(Input::Text {
                    text: "369.756134".into(),
                });
                assert_eq!(ui.editor.draft().unwrap().text, "369.756134");
                key(&mut ui, Key::Enter);
                assert_eq!(ui.controls[0].value, json!(369.756134));
                ui.prepare_scene();
                assert!(ui.scene.labels.iter().any(|l| l.text == "369.8"));
                click(&mut ui, control_field);
                key(&mut ui, Key::Enter);
                assert_eq!(ui.controls[0].value, json!(369.756134));
            }
        }
    }
}

#[test]
fn compact_numeric_drafts_keep_exact_values_until_the_user_edits_them() {
    let mut ui = fixture();
    let value = 0.123456789;
    ui.message(
        json!({"type":"state","state":{"params":[{"name":"precise","type":"f64","value":value}]}}),
    );
    click(&mut ui, control_field);
    assert_eq!(ui.editor.draft().unwrap().text, "0.12346");
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, value);
    click(&mut ui, control_field);
    key(&mut ui, Key::ArrowUp);
    key(&mut ui, Key::Enter);
    assert_eq!(ui.controls[0].value, json!(value + 0.001));
}

#[test]
fn scrolled_dropdown_selection_preserves_visible_parent_settings() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"inputDevices":(0..40).map(|i|format!("Input {i}")).collect::<Vec<_>>()}}));
    click(&mut ui, |t| matches!(t, Target::Devices));
    ui.prepare_scene();
    let parent_height = ui.scene.popup_height;
    click(&mut ui, |t| *t == Target::Choose(SelectId::AudioInput));
    ui.prepare_scene();
    let r = ui.scene.menu_rect;
    ui.input(Input::PointerMove {
        x: r[0] + 20.0,
        y: r[1] + 20.0,
        fine: false,
    });
    ui.input(Input::Scroll { delta: 800.0 });
    ui.prepare_scene();
    let selected = ui
        .scene
        .hits
        .iter()
        .rev()
        .find_map(|h| match h.target {
            Target::Option(i) if h.rect.0[3] > 10.0 => Some(i),
            _ => None,
        })
        .unwrap();
    click(&mut ui, |t| *t == Target::Option(selected));
    ui.prepare_scene();
    assert!(ui.interaction.menu().is_none());
    assert_eq!(ui.interaction.popup_scroll(), 0.0);
    assert_eq!(ui.scene.popup_height, parent_height);
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|l| l.text == "Audio input" && l.y >= ui.scene.popup_rect[1]));
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setInputDevice"));
}
#[test]
fn dropdown_keyboard_selection_and_escape_stay_in_the_popup() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    click(&mut ui, |t| matches!(t, Target::Devices));
    click(&mut ui, |t| *t == Target::Choose(SelectId::AudioInput));
    key(&mut ui, Key::End);
    key(&mut ui, Key::Enter);
    assert_eq!(ui.state["currentInputDevice"], json!("Input B"));
    assert!(ui.take_commands().iter().all(|v| v["type"] != "start"));
    assert!(ui.interaction.settings_open());
    click(&mut ui, |t| *t == Target::Choose(SelectId::AudioInput));
    key(&mut ui, Key::Escape);
    assert!(ui.interaction.menu().is_none());
    assert!(ui.interaction.settings_open());
}
#[test]
fn dropdown_typeahead_does_not_play_computer_keyboard_notes() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"midiKeyboardInteractive":true,"midi":{"noteOn":true}}}),
    );
    click(&mut ui, |t| matches!(t, Target::Devices));
    click(&mut ui, |t| *t == Target::Choose(SelectId::AudioInput));
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.input(Input::Text { text: "i".into() });
    assert!(ui.take_commands().iter().all(|v| v["type"] != "midiNote"));
    assert!(ui.interaction.midi().keyboard.is_empty());
}
#[test]
fn opening_a_dropdown_keeps_its_selected_item_visible_in_a_short_window() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    ui.resize(360.0, 400.0, 1.0);
    ui.message(json!({"type":"state","state":{"inputDevices":(0..40).map(|i|format!("Input {i}")).collect::<Vec<_>>(),"currentInputDevice":"Input 35"}}));
    click(&mut ui, |t| matches!(t, Target::Devices));
    click(&mut ui, |t| *t == Target::Choose(SelectId::AudioInput));
    ui.prepare_scene();
    let index = ui.interaction.menu().unwrap().highlighted;
    assert!(ui
        .scene
        .hits
        .iter()
        .any(|h| h.target == Target::Option(index) && h.rect.0[3] >= 27.0));
}
#[test]
fn inline_run_settings_support_keyboard_choice_and_tab() {
    use crate::interaction::SelectId;
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"supportsSourceSelection":true,"supportsRunSettings":true,"sampleRateHz":48000,"blockSamples":512}}));
    key(&mut ui, Key::Tab);
    key(&mut ui, Key::Tab);
    assert_eq!(
        ui.interaction.focus().cloned(),
        Some(Target::Choose(SelectId::SampleRate))
    );
    key(&mut ui, Key::Enter);
    key(&mut ui, Key::End);
    key(&mut ui, Key::Enter);
    assert_eq!(ui.state["sampleRateHz"], json!(192000));
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setRunSettings" && v["sampleRateHz"] == 192000));
    key(&mut ui, Key::Tab);
    assert_eq!(
        ui.interaction.focus().cloned(),
        Some(Target::Choose(SelectId::BlockSize))
    );
}
#[test]
fn fast_math_toggles_repeatedly_from_the_keyboard_without_run_selectors() {
    let mut ui = RunUi::default();
    ui.message(
        json!({"type":"state","state":{"supportsSourceSelection":true,"supportsFastMath":true}}),
    );
    key(&mut ui, Key::Tab);
    key(&mut ui, Key::Tab);
    assert_eq!(ui.interaction.focus().cloned(), Some(Target::FastMath));
    for enabled in [true, false, true] {
        key(&mut ui, Key::Enter);
        assert_eq!(ui.state["fastMath"], json!(enabled));
        assert!(ui
            .take_commands()
            .iter()
            .any(|v| v["type"] == "setFastMath" && v["enabled"] == enabled));
    }
}

#[test]
fn loaded_hosts_can_offer_editable_run_settings_without_unlocking_native_settings() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{
        "supportsRunSettings":true,"canSetSampleRate":true,"canSetBlockSize":true,
        "sampleRateHz":48000,"blockSamples":512,
        "sampleRateOptions":[44100,48000],"blockSizeOptions":[128,256,512,1024,2048]
    }}));
    click(&mut ui, |target| *target == Target::Devices);
    click(&mut ui, |target| {
        *target == Target::Choose(SelectId::SampleRate)
    });
    key(&mut ui, Key::Home);
    key(&mut ui, Key::Enter);
    click(&mut ui, |target| {
        *target == Target::Choose(SelectId::BlockSize)
    });
    key(&mut ui, Key::End);
    key(&mut ui, Key::Enter);
    assert!(ui
        .take_commands()
        .iter()
        .any(|command| command["type"] == "setRunSettings"
            && command["sampleRateHz"] == 44100
            && command["blockSamples"] == 2048));
    ui.message(json!({"type":"state","state":{"canSetSampleRate":false,"canSetBlockSize":false}}));
    assert!(!SelectId::SampleRate.spec(&ui).enabled);
    assert!(!SelectId::BlockSize.spec(&ui).enabled);
}
#[test]
fn field_and_range_cursors_follow_hover_and_editing() {
    let mut ui = fixture();
    let r = rect(&mut ui, control_field);
    ui.input(Input::PointerMove {
        x: r[0] + 10.0,
        y: r[1] + 10.0,
        fine: false,
    });
    assert_eq!(ui.cursor(), "ew-resize");
    click(&mut ui, control_field);
    assert_eq!(ui.cursor(), "text");
    key(&mut ui, Key::Escape);
    ui.message(json!({"type":"state","state":{"paramLayout":"knobs"}}));
    let r = rect(&mut ui, |t| matches!(t, Target::Control(0)));
    ui.input(Input::PointerMove {
        x: r[0] + 32.0,
        y: r[1] + 32.0,
        fine: false,
    });
    assert_eq!(ui.cursor(), "ns-resize");
    ui.input(Input::PointerLeave);
    assert_eq!(ui.cursor(), "default");
}

#[test]
fn note_on_only_processors_accept_pointer_and_computer_notes_and_release_at_zero_velocity() {
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"midi":{"noteOn":true,"noteOff":false},
        "currentMidiInputDevice":"Computer Keyboard","supportsMidiInputSelection":true}}),
    );
    click(&mut ui, |target| matches!(target, Target::Note(60)));
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: false,
    });
    let notes: Vec<_> = ui
        .take_commands()
        .into_iter()
        .filter(|c| c["type"] == "midiNote")
        .collect();
    assert_eq!(notes.len(), 4);
    assert_eq!(notes[0]["pressed"], true);
    assert_eq!(notes[2]["pressed"], true);
    for release in [&notes[1], &notes[3]] {
        assert_eq!(release["pressed"], false);
        assert_eq!(release["velocity"], 0.0);
    }
}

#[test]
fn computer_midi_respects_selection_without_disabling_plugin_virtual_keys() {
    for (selection, selectable, expected) in [
        (json!("Computer Keyboard"), true, true),
        (json!("Hardware controller"), true, false),
        (json!("Hardware controller"), false, false),
        (json!(null), true, false),
        (json!(null), false, true),
    ] {
        let mut ui = fixture();
        ui.message(
            json!({"type":"state","state":{"midiKeyboardInteractive":true,
            "midi":{"noteOn":true},"currentMidiInputDevice":selection,"supportsMidiInputSelection":selectable}}),
        );
        ui.input(Input::ComputerKey {
            key: "a".into(),
            pressed: true,
        });
        assert_eq!(
            ui.take_commands().iter().any(|c| c["type"] == "midiNote"),
            expected
        );
    }
}
