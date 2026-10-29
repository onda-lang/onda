use crate::{Input, Key, RunUi, editing::Field, scene::Target};
use serde_json::{Value, json};

fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "path":"midi.onda","running":true,"connected":true,"supportsScope":false,
        "midi":{"noteOn":true,"noteOff":true},"currentMidiInputDevice":"Computer Keyboard",
        "params":[{"name":"gain","type":"f64","value":0.0}]
    }}));
    ui.resize(720.0, 840.0, 1.0);
    ui
}
fn keyboard(ui: &mut RunUi, key: &str, pressed: bool) {
    ui.input(Input::ComputerKey {
        key: key.into(),
        pressed,
    });
}
fn notes(ui: &mut RunUi) -> Vec<Value> {
    ui.take_commands()
        .into_iter()
        .filter(|v| v["type"] == "midiNote")
        .collect()
}
fn position(ui: &mut RunUi, note: u8) -> (f32, f32) {
    ui.prepare_scene();
    let r = ui
        .scene
        .hits
        .iter()
        .find(|h| h.target == Target::Note(note))
        .unwrap()
        .rect
        .0;
    (r[0] + r[2] * 0.5, r[1] + r[3] - 4.0)
}

#[test]
fn idle_plugin_leaves_unused_keyboard_input_to_the_host() {
    let mut ui = fixture();
    ui.message(json!({"type":"state","state":{
        "supportsTransport":false,"showComputerKeyLabels":false,
        "currentMidiInputDevice":null,"midiKeyboardInteractive":true
    }}));
    ui.take_commands();
    for text in [" ", "a", "x"] {
        assert!(!ui.input(Input::Text { text: text.into() }));
    }
    for key in [
        Key::Enter,
        Key::Escape,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::Home,
        Key::End,
        Key::Backspace,
        Key::Delete,
        Key::Copy,
        Key::Cut,
        Key::SelectAll,
        Key::Play,
        Key::Stop,
    ] {
        assert!(!ui.input(Input::Key { key, shift: false }));
    }
    assert!(ui.take_commands().is_empty());
    let (x, y) = position(&mut ui, 60);
    ui.input(Input::PointerDown { x, y, clicks: 1 });
    ui.input(Input::PointerUp);
    let played = notes(&mut ui);
    assert_eq!(played.len(), 2, "the on-screen piano remains interactive");
    assert_eq!(played[0]["pressed"], true);
    assert_eq!(played[1]["pressed"], false);
}

#[test]
fn editing_navigation_and_menu_search_consume_keyboard_input() {
    let mut ui = fixture();
    assert!(ui.input(Input::Key {
        key: Key::Tab,
        shift: false
    }));
    ui.begin_field(Field::Control(0));
    assert!(
        ui.input(Input::Text { text: "\n".into() }),
        "rejected draft text must not leak to host shortcuts"
    );
    assert!(ui.input(Input::Text { text: "0.5".into() }));
    assert!(ui.input(Input::Key {
        key: Key::SelectAll,
        shift: false
    }));
    assert!(ui.input(Input::Key {
        key: Key::Copy,
        shift: false
    }));
    assert!(
        ui.take_commands()
            .iter()
            .any(|c| c["type"] == "copyText" && c["text"] == "0.5")
    );
    assert!(ui.input(Input::Key {
        key: Key::Enter,
        shift: false
    }));
    assert!(
        ui.take_commands()
            .iter()
            .any(|c| c["type"] == "setParam" && c["value"] == 0.5)
    );
    assert!(!ui.input(Input::Text { text: " ".into() }));
    ui.message(json!({"type":"state","state":{
        "supportsRunSettings":true,"canSetSampleRate":true,"sampleRateHz":48000
    }}));
    ui.toggle_settings();
    ui.prepare_scene();
    ui.open_select(crate::interaction::SelectId::SampleRate);
    assert!(ui.interaction.menu().is_some());
    assert!(!ui.input(Input::Key {
        key: Key::Copy,
        shift: false
    }));
    assert!(ui.input(Input::Text { text: "48".into() }));
    assert!(ui.input(Input::Key {
        key: Key::ArrowDown,
        shift: false
    }));
    assert!(ui.input(Input::Key {
        key: Key::Escape,
        shift: false
    }));
    assert!(ui.interaction.menu().is_none());
    assert!(ui.input(Input::Key {
        key: Key::Escape,
        shift: false
    }));
    assert!(!ui.input(Input::Key {
        key: Key::Escape,
        shift: false
    }));
}

#[test]
fn touch_chords_glissando_and_shared_notes_keep_independent_owners() {
    let mut ui = fixture();
    let (x, y) = position(&mut ui, 60);
    ui.input(Input::TouchStart { id: 1, x, y });
    let (other, y) = position(&mut ui, 64);
    ui.input(Input::TouchStart { id: 2, x: other, y });
    ui.input(Input::TouchStart { id: 3, x, y });
    let starts = notes(&mut ui);
    assert_eq!(
        starts
            .iter()
            .map(|v| v["key"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [60, 64]
    );
    ui.input(Input::TouchEnd { id: 1 });
    assert!(notes(&mut ui).is_empty(), "another finger still owns C");
    ui.input(Input::TouchMove { id: 2, x, y });
    let released = notes(&mut ui);
    assert_eq!(released.len(), 1);
    assert_eq!(
        (
            released[0]["key"].as_u64(),
            released[0]["pressed"].as_bool()
        ),
        (Some(64), Some(false))
    );
    ui.input(Input::TouchCancel { id: 3 });
    assert!(notes(&mut ui).is_empty());
    keyboard(&mut ui, "a", true);
    ui.input(Input::TouchEnd { id: 2 });
    assert!(notes(&mut ui).is_empty(), "keyboard still owns C");
    keyboard(&mut ui, "a", false);
    let released = notes(&mut ui);
    assert_eq!(released.len(), 1);
    assert_eq!(released[0]["key"], 60);
    assert_eq!(released[0]["pressed"], false);
}

#[test]
fn octave_shortcuts_ignore_repeat_release_old_notes_and_respect_editing() {
    let mut ui = fixture();
    keyboard(&mut ui, "a", true);
    notes(&mut ui);
    keyboard(&mut ui, "x", true);
    keyboard(&mut ui, "x", true);
    assert_eq!(ui.interaction.midi().octave, 5);
    assert_eq!(notes(&mut ui)[0]["pressed"], false);
    keyboard(&mut ui, "a", true);
    assert!(
        notes(&mut ui).is_empty(),
        "a held note must not restart on auto-repeat after an octave shift"
    );
    keyboard(&mut ui, "a", false);
    keyboard(&mut ui, "a", true);
    assert_eq!(notes(&mut ui)[0]["key"], 72);
    keyboard(&mut ui, "x", false);
    keyboard(&mut ui, "x", true);
    assert_eq!(ui.interaction.midi().octave, 6);
    ui.begin_field(Field::Control(0));
    keyboard(&mut ui, "z", true);
    assert_eq!(ui.interaction.midi().octave, 6);
    ui.input(Input::FocusLost);
    assert!(ui.interaction.midi().pressed_keys.is_empty());
    assert!(ui.interaction.midi().keyboard.is_empty());
}

#[test]
fn source_changes_and_presentation_resets_restore_midi_defaults() {
    for state in [
        json!({"path":"another.onda"}),
        json!({"viewState":{"reset":true}}),
    ] {
        let mut ui = fixture();
        ui.message(json!({"type":"state","state":{
            "viewState":{"octave":2,"velocity":0.25}
        }}));
        ui.message(json!({"type":"state","state":{"status":"Playing"}}));
        keyboard(&mut ui, "a", true);
        assert_eq!(
            notes(&mut ui),
            [json!({"type":"midiNote","key":36,"velocity":0.25,"pressed":true})]
        );

        ui.message(json!({"type":"state","state":state}));
        assert_eq!(
            notes(&mut ui),
            [json!({"type":"midiNote","key":36,"velocity":0.0,"pressed":false})],
            "resetting presentation must release notes from the previous octave"
        );
        keyboard(&mut ui, "a", true);
        assert_eq!(
            notes(&mut ui),
            [json!({"type":"midiNote","key":60,"velocity":1.0,"pressed":true})]
        );

        ui.message(json!({"type":"state","state":{
            "path":"saved.onda","viewState":{"octave":5,"velocity":0.75}
        }}));
        notes(&mut ui);
        keyboard(&mut ui, "a", true);
        assert_eq!(
            notes(&mut ui),
            [json!({"type":"midiNote","key":72,"velocity":0.75,"pressed":true})],
            "a saved source must retain its explicit MIDI preferences"
        );
    }
}

#[test]
fn ime_preedit_is_reversible_and_never_publishes_an_uncommitted_value() {
    let mut ui = fixture();
    ui.begin_field(Field::Control(0));
    ui.input(Input::Preedit {
        text: "12".into(),
        anchor: 0,
        cursor: 2,
    });
    assert_eq!(ui.editor.draft().unwrap().display().0, "12");
    assert_eq!(ui.editor.draft().unwrap().text, "0");
    assert!(!ui.commit_draft());
    assert!(ui.take_commands().is_empty());
    ui.input(Input::Preedit {
        text: String::new(),
        anchor: 0,
        cursor: 0,
    });
    assert_eq!(ui.editor.draft().unwrap().display().0, "0");
    ui.input(Input::Preedit {
        text: "😀".into(),
        anchor: 2,
        cursor: 2,
    });
    let (_, cursor, _, underline) = ui.editor.draft().unwrap().display();
    assert_eq!(cursor, 4);
    assert_eq!(underline, Some([0, 4]));
    ui.input(Input::Text { text: "12".into() });
    ui.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert_eq!(ui.controls[0].value, 12.0);
    assert!(ui.editor.draft().is_none());
    assert_eq!(ui.take_commands()[0]["value"], 12.0);
}

#[test]
fn clicking_the_active_draft_does_not_commit_or_discard_it() {
    let mut ui = fixture();
    ui.begin_field(Field::Control(0));
    for text in ["678.9", "invalid"] {
        ui.input(Input::EditText {
            text: text.into(),
            anchor: 2,
            cursor: 2,
        });
        let edit = ui.text_edit().unwrap();
        let rect = edit["rect"].as_array().unwrap();
        ui.input(Input::PointerDown {
            x: rect[0].as_f64().unwrap() as f32 + 20.0,
            y: rect[1].as_f64().unwrap() as f32 + 10.0,
            clicks: 1,
        });
        ui.input(Input::PointerUp);
        assert_eq!(ui.text_edit().unwrap()["text"], text);
        assert!(ui.take_commands().is_empty());
        assert_eq!(ui.controls[0].value, 0.0);
    }
}

#[test]
fn focus_loss_commits_valid_drafts_and_releases_notes_without_committing_invalid_or_composing_text()
{
    for (text, composing, expected) in [
        ("0.75", false, Some(0.75)),
        ("invalid", false, None),
        ("0.75", true, None),
    ] {
        let mut ui = fixture();
        keyboard(&mut ui, "a", true);
        ui.take_commands();
        ui.begin_field(Field::Control(0));
        ui.input(Input::Text { text: text.into() });
        if composing {
            ui.input(Input::Preedit {
                text: "9".into(),
                anchor: 0,
                cursor: 1,
            });
        }
        ui.input(Input::FocusLost);
        let commands = ui.take_commands();
        let edits: Vec<_> = commands
            .iter()
            .filter(|c| c["type"] == "setParam")
            .collect();
        if let Some(value) = expected {
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0]["value"], value);
            assert_eq!(edits[0]["commit"], true);
        } else {
            assert!(edits.is_empty());
        }
        assert_eq!(ui.controls[0].value, expected.unwrap_or(0.0));
        assert!(!ui.editing());
        assert!(
            commands
                .iter()
                .any(|c| c["type"] == "midiNote" && c["pressed"] == false)
        );
        assert!(ui.interaction.midi().keyboard.is_empty());
    }
}
#[test]
fn clicking_an_event_trigger_does_not_leave_keyboard_focus_highlighting() {
    use crate::scene::Target;
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "path":"events.onda","supportsScope":false,
        "events":[{"name":"play","args":[{"name":"start_frame","type":"f32","value":0}]}]
    }}));
    ui.prepare_scene();
    let rect = ui
        .scene
        .hits
        .iter()
        .find(|h| h.target == Target::Trigger(0))
        .unwrap()
        .rect
        .0;
    let fill = |ui: &RunUi| {
        let index = ui.scene.shapes.iter().position(|s| s.rect == rect).unwrap();
        ui.scene.shapes[index + 1].color
    };
    let normal = fill(&ui);
    ui.input(Input::PointerDown {
        x: rect[0] + 10.0,
        y: rect[1] + 10.0,
        clicks: 1,
    });
    ui.input(Input::PointerUp);
    ui.input(Input::PointerMove {
        x: 0.0,
        y: 0.0,
        fine: false,
    });
    ui.prepare_scene();
    assert_eq!(
        fill(&ui),
        normal,
        "a mouse click must not leave the trigger highlighted after the pointer leaves"
    );
    assert_eq!(
        ui.take_commands()
            .iter()
            .filter(|c| c["type"] == "triggerEvent")
            .count(),
        1
    );

    for shift in [false, true] {
        ui.input(Input::Key {
            key: Key::Tab,
            shift,
        });
    }
    ui.prepare_scene();
    assert_eq!(ui.interaction.focus().cloned(), Some(Target::Trigger(0)));
    assert_ne!(
        fill(&ui),
        normal,
        "Tab navigation must retain a visible focus indicator"
    );
}

#[test]
fn schema_replacement_ends_a_numeric_drag_and_releases_owned_notes_once() {
    let mut ui = fixture();
    keyboard(&mut ui, "a", true);
    ui.prepare_scene();
    let field = ui
        .scene
        .hits
        .iter()
        .find(|hit| hit.target == Target::Field(Field::Control(0)))
        .unwrap()
        .rect
        .0;
    let x = field[0] + field[2] * 0.5;
    let y = field[1] + field[3] * 0.5;
    ui.input(Input::PointerDown { x, y, clicks: 1 });
    ui.input(Input::PointerMove {
        x: x + 20.0,
        y,
        fine: false,
    });
    assert!(
        ui.take_commands()
            .iter()
            .any(|command| { command["type"] == "setParam" && command["commit"] == false })
    );

    ui.message(json!({"type":"state","state":{
        "params":[{"name":"replacement","type":"f64","value":0.5}]
    }}));
    assert_eq!(
        ui.take_commands(),
        vec![
            json!({"type":"endParamGesture","name":"gain"}),
            json!({"type":"midiNote","key":60,"pressed":false,"velocity":0.0}),
        ]
    );
    ui.input(Input::FocusLost);
    ui.input(Input::PointerUp);
    assert!(ui.take_commands().is_empty());
}
