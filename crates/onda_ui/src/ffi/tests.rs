//! C interface regressions using a host-owned native surface.
use super::*;
use serde_json::{json, Value};
use x11_dl::xlib::{Display, Window, Xlib};

struct TestWindow {
    xlib: Xlib,
    display: *mut Display,
    window: Window,
}

#[test]
fn creation_requires_options_and_reports_the_error() {
    unsafe {
        let mut error = [0; 128];
        assert!(onda_ui_create(std::ptr::null(), error.as_mut_ptr(), error.len()).is_null());
        assert_eq!(
            CStr::from_ptr(error.as_ptr()).to_str().unwrap(),
            "Missing UI creation options"
        );
    }
}

#[test]
fn optional_c_pacer_uses_only_host_timestamps() {
    unsafe {
        assert!(onda_ui_frame_pacer_create(-1.0).is_null());
        for hz in [0.0, 60.0, 120.0] {
            let pacer = onda_ui_frame_pacer_create(hz);
            assert!(!pacer.is_null());
            assert_eq!(onda_ui_frame_pacer_timer_delay_ms(pacer, 100.0), 0.0);
            assert_eq!(onda_ui_frame_pacer_display_delay_ms(pacer, 100.0), 0.0);
            onda_ui_frame_pacer_timer_submitted(pacer, 100.0);
            let delay = onda_ui_frame_pacer_timer_delay_ms(pacer, 100.0);
            assert!((delay - if hz == 0.0 { 0.0 } else { 1000.0 / hz }).abs() < 1e-10);
            assert!(
                (onda_ui_frame_pacer_display_delay_ms(pacer, 100.0) - delay * 0.5).abs() < 1e-10
            );
            if hz > 0.0 {
                let early = 100.0 + 1000.0 / hz - 0.1;
                assert!(onda_ui_frame_pacer_timer_delay_ms(pacer, early) > 0.0);
                assert_eq!(onda_ui_frame_pacer_display_delay_ms(pacer, early), 0.0);
            }
            onda_ui_frame_pacer_destroy(pacer);
        }
    }
}

#[test]
fn c_display_pacer_skips_expired_slots_without_resetting_its_phase() {
    unsafe {
        let pacer = onda_ui_frame_pacer_create(60.0);
        assert!(!pacer.is_null());
        for timestamp in [3.0, 29.667, 56.333, 63.667] {
            assert_eq!(onda_ui_frame_pacer_display_delay_ms(pacer, timestamp), 0.0);
            onda_ui_frame_pacer_display_submitted(pacer, timestamp);
            assert!(onda_ui_frame_pacer_display_delay_ms(pacer, timestamp) > 0.0);
        }
        onda_ui_frame_pacer_destroy(pacer);
    }
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
            (xlib.XMapWindow)(display, window);
            (xlib.XSync)(display, 0);
            Self {
                xlib,
                display,
                window,
            }
        }
    }
}

#[test]
#[ignore = "requires an X11 display"]
fn scheduling_updates_without_drawing_and_clears_deadlines_after_command_delivery() {
    let window = TestWindow::new();
    unsafe {
        let mut error = [0; 2048];
        let options = OndaUiCreateOptions {
            platform: 1,
            window: window.window as usize,
            display: window.display as usize,
            width: 1000,
            height: 600,
            scale: 2.0,
            present_mode: 2,
            maximum_frame_latency: 1,
        };
        let ui = onda_ui_create(&options, error.as_mut_ptr(), error.len());
        assert!(!ui.is_null(), "{:?}", CStr::from_ptr(error.as_ptr()));
        let mut owned = Box::from_raw(ui);
        let ui = &mut *owned as *mut OndaUi;
        assert_eq!(onda_ui_present_mode(ui), 2);
        assert_ne!(onda_ui_present_modes(ui) & (1 << 2), 0);
        assert_eq!(onda_ui_set_presentation(ui, 2, 0), 0);
        assert_eq!(
            onda_ui_present_mode(ui),
            2,
            "rejected settings preserve the surface"
        );
        assert_eq!(onda_ui_set_presentation(ui, 6, 2), 1);
        assert_eq!(onda_ui_present_modes(ui) & (1 << 6), 0);
        let expected = if onda_ui_present_modes(ui) & (1 << 5) != 0 {
            5
        } else {
            2
        };
        assert_eq!(onda_ui_present_mode(ui), expected);
        let state = string(json!({"type":"state","state":{
            "path":"schedule.onda","supportsScope":false,"supportsViewState":true,
            "params":(0..30).map(|i| json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>()
        }}).to_string());
        assert_eq!(onda_ui_message(ui, state.as_ptr()), 1);
        assert_eq!(onda_ui_update(ui), 1);
        assert_eq!(onda_ui_render(ui), 1);
        onda_ui_commands(ui);
        assert_eq!(onda_ui_update(ui), 0);
        // Explicit rendering is independent of dirty state and frame pacing.
        assert_eq!(onda_ui_render(ui), 1);
        assert_eq!(onda_ui_wake_delay_ms(ui), -1.0);

        assert_eq!(
            onda_ui_input(ui, c"{\"type\":\"scroll\",\"delta\":20}".as_ptr()),
            1
        );
        assert_eq!(onda_ui_update(ui), 1);
        assert_eq!(onda_ui_render(ui), 1);
        onda_ui_commands(ui);
        let delay = onda_ui_wake_delay_ms(ui);
        assert!(delay > 0.0 && delay <= 200.0, "{delay}");
        std::thread::sleep(std::time::Duration::from_millis(delay.ceil() as u64 + 1));
        assert_eq!(onda_ui_update(ui), 0, "persistence needs no GPU frame");
        let commands: Vec<Value> =
            serde_json::from_slice(CStr::from_ptr(onda_ui_commands(ui)).to_bytes()).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["state"]["scrollTop"], 20.0);
        assert_eq!(onda_ui_wake_delay_ms(ui), -1.0);
        assert_eq!(onda_ui_update(ui), 0);
        onda_ui_resize(ui, 1000, 600, 2.0);
        assert_eq!(onda_ui_update(ui), 1, "exposure invalidates clean pixels");
        assert_eq!(onda_ui_update(std::ptr::null_mut()), -1);
        assert_eq!(onda_ui_wake_delay_ms(std::ptr::null()), -1.0);
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
        let options = OndaUiCreateOptions {
            platform: 1,
            window: window.window as usize,
            display: window.display as usize,
            width: 1000,
            height: 600,
            scale: 2.0,
            present_mode: 2,
            maximum_frame_latency: 1,
        };
        let ui = onda_ui_create(&options, error.as_mut_ptr(), error.len());
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
        assert_eq!(
            onda_ui_input(ui, c"{\"type\":\"text\",\"text\":\" \"}".as_ptr()),
            0
        );
        assert_eq!(
            onda_ui_input(ui, c"{\"type\":\"key\",\"key\":\"Tab\"}".as_ptr()),
            1
        );
        assert_eq!(onda_ui_input(ui, c"invalid JSON".as_ptr()), -1);

        let state = string(
            json!({"type":"state","state":{
                "supportsScope":true,"viewState":{"readyId":1,"scopePinned":true}
            }})
            .to_string(),
        );
        assert_eq!(onda_ui_message(ui, state.as_ptr()), 1);
        assert_eq!(onda_ui_scope_visible(ui), 1);
        assert_eq!(onda_ui_meter_visible(ui), 1);
        assert_eq!(
            onda_ui_input(ui, c"{\"type\":\"pointerMove\",\"x\":2,\"y\":290}".as_ptr()),
            1
        );
        for (delta, visible) in [(1000, 0), (-1000, 1)] {
            let input = string(json!({"type":"scroll","delta":delta}).to_string());
            assert_eq!(onda_ui_input(ui, input.as_ptr()), 1);
            assert_eq!(onda_ui_scope_visible(ui), visible);
            assert_eq!(onda_ui_meter_visible(ui), visible);
        }
    }
}
