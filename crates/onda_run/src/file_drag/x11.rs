//! Observe XDND coordinates on the window's Xlib connection.
//!
//! Xwayland's core pointer can freeze during drags from Wayland clients. winit
//! receives XDND positions but exposes only file paths. Xlib's event conversion
//! hook lets us retain those positions while preserving winit's original events.

use std::{
    ffi::{c_int, c_ulong},
    ptr::NonNull,
    sync::{Arc, Mutex},
};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use x11_dl::xlib::{self, xEvent, Display, XEvent, Xlib};
use x11rb::{protocol::xproto::ConnectionExt, rust_connection::RustConnection};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndEnter,
        XdndPosition,
        XdndLeave,
    }
}

type Error = Box<dyn std::error::Error + Send + Sync>;
type Converter = unsafe extern "C" fn(*mut Display, *mut XEvent, *mut xEvent) -> c_int;

#[derive(Clone)]
struct Target {
    window: c_ulong,
    atoms: Atoms,
    position: Arc<Mutex<Option<(i16, i16)>>>,
}

struct DisplayHook {
    display: usize,
    original: Converter,
    targets: Vec<Target>,
}

// Xlib invokes the converter while holding its display lock. Installation and
// removal acquire that same lock before this registry, keeping the order fixed.
static HOOKS: Mutex<Vec<DisplayHook>> = Mutex::new(Vec::new());

pub(super) struct DragPosition {
    xlib: Xlib,
    display: NonNull<Display>,
    connection: RustConnection,
    root: u32,
    target: Target,
}

impl DragPosition {
    pub(super) fn new(window: RawWindowHandle, display: RawDisplayHandle) -> Result<Self, Error> {
        let (RawWindowHandle::Xlib(window), RawDisplayHandle::Xlib(display)) = (window, display)
        else {
            return Err("file drag tracking requires an Xlib window".into());
        };
        let display = display
            .display
            .ok_or("Xlib display is unavailable")?
            .cast::<Display>();
        let xlib = Xlib::open()?;
        let (connection, _) = x11rb::connect(None)?;
        let atoms = Atoms::new(&connection)?.reply()?;
        let root = connection
            .get_geometry(u32::try_from(window.window)?)?
            .reply()?
            .root;
        let target = Target {
            window: window.window,
            atoms,
            position: Arc::new(Mutex::new(None)),
        };
        // SAFETY: The host supplies its live Xlib display. The public Xlib hook
        // changes only event conversion, under the display lock. We retain and
        // call the previous converter and restore it when its last target exits.
        unsafe {
            (xlib.XLockDisplay)(display.as_ptr());
            let mut hooks = HOOKS.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(hook) = hooks
                .iter_mut()
                .find(|hook| hook.display == display.as_ptr() as usize)
            {
                hook.targets.push(target.clone());
            } else {
                let original = (xlib.XESetWireToEvent)(
                    display.as_ptr(),
                    xlib::ClientMessage,
                    Some(convert_event),
                );
                let Some(original) = original else {
                    (xlib.XESetWireToEvent)(display.as_ptr(), xlib::ClientMessage, None);
                    (xlib.XUnlockDisplay)(display.as_ptr());
                    return Err("Xlib client-message converter is unavailable".into());
                };
                hooks.push(DisplayHook {
                    display: display.as_ptr() as usize,
                    original,
                    targets: vec![target.clone()],
                });
            }
            drop(hooks);
            (xlib.XUnlockDisplay)(display.as_ptr());
        }
        Ok(Self {
            xlib,
            display,
            connection,
            root,
            target,
        })
    }

    pub(super) fn position(&self) -> Option<[f32; 2]> {
        let (x, y) = (*self.target.position.lock().ok()?)?;
        let translated = self
            .connection
            .translate_coordinates(self.root, u32::try_from(self.target.window).ok()?, x, y)
            .ok()?
            .reply()
            .ok()?;
        Some([f32::from(translated.dst_x), f32::from(translated.dst_y)])
    }
}

impl Drop for DragPosition {
    fn drop(&mut self) {
        // SAFETY: The tracker belongs to the app, which exits before the host's
        // display closes. Use the same lock order as installation and conversion.
        unsafe {
            (self.xlib.XLockDisplay)(self.display.as_ptr());
            let mut hooks = HOOKS.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(index) = hooks
                .iter()
                .position(|hook| hook.display == self.display.as_ptr() as usize)
            {
                let hook = &mut hooks[index];
                hook.targets
                    .retain(|target| target.window != self.target.window);
                if hook.targets.is_empty() {
                    (self.xlib.XESetWireToEvent)(
                        self.display.as_ptr(),
                        xlib::ClientMessage,
                        Some(hook.original),
                    );
                    hooks.remove(index);
                }
            }
            drop(hooks);
            (self.xlib.XUnlockDisplay)(self.display.as_ptr());
        }
    }
}

unsafe extern "C" fn convert_event(
    display: *mut Display,
    event: *mut XEvent,
    wire: *mut xEvent,
) -> c_int {
    let original = {
        let hooks = HOOKS.lock().unwrap_or_else(|error| error.into_inner());
        let Some(hook) = hooks.iter().find(|hook| hook.display == display as usize) else {
            return 0;
        };
        hook.original
    };
    // SAFETY: Xlib passes the buffers expected by its original converter. This
    // hook is installed only for ClientMessage; a successful conversion initializes
    // that member of XEvent. Neither the event nor the wire packet is modified here.
    let converted = unsafe { original(display, event, wire) };
    if converted == 0 {
        return converted;
    }
    let event = unsafe { (*event).client_message };
    let target = {
        let hooks = HOOKS.lock().unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .find(|hook| hook.display == display as usize)
            .and_then(|hook| {
                hook.targets
                    .iter()
                    .find(|target| target.window == event.window)
            })
            .cloned()
    };
    if let Some(target) = target {
        let position = if event.message_type == c_ulong::from(target.atoms.XdndPosition)
            && event.format == 32
        {
            let packed = event.data.get_long(2) as u32;
            Some(Some(((packed >> 16) as i16, packed as i16)))
        } else if event.message_type == c_ulong::from(target.atoms.XdndEnter)
            || event.message_type == c_ulong::from(target.atoms.XdndLeave)
        {
            Some(None)
        } else {
            None
        };
        if let Some(position) = position {
            *target
                .position
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = position;
        }
    }
    converted
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::{XlibDisplayHandle, XlibWindowHandle};
    use std::time::{Duration, Instant};
    use x11rb::{
        connection::Connection,
        protocol::xproto::{ClientMessageEvent, EventMask},
    };

    // Own an unmapped Xlib window so the test exercises real event conversion
    // without opening a GUI or moving the user's pointer.
    struct TestWindow {
        xlib: Xlib,
        display: NonNull<Display>,
        window: c_ulong,
    }

    impl TestWindow {
        fn new() -> Self {
            let xlib = Xlib::open().unwrap();
            // SAFETY: Initialize Xlib threading before opening this test display.
            let (display, window) = unsafe {
                (xlib.XInitThreads)();
                let display =
                    NonNull::new((xlib.XOpenDisplay)(std::ptr::null())).expect("X11 display");
                let root = (xlib.XDefaultRootWindow)(display.as_ptr());
                let window =
                    (xlib.XCreateSimpleWindow)(display.as_ptr(), root, 100, 100, 400, 300, 0, 0, 0);
                (xlib.XSync)(display.as_ptr(), 0);
                (display, window)
            };
            Self {
                xlib,
                display,
                window,
            }
        }

        fn tracker(&self) -> DragPosition {
            DragPosition::new(
                RawWindowHandle::Xlib(XlibWindowHandle::new(self.window)),
                RawDisplayHandle::Xlib(XlibDisplayHandle::new(Some(self.display.cast()), 0)),
            )
            .unwrap()
        }

        fn receive(&self) -> xlib::XClientMessageEvent {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                // SAFETY: This test owns the display; XNextEvent initializes the
                // event and we inspect its matching union member after checking type.
                unsafe {
                    if (self.xlib.XPending)(self.display.as_ptr()) != 0 {
                        let mut event = std::mem::MaybeUninit::<XEvent>::uninit();
                        (self.xlib.XNextEvent)(self.display.as_ptr(), event.as_mut_ptr());
                        let event = event.assume_init();
                        if event.type_ == xlib::ClientMessage {
                            return event.client_message;
                        }
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "native drag event did not arrive"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: Trackers borrow this fixture and are dropped first.
            unsafe {
                (self.xlib.XDestroyWindow)(self.display.as_ptr(), self.window);
                (self.xlib.XCloseDisplay)(self.display.as_ptr());
            }
        }
    }

    #[test]
    #[ignore = "requires an X11 display"]
    fn xdnd_coordinates_follow_native_events_without_core_pointer_motion() {
        let fixture = TestWindow::new();
        let (sender, _) = x11rb::connect(None).unwrap();
        let atoms = Atoms::new(&sender).unwrap().reply().unwrap();
        let send = |type_, packed| {
            let data = [0, 0, packed, 0, 0];
            sender
                .send_event(
                    false,
                    u32::try_from(fixture.window).unwrap(),
                    EventMask::NO_EVENT,
                    ClientMessageEvent::new(
                        32,
                        u32::try_from(fixture.window).unwrap(),
                        type_,
                        data,
                    ),
                )
                .unwrap()
                .check()
                .unwrap();
            sender.flush().unwrap();
            let event = fixture.receive();
            assert_eq!(event.window, fixture.window);
            assert_eq!(event.message_type, c_ulong::from(type_));
            assert_eq!(
                std::array::from_fn::<_, 5, _>(|index| event.data.get_long(index) as u32),
                data,
                "winit must receive the original drag event unchanged"
            );
        };
        // Exercise repeated installation and removal on the same display.
        for _ in 0..2 {
            let tracker = fixture.tracker();
            send(atoms.XdndEnter, 0);
            assert_eq!(tracker.position(), None);
            // Only the native drag payload moves; core pointer position is irrelevant.
            for (x, y) in [(120_u32, 130_u32), (260, 190), (80, 70)] {
                send(atoms.XdndPosition, (x << 16) | y);
                assert_eq!(
                    tracker.position(),
                    Some([x as f32 - 100.0, y as f32 - 100.0])
                );
            }
            send(atoms.XdndLeave, 0);
            assert_eq!(tracker.position(), None);
            drop(tracker);
            // Removing the observer must preserve ordinary Xlib event decoding.
            send(atoms.XdndPosition, (120 << 16) | 130);
        }
    }
}
