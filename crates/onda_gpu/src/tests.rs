use super::*;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a native Wayland display"]
fn standalone_prefers_native_wayland_over_the_xwayland_bridge() {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use winit::platform::x11::EventLoopBuilderExtX11;
    let event_loop = event_loop_builder().with_any_thread(true).build().unwrap();
    assert!(
        matches!(
            event_loop
                .owned_display_handle()
                .display_handle()
                .unwrap()
                .as_raw(),
            RawDisplayHandle::Wayland(_)
        ),
        "native Wayland file offers must not cross the Xwayland drag/drop bridge"
    );
}

#[test]
fn ime_composition_preserves_computer_key_releases() {
    let mut app = App::new(None, RunHostOptions::default());
    app.ui.message(json!({"type":"state","state":{
        "path":"midi.onda","supportsScope":false,"supportsTransport":false,"supportsViewState":false,
        "supportsParamSmoothing":false,
        "currentMidiInputDevice":"Computer Keyboard","midi":{"noteOn":true},
        "params":[{"name":"gain","type":"f64","value":0.5}]
    }}));
    let event = |state: ElementState| winit::event::KeyEvent {
        physical_key: PhysicalKey::Code(KeyCode::KeyA),
        logical_key: WinitKey::Character("a".into()),
        key_without_modifiers: WinitKey::Character("a".into()),
        text: state.is_pressed().then(|| "a".into()),
        text_with_all_modifiers: state.is_pressed().then(|| "a".into()),
        location: winit::keyboard::KeyLocation::Standard,
        state,
        repeat: false,
    };
    app.ui.input(Input::ComputerKey {
        key: "a".into(),
        pressed: true,
    });
    assert_eq!(
        app.ui.take_commands(),
        vec![json!({
            "type":"midiNote","key":60,"pressed":true,"velocity":1.0
        })]
    );
    // Entering a field keeps the previously held piano key owned until release.
    for _ in 0..16 {
        app.ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        if app.ui.editing() {
            break;
        }
    }
    assert!(app.ui.editing());
    assert!(app.ui.take_commands().is_empty());
    app.composing = true;
    app.ui.input(Input::Preedit {
        text: "あ".into(),
        anchor: 0,
        cursor: 1,
    });
    app.keyboard_input(event(ElementState::Pressed));
    assert_eq!(
        app.ui.text_edit().unwrap()["text"],
        "0.5",
        "IME owns composing key presses"
    );
    app.keyboard_input(event(ElementState::Released));
    app.ui.input(Input::ReleaseComputerKeys);
    assert!(
        app.ui.take_commands().is_empty(),
        "the physical release must already have released the note"
    );
}

#[test]
fn hovering_wakes_an_idle_host_once_at_the_tooltip_deadline() {
    let mut app = App::new(None, RunHostOptions::default());
    app.resize_surface(720, 840, 1.0);
    app.ui.message(json!({"type":"state","state":{
        "path":"hover.onda","supportsScope":false
    }}));
    app.input(Input::PointerMove {
        x: 100.0,
        y: 35.0,
        fine: false,
    });
    let deadline = app.ui.repaint_deadline().unwrap();
    app.rendered = Some(app.ui.revisions());
    assert!(!app.needs_redraw(Instant::now()));
    assert_eq!(app.next_wake(), Some(deadline));
    let delay = deadline.saturating_duration_since(Instant::now());
    assert!(delay > Duration::from_millis(400) && delay <= Duration::from_millis(500));
    app.input(Input::PointerLeave);
    assert_eq!(app.next_wake(), None);
}

fn source_controller(path: &Path) -> RunController {
    std::fs::write(path, "sample:\n  out1 = 0.0\n").unwrap();
    RunController::new(
        path,
        RunHostOptions {
            onda_bin: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn stopped_and_replaced_controller_state_clears_the_retained_scope() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("source.onda");
    for update in ["stop", "runtime", "schema"] {
        let mut app = App::new(None, RunHostOptions::default());
        app.controller = Some(source_controller(&path));
        app.controller.as_mut().unwrap().stop();
        app.ui.message(
            json!({"type":"state","state":{"path":path,"running":true,"supportsScope":true}}),
        );
        app.ui.set_scope_interleaved(1, &[0.25, -0.5]);
        match update {
            "stop" => app.handle_commands(vec![json!({"type":"stop"})]),
            _ => app.publish_changes(onda_run::PollResult {
                state_changed: true,
                schema_changed: update == "schema",
                ..Default::default()
            }),
        }
        let cleared = app.ui.revisions();
        app.ui.set_scope_interleaved(0, &[]);
        assert_eq!(
            app.ui.revisions(),
            cleared,
            "{update} must already have cleared the scope"
        );
    }
}

#[test]
fn log_only_publications_preserve_drafts_and_the_folded_scene() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("source.onda");
    let mut app = App::new(None, RunHostOptions::default());
    app.controller = Some(source_controller(&path));
    app.controller.as_mut().unwrap().stop();
    app.ui.message(json!({"type":"state","state":{
        "path":path,"supportsScope":false,
        "viewState":{"sections":{"log":false}},
        "params":[{"name":"gain","type":"f64","value":0.5}],
        "events":[{"name":"send","args":[{"name":"values","type":"f64[]","value":vec![0.5;4096]}]}]
    }}));
    for _ in 0..16 {
        app.ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        if app.ui.editing() {
            break;
        }
    }
    assert!(app.ui.editing());
    let revision = app.ui.revisions();
    app.publish_changes(onda_run::PollResult {
        log_changed: true,
        ..Default::default()
    });
    assert_eq!(app.ui.revisions(), revision);
    assert!(app.ui.editing());

    app.publish_changes(onda_run::PollResult {
        state_changed: true,
        ..Default::default()
    });
    assert!(
        app.ui.editing(),
        "runtime status must preserve the current draft"
    );

    app.publish_changes(onda_run::PollResult {
        state_changed: true,
        schema_changed: true,
        log_changed: true,
        ..Default::default()
    });
    assert_ne!(app.ui.revisions(), revision);
    assert!(
        !app.ui.editing(),
        "a simultaneous schema change must still publish"
    );
}

#[test]
fn transport_shortcuts_ignore_repeats_without_swallowing_other_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("source.onda");
    for modifiers in [
        winit::keyboard::ModifiersState::CONTROL,
        winit::keyboard::ModifiersState::META,
    ] {
        let mut app = App::new(None, RunHostOptions::default());
        app.controller = Some(source_controller(&path));
        app.modifiers = modifiers;
        for key in [
            WinitKey::Named(NamedKey::Enter),
            WinitKey::Character("s".into()),
        ] {
            app.controller.as_mut().unwrap().stop();
            assert!(app.transport_shortcut(&key, true));
            assert!(!app.controller.as_ref().unwrap().state().running);
            assert!(app.transport_shortcut(&key, false));
            assert!(app.controller.as_ref().unwrap().state().running);
        }
        let stop = WinitKey::Character(".".into());
        assert!(app.transport_shortcut(&stop, true));
        assert!(app.controller.as_ref().unwrap().state().running);
        assert!(app.transport_shortcut(&stop, false));
        assert!(!app.controller.as_ref().unwrap().state().running);
        assert!(!app.transport_shortcut(&WinitKey::Character("c".into()), false));
        app.modifiers = Default::default();
        assert!(!app.transport_shortcut(&WinitKey::Named(NamedKey::Enter), false));
    }
}

#[test]
fn zero_size_and_occlusion_suspend_redraws_until_the_surface_returns() {
    let mut app = App::new(None, RunHostOptions::default());
    let now = Instant::now();
    for size in [[0, 0], [0, 840], [720, 0]] {
        app.resize_surface(size[0], size[1], 1.0);
        assert!(!app.surface_visible());
        assert!(!app.needs_redraw(now));
        assert_eq!(app.next_wake(), None);
    }
    app.resize_surface(720, 840, 1.0);
    assert!(app.surface_visible());
    assert!(app.needs_redraw(now));
    app.occluded = true;
    assert!(!app.surface_visible());
    assert!(!app.needs_redraw(now));
    app.occluded = false;
    app.rendered = Some(app.ui.revisions());
    assert!(!app.needs_redraw(now));
    assert!(app.redraw_ready(now), "OS exposure must repaint clean UI");
}

#[test]
fn unavailable_surface_retries_wake_an_idle_host_without_spinning() {
    let mut app = App::new(None, RunHostOptions::default());
    app.resize_surface(720, 840, 1.0);
    let now = Instant::now();
    let deadline = now + SCOPE_INTERVAL;
    app.redraw_retry = Some(deadline);
    assert!(!app.redraw_ready(now));
    assert_eq!(app.next_wake(), Some(deadline));
    assert!(app.needs_redraw(deadline));
    app.resize_surface(0, 0, 1.0);
    assert_eq!(app.next_wake(), None);
    app.resize_surface(720, 840, 1.0);
    assert!(
        app.needs_redraw(now),
        "restoring size must not wait for the old retry"
    );
}

#[test]
fn deferred_presentation_wakes_an_idle_host_and_flushes_on_focus_loss() {
    let mut app = App::new(None, RunHostOptions::default());
    app.ui.message(json!({"type":"state","state":{
        "path":"scroll.onda","supportsScope":false,
        "params":(0..60).map(|i| json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>()
    }}));
    app.ui.input(Input::Scroll { delta: 20.0 });
    app.commands();
    let deadline = app.ui.view_state_deadline().unwrap();
    assert_eq!(app.next_wake(), Some(deadline));
    assert!(!app.views.contains_key("scroll.onda"));
    app.input(Input::FocusLost);
    assert_eq!(app.views["scroll.onda"]["scrollTop"], 20.0);
    assert_eq!(app.next_wake(), None);
}

#[test]
fn reopening_a_source_restores_its_own_layout() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(None, RunHostOptions::default());
    for (name, layout) in [
        ("a.onda", "sliders"),
        ("b.onda", "knobs"),
        ("a.onda", "sliders"),
    ] {
        let path = directory.path().canonicalize().unwrap().join(name);
        app.controller = Some(source_controller(&path));
        let view_path = app.controller.as_ref().unwrap().state().path.clone();
        app.views
            .entry(view_path)
            .or_insert_with(|| json!({"paramLayout":layout}));
        app.publish();
        assert_eq!(app.panel_state()["schemaReady"], false);
        app.ui.input(Input::Scroll { delta: 0.0 });
        assert!(app.ui.take_commands().is_empty());
        app.controller.as_mut().unwrap().stop();
        app.publish();
        app.ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        let view = app
            .ui
            .take_commands()
            .into_iter()
            .find(|v| v["type"] == "viewState")
            .unwrap();
        assert_eq!(view["state"]["paramLayout"], layout);
    }
}

#[test]
fn reopening_the_current_source_restores_event_values_without_replaying_ordinary_updates() {
    fn publish_ready(app: &mut App) {
        let mut state = app.panel_state();
        state["schemaReady"] = json!(true);
        state["supportsScope"] = json!(false);
        state["events"] = json!([{"name":"send","args":[{
            "name":"value","type":"i64","default":"0"
        }]}]);
        app.ui.message(json!({"type":"state","state":state}));
    }
    fn focus_argument(app: &mut App) {
        for _ in 0..16 {
            app.input(Input::Key {
                key: Key::Tab,
                shift: false,
            });
            if app.ui.editing() {
                return;
            }
        }
        panic!("event argument must be reachable");
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("source.onda");
    std::fs::write(&path, "sample:\n  out1 = 0.0\n").unwrap();
    let mut app = App::new(
        None,
        RunHostOptions {
            onda_bin: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        },
    );
    app.drop_files(std::slice::from_ref(&path), None);
    let view_path = app.controller.as_ref().unwrap().state().path.clone();
    publish_ready(&mut app);
    focus_argument(&mut app);
    app.input(Input::Text { text: "77".into() });
    app.input(Input::Key {
        key: Key::Enter,
        shift: false,
    });
    assert_eq!(app.views[&view_path]["eventValues"][0]["values"][0], "77");

    for _ in 0..2 {
        app.drop_files(std::slice::from_ref(&path), None);
        assert_eq!(app.panel_state()["schemaReady"], false);
        publish_ready(&mut app);
        focus_argument(&mut app);
        assert_eq!(app.ui.text_edit().unwrap()["text"], "77");
        app.input(Input::Text { text: "88".into() });
        publish_ready(&mut app);
        assert_eq!(
            app.ui.text_edit().unwrap()["text"],
            "88",
            "ordinary publications must retain the active draft"
        );
        app.input(Input::Key {
            key: Key::Escape,
            shift: false,
        });
    }
}

#[test]
fn parameter_and_midi_commands_preserve_the_ui_schema_and_retained_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("source.onda");
    let mut app = App::new(None, RunHostOptions::default());
    app.controller = Some(source_controller(&path));
    app.ui.message(
        json!({"type":"state","state":{"path":path,"supportsScope":false,
        "params":[{"name":"gain","type":"f64","value":0.5}],"logText":"retained output"}}),
    );
    let revision = app.ui.revisions();
    app.handle_commands(vec![
        json!({"type":"midiNote","key":60,"pressed":true,"velocity":0.8}),
    ]);
    assert_eq!(
        app.ui.revisions(),
        revision,
        "MIDI must not rebuild structural state"
    );
    app.handle_commands(vec![json!({"type":"setParam","name":"gain","value":0.75})]);
    for _ in 0..16 {
        app.ui.input(Input::Key {
            key: Key::Tab,
            shift: false,
        });
        if let Some(edit) = app.ui.text_edit() {
            if edit["text"] == "0.75" {
                return;
            }
        }
    }
    panic!("A value update replaced the current parameter schema");
}

#[test]
fn failed_initial_load_retains_the_source_picker() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("missing.onda");
    let mut app = App::new(Some(&missing), RunHostOptions::default());
    app.open(missing);
    assert!(app.controller.is_none());
    // The empty screen's Open button must still produce its host command.
    app.ui.input(Input::PointerDown {
        x: 360.0,
        y: 420.0,
        clicks: 1,
    });
    app.ui.input(Input::PointerUp);
    assert!(app
        .ui
        .take_commands()
        .iter()
        .any(|c| c["type"] == "chooseOndaFile"));
}

#[test]
fn smoothing_reset_keeps_the_exact_launch_default_after_edits_and_unload() {
    let mut app = App::new(
        None,
        RunHostOptions {
            param_smoothing_seconds: 0.1234,
            ..Default::default()
        },
    );
    app.handle_commands(vec![json!({"type":"setParamSmoothing","milliseconds":42})]);
    app.handle_commands(vec![json!({"type":"unload"})]);
    let state = app.panel_state();
    assert_eq!(state["paramSmoothingDefaultMs"], json!(0.1234 * 1000.0));
    assert_eq!(state["paramSmoothingMs"], 42.0);
}

#[test]
fn unloading_preserves_the_controllers_device_preferences() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().canonicalize().unwrap().join("source.onda");
    std::fs::write(&source, "sample:\n  out1 = 0.0\n").unwrap();
    // This test executable exits on the CLI arguments; no audio device is opened.
    let options = RunHostOptions {
        onda_bin: std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        input_device: Some("Chosen input".into()),
        output_device: Some("Chosen output".into()),
        midi_input_device: Some("Chosen MIDI".into()),
        ..Default::default()
    };
    let mut app = App::new(None, RunHostOptions::default());
    app.controller = Some(RunController::new(&source, options.clone()).unwrap());
    app.handle_commands(vec![json!({"type":"unload"})]);
    assert!(app.controller.is_none());
    assert_eq!(app.options.input_device, options.input_device);
    assert_eq!(app.options.output_device, options.output_device);
    assert_eq!(app.options.midi_input_device, options.midi_input_device);
}

#[test]
fn source_drops_open_the_source_even_over_a_buffer_card() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("new.onda");
    std::fs::write(&path, "sample:\n  out1 = 0.0\n").unwrap();
    let mut app = App::new(
        None,
        RunHostOptions {
            onda_bin: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        },
    );
    app.ui.message(
        json!({"type":"state","state":{"path":"old.onda","supportsScope":false,
        "buffers":[{"name":"audio","type":"f32"}]}}),
    );
    assert_eq!(app.ui.buffer_target(50.0, 150.0).as_deref(), Some("audio"));
    app.drop_files(std::slice::from_ref(&path), Some("audio".into()));
    assert_eq!(app.path, Some(path));
    assert!(app.controller.is_some());
}

#[test]
fn unsupported_and_multiple_file_drops_never_open_as_sources() {
    let mut app = App::new(None, RunHostOptions::default());
    for paths in [
        vec!["audio.wav"],
        vec!["notes.txt"],
        vec!["a.onda", "b.onda"],
    ] {
        app.drop_files(
            &paths.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
            Some("audio".into()),
        );
        assert!(app.path.is_none());
        assert!(app.controller.is_none());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an isolated X11 display; replaces its clipboard"]
fn copied_text_survives_the_copy_command_and_remains_available_to_other_clients() {
    let mut app = App::new(None, RunHostOptions::default());
    app.handle_commands(vec![
        json!({"type":"copyText","text":"owned clipboard text"}),
    ]);
    let mut reader = arboard::Clipboard::new().unwrap();
    assert_eq!(reader.get_text().unwrap(), "owned clipboard text");
    app.handle_commands(vec![json!({"type":"copyText","text":"updated text"})]);
    assert_eq!(reader.get_text().unwrap(), "updated text");
}

#[test]
fn an_idle_loaded_host_waits_for_controller_work_instead_of_polling_at_frame_rate() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(None, RunHostOptions::default());
    let mut controller =
        source_controller(&directory.path().canonicalize().unwrap().join("source.onda"));
    controller.stop();
    controller.poll();
    app.controller = Some(controller);
    assert!(app.next_wake().unwrap() > Instant::now() + Duration::from_millis(800));
}
