use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{c_void, CStr, CString};

use onda::*;

fn diag_message(diag: &onda_diag_t) -> String {
    if diag.message.is_null() {
        return "<null>".to_owned();
    }
    unsafe { CStr::from_ptr(diag.message).to_string_lossy().into_owned() }
}

fn empty_diag() -> onda_diag_t {
    onda_diag_t {
        code: 0,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        message: std::ptr::null(),
        file: std::ptr::null(),
        trace: std::ptr::null(),
    }
}

struct ProgramHandle(*mut onda_program);

impl Drop for ProgramHandle {
    fn drop(&mut self) {
        unsafe {
            onda_program_destroy(self.0);
        }
    }
}

struct InstanceHandle(*mut onda_instance);

impl Drop for InstanceHandle {
    fn drop(&mut self) {
        unsafe {
            onda_instance_destroy(self.0);
        }
    }
}

#[derive(Default)]
struct AllocStats {
    allocs: usize,
    frees: usize,
    live: usize,
}

unsafe extern "C" fn test_alloc(context: *mut c_void, size: usize, align: usize) -> *mut c_void {
    let stats = &mut *(context.cast::<AllocStats>());
    let Ok(layout) = Layout::from_size_align(size, align) else {
        return std::ptr::null_mut();
    };
    let ptr = alloc(layout).cast::<c_void>();
    if !ptr.is_null() {
        stats.allocs += 1;
        stats.live += 1;
    }
    ptr
}

unsafe extern "C" fn test_free(context: *mut c_void, ptr: *mut c_void, size: usize, align: usize) {
    let stats = &mut *(context.cast::<AllocStats>());
    let layout = Layout::from_size_align(size, align).expect("valid free layout");
    dealloc(ptr.cast::<u8>(), layout);
    stats.frees += 1;
    stats.live -= 1;
}

unsafe fn compile_program(src: &str) -> ProgramHandle {
    let src_c = CString::new(src).expect("source contains no NUL bytes");
    let options = onda_compile_options_t {
        fast_math: 0,
        sample_rate: 48_000.0,
        block_size: 512,
    };
    let mut diag = empty_diag();
    let program = onda_compile(src_c.as_ptr(), &options, &mut diag);
    assert!(
        !program.is_null(),
        "compile failed: {}",
        diag_message(&diag)
    );
    ProgramHandle(program)
}

#[test]
fn c_api_compile_reports_diagnostic_ranges() {
    unsafe {
        let src = CString::new(
            r#"
const BAD = false
outs:
  out1
sample:
  a = [0.0, 0.0]
  a[BAD:] = 0.5
  out1 = a[0]
"#,
        )
        .expect("source contains no NUL bytes");
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 512,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut diag);
        assert!(program.is_null(), "compile unexpectedly succeeded");
        assert_eq!((diag.line, diag.column), (7, 5));
        assert_eq!(diag.end_line, 7);
        assert_eq!(diag.end_column, 8);
    }
}

#[test]
fn c_api_event_metadata_queries_work() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
events {
  note_on(note: i32 = 60, vel: i32, accent: bool = true) {
    amp = f32(vel) / 127.0
  }
  set_curve(values: f32[2] = [0.25, 0.75]) {
    amp = values[0] + values[1]
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        assert_eq!(onda_event_count(program.0), 2);
        assert_eq!(onda_event_param_count(program.0, 0), 3);
        assert_eq!(onda_event_param_count(program.0, 1), 1);
        let name0 = CStr::from_ptr(onda_event_name(program.0, 0))
            .to_string_lossy()
            .into_owned();
        let name1 = CStr::from_ptr(onda_event_name(program.0, 1))
            .to_string_lossy()
            .into_owned();
        assert_eq!(name0, "note_on");
        assert_eq!(name1, "set_curve");

        let note_on = CString::new("note_on").expect("valid cstr");
        let set_curve = CString::new("set_curve").expect("valid cstr");
        assert_eq!(onda_event_index(program.0, note_on.as_ptr()), 0);
        assert_eq!(onda_event_index(program.0, set_curve.as_ptr()), 1);
        assert_eq!(onda_event_payload_bytes(program.0, 0), 9);
        assert_eq!(onda_event_payload_bytes(program.0, 1), 8);

        let note_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 0))
            .to_string_lossy()
            .into_owned();
        let vel_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 1))
            .to_string_lossy()
            .into_owned();
        let accent_name = CStr::from_ptr(onda_event_param_name(program.0, 0, 2))
            .to_string_lossy()
            .into_owned();
        assert_eq!(note_name, "note");
        assert_eq!(vel_name, "vel");
        assert_eq!(accent_name, "accent");

        assert_eq!(onda_event_param_elem_type(program.0, 0, 0), 2);
        assert_eq!(onda_event_param_array_len(program.0, 0, 0), 1);
        assert_eq!(onda_event_param_is_slice(program.0, 0, 0), 0);
        assert_eq!(onda_event_param_offset_bytes(program.0, 0, 0), 0);
        assert_eq!(onda_event_param_has_default(program.0, 0, 0), 1);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 0, 0, std::ptr::null_mut(), 0),
            4
        );
        let mut note_default = [0_u8; 4];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                0,
                0,
                note_default.as_mut_ptr().cast::<c_void>(),
                note_default.len() as i32,
            ),
            4
        );
        assert_eq!(i32::from_ne_bytes(note_default), 60);

        assert_eq!(onda_event_param_has_default(program.0, 0, 1), 0);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 0, 1, std::ptr::null_mut(), 0),
            0
        );

        assert_eq!(onda_event_param_elem_type(program.0, 0, 2), 4);
        assert_eq!(onda_event_param_offset_bytes(program.0, 0, 2), 8);
        let mut accent_default = [0_u8; 1];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                0,
                2,
                accent_default.as_mut_ptr().cast::<c_void>(),
                accent_default.len() as i32,
            ),
            1
        );
        assert_eq!(accent_default[0], 1);

        assert_eq!(onda_event_param_elem_type(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_array_len(program.0, 1, 0), 2);
        assert_eq!(onda_event_param_is_slice(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_offset_bytes(program.0, 1, 0), 0);
        assert_eq!(onda_event_param_has_default(program.0, 1, 0), 1);
        assert_eq!(
            onda_event_param_default_bytes(program.0, 1, 0, std::ptr::null_mut(), 0),
            8
        );
        let mut curve_default = [0_u8; 8];
        assert_eq!(
            onda_event_param_default_bytes(
                program.0,
                1,
                0,
                curve_default.as_mut_ptr().cast::<c_void>(),
                curve_default.len() as i32,
            ),
            8
        );
        assert_eq!(
            f32::from_ne_bytes(curve_default[0..4].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            f32::from_ne_bytes(curve_default[4..8].try_into().unwrap()),
            0.75
        );
    }
}

#[test]
fn c_api_param_default_bytes_preserve_declared_types_and_arrays() {
    unsafe {
        let program = compile_program(
            r#"
params {
  gain: f32 = 0.25
  ratio: f64 = 0.5
  mode: i32 = 7
  wide: i64 = 1099511627776
  gate: bool = true
  curve: f32[2] = [0.25, 0.75]
}
outs { out1 }
sample { out1 = gain }
"#,
        );

        assert_eq!(onda_param_count(program.0), 6);

        assert_eq!(onda_param_type_bytes(program.0, 0), 4);
        assert_eq!(
            onda_param_default_bytes(program.0, 0, std::ptr::null_mut(), 0),
            4
        );
        let mut gain = [0_u8; 4];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                0,
                gain.as_mut_ptr().cast::<c_void>(),
                gain.len() as i32,
            ),
            4
        );
        assert_eq!(f32::from_ne_bytes(gain), 0.25);

        let mut ratio = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                1,
                ratio.as_mut_ptr().cast::<c_void>(),
                ratio.len() as i32,
            ),
            8
        );
        assert_eq!(f64::from_ne_bytes(ratio), 0.5);

        let mut mode = [0_u8; 4];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                2,
                mode.as_mut_ptr().cast::<c_void>(),
                mode.len() as i32,
            ),
            4
        );
        assert_eq!(i32::from_ne_bytes(mode), 7);

        let mut wide = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                3,
                wide.as_mut_ptr().cast::<c_void>(),
                wide.len() as i32,
            ),
            8
        );
        assert_eq!(i64::from_ne_bytes(wide), 1_099_511_627_776);

        let mut gate = [0_u8; 1];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                4,
                gate.as_mut_ptr().cast::<c_void>(),
                gate.len() as i32,
            ),
            1
        );
        assert_eq!(gate[0], 1);

        assert_eq!(onda_param_array_len(program.0, 5), 2);
        assert_eq!(
            onda_param_default_bytes(program.0, 5, std::ptr::null_mut(), 0),
            8
        );
        let mut curve = [0_u8; 8];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                5,
                curve.as_mut_ptr().cast::<c_void>(),
                curve.len() as i32,
            ),
            8
        );
        assert_eq!(f32::from_ne_bytes(curve[0..4].try_into().unwrap()), 0.25);
        assert_eq!(f32::from_ne_bytes(curve[4..8].try_into().unwrap()), 0.75);

        let mut too_small = [0_u8; 1];
        assert_eq!(
            onda_param_default_bytes(
                program.0,
                5,
                too_small.as_mut_ptr().cast::<c_void>(),
                too_small.len() as i32,
            ),
            8
        );
        assert_eq!(too_small, [0]);
        assert_eq!(
            onda_param_default_bytes(program.0, -1, std::ptr::null_mut(), 0),
            -1
        );
    }
}

#[test]
fn c_api_control_outputs_metadata_and_readback_work() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
kouts {
  meter: f32
  flags: bool[2]
}

block {
  meter = 3.5
  flags[0] = true
  flags[1] = false
}
"#,
        );

        assert_eq!(onda_output_count(program.0), 0);
        assert_eq!(onda_control_output_count(program.0), 2);

        let meter_name = CStr::from_ptr(onda_control_output_name(program.0, 0))
            .to_string_lossy()
            .into_owned();
        let flags_name = CStr::from_ptr(onda_control_output_name(program.0, 1))
            .to_string_lossy()
            .into_owned();
        assert_eq!(meter_name, "meter");
        assert_eq!(flags_name, "flags");

        let flags_key = CString::new("flags").expect("name contains no NUL bytes");
        assert_eq!(onda_control_output_index(program.0, flags_key.as_ptr()), 1);
        assert_eq!(onda_control_output_elem_type(program.0, 0), 0);
        assert_eq!(onda_control_output_type_bytes(program.0, 0), 4);
        assert_eq!(onda_control_output_array_len(program.0, 0), 1);
        assert_eq!(onda_control_output_elem_type(program.0, 1), 4);
        assert_eq!(onda_control_output_type_bytes(program.0, 1), 2);
        assert_eq!(onda_control_output_array_len(program.0, 1), 2);
        assert!(onda_control_output_slot_offset(program.0, 1) >= 0);
        assert!(onda_control_output_byte_offset(program.0, 1) >= 0);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 0, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        assert_eq!(onda_process_checked(instance.0, frames), 0);

        let mut meter = [0_u8; 4];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                0,
                meter.as_mut_ptr().cast::<c_void>(),
                meter.len() as i32,
            ),
            4
        );
        assert!((f32::from_ne_bytes(meter) - 3.5).abs() < 1e-6);

        let mut flags = [0_u8; 2];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                1,
                flags.as_mut_ptr().cast::<c_void>(),
                flags.len() as i32,
            ),
            2
        );
        assert_eq!(flags, [1, 0]);
    }
}

#[test]
fn c_api_trigger_event_by_index_validates_and_dispatches() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_trigger_event_by_index(instance.0, 99, std::ptr::null(), 0),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert_eq!(*sample, 0.0);
        }

        assert_eq!(
            onda_trigger_event_by_index(instance.0, 0, std::ptr::null(), 0),
            -2
        );

        let payload = 0.625_f32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 0.625).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_slice_events_report_dynamic_payload_and_dispatch() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { gate = 0.0 }
events {
  load(values: f32[]) {
    gate = values[0] + f32(values.len())
  }
}
sample { out1 = gate }
"#,
        );

        let load = CString::new("load").expect("valid cstr");
        let event_idx = onda_event_index(program.0, load.as_ptr());
        assert_eq!(event_idx, 0);
        assert_eq!(onda_event_payload_bytes(program.0, event_idx), -1);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        let bad_payload = 2_i32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                event_idx,
                bad_payload.as_ptr().cast::<c_void>(),
                bad_payload.len() as i32,
            ),
            -2
        );

        let mut payload = Vec::new();
        payload.extend_from_slice(&(2_i32).to_ne_bytes());
        payload.extend_from_slice(&0.25_f32.to_ne_bytes());
        payload.extend_from_slice(&0.75_f32.to_ne_bytes());
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                event_idx,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in out {
            assert!((sample - 2.25).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_reset_instance_state_restores_initial_state() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        let payload = 0.5_f32.to_ne_bytes();
        assert_eq!(
            onda_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 0.5).abs() < 1e-6);
        }

        assert_eq!(onda_reset_instance_state(instance.0), 0);
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 0.0).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_custom_allocator_instance_uses_allocator_and_frees_it() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { phase = 0.25 }
sample {
  phase = phase + 0.25
  out1 = phase
}
"#,
        );

        let mut stats = AllocStats::default();
        let allocator = onda_allocator_t {
            context: (&mut stats as *mut AllocStats).cast::<c_void>(),
            alloc: Some(test_alloc),
            free: Some(test_free),
        };
        let mut diag = empty_diag();
        let instance = onda_instance_create_with_allocator(program.0, 0, 1, &allocator, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        assert!(stats.allocs > 0);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_validate_bindings(instance), 0);
        assert_eq!(onda_process_unchecked(instance), 0);
        assert!((out[0] - 0.5).abs() < 1e-6);

        onda_instance_destroy(instance);
        assert_eq!(stats.live, 0);
        assert_eq!(stats.allocs, stats.frees);
    }
}

#[test]
fn c_api_state_manifest_and_snapshot_restore_work() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init { phase = 0.0 }
sample {
  phase = phase + 1.0
  out1 = phase
}
"#,
        );

        assert_eq!(onda_state_count(program.0), 1);
        assert_eq!(
            CStr::from_ptr(onda_state_name(program.0, 0)).to_string_lossy(),
            "phase"
        );
        assert_eq!(
            CStr::from_ptr(onda_state_type(program.0, 0)).to_string_lossy(),
            "f32"
        );
        assert_eq!(onda_state_elem_type(program.0, 0), 0);
        assert_eq!(onda_state_array_len(program.0, 0), 1);
        assert_eq!(onda_state_type_bytes(program.0, 0), 4);
        assert_eq!(onda_state_byte_offset(program.0, 0), 0);
        assert!(onda_state_total_bytes(program.0) >= 4);

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[frames as usize - 1], 512.0);

        let state_bytes = onda_instance_state_bytes(instance.0);
        assert_eq!(state_bytes, onda_state_total_bytes(program.0));
        let mut snapshot = vec![0_u8; state_bytes as usize];
        assert_eq!(
            onda_instance_snapshot_state(
                instance.0,
                snapshot.as_mut_ptr().cast::<c_void>(),
                snapshot.len() as i32,
            ),
            state_bytes
        );

        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 513.0);
        assert_eq!(out[frames as usize - 1], 1024.0);

        assert_eq!(
            onda_instance_restore_state(
                instance.0,
                snapshot.as_ptr().cast::<c_void>(),
                snapshot.len() as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        assert_eq!(out[0], 513.0);
        assert_eq!(out[frames as usize - 1], 1024.0);
    }
}

#[test]
fn c_api_compile_options_block_size_controls_runtime_block_size() {
    unsafe {
        let src = CString::new(
            r#"
outs { out1 }
sample { out1 = 0.25 }
"#,
        )
        .expect("source contains no NUL bytes");

        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48_000.0,
            block_size: 128,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let program = ProgramHandle(program);

        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; 128];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, 128), 0);
        for sample in out {
            assert!((sample - 0.25).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_process_checked_accepts_sub_block_frame_counts() {
    unsafe {
        let frames = 512_i32;
        let sub_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
sample { out1 = 0.25 }
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![-1.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, sub_frames), 0);
        for sample in &out[..sub_frames as usize] {
            assert!((*sample - 0.25).abs() < 1e-6);
        }
        for sample in &out[sub_frames as usize..] {
            assert_eq!(*sample, -1.0);
        }
    }
}

#[test]
fn c_api_process_checked_segment_gates_block_hooks() {
    unsafe {
        let frames = 512_i32;
        let segment_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_process_checked_segment(instance.0, 0, segment_frames, ONDA_PROCESS_BEGIN_BLOCK),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                segment_frames,
                segment_frames,
                ONDA_PROCESS_END_BLOCK
            ),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!(sample.abs() < 1e-6);
        }
        for sample in &out[segment_frames as usize..(segment_frames * 2) as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_checked_segment(instance.0, 0, frames, ONDA_PROCESS_FULL_BLOCK),
            0
        );
        for sample in &out {
            assert!((*sample - 201.0).abs() < 1e-6);
        }

        assert_eq!(
            onda_process_checked_segment(instance.0, 0, frames, 1 << 8),
            -2
        );
        assert_eq!(
            onda_process_checked_segment(instance.0, frames - 1, 2, ONDA_PROCESS_END_BLOCK),
            -2
        );
    }
}

#[test]
fn c_api_process_checked_segment_uses_start_frame_for_io() {
    unsafe {
        let frames = 512_i32;
        let start_frame = 128_i32;
        let segment_frames = 2_i32;
        let program = compile_program(
            r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1 + 10.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 1, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let input = (0..frames).map(|frame| frame as f32).collect::<Vec<_>>();
        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_input(
                instance.0,
                0,
                input.as_ptr().cast::<c_void>(),
                (input.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                start_frame,
                segment_frames,
                ONDA_PROCESS_FULL_BLOCK
            ),
            0
        );
        assert!(out[..start_frame as usize]
            .iter()
            .all(|sample| sample.abs() < 1e-6));
        assert_eq!(out[start_frame as usize], 138.0);
        assert_eq!(out[start_frame as usize + 1], 139.0);
        assert!(out[start_frame as usize + segment_frames as usize..]
            .iter()
            .all(|sample| sample.abs() < 1e-6));
    }
}

#[test]
fn c_api_process_unchecked_segment_gates_block_hooks() {
    unsafe {
        let frames = 512_i32;
        let segment_frames = 128_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_prepare_unchecked_process(instance.0), 0);

        assert_eq!(
            onda_process_unchecked_segment(instance.0, 0, segment_frames, ONDA_PROCESS_BEGIN_BLOCK),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_unchecked_segment(
                instance.0,
                segment_frames,
                segment_frames,
                ONDA_PROCESS_END_BLOCK
            ),
            0
        );
        for sample in &out[..segment_frames as usize] {
            assert!(sample.abs() < 1e-6);
        }
        for sample in &out[segment_frames as usize..(segment_frames * 2) as usize] {
            assert!((*sample - 100.0).abs() < 1e-6);
        }

        out.fill(0.0);
        assert_eq!(
            onda_process_unchecked_segment(instance.0, 0, frames, ONDA_PROCESS_FULL_BLOCK),
            0
        );
        for sample in &out {
            assert!((*sample - 201.0).abs() < 1e-6);
        }
    }
}

#[test]
fn c_api_compile_options_sample_rate_controls_builtin_sample_rate() {
    unsafe {
        let src = CString::new(
            r#"
outs { out1 }
sample { out1 = SAMPLE_RATE }
"#,
        )
        .expect("source contains no NUL bytes");

        let sample_rate = 12_345.0_f32;
        let block_size = 64_i32;
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate,
            block_size,
        };
        let mut diag = empty_diag();
        let program = onda_compile(src.as_ptr(), &options, &mut diag);
        assert!(
            !program.is_null(),
            "compile failed: {}",
            diag_message(&diag)
        );
        let program = ProgramHandle(program);

        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; block_size as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, block_size), 0);
        for sample in out {
            assert!((sample - sample_rate).abs() < 1e-3);
        }
    }
}

#[test]
fn c_api_buffer_may_write_metadata_tracks_reachable_writes() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  write_buf: buffer[f32]
  read_buf: buffer[f32]
}
def touch(buf: buffer[f32]):
  unsafe_write(buf, 0, 0.75)
proc Writer:
  ins:
    in1
  buffers:
    b: buffer[f32]
  outs:
    out1
  sample:
    touch(b)
    out1 = in1
init:
  w = Writer(b = write_buf)
sample:
  out1 = w(read_buf[0])
"#,
        );

        let write_name = CString::new("write_buf").expect("valid cstr");
        let read_name = CString::new("read_buf").expect("valid cstr");
        let write_idx = onda_buffer_index(program.0, write_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(write_idx >= 0);
        assert!(read_idx >= 0);

        assert_eq!(onda_buffer_may_write(program.0, write_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
        assert_eq!(onda_buffer_may_write(program.0, -1), -1);
        assert_eq!(onda_buffer_may_write(std::ptr::null(), write_idx), -1);
    }
}

#[test]
fn c_api_buffer_may_write_marks_conditional_and_multichannel_writes() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  branch_buf: buffer[f32]
  stereo_buf: buffer[f32[2]]
  read_buf: buffer[f32]
}
def write_if(buf: buffer[f32]):
  if (1 > 0):
    unsafe_write(buf, 0, 1.0)
proc StereoWriter:
  ins:
    in1
  outs:
    out1
  buffers:
    b: buffer[f32[2]]
  sample:
    b[0][0] = 0.1
    out1 = in1
init:
  sw = StereoWriter(b = stereo_buf)
sample:
  write_if(branch_buf)
  out1 = sw(read_buf[0])
"#,
        );

        let branch_name = CString::new("branch_buf").expect("valid cstr");
        let stereo_name = CString::new("stereo_buf").expect("valid cstr");
        let read_name = CString::new("read_buf").expect("valid cstr");
        let branch_idx = onda_buffer_index(program.0, branch_name.as_ptr());
        let stereo_idx = onda_buffer_index(program.0, stereo_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(branch_idx >= 0);
        assert!(stereo_idx >= 0);
        assert!(read_idx >= 0);

        assert_eq!(onda_buffer_may_write(program.0, branch_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, stereo_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
    }
}

#[test]
fn c_api_buffer_may_write_is_true_when_buffer_is_read_and_written() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  rw_buf: buffer[f32]
}
sample:
  x = rw_buf[0]
  unsafe_write(rw_buf, 0, x)
  out1 = x
"#,
        );

        let rw_name = CString::new("rw_buf").expect("valid cstr");
        let rw_idx = onda_buffer_index(program.0, rw_name.as_ptr());
        assert!(rw_idx >= 0);
        assert_eq!(onda_buffer_may_write(program.0, rw_idx), 1);
    }
}

#[test]
fn c_api_buffer_may_write_tracks_method_style_buffer_calls() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
buffers {
  method_write_buf: buffer[f32]
  method_read_buf: buffer[f32]
}
def touch(buf: buffer[f32]):
  buf.unsafe_write(0, 0.5)
proc Writer:
  ins:
    in1
  outs:
    out1
  buffers:
    b: buffer[f32]
  sample:
    touch(b)
    out1 = in1
init:
  w = Writer(b = method_write_buf)
sample:
  out1 = w(method_read_buf.unsafe_read(0))
"#,
        );

        let write_name = CString::new("method_write_buf").expect("valid cstr");
        let read_name = CString::new("method_read_buf").expect("valid cstr");
        let write_idx = onda_buffer_index(program.0, write_name.as_ptr());
        let read_idx = onda_buffer_index(program.0, read_name.as_ptr());
        assert!(write_idx >= 0);
        assert!(read_idx >= 0);
        assert_eq!(onda_buffer_may_write(program.0, write_idx), 1);
        assert_eq!(onda_buffer_may_write(program.0, read_idx), 0);
    }
}

#[test]
fn c_api_infers_local_from_primitive_array_index_read_in_sample() {
    unsafe {
        let _program = compile_program(
            r#"
ins { in1 }
outs { out1 }
params {
  delayTime = 0.2 {0.01, 0.5}
}
init {
  line: f32[SR]
  writePos: i32 = 0
  lineLen: i32 = i32(SR)
}
sample {
  delaySamples: i32 = i32(delayTime * SR)
  if (delaySamples < 1) { delaySamples = 1 }
  if (delaySamples >= lineLen) { delaySamples = lineLen - 1 }

  readPos: i32 = writePos - delaySamples
  if (readPos < 0) { readPos = readPos + lineLen }

  delayed = line[readPos]
  line[writePos] = in1
  writePos = writePos + 1
  if (writePos >= lineLen) { writePos = 0 }
  out1 = delayed
}
"#,
        );
    }
}

#[test]
fn c_api_pi_and_two_pi_use_f64_precision_constants() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs {
  out1
  out2
}
sample {
  out1 = f32(PI - f64(f32(PI)))
  out2 = f32(TWO_PI - f64(f32(TWO_PI)))
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 2, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out1 = vec![0.0_f32; frames as usize];
        let mut out2 = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out1.as_mut_ptr().cast::<c_void>(),
                (out1.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(
            onda_bind_output(
                instance.0,
                1,
                out2.as_mut_ptr().cast::<c_void>(),
                (out2.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);

        for sample in out1 {
            assert!(sample.abs() > 1e-9);
        }
        for sample in out2 {
            assert!(sample.abs() > 1e-9);
        }
    }
}

#[test]
fn c_api_block_size_builtin_is_i32_typed() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
outs { out1 }
init {
  bs: i32 = BLOCK_SIZE
}
sample {
  out1 = f32(bs)
}
"#,
        );

        let mut diag = empty_diag();
        let instance = onda_instance_create(program.0, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );
        assert_eq!(onda_process_checked(instance.0, frames), 0);
        for sample in out {
            assert!((sample - 512.0).abs() < 1e-6);
        }
    }
}
