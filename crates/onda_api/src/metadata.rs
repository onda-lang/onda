use super::*;

#[no_mangle]
pub unsafe extern "C" fn onda_input_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.input_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.output_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.control_output_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.param_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.buffer_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_array_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.buffer_arrays().len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.event_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.delegate_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_file_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    let program = &*program;
    saturating_usize_to_i32(program.inner.source_file_paths.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_file_path(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    let program = &*program;
    cstr_ptr_at(&program.inner.source_file_paths, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_log_site_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    let program = &*program;
    saturating_usize_to_i32(program.inner.log_site_labels.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_log_site_info(
    program: *const onda_program,
    index: i32,
    out_info: *mut onda_log_site_info_t,
) -> i32 {
    if out_info.is_null() {
        return -1;
    }
    ptr::write(
        out_info,
        onda_log_site_info_t {
            label: ptr::null(),
            source: onda_source_span_t {
                file_index: -1,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
            },
            lexical_owner: ptr::null(),
            declaration: ptr::null(),
            argument_types: ptr::null(),
            argument_count: 0,
            payload_size_bytes: 0,
        },
    );
    if program.is_null() || index < 0 {
        return -1;
    }
    let program = &(*program).inner;
    let index = index as usize;
    let Some(site) = program.jit.mir().log_sites.get(index) else {
        return -1;
    };
    let Some(label) = program.log_site_labels.get(index) else {
        return -1;
    };
    let Some(owner) = program.log_site_owners.get(index) else {
        return -1;
    };
    let Some(declaration) = program.log_site_declarations.get(index) else {
        return -1;
    };
    let Some(argument_types) = program.log_site_argument_types.get(index) else {
        return -1;
    };
    let Ok(argument_count) = u32::try_from(argument_types.len()) else {
        return -1;
    };
    let file_index = site
        .source
        .file
        .and_then(|file| i32::try_from(file.index()).ok())
        .unwrap_or(-1);
    ptr::write(
        out_info,
        onda_log_site_info_t {
            label: label.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            source: onda_source_span_t {
                file_index,
                line: site.source.line,
                column: site.source.column,
                end_line: site.source.end_line,
                end_column: site.source.end_column,
            },
            lexical_owner: owner.as_ptr(),
            declaration: declaration
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr()),
            argument_types: if argument_types.is_empty() {
                ptr::null()
            } else {
                argument_types.as_ptr()
            },
            argument_count,
            payload_size_bytes: site.payload_size,
        },
    );
    0
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.state_count())
}

fn cstr_ptr_at(values: &[CString], index: i32) -> *const c_char {
    if index < 0 {
        return ptr::null();
    }
    values
        .get(index as usize)
        .map_or(ptr::null(), |v| v.as_ptr())
}

fn index_from_name<F>(name: *const c_char, resolver: F) -> i32
where
    F: FnOnce(&str) -> Option<usize>,
{
    if name.is_null() {
        return -1;
    }
    let key = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(v) => v,
        Err(_) => return -1,
    };
    resolver(key)
        .and_then(|idx| i32::try_from(idx).ok())
        .unwrap_or(-1)
}

fn bytes_from_index<F>(index: i32, resolver: F) -> i32
where
    F: FnOnce(usize) -> Option<usize>,
{
    if index < 0 {
        return -1;
    }
    resolver(index as usize)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(-1)
}

fn bool_flag_from_index<F>(index: i32, resolver: F) -> i32
where
    F: FnOnce(usize) -> Option<bool>,
{
    if index < 0 {
        return -1;
    }
    match resolver(index as usize) {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

fn f64_from_index_or_nan<F>(index: i32, resolver: F) -> f64
where
    F: FnOnce(usize) -> Option<f64>,
{
    if index < 0 {
        return f64::NAN;
    }
    resolver(index as usize).unwrap_or(f64::NAN)
}

pub(super) fn primitive_type_from_i32(value: i32) -> Option<PrimitiveType> {
    match value {
        0 => Some(PrimitiveType::F32),
        1 => Some(PrimitiveType::F64),
        2 => Some(PrimitiveType::I32),
        3 => Some(PrimitiveType::I64),
        4 => Some(PrimitiveType::Bool),
        _ => None,
    }
}

pub(super) fn primitive_type_to_i32(value: PrimitiveType) -> i32 {
    match value {
        PrimitiveType::F32 => 0,
        PrimitiveType::F64 => 1,
        PrimitiveType::I32 => 2,
        PrimitiveType::I64 => 3,
        PrimitiveType::Bool => 4,
    }
}

pub(super) fn primitive_type_bytes(value: PrimitiveType) -> usize {
    match value {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn decl_buffer_channels_kind_to_i32(channels: DeclaredBufferChannels) -> i32 {
    match channels {
        DeclaredBufferChannels::Mono => 0,
        DeclaredBufferChannels::Static(_) => 1,
        DeclaredBufferChannels::Dynamic => 2,
    }
}

fn usize_from_index<F>(index: i32, resolver: F) -> i32
where
    F: FnOnce(usize) -> Option<usize>,
{
    bytes_from_index(index, resolver)
}

fn i32_from_index_or<F>(index: i32, fallback: i32, resolver: F) -> i32
where
    F: FnOnce(usize) -> Option<i32>,
{
    if index < 0 {
        return fallback;
    }
    resolver(index as usize).unwrap_or(fallback)
}

unsafe fn event_param_descriptor<'a>(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> Option<&'a DeclaredEventParam> {
    if program.is_null() || event_index < 0 || param_index < 0 {
        return None;
    }
    (&*program)
        .inner
        .jit
        .event_descriptor(event_index as usize)
        .and_then(|event| event.params().get(param_index as usize))
}

unsafe fn delegate_param_descriptor<'a>(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> Option<&'a DeclaredEventParam> {
    if program.is_null() || delegate_index < 0 || param_index < 0 {
        return None;
    }
    (&*program)
        .inner
        .jit
        .delegate_descriptor(delegate_index as usize)
        .and_then(|delegate| delegate.params().get(param_index as usize))
}

unsafe fn state_descriptor<'a>(
    program: *const onda_program,
    index: i32,
) -> Option<&'a DeclaredState> {
    if program.is_null() || index < 0 {
        return None;
    }
    (&*program).inner.jit.state_entries().get(index as usize)
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.input_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.output_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.control_output_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.param_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.buffer_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_array_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.buffer_array_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_array_first(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() || index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .buffer_arrays()
        .get(index as usize)
        .map_or(-1, |array| saturating_usize_to_i32(array.first()))
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() || index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .buffer_arrays()
        .get(index as usize)
        .map_or(-1, |array| saturating_usize_to_i32(array.len()))
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.event_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.delegate_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.state_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_count(
    program: *const onda_program,
    event_index: i32,
) -> i32 {
    if program.is_null() || event_index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .event_descriptor(event_index as usize)
        .and_then(|event| i32::try_from(event.params().len()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_name(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> *const c_char {
    if program.is_null() || event_index < 0 || param_index < 0 {
        return ptr::null();
    }
    let event_param_names = &*ptr::addr_of!((&*program).inner.event_param_names);
    event_param_names
        .get(event_index as usize)
        .map_or(ptr::null(), |names| cstr_ptr_at(names, param_index))
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_count(
    program: *const onda_program,
    delegate_index: i32,
) -> i32 {
    if program.is_null() || delegate_index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .delegate_descriptor(delegate_index as usize)
        .and_then(|delegate| i32::try_from(delegate.params().len()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_name(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> *const c_char {
    if program.is_null() || delegate_index < 0 || param_index < 0 {
        return ptr::null();
    }
    let names = &*ptr::addr_of!((&*program).inner.delegate_param_names);
    names
        .get(delegate_index as usize)
        .map_or(ptr::null(), |names| cstr_ptr_at(names, param_index))
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.input_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.output_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.control_output_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.param_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.buffer_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.event_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (&*program).inner.jit.delegate_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.input_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.output_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.control_output_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.param_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.buffer_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(&*program).inner.state_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (&*program).inner.jit.input_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (&*program).inner.jit.output_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_type_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| {
        (&*program).inner.jit.control_output_type_bytes(idx)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (&*program).inner.jit.param_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (&*program).inner.jit.state_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_payload_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    /* Dynamic slice-event payloads also report -1 here. */
    bytes_from_index(index, |idx| (&*program).inner.jit.event_payload_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_payload_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| {
        (&*program).inner.jit.delegate_payload_bytes(idx)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_payload_min_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| {
        (&*program).inner.jit.delegate_payload_min_bytes(idx)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_record_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| {
        (&*program).inner.jit.delegate_record_bytes(idx)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_record_min_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| {
        (&*program).inner.jit.delegate_record_min_bytes(idx)
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_elem_type(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .map(|param| primitive_type_to_i32(param.elem_ty()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_array_len(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .and_then(|param| i32::try_from(param.array_len()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_is_array(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .map(|param| i32::from(param.is_array()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_is_slice(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .map(|param| if param.is_slice() { 1 } else { 0 })
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_offset_bytes(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .and_then(DeclaredEventParam::byte_offset)
        .and_then(|offset| i32::try_from(offset).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_elem_type(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> i32 {
    delegate_param_descriptor(program, delegate_index, param_index)
        .map(|param| primitive_type_to_i32(param.elem_ty()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_array_len(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> i32 {
    delegate_param_descriptor(program, delegate_index, param_index)
        .and_then(|param| i32::try_from(param.array_len()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_is_array(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> i32 {
    delegate_param_descriptor(program, delegate_index, param_index)
        .map(|param| i32::from(param.is_array()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_is_slice(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> i32 {
    delegate_param_descriptor(program, delegate_index, param_index)
        .map(|param| i32::from(param.is_slice()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_delegate_param_offset_bytes(
    program: *const onda_program,
    delegate_index: i32,
    param_index: i32,
) -> i32 {
    delegate_param_descriptor(program, delegate_index, param_index)
        .and_then(DeclaredEventParam::byte_offset)
        .and_then(|offset| i32::try_from(offset).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_has_default(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
) -> i32 {
    event_param_descriptor(program, event_index, param_index)
        .map(|param| if param.has_default() { 1 } else { 0 })
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_default_bytes(
    program: *const onda_program,
    event_index: i32,
    param_index: i32,
    out_bytes: *mut c_void,
    out_capacity: i32,
) -> i32 {
    let Some(param) = event_param_descriptor(program, event_index, param_index) else {
        return -1;
    };
    let Some(default_bytes) = param.default_bytes() else {
        return 0;
    };
    let required = match i32::try_from(default_bytes.len()) {
        Ok(value) => value,
        Err(_) => return -1,
    };
    if out_capacity < 0 {
        return -1;
    }
    if out_bytes.is_null() || out_capacity < required {
        return required;
    }
    ptr::copy_nonoverlapping(
        default_bytes.as_ptr(),
        out_bytes.cast::<u8>(),
        default_bytes.len(),
    );
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_elem_type(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .buffers()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_elem_type_bytes(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .buffers()
            .get(idx)
            .map(|d| primitive_type_bytes(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_channels_kind(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .buffers()
            .get(idx)
            .map(|d| decl_buffer_channels_kind_to_i32(d.channels()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_channels_static(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .buffers()
            .get(idx)
            .and_then(|d| match d.channels() {
                DeclaredBufferChannels::Mono => Some(1),
                DeclaredBufferChannels::Static(ch) => i32::try_from(ch).ok(),
                DeclaredBufferChannels::Dynamic => None,
            })
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_may_write(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .buffers()
            .get(idx)
            .map(|d| if d.may_write() { 1 } else { 0 })
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_elem_type(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_elem_type(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_elem_type(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .control_outputs()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_elem_type(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    i32_from_index_or(index, -1, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_elem_type(program: *const onda_program, index: i32) -> i32 {
    state_descriptor(program, index)
        .map(|d| primitive_type_to_i32(d.elem_ty()))
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_array_len(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .control_outputs()
            .get(idx)
            .map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_array_len(program: *const onda_program, index: i32) -> i32 {
    state_descriptor(program, index)
        .and_then(|d| i32::try_from(d.array_len()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_slot_offset(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .control_outputs()
            .get(idx)
            .map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_control_output_byte_offset(
    program: *const onda_program,
    index: i32,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .control_outputs()
            .get(idx)
            .map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_byte_offset(program: *const onda_program, index: i32) -> i32 {
    state_descriptor(program, index)
        .and_then(|d| i32::try_from(d.byte_offset()).ok())
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_state_total_bytes(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.state_size_bytes())
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.has_default())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.has_default())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| d.has_default())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_default_bytes(
    program: *const onda_program,
    index: i32,
    out_bytes: *mut c_void,
    out_capacity: i32,
) -> i32 {
    if program.is_null() || index < 0 || out_capacity < 0 {
        return -1;
    }
    let Some(param) = (&*program).inner.jit.params().get(index as usize) else {
        return -1;
    };
    let Some(default_bytes) = param.default_bytes() else {
        return 0;
    };
    let required = match i32::try_from(default_bytes.len()) {
        Ok(value) => value,
        Err(_) => return -1,
    };
    if out_bytes.is_null() || out_capacity < required {
        return required;
    }
    ptr::copy_nonoverlapping(
        default_bytes.as_ptr(),
        out_bytes.cast::<u8>(),
        default_bytes.len(),
    );
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_default_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .and_then(|d| d.default_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_default_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .and_then(|d| d.default_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_default_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .and_then(|d| d.default_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_has_range(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_range(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_has_range(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_range_min_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .and_then(|d| d.range_min_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_range_max_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .inputs()
            .get(idx)
            .and_then(|d| d.range_max_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_range_min_f64(
    program: *const onda_program,
    index: i32,
) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .and_then(|d| d.range_min_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_range_max_f64(
    program: *const onda_program,
    index: i32,
) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .outputs()
            .get(idx)
            .and_then(|d| d.range_max_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_range_min_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .and_then(|d| d.range_min_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_range_max_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .and_then(|d| d.range_max_as_f64())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_scale(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() || index < 0 {
        return -1;
    }
    match (&*program)
        .inner
        .jit
        .params()
        .get(index as usize)
        .and_then(|param| param.param_domain())
        .map(|domain| domain.scale_name())
    {
        Some("linear") => 0,
        Some("log") => 1,
        None => -1,
        Some(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_curve(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() || index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .params()
        .get(index as usize)
        .map(|param| {
            i32::from(
                param
                    .param_domain()
                    .is_some_and(|domain| domain.curve().is_some()),
            )
        })
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_curve(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .and_then(|param| param.param_domain())
            .and_then(|domain| domain.curve())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_unit_copy(
    program: *const onda_program,
    index: i32,
    out_bytes: *mut c_char,
    out_capacity: i32,
) -> i32 {
    if program.is_null() || index < 0 || out_capacity < 0 {
        return -1;
    }
    let Some(param) = (&*program).inner.jit.params().get(index as usize) else {
        return -1;
    };
    let Some(unit) = param.param_domain().and_then(|domain| domain.unit()) else {
        return 0;
    };
    let Ok(required) = i32::try_from(unit.len().saturating_add(1)) else {
        return -1;
    };
    if out_bytes.is_null() || out_capacity < required {
        return required;
    }
    ptr::copy_nonoverlapping(unit.as_ptr().cast::<c_char>(), out_bytes, unit.len());
    *out_bytes.add(unit.len()) = 0;
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_step(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    if index < 0 {
        return -1;
    }
    (&*program)
        .inner
        .jit
        .params()
        .get(index as usize)
        .map(|param| {
            i32::from(
                param
                    .param_domain()
                    .is_some_and(|domain| domain.step_count().is_some()),
            )
        })
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_step_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (&*program)
            .inner
            .jit
            .params()
            .get(idx)
            .and_then(|param| param.param_domain())
            .and_then(|domain| domain.step())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_step_count(program: *const onda_program, index: i32) -> u32 {
    if program.is_null() || index < 0 {
        return 0;
    }
    (&*program)
        .inner
        .jit
        .params()
        .get(index as usize)
        .and_then(|param| param.param_domain())
        .and_then(|domain| domain.step_count())
        .unwrap_or(0)
}

#[derive(Clone, Copy)]
enum ParamValueConversion {
    NormalizedToPlain,
    PlainToNormalized,
}

unsafe fn convert_param_value(
    program: *const onda_program,
    index: i32,
    value: f64,
    conversion: ParamValueConversion,
) -> f64 {
    if program.is_null() || index < 0 {
        return f64::NAN;
    }
    let Some(param) = (&*program).inner.jit.params().get(index as usize) else {
        return f64::NAN;
    };
    if !param.is_array() && param.elem_ty() == PrimitiveType::Bool {
        return if value >= 0.5 { 1.0 } else { 0.0 };
    }
    let Some(domain) = param.param_domain() else {
        return f64::NAN;
    };
    match conversion {
        ParamValueConversion::NormalizedToPlain => domain.normalized_to_plain(value),
        ParamValueConversion::PlainToNormalized => domain.plain_to_normalized(value),
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_normalized_to_plain(
    program: *const onda_program,
    index: i32,
    normalized: f64,
) -> f64 {
    convert_param_value(
        program,
        index,
        normalized,
        ParamValueConversion::NormalizedToPlain,
    )
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_plain_to_normalized(
    program: *const onda_program,
    index: i32,
    plain: f64,
) -> f64 {
    convert_param_value(
        program,
        index,
        plain,
        ParamValueConversion::PlainToNormalized,
    )
}
