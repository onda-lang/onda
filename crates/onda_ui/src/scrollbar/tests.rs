use crate::{
    scene::{Rect, Target},
    Input, RunUi,
};
use serde_json::json;

fn fixture() -> RunUi {
    let mut ui = RunUi::default();
    ui.resize(720.0, 840.0, 1.0);
    ui.message(json!({"type":"state","state":{
        "path":"scroll.onda","supportsScope":true,"outputChannels":64,
        "logRevealed":true,"logText":"line\n".repeat(100)
    }}));
    ui
}

#[test]
fn internal_scrollbars_are_interactive_in_the_log_and_both_scope_views() {
    for pinned in [false, true] {
        let mut ui = fixture();
        ui.presentation.scope_pinned = pinned;
        if !pinned {
            ui.interaction.scope_open = true;
            ui.invalidate();
        }
        ui.prepare_scene();
        for area in [ui.scene.log_rect, ui.scene.scope_clip] {
            assert!(area[2] > 0.0 && area[3] > 0.0);
            assert!(
                ui.scene
                    .hits
                    .iter()
                    .any(|hit| matches!(hit.target, Target::Scrollbar(_))
                        && Rect(area).contains(
                            hit.rect.0[0] + hit.rect.0[2] * 0.5,
                            hit.rect.0[1] + hit.rect.0[3] * 0.5
                        )),
                "overflowing internal viewport must expose a draggable scrollbar"
            );
        }
    }
}

#[test]
fn loaded_and_empty_views_have_equal_left_and_right_gutters() {
    for width in [195.0, 720.0, 1440.0] {
        for loaded in [false, true] {
            let mut ui = RunUi::default();
            ui.resize(width, 840.0, 1.0);
            ui.message(json!({"type":"state","state":{
                "path":if loaded { "view.onda" } else { "" },
                "supportsSourceSelection":true
            }}));
            ui.prepare_scene();
            let rect = if loaded {
                ui.scene.header_rect
            } else {
                ui.scene.source_picker.unwrap().0
            };
            assert_eq!(rect[0], width - rect[0] - rect[2]);
        }
    }
}

#[test]
fn internal_thumb_drag_keeps_its_grab_and_log_follow_tracks_the_bottom() {
    use super::ScrollRegion;
    for region in [
        ScrollRegion::Log,
        ScrollRegion::Scope,
        ScrollRegion::LogHorizontal,
    ] {
        let mut ui = fixture();
        ui.presentation.scope_pinned = true;
        if region == ScrollRegion::LogHorizontal {
            ui.log_max_scroll_x = 1000.0;
        }
        ui.prepare_scene();
        let bar = ui.scene.scrollbar_for(region).unwrap();
        let (x, y) = (bar.thumb.0[0] + 3.0, bar.thumb.0[1] + 3.0);
        ui.input(Input::PointerDown { x, y, clicks: 1 });
        ui.prepare_scene();
        assert_eq!(ui.scene.scrollbar_for(region).unwrap().scroll, bar.scroll);
        for end in [false, true] {
            ui.input(Input::PointerMove {
                x: if region == ScrollRegion::LogHorizontal {
                    if end {
                        2000.0
                    } else {
                        0.0
                    }
                } else {
                    x
                },
                y: if region == ScrollRegion::LogHorizontal {
                    y
                } else if end {
                    2000.0
                } else {
                    0.0
                },
                fine: false,
            });
            ui.prepare_scene();
            let current = ui.scene.scrollbar_for(region).unwrap();
            assert_eq!(current.scroll, if end { current.max_scroll } else { 0.0 });
            assert_eq!(
                ui.scroll, 0.0,
                "dragging an internal bar must not move the document"
            );
            if region == ScrollRegion::Log {
                assert_eq!(ui.presentation.log_follow, end);
                let before = current.scroll;
                ui.message(json!({"type":"state","state":{"logText":"line\n".repeat(if end { 120 } else { 110 })}}));
                ui.prepare_scene();
                let next = ui.scene.scrollbar_for(region).unwrap();
                assert_eq!(next.scroll, if end { next.max_scroll } else { before });
            }
        }
        ui.input(Input::PointerUp);
    }
}
