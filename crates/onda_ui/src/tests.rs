use crate::{Input, Key, RunUi};
use serde_json::{json, Value};

fn click_control(ui: &mut RunUi, index: usize, drag: bool) {
    ui.prepare_scene();
    let rect = ui
        .scene
        .hits
        .iter()
        .find(|h| match h.target {
            crate::scene::Target::Control(i) => drag && i == index,
            crate::scene::Target::Field(crate::editing::Field::Control(i)) => !drag && i == index,
            _ => false,
        })
        .expect("control is visible")
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: rect[0] + rect[2] * 0.5,
        y: rect[1] + rect[3] * 0.5,
        clicks: 1,
    });
    if !drag {
        ui.input(Input::PointerUp);
    }
}
fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"path":"patch.onda","params":[
        {"name":"frequency","type":"f64","value":440.0,"default":220.0,"rangeMin":20.0,"rangeMax":20000.0,"scale":"log"},
        {"name":"exact","type":"i64","value":"9223372036854775807","default":"9223372036854775807"}
    ]}}));
    ui.prepare_scene();
    ui
}

#[test]
fn floating_drafts_preserve_zero_signs_in_both_control_banks_and_events() {
    use crate::{editing::Field, events::EventField};
    for scalar in ["f32", "f64"] {
        for bank in ["params", "inputControls", "events"] {
            let mut ui = RunUi::default();
            let mut state = json!({"path":"zeros.onda","supportsScope":false});
            let field = if bank == "events" {
                state[bank] =
                    json!([{"name":"send","args":[{"name":"x","type":scalar,"value":0.0}]}]);
                Field::Event(EventField {
                    event: 0,
                    arg: 0,
                    path: String::new(),
                    scalar: scalar.into(),
                })
            } else {
                state[bank] = json!([{"name":"x","type":scalar,"value":0.0}]);
                Field::Control(0)
            };
            ui.message(json!({"type":"state","state":state}));
            ui.take_commands();
            for negative in [true, false, true] {
                ui.begin_field(field.clone());
                ui.input(Input::Text {
                    text: if negative { "-0" } else { "0" }.into(),
                });
                ui.input(Input::Key {
                    key: Key::Enter,
                    shift: false,
                });
                assert_eq!(
                    ui.field_value(&field).as_f64().unwrap().is_sign_negative(),
                    negative,
                    "{bank} {scalar} draft must retain its zero sign"
                );
                let commands = ui.take_commands();
                if bank != "events" {
                    let command = commands
                        .iter()
                        .find(|c| {
                            c["type"]
                                == if bank == "params" {
                                    "setParam"
                                } else {
                                    "setInputControl"
                                }
                        })
                        .unwrap();
                    assert_eq!(
                        command["value"].as_f64().unwrap().is_sign_negative(),
                        negative
                    );
                }
                ui.begin_field(field.clone());
                ui.input(Input::Key {
                    key: Key::Enter,
                    shift: false,
                });
                assert!(
                    ui.take_commands().is_empty(),
                    "an unedited draft must remain a no-op"
                );
            }
        }
    }
}

#[test]
fn signed_zero_value_updates_invalidate_visible_controls() {
    for bank in ["params", "inputControls"] {
        let mut ui = RunUi::default();
        ui.message(json!({"type":"state","state":{bank:[{"name":"x","type":"f64","value":0.0}]}}));
        for value in [-0.0_f64, 0.0] {
            ui.prepare_scene();
            let revision = ui.revisions();
            ui.message(
                json!({"type":"parameterValues","state":{bank:[{"name":"x","value":value}]}}),
            );
            assert_eq!(
                ui.controls[0].value.as_f64().unwrap().to_bits(),
                value.to_bits()
            );
            assert_ne!(ui.revisions(), revision, "the zero-sign label changed");
        }
    }
}

#[test]
fn signed_zero_default_changes_replace_schemas_and_cancel_drafts() {
    use crate::{editing::Field, events::EventField};
    for bank in ["params", "inputControls", "events"] {
        let mut ui = RunUi::default();
        let collection = |default| {
            if bank == "events" {
                json!([{"name":"send","args":[{"name":"x",
                    "shape":{"kind":"tuple","elements":[{"kind":"scalar","encoding":"f64"}]},
                    "value":[default],"default":[default]}]}])
            } else {
                json!([{"name":"x","type":"f64","value":default,"default":default}])
            }
        };
        ui.message(json!({"type":"state","state":{"path":"defaults.onda",bank:collection(0.0)}}));
        let field = if bank == "events" {
            Field::Event(EventField {
                event: 0,
                arg: 0,
                path: "/0".into(),
                scalar: "f64".into(),
            })
        } else {
            Field::Control(0)
        };
        ui.set_field(&field, json!(0.75), true);
        ui.begin_field(field.clone());
        ui.input(Input::Text { text: "0.9".into() });
        ui.message(json!({"type":"state","state":{bank:collection(-0.0)}}));
        assert!(
            !ui.editing(),
            "a changed {bank} default invalidates the draft"
        );
        assert!(ui.field_value(&field).as_f64().unwrap().is_sign_negative());
    }
}

#[test]
fn value_updates_follow_names_and_banks_after_schema_replacement() {
    let mut ui = RunUi::default();
    let control = |name| json!({"name":name,"type":"f64","value":0});
    ui.message(json!({"type":"state","state":{
        "params":[control("shared"),control("removed")],
        "inputControls":[control("shared")]
    }}));
    let update = json!({"type":"parameterValues","state":{
        "params":[{"name":"shared","value":1},{"name":"removed","value":99}],
        "inputControls":[{"name":"shared","value":2}]
    }});
    ui.message(update.clone());
    assert_eq!(ui.controls[0].value, 2);
    assert_eq!(ui.controls[1].value, 1);
    ui.message(json!({"type":"state","state":{
        "params":[control("replacement"),control("shared")]
    }}));
    ui.message(json!({"type":"state","state":{
        "inputControls":[control("new"),control("shared")]
    }}));
    ui.message(update);
    assert_eq!(
        ui.controls
            .iter()
            .map(|c| c.value.clone())
            .collect::<Vec<_>>(),
        json!([0, 2, 0, 1]).as_array().unwrap().clone()
    );
    let revision = ui.revisions();
    ui.message(json!({"type":"parameterValues","state":{
        "params":[{"name":"shared","value":1}],
        "inputControls":[{"name":"shared","value":2}]
    }}));
    assert_eq!(ui.revisions(), revision);
}

#[test]
fn folded_and_offscreen_value_updates_retain_the_scene_and_reveal_latest_values() {
    use crate::{editing::Field, presentation::Section};
    for (bank, section) in [
        ("params", Section::Params),
        ("inputControls", Section::InputControls),
    ] {
        let mut ui = RunUi::default();
        ui.resize(360.0, 400.0, 1.0);
        let controls: Vec<_> = (0..100)
            .map(|i| {
                json!({
                    "name":format!("p{i}"),"type":"f64","value":0
                })
            })
            .collect();
        ui.message(json!({"type":"state","state":{"path":"values.onda","supportsScope":false,bank:controls}}));
        ui.presentation.sections[section.index()] = false;
        ui.prepare_scene();
        let revisions = ui.revisions();
        ui.message(json!({"type":"parameterValues","state":{bank:[{"name":"p0","value":1.25}]}}));
        assert_eq!(ui.revisions(), revisions);
        assert!(!ui.scene_dirty);
        assert_eq!(ui.controls[0].value, 1.25);

        ui.presentation.sections[section.index()] = true;
        ui.invalidate();
        ui.prepare_scene();
        assert!(ui
            .scene
            .labels
            .iter()
            .any(|label| label.text == ui.field_text(&Field::Control(0))));
        let revisions = ui.revisions();
        ui.message(json!({"type":"parameterValues","state":{bank:[{"name":"p99","value":9.75}]}}));
        assert_eq!(ui.revisions(), revisions);
        assert!(!ui.scene_dirty);
        ui.scroll = ui.scene.content_height - ui.scene.viewport_height;
        ui.invalidate();
        ui.prepare_scene();
        assert!(ui
            .scene
            .labels
            .iter()
            .any(|label| label.text == ui.field_text(&Field::Control(99))));
        ui.message(json!({"type":"parameterValues","state":{bank:[{"name":"p99","value":8.5}]}}));
        assert!(ui.scene_dirty);
        ui.prepare_scene();
        assert!(ui
            .scene
            .labels
            .iter()
            .any(|label| label.text == ui.field_text(&Field::Control(99))));
    }
}

#[test]
fn visible_read_only_controls_repaint_on_value_changes() {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "path":"external.onda","inputControls":[{"name":"external","type":"bool","enabled":false,"value":false}]
    }}));
    ui.prepare_scene();
    let revisions = ui.revisions();
    ui.message(json!({"type":"parameterValues","state":{"inputControls":[{"name":"external","value":true}]}}));
    assert_ne!(ui.revisions(), revisions);
    assert!(ui.scene_dirty);
}
#[test]
fn scope_frames_preserve_layout_and_text_revision() {
    let mut ui = fixture();
    ui.set_scope_interleaved(2, &[0.0; 6]);
    ui.prepare_scene();
    let rev = ui.revisions().0;
    ui.set_scope_interleaved(2, &[0.0, 0.5, 0.2, 0.4, 0.3, 0.0]);
    assert_eq!(ui.revisions().0, rev);
    assert!(!ui.scene_dirty);
    assert_eq!(ui.scope.channels(), 2);
    assert_eq!(ui.scope.columns(), 3);
    assert_eq!(ui.trace.len(), 6);
    ui.set_scope_interleaved(2, &[f32::NAN, f32::INFINITY, -2.0, 2.0]);
    assert_eq!(
        ui.scope.ranges(),
        &[[0, 0], [-32767, -32767], [0, 0], [32767, 32767]]
    );
}

#[test]
fn raw_scope_messages_retain_the_original_channel_stride() {
    let mut ui = fixture();
    let mut samples = vec![0.0; 65 * 2];
    samples[65] = 1.0;
    ui.message(json!({"type":"scopeData","channels":65,"samples":samples}));
    assert_eq!(ui.scope.channels(), 64);
    assert_eq!(&ui.scope.ranges()[..2], &[[0, 0], [32767, 32767]]);
    assert!(ui.scope.ranges()[2..].iter().all(|r| *r == [0, 0]));
}
#[test]
fn exact_integer_draft_survives_host_updates_and_escape() {
    let mut ui = fixture();
    click_control(&mut ui, 1, false);
    assert!(ui.editing());
    assert_eq!(ui.editor.draft().unwrap().text, "9223372036854775807");
    ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"exact","value":"-9223372036854775808"}]}}));
    assert_eq!(ui.editor.draft().unwrap().text, "9223372036854775807");
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    assert_eq!(ui.controls[1].value, "-9223372036854775808");
    assert!(ui.take_commands().is_empty());
}
#[test]
fn releasing_parent_keyboard_notes_preserves_a_child_field_draft() {
    let mut ui = fixture();
    ui.message(
        json!({"type":"state","state":{"midiKeyboardInteractive":true,"midi":{"noteOn":true}}}),
    );
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.take_commands();
    ui.input(Input::ReleaseComputerKeys);
    assert!(ui.interaction.midi().keyboard.is_empty());
    assert!(ui
        .take_commands()
        .iter()
        .any(|command| command["type"] == "midiNote" && command["pressed"] == false));
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.prepare_scene();
    let key = ui
        .scene
        .hits
        .iter()
        .find(|hit| hit.target == crate::scene::Target::Note(60))
        .unwrap()
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: key[0] + key[2] * 0.5,
        y: key[1] + key[3] - 4.0,
        clicks: 1,
    });
    ui.take_commands();
    ui.input(Input::ReleaseComputerKeys);
    assert!(ui.interaction.midi().keyboard.is_empty());
    assert_eq!(ui.interaction.midi().pointer, Some(60));
    assert!(ui.interaction.midi().pointer_down);
    assert!(ui.take_commands().is_empty());
    ui.input(Input::PointerUp);
    assert_eq!(ui.take_commands()[0]["pressed"], false);
    click_control(&mut ui, 1, false);
    ui.input(Input::ReleaseComputerKeys);
    assert!(ui.editing());
}
#[test]
fn numeric_edit_retains_all_i64_bits_and_rejects_invalid_commit() {
    let mut ui = fixture();
    click_control(&mut ui, 1, false);
    ui.input(Input::Text {
        text: "-9223372036854775808".into(),
    });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert_eq!(ui.take_commands()[0]["value"], "-9223372036854775808");
    click_control(&mut ui, 1, false);
    ui.input(Input::Text { text: "-".into() });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert!(ui.editing());
    assert!(ui.editor.draft().unwrap().invalid);
    assert!(ui.take_commands().is_empty());
}
#[test]
fn schema_replacement_cancels_daw_gesture_and_automation_preserves_drag() {
    let mut ui = fixture();
    click_control(&mut ui, 0, true);
    ui.input(Input::PointerMove {
        x: 230.0,
        y: 360.0,
        fine: false,
    });
    let value = ui.controls[0].value.clone();
    ui.message(
        json!({"type":"parameterValues","state":{"params":[{"name":"frequency","value":999.0}]}}),
    );
    assert_eq!(ui.controls[0].value, value);
    ui.message(json!({"type":"state","state":{"params":[]}}));
    let commands = ui.take_commands();
    assert_eq!(commands.last().unwrap()["type"], "endParamGesture");
    assert!(ui.active_control().is_none());
}
#[test]
fn plugin_domains_use_shared_curve_and_step_conversion() {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"params":[{"name":"slot","type":"f64",
        "hostValueDomain":"normalized","normalizedValue":0.5,"rangeMin":10.0,"rangeMax":20.0,"scale":"linear","curve":-4.0}]}}));
    let c = &ui.controls[0];
    assert!((c.wire_value().as_f64().unwrap() - 0.5).abs() < 1e-12);
    ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"slot","normalizedValue":0.25}]}}));
    assert!((ui.controls[0].wire_value().as_f64().unwrap() - 0.25).abs() < 1e-12);
    let _: Vec<Value> = ui.take_commands();
}

#[test]
fn normalized_integer_values_keep_arrow_key_editing_in_both_banks() {
    for scalar in ["i32", "i64"] {
        for bank in ["params", "inputControls"] {
            for initial in [true, false] {
                let mut ui = RunUi::default();
                let mut control = json!({"name":"count","type":scalar,
                    "rangeMin":0,"rangeMax":100,"step":1,"stepCount":100,
                    "hostValueDomain":"normalized"});
                if initial {
                    control["normalizedValue"] = json!(0.5);
                } else {
                    control["value"] = if scalar == "i64" {
                        json!("0")
                    } else {
                        json!(0)
                    };
                }
                ui.message(json!({"type":"state","state":{
                    "path":"integers.onda","supportsScope":false,bank:[control]
                }}));
                if !initial {
                    ui.message(json!({"type":"parameterValues","state":{
                        bank:[{"name":"count","normalizedValue":0.5}]
                    }}));
                }
                click_control(&mut ui, 0, false);
                assert_eq!(ui.text_edit().unwrap()["text"], "50");
                ui.input(Input::Key {
                    key: Key::ArrowUp,
                    shift: false,
                });
                assert_eq!(
                    ui.text_edit().unwrap()["text"],
                    "51",
                    "{scalar} {bank}, initial={initial}"
                );
                ui.input(Input::Key {
                    key: Key::Enter,
                    shift: false,
                });
                let commands = ui.take_commands();
                let command = commands
                    .iter()
                    .find(|command| {
                        command["type"]
                            == if bank == "params" {
                                "setParam"
                            } else {
                                "setInputControl"
                            }
                    })
                    .unwrap();
                assert_eq!(command["value"], 0.51);
                assert_eq!(
                    ui.controls[0].value,
                    if scalar == "i64" {
                        json!("51")
                    } else {
                        json!(51)
                    }
                );
            }
        }
    }
}

#[test]
fn plugin_boolean_values_and_constant_controls_keep_host_semantics() {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"params":[
        {"name":"enabled","type":"bool","hostValueDomain":"normalized","normalizedValue":1.0},
        {"name":"constant","type":"f64","value":5.0,"rangeMin":5.0,"rangeMax":5.0}
    ]}}));
    assert_eq!(ui.controls[0].value, true);
    assert_eq!(ui.controls[0].wire_value(), 1.0);
    assert!(!ui.controls[1].enabled());
    ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"enabled","normalizedValue":0.0}]}}));
    assert_eq!(ui.controls[0].value, false);
}

#[test]
fn plugin_numeric_booleans_toggle_reset_and_follow_host_updates() {
    let mut ui = RunUi::default();
    ui.message(
        json!({"type":"state","state":{"path":"gate.onda","supportsScope":false,
        "params":[{"name":"gate","type":"bool","hostValueDomain":"normalized",
            "value":1.0,"normalizedValue":1.0,"default":1.0,"rangeMin":0,"rangeMax":1}]}}),
    );
    assert_eq!(ui.controls[0].value, true);
    assert_eq!(ui.controls[0].metadata["default"], true);
    click_control(&mut ui, 0, false);
    assert!(!ui.editing());
    assert_eq!(ui.take_commands()[0]["value"], 0.0);
    ui.prepare_scene();
    let r = ui
        .scene
        .hits
        .iter()
        .find(|h| {
            matches!(
                h.target,
                crate::scene::Target::Field(crate::editing::Field::Control(0))
            )
        })
        .unwrap()
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: r[0] + 12.0,
        y: r[1] + 12.0,
        clicks: 2,
    });
    ui.input(Input::PointerUp);
    assert_eq!(ui.take_commands()[0]["value"], 1.0);
    ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"gate","value":0.0}]}}));
    assert_eq!(ui.controls[0].value, false);
    ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"gate","value":1.0}]}}));
    assert_eq!(ui.controls[0].value, true);
}

#[test]
fn changing_source_cancels_gestures_even_when_metadata_is_identical() {
    let mut ui = fixture();
    click_control(&mut ui, 0, true);
    ui.message(json!({"type":"state","state":{"path":"different.onda"}}));
    assert!(ui.active_control().is_none());
    assert_eq!(
        ui.take_commands().last().unwrap()["type"],
        "endParamGesture"
    );
}

#[test]
fn unedited_commit_preserves_published_values_and_accepts_new_host_values() {
    for update in [None, Some(0.25)] {
        let mut ui = RunUi::default();
        ui.message(
            json!({"type":"state","state":{"params":[{"name":"value","type":"f64",
            "value":2.0,"rangeMin":0.0,"rangeMax":1.0,"scale":"linear"}]}}),
        );
        click_control(&mut ui, 0, false);
        if let Some(value) = update {
            ui.message(json!({"type":"parameterValues","state":{"params":[{"name":"value","value":value}]}}));
        }
        ui.input(Input::Key {
            key: Key::Enter,
            shift: false,
        });
        assert_eq!(ui.controls[0].value, json!(update.unwrap_or(2.0)));
        assert!(ui.take_commands().is_empty());
    }
}

#[test]
fn compact_scope_preserves_layout_and_rejects_invalid_packets_without_losing_the_last_frame() {
    let mut ui = fixture();
    ui.presentation.scope_pinned = false;
    ui.set_scope_interleaved(2, &[0.0; 4]);
    ui.prepare_scene();
    let revision = ui.revisions().0;
    let mut frame = onda_scope::Envelope::default();
    frame.set_interleaved(2, &[0.1, 0.2, -1.0, 1.0], 2);
    ui.set_scope_packet(&frame.packet()).unwrap();
    assert_eq!(ui.scope, frame);
    assert_eq!(ui.revisions().0, revision);
    let revisions = ui.revisions();
    assert!(ui.set_scope_packet(b"broken").is_err());
    assert_eq!(ui.revisions(), revisions);
    assert_eq!(ui.scope, frame);
    ui.resize(400.0, 600.0, 2.0);
    assert_eq!(ui.scope_columns(), 304);
}

#[test]
fn identical_scope_frames_skip_uploads_but_channel_layout_changes_invalidate() {
    let mut ui = fixture();
    let mut frame = onda_scope::Envelope::default();
    frame.set_interleaved(2, &[0.0; 8], 2);
    ui.set_scope_envelope(&frame);
    let revisions = ui.revisions();
    ui.set_scope_packet(&frame.packet()).unwrap();
    assert_eq!(ui.revisions(), revisions);
    // Same number of instances and identical samples, different channel layout.
    frame.set_interleaved(1, &[0.0; 8], 4);
    ui.set_scope_envelope(&frame);
    assert_ne!(ui.revisions().1, revisions.1);
}
