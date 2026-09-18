use super::*;

#[no_mangle]
pub unsafe extern "C" fn onda_trigger_event_by_index_checked(
    instance: *mut onda_instance,
    index: i32,
    payload_ptr: *const c_void,
    payload_bytes: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || index < 0 || payload_bytes < 0 {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if payload_bytes > 0 && payload_ptr.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let payload = if payload_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(payload_ptr.cast::<u8>(), payload_bytes as usize)
    };
    execution_status_to_c(with_runtime_execution_output(output, |output| {
        onda_runtime::trigger_event_by_index_checked_with_status(
            &mut (*instance).inner,
            index as usize,
            payload,
            output,
        )
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_trigger_event_by_index_unchecked(
    instance: *mut onda_instance,
    index: i32,
    payload_ptr: *const c_void,
    payload_bytes: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || index < 0 || payload_bytes < 0 {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if payload_bytes > 0 && payload_ptr.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let payload = if payload_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(payload_ptr.cast::<u8>(), payload_bytes as usize)
    };
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        trigger_event_by_index_unchecked(&mut (*instance).inner, index as usize, payload, output)
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_trigger_event_views_by_index_checked(
    instance: *mut onda_instance,
    index: i32,
    tensors: *const onda_event_tensor_view_t,
    tensor_count: i32,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || index < 0 || tensor_count < 0 {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    if tensor_count > 0 && tensors.is_null() {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let views = if tensor_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(
            tensors.cast::<onda_codegen_llvm::EventTensorView>(),
            tensor_count as usize,
        )
    };
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        trigger_event_views_by_index_checked_with_status(
            &mut (*instance).inner,
            index as usize,
            views,
            output,
        )
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_trigger_event_views_by_index_unchecked(
    instance: *mut onda_instance,
    index: i32,
    tensors: *const onda_event_tensor_view_t,
    output: *mut onda_execution_output_t,
) -> i32 {
    if instance.is_null() || index < 0 {
        reset_c_execution_output(output);
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    execution_status_to_c(with_runtime_execution_output(output, |output| unsafe {
        runtime_trigger_event_views_by_index_unchecked(
            &mut (*instance).inner,
            index as usize,
            tensors.cast::<onda_codegen_llvm::EventTensorView>(),
            output,
        )
    }))
}
