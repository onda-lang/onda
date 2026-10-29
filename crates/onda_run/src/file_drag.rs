//! Native drag coordinates shared by the windowed run hosts.
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

#[cfg(target_os = "linux")]
mod x11;

#[derive(Default)]
pub struct FileDragPosition {
    #[cfg(target_os = "linux")]
    x11: Option<x11::DragPosition>,
    #[cfg(not(target_os = "linux"))]
    window: Option<RawWindowHandle>,
}

impl FileDragPosition {
    /// Install tracking once, before the first drag. Unsupported handles disable tracking.
    ///
    /// # Safety
    /// The host must keep the window and display alive until this tracker is dropped,
    /// and construct, query and drop it on the window's thread.
    pub unsafe fn new(window: RawWindowHandle, display: RawDisplayHandle) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                x11: x11::DragPosition::new(window, display).ok(),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = display;
            Self {
                window: Some(window),
            }
        }
    }

    /// Window-relative native coordinates: logical AppKit points, physical pixels elsewhere.
    pub fn position(&self) -> Option<[f32; 2]> {
        #[cfg(target_os = "linux")]
        {
            self.x11.as_ref()?.position()
        }
        #[cfg(not(target_os = "linux"))]
        {
            native_pointer_position(self.window?)
        }
    }
}

#[cfg(target_os = "windows")]
fn native_pointer_position(handle: RawWindowHandle) -> Option<[f32; 2]> {
    use windows_sys::Win32::{
        Foundation::POINT, Graphics::Gdi::ScreenToClient, UI::WindowsAndMessaging::GetCursorPos,
    };
    let RawWindowHandle::Win32(handle) = handle else {
        return None;
    };
    let mut position = POINT { x: 0, y: 0 };
    // SAFETY: The host retains this HWND; both APIs write to our local POINT.
    unsafe {
        if GetCursorPos(&mut position) == 0
            || ScreenToClient(handle.hwnd.get() as _, &mut position) == 0
        {
            return None;
        }
    }
    Some([position.x as f32, position.y as f32])
}

#[cfg(target_os = "macos")]
fn native_pointer_position(handle: RawWindowHandle) -> Option<[f32; 2]> {
    use objc2_app_kit::NSView;
    let RawWindowHandle::AppKit(handle) = handle else {
        return None;
    };
    // SAFETY: The host retains this NSView and queries it on the main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let position = view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
    let y = if view.isFlipped() {
        position.y
    } else {
        view.bounds().size.height - position.y
    };
    Some([position.x as f32, y as f32])
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn native_pointer_position(_: RawWindowHandle) -> Option<[f32; 2]> {
    None
}
