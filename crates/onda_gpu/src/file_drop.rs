//! Native file offers, including asynchronous Wayland URI delivery.
use super::App;
use onda_ui::RunUi;
use serde_json::json;
use std::path::{Path, PathBuf};
use winit::{
    data_transfer::{DataTransferId, TypeHint},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, AsyncRequestSerial, DndAction},
};

#[derive(Default)]
pub(super) struct FileDrop {
    drag: Option<Offer>,
    // A drop and DragLeft can precede the asynchronous URI response.
    pending: Vec<Offer>,
}
struct Offer {
    id: DataTransferId,
    request: Option<AsyncRequestSerial>,
    position: Option<[f32; 2]>,
    paths: Option<Vec<PathBuf>>,
    buffer: Option<String>,
    accepted: bool,
}
pub(super) enum Response {
    Ignored,
    Handled,
    Dropped {
        paths: Vec<PathBuf>,
        buffer: Option<String>,
    },
}
impl Offer {
    fn matches(&self, id: DataTransferId, request: AsyncRequestSerial) -> bool {
        self.id == id && self.request == Some(request)
    }
    fn complete(self, paths: Vec<PathBuf>) -> Response {
        Response::Dropped {
            paths,
            buffer: self.buffer,
        }
    }
}

impl FileDrop {
    pub(super) fn cancel(&mut self, ui: &mut RunUi) {
        self.leave(ui);
        self.pending.clear();
    }
    fn leave(&mut self, ui: &mut RunUi) {
        self.drag = None;
        ui.message(json!({"type":"bufferFileDrag","phase":"leave"}));
    }

    pub(super) fn event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        event: &WindowEvent,
        scale: f32,
        ui: &mut RunUi,
    ) -> Response {
        let logical =
            |p: winit::dpi::PhysicalPosition<f64>| [p.x as f32 / scale, p.y as f32 / scale];
        match event {
            WindowEvent::DragEntered { id, position } => {
                self.leave(ui);
                let supported = event_loop
                    .data_transfer(*id)
                    .is_ok_and(|d| d.has_type(&TypeHint::UriList));
                if !supported {
                    return Response::Handled;
                }
                let Ok(request) = event_loop.fetch_data_transfer(*id, &TypeHint::UriList) else {
                    return Response::Handled;
                };
                self.drag = Some(Offer {
                    id: *id,
                    request: Some(request),
                    position: position.map(logical),
                    paths: None,
                    buffer: None,
                    accepted: false,
                });
            }
            WindowEvent::DragPosition { id, position, .. } => {
                let Some(offer) = self.drag.as_mut().filter(|o| o.id == *id) else {
                    return Response::Handled;
                };
                offer.position = Some(logical(*position));
            }
            WindowEvent::DataTransferReceived { id, serial, value } => {
                let paths = file_paths(value.try_as_uris().unwrap_or_default());
                let response = self.receive(*id, *serial, paths);
                if matches!(response, Response::Dropped { .. }) {
                    return response;
                }
            }
            WindowEvent::DragDropped { id, .. } => return self.drop(*id, ui),
            WindowEvent::DragLeft { id } => {
                if self.drag.as_ref().is_some_and(|o| o.id == *id) {
                    self.leave(ui);
                }
                return Response::Handled;
            }
            _ => return Response::Ignored,
        }
        self.update(event_loop, ui);
        Response::Handled
    }

    fn receive(
        &mut self,
        id: DataTransferId,
        request: AsyncRequestSerial,
        paths: Vec<PathBuf>,
    ) -> Response {
        if let Some(offer) = self.drag.as_mut().filter(|o| o.matches(id, request)) {
            offer.request = None;
            offer.paths = Some(paths);
        } else if let Some(index) = self.pending.iter().position(|o| o.matches(id, request)) {
            return self.pending.remove(index).complete(paths);
        }
        Response::Handled
    }

    fn drop(&mut self, id: DataTransferId, ui: &mut RunUi) -> Response {
        if !self.drag.as_ref().is_some_and(|o| o.id == id) {
            return Response::Handled;
        }
        let mut offer = self.drag.take().unwrap();
        offer.buffer = offer.position.and_then(|[x, y]| ui.buffer_target(x, y));
        self.leave(ui);
        if let Some(paths) = offer.paths.take() {
            offer.complete(paths)
        } else {
            self.pending.push(offer);
            Response::Handled
        }
    }

    fn update(&mut self, event_loop: &dyn ActiveEventLoop, ui: &mut RunUi) {
        let Some(offer) = &mut self.drag else {
            return;
        };
        let target = offer.position.and_then(|[x, y]| ui.buffer_target(x, y));
        let path = offer.paths.as_deref().and_then(single_file);
        let buffer = path.is_some_and(is_buffer);
        let source = path.is_some_and(is_source);
        let accepted = offer.paths.is_none() || source || (buffer && target.is_some());
        if offer.accepted != accepted {
            let _ = event_loop
                .set_valid_dnd_actions(offer.id, if accepted { &[DndAction::Copy] } else { &[] });
            offer.accepted = accepted;
        }
        ui.message(
            json!({"type":if source {"sourceFileDrag"} else {"bufferFileDrag"},
            "phase":if source || buffer {"over"} else {"leave"},
            "position":offer.position.map(|[x,y]|json!({"x":x,"y":y})),"paths":offer.paths}),
        );
    }
}

impl App {
    pub(super) fn file_drop_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        event: &WindowEvent,
        scale: f32,
    ) -> bool {
        match self.file_drop.event(event_loop, event, scale, &mut self.ui) {
            Response::Ignored => false,
            Response::Handled => true,
            Response::Dropped { paths, buffer } => {
                self.drop_files(&paths, buffer);
                true
            }
        }
    }

    pub(super) fn drop_files(&mut self, paths: &[PathBuf], buffer: Option<String>) {
        self.ui
            .message(json!({"type":"bufferFileDrag","phase":"leave"}));
        let Some(path) = single_file(paths) else {
            return;
        };
        if is_source(path) {
            self.commands();
            self.open(path.to_path_buf());
        } else if let Some(name) = buffer.filter(|_| is_buffer(path)) {
            self.handle_commands(vec![
                json!({"type":"bindBufferFile","name":name,"filePath":path}),
            ]);
        }
    }
}

fn file_paths(uris: Vec<String>) -> Vec<PathBuf> {
    uris.iter()
        .map(|uri| url::Url::parse(uri).ok()?.to_file_path().ok())
        .collect::<Option<_>>()
        .unwrap_or_default()
}
fn single_file(paths: &[PathBuf]) -> Option<&Path> {
    let first = paths.first()?;
    paths
        .iter()
        .all(|path| path == first)
        .then_some(first.as_path())
}
fn is_buffer(path: &Path) -> bool {
    has_extension(path, &["wav", "ondabuffer"])
}
fn is_source(path: &Path) -> bool {
    has_extension(path, &["onda", "on", "ondaproject"])
}
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| extensions.iter().any(|ext| s.eq_ignore_ascii_case(ext)))
}

#[cfg(test)]
mod tests;
