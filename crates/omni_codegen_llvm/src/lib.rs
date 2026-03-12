use std::collections::HashMap;
use std::sync::Arc;

use omni_frontend::{Diagnostic, PrimitiveType};
use omni_semantics::{TypedConstValue, TypedProgram, TypedValueRange};

mod metadata;
#[cfg(feature = "llvm-orc")]
mod orc_backend;
mod primitives;
mod runtime_validation;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionBackend {
    Auto,
    OrcJit,
}

#[derive(Debug, Clone, Copy)]
pub struct CompileOptions {
    pub backend: ExecutionBackend,
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 512,
            fast_math: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JitProgram {
    pub typed: Arc<TypedProgram>,
    sample_rate: f32,
    block_size: usize,
    inputs: Arc<Vec<DeclaredIo>>,
    outputs: Arc<Vec<DeclaredIo>>,
    params: Arc<Vec<DeclaredIo>>,
    events: Arc<Vec<DeclaredEvent>>,
    buffers: Arc<Vec<DeclaredBuffer>>,
    input_index: Arc<HashMap<String, usize>>,
    output_index: Arc<HashMap<String, usize>>,
    param_index: Arc<HashMap<String, usize>>,
    event_index: Arc<HashMap<String, usize>>,
    buffer_index: Arc<HashMap<String, usize>>,
    #[cfg(feature = "llvm-orc")]
    compiled: Arc<orc_backend::OrcProcess>,
}

#[cfg_attr(not(feature = "llvm-orc"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
    pub(crate) state_words: Vec<u64>,
    pub(crate) state_size_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DeclaredIo {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    slot_offset: usize,
    byte_offset: usize,
    default: Option<TypedConstValue>,
    range: Option<TypedValueRange>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeclaredBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct DeclaredBuffer {
    name: String,
    elem_ty: PrimitiveType,
    channels: DeclaredBufferChannels,
    may_write: bool,
}

#[derive(Debug, Clone)]
pub struct DeclaredEvent {
    name: String,
    params: Vec<DeclaredEventParam>,
    payload_bytes: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct DeclaredEventParam {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_slice: bool,
    byte_offset: usize,
}

pub fn lower_and_jit(typed: TypedProgram) -> Result<JitProgram, Vec<Diagnostic>> {
    lower_and_jit_with_options(typed, CompileOptions::default())
}

pub fn lower_and_jit_with_options(
    typed: TypedProgram,
    options: CompileOptions,
) -> Result<JitProgram, Vec<Diagnostic>> {
    runtime_validation::validate_compile_options(&options).map_err(|diag| vec![diag])?;
    match options.backend {
        ExecutionBackend::Auto | ExecutionBackend::OrcJit => build_orc_program(
            typed,
            options.sample_rate,
            options.block_size,
            options.fast_math,
        ),
    }
}

pub fn lower_to_llvm_ir_with_options(
    typed: TypedProgram,
    options: CompileOptions,
) -> Result<String, Vec<Diagnostic>> {
    runtime_validation::validate_compile_options(&options).map_err(|diag| vec![diag])?;
    match options.backend {
        ExecutionBackend::Auto | ExecutionBackend::OrcJit => emit_orc_ir(
            typed,
            options.sample_rate,
            options.block_size,
            options.fast_math,
        ),
    }
}

#[cfg(feature = "llvm-orc")]
fn build_orc_program(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<JitProgram, Vec<Diagnostic>> {
    let compiled = orc_backend::compile_orc(&typed, sample_rate, block_size, fast_math)
        .map_err(|diag| vec![diag])?;
    let metadata = metadata::build_program_metadata(&typed);

    Ok(JitProgram {
        typed: Arc::new(typed),
        sample_rate,
        block_size,
        input_index: Arc::new(metadata.input_index),
        output_index: Arc::new(metadata.output_index),
        param_index: Arc::new(metadata.param_index),
        event_index: Arc::new(metadata.event_index),
        buffer_index: Arc::new(metadata.buffer_index),
        inputs: Arc::new(metadata.inputs),
        outputs: Arc::new(metadata.outputs),
        params: Arc::new(metadata.params),
        events: Arc::new(metadata.events),
        buffers: Arc::new(metadata.buffers),
        compiled: Arc::new(compiled),
    })
}

#[cfg(feature = "llvm-orc")]
fn emit_orc_ir(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<String, Vec<Diagnostic>> {
    orc_backend::emit_optimized_ir(&typed, sample_rate, block_size, fast_math)
        .map_err(|diag| vec![diag])
}

#[cfg(not(feature = "llvm-orc"))]
fn emit_orc_ir(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
) -> Result<String, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but omni_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(not(feature = "llvm-orc"))]
fn build_orc_program(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
) -> Result<JitProgram, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but omni_codegen_llvm was built without 'llvm-orc' feature",
    )])
}
