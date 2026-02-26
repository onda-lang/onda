use std::ffi::{c_void, CStr, CString};

use omni_api::*;

fn diag_message(diag: &omni_diag_t) -> String {
    if diag.message.is_null() {
        return "<null>".to_owned();
    }
    unsafe { CStr::from_ptr(diag.message).to_string_lossy().into_owned() }
}

struct ProgramHandle(*mut omni_program);

impl Drop for ProgramHandle {
    fn drop(&mut self) {
        unsafe {
            omni_program_destroy(self.0);
        }
    }
}

struct InstanceHandle(*mut omni_instance);

impl Drop for InstanceHandle {
    fn drop(&mut self) {
        unsafe {
            omni_instance_destroy(self.0);
        }
    }
}

unsafe fn compile_program(src: &str) -> ProgramHandle {
    let src_c = CString::new(src).expect("source contains no NUL bytes");
    let mut diag = omni_diag_t {
        code: 0,
        line: 0,
        column: 0,
        message: std::ptr::null(),
        file: std::ptr::null(),
        trace: std::ptr::null(),
    };
    let program = omni_compile(src_c.as_ptr(), 0, &mut diag);
    assert!(
        !program.is_null(),
        "compile failed: {}",
        diag_message(&diag)
    );
    ProgramHandle(program)
}

#[test]
fn c_api_event_metadata_queries_work() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
events {
  note_on(note: i32, vel: i32) {
    amp = f32(vel) / 127.0
  }
  set_curve(values: f32[2]) {
    amp = values[0] + values[1]
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        assert_eq!(omni_event_count(program.0), 2);
        let name0 = CStr::from_ptr(omni_event_name(program.0, 0))
            .to_string_lossy()
            .into_owned();
        let name1 = CStr::from_ptr(omni_event_name(program.0, 1))
            .to_string_lossy()
            .into_owned();
        assert_eq!(name0, "note_on");
        assert_eq!(name1, "set_curve");

        let note_on = CString::new("note_on").expect("valid cstr");
        let set_curve = CString::new("set_curve").expect("valid cstr");
        assert_eq!(omni_event_index(program.0, note_on.as_ptr()), 0);
        assert_eq!(omni_event_index(program.0, set_curve.as_ptr()), 1);
        assert_eq!(omni_event_payload_bytes(program.0, 0), 8);
        assert_eq!(omni_event_payload_bytes(program.0, 1), 8);
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

        let mut diag = omni_diag_t {
            code: 0,
            line: 0,
            column: 0,
            message: std::ptr::null(),
            file: std::ptr::null(),
            trace: std::ptr::null(),
        };
        let instance = omni_instance_create(program.0, 48_000.0, frames, 0, 1, &mut diag);
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        let mut out = vec![0.0_f32; frames as usize];
        assert_eq!(
            omni_bind_output(
                instance.0,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                (out.len() * std::mem::size_of::<f32>()) as i32,
            ),
            0
        );

        assert_eq!(
            omni_trigger_event_by_index(instance.0, 99, std::ptr::null(), 0),
            0
        );
        assert_eq!(omni_process_bound(instance.0, frames), 0);
        for sample in &out {
            assert_eq!(*sample, 0.0);
        }

        assert_eq!(
            omni_trigger_event_by_index(instance.0, 0, std::ptr::null(), 0),
            -2
        );

        let payload = 0.625_f32.to_ne_bytes();
        assert_eq!(
            omni_trigger_event_by_index(
                instance.0,
                0,
                payload.as_ptr().cast::<c_void>(),
                payload.len() as i32,
            ),
            0
        );
        assert_eq!(omni_process_bound(instance.0, frames), 0);
        for sample in &out {
            assert!((*sample - 0.625).abs() < 1e-6);
        }
    }
}
