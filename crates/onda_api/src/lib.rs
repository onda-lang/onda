// This crate is the implementation of the C ABI declared in `include/onda.h`.
// Pointer lifetime, alignment, ownership, and nullability contracts live with
// that public C surface instead of being duplicated on every Rust export.
#![allow(clippy::missing_safety_doc)]

use std::alloc::Layout;
use std::collections::{BTreeMap, HashMap};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::{Arc, OnceLock};

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, DeclaredBufferChannels, DeclaredEventParam,
    DeclaredState, JitProgram, MirCompileOptions, RuntimeAllocator, TargetOptLevel,
};
use onda_frontend::{
    load_program_file, load_program_file_from_snapshot, parse_program, rewrite_source_references,
    DiagCode, Diagnostic, PrimitiveType, Program, SourceManifest, SourceReferenceKind,
    SourceReferenceRewrite, SourceResolution,
};
use onda_project::{
    decode_ondabuffer, encode_ondabuffer, validate_ondabuffer, AssetId, BufferAsset, BufferElement,
    BufferSamples, MaterializationPlan, ProjectBufferChannels, ProjectBufferDeclaration,
    ProjectImage, ProjectLimits, SourceImage,
};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, create_instance_with_allocator,
    prepare_unchecked_process, process_checked, process_checked_segment, process_unchecked,
    process_unchecked_segment, read_control_output_bytes, reset_instance_state, set_param_by_index,
    set_param_normalized as runtime_set_param_normalized,
    set_param_plain_f64 as runtime_set_param_plain_f64, trigger_event_by_index,
    trigger_event_by_index_unchecked, validate_bindings, validate_buffers, validate_inputs,
    validate_outputs, Instance, InstanceConfig,
};
use onda_semantics::{
    analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions, TypedBufferChannels,
    TypedProgram,
};

pub const ONDA_PROCESS_BEGIN_BLOCK: i32 = onda_runtime::PROCESS_BEGIN_BLOCK as i32;
pub const ONDA_PROCESS_END_BLOCK: i32 = onda_runtime::PROCESS_END_BLOCK as i32;
pub const ONDA_PROCESS_FULL_BLOCK: i32 = onda_runtime::PROCESS_FULL_BLOCK as i32;
pub const ONDA_EXECUTION_OK: i32 = onda_codegen_llvm::PROCESSOR_EXECUTION_OK as i32;
pub const ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE: i32 =
    onda_codegen_llvm::PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE as i32;
pub const ONDA_PRIMITIVE_F32: i32 = 0;
pub const ONDA_PRIMITIVE_F64: i32 = 1;
pub const ONDA_PRIMITIVE_I32: i32 = 2;
pub const ONDA_PRIMITIVE_I64: i32 = 3;
pub const ONDA_PRIMITIVE_BOOL: i32 = 4;

fn execution_status_to_c(status: Result<u32, Diagnostic>) -> i32 {
    match status {
        Ok(value) => i32::try_from(value).unwrap_or(-2),
        Err(_) => -2,
    }
}

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

#[repr(C)]
pub struct onda_source_graph_document_t {
    pub path_utf8: *const c_char,
    pub source_utf8: *const c_char,
    pub source_bytes: usize,
}

#[repr(C)]
pub struct onda_source_graph_resolution_t {
    pub source_path_utf8: *const c_char,
    pub kind: i32,
    pub specifier_utf8: *const c_char,
    pub target_path_utf8: *const c_char,
}

#[repr(C)]
pub struct onda_source_rewrite_t {
    pub kind: i32,
    pub specifier_utf8: *const c_char,
    pub replacement_utf8: *const c_char,
}

#[repr(C)]
pub struct onda_project_buffer_asset_t {
    pub name_utf8: *const c_char,
    pub ondabuffer_bytes: *const c_void,
    pub ondabuffer_byte_count: usize,
}

#[repr(C)]
pub struct onda_project_file_t {
    pub path_utf8: *const c_char,
    pub bytes: *const c_void,
    pub byte_count: usize,
}

#[repr(C)]
pub struct onda_buffer_asset_info_t {
    pub element_type: i32,
    pub frames: u32,
    pub channels: u32,
    pub sample_rate: f32,
    pub sample_bytes: usize,
}

pub const ONDA_SOURCE_REFERENCE_INCLUDE: i32 = 0;
pub const ONDA_SOURCE_REFERENCE_IMPORT: i32 = 1;

fn compile_typed_mir(typed: TypedProgram, fast_math: bool) -> Result<JitProgram, Vec<Diagnostic>> {
    let mir = lower_program_to_optimized_mir(&typed).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| Diagnostic::internal(format!("MIR lowering failed: {error}")))
            .collect::<Vec<_>>()
    })?;
    jit_program_from_optimized_mir_with_options(
        mir,
        MirCompileOptions {
            fast_math,
            opt_level: TargetOptLevel::O3,
        },
    )
}

#[allow(non_camel_case_types)]
pub type onda_alloc_fn =
    unsafe extern "C" fn(context: *mut c_void, size: usize, align: usize) -> *mut c_void;
#[allow(non_camel_case_types)]
pub type onda_free_fn =
    unsafe extern "C" fn(context: *mut c_void, ptr: *mut c_void, size: usize, align: usize);

#[repr(C)]
pub struct onda_allocator_t {
    pub context: *mut c_void,
    pub alloc: Option<onda_alloc_fn>,
    pub free: Option<onda_free_fn>,
}

#[allow(non_camel_case_types)]
pub struct onda_program {
    inner: Arc<CompiledProgram>,
}

struct CompiledProgram {
    jit: JitProgram,
    input_names: Vec<CString>,
    input_types: Vec<CString>,
    output_names: Vec<CString>,
    output_types: Vec<CString>,
    control_output_names: Vec<CString>,
    control_output_types: Vec<CString>,
    param_names: Vec<CString>,
    param_types: Vec<CString>,
    buffer_names: Vec<CString>,
    buffer_types: Vec<CString>,
    buffer_array_names: Vec<CString>,
    event_names: Vec<CString>,
    event_param_names: Vec<Vec<CString>>,
    state_names: Vec<CString>,
    state_types: Vec<CString>,
    project_defaults: Option<ProjectDefaults>,
}

struct ProjectDefaults {
    image: Arc<ProjectImage>,
    bindings: Vec<Option<AssetId>>,
}

#[allow(non_camel_case_types)]
pub struct onda_source_manifest {
    inner: SourceManifest,
    paths: Vec<CString>,
    unresolved_paths: Vec<CString>,
    document_paths: Vec<CString>,
    document_contents: Vec<Box<[u8]>>,
    resolutions: Vec<CSourceResolution>,
    unresolved_resolutions: Vec<CUnresolvedSourceResolution>,
}

#[allow(non_camel_case_types)]
pub struct onda_project_image {
    inner: Arc<ProjectImage>,
    content_digest: CString,
    entry: CString,
    stdlib_digest: CString,
    document_paths: Vec<CString>,
    resolution_sources: Vec<CString>,
    resolution_specifiers: Vec<CString>,
    resolution_targets: Vec<CString>,
    buffers: Vec<CProjectBufferInfo>,
}

struct CProjectBufferInfo {
    name: CString,
    asset_id: CString,
    element_type: i32,
    frames: u32,
    channels: u32,
    sample_rate: f32,
}

#[allow(non_camel_case_types)]
pub struct onda_project_materialization_plan {
    inner: MaterializationPlan,
    file_paths: Vec<CString>,
}

struct CSourceReference {
    source_path: CString,
    kind: i32,
    specifier: CString,
}

struct CSourceResolution {
    reference: CSourceReference,
    target_path: CString,
}

struct CUnresolvedSourceResolution {
    reference: CSourceReference,
    candidates: Vec<CString>,
}

#[allow(non_camel_case_types)]
pub struct onda_instance {
    allocation: OndaInstanceAllocation,
    program: Arc<CompiledProgram>,
    inner: Instance,
}

#[derive(Clone, Copy)]
struct OndaInstanceAllocation {
    allocator: Option<RuntimeAllocator>,
}

const STATIC_ERR_NULL_ARG: &[u8] = b"null argument\0";
const STATIC_ERR_INTERNAL: &[u8] = b"internal error\0";
const STATIC_ERR_INVALID_ALLOCATOR: &[u8] = b"invalid allocator\0";

fn validate_compile_options(options: &onda_compile_options_t) -> Result<(), &'static [u8]> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(b"compile options require finite sample_rate > 0\0");
    }
    if options.block_size <= 0 {
        return Err(b"compile options require block_size > 0\0");
    }
    Ok(())
}

unsafe fn parse_required_c_string(value: *const c_char, name: &str) -> Result<String, Diagnostic> {
    if value.is_null() {
        return Err(Diagnostic::syntax(format!("{name} is null"), 0, 0));
    }
    CStr::from_ptr(value)
        .to_str()
        .map(ToOwned::to_owned)
        .map_err(|_| Diagnostic::syntax(format!("{name} is not valid UTF-8"), 0, 0))
}

fn source_reference_kind(kind: i32) -> Option<SourceReferenceKind> {
    match kind {
        ONDA_SOURCE_REFERENCE_INCLUDE => Some(SourceReferenceKind::Include),
        ONDA_SOURCE_REFERENCE_IMPORT => Some(SourceReferenceKind::Import),
        _ => None,
    }
}

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

unsafe fn write_source_manifest(
    out_manifest: *mut *mut onda_source_manifest,
    manifest: &SourceManifest,
) -> Result<(), Diagnostic> {
    if out_manifest.is_null() {
        return Ok(());
    }
    fn path_strings(paths: &[std::path::PathBuf]) -> Result<Vec<String>, Diagnostic> {
        paths
            .iter()
            .map(|path| {
                path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "source path '{}' is not valid UTF-8",
                        path.display()
                    ))
                })
            })
            .collect()
    }
    let paths = build_cstring_cache(path_strings(&manifest.files)?, "source path")?;
    let unresolved_paths = build_cstring_cache(
        path_strings(&manifest.unresolved_files)?,
        "unresolved source path",
    )?;
    let document_paths = build_cstring_cache(
        manifest
            .documents
            .iter()
            .map(|source| {
                source.path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "source path '{}' is not valid UTF-8",
                        source.path.display()
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        "source document path",
    )?;
    let document_contents = manifest
        .documents
        .iter()
        .map(|source| source.contents.as_bytes().to_vec().into_boxed_slice())
        .collect();
    let mut resolutions = Vec::with_capacity(manifest.resolutions.len());
    for resolution in &manifest.resolutions {
        resolutions.push(CSourceResolution {
            reference: source_reference_to_c(
                &resolution.source,
                resolution.kind,
                &resolution.specifier,
            )?,
            target_path: path_to_cstring(&resolution.target, "resolution target path")?,
        });
    }
    let mut unresolved_resolutions = Vec::with_capacity(manifest.unresolved_resolutions.len());
    for resolution in &manifest.unresolved_resolutions {
        unresolved_resolutions.push(CUnresolvedSourceResolution {
            reference: source_reference_to_c(
                &resolution.source,
                resolution.kind,
                &resolution.specifier,
            )?,
            candidates: resolution
                .candidates
                .iter()
                .map(|path| path_to_cstring(path, "unresolved resolution candidate path"))
                .collect::<Result<_, _>>()?,
        });
    }
    *out_manifest = Box::into_raw(Box::new(onda_source_manifest {
        inner: manifest.clone(),
        paths,
        unresolved_paths,
        document_paths,
        document_contents,
        resolutions,
        unresolved_resolutions,
    }));
    Ok(())
}

fn path_to_cstring(path: &Path, name: &str) -> Result<CString, Diagnostic> {
    let path = path
        .to_str()
        .ok_or_else(|| Diagnostic::internal(format!("{name} is not UTF-8")))?;
    CString::new(path).map_err(|_| Diagnostic::internal(format!("{name} contains NUL")))
}

fn source_reference_to_c(
    source: &Path,
    kind: SourceReferenceKind,
    specifier: &str,
) -> Result<CSourceReference, Diagnostic> {
    Ok(CSourceReference {
        source_path: path_to_cstring(source, "resolution source path")?,
        kind: match kind {
            SourceReferenceKind::Include => ONDA_SOURCE_REFERENCE_INCLUDE,
            SourceReferenceKind::Import => ONDA_SOURCE_REFERENCE_IMPORT,
        },
        specifier: CString::new(specifier)
            .map_err(|_| Diagnostic::internal("source specifier contains NUL"))?,
    })
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

fn static_runtime_diag(message: &'static [u8]) -> onda_diag_t {
    onda_diag_t {
        code: DiagCode::Runtime as i32,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        message: message.as_ptr().cast::<c_char>(),
        file: ptr::null(),
        trace: ptr::null(),
    }
}

unsafe fn runtime_allocator_from_c(
    allocator: *const onda_allocator_t,
) -> Result<RuntimeAllocator, onda_diag_t> {
    if allocator.is_null() {
        return Err(static_runtime_diag(STATIC_ERR_INVALID_ALLOCATOR));
    }
    let allocator = &*allocator;
    let Some(alloc) = allocator.alloc else {
        return Err(static_runtime_diag(STATIC_ERR_INVALID_ALLOCATOR));
    };
    let Some(free) = allocator.free else {
        return Err(static_runtime_diag(STATIC_ERR_INVALID_ALLOCATOR));
    };
    Ok(RuntimeAllocator::new(allocator.context, alloc, free))
}

fn allocate_instance_handle(
    inner: Instance,
    program: Arc<CompiledProgram>,
    allocator: Option<RuntimeAllocator>,
    out_diag: *mut onda_diag_t,
) -> *mut onda_instance {
    let allocation = OndaInstanceAllocation { allocator };
    let Some(allocator) = allocator else {
        return Box::into_raw(Box::new(onda_instance {
            allocation,
            program,
            inner,
        }));
    };

    let layout = Layout::new::<onda_instance>();
    let raw = unsafe { allocator.allocate(layout.size(), layout.align()) };
    if raw.is_null() {
        write_diag(
            out_diag,
            static_runtime_diag(b"runtime allocator returned null for instance handle\0"),
        );
        drop((inner, program));
        return ptr::null_mut();
    }
    if !(raw as usize).is_multiple_of(layout.align()) {
        unsafe {
            allocator.deallocate(raw, layout.size(), layout.align());
        }
        write_diag(
            out_diag,
            static_runtime_diag(b"runtime allocator returned misaligned instance handle\0"),
        );
        drop((inner, program));
        return ptr::null_mut();
    }

    let instance = raw.cast::<onda_instance>();
    unsafe {
        ptr::write(
            instance,
            onda_instance {
                allocation,
                program,
                inner,
            },
        );
    }
    instance
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
    if let Err(message) = validate_compile_options(options) {
        write_diag(out_diag, static_runtime_diag(message));
        return ptr::null_mut();
    }

    let source = match parse_required_c_string(src_utf8, "source string") {
        Ok(source) => source,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let parsed = match parse_program(&source) {
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

    compile_parsed_program(parsed, options, None, out_diag)
}

fn compile_parsed_program(
    parsed: Program,
    options: &onda_compile_options_t,
    project_image: Option<Arc<ProjectImage>>,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
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

    if let Some(image) = project_image.as_deref() {
        let mut declarations = Vec::with_capacity(typed.buffers.len());
        for buffer in &typed.buffers {
            let channels = match &buffer.channels {
                TypedBufferChannels::Mono => ProjectBufferChannels::Mono,
                TypedBufferChannels::Static(channels) => match u32::try_from(*channels) {
                    Ok(channels) => ProjectBufferChannels::Static(channels),
                    Err(_) => {
                        write_diag(
                            out_diag,
                            project_error_diag(format!(
                                "buffer '{}' channel count does not fit the project format",
                                buffer.name
                            )),
                        );
                        return ptr::null_mut();
                    }
                },
                TypedBufferChannels::Dynamic => ProjectBufferChannels::Dynamic,
            };
            declarations.push(ProjectBufferDeclaration {
                name: buffer.name.clone(),
                element: match buffer.elem_ty {
                    PrimitiveType::F32 => BufferElement::F32,
                    PrimitiveType::F64 => BufferElement::F64,
                    PrimitiveType::I32 => BufferElement::I32,
                    PrimitiveType::I64 => BufferElement::I64,
                    PrimitiveType::Bool => BufferElement::Bool,
                },
                channels,
                array_len: buffer.array_len,
                is_array: buffer.is_array,
            });
        }
        if let Err(error) = image.validate_buffer_declarations(&declarations) {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    }

    let jit = match compile_typed_mir(typed, options.fast_math != 0) {
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

    let project_defaults = match project_image {
        Some(image) => match project_defaults(&jit, image) {
            Ok(defaults) => Some(defaults),
            Err(error) => {
                write_diag(out_diag, project_error_diag(error));
                return ptr::null_mut();
            }
        },
        None => None,
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
    let control_output_names = match build_cstring_cache(
        (0..jit.control_output_count())
            .filter_map(|idx| jit.control_output_name(idx).map(ToOwned::to_owned))
            .collect(),
        "control output name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let control_output_types = match build_cstring_cache(
        (0..jit.control_output_count())
            .filter_map(|idx| jit.control_output_type(idx))
            .collect(),
        "control output type",
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
    let buffer_array_names = match build_cstring_cache(
        jit.buffer_arrays()
            .iter()
            .map(|array| array.name().to_owned())
            .collect(),
        "buffer array name",
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
    let state_names = match build_cstring_cache(
        (0..jit.state_count())
            .filter_map(|idx| jit.state_name(idx).map(ToOwned::to_owned))
            .collect(),
        "state name",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let state_types = match build_cstring_cache(
        (0..jit.state_count())
            .filter_map(|idx| jit.state_type(idx))
            .collect(),
        "state type",
    ) {
        Ok(v) => v,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let inner = CompiledProgram {
        jit,
        input_names,
        input_types,
        output_names,
        output_types,
        control_output_names,
        control_output_types,
        param_names,
        param_types,
        buffer_names,
        buffer_types,
        buffer_array_names,
        event_names,
        event_param_names,
        state_names,
        state_types,
        project_defaults,
    };
    Box::into_raw(Box::new(onda_program {
        inner: Arc::new(inner),
    }))
}

fn project_defaults(
    jit: &JitProgram,
    image: Arc<ProjectImage>,
) -> Result<ProjectDefaults, onda_project::ProjectError> {
    let mut bindings = vec![None; jit.buffer_count()];
    for (name, asset_id) in image.buffer_bindings() {
        let index = jit.buffer_index(name).ok_or_else(|| {
            onda_project::ProjectError::new(format!(
                "project buffer '{name}' is not a physical buffer in the compiled program"
            ))
        })?;
        let declaration = &jit.buffers()[index];
        if declaration.may_write() {
            return Err(onda_project::ProjectError::new(format!(
                "project asset for buffer '{name}' is immutable, but reachable Onda code may write that buffer; provide writable host memory with onda_bind_buffer instead"
            )));
        }
        bindings[index] = Some(asset_id.clone());
    }
    Ok(ProjectDefaults { image, bindings })
}

fn primitive_type_for_buffer_element(element: BufferElement) -> PrimitiveType {
    match element {
        BufferElement::Bool => PrimitiveType::Bool,
        BufferElement::I32 => PrimitiveType::I32,
        BufferElement::I64 => PrimitiveType::I64,
        BufferElement::F32 => PrimitiveType::F32,
        BufferElement::F64 => PrimitiveType::F64,
    }
}

fn bind_project_default(
    instance: &mut Instance,
    defaults: &ProjectDefaults,
    index: usize,
) -> Result<bool, Diagnostic> {
    let Some(asset_id) = defaults.bindings.get(index).and_then(Option::as_ref) else {
        return Ok(false);
    };
    let asset = defaults.image.assets().get(asset_id).ok_or_else(|| {
        Diagnostic::internal(format!(
            "project default buffer references missing asset '{}'",
            asset_id.as_str()
        ))
    })?;
    unsafe {
        bind_buffer(
            instance,
            index,
            asset.samples.as_ptr().cast_mut(),
            asset.frames as usize,
            asset.channels as usize,
            asset.sample_rate,
            primitive_type_for_buffer_element(asset.element()),
        )?;
    }
    Ok(true)
}

fn bind_project_defaults(
    instance: &mut Instance,
    defaults: &ProjectDefaults,
) -> Result<(), Diagnostic> {
    for index in 0..defaults.bindings.len() {
        bind_project_default(instance, defaults, index)?;
    }
    Ok(())
}

#[no_mangle]
pub unsafe extern "C" fn onda_compile_file(
    file_path_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_sources: *mut *mut onda_source_manifest,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    onda_compile_file_impl(file_path_utf8, options, out_sources, out_diag)
}

unsafe fn onda_compile_file_impl(
    file_path_utf8: *const c_char,
    options: *const onda_compile_options_t,
    out_sources: *mut *mut onda_source_manifest,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    if !out_sources.is_null() {
        *out_sources = ptr::null_mut();
    }
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
    if let Err(message) = validate_compile_options(options) {
        write_diag(out_diag, static_runtime_diag(message));
        return ptr::null_mut();
    }

    let path = match parse_required_c_string(file_path_utf8, "file path") {
        Ok(path) => path,
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };

    let loaded = match load_program_file(Path::new(&path)) {
        Ok(loaded) => loaded,
        Err(error) => {
            if let Err(diag) = write_source_manifest(out_sources, &error.sources) {
                write_diag(out_diag, diag_to_c(&diag));
                return ptr::null_mut();
            }
            let diag = error
                .diagnostics
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("parse failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    if let Err(diag) = write_source_manifest(out_sources, &loaded.sources) {
        write_diag(out_diag, diag_to_c(&diag));
        return ptr::null_mut();
    }
    compile_parsed_program(loaded.program, options, None, out_diag)
}

#[no_mangle]
pub unsafe extern "C" fn onda_compile_source_graph(
    entry_path_utf8: *const c_char,
    sources: *const onda_source_graph_document_t,
    source_count: usize,
    resolutions: *const onda_source_graph_resolution_t,
    resolution_count: usize,
    options: *const onda_compile_options_t,
    out_sources: *mut *mut onda_source_manifest,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    if !out_sources.is_null() {
        *out_sources = ptr::null_mut();
    }
    if entry_path_utf8.is_null()
        || sources.is_null()
        || source_count == 0
        || options.is_null()
        || (resolution_count > 0 && resolutions.is_null())
    {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let options = &*options;
    if let Err(message) = validate_compile_options(options) {
        write_diag(out_diag, static_runtime_diag(message));
        return ptr::null_mut();
    }

    let entry = match parse_required_c_string(entry_path_utf8, "entry path") {
        Ok(value) => PathBuf::from(value),
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    let mut source_map = HashMap::with_capacity(source_count);
    for source in slice::from_raw_parts(sources, source_count) {
        let path = match parse_required_c_string(source.path_utf8, "source path") {
            Ok(value) => PathBuf::from(value),
            Err(diag) => {
                write_diag(out_diag, diag_to_c(&diag));
                return ptr::null_mut();
            }
        };
        if source.source_bytes > 0 && source.source_utf8.is_null() {
            write_diag(
                out_diag,
                diag_to_c(&Diagnostic::syntax("source contents are null", 0, 0)),
            );
            return ptr::null_mut();
        }
        let bytes = if source.source_bytes == 0 {
            &[]
        } else {
            slice::from_raw_parts(source.source_utf8.cast::<u8>(), source.source_bytes)
        };
        let contents = match std::str::from_utf8(bytes) {
            Ok(value) => value.to_owned(),
            Err(_) => {
                write_diag(
                    out_diag,
                    diag_to_c(&Diagnostic::syntax(
                        "source contents are not valid UTF-8",
                        0,
                        0,
                    )),
                );
                return ptr::null_mut();
            }
        };
        if source_map.insert(path.clone(), contents).is_some() {
            write_diag(
                out_diag,
                diag_to_c(&Diagnostic::syntax(
                    format!("duplicate snapshot source '{}'", path.display()),
                    0,
                    0,
                )),
            );
            return ptr::null_mut();
        }
    }

    let resolution_inputs = if resolution_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(resolutions, resolution_count)
    };
    let mut source_resolutions = Vec::with_capacity(resolution_count);
    for resolution in resolution_inputs {
        let source =
            match parse_required_c_string(resolution.source_path_utf8, "resolution source path") {
                Ok(value) => PathBuf::from(value),
                Err(diag) => {
                    write_diag(out_diag, diag_to_c(&diag));
                    return ptr::null_mut();
                }
            };
        let specifier = match parse_required_c_string(resolution.specifier_utf8, "source specifier")
        {
            Ok(value) => value,
            Err(diag) => {
                write_diag(out_diag, diag_to_c(&diag));
                return ptr::null_mut();
            }
        };
        let target =
            match parse_required_c_string(resolution.target_path_utf8, "resolution target path") {
                Ok(value) => PathBuf::from(value),
                Err(diag) => {
                    write_diag(out_diag, diag_to_c(&diag));
                    return ptr::null_mut();
                }
            };
        let kind = match source_reference_kind(resolution.kind) {
            Some(kind) => kind,
            None => {
                write_diag(
                    out_diag,
                    diag_to_c(&Diagnostic::syntax(
                        "source resolution kind is invalid",
                        0,
                        0,
                    )),
                );
                return ptr::null_mut();
            }
        };
        source_resolutions.push(SourceResolution {
            source,
            kind,
            specifier,
            target,
        });
    }

    let loaded = match load_program_file_from_snapshot(&entry, &source_map, &source_resolutions) {
        Ok(loaded) => loaded,
        Err(error) => {
            if let Err(diag) = write_source_manifest(out_sources, &error.sources) {
                write_diag(out_diag, diag_to_c(&diag));
                return ptr::null_mut();
            }
            let diag = error
                .diagnostics
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("snapshot parse failed"));
            write_diag(out_diag, diag_to_c(&diag));
            return ptr::null_mut();
        }
    };
    if let Err(diag) = write_source_manifest(out_sources, &loaded.sources) {
        write_diag(out_diag, diag_to_c(&diag));
        return ptr::null_mut();
    }
    compile_parsed_program(loaded.program, options, None, out_diag)
}

#[no_mangle]
pub unsafe extern "C" fn onda_rewrite_source_references(
    source_path_utf8: *const c_char,
    source_utf8: *const c_char,
    source_bytes: usize,
    rewrites: *const onda_source_rewrite_t,
    rewrite_count: usize,
    out_utf8: *mut c_char,
    out_capacity: i32,
    out_diag: *mut onda_diag_t,
) -> i32 {
    if source_path_utf8.is_null()
        || (source_bytes > 0 && source_utf8.is_null())
        || (rewrite_count > 0 && rewrites.is_null())
        || out_capacity < 0
    {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return -1;
    }
    let path = match parse_required_c_string(source_path_utf8, "source path") {
        Ok(value) => PathBuf::from(value),
        Err(diag) => {
            write_diag(out_diag, diag_to_c(&diag));
            return -1;
        }
    };
    let source_bytes = if source_bytes == 0 {
        &[]
    } else {
        slice::from_raw_parts(source_utf8.cast::<u8>(), source_bytes)
    };
    let source = match std::str::from_utf8(source_bytes) {
        Ok(value) => value,
        Err(_) => {
            write_diag(
                out_diag,
                diag_to_c(&Diagnostic::syntax(
                    "source contents are not valid UTF-8",
                    0,
                    0,
                )),
            );
            return -1;
        }
    };
    let rewrite_inputs = if rewrite_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(rewrites, rewrite_count)
    };
    let mut source_rewrites = Vec::with_capacity(rewrite_count);
    for rewrite in rewrite_inputs {
        let Some(kind) = source_reference_kind(rewrite.kind) else {
            write_diag(
                out_diag,
                diag_to_c(&Diagnostic::syntax(
                    "source reference rewrite kind is invalid",
                    0,
                    0,
                )),
            );
            return -1;
        };
        let specifier = match parse_required_c_string(rewrite.specifier_utf8, "source specifier") {
            Ok(value) => value,
            Err(diag) => {
                write_diag(out_diag, diag_to_c(&diag));
                return -1;
            }
        };
        let replacement =
            match parse_required_c_string(rewrite.replacement_utf8, "replacement specifier") {
                Ok(value) => value,
                Err(diag) => {
                    write_diag(out_diag, diag_to_c(&diag));
                    return -1;
                }
            };
        source_rewrites.push(SourceReferenceRewrite {
            kind,
            specifier,
            replacement,
        });
    }

    let rewritten = match rewrite_source_references(&path, source, &source_rewrites) {
        Ok(value) => value,
        Err(diagnostics) => {
            let diagnostic = diagnostics
                .into_iter()
                .next()
                .unwrap_or_else(|| Diagnostic::internal("source reference rewrite failed"));
            write_diag(out_diag, diag_to_c(&diagnostic));
            return -1;
        }
    };
    let Ok(required) = i32::try_from(rewritten.len()) else {
        write_diag(
            out_diag,
            diag_to_c(&Diagnostic::syntax(
                "rewritten source is too large for the C API",
                0,
                0,
            )),
        );
        return -1;
    };
    if out_utf8.is_null() || out_capacity < required {
        return required;
    }
    ptr::copy_nonoverlapping(
        rewritten.as_ptr().cast::<c_char>(),
        out_utf8,
        rewritten.len(),
    );
    required
}

#[no_mangle]
pub unsafe extern "C" fn onda_program_destroy(program: *mut onda_program) {
    if program.is_null() {
        return;
    }
    drop(Box::from_raw(program));
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_count(manifest: *const onda_source_manifest) -> i32 {
    if manifest.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*manifest).paths.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    if manifest.is_null() || index < 0 {
        return ptr::null();
    }
    let manifest = &*manifest;
    manifest
        .paths
        .get(index as usize)
        .map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_count(
    manifest: *const onda_source_manifest,
) -> i32 {
    if manifest.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*manifest).unresolved_paths.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    if manifest.is_null() || index < 0 {
        return ptr::null();
    }
    let manifest = &*manifest;
    manifest
        .unresolved_paths
        .get(index as usize)
        .map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_document_count(
    manifest: *const onda_source_manifest,
) -> i32 {
    if manifest.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*manifest).document_paths.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_document_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    if manifest.is_null() || index < 0 {
        return ptr::null();
    }
    let manifest = &*manifest;
    manifest
        .document_paths
        .get(index as usize)
        .map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_document_contents(
    manifest: *const onda_source_manifest,
    index: i32,
    out_bytes: *mut usize,
) -> *const c_char {
    if !out_bytes.is_null() {
        *out_bytes = 0;
    }
    if manifest.is_null() || index < 0 {
        return ptr::null();
    }
    let manifest = &*manifest;
    let Some(contents) = manifest.document_contents.get(index as usize) else {
        return ptr::null();
    };
    if !out_bytes.is_null() {
        *out_bytes = contents.len();
    }
    contents.as_ptr().cast::<c_char>()
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_count(
    manifest: *const onda_source_manifest,
) -> i32 {
    if manifest.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*manifest).resolutions.len())
}

unsafe fn map_source_resolution<T>(
    manifest: *const onda_source_manifest,
    index: i32,
    map: impl FnOnce(&CSourceResolution) -> T,
) -> Option<T> {
    if manifest.is_null() || index < 0 {
        return None;
    }
    let manifest = &*manifest;
    manifest.resolutions.get(index as usize).map(map)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_source_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    map_source_resolution(manifest, index, |value| {
        value.reference.source_path.as_ptr()
    })
    .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_kind(
    manifest: *const onda_source_manifest,
    index: i32,
) -> i32 {
    map_source_resolution(manifest, index, |value| value.reference.kind).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_specifier(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    map_source_resolution(manifest, index, |value| value.reference.specifier.as_ptr())
        .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_resolution_target_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    map_source_resolution(manifest, index, |value| value.target_path.as_ptr())
        .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_count(
    manifest: *const onda_source_manifest,
) -> i32 {
    if manifest.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*manifest).unresolved_resolutions.len())
}

unsafe fn map_unresolved_source_resolution<T>(
    manifest: *const onda_source_manifest,
    index: i32,
    map: impl FnOnce(&CUnresolvedSourceResolution) -> T,
) -> Option<T> {
    if manifest.is_null() || index < 0 {
        return None;
    }
    let manifest = &*manifest;
    manifest.unresolved_resolutions.get(index as usize).map(map)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_source_path(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    map_unresolved_source_resolution(manifest, index, |value| {
        value.reference.source_path.as_ptr()
    })
    .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_kind(
    manifest: *const onda_source_manifest,
    index: i32,
) -> i32 {
    map_unresolved_source_resolution(manifest, index, |value| value.reference.kind).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_specifier(
    manifest: *const onda_source_manifest,
    index: i32,
) -> *const c_char {
    map_unresolved_source_resolution(manifest, index, |value| value.reference.specifier.as_ptr())
        .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_candidate_count(
    manifest: *const onda_source_manifest,
    index: i32,
) -> i32 {
    map_unresolved_source_resolution(manifest, index, |value| {
        saturating_usize_to_i32(value.candidates.len())
    })
    .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_unresolved_resolution_candidate_path(
    manifest: *const onda_source_manifest,
    index: i32,
    candidate_index: i32,
) -> *const c_char {
    if candidate_index < 0 {
        return ptr::null();
    }
    map_unresolved_source_resolution(manifest, index, |value| {
        value
            .candidates
            .get(candidate_index as usize)
            .map_or(ptr::null(), |candidate| candidate.as_ptr())
    })
    .unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn onda_source_manifest_destroy(manifest: *mut onda_source_manifest) {
    if manifest.is_null() {
        return;
    }
    drop(Box::from_raw(manifest));
}

fn project_error_diag(error: impl ToString) -> onda_diag_t {
    diag_to_c(&Diagnostic::syntax(error.to_string(), 0, 0))
}

fn project_image_handle(
    image: ProjectImage,
) -> Result<*mut onda_project_image, onda_project::ProjectError> {
    let cstring = |value: &str, context: &str| {
        CString::new(value)
            .map_err(|_| onda_project::ProjectError::new(format!("{context} contains NUL")))
    };
    let content_digest = CString::new(image.content_digest_string())
        .map_err(|_| onda_project::ProjectError::new("project digest contains NUL"))?;
    let entry = cstring(&image.sources().entry, "project entry")?;
    let stdlib_digest = cstring(&image.sources().stdlib_digest, "standard-library digest")?;
    let document_paths = image
        .sources()
        .documents
        .iter()
        .map(|document| cstring(&document.path, "project document path"))
        .collect::<Result<Vec<_>, _>>()?;
    let resolution_sources = image
        .sources()
        .resolutions
        .iter()
        .map(|resolution| cstring(&resolution.source, "project resolution source"))
        .collect::<Result<Vec<_>, _>>()?;
    let resolution_specifiers = image
        .sources()
        .resolutions
        .iter()
        .map(|resolution| cstring(&resolution.specifier, "project resolution specifier"))
        .collect::<Result<Vec<_>, _>>()?;
    let resolution_targets = image
        .sources()
        .resolutions
        .iter()
        .map(|resolution| cstring(&resolution.target, "project resolution target"))
        .collect::<Result<Vec<_>, _>>()?;
    let buffers = image
        .buffer_bindings()
        .iter()
        .map(|(name, asset_id)| {
            let asset = image.assets().get(asset_id).ok_or_else(|| {
                onda_project::ProjectError::new(format!(
                    "project buffer '{name}' references a missing asset"
                ))
            })?;
            Ok(CProjectBufferInfo {
                name: cstring(name, "project buffer name")?,
                asset_id: cstring(asset_id.as_str(), "project asset ID")?,
                element_type: buffer_element_to_c(asset.element()),
                frames: asset.frames,
                channels: asset.channels,
                sample_rate: asset.sample_rate,
            })
        })
        .collect::<Result<Vec<_>, onda_project::ProjectError>>()?;
    Ok(Box::into_raw(Box::new(onda_project_image {
        inner: Arc::new(image),
        content_digest,
        entry,
        stdlib_digest,
        document_paths,
        resolution_sources,
        resolution_specifiers,
        resolution_targets,
        buffers,
    })))
}

fn buffer_element_from_c(value: i32) -> Option<BufferElement> {
    match value {
        ONDA_PRIMITIVE_F32 => Some(BufferElement::F32),
        ONDA_PRIMITIVE_F64 => Some(BufferElement::F64),
        ONDA_PRIMITIVE_I32 => Some(BufferElement::I32),
        ONDA_PRIMITIVE_I64 => Some(BufferElement::I64),
        ONDA_PRIMITIVE_BOOL => Some(BufferElement::Bool),
        _ => None,
    }
}

fn buffer_element_to_c(value: BufferElement) -> i32 {
    match value {
        BufferElement::F32 => ONDA_PRIMITIVE_F32,
        BufferElement::F64 => ONDA_PRIMITIVE_F64,
        BufferElement::I32 => ONDA_PRIMITIVE_I32,
        BufferElement::I64 => ONDA_PRIMITIVE_I64,
        BufferElement::Bool => ONDA_PRIMITIVE_BOOL,
    }
}

unsafe fn native_buffer_samples(
    element: BufferElement,
    bytes: &[u8],
) -> Result<BufferSamples, onda_project::ProjectError> {
    fn read_values<T: Copy>(bytes: &[u8]) -> Vec<T> {
        bytes
            .chunks_exact(std::mem::size_of::<T>())
            .map(|chunk| unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<T>()) })
            .collect()
    }
    if !bytes.len().is_multiple_of(element.byte_size()) {
        return Err(onda_project::ProjectError::new(
            "buffer sample bytes are not aligned to the element type",
        ));
    }
    Ok(match element {
        BufferElement::Bool => BufferSamples::Bool(bytes.to_vec()),
        BufferElement::I32 => BufferSamples::I32(read_values(bytes)),
        BufferElement::I64 => BufferSamples::I64(read_values(bytes)),
        BufferElement::F32 => BufferSamples::F32(read_values(bytes)),
        BufferElement::F64 => BufferSamples::F64(read_values(bytes)),
    })
}

fn native_sample_bytes(samples: &BufferSamples) -> &[u8] {
    unsafe {
        match samples {
            BufferSamples::Bool(values) => values,
            BufferSamples::I32(values) => slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * std::mem::size_of::<i32>(),
            ),
            BufferSamples::I64(values) => slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * std::mem::size_of::<i64>(),
            ),
            BufferSamples::F32(values) => slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * std::mem::size_of::<f32>(),
            ),
            BufferSamples::F64(values) => slice::from_raw_parts(
                values.as_ptr().cast::<u8>(),
                values.len() * std::mem::size_of::<f64>(),
            ),
        }
    }
}

unsafe fn copy_sized_result(bytes: &[u8], output: *mut c_void, capacity: usize) -> Result<i64, ()> {
    let required = i64::try_from(bytes.len()).map_err(|_| ())?;
    if output.is_null() || capacity < bytes.len() {
        return Ok(required);
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
    Ok(required)
}

#[no_mangle]
pub extern "C" fn onda_project_image_format_version() -> i32 {
    onda_project::ONDA_PROJECT_IMAGE_FORMAT_VERSION as i32
}

#[no_mangle]
pub extern "C" fn onda_buffer_asset_format_version() -> i32 {
    onda_project::ONDA_BUFFER_FORMAT_VERSION as i32
}

#[no_mangle]
pub extern "C" fn onda_current_stdlib_digest() -> *const c_char {
    static DIGEST: OnceLock<CString> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            CString::new(onda_project::current_stdlib_digest()).expect("digest has no NUL")
        })
        .as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_asset_encode(
    element_type: i32,
    frames: u32,
    channels: u32,
    sample_rate: f32,
    samples: *const c_void,
    sample_bytes: usize,
    out_bytes: *mut c_void,
    out_capacity: usize,
    out_diag: *mut onda_diag_t,
) -> i64 {
    if sample_bytes > 0 && samples.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return -1;
    }
    let Some(element) = buffer_element_from_c(element_type) else {
        write_diag(out_diag, project_error_diag("invalid buffer element type"));
        return -1;
    };
    let limits = ProjectLimits::default();
    if sample_bytes > limits.max_asset_bytes {
        write_diag(
            out_diag,
            project_error_diag(format!(
                "buffer sample payload exceeds the {} byte limit",
                limits.max_asset_bytes
            )),
        );
        return -1;
    }
    let input = if sample_bytes == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(samples.cast::<u8>(), sample_bytes)
    };
    let samples = match native_buffer_samples(element, input) {
        Ok(samples) => samples,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return -1;
        }
    };
    let asset = match BufferAsset::new(frames, channels, sample_rate, samples) {
        Ok(asset) => asset,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return -1;
        }
    };
    let encoded = match encode_ondabuffer(&asset) {
        Ok(encoded) => encoded,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return -1;
        }
    };
    copy_sized_result(&encoded, out_bytes, out_capacity).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_buffer_asset_decode(
    bytes: *const c_void,
    byte_count: usize,
    out_info: *mut onda_buffer_asset_info_t,
    out_samples: *mut c_void,
    out_capacity: usize,
    out_diag: *mut onda_diag_t,
) -> i64 {
    if byte_count > 0 && bytes.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return -1;
    }
    let encoded = if byte_count == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(bytes.cast::<u8>(), byte_count)
    };
    let asset = match decode_ondabuffer(encoded, ProjectLimits::default()) {
        Ok(asset) => asset,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return -1;
        }
    };
    let samples = native_sample_bytes(&asset.samples);
    if !out_info.is_null() {
        ptr::write(
            out_info,
            onda_buffer_asset_info_t {
                element_type: buffer_element_to_c(asset.element()),
                frames: asset.frames,
                channels: asset.channels,
                sample_rate: asset.sample_rate,
                sample_bytes: samples.len(),
            },
        );
    }
    copy_sized_result(samples, out_samples, out_capacity).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_capture(
    entry_path_utf8: *const c_char,
    source_root_utf8: *const c_char,
    manifest: *const onda_source_manifest,
    buffers: *const onda_project_buffer_asset_t,
    buffer_count: usize,
    out_diag: *mut onda_diag_t,
) -> *mut onda_project_image {
    if entry_path_utf8.is_null()
        || source_root_utf8.is_null()
        || manifest.is_null()
        || (buffer_count > 0 && buffers.is_null())
    {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let entry = match parse_required_c_string(entry_path_utf8, "project entry path") {
        Ok(value) => value,
        Err(error) => {
            write_diag(out_diag, diag_to_c(&error));
            return ptr::null_mut();
        }
    };
    let source_root = match parse_required_c_string(source_root_utf8, "project source root") {
        Ok(value) => value,
        Err(error) => {
            write_diag(out_diag, diag_to_c(&error));
            return ptr::null_mut();
        }
    };
    let sources = match SourceImage::capture(
        Path::new(&entry),
        Path::new(&source_root),
        &(*manifest).inner,
        ProjectLimits::default(),
    ) {
        Ok(sources) => sources,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    let limits = ProjectLimits::default();
    if buffer_count > limits.max_buffer_bindings {
        write_diag(
            out_diag,
            project_error_diag(format!(
                "project contains {buffer_count} buffers, exceeding the {} binding limit",
                limits.max_buffer_bindings
            )),
        );
        return ptr::null_mut();
    }
    let mut buffer_bindings = BTreeMap::new();
    let mut assets = BTreeMap::<AssetId, BufferAsset>::new();
    let mut total_asset_bytes = 0usize;
    if buffer_count > 0 {
        for buffer in slice::from_raw_parts(buffers, buffer_count) {
            let name = match parse_required_c_string(buffer.name_utf8, "project buffer name") {
                Ok(value) => value,
                Err(error) => {
                    write_diag(out_diag, diag_to_c(&error));
                    return ptr::null_mut();
                }
            };
            if buffer_bindings.contains_key(&name) {
                write_diag(
                    out_diag,
                    project_error_diag(format!("duplicate project buffer '{name}'")),
                );
                return ptr::null_mut();
            }
            if buffer.ondabuffer_byte_count > 0 && buffer.ondabuffer_bytes.is_null() {
                write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
                return ptr::null_mut();
            }
            let bytes = if buffer.ondabuffer_byte_count == 0 {
                &[][..]
            } else {
                slice::from_raw_parts(
                    buffer.ondabuffer_bytes.cast::<u8>(),
                    buffer.ondabuffer_byte_count,
                )
            };
            let validated = match validate_ondabuffer(bytes, limits) {
                Ok(validated) => validated,
                Err(error) => {
                    write_diag(out_diag, project_error_diag(error));
                    return ptr::null_mut();
                }
            };
            let id = AssetId::from_buffer_digest(validated.content_digest());
            if !assets.contains_key(&id) {
                let asset =
                    match validated.decode_with_remaining_asset_budget(limits, total_asset_bytes) {
                        Ok(asset) => asset,
                        Err(error) => {
                            write_diag(out_diag, project_error_diag(error));
                            return ptr::null_mut();
                        }
                    };
                total_asset_bytes = match total_asset_bytes.checked_add(asset.payload_bytes()) {
                    Some(total) if total <= limits.max_total_asset_bytes => total,
                    Some(_) => {
                        write_diag(
                            out_diag,
                            project_error_diag(format!(
                                "project buffer payloads exceed the {} byte limit",
                                limits.max_total_asset_bytes
                            )),
                        );
                        return ptr::null_mut();
                    }
                    None => {
                        write_diag(
                            out_diag,
                            project_error_diag("project buffer byte total overflows"),
                        );
                        return ptr::null_mut();
                    }
                };
                assets.insert(id.clone(), asset);
            }
            buffer_bindings.insert(name, id);
        }
    }
    let image = match ProjectImage::new(sources, buffer_bindings, assets) {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    match project_image_handle(image) {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_deserialize(
    bytes: *const c_void,
    byte_count: usize,
    out_diag: *mut onda_diag_t,
) -> *mut onda_project_image {
    if byte_count > 0 && bytes.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let bytes = if byte_count == 0 {
        &[][..]
    } else {
        slice::from_raw_parts(bytes.cast::<u8>(), byte_count)
    };
    let image = match ProjectImage::deserialize(bytes, ProjectLimits::default()) {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    match project_image_handle(image) {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_load_files(
    files: *const onda_project_file_t,
    file_count: usize,
    project_file_path_utf8: *const c_char,
    out_diag: *mut onda_diag_t,
) -> *mut onda_project_image {
    if file_count == 0 || files.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let limits = ProjectLimits::default();
    let selected_manifest = if project_file_path_utf8.is_null() {
        None
    } else {
        match parse_required_c_string(project_file_path_utf8, "selected project manifest path") {
            Ok(path) => Some(path),
            Err(error) => {
                write_diag(out_diag, diag_to_c(&error));
                return ptr::null_mut();
            }
        }
    };
    if file_count > limits.max_materialized_file_count() {
        write_diag(
            out_diag,
            project_error_diag(format!(
                "materialized project contains {file_count} files, exceeding the {} file limit",
                limits.max_materialized_file_count()
            )),
        );
        return ptr::null_mut();
    }
    let mut materialized = BTreeMap::<String, &[u8]>::new();
    for file in slice::from_raw_parts(files, file_count) {
        let path = match parse_required_c_string(file.path_utf8, "project file path") {
            Ok(path) => path,
            Err(error) => {
                write_diag(out_diag, diag_to_c(&error));
                return ptr::null_mut();
            }
        };
        if file.byte_count > 0 && file.bytes.is_null() {
            write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
            return ptr::null_mut();
        }
        let bytes = if file.byte_count == 0 {
            &[][..]
        } else {
            slice::from_raw_parts(file.bytes.cast::<u8>(), file.byte_count)
        };
        if materialized.insert(path.clone(), bytes).is_some() {
            write_diag(
                out_diag,
                project_error_diag(format!("duplicate project file '{path}'")),
            );
            return ptr::null_mut();
        }
    }
    let loaded_image = match selected_manifest.as_deref() {
        Some(manifest_path) => ProjectImage::from_materialized_file_slices_with_manifest(
            &materialized,
            manifest_path,
            limits,
        ),
        None => ProjectImage::from_materialized_file_slices(&materialized, limits),
    };
    let image = match loaded_image {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    match project_image_handle(image) {
        Ok(image) => image,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_serialize(
    image: *const onda_project_image,
    out_bytes: *mut c_void,
    out_capacity: usize,
    out_diag: *mut onda_diag_t,
) -> i64 {
    if image.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return -1;
    }
    let bytes = match (*image).inner.serialize() {
        Ok(bytes) => bytes,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return -1;
        }
    };
    copy_sized_result(&bytes, out_bytes, out_capacity).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_content_digest(
    image: *const onda_project_image,
) -> *const c_char {
    if image.is_null() {
        return ptr::null();
    }
    (*image).content_digest.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_entry(
    image: *const onda_project_image,
) -> *const c_char {
    if image.is_null() {
        return ptr::null();
    }
    (*image).entry.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_stdlib_digest(
    image: *const onda_project_image,
) -> *const c_char {
    if image.is_null() {
        return ptr::null();
    }
    (*image).stdlib_digest.as_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_document_count(
    image: *const onda_project_image,
) -> i32 {
    if image.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*image).inner.sources().documents.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_document_path(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    let image = &*image;
    image
        .document_paths
        .get(index as usize)
        .map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_document_contents(
    image: *const onda_project_image,
    index: i32,
    out_bytes: *mut usize,
) -> *const c_char {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    let image = &*image;
    let Some(document) = image.inner.sources().documents.get(index as usize) else {
        return ptr::null();
    };
    if !out_bytes.is_null() {
        *out_bytes = document.contents.len();
    }
    document.contents.as_ptr().cast::<c_char>()
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_count(
    image: *const onda_project_image,
) -> i32 {
    if image.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*image).inner.sources().resolutions.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_source(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    let image = &*image;
    image
        .resolution_sources
        .get(index as usize)
        .map_or(ptr::null(), |source| source.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_kind(
    image: *const onda_project_image,
    index: i32,
) -> i32 {
    if image.is_null() || index < 0 {
        return -1;
    }
    let image = &*image;
    image
        .inner
        .sources()
        .resolutions
        .get(index as usize)
        .map_or(-1, |resolution| match resolution.kind {
            onda_project::SourceReferenceKind::Include => ONDA_SOURCE_REFERENCE_INCLUDE,
            onda_project::SourceReferenceKind::Import => ONDA_SOURCE_REFERENCE_IMPORT,
        })
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_specifier(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    let image = &*image;
    image
        .resolution_specifiers
        .get(index as usize)
        .map_or(ptr::null(), |specifier| specifier.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_resolution_target(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    let image = &*image;
    image
        .resolution_targets
        .get(index as usize)
        .map_or(ptr::null(), |target| target.as_ptr())
}

unsafe fn project_image_buffer(
    image: *const onda_project_image,
    index: i32,
) -> *const CProjectBufferInfo {
    if image.is_null() || index < 0 {
        return ptr::null();
    }
    (&*image)
        .buffers
        .get(index as usize)
        .map_or(ptr::null(), |buffer| buffer)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_count(image: *const onda_project_image) -> i32 {
    if image.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*image).buffers.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_name(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        ptr::null()
    } else {
        (*buffer).name.as_ptr()
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_asset_id(
    image: *const onda_project_image,
    index: i32,
) -> *const c_char {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        ptr::null()
    } else {
        (*buffer).asset_id.as_ptr()
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_element_type(
    image: *const onda_project_image,
    index: i32,
) -> i32 {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        -1
    } else {
        (*buffer).element_type
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_frames(
    image: *const onda_project_image,
    index: i32,
) -> i64 {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        -1
    } else {
        i64::from((*buffer).frames)
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_channels(
    image: *const onda_project_image,
    index: i32,
) -> i64 {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        -1
    } else {
        i64::from((*buffer).channels)
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_buffer_sample_rate(
    image: *const onda_project_image,
    index: i32,
) -> f32 {
    let buffer = project_image_buffer(image, index);
    if buffer.is_null() {
        f32::NAN
    } else {
        (*buffer).sample_rate
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_compile(
    image: *const onda_project_image,
    options: *const onda_compile_options_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_program {
    if image.is_null() || options.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let options = &*options;
    if let Err(message) = validate_compile_options(options) {
        write_diag(out_diag, static_runtime_diag(message));
        return ptr::null_mut();
    }
    let image = Arc::clone(&(*image).inner);
    let loaded = match image.sources().replay(ProjectLimits::default()) {
        Ok(loaded) => loaded,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    compile_parsed_program(loaded.program, options, Some(image), out_diag)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_materialize(
    image: *const onda_project_image,
    out_diag: *mut onda_diag_t,
) -> *mut onda_project_materialization_plan {
    if image.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }
    let plan = match (*image).inner.materialization_plan() {
        Ok(plan) => plan,
        Err(error) => {
            write_diag(out_diag, project_error_diag(error));
            return ptr::null_mut();
        }
    };
    let file_paths = match plan
        .files
        .iter()
        .map(|file| CString::new(file.relative_path.as_str()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(paths) => paths,
        Err(_) => {
            write_diag(out_diag, project_error_diag("project path contains NUL"));
            return ptr::null_mut();
        }
    };
    Box::into_raw(Box::new(onda_project_materialization_plan {
        inner: plan,
        file_paths,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_materialization_file_count(
    plan: *const onda_project_materialization_plan,
) -> i32 {
    if plan.is_null() {
        return -1;
    }
    saturating_usize_to_i32((*plan).inner.files.len())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_materialization_file_path(
    plan: *const onda_project_materialization_plan,
    index: i32,
) -> *const c_char {
    if plan.is_null() || index < 0 {
        return ptr::null();
    }
    let plan = &*plan;
    plan.file_paths
        .get(index as usize)
        .map_or(ptr::null(), |path| path.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_materialization_file_bytes(
    plan: *const onda_project_materialization_plan,
    index: i32,
    out_bytes: *mut c_void,
    out_capacity: usize,
) -> i64 {
    if plan.is_null() || index < 0 {
        return -1;
    }
    let plan = &*plan;
    let Some(file) = plan.inner.files.get(index as usize) else {
        return -1;
    };
    copy_sized_result(&file.bytes, out_bytes, out_capacity).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_materialization_destroy(
    plan: *mut onda_project_materialization_plan,
) {
    if !plan.is_null() {
        drop(Box::from_raw(plan));
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_project_image_destroy(image: *mut onda_project_image) {
    if !image.is_null() {
        drop(Box::from_raw(image));
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_create(
    program: *const onda_program,
    in_channels: i32,
    out_channels: i32,
    out_diag: *mut onda_diag_t,
) -> *mut onda_instance {
    onda_instance_create_impl(program, in_channels, out_channels, None, out_diag)
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_create_with_allocator(
    program: *const onda_program,
    in_channels: i32,
    out_channels: i32,
    allocator: *const onda_allocator_t,
    out_diag: *mut onda_diag_t,
) -> *mut onda_instance {
    let allocator = match runtime_allocator_from_c(allocator) {
        Ok(allocator) => allocator,
        Err(diag) => {
            write_diag(out_diag, diag);
            return ptr::null_mut();
        }
    };
    onda_instance_create_impl(
        program,
        in_channels,
        out_channels,
        Some(allocator),
        out_diag,
    )
}

unsafe fn onda_instance_create_impl(
    program: *const onda_program,
    in_channels: i32,
    out_channels: i32,
    allocator: Option<RuntimeAllocator>,
    out_diag: *mut onda_diag_t,
) -> *mut onda_instance {
    if program.is_null() {
        write_diag(out_diag, static_runtime_diag(STATIC_ERR_NULL_ARG));
        return ptr::null_mut();
    }

    if in_channels < 0 || out_channels < 0 {
        write_diag(
            out_diag,
            static_runtime_diag(b"invalid instance configuration\0"),
        );
        return ptr::null_mut();
    }

    let compiled = Arc::clone(&(&*program).inner);
    let config = InstanceConfig {
        sample_rate: compiled.jit.sample_rate(),
        frames_per_block: compiled.jit.block_size(),
        in_channels: in_channels as usize,
        out_channels: out_channels as usize,
    };

    let jit = compiled.jit.clone();
    let instance = match allocator {
        Some(allocator) => create_instance_with_allocator(jit, config, allocator),
        None => create_instance(jit, config),
    };
    let mut instance = match instance {
        Ok(i) => i,
        Err(e) => {
            write_diag(out_diag, diag_to_c(&e));
            return ptr::null_mut();
        }
    };

    if let Some(defaults) = &compiled.project_defaults {
        if let Err(error) = bind_project_defaults(&mut instance, defaults) {
            write_diag(out_diag, diag_to_c(&error));
            return ptr::null_mut();
        }
    }

    allocate_instance_handle(instance, compiled, allocator, out_diag)
}

#[no_mangle]
pub unsafe extern "C" fn onda_instance_destroy(instance: *mut onda_instance) {
    if instance.is_null() {
        return;
    }
    let allocator = (*instance).allocation.allocator;
    if let Some(allocator) = allocator {
        ptr::drop_in_place(instance);
        let layout = Layout::new::<onda_instance>();
        allocator.deallocate(instance.cast::<c_void>(), layout.size(), layout.align());
    } else {
        drop(Box::from_raw(instance));
    }
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
pub unsafe extern "C" fn onda_set_param_plain_f64(
    instance: *mut onda_instance,
    index: i32,
    plain: f64,
) -> i32 {
    if instance.is_null() || index < 0 {
        return -1;
    }
    match runtime_set_param_plain_f64(&mut (*instance).inner, index as usize, plain) {
        Ok(()) => 0,
        Err(_) => -3,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_set_param_normalized(
    instance: *mut onda_instance,
    index: i32,
    normalized: f64,
) -> i32 {
    if instance.is_null() || index < 0 {
        return -1;
    }
    match runtime_set_param_normalized(&mut (*instance).inner, index as usize, normalized) {
        Ok(()) => 0,
        Err(_) => -3,
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
        Err(_) => -2,
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
pub unsafe extern "C" fn onda_trigger_event_by_index_unchecked(
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
    execution_status_to_c(trigger_event_by_index_unchecked(
        &mut (*instance).inner,
        index as usize,
        payload,
    ))
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
    match unsafe { bind_input(&mut (*instance).inner, index as usize, ptr, bytes) } {
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
    match unsafe { bind_output(&mut (*instance).inner, index as usize, ptr, bytes) } {
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
    if instance.is_null() || index < 0 {
        return -1;
    }
    let Some(elem_ty) = primitive_type_from_i32(elem_type) else {
        return -1;
    };
    let (ptr, frames, channels) = if sample_rate == 0.0 {
        (std::ptr::null_mut(), 0, 0)
    } else {
        if frames < 0 || channels < 0 {
            return -1;
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
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_reset_buffer_to_project_default(
    instance: *mut onda_instance,
    index: i32,
) -> i32 {
    if instance.is_null() || index < 0 {
        return -1;
    }
    let instance = &mut *instance;
    let program = Arc::clone(&instance.program);
    let Some(defaults) = &program.project_defaults else {
        return -2;
    };
    match bind_project_default(&mut instance.inner, defaults, index as usize) {
        Ok(true) => 0,
        Ok(false) | Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_checked(instance: *mut onda_instance, frames: i32) -> i32 {
    if instance.is_null() || frames < 0 {
        return -1;
    }
    match process_checked(&mut (*instance).inner, frames as usize) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_checked_segment(
    instance: *mut onda_instance,
    start_frame: i32,
    frames: i32,
    flags: i32,
) -> i32 {
    if instance.is_null() || start_frame < 0 || frames < 0 || flags < 0 {
        return -1;
    }
    match process_checked_segment(
        &mut (*instance).inner,
        start_frame as usize,
        frames as usize,
        flags as u32,
    ) {
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
        return -1;
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
        return -1;
    }
    if byte_count > 0 && bytes.is_null() {
        return -1;
    }
    let snapshot = if byte_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(bytes.cast::<u8>(), byte_count as usize)
    };
    match (*instance).inner.restore_state_bytes(snapshot) {
        Ok(_) => 0,
        Err(_) => -2,
    }
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
    execution_status_to_c(process_unchecked(&mut (*instance).inner))
}

#[no_mangle]
pub unsafe extern "C" fn onda_prepare_unchecked_process(instance: *mut onda_instance) -> i32 {
    if instance.is_null() {
        return -1;
    }
    match prepare_unchecked_process(&mut (*instance).inner) {
        Ok(_) => 0,
        Err(_) => -2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn onda_process_unchecked_segment(
    instance: *mut onda_instance,
    start_frame: i32,
    frames: i32,
    flags: i32,
) -> i32 {
    if instance.is_null() || start_frame < 0 || frames < 0 || flags < 0 {
        return -1;
    }
    execution_status_to_c(process_unchecked_segment(
        &mut (*instance).inner,
        start_frame as usize,
        frames as usize,
        flags as u32,
    ))
}

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
    (&*program)
        .inner
        .jit
        .event_descriptor(event_index as usize)
        .and_then(|event| event.params().get(param_index as usize))
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
