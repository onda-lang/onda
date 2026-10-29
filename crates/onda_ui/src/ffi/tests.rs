//! C interface regressions using a host-owned native surface.
use super::*;
use serde_json::{Value, json};
use x11_dl::xlib::{Display, Window, Xlib};

struct TestWindow {
    xlib: Xlib,
    display: *mut Display,
    window: Window,
}

impl TestWindow {
    fn new() -> Self {
        let xlib = Xlib::open().expect("Xlib");
        unsafe {
            (xlib.XInitThreads)();
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            assert!(!display.is_null(), "An accessible X11 display is required");
            let window = (xlib.XCreateSimpleWindow)(
                display,
                (xlib.XDefaultRootWindow)(display),
                0,
                0,
                1000,
                600,
                0,
                0,
                0,
            );
            (xlib.XSync)(display, 0);
            Self {
                xlib,
                display,
                window,
            }
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            (self.xlib.XDestroyWindow)(self.display, self.window);
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}

#[test]
#[ignore = "requires an X11 display"]
fn initial_state_restores_scroll_before_first_resize_or_render() {
    let window = TestWindow::new();
    unsafe {
        let mut error = [0; 2048];
        let ui = onda_ui_create(
            1,
            window.window as usize,
            window.display as usize,
            1000,
            600,
            2.0,
            error.as_mut_ptr(),
            error.len(),
        );
        assert!(!ui.is_null(), "{:?}", CStr::from_ptr(error.as_ptr()));
        // Own the C allocation so it is released before the native handles,
        // including when an assertion fails.
        let mut ui = Box::from_raw(ui);
        let ui = &mut *ui as *mut OndaUi;
        let state = string(
            json!({"type":"state","state":{
                "path":"test.onda","supportsScope":false,"supportsViewState":true,
                "paramLayout":"sliders",
                "params":(0..8).map(|i| json!({
                    "name":format!("gain{i}"),"type":"f64","value":0.5
                })).collect::<Vec<_>>(),
                "viewState":{"scrollTop":100}
            }})
            .to_string(),
        );
        assert_eq!(onda_ui_message(ui, state.as_ptr()), 1);
        // JUCE queries text input state while draining its ready callback,
        // before the first resize or rendered frame.
        onda_ui_text_edit(ui);
        onda_ui_resize(ui, 1000, 600, 2.0);
        for input in [
            json!({"type":"scroll","delta":1}),
            json!({"type":"focusLost"}),
        ] {
            assert_eq!(onda_ui_input(ui, string(input.to_string()).as_ptr()), 1);
        }
        let commands: Vec<Value> =
            serde_json::from_slice(CStr::from_ptr(onda_ui_commands(ui)).to_bytes()).unwrap();
        let view = commands.iter().find(|c| c["type"] == "viewState").unwrap();
        assert_eq!(view["state"]["scrollTop"], 101.0);
        assert_eq!(onda_ui_input(ui, c"{\"type\":\"text\",\"text\":\" \"}".as_ptr()), 0);
        assert_eq!(onda_ui_input(ui, c"{\"type\":\"key\",\"key\":\"Tab\"}".as_ptr()), 1);
        assert_eq!(onda_ui_input(ui, c"invalid JSON".as_ptr()), -1);
    }
}
