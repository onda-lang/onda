use crate::{
    editing::{Field, Setting},
    events::EventField,
    presentation::Section,
    scene::Target,
    Input, Key, RunUi,
};
use serde_json::json;

fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{"path":"panels.onda","engineGeneration":"18446744073709551615",
        "supportsScope":true,"supportsViewState":true,"midiKeyboardInteractive":true,"midi":{"noteOn":true},
        "params":[{"name":"gain","type":"f64","value":0.5,"rangeMin":0,"rangeMax":1}],
        "inputControls":[{"name":"wet","type":"f64","value":0.25,"rangeMin":0,"rangeMax":1,"hostValueDomain":"normalized"}],
        "events":[{"name":"play","args":[{"name":"voice","shape":{"kind":"struct","fields":[
            {"name":"id","ty":{"kind":"scalar","encoding":"i64"}},
            {"name":"levels","ty":{"kind":"slice","element":{"kind":"scalar","encoding":"f32"}}}
        ]},"default":{"id":"9223372036854775807","levels":[0.5]}}]}],
        "buffers":[{"name":"audio","type":"f32","waveform":{"minimums":[-0.5],"maximums":[0.5]}}],
        "sampleRateHz":48000,"blockSamples":512,"paramSmoothingMs":30,"supportsParamSmoothing":true
    }}));
    ui
}
fn show_section(ui: &mut RunUi, section: Section) {
    for s in Section::ALL {
        ui.presentation.sections[s.index()] = s == section || s == Section::Midi;
    }
    ui.scroll = 0.0;
    ui.invalidate();
    ui.prepare_scene();
}
fn click(ui: &mut RunUi, predicate: impl Fn(&Target) -> bool) {
    ui.prepare_scene();
    let rect = ui
        .scene
        .hits
        .iter()
        .find(|h| predicate(&h.target))
        .expect("visible target")
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: rect[0] + rect[2] / 2.0,
        y: rect[1] + rect[3] / 2.0,
        clicks: 1,
    });
    ui.input(Input::PointerUp);
}
fn edit(ui: &mut RunUi, target: Field, text: &str) {
    ui.begin_field(target);
    ui.input(Input::Text { text: text.into() });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
}
#[test]
fn typed_event_edits_survive_status_updates_and_dispatch_generation_losslessly() {
    let mut ui = fixture();
    show_section(&mut ui, Section::Events);
    edit(
        &mut ui,
        Field::Event(EventField {
            event: 0,
            arg: 0,
            path: "/id".into(),
            scalar: "i64".into(),
        }),
        "-9223372036854775808",
    );
    ui.message(json!({"type":"state","state":{"status":"playing","events":ui.state["events"]}}));
    click(&mut ui, |t| matches!(t, Target::Trigger(0)));
    let commands = ui.take_commands();
    let event = commands
        .iter()
        .find(|v| v["type"] == "triggerEvent")
        .unwrap();
    assert_eq!(event["values"][0]["id"], "-9223372036854775808");
    assert_eq!(event["engineGeneration"], "18446744073709551615");
}
#[test]
fn json_event_editor_rejects_bad_shapes_and_preserves_the_last_valid_value() {
    let mut ui = fixture();
    let field = Field::Event(EventField {
        event: 0,
        arg: 0,
        path: String::new(),
        scalar: "json".into(),
    });
    edit(&mut ui, field.clone(), r#"{"id":"invalid","levels":[]}"#);
    assert!(ui.editor.draft().unwrap().invalid);
    assert_eq!(ui.events[0].args[0].value["id"], "9223372036854775807");
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    edit(
        &mut ui,
        field,
        r#"{"id":"42","levels":["NaN","Infinity",0.25]}"#,
    );
    assert!(!ui.editing());
    assert_eq!(ui.events[0].args[0].value["levels"][1], "Infinity");
}
#[test]
fn schema_replacement_cancels_event_draft_and_source_change_resets_arguments() {
    let mut ui = fixture();
    ui.begin_field(Field::Event(EventField {
        event: 0,
        arg: 0,
        path: "/id".into(),
        scalar: "i64".into(),
    }));
    ui.message(json!({"type":"state","state":{"events":[]}}));
    assert!(!ui.editing());
    ui.message(json!({"type":"state","state":{"path":"new.onda"}}));
    assert!(ui.events.is_empty());
}

#[test]
fn changed_event_defaults_replace_preserved_values_and_cancel_drafts() {
    let mut ui = fixture();
    for default in [0.5, 1.0] {
        ui.message(
            json!({"type":"state","state":{"events":[{"name":"hit","args":[
                {"name":"gain","type":"f64","default":default,"value":default}
            ]}]}}),
        );
        assert_eq!(ui.events[0].args[0].value, json!(default));
        assert!(!ui.editing());
        ui.begin_field(Field::Event(EventField {
            event: 0,
            arg: 0,
            path: String::new(),
            scalar: "f64".into(),
        }));
        ui.input(Input::Text {
            text: "0.75".into(),
        });
    }
}

#[test]
fn resetting_presentation_retains_event_schema_and_restores_defaults() {
    for full in [false, true] {
        let mut ui = fixture();
        ui.events[0].args[0].value["id"] = json!("42");
        let mut state = json!({"viewState":{"reset":true}});
        if full {
            state["events"] = ui.state["events"].clone();
            state["events"][0]["args"][0]["value"] = ui.events[0].args[0].value.clone();
        }
        ui.message(json!({"type":"state","state":state}));
        assert_eq!(ui.events.len(), 1);
        assert_eq!(ui.events[0].args[0].value, ui.events[0].args[0].default);
        show_section(&mut ui, Section::Events);
        click(&mut ui, |t| *t == Target::Trigger(0));
        assert!(ui
            .take_commands()
            .iter()
            .any(|c| c["type"] == "triggerEvent"));
    }
}

#[test]
fn host_text_selection_preserves_utf16_offsets_and_unicode_boundaries() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"events":[{
        "name":"unicode","args":[{"name":"value",
            "shape":{"kind":"struct","fields":[{"name":"😀a","ty":{"kind":"scalar","encoding":"i32"}}]},
            "value":{"😀a":1}}]
    }]}}));
    show_section(&mut ui, Section::Events);
    ui.begin_field(Field::Event(EventField {
        event: 0,
        arg: 0,
        path: String::new(),
        scalar: "json".into(),
    }));
    ui.input(Input::EditText {
        text: "{\"😀a\":1}".into(),
        anchor: 3,
        cursor: 4,
    });
    let edit = ui.text_edit().unwrap();
    assert_eq!(
        edit["anchor"], 2,
        "an offset inside a surrogate pair clamps to its start"
    );
    assert_eq!(edit["cursor"], 4);
    ui.input(Input::Key {
        key: Key::Copy,
        shift: false,
    });
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "copyText" && c["text"] == "😀"));
    ui.input(Input::Text { text: "b".into() });
    assert_eq!(ui.editor.draft().unwrap().text, "{\"ba\":1}");
}
#[test]
fn slice_buttons_and_reset_preserve_the_typed_argument() {
    let mut ui = fixture();
    show_section(&mut ui, Section::Events);
    click(&mut ui, |t| matches!(t, Target::Slice { add: true, .. }));
    assert_eq!(ui.events[0].args[0].value["levels"], json!([0.5, 0]));
    click(
        &mut ui,
        |t| matches!(t,Target::Command(v) if v["type"]=="resetEventArguments"),
    );
    assert_eq!(ui.events[0].args[0].value["levels"], json!([0.5]));
}

#[test]
fn new_struct_slice_elements_decode_recursive_abi_defaults() {
    let mut ui = fixture();
    let voice = json!({"kind":"struct","fields":[
        {"name":"gate","ty":{"kind":"scalar","encoding":"bool"},"default":"true"},
        {"name":"id","ty":{"kind":"scalar","encoding":"i64"},"default":"9223372036854775807"},
        {"name":"gain","ty":{"kind":"scalar","encoding":"f32"},"default":"0.1"},
        {"name":"bits","ty":{"kind":"scalar","encoding":"f64"},"default":"0x3ff0000000000000"},
        {"name":"nested","ty":{"kind":"struct","fields":[
            {"name":"enabled","ty":{"kind":"scalar","encoding":"bool"},"default":"false"},
            {"name":"values","ty":{"kind":"tuple","elements":[
                {"kind":"scalar","encoding":"f64"},
                {"kind":"array","len":2,"element":{"kind":"scalar","encoding":"i32"}}
            ]}}
        ]},"default":["true",["Infinity",["-1","2"]]]}
    ]});
    ui.message(
        json!({"type":"state","state":{"events":[{"name":"voices","args":[{
            "name":"voices","shape":{"kind":"slice","element":voice},"default":[]
        }]}]}}),
    );
    show_section(&mut ui, Section::Events);
    click(&mut ui, |t| matches!(t, Target::Slice { add: true, .. }));
    let expected = json!([{"gate":true,"id":"9223372036854775807",
        "gain":f64::from(0.1_f32),"bits":1.0,
        "nested":{"enabled":true,"values":["Infinity",[-1,2]]}}]);
    assert_eq!(ui.events[0].args[0].value, expected);
    assert!(ui.events[0].valid());
    click(&mut ui, |t| *t == Target::Trigger(0));
    let commands = ui.take_commands();
    let event = commands
        .iter()
        .find(|c| c["type"] == "triggerEvent")
        .unwrap();
    assert_eq!(event["values"][0], expected);
}

#[test]
fn saved_view_waits_for_complete_schema_and_restores_only_once() {
    let mut ui = RunUi::default();
    let view = json!({"path":"restore.onda","scrollTop":1000,"paramLayout":"knobs",
        "eventValues":[{"name":"hit","args":[{"name":"level",
            "shape":{"kind":"scalar","encoding":"f64"},"default":1}],"values":[42]}]});
    ui.message(json!({"type":"state","state":{"path":"restore.onda",
        "schemaReady":false,"params":[],"events":[],"viewState":view}}));
    ui.prepare_scene();
    assert_eq!(ui.scroll, 0.0);
    ui.presentation.dirty = true;
    assert!(
        ui.take_commands().is_empty(),
        "loading must not overwrite saved presentation"
    );
    ui.message(json!({"type":"state","state":{
        "params":(0..80).map(|i|json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>(),
        "events":[{"name":"hit","args":[{"name":"level","type":"f64","default":1}]}],
        "viewState":view,"paramLayout":"sliders"
    }}));
    ui.prepare_scene();
    assert_eq!(ui.scroll, 0.0);
    ui.message(json!({"type":"state","state":{"schemaReady":true}}));
    ui.prepare_scene();
    assert_eq!(ui.scroll, 1000.0);
    assert!(ui.knobs, "saved layout must win over host defaults");
    assert_eq!(ui.events[0].args[0].value, 42);
    ui.events[0].args[0].value = json!(43);
    ui.scroll = 200.0;
    ui.message(json!({"type":"state","state":{"viewState":view,"status":"Playing"}}));
    assert_eq!(ui.events[0].args[0].value, 43);
    assert_eq!(ui.scroll, 200.0);
}

#[test]
fn changing_source_or_resetting_cancels_pending_presentation() {
    for reset in [false, true] {
        let mut ui = RunUi::default();
        ui.message(
            json!({"type":"state","state":{"path":"old.onda","schemaReady":false,
            "viewState":{"scrollTop":1000,"paramLayout":"knobs"}}}),
        );
        let state = if reset {
            json!({"schemaReady":true,"viewState":{"reset":true}})
        } else {
            json!({"schemaReady":true,"path":"new.onda"})
        };
        ui.message(json!({"type":"state","state":state}));
        assert!(ui.pending_view.is_none());
        assert_eq!(ui.scroll, 0.0);
        assert!(ui.knobs);
    }
}

#[test]
fn midi_input_selection_releases_all_owned_notes_before_switching() {
    use crate::interaction::SelectId;
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{
        "supportsMidiInputSelection":true,"currentMidiInputDevice":"Computer Keyboard",
        "midiInputDevices":["Computer Keyboard","Hardware"]
    }}));
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.prepare_scene();
    for (id, note) in [(1, 60), (2, 64)] {
        let key = ui
            .scene
            .hits
            .iter()
            .find(|hit| hit.target == Target::Note(note))
            .unwrap()
            .rect
            .0;
        ui.input(Input::TouchStart {
            id,
            x: key[0] + key[2] * 0.5,
            y: key[1] + key[3] - 4.0,
        });
    }
    ui.take_commands();
    ui.open_select(SelectId::MidiInput);
    ui.choose_option(1);
    let commands = ui.take_commands();
    assert_eq!(
        commands
            .iter()
            .filter(|c| c["type"] == "midiNote")
            .map(|c| (c["key"].as_u64().unwrap(), c["pressed"] == true))
            .collect::<Vec<_>>(),
        [(60, false), (64, false)]
    );
    assert_eq!(commands.last().unwrap()["type"], "setMidiInputDevice");
    ui.message(json!({"type":"state","state":{"currentMidiInputDevice":"Hardware"}}));
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: false,
    });
    assert!(
        ui.take_commands().is_empty(),
        "host echoes and later releases must not duplicate note-offs"
    );
}
#[test]
fn midi_pointer_keyboard_overlap_glissando_and_focus_release_never_stick() {
    let mut ui = fixture();
    show_section(&mut ui, Section::Midi);
    ui.take_commands();
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    ui.prepare_scene();
    let r = ui
        .scene
        .hits
        .iter()
        .find(|h| matches!(h.target, Target::Note(60)))
        .unwrap()
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: r[0] + 2.0,
        y: r[1] + r[3] - 4.0,
        clicks: 1,
    });
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: false,
    });
    let commands = ui.take_commands();
    assert_eq!(
        commands.iter().filter(|v| v["type"] == "midiNote").count(),
        1
    );
    ui.input(Input::PointerMove {
        x: 0.0,
        y: 0.0,
        fine: false,
    });
    assert_eq!(ui.take_commands()[0]["pressed"], false);
    ui.input(Input::PointerMove {
        x: r[0] + 2.0,
        y: r[1] + r[3] - 4.0,
        fine: false,
    });
    assert_eq!(ui.take_commands()[0]["pressed"], true);
    ui.input(Input::FocusLost);
    assert_eq!(ui.take_commands()[0]["pressed"], false);
    ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: false,
    });
    assert!(ui.take_commands().is_empty());
}
#[test]
fn buffer_drop_targets_only_the_visible_buffer_panel() {
    let mut ui = fixture();
    let drag = json!({"type":"bufferFileDrag","phase":"drop","paths":["/tmp/audio.wav"],"position":{"x":50,"y":360}});
    show_section(&mut ui, Section::Params);
    ui.message(drag.clone());
    assert!(ui.take_commands().is_empty());
    show_section(&mut ui, Section::Buffers);
    ui.prepare_scene();
    let r = ui.scene.buffers[0].0 .0;
    let mut drag = drag;
    drag["position"] = json!({"x":r[0]+10.0,"y":r[1]+10.0});
    ui.message(drag);
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "bindBufferFile"
            && v["name"] == "audio"
            && v["filePath"] == "/tmp/audio.wav"));
}

#[test]
fn buffer_drops_require_one_distinct_host_validated_path() {
    let mut ui = fixture();
    show_section(&mut ui, Section::Buffers);
    let r = ui.scene.buffers[0].0 .0;
    for paths in [
        json!(["a.wav", "b.wav"]),
        json!([]),
        json!(["clip.wav", null]),
        json!([""]),
    ] {
        ui.message(json!({"type":"bufferFileDrag","phase":"drop","paths":paths,
            "position":{"x":r[0]+10.0,"y":r[1]+10.0}}));
        assert!(
            ui.take_commands().is_empty(),
            "invalid asset drop was accepted"
        );
    }
    for path in ["clip.WAV", "clip.flac", "clip.aiff", "clip.ogg"] {
        ui.message(
            json!({"type":"bufferFileDrag","phase":"drop","paths":[path,path],
            "position":{"x":r[0]+10.0,"y":r[1]+10.0}}),
        );
        assert_eq!(ui.take_commands()[0]["filePath"], path);
    }
}

#[test]
fn settings_block_buffer_drops_to_the_card_beneath_them() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"supportsDeviceSelection":true}}));
    show_section(&mut ui, Section::Buffers);
    let r = ui.scene.buffers[0].0 .0;
    let (x, y) = (r[0] + r[2] - 20.0, r[1] + 10.0);
    assert_eq!(ui.buffer_target(x, y).as_deref(), Some("audio"));
    ui.toggle_settings();
    ui.invalidate();
    assert!(ui.buffer_target(x, y).is_none());
    ui.message(
        json!({"type":"bufferFileDrag","phase":"drop","paths":["clip.wav"],
        "position":{"x":x,"y":y}}),
    );
    assert!(ui.take_commands().is_empty());
}

#[test]
fn visible_log_rows_retain_their_source_context_when_scrolled() {
    let mut ui = fixture();
    let text = (0..100)
        .map(|i| format!("message {i}\n"))
        .collect::<String>();
    let entries: Vec<_> = (0..100)
        .map(|i| {
            json!({"source":{"file":"main.onda","line":i},
        "lexicalOwner":"voice"})
        })
        .collect();
    ui.message(json!({"type":"state","state":{"logText":text,"logEntries":entries}}));
    show_section(&mut ui, Section::Log);
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|l| l.text == "main.onda:99 · voice"));
    assert!(!ui
        .scene
        .labels
        .iter()
        .any(|l| l.text == "main.onda:0 · voice"));
    ui.presentation.log_follow = false;
    ui.presentation.log_scroll = 0.0;
    ui.invalidate();
    ui.prepare_scene();
    assert!(ui
        .scene
        .labels
        .iter()
        .any(|l| l.text == "main.onda:0 · voice"));
}
#[test]
fn separate_banks_route_values_and_gestures_to_their_host_commands() {
    let mut ui = fixture();
    show_section(&mut ui, Section::InputControls);
    click(&mut ui, |t| matches!(t, Target::Field(Field::Control(_))));
    ui.input(Input::Text {
        text: "0.75".into(),
    });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setInputControl" && v["name"] == "wet" && v["value"] == 0.75));
    click(&mut ui, |t| {
        matches!(
            t,
            Target::Layout {
                input: true,
                knobs: true
            }
        )
    });
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setInputControlLayout" && v["layout"] == "knobs"));
}
#[test]
fn presentation_roundtrips_sections_scroll_and_event_values() {
    let mut ui = fixture();
    show_section(&mut ui, Section::Events);
    edit(
        &mut ui,
        Field::Event(EventField {
            event: 0,
            arg: 0,
            path: "/id".into(),
            scalar: "i64".into(),
        }),
        "123",
    );
    ui.scroll = 42.0;
    ui.presentation.sections[Section::Scope.index()] = false;
    ui.presentation.dirty = true;
    let commands = ui.take_commands();
    let state = commands.iter().find(|v| v["type"] == "viewState").unwrap()["state"].clone();
    let mut other = fixture();
    other.state.as_object_mut().unwrap().remove("viewState");
    other.message(json!({"type":"state","state":{"viewState":state}}));
    assert!(other.presentation.open(Section::Events));
    assert_eq!(other.scroll, 42.0);
    assert!(!other.scope_enabled());
    assert_eq!(other.events[0].args[0].value["id"], "123");
}
#[test]
fn scrolling_defers_large_event_snapshots_until_it_settles() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{
        "events":[{"name":"send","args":[{"name":"values","type":"f64[]","value":vec![0.5;65536]}]}]
    }}));
    show_section(&mut ui, Section::Params);
    ui.resize(720.0, 200.0, 1.0);
    ui.take_commands();
    for _ in 0..100 {
        ui.input(Input::Scroll { delta: 1.0 });
        assert!(!ui.take_commands().iter().any(|c| c["type"] == "viewState"));
    }
    ui.input(Input::FocusLost);
    let commands = ui.take_commands();
    let saved = &commands.iter().find(|c| c["type"] == "viewState").unwrap()["state"];
    assert_eq!(
        saved["eventValues"][0]["values"][0]
            .as_array()
            .unwrap()
            .len(),
        65536
    );
    assert_eq!(saved["scrollTop"], json!(ui.scroll));
}
#[test]
fn log_is_virtualized_and_follow_tail_can_be_suspended() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"logText":(0..10000).map(|i|format!("Line {i}\n")).collect::<String>()}}));
    show_section(&mut ui, Section::Log);
    ui.prepare_scene();
    assert!(
        ui.scene
            .labels
            .iter()
            .filter(|l| l.text.starts_with("Line "))
            .count()
            <= 9
    );
    ui.input(Input::PointerMove {
        x: ui.scene.log_rect[0] + 10.0,
        y: ui.scene.log_rect[1] + 10.0,
        fine: false,
    });
    assert!(ui.scene.labels.iter().any(|l| l.text == "Line 9999"));
    ui.input(Input::Scroll { delta: -200.0 });
    assert!(!ui.presentation.log_follow);
    ui.input(Input::HorizontalScroll { delta: 300.0 });
    assert_eq!(ui.presentation.log_x, 0.0, "short lines must stay visible");
}
#[test]
fn integer_arrow_steps_and_settings_validation_keep_exact_values() {
    let mut ui = fixture();
    ui.begin_field(Field::Event(EventField {
        event: 0,
        arg: 0,
        path: "/id".into(),
        scalar: "i64".into(),
    }));
    ui.input(Input::Key {
        key: Key::ArrowDown,
        shift: false,
    });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert_eq!(ui.events[0].args[0].value["id"], "9223372036854775806");
    assert!(ui
        .parse_field(&Field::Setting(Setting::SampleRate), "NaN")
        .is_none());
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    edit(&mut ui, Field::Setting(Setting::ParamSmoothing), "12.5");
    assert!(ui
        .take_commands()
        .iter()
        .any(|v| v["type"] == "setParamSmoothing" && v["milliseconds"] == 13.0));
}
#[test]
fn sections_collapse_independently_and_scope_updates_retain_the_scene() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"buffers":[],"events":[],"inputControls":[]}}));
    ui.set_scope_interleaved(2, &[0.0; 4]);
    for size in [[720.0, 840.0], [360.0, 400.0]] {
        ui.resize(size[0], size[1], 2.0);
        for section in [Section::Scope, Section::Params, Section::Midi] {
            if section == Section::Params && size[0] < 400.0 {
                ui.input(Input::Scroll { delta: 200.0 });
            }
            click(&mut ui, |t| matches!(t,Target::Section(s) if *s==section));
            assert!(!ui.presentation.open(section), "{section:?} at {size:?}");
            click(&mut ui, |t| matches!(t,Target::Section(s) if *s==section));
            ui.prepare_scene();
            let revision = ui.revisions().0;
            ui.set_scope_interleaved(2, &[0.1, 0.2, 0.3, 0.4]);
            assert_eq!(ui.revisions().0, revision);
            assert!(!ui.scene_dirty);
        }
    }
}

#[test]
fn stopping_transport_or_disabling_midi_releases_computer_notes() {
    for state in [json!({"running":false}), json!({"midi":{"noteOn":false}})] {
        let mut ui = fixture();
        ui.state["running"] = json!(true);
        show_section(&mut ui, Section::Midi);
        ui.take_commands();
        ui.input(Input::ComputerKey {
            key: "a".into(),
            pressed: true,
        });
        ui.take_commands();
        ui.message(json!({"type":"state","state":state}));
        assert_eq!(
            ui.take_commands()
                .iter()
                .filter(|c| c["type"] == "midiNote" && c["pressed"] == false)
                .count(),
            1
        );
        assert!(!ui.interaction.midi().held(60));
    }
}

#[test]
fn pasted_json_normalizes_wide_integers_before_crossing_the_browser_bridge() {
    let mut ui = fixture();
    edit(
        &mut ui,
        Field::Event(EventField {
            event: 0,
            arg: 0,
            path: String::new(),
            scalar: "json".into(),
        }),
        "{\n \"id\": 9223372036854775807,\n \"levels\": [0.5]\n}",
    );
    assert!(!ui.editing());
    assert_eq!(ui.events[0].args[0].value["id"], "9223372036854775807");
}

#[test]
fn hidden_logs_retain_the_active_scene_and_show_current_output_when_opened() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"logRevealed":true}}));
    ui.presentation.sections[Section::Log.index()] = false;
    ui.prepare_scene();
    let revision = ui.revisions();
    ui.message(json!({"type":"state","state":{"logText":"new output\n","printOverflowCount":2}}));
    assert_eq!(ui.revisions(), revision);
    assert!(!ui.scene_dirty);
    show_section(&mut ui, Section::Log);
    ui.prepare_scene();
    assert!(ui.scene.labels.iter().any(|l| l.text == "new output"));
}

#[test]
fn tab_reaches_offscreen_controls_and_scrolling_preserves_the_midi_dock() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{"inputControls":[],"events":[],"buffers":[],
        "params":(0..60).map(|i|json!({"name":format!("p{i}"),"type":"f64","value":0,"rangeMin":0,"rangeMax":1})).collect::<Vec<_>>()}}));
    ui.resize(360.0, 400.0, 2.0);
    for _ in 0..20 {
        if ui
            .editor
            .draft()
            .is_some_and(|d| d.target == Field::Control(0))
        {
            break;
        }
        ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
    }
    for index in 0..20 {
        assert_eq!(ui.editor.draft().unwrap().target, Field::Control(index));
        ui.prepare_scene();
        assert!(ui.scene.hits.iter().any(
            |h| matches!(h.target,Target::Field(Field::Control(i)) if i==index)
                && h.rect.0[3] > 0.0
        ));
        ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
    }
    assert!(ui.scroll > 0.0);
    let rect = |ui: &RunUi| {
        ui.scene
            .hits
            .iter()
            .find(|h| matches!(h.target, Target::Note(60)))
            .unwrap()
            .rect
    };
    let before = rect(&ui);
    ui.input(Input::Scroll { delta: 200.0 });
    ui.prepare_scene();
    assert_eq!(rect(&ui), before);
}

#[test]
fn collapsed_log_invalidates_only_when_its_header_appears_or_disappears() {
    let mut ui = fixture();
    ui.presentation.sections[Section::Log.index()] = false;
    ui.prepare_scene();
    assert!(!ui
        .scene
        .hits
        .iter()
        .any(|hit| hit.target == Target::Section(Section::Log)));
    let revision = ui.revisions();
    ui.message(json!({"type":"state","state":{"logText":"first\n","logRevealed":true}}));
    assert_ne!(ui.revisions(), revision);
    ui.prepare_scene();
    assert!(ui
        .scene
        .hits
        .iter()
        .any(|hit| hit.target == Target::Section(Section::Log)));
    let revision = ui.revisions();
    ui.message(json!({"type":"state","state":{"logText":"second\n"}}));
    assert_eq!(ui.revisions(), revision);
    ui.message(json!({"type":"state","state":{"logText":"","logRevealed":false}}));
    assert_ne!(ui.revisions(), revision);
    ui.prepare_scene();
    assert!(!ui
        .scene
        .hits
        .iter()
        .any(|hit| hit.target == Target::Section(Section::Log)));
}
#[test]
fn log_copy_uses_the_complete_current_output_including_offscreen_lines() {
    let mut ui = RunUi::default();
    ui.resize(320.0, 400.0, 1.0);
    let log = (0..100)
        .map(|i| format!("Line {i}: 世界\n"))
        .collect::<String>();
    ui.message(serde_json::json!({"type":"state","state":{
        "path":"log.onda","supportsScope":false,"logText":log
    }}));
    ui.prepare_scene();
    let r = ui
        .scene
        .labels
        .iter()
        .find(|l| l.text == "Copy")
        .expect("copy action");
    let [x, y] = [r.x + 1.0, r.y + 1.0];
    ui.input(Input::PointerDown { x, y, clicks: 1 });
    ui.input(Input::PointerUp);
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "copyText" && c["text"] == log));
    ui.message(serde_json::json!({"type":"state","state":{"logText":"Newest output\n"}}));
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert!(ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "copyText" && c["text"] == "Newest output\n"));
    assert!(
        !matches!(ui.interaction.focus().cloned(), Some(Target::Section(_))),
        "Copy must own keyboard focus"
    );
}
