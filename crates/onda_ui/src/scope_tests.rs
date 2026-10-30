use crate::{
    presentation::Section,
    scene::{Rect, Target},
    Input, Key, RunUi,
};
use serde_json::json;

fn fixture(channels: usize) -> RunUi {
    let mut ui = RunUi::default();
    ui.message(json!({"type":"state","state":{
        "path":"meter.onda","running":true,"outputChannels":channels,"supportsViewState":true,
        "supportsDeviceSelection":true,"supportsSourceSelection":true,
        "supportsProjectExport":true,"canExportProject":true,
        "params":[{"name":"gain","type":"f64","value":0.5}]
    }}));
    ui
}
fn click(ui: &mut RunUi, target: Target) {
    ui.prepare_scene();
    let r = ui
        .scene
        .hits
        .iter()
        .find(|h| h.target == target)
        .unwrap()
        .rect
        .0;
    ui.input(Input::PointerDown {
        x: r[0] + r[2] * 0.5,
        y: r[1] + r[3] * 0.5,
        clicks: 1,
    });
    ui.input(Input::PointerUp);
    ui.prepare_scene();
}

#[test]
fn headers_keep_audio_and_overflow_on_one_row_at_every_supported_zoom() {
    for physical_width in [360.0, 400.0, 720.0, 1024.0, 1600.0] {
        for zoom in [0.75, 1.0, 1.25, 1.5, 2.0, 2.5] {
            for channels in [1, 2, 3, 4, 8, 64] {
                let mut ui = fixture(channels);
                ui.resize(physical_width / zoom, 840.0 / zoom, zoom);
                ui.prepare_scene();
                let header = ui.scene.header_rect;
                let audio: Vec<_> = ui
                    .scene
                    .hits
                    .iter()
                    .filter(|h| {
                        matches!(
                            h.target,
                            Target::Scope
                                | Target::Transport
                                | Target::HeaderMenu
                                | Target::Devices
                        )
                    })
                    .collect();
                let y = audio[0].rect.0[1];
                for hit in audio {
                    let r = hit.rect.0;
                    assert_eq!(r[1], y);
                    assert_eq!(r[3], 28.0);
                    assert!(
                        r[0] >= header[0] && r[0] + r[2] <= header[0] + header[2],
                        "width={physical_width}, zoom={zoom}, channels={channels}, rect={r:?}"
                    );
                }
                let meters = ui.scene.meter_rects.len();
                assert!(meters == channels || meters == 1);
                let count = ui
                    .scene
                    .labels
                    .iter()
                    .any(|l| l.text == channels.to_string());
                assert_eq!(count, channels > 1 && meters == 1);
                assert_eq!(
                    ui.scene.hits.iter().any(|h| h.target == Target::HeaderMenu),
                    header[2] < 452.0
                );
                assert!(!ui.scope_enabled());
                assert!(ui.meter_enabled());
            }
        }
    }
}

#[test]
fn channel_meters_expand_when_space_returns_without_new_meter_data() {
    let mut ui = fixture(64);
    let mut peaks = vec![0.0; 64];
    peaks[63] = 1.2;
    ui.message(json!({"type":"meterData","channels":64,"peaks":peaks}));
    for (width, meters) in [(1600.0, 64), (360.0, 1), (1600.0, 64)] {
        ui.resize(width, 840.0, 1.0);
        ui.prepare_scene();
        assert_eq!(ui.scene.meter_rects.len(), meters);
        assert_eq!(ui.meter_peaks[63], 1.2);
    }
    ui.message(json!({"type":"state","state":{"outputChannels":8}}));
    ui.resize(360.0, 840.0, 1.0);
    ui.prepare_scene();
    assert_eq!(ui.scene.meter_rects.len(), 8);
    ui.resize(160.0, 840.0, 1.0);
    ui.prepare_scene();
    assert_eq!(ui.scene.meter_rects.len(), 1);
    ui.set_meter(2, &[0.5, f32::NAN]);
    assert_eq!(ui.meter_peaks[0], 0.5);
    assert!(ui.meter_peaks[1..].iter().all(|&peak| peak == 0.0));
}

#[test]
fn overlay_pin_and_unpin_preserve_controls_and_unpin_closes_the_scope() {
    let mut ui = fixture(8);
    ui.resize(720.0, 840.0, 1.0);
    ui.prepare_scene();
    let height = ui.scene.content_height;
    click(&mut ui, Target::Scope);
    assert!(ui.scope_enabled() && ui.scene.scope_overlay);
    assert_eq!(ui.scene.content_height, height);
    assert!(!ui.take_commands().iter().any(|c| c["type"] == "viewState"));
    let r = ui.scene.scope_clip;
    ui.input(Input::PointerMove {
        x: r[0] + 5.0,
        y: r[1] + 5.0,
        fine: false,
    });
    ui.input(Input::Scroll { delta: 1000.0 });
    ui.prepare_scene();
    assert_eq!(ui.interaction.scope_scroll, ui.scene.scope_scroll_max);
    assert_eq!(ui.scroll, 0.0);
    click(&mut ui, Target::PinScope);
    assert!(ui.presentation.scope_pinned && !ui.scene.scope_overlay);
    assert!(ui.scene.content_height > height);
    let saved = ui
        .take_commands()
        .into_iter()
        .find(|c| c["type"] == "viewState")
        .unwrap()["state"]
        .clone();
    assert_eq!(saved["scopePinned"], true);
    let mut restored = fixture(8);
    restored.restore_view(&saved);
    assert!(restored.scope_enabled());
    click(&mut ui, Target::Section(Section::Scope));
    assert!(!ui.scope_enabled() && ui.meter_enabled());
    click(&mut ui, Target::PinScope);
    assert!(!ui.presentation.scope_pinned && !ui.scope_enabled());
    assert!(!ui.scene.scope_overlay && ui.meter_enabled());
    assert_eq!(ui.scene.content_height, height);
    assert_eq!(
        ui.take_commands().last().unwrap()["state"]["scopePinned"],
        false
    );
    click(&mut ui, Target::Scope);
    assert!(ui.scene.scope_overlay && ui.scope_enabled());
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    assert!(!ui.scope_enabled());
}

#[test]
fn meter_updates_do_not_rebuild_layout_and_short_overflow_menus_scroll() {
    let mut ui = fixture(4);
    ui.resize(160.0, 150.0, 2.0);
    ui.set_meter(4, &[0.0; 4]);
    ui.prepare_scene();
    let revision = ui.revisions();
    ui.set_meter(4, &[0.1, 0.3, 0.7, 1.2]);
    assert_eq!(ui.revisions().0, revision.0);
    assert_ne!(ui.revisions().1, revision.1);
    assert!(!ui.scene_dirty);
    click(&mut ui, Target::HeaderMenu);
    let r = ui.scene.toolbar_menu_rect;
    assert!(ui.scene.toolbar_scroll_max > 0.0);
    ui.input(Input::PointerMove {
        x: r[0] + 4.0,
        y: r[1] + 4.0,
        fine: false,
    });
    ui.input(Input::Scroll { delta: 1000.0 });
    ui.prepare_scene();
    let unload = ui
        .scene
        .hits
        .iter()
        .find(|h| h.target == Target::Command(json!({"type":"unload"})))
        .unwrap();
    assert!(Rect(r).contains(unload.rect.0[0] + 1.0, unload.rect.0[1] + 1.0));
    assert_eq!(unload.rect.0[3], 28.0);
}

#[test]
fn escape_cancels_edits_and_settings_before_the_scope_overlay() {
    let mut ui = fixture(2);
    ui.resize(720.0, 840.0, 1.0);
    click(&mut ui, Target::Scope);
    click(&mut ui, Target::Field(crate::editing::Field::Control(0)));
    assert!(ui.editing());
    ui.input(Input::Text { text: "0.8".into() });
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    assert!(!ui.editing() && ui.scope_enabled());
    assert_eq!(ui.controls[0].value, json!(0.5));
    click(&mut ui, Target::Devices);
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    assert!(!ui.interaction.settings_open() && ui.scope_enabled());
    ui.input(Input::Key {
        key: Key::Escape,
        shift: false,
    });
    assert!(!ui.scope_enabled());
}
