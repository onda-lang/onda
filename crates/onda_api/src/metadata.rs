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
        PrimitiveType::I32 | PrimitiveType::Enum(_) => 2,
        PrimitiveType::I64 => 3,
        PrimitiveType::Bool => 4,
    }
}

pub(super) fn primitive_type_bytes(value: PrimitiveType) -> usize {
    match value {
        PrimitiveType::F32 | PrimitiveType::I32 | PrimitiveType::Enum(_) => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

pub(super) fn decl_buffer_channels_kind_to_i32(channels: DeclaredBufferChannels) -> i32 {
    match channels {
        DeclaredBufferChannels::Mono => 0,
        DeclaredBufferChannels::Static(_) => 1,
        DeclaredBufferChannels::Dynamic => 2,
    }
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
pub unsafe extern "C" fn onda_state_total_bytes(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((&*program).inner.jit.state_size_bytes())
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
    if param.elem_ty() == PrimitiveType::Bool {
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
