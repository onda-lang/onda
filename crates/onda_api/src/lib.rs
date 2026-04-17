use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use onda_codegen_llvm::{
    lower_and_jit_with_options, CompileOptions, DeclaredBufferChannels, DeclaredEventParam,
    JitProgram,
};
use onda_frontend::{parse_program, parse_program_file, DiagCode, Diagnostic, PrimitiveType};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_bound, process_unchecked,
    reset_instance_state, set_param_by_index, trigger_event_by_index, validate_bindings,
    validate_buffers, validate_inputs, validate_outputs, Instance, InstanceConfig,
};
use onda_semantics::{analyze_with_options, AnalysisOptions};

#[repr(C)]
pub struct onda_diag_t {
    pub code: i32,
    pub line: i32,
    pub column: i32,
    pub end_line: i32,
    pub end_column: i32,
    pub message: *const c_char,
    pub file: *const c_char,
    pub trace: *const c_char,
}

#[repr(C)]
pub struct onda_compile_options_t {
    pub fast_math: i32,
    pub sample_rate: f32,
    pub block_size: i32,
}

#[allow(non_camel_case_types)]
pub struct onda_program {
    jit: JitProgram,
    input_names: Vec<CString>,
    input_types: Vec<CString>,
    output_names: Vec<CString>,
    output_types: Vec<CString>,
    param_names: Vec<CString>,
    param_types: Vec<CString>,
    buffer_names: Vec<CString>,
    buffer_types: Vec<CString>,
    event_names: Vec<CString>,
    event_param_names: Vec<Vec<CString>>,
}

#[allow(non_camel_case_types)]
pub struct onda_instance {
    inner: Instance,
}

const STATIC_ERR_INVALID_UTF8: &[u8] = b"source string is not valid UTF-8\0";
const STATIC_ERR_NULL_ARG: &[u8] = b"null argument\0";
const STATIC_ERR_INTERNAL: &[u8] = b"internal error\0";

fn build_cstring_cache(strings: Vec<String>, context: &str) -> Result<Vec<CString>, Diagnostic> {
    let mut out = Vec::with_capacity(strings.len());
    for s in strings {
        let c = CString::new(s).map_err(|_| {
            Diagnostic::internal(format!(
                "{context} contains NUL byte; cannot expose over C ABI"
            ))
        })?;
        out.push(c);
    }
    Ok(out)
}

fn build_nested_cstring_cache(
    groups: Vec<Vec<String>>,
    context: &str,
) -> Result<Vec<Vec<CString>>, Diagnostic> {
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        out.push(build_cstring_cache(group, context)?);
    }
    Ok(out)
}

fn diag_to_c(diag: &Diagnostic) -> onda_diag_t {
    let msg_ptr = leaked_c_string_ptr(Some(diag.message.as_str()));
    let file_ptr = leaked_c_string_ptr(diag.file.as_deref());
    let trace_text = if diag.trace.is_empty() {
        None
    } else {
        Some(
            diag.trace
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };
    let trace_ptr = leaked_c_string_ptr(trace_text.as_deref());

    onda_diag_t {
        code: diag_code_to_i32(diag.code),
        line: saturating_usize_to_i32(diag.line),
        column: saturating_usize_to_i32(diag.column),
        end_line: saturating_usize_to_i32(diag.end_line),
        end_column: saturating_usize_to_i32(diag.end_column),
        message: msg_ptr,
        file: file_ptr,
        trace: trace_ptr,
    }
}

fn leaked_c_string_ptr(text: Option<&str>) -> *const c_char {
    let Some(text) = text else {
        return ptr::null();
    };
    if !text.as_bytes().iter().all(|b| *b != 0) {
        return STATIC_ERR_INTERNAL.as_ptr().cast::<c_char>();
    }
    let mut owned = text.as_bytes().to_vec();
    owned.push(0);
    Box::leak(owned.into_boxed_slice())
        .as_ptr()
        .cast::<c_char>()
}

fn diag_code_to_i32(code: DiagCode) -> i32 {
    code as i32
}

fn saturating_usize_to_i32(value: usize) -> i32 {
    if value > i32::MAX as usize {
        i32::MAX
    } else {
        value as i32
    }
}

fn write_diag(out_diag: *mut onda_diag_t, diag: onda_diag_t) {
    if !out_diag.is_null() {
        // SAFETY: caller provides a writable pointer or null.
        unsafe {
            ptr::write(out_diag, diag);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_compile(
    src_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    onda_compile_impl(src_utf8, options, out_diag)
}

unsafe fn onda_compile_impl(
    src_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    if src_utf8.is_null() || options.is_null() {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: STATIC_ERR_NULL_ARG.as_ptr().cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    let options = &*options;
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: b"compile options require finite sample_rate > 0\0"
                    .as_ptr()
                    .cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }
    if options.block_size <= 0 {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: b"compile options require block_size > 0\0"
                    .as_ptr()
                    .cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    let src_cstr = CStr::from_ptr(src_utf8);
    let source = match src_cstr.to_str() {
        Ok(s) => s,
        Err(_) => {
            write_diag(
                out_diag,
                onda_diag_t {
                    code: DiagCode::Syntax as i32,
                    line: 0,
                    column: 0,
                    end_line: 0,
                    end_column: 0,
                    message: STATIC_ERR_INVALID_UTF8.as_ptr().cast::<c_char>(),
                    file: ptr::null(),
                    trace: ptr::null(),
                },
            );
            return ptr::null_mut();
        }
    };

    let parsed = match parse_program(source) {
        Ok(p) => p,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("parse failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let typed = match analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size as usize,
        },
    ) {
        Ok(t) => t,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("semantic analysis failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let mut jit_options = CompileOptions::default();
    jit_options.fast_math = options.fast_math != 0;
    jit_options.sample_rate = options.sample_rate;
    jit_options.block_size = options.block_size as usize;
    let jit = match lower_and_jit_with_options(typed, jit_options) {
        Ok(j) => j,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("codegen failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let input_names = match build_cstring_cache(
        (0..jit.input_count())
            .filter_map(|idx| jit.input_name(idx).map(ToOwned::to_owned))
            .collect(),
        "input name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let input_types = match build_cstring_cache(
        (0..jit.input_count())
            .filter_map(|idx| jit.input_type(idx))
            .collect(),
        "input type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let output_names = match build_cstring_cache(
        (0..jit.output_count())
            .filter_map(|idx| jit.output_name(idx).map(ToOwned::to_owned))
            .collect(),
        "output name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let output_types = match build_cstring_cache(
        (0..jit.output_count())
            .filter_map(|idx| jit.output_type(idx))
            .collect(),
        "output type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let param_names = match build_cstring_cache(
        (0..jit.param_count())
            .filter_map(|idx| jit.param_name(idx).map(ToOwned::to_owned))
            .collect(),
        "param name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let param_types = match build_cstring_cache(
        (0..jit.param_count())
            .filter_map(|idx| jit.param_type(idx))
            .collect(),
        "param type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let buffer_names = match build_cstring_cache(
        (0..jit.buffer_count())
            .filter_map(|idx| jit.buffer_name(idx).map(ToOwned::to_owned))
            .collect(),
        "buffer name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let buffer_types = match build_cstring_cache(
        (0..jit.buffer_count())
            .filter_map(|idx| jit.buffer_type(idx))
            .collect(),
        "buffer type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let event_names = match build_cstring_cache(
        (0..jit.event_count())
            .filter_map(|idx| jit.event_name(idx).map(ToOwned::to_owned))
            .collect(),
        "event name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let event_param_names = match build_nested_cstring_cache(
        (0..jit.event_count())
            .map(|event_idx| {
                jit.event_descriptor(event_idx)
                    .map(|event| {
                        event
                            .params()
                            .iter()
                            .map(|param| param.name().to_owned())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect(),
        "event parameter name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    Box::into_raw(Box::new(onda_program {
        jit,
        input_names,
        input_types,
        output_names,
        output_types,
        param_names,
        param_types,
        buffer_names,
        buffer_types,
        event_names,
        event_param_names,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_compile_file(
    file_path_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    onda_compile_file_impl(file_path_utf8, options, out_diag)
}

unsafe fn onda_compile_file_impl(
    file_path_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    if file_path_utf8.is_null() || options.is_null() {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: STATIC_ERR_NULL_ARG.as_ptr().cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    let options = &*options;
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: b"compile options require finite sample_rate > 0\0"
                    .as_ptr()
                    .cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }
    if options.block_size <= 0 {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: b"compile options require block_size > 0\0"
                    .as_ptr()
                    .cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    let path_cstr = CStr::from_ptr(file_path_utf8);
    let path_str = match path_cstr.to_str() {
        Ok(s) => s,
        Err(_) => {
            write_diag(
                out_diag,
                onda_diag_t {
                    code: DiagCode::Syntax as i32,
                    line: 0,
                    column: 0,
                    end_line: 0,
                    end_column: 0,
                    message: STATIC_ERR_INVALID_UTF8.as_ptr().cast::<c_char>(),
                    file: ptr::null(),
                    trace: ptr::null(),
                },
            );
            return ptr::null_mut();
        }
    };

    let parsed = match parse_program_file(std::path::Path::new(path_str)) {
        Ok(p) => p,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("parse failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let typed = match analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size as usize,
        },
    ) {
        Ok(t) => t,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("semantic analysis failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let mut jit_options = CompileOptions::default();
    jit_options.fast_math = options.fast_math != 0;
    jit_options.sample_rate = options.sample_rate;
    jit_options.block_size = options.block_size as usize;
    let jit = match lower_and_jit_with_options(typed, jit_options) {
        Ok(j) => j,
        Err(errs) => {
            let diag = errs
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("codegen failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let input_names = match build_cstring_cache(
        (0..jit.input_count())
            .filter_map(|idx| jit.input_name(idx).map(ToOwned::to_owned))
            .collect(),
        "input name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let input_types = match build_cstring_cache(
        (0..jit.input_count())
            .filter_map(|idx| jit.input_type(idx))
            .collect(),
        "input type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let output_names = match build_cstring_cache(
        (0..jit.output_count())
            .filter_map(|idx| jit.output_name(idx).map(ToOwned::to_owned))
            .collect(),
        "output name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let output_types = match build_cstring_cache(
        (0..jit.output_count())
            .filter_map(|idx| jit.output_type(idx))
            .collect(),
        "output type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let param_names = match build_cstring_cache(
        (0..jit.param_count())
            .filter_map(|idx| jit.param_name(idx).map(ToOwned::to_owned))
            .collect(),
        "param name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let param_types = match build_cstring_cache(
        (0..jit.param_count())
            .filter_map(|idx| jit.param_type(idx))
            .collect(),
        "param type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let buffer_names = match build_cstring_cache(
        (0..jit.buffer_count())
            .filter_map(|idx| jit.buffer_name(idx).map(ToOwned::to_owned))
            .collect(),
        "buffer name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let buffer_types = match build_cstring_cache(
        (0..jit.buffer_count())
            .filter_map(|idx| jit.buffer_type(idx))
            .collect(),
        "buffer type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let event_names = match build_cstring_cache(
        (0..jit.event_count())
            .filter_map(|idx| jit.event_name(idx).map(ToOwned::to_owned))
            .collect(),
        "event name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let event_param_names = match build_nested_cstring_cache(
        (0..jit.event_count())
            .map(|event_idx| {
                jit.event_descriptor(event_idx)
                    .map(|event| {
                        event
                            .params()
                            .iter()
                            .map(|param| param.name().to_owned())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect(),
        "event parameter name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    Box::into_raw(Box::new(onda_program {
        jit,
        input_names,
        input_types,
        output_names,
        output_types,
        param_names,
        param_types,
        buffer_names,
        buffer_types,
        event_names,
        event_param_names,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_program_destroy(program: *mut onda_program) {
    if program.is_null() {
        return;
    }
    drop(Box::from_raw(program));
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_create(
    program: *const onda_program,
    in_channels: i32,
    out_channels: i32,
    out_diag: *mut onda_diag_t,
) -> *mut onda_instance {
    if program.is_null() {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: STATIC_ERR_NULL_ARG.as_ptr().cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    if in_channels < 0 || out_channels <= 0 {
        write_diag(
            out_diag,
            onda_diag_t {
                code: DiagCode::Runtime as i32,
                line: 0,
                column: 0,
                end_line: 0,
                end_column: 0,
                message: b"invalid instance configuration\0"
                    .as_ptr()
                    .cast::<c_char>(),
                file: ptr::null(),
                trace: ptr::null(),
            },
        );
        return ptr::null_mut();
    }

    let compiled = &*program;
    let config = InstanceConfig {
        sample_rate: compiled.jit.sample_rate(),
        frames_per_block: compiled.jit.block_size(),
        in_channels: in_channels as usize,
        out_channels: out_channels as usize,
    };

    let jit = compiled.jit.clone();
    let instance = match create_instance(jit, config) {
        Ok(i) => i,
        Err(e) => {
            write_diag(out_diag, diag_to_c(&e));
            return ptr::null_mut();
        }
    };

    Box::into_raw(Box::new(onda_instance { inner: instance }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_destroy(instance: *mut onda_instance) {
    if instance.is_null() {
        return;
    }
    drop(Box::from_raw(instance));
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_by_index(
    instance: *mut onda_instance,
    index: i32,
    value_ptr: *const c_void,
    value_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || value_bytes < 0 {
        return -1;
    }
    if value_bytes > 0 && value_ptr.is_null() {
        return -1;
    }
    let bytes = if value_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(value_ptr.cast::<u8>(), value_bytes as usize)
    };
    match set_param_by_index(&mut (*instance).inner, index as usize, bytes) {
        Ok(_) => 0,
        Err(_) => -3,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_trigger_event_by_index(
    instance: *mut onda_instance,
    index: i32,
    payload_ptr: *const c_void,
    payload_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || payload_bytes < 0 {
        return -1;
    }
    if payload_bytes > 0 && payload_ptr.is_null() {
        return -1;
    }
    let payload = if payload_bytes == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(payload_ptr.cast::<u8>(), payload_bytes as usize)
    };
    match trigger_event_by_index(&mut (*instance).inner, index as usize, payload) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_bind_input(
    instance: *mut onda_instance,
    index: i32,
    src_ptr: *const c_void,
    src_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || src_bytes < 0 {
        return -1;
    }
    let ptr = src_ptr.cast::<u8>();
    let bytes = src_bytes as usize;
    match bind_input(&mut (*instance).inner, index as usize, ptr, bytes) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_bind_output(
    instance: *mut onda_instance,
    index: i32,
    dst_ptr: *mut c_void,
    dst_bytes: i32,
) -> i32 {
    if instance.is_null() || index < 0 || dst_bytes < 0 {
        return -1;
    }
    let ptr = dst_ptr.cast::<u8>();
    let bytes = dst_bytes as usize;
    match bind_output(&mut (*instance).inner, index as usize, ptr, bytes) {
        Ok(_) => 0,
        Err(_) => -2,
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
    if instance.is_null() || index < 0 || frames < 0 || channels < 0 {
        return -1;
    }
    let Some(elem_ty) = primitive_type_from_i32(elem_type) else {
        return -1;
    };
    match bind_buffer(
        &mut (*instance).inner,
        index as usize,
        ptr.cast::<u8>(),
        frames as usize,
        channels as usize,
        sample_rate,
        elem_ty,
    ) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_bound(instance: *mut onda_instance, frames: i32) -> i32 {
    if instance.is_null() || frames < 0 {
        return -1;
    }
    match process_bound(&mut (*instance).inner, frames as usize) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_reset_instance_state(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    reset_instance_state(&mut (*instance).inner);
    0
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_bindings(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match validate_bindings(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_inputs(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match validate_inputs(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_outputs(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match validate_outputs(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_validate_buffers(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match validate_buffers(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_unchecked(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match process_unchecked(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*program).jit.input_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*program).jit.output_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*program).jit.param_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*program).jit.buffer_count())
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_count(program: *const onda_program) -> i32 {
    if program.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*program).jit.event_count())
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

fn primitive_type_from_i32(value: i32) -> Option<PrimitiveType> {
    match value {
        0 => Some(PrimitiveType::F32),
        1 => Some(PrimitiveType::F64),
        2 => Some(PrimitiveType::I32),
        3 => Some(PrimitiveType::I64),
        4 => Some(PrimitiveType::Bool),
        _ => None,
    }
}

fn primitive_type_to_i32(value: PrimitiveType) -> i32 {
    match value {
        PrimitiveType::F32 => 0,
        PrimitiveType::F64 => 1,
        PrimitiveType::I32 => 2,
        PrimitiveType::I64 => 3,
        PrimitiveType::Bool => 4,
    }
}

fn primitive_type_bytes(value: PrimitiveType) -> usize {
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
    (*program)
        .jit
        .event_descriptor(event_index as usize)
        .and_then(|event| event.params().get(param_index as usize))
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).input_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).output_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).param_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).buffer_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_name(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).event_names, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_param_count(
    program: *const onda_program,
    event_index: i32,
) -> i32 {
    if program.is_null() || event_index < 0 {
        return -1;
    }
    (*program)
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
    (&(*program).event_param_names)
        .get(event_index as usize)
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
    index_from_name(name, |key| (*program).jit.input_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (*program).jit.output_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (*program).jit.param_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (*program).jit.buffer_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_index(
    program: *const onda_program,
    name: *const c_char,
) -> i32 {
    if program.is_null() {
        return -1;
    }
    index_from_name(name, |key| (*program).jit.event_index(key))
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).input_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).output_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).param_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_type(
    program: *const onda_program,
    index: i32,
) -> *const c_char {
    if program.is_null() {
        return ptr::null();
    }
    cstr_ptr_at(&(*program).buffer_types, index)
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (*program).jit.input_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (*program).jit.output_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_type_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bytes_from_index(index, |idx| (*program).jit.param_type_bytes(idx))
}

#[no_mangle]
pub unsafe extern "C" fn onda_event_payload_bytes(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    /* Dynamic slice-event payloads also report -1 here. */
    bytes_from_index(index, |idx| (*program).jit.event_payload_bytes(idx))
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
        .and_then(|param| i32::try_from(param.byte_offset()).ok())
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
            .jit
            .outputs()
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
        (*program)
            .jit
            .params()
            .get(idx)
            .map(|d| primitive_type_to_i32(d.elem_ty()))
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.inputs().get(idx).map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.outputs().get(idx).map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_array_len(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.params().get(idx).map(|d| d.array_len())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.inputs().get(idx).map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.outputs().get(idx).map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_slot_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.params().get(idx).map(|d| d.slot_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.inputs().get(idx).map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.outputs().get(idx).map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_byte_offset(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    usize_from_index(index, |idx| {
        (*program).jit.params().get(idx).map(|d| d.byte_offset())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (*program)
            .jit
            .inputs()
            .get(idx)
            .map(|d| d.default().is_some())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (*program)
            .jit
            .outputs()
            .get(idx)
            .map(|d| d.default().is_some())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_default(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (*program)
            .jit
            .params()
            .get(idx)
            .map(|d| d.default().is_some())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_default_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (*program)
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
        (*program)
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
        (*program)
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
        (*program).jit.inputs().get(idx).map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_param_has_range(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (*program).jit.params().get(idx).map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_output_has_range(program: *const onda_program, index: i32) -> i32 {
    if program.is_null() {
        return -1;
    }
    bool_flag_from_index(index, |idx| {
        (*program).jit.outputs().get(idx).map(|d| d.has_range())
    })
}

#[no_mangle]
pub unsafe extern "C" fn onda_input_range_min_f64(program: *const onda_program, index: i32) -> f64 {
    if program.is_null() {
        return f64::NAN;
    }
    f64_from_index_or_nan(index, |idx| {
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
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
        (*program)
            .jit
            .params()
            .get(idx)
            .and_then(|d| d.range_max_as_f64())
    })
}
