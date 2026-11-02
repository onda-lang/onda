use crate::{
    editing::{Field, Setting},
    interaction::ToolbarMenu,
    model::RunUi,
    presentation::Section,
    scene::{Rect, Target},
    Input, Key,
};
use serde_json::json;

const WIDTHS: &[f32] = &[
    128.0, 144.0, 195.0, 240.0, 280.0, 300.0, 312.0, 320.0, 360.0, 390.0, 419.0, 420.0, 439.0,
    440.0, 480.0, 720.0, 1000.0, 1440.0,
];

#[test]
fn resize_resolves_edits_that_move_into_overflow_and_releases_their_focus() {
    for setting in [
        Setting::ParamSmoothing,
        Setting::InputSmoothing,
        Setting::Octave,
        Setting::Velocity,
    ] {
        for valid in [false, true] {
            let mut ui = RunUi::default();
            ui.message(json!({"type":"state","state":{
                "path":"resize.onda","supportsScope":false,"running":true,
                "supportsParamSmoothing":true,"supportsInputControlSmoothing":true,
                "paramSmoothingMs":30,"inputControlSmoothingMs":30,
                "currentMidiInputDevice":"Computer Keyboard","midiKeyboardInteractive":true,
                "midi":{"noteOn":true},
                "params":[{"name":"gain","type":"f64","value":0.5}],
                "inputControls":[{"name":"cutoff","type":"f64","value":0.5}]
            }}));
            let field = Field::Setting(setting);
            let original = ui.field_value(&field);
            let value = match setting {
                Setting::Octave => "5",
                Setting::Velocity => "0.75",
                _ => "75",
            };
            let text = if valid { value } else { "invalid" };
            ui.begin_field(field.clone());
            ui.input(Input::Text { text: text.into() });
            ui.take_commands();

            // Resizing while the field remains inline must retain the draft.
            ui.resize(640.0, 840.0, 1.0);
            assert_eq!(ui.text_edit().unwrap()["text"], text);
            ui.resize(195.0, 840.0, 1.0);
            assert!(!ui.editing(), "hidden {setting:?} draft, valid={valid}");
            if valid {
                assert_eq!(ui.field_value(&field).as_f64(), value.parse::<f64>().ok());
            } else {
                assert_eq!(ui.field_value(&field), original);
            }
            assert!(ui.text_edit().is_none());
            ui.input(Input::Key {
                key: Key::Enter,
                shift: false,
            });
            assert!(!ui.editing(), "Enter must not reopen the hidden field");
            ui.take_commands();
            ui.input(Input::ComputerKey {
                key: "a".into(),
                pressed: true,
            });
            assert!(ui
                .take_commands()
                .iter()
                .any(|command| command["type"] == "midiNote" && command["pressed"] == true));
        }
    }
}

fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "path":"resize.onda","supportsScope":true,"supportsViewState":true,
        "supportsParamSmoothing":true,"supportsInputControlSmoothing":true,
        "paramLayout":"sliders","inputControlLayout":"sliders",
        "midiKeyboardInteractive":true,"midi":{"noteOn":true},
        "params":[{"name":"gain","type":"f64","value":0.5,"rangeMin":0,"rangeMax":1}],
        "inputControls":[{"name":"cutoff","type":"f64","value":0.5,"rangeMin":0,"rangeMax":1}],
        "events":[{"name":"send","args":[{"name":"value","type":"f64","value":0.5}]}],
        "buffers":[{"name":"clip","type":"f32","loadedPath":"piano.wav","loadedFrames":96000,
            "loadedChannels":2,"loadedSampleRate":48000,"waveform":{"minimums":[-0.5],"maximums":[0.5]}}],
        "logRevealed":true,"logText":"hello\nworld\n"
    }}));
    ui.presentation.scope_pinned = true;
    ui
}

fn rect(ui: &RunUi, target: &Target) -> [f32; 4] {
    ui.scene
        .hits
        .iter()
        .find(|h| &h.target == target)
        .unwrap()
        .rect
        .0
}

fn click(ui: &mut RunUi, target: Target) {
    ui.prepare_scene();
    let [x, y, w, h] = rect(ui, &target);
    ui.input(Input::PointerDown {
        x: x + w * 0.5,
        y: y + h * 0.5,
        clicks: 1,
    });
    ui.input(Input::PointerUp);
    ui.prepare_scene();
}

#[test]
fn section_headers_keep_one_row_at_every_width_and_scale() {
    let mut ui = fixture();
    ui.presentation.sections.fill(false);
    for &width in WIDTHS {
        for scale in [1.0, 1.25, 2.0, 2.5] {
            ui.resize(width, 840.0, scale);
            ui.prepare_scene();
            let headers: Vec<_> = Section::ALL
                .into_iter()
                .filter(|s| *s != Section::Midi)
                .map(|s| rect(&ui, &Target::Section(s)))
                .collect();
            for pair in headers.windows(2) {
                assert_eq!(
                    pair[1][1] - pair[0][1],
                    52.0,
                    "extra header row at {width}/{scale}"
                );
            }
            for hit in &ui.scene.hits {
                if matches!(hit.target, Target::ToolbarMenu(_)) {
                    let [x, _, w, _] = hit.rect.0;
                    assert!(
                        x >= 0.0 && x + w <= width,
                        "menu outside viewport at {width}"
                    );
                    for heading in &headers {
                        let overlap = Rect(*heading).intersect(hit.rect).0;
                        assert!(
                            overlap[2] == 0.0 || overlap[3] == 0.0,
                            "menu overlaps section at {width}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn midi_settings_stay_beside_the_heading_and_the_piano_keeps_its_top_inset() {
    let mut ui = fixture();
    for &width in WIDTHS {
        ui.resize(width, 840.0, 1.0);
        ui.prepare_scene();
        let heading = rect(&ui, &Target::Section(Section::Midi));
        for setting in [Setting::Octave, Setting::Velocity] {
            if let Some(hit) = ui
                .scene
                .hits
                .iter()
                .find(|h| h.target == Target::Field(Field::Setting(setting)))
            {
                let r = hit.rect.0;
                assert!((r[1] - heading[1]).abs() <= 4.0, "MIDI wrapped at {width}");
                assert!(r[0] >= heading[0] + heading[2] && r[0] + r[2] <= width - 12.0);
            } else {
                assert!(ui
                    .scene
                    .hits
                    .iter()
                    .any(|h| h.target == Target::ToolbarMenu(ToolbarMenu::Midi)));
            }
        }
        let note = rect(&ui, &Target::Note(60));
        assert_eq!(
            note[1] - ui.scene.viewport_height,
            53.0,
            "piano moved down at {width}"
        );
    }
}

#[test]
fn slider_values_and_buffer_details_keep_their_original_row_when_narrowed() {
    let mut ui = fixture();
    ui.presentation.scope_pinned = false;
    ui.presentation.sections.fill(false);
    for section in [Section::Params, Section::Buffers] {
        ui.presentation.sections[section.index()] = true;
        let mut body_height = None;
        for &width in WIDTHS.iter().rev() {
            ui.resize(width, 840.0, 1.0);
            ui.invalidate();
            ui.prepare_scene();
            let bounds = rect(&ui, &Target::Section(section));
            let panel = ui
                .scene
                .shapes
                .iter()
                .find(|s| s.rect[0] == 12.0 && s.rect[1] == bounds[1] - 4.0)
                .unwrap()
                .rect;
            assert_eq!(
                *body_height.get_or_insert(panel[3]),
                panel[3],
                "body gained a row at {width}: {section:?}"
            );
            if section == Section::Params {
                let name = ui.scene.labels.iter().find(|l| l.text == "gain").unwrap();
                let index = ui.controls.iter().position(|c| c.name() == "gain").unwrap();
                let value = rect(&ui, &Target::Field(Field::Control(index)));
                assert!((name.y + name.size * 0.7 - value[1] - value[3] * 0.5).abs() < 0.1);
                assert!(name.x + name.width <= value[0]);
            } else {
                let file = ui
                    .scene
                    .labels
                    .iter()
                    .find(|l| l.text == "piano.wav")
                    .unwrap();
                let summary = ui
                    .scene
                    .labels
                    .iter()
                    .find(|l| l.text.contains("96,000 frames"))
                    .unwrap();
                assert!((file.y + file.size * 0.7 - summary.y - summary.size * 0.7).abs() < 0.1);
                assert!(file.x + file.width <= summary.x);
            }
        }
        ui.presentation.sections[section.index()] = false;
    }
}

#[test]
fn compact_menus_keep_actions_and_midi_edits_reachable_and_close_when_their_anchor_disappears() {
    let mut ui = fixture();
    ui.resize(195.0, 840.0, 1.0);
    ui.presentation.sections.fill(false);
    ui.presentation.sections[Section::Buffers.index()] = true;
    ui.presentation.sections[Section::Midi.index()] = true;
    ui.prepare_scene();
    for menu in [
        ToolbarMenu::Log,
        ToolbarMenu::Buffer("clip".into()),
        ToolbarMenu::Midi,
    ] {
        click(&mut ui, Target::ToolbarMenu(menu.clone()));
        let bounds = ui.scene.toolbar_menu_rect;
        assert!(bounds[1] >= 0.0 && bounds[1] + bounds[3] <= ui.size[1]);
        match menu {
            ToolbarMenu::Log => {
                click(&mut ui, Target::CopyLog);
                assert!(ui.take_commands().iter().any(|c| c["type"] == "copyText"));
                click(&mut ui, Target::Command(json!({"type":"clearLog"})));
                assert_eq!(ui.state["logText"], "");
            }
            ToolbarMenu::Buffer(_) => {
                click(
                    &mut ui,
                    Target::Command(json!({"type":"chooseBufferFile","name":"clip"})),
                );
                assert!(ui
                    .take_commands()
                    .iter()
                    .any(|c| c["type"] == "chooseBufferFile" && c["name"] == "clip"));
            }
            ToolbarMenu::Midi => {
                ui.input(Input::Key {
                    key: Key::Tab,
                    shift: false,
                });
                assert!(ui.focused(&Target::Field(Field::Setting(Setting::Octave))));
                ui.input(Input::EditText {
                    text: "5".into(),
                    anchor: 1,
                    cursor: 1,
                });
                ui.input(Input::Key {
                    key: Key::Enter,
                    shift: false,
                });
                assert_eq!(ui.field_value(&Field::Setting(Setting::Octave)), json!(5));
            }
            _ => unreachable!(),
        }
        ui.resize(720.0, 840.0, 1.0);
        ui.prepare_scene();
        assert!(ui.interaction.toolbar_menu.is_none());
        ui.resize(195.0, 840.0, 1.0);
        ui.prepare_scene();
    }
}

#[test]
fn event_actions_and_fields_stay_inside_their_cards_without_overlapping() {
    let mut ui = fixture();
    ui.presentation.sections.fill(false);
    ui.presentation.sections[Section::Events.index()] = true;
    ui.message(json!({"type":"state","state":{"events":[{
        "name":"dispatch","args":[{"name":"values","type":"f64[]","value":[0.5,1.0]}]
    }]}}));
    for &width in WIDTHS {
        ui.resize(width, 840.0, 1.0);
        ui.prepare_scene();
        let hits: Vec<_> = ui
            .scene
            .hits
            .iter()
            .filter(|h| {
                matches!(
                    h.target,
                    Target::Trigger(_)
                        | Target::Slice { .. }
                        | Target::Fold(_)
                        | Target::Field(Field::Event(_))
                )
            })
            .collect();
        for (i, hit) in hits.iter().enumerate() {
            assert!(
                hit.rect.0[0] >= 21.0 && hit.rect.0[0] + hit.rect.0[2] <= width - 25.0,
                "event action outside its card at {width}: {:?}",
                hit.target
            );
            for other in &hits[i + 1..] {
                let overlap = hit.rect.intersect(other.rect).0;
                assert!(
                    overlap[2] == 0.0 || overlap[3] == 0.0,
                    "event actions overlap at {width}: {:?} / {:?}",
                    hit.target,
                    other.target
                );
            }
        }
        let title = ui
            .scene
            .labels
            .iter()
            .find(|l| l.text == "dispatch")
            .unwrap();
        let trigger = rect(&ui, &Target::Trigger(0));
        assert!(title.x + title.width <= trigger[0]);
    }
}

#[test]
fn source_picker_run_settings_stay_on_one_row() {
    use crate::interaction::SelectId;
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "supportsSourceSelection":true,"supportsRunSettings":true,"supportsFastMath":true,
        "sampleRateHz":48000,"blockSamples":512
    }}));
    for &width in WIDTHS {
        ui.resize(width, 840.0, 1.0);
        ui.prepare_scene();
        let rate = rect(&ui, &Target::Choose(SelectId::SampleRate));
        let block = rect(&ui, &Target::Choose(SelectId::BlockSize));
        let fast = rect(&ui, &Target::FastMath);
        assert_eq!(rate[1], block[1]);
        assert_eq!(rate[1] + rate[3] * 0.5, fast[1] + fast[3] * 0.5);
        assert!(rate[0] + rate[2] <= block[0] && block[0] + block[2] <= fast[0]);
        assert!(fast[0] + fast[2] <= width - 12.0);
    }
}

#[test]
fn plugin_picker_centers_editable_settings_and_tracks_host_block_size() {
    use crate::interaction::SelectId;
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "supportsSourceSelection":true,"supportsRunSettings":true,"supportsFastMath":true,
        "hostBlockSamples":2048,"preferredBlockSamples":0,"canSetBlockSize":true,
        "sampleRateHz":48000,"blockSamples":2048
    }}));
    for &width in WIDTHS {
        ui.resize(width, 840.0, 1.0);
        ui.prepare_scene();
        assert!(!ui.scene.labels.iter().any(|l| l.text == "SAMPLE RATE"));
        let block = rect(&ui, &Target::Choose(SelectId::BlockSize));
        let fast_label = ui
            .scene
            .labels
            .iter()
            .find(|l| l.text == "FAST MATH")
            .unwrap();
        let right = fast_label.x + fast_label.width;
        let [x, _, w, _] = ui.scene.source_picker.unwrap().0;
        assert!((block[0] + right - (2.0 * x + w)).abs() < 0.01);
        let fast = rect(&ui, &Target::FastMath);
        assert_eq!(block[1] + block[3] * 0.5, fast[1] + fast[3] * 0.5);
    }
    assert_eq!(SelectId::BlockSize.spec(&ui).label, "2048 (Host)");
    ui.message(json!({"type":"state","state":{"hostBlockSamples":512,"blockSamples":512}}));
    assert_eq!(SelectId::BlockSize.spec(&ui).label, "512 (Host)");
    ui.message(json!({"type":"state","state":{
        "path":"plugin.onda","supportsRunSettings":false,"supportsTransport":false
    }}));
    ui.prepare_scene();
    assert!(ui.scene.hits.iter().any(|h| h.target == Target::Devices));
}
