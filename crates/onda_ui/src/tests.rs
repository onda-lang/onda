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
    assert_eq!(ui.scope_columns(), 356);
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
