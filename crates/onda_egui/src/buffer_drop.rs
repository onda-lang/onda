use std::path::{Path, PathBuf};

use eframe::egui;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

#[derive(Default)]
pub(super) struct FileDrag(onda_run::file_drag::FileDragPosition);

impl FileDrag {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let tracker = cc
            .window_handle()
            .ok()
            .zip(cc.display_handle().ok())
            .map(|(window, display)| {
                // SAFETY: The app is destroyed before eframe closes its window and display.
                unsafe {
                    onda_run::file_drag::FileDragPosition::new(window.as_raw(), display.as_raw())
                }
            })
            .unwrap_or_default();
        Self(tracker)
    }

    pub(super) fn pointer_position(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        let (hovered, dropped) = ctx.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                !input.raw.dropped_files.is_empty(),
            )
        });
        if !hovered && !dropped {
            return None;
        }
        if hovered {
            ctx.request_repaint_after(super::CONTROLLER_POLL_INTERVAL);
        }
        let [x, y] = self.0.position()?;
        let scale = if cfg!(target_os = "macos") {
            ctx.zoom_factor()
        } else {
            ctx.pixels_per_point()
        };
        Some(egui::pos2(x / scale, y / scale))
    }
}

fn is_buffer_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("wav") || extension.eq_ignore_ascii_case("ondabuffer")
        })
}

// winit can report the same path more than once while native selection replies
// arrive. Accept one distinct file, without mistaking those reports for a batch.
fn single_buffer_path<'a>(paths: impl IntoIterator<Item = Option<&'a Path>>) -> Option<&'a Path> {
    let mut paths = paths.into_iter();
    let path = paths.next()??;
    (is_buffer_file(path) && paths.all(|candidate| candidate == Some(path))).then_some(path)
}

pub(super) fn file_drop(
    ui: &egui::Ui,
    rect: egui::Rect,
    position: Option<egui::Pos2>,
    frame: &mut egui::Frame,
) -> Option<PathBuf> {
    let position = position?;

    if !rect.intersect(ui.clip_rect()).contains(position)
        || ui
            .ctx()
            .layer_id_at(position)
            .is_some_and(|layer| layer != ui.layer_id())
    {
        return None;
    }
    let (hovered, dropped) = ui.input(|input| {
        let hovered = single_buffer_path(
            input
                .raw
                .hovered_files
                .iter()
                .map(|file| file.path.as_deref()),
        )
        .is_some();
        let dropped = single_buffer_path(
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path.as_deref()),
        )
        .map(Path::to_path_buf);
        (hovered, dropped)
    });
    if hovered {
        frame.fill = ui.visuals().selection.bg_fill;
        frame.stroke.color = ui.visuals().selection.stroke.color;
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop_on_cards(
        files: &[&str],
        position: egui::Pos2,
        clip: egui::Rect,
    ) -> Vec<Option<PathBuf>> {
        let ctx = egui::Context::default();
        let rects = [
            egui::Rect::from_min_max(egui::pos2(10.0, 10.0), egui::pos2(200.0, 70.0)),
            egui::Rect::from_min_max(egui::pos2(10.0, 80.0), egui::pos2(200.0, 140.0)),
        ];
        let mut results = Vec::new();
        // Establish the layer before hit testing the drop.
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 200.0),
                    )),
                    // The OS drop position must win over stale normal pointer input.
                    events: vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
                    dropped_files: files
                        .iter()
                        .map(|path| egui::DroppedFile {
                            path: Some(PathBuf::from(path)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_clip_rect(clip);
                        results = rects
                            .iter()
                            .map(|rect| {
                                let mut frame = egui::Frame::NONE;
                                file_drop(ui, *rect, Some(position), &mut frame)
                            })
                            .collect();
                    });
                },
            );
        }
        results
    }

    #[test]
    fn hovering_tints_one_distinct_file_and_clears_after_leaving() {
        let ctx = egui::Context::default();
        let mut app = crate::RunApp::new(
            None,
            onda_run::RunHostOptions::default(),
            None,
            crate::ParamLayout::Sliders,
        );
        let buffers = [
            serde_json::json!({ "name": "left", "type": "f32[]" }),
            serde_json::json!({ "name": "right", "type": "f32[]" }),
        ];
        let mut render = |position, file: Option<&str>, reports| {
            app.buffer_drop_position = position;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(720.0, 400.0),
                    )),
                    // A file drag must use the OS position despite stale normal pointer input.
                    events: vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
                    hovered_files: file
                        .into_iter()
                        .cycle()
                        .take(reports)
                        .map(|path| egui::HoveredFile {
                            path: Some(PathBuf::from(path)),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.render_buffers(ui, &buffers);
                    });
                },
            );
            let cards: Vec<_> = output
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::Shape::Rect(rect)
                        if rect.rect.width() > 600.0
                            && rect.corner_radius
                                == egui::CornerRadius::from(crate::CARD_CORNER_RADIUS) =>
                    {
                        Some(rect)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(cards.len(), 2);
            cards
        };
        let normal = render(None, None, 0);
        // Native file hover events may repeat before selection requests settle.
        for (target, reports) in [(1, 1), (0, 2), (1, 3)] {
            let cards = render(
                Some(normal[target].rect.center()),
                Some("clip.wav"),
                reports,
            );
            assert_eq!(cards[1 - target].fill, normal[1 - target].fill);
            assert_eq!(cards[1 - target].stroke, normal[1 - target].stroke);
            assert_ne!(
                cards[target].fill, normal[target].fill,
                "the drop target needs a visible background"
            );
            assert_eq!(cards[target].fill, ctx.style().visuals.selection.bg_fill);
            assert_eq!(
                cards[target].stroke.color,
                ctx.style().visuals.selection.stroke.color
            );
        }
        for (position, file) in [
            (Some(egui::pos2(710.0, 390.0)), Some("clip.wav")),
            (None, None),
            (Some(normal[0].rect.center()), Some("main.onda")),
        ] {
            let cards = render(position, file, 1);
            for (card, normal) in cards.iter().zip(&normal) {
                assert_eq!(card.fill, normal.fill);
                assert_eq!(card.stroke, normal.stroke);
            }
        }
    }

    #[test]
    fn drops_bind_only_the_card_under_the_pointer() {
        for path in ["clip.wav", "typed.ONDaBUFFER"] {
            assert_eq!(
                drop_on_cards(&[path], egui::pos2(50.0, 100.0), egui::Rect::EVERYTHING),
                vec![None, Some(PathBuf::from(path))],
            );
        }
        assert_eq!(
            drop_on_cards(
                &["clip.wav"],
                egui::pos2(50.0, 75.0),
                egui::Rect::EVERYTHING
            ),
            vec![None, None],
        );
    }

    #[test]
    fn repeated_reports_of_one_dropped_file_bind_only_the_target_card() {
        assert_eq!(
            drop_on_cards(
                &["clip.wav", "clip.wav"],
                egui::pos2(50.0, 100.0),
                egui::Rect::EVERYTHING
            ),
            vec![None, Some(PathBuf::from("clip.wav"))],
        );
    }

    #[test]
    fn drops_reject_unsupported_files_multiple_files_and_clipped_cards() {
        for files in [&["main.onda"][..], &["a.wav", "b.wav"], &[]] {
            assert_eq!(
                drop_on_cards(files, egui::pos2(50.0, 100.0), egui::Rect::EVERYTHING),
                vec![None, None],
            );
        }
        assert_eq!(
            drop_on_cards(
                &["clip.wav"],
                egui::pos2(50.0, 100.0),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(200.0, 70.0)),
            ),
            vec![None, None],
        );
    }
}
