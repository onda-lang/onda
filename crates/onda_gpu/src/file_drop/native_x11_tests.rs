//! Real winit/XDND delivery without mapping a window or moving the pointer.
use super::super::{FileDrop, Response};
use onda_ui::RunUi;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode,
            SelectionNotifyEvent, WindowClass, SELECTION_NOTIFY_EVENT,
        },
        Event,
    },
    wrapper::ConnectionExt as _,
    CURRENT_TIME, NONE,
};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndEnter, XdndPosition, XdndStatus, XdndDrop, XdndFinished,
        XdndSelection, XdndActionCopy,
        UriList: b"text/uri-list",
    }
}

struct DropTarget {
    window: Option<Arc<dyn Window>>,
    ui: RunUi,
    drag: FileDrop,
    ready: std::sync::mpsc::Sender<u32>,
    deadline: Instant,
    hovered: Option<[f32; 2]>,
    buffer: Option<String>,
    paths: Vec<PathBuf>,
}
impl ApplicationHandler for DropTarget {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window: Arc<dyn Window> = Arc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_visible(false)
                        .with_surface_size(winit::dpi::LogicalSize::new(720.0, 840.0)),
                )
                .unwrap(),
        );
        let target = match window.window_handle().unwrap().as_raw() {
            RawWindowHandle::Xlib(handle) => handle.window as u32,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            _ => panic!("The regression requires an X11 window"),
        };
        self.ready.send(target).unwrap();
        self.window = Some(window);
    }
    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if let WindowEvent::DragPosition { position, .. } = &event {
            self.hovered = Some([position.x as f32, position.y as f32]);
        }
        if let Response::Dropped { paths, buffer } =
            self.drag.event(event_loop, &event, 1.0, &mut self.ui)
        {
            self.paths = paths;
            self.buffer = buffer;
            event_loop.exit();
        }
    }
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        if Instant::now() >= self.deadline {
            event_loop.exit();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
    }
}

#[test]
#[ignore = "requires an X11 display"]
fn xdnd_file_drop_retains_the_drag_coordinates_through_winit() {
    use winit::platform::x11::EventLoopBuilderExtX11;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("sample.wav");
    std::fs::write(&file, []).unwrap();
    let uri = format!("file://{}\r\n", file.display());
    let (ready, receive) = std::sync::mpsc::channel();
    let sender = std::thread::spawn(move || {
        let target = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let atoms = Atoms::new(&connection).unwrap().reply().unwrap();
        let source = connection.generate_id().unwrap();
        connection
            .create_window(
                0,
                source,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::default(),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .set_selection_owner(source, atoms.XdndSelection, CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        let send = |type_, data| {
            connection
                .send_event(
                    false,
                    target,
                    EventMask::NO_EVENT,
                    ClientMessageEvent::new(32, target, type_, data),
                )
                .unwrap()
                .check()
                .unwrap();
            connection.flush().unwrap();
        };
        let position = connection
            .translate_coordinates(target, root, 42, 150)
            .unwrap()
            .reply()
            .unwrap();
        let packed = (u32::from(position.dst_x as u16) << 16) | u32::from(position.dst_y as u16);
        send(atoms.XdndEnter, [source, 5 << 24, atoms.UriList, 0, 0]);
        send(
            atoms.XdndPosition,
            [source, 0, packed, CURRENT_TIME, atoms.XdndActionCopy],
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut dropped = false;
        let mut next_position = Instant::now();
        while Instant::now() < deadline {
            if !dropped && Instant::now() >= next_position {
                send(
                    atoms.XdndPosition,
                    [source, 0, packed, CURRENT_TIME, atoms.XdndActionCopy],
                );
                next_position = Instant::now() + Duration::from_millis(20);
            }
            match connection.poll_for_event().unwrap() {
                Some(Event::SelectionRequest(request)) => {
                    let property = if request.property == NONE {
                        request.target
                    } else {
                        request.property
                    };
                    connection
                        .change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            property,
                            atoms.UriList,
                            uri.as_bytes(),
                        )
                        .unwrap()
                        .check()
                        .unwrap();
                    connection
                        .send_event(
                            false,
                            request.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: request.time,
                                requestor: request.requestor,
                                selection: request.selection,
                                target: request.target,
                                property,
                            },
                        )
                        .unwrap()
                        .check()
                        .unwrap();
                    connection.flush().unwrap();
                }
                Some(Event::ClientMessage(event))
                    if event.type_ == atoms.XdndStatus && !dropped =>
                {
                    if event.data.as_data32()[1] & 1 != 0 {
                        send(atoms.XdndDrop, [source, 0, CURRENT_TIME, 0, 0]);
                        dropped = true;
                    }
                }
                Some(Event::ClientMessage(event)) if event.type_ == atoms.XdndFinished => return,
                _ => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        assert!(dropped, "XDND never reached the drop phase");
    });
    let mut builder = EventLoop::builder();
    builder.with_x11().with_any_thread(true);
    let mut event_loop = builder.build().unwrap();
    let mut ui = RunUi::default();
    ui.resize(720.0, 840.0, 1.0);
    ui.message(
        json!({"type":"state","state":{"path":"sample.onda","supportsScope":false,
        "buffers":[{"name":"audio","type":"f32"}]}}),
    );
    let mut target = DropTarget {
        window: None,
        ui,
        drag: Default::default(),
        ready,
        deadline: Instant::now() + Duration::from_secs(5),
        hovered: None,
        buffer: None,
        paths: Vec::new(),
    };
    use winit::event_loop::run_on_demand::EventLoopExtRunOnDemand;
    event_loop.run_app_on_demand(&mut target).unwrap();
    sender.join().unwrap();
    assert_eq!(target.paths, [file], "the real file offer must reach winit");
    assert_eq!(
        target.hovered,
        Some([42.0, 150.0]),
        "buffer hover needs native drag coordinates"
    );
    assert_eq!(
        target.buffer.as_deref(),
        Some("audio"),
        "buffer binding needs the drop coordinates"
    );
}
