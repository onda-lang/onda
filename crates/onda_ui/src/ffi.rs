use crate::{FramePacer, Input, PresentationOptions, RunUi, SurfaceRenderer};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::{
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
};

pub struct OndaUi {
    gpu: SurfaceRenderer,
    ui: RunUi,
    scale: f32,
    rendered: Option<(u64, u64)>,
    commands: CString,
    error: CString,
    text_edit: CString,
}

#[repr(C)]
pub struct OndaUiCreateOptions {
    pub platform: u32,
    pub window: usize,
    pub display: usize,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub present_mode: u32,
    pub maximum_frame_latency: u32,
}

fn string(s: impl AsRef<str>) -> CString {
    CString::new(s.as_ref().replace('\0', " ")).expect("NULs removed")
}
fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

#[no_mangle]
pub extern "C" fn onda_ui_frame_pacer_create(rate_hz: f64) -> *mut FramePacer {
    guard(std::ptr::null_mut(), || {
        FramePacer::new((rate_hz != 0.0).then_some(rate_hz))
            .map_or(std::ptr::null_mut(), |pacer| Box::into_raw(Box::new(pacer)))
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_frame_pacer_destroy(pacer: *mut FramePacer) {
    guard((), || {
        if !pacer.is_null() {
            drop(unsafe { Box::from_raw(pacer) });
        }
    });
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_frame_pacer_timer_delay_ms(
    pacer: *const FramePacer,
    now_ms: f64,
) -> f64 {
    guard(0.0, || {
        unsafe { pacer.as_ref() }.map_or(0.0, |pacer| pacer.timer_delay_ms(now_ms))
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_frame_pacer_display_delay_ms(
    pacer: *const FramePacer,
    now_ms: f64,
) -> f64 {
    guard(0.0, || {
        unsafe { pacer.as_ref() }.map_or(0.0, |pacer| pacer.display_delay_ms(now_ms))
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_frame_pacer_timer_submitted(
    pacer: *mut FramePacer,
    started_ms: f64,
) {
    guard((), || {
        if let Some(pacer) = unsafe { pacer.as_mut() } {
            pacer.timer_submitted(started_ms);
        }
    });
}

#[no_mangle]
pub unsafe extern "C" fn onda_ui_frame_pacer_display_submitted(
    pacer: *mut FramePacer,
    display_ms: f64,
) {
    guard((), || {
        if let Some(pacer) = unsafe { pacer.as_mut() } {
            pacer.display_submitted(display_ms);
        }
    });
}

#[no_mangle]
pub unsafe extern "C" fn onda_ui_present_modes(ui: *const OndaUi) -> u32 {
    guard(0, || {
        unsafe { ui.as_ref() }.map_or(0, |ui| ui.gpu.present_mode_mask())
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_present_mode(ui: *const OndaUi) -> u32 {
    guard(u32::MAX, || {
        unsafe { ui.as_ref() }.map_or(u32::MAX, |ui| ui.gpu.presentation().present_mode as u32)
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_set_presentation(
    ui: *mut OndaUi,
    mode: u32,
    maximum_frame_latency: u32,
) -> i32 {
    guard(0, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return 0;
        };
        match PresentationOptions::from_raw(mode, maximum_frame_latency)
            .and_then(|options| ui.gpu.set_presentation(options))
        {
            Ok(()) => {
                ui.rendered = None;
                1
            }
            Err(error) => {
                ui.error = string(error);
                0
            }
        }
    })
}

/// The caller keeps both handles valid until the returned UI is destroyed.
unsafe fn surface(
    platform: u32,
    window: usize,
    display: usize,
) -> Result<(wgpu::Instance, wgpu::Surface<'static>), String> {
    #[cfg(not(target_os = "linux"))]
    let _ = display;
    let (raw_window_handle, raw_display_handle) = match platform {
        #[cfg(target_os = "linux")]
        1 if window != 0 && display != 0 => (
            RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(window as _)),
            RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(
                std::ptr::NonNull::new(display as *mut _),
                0,
            )),
        ),
        #[cfg(target_os = "windows")]
        2 if window != 0 => (
            RawWindowHandle::Win32(raw_window_handle::Win32WindowHandle::new(
                std::num::NonZeroIsize::new(window as isize).ok_or("Null HWND")?,
            )),
            RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new()),
        ),
        #[cfg(target_os = "macos")]
        3 if window != 0 => (
            RawWindowHandle::AppKit(raw_window_handle::AppKitWindowHandle::new(
                std::ptr::NonNull::new(window as *mut _).ok_or("Null NSView")?,
            )),
            RawDisplayHandle::AppKit(raw_window_handle::AppKitDisplayHandle::new()),
        ),
        _ => return Err("Unsupported or invalid native UI handles".into()),
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        // WGL's detached cleanup thread can outlive this unloadable DLL.
        // DirectX 12 and Vulkan do not start that thread.
        #[cfg(target_os = "windows")]
        backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // SAFETY: handle validity and lifetime are part of the C interface contract.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_window_handle,
            raw_display_handle: Some(raw_display_handle),
        })
    }
    .map_err(|e| e.to_string())?;
    Ok((instance, surface))
}

#[no_mangle]
pub unsafe extern "C" fn onda_ui_create(
    options: *const OndaUiCreateOptions,
    error: *mut c_char,
    capacity: usize,
) -> *mut OndaUi {
    guard(std::ptr::null_mut(), || {
        let result = (|| {
            let &OndaUiCreateOptions {
                platform,
                window,
                display,
                width,
                height,
                scale,
                present_mode,
                maximum_frame_latency,
            } = unsafe { options.as_ref() }.ok_or("Missing UI creation options")?;
            if !scale.is_finite() || scale <= 0.0 {
                return Err("Invalid display scale".into());
            }
            let (instance, surface) = unsafe { surface(platform, window, display) }?;
            let presentation = PresentationOptions::from_raw(present_mode, maximum_frame_latency)?;
            let gpu = pollster::block_on(SurfaceRenderer::new(
                instance,
                surface,
                width,
                height,
                presentation,
            ))?;
            let mut ui = RunUi::default();
            ui.resize(width as f32 / scale, height as f32 / scale, scale);
            Ok::<_, String>(OndaUi {
                gpu,
                ui,
                scale,
                rendered: None,
                commands: string("[]"),
                error: string(""),
                text_edit: string("null"),
            })
        })();
        match result {
            Ok(ui) => Box::into_raw(Box::new(ui)),
            Err(message) => {
                if !error.is_null() && capacity > 0 {
                    let bytes = message.as_bytes();
                    let n = bytes.len().min(capacity - 1);
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), error.cast(), n);
                        *error.add(n) = 0;
                    }
                }
                std::ptr::null_mut()
            }
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_destroy(ui: *mut OndaUi) {
    guard((), || {
        if !ui.is_null() {
            drop(unsafe { Box::from_raw(ui) });
        }
    });
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_resize(ui: *mut OndaUi, width: u32, height: u32, scale: f32) {
    guard((), || {
        if let Some(ui) = unsafe { ui.as_mut() } {
            if !scale.is_finite() || scale <= 0.0 {
                return;
            }
            ui.gpu.resize(width, height);
            ui.scale = scale;
            ui.ui
                .resize(width as f32 / scale, height as f32 / scale, scale);
            ui.rendered = None;
        }
    });
}

unsafe fn receive(ui: *mut OndaUi, json: *const c_char, input: bool) -> i32 {
    let failure = if input { -1 } else { 0 };
    guard(failure, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return failure;
        };
        if json.is_null() {
            return failure;
        }
        let result = (|| {
            let text = unsafe { CStr::from_ptr(json) }
                .to_str()
                .map_err(|e| e.to_string())?;
            if input {
                let input: Input = serde_json::from_str(text).map_err(|e| e.to_string())?;
                Ok::<_, String>(i32::from(ui.gpu.renderer.input(&mut ui.ui, input)))
            } else {
                ui.ui
                    .message(serde_json::from_str(text).map_err(|e| e.to_string())?);
                Ok(1)
            }
        })();
        match result {
            Ok(consumed) => consumed,
            Err(error) => {
                ui.error = string(error);
                failure
            }
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_message(ui: *mut OndaUi, json: *const c_char) -> i32 {
    unsafe { receive(ui, json, false) }
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_input(ui: *mut OndaUi, json: *const c_char) -> i32 {
    unsafe { receive(ui, json, true) }
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_scope(
    ui: *mut OndaUi,
    channels: u32,
    samples: *const f32,
    count: usize,
) {
    guard((), || {
        if let Some(ui) = unsafe { ui.as_mut() } {
            if count == 0 {
                ui.ui.set_scope_interleaved(0, &[]);
            } else if !samples.is_null() && channels > 0 && channels <= 64 && count <= 64 * 16384 {
                ui.ui.set_scope_interleaved(channels as usize, unsafe {
                    std::slice::from_raw_parts(samples, count)
                });
            }
        }
    });
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_update(ui: *mut OndaUi) -> i32 {
    guard(-1, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return -1;
        };
        ui.ui.update();
        i32::from(ui.rendered != Some(ui.ui.revisions()))
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_wake_delay_ms(ui: *const OndaUi) -> f64 {
    guard(-1.0, || {
        unsafe { ui.as_ref() }
            .and_then(|ui| ui.ui.wake_deadline())
            .map_or(-1.0, |deadline| {
                deadline
                    .saturating_duration_since(web_time::Instant::now())
                    .as_secs_f64()
                    * 1000.0
            })
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_render(ui: *mut OndaUi) -> i32 {
    guard(-1, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return -1;
        };
        ui.ui.update();
        match ui.gpu.render(&mut ui.ui, ui.scale) {
            Ok(true) => {
                ui.rendered = Some(ui.ui.revisions());
                1
            }
            Ok(false) => 0,
            Err(error) => {
                ui.error = string(error);
                -1
            }
        }
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_commands(ui: *mut OndaUi) -> *const c_char {
    guard(std::ptr::null(), || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return std::ptr::null();
        };
        ui.commands =
            string(serde_json::to_string(&ui.ui.take_commands()).unwrap_or_else(|_| "[]".into()));
        ui.commands.as_ptr()
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_error(ui: *const OndaUi) -> *const c_char {
    unsafe { ui.as_ref() }
        .map(|ui| ui.error.as_ptr())
        .unwrap_or(std::ptr::null())
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_text_edit(ui: *mut OndaUi) -> *const c_char {
    guard(std::ptr::null(), || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return std::ptr::null();
        };
        ui.text_edit =
            string(serde_json::to_string(&ui.ui.text_edit()).unwrap_or_else(|_| "null".into()));
        ui.text_edit.as_ptr()
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_text_bounds(
    ui: *mut OndaUi,
    start: usize,
    end: usize,
    rect: *mut f32,
) -> i32 {
    guard(0, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return 0;
        };
        if rect.is_null() {
            return 0;
        }
        let Some(bounds) = ui.gpu.renderer.text_bounds(&mut ui.ui, start, end) else {
            return 0;
        };
        unsafe { std::ptr::copy_nonoverlapping(bounds.as_ptr(), rect, 4) };
        1
    })
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_text_hit(ui: *mut OndaUi, x: f32, y: f32) -> usize {
    guard(usize::MAX, || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return usize::MAX;
        };
        ui.gpu
            .renderer
            .text_hit(&mut ui.ui, x, y)
            .unwrap_or(usize::MAX)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_ui_scope_visible(ui: *mut OndaUi) -> i32 {
    guard(0, || {
        unsafe { ui.as_mut() }.is_some_and(|ui| ui.ui.scope_enabled()) as i32
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_ui_meter(
    ui: *mut OndaUi,
    channels: u32,
    peaks: *const f32,
    count: usize,
) {
    guard((), || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return;
        };
        if count > onda_scope::MAX_CHANNELS || (count > 0 && peaks.is_null()) {
            return;
        }
        let peaks = if count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(peaks, count) }
        };
        ui.ui.set_meter(channels as usize, peaks);
    });
}
#[no_mangle]
pub unsafe extern "C" fn onda_ui_meter_visible(ui: *mut OndaUi) -> i32 {
    guard(0, || {
        unsafe { ui.as_mut() }.is_some_and(|ui| ui.ui.meter_enabled()) as i32
    })
}

/// CSS cursor identifier, with static lifetime, for the host's native pointer.
#[no_mangle]
pub unsafe extern "C" fn onda_ui_cursor(ui: *mut OndaUi) -> *const c_char {
    guard(c"default".as_ptr(), || {
        let Some(ui) = (unsafe { ui.as_mut() }) else {
            return c"default".as_ptr();
        };
        match ui.ui.cursor() {
            "pointer" => c"pointer".as_ptr(),
            "text" => c"text".as_ptr(),
            "ew-resize" => c"ew-resize".as_ptr(),
            "ns-resize" => c"ns-resize".as_ptr(),
            _ => c"default".as_ptr(),
        }
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
