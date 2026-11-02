//! Host-facing enum strings for scalar and fixed-array interfaces. Event and
//! delegate enum leaves are exposed by their existing recursive JSON schemas.
use super::*;

unsafe fn copy_enum_json(
    program: *const onda_program,
    index: i32,
    out_bytes: *mut c_char,
    out_capacity: i32,
    descriptors: fn(&JitProgram) -> &[onda_codegen_llvm::DeclaredIo],
) -> i32 {
    if program.is_null() || index < 0 || out_capacity < 0 {
        return -1;
    }
    let Some(descriptor) = descriptors(&(&*program).inner.jit).get(index as usize) else {
        return -1;
    };
    let Some(enumeration) = descriptor.enumeration() else {
        return 0;
    };
    let Ok(json) = serde_json::to_string(enumeration) else {
        return -1;
    };
    let Ok(required) = i32::try_from(json.len().saturating_add(1)) else {
        return -1;
    };
    if !out_bytes.is_null() && out_capacity >= required {
        ptr::copy_nonoverlapping(json.as_ptr().cast::<c_char>(), out_bytes, json.len());
        *out_bytes.add(json.len()) = 0;
    }
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_enum_json_copy(
    program: *const onda_program,
    index: i32,
    out_bytes: *mut c_char,
    out_capacity: i32,
) -> i32 {
    copy_enum_json(program, index, out_bytes, out_capacity, JitProgram::params)
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_enum_json_copy(
    program: *const onda_program,
    index: i32,
    out_bytes: *mut c_char,
    out_capacity: i32,
) -> i32 {
    copy_enum_json(
        program,
        index,
        out_bytes,
        out_capacity,
        JitProgram::control_outputs,
    )
}
