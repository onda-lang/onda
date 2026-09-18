use super::*;

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_by_index(
    instance: *mut onda_instance,
    index: i32,
    value_ptr: *const c_void,
    value_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || value_bytes < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if value_bytes > 0 && value_ptr.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let bytes = if value_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(value_ptr.cast::<u8>(), value_bytes as usize)
    };
    match set_param_by_index(&mut (*instance).inner, index as usize, bytes) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_element_by_index(
    instance: *mut onda_instance,
    index: i32,
    element: i32,
    value_ptr: *const c_void,
    value_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || element < 0 || value_bytes < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if value_bytes > 0 && value_ptr.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let bytes = if value_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(value_ptr.cast::<u8>(), value_bytes as usize)
    };
    match runtime_set_param_element_by_index(
        &mut (*instance).inner,
        index as usize,
        element as usize,
        bytes,
    ) {
        Ok(()) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_plain_f64(
    instance: *mut onda_instance,
    index: i32,
    plain: f64,
) -> i32 {
    if instance.is_null() || index < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match runtime_set_param_plain_f64(&mut (*instance).inner, index as usize, plain) {
        Ok(()) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_element_plain_f64(
    instance: *mut onda_instance,
    index: i32,
    element: i32,
    plain: f64,
) -> i32 {
    if instance.is_null() || index < 0 || element < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match runtime_set_param_element_plain_f64(
        &mut (*instance).inner,
        index as usize,
        element as usize,
        plain,
    ) {
        Ok(()) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_normalized(
    instance: *mut onda_instance,
    index: i32,
    normalized: f64,
) -> i32 {
    if instance.is_null() || index < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match runtime_set_param_normalized(&mut (*instance).inner, index as usize, normalized) {
        Ok(()) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_element_normalized(
    instance: *mut onda_instance,
    index: i32,
    element: i32,
    normalized: f64,
) -> i32 {
    if instance.is_null() || index < 0 || element < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match runtime_set_param_element_normalized(
        &mut (*instance).inner,
        index as usize,
        element as usize,
        normalized,
    ) {
        Ok(()) => 0,
        Err(_) => ONDA_API_ERROR_PARAMETER_REJECTED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_read_bytes(
    instance: *const onda_instance,
    index: i32,
    out_bytes: *mut c_void,
    out_capacity: i32,
) -> i32 {
    if instance.is_null() || index < 0 || out_capacity < 0 {
        return -1;
    }
    let Some(required_usize) = (*instance).inner.control_output_type_bytes(index as usize) else {
        return -1;
    };
    let required = match i32::try_from(required_usize) {
        Ok(value) => value,
        Err(_) => return -1,
    };
    if out_bytes.is_null() || out_capacity < required {
        return required;
    }
    let out_slice = std::slice::from_raw_parts_mut(out_bytes.cast::<u8>(), required as usize);
    match read_control_output_bytes(&(*instance).inner, index as usize, out_slice) {
        Ok(_) => required,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_bind_input(
    instance: *mut onda_instance,
    index: i32,
    src_ptr: *const c_void,
    src_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || src_bytes < 0 || (src_ptr.is_null() && src_bytes > 0) {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let ptr = src_ptr.cast::<u8>();
    let bytes = src_bytes as usize;
    match unsafe { bind_input(&mut (*instance).inner, index as usize, ptr, bytes) } {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_bind_output(
    instance: *mut onda_instance,
    index: i32,
    dst_ptr: *mut c_void,
    dst_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || dst_bytes < 0 || (dst_ptr.is_null() && dst_bytes > 0) {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let ptr = dst_ptr.cast::<u8>();
    let bytes = dst_bytes as usize;
    match unsafe { bind_output(&mut (*instance).inner, index as usize, ptr, bytes) } {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_bind_buffer(
    instance: *mut onda_instance,
    index: i32,
    ptr: *mut c_void,
    frames: i32,
    channels: i32,
    sample_rate: f32,
    elem_type: i32,
) -> i32 {
    if instance.is_null() || index < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let Some(elem_ty) = primitive_type_from_i32(elem_type) else {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    let unbinds = sample_rate == 0.0 || (ptr.is_null() && frames == 0 && channels == 0);
    let (ptr, frames, channels) = if unbinds {
        (std::ptr::null_mut(), 0, 0)
    } else {
        if ptr.is_null()
            || frames <= 0
            || channels <= 0
            || !sample_rate.is_finite()
            || sample_rate <= 0.0
        {
            return ONDA_API_ERROR_INVALID_ARGUMENT;
        }
        (ptr.cast::<u8>(), frames as usize, channels as usize)
    };
    match unsafe {
        bind_buffer(
            &mut (*instance).inner,
            index as usize,
            ptr,
            frames,
            channels,
            sample_rate,
            elem_ty,
        )
    } {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_reset_buffer_to_project_default(
    instance: *mut onda_instance,
    index: i32,
) -> i32 {
    if instance.is_null() || index < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let instance = &mut *instance;
    let program = Arc::clone(&instance.program);
    let Some(defaults) = &program.project_defaults else {
        return ONDA_API_ERROR_VALIDATION_FAILED;
    };
    match bind_project_default(&mut instance.inner, defaults, index as usize) {
        Ok(true) => 0,
        Ok(false) | Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_checked(
    instance: *mut onda_instance,
    frames: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || frames < 0 {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    execution_status_to_c(with_runtime_execution_output(output, |output| {
        process_checked_with_status(&mut (*instance).inner, frames as usize, output)
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_checked_segment(
    instance: *mut onda_instance,
    start_frame: i32,
    frames: i32,
    flags: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || start_frame < 0 || frames < 0 || !process_flags_are_valid(flags) {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    execution_status_to_c(with_runtime_execution_output(output, |output| {
        process_checked_segment_with_status(
            &mut (*instance).inner,
            start_frame as usize,
            frames as usize,
            flags as u32,
            output,
        )
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_init_checked(
    instance: *mut onda_instance,
    mode: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let Some(mode) = init_mode_from_c(mode) else {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    execution_status_to_c(with_runtime_execution_output(output, |output| {
        runtime_init_checked_with_status(&mut (*instance).inner, mode, output)
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_init_unchecked(
    instance: *mut onda_instance,
    mode: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let Some(mode) = init_mode_from_c(mode) else {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        runtime_init_unchecked(&mut (*instance).inner, mode, output)
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_state_bytes(instance: *const onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*instance).inner.state_size_bytes())
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_snapshot_state(
    instance: *const onda_instance,
    out_bytes: *mut c_void,
    out_capacity: i32,
) -> i32 {
    if instance.is_null() || out_capacity < 0 {
        return -1;
    }
    let required = match i32::try_from((*instance).inner.state_size_bytes()) {
        Ok(value) => value,
        Err(_) => return -1,
    };
    if out_bytes.is_null() || out_capacity < required {
        return required;
    }
    let destination = std::slice::from_raw_parts_mut(out_bytes.cast::<u8>(), required as usize);
    if (*instance)
        .inner
        .write_snapshot_state_bytes(destination)
        .is_err()
    {
        return ONDA_API_ERROR_VALIDATION_FAILED;
    }
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_restore_state(
    instance: *mut onda_instance,
    bytes: *const c_void,
    byte_count: i32,
) -> i32 {
    if instance.is_null() || byte_count < 0 {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if byte_count > 0 && bytes.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let snapshot = if byte_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(bytes.cast::<u8>(), byte_count as usize)
    };
    execution_status_to_c((*instance).inner.restore_state_bytes_with_status(snapshot))
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_bindings(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match validate_bindings(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_inputs(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match validate_inputs(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_outputs(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match validate_outputs(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_buffers(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match validate_buffers(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_unchecked(
    instance: *mut onda_instance,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        process_unchecked(&mut (*instance).inner, output)
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_prepare_unchecked_process(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    match prepare_unchecked_process(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => ONDA_API_ERROR_VALIDATION_FAILED,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_unchecked_segment(
    instance: *mut onda_instance,
    start_frame: i32,
    frames: i32,
    flags: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || start_frame < 0 || frames < 0 || !process_flags_are_valid(flags) {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        process_unchecked_segment(
            &mut (*instance).inner,
            start_frame as usize,
            frames as usize,
            flags as u32,
            output,
        )
    }))
}
