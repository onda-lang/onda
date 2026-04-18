use std::collections::HashMap;
use std::sync::Arc;

use onda_frontend::{Diagnostic, PrimitiveType};
use onda_semantics::{TypedConstValue, TypedProgram, TypedValueRange};

mod aot_artifact;
mod metadata;
#[cfg(feature = "llvm-orc")]
mod orc_backend;
mod primitives;
mod runtime_validation;
mod target_config;

pub use aot_artifact::{AotMetadata, AotObjectArtifact};
pub use target_config::{
    CodegenOptions, TargetCodeModel, TargetConfig, TargetCpu, TargetOptLevel, TargetRelocMode,
};

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
    pub opt_level: TargetOptLevel,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 512,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
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
    default_bytes: Option<Vec<u8>>,
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
    default_bytes: Option<Vec<u8>>,
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
            options.opt_level,
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
            options.opt_level,
        ),
    }
}

pub fn lower_to_target_llvm_ir_with_options(
    typed: TypedProgram,
    options: CodegenOptions,
) -> Result<String, Vec<Diagnostic>> {
    runtime_validation::validate_codegen_options(&options).map_err(|diag| vec![diag])?;
    emit_targeted_ir(
        typed,
        options.sample_rate,
        options.block_size,
        options.fast_math,
        options.target,
    )
}

pub fn lower_to_object_with_options(
    typed: TypedProgram,
    options: CodegenOptions,
) -> Result<AotObjectArtifact, Vec<Diagnostic>> {
    runtime_validation::validate_codegen_options(&options).map_err(|diag| vec![diag])?;
    emit_targeted_object(
        typed,
        options.sample_rate,
        options.block_size,
        options.fast_math,
        options.target,
    )
}

#[cfg(feature = "llvm-orc")]
fn build_orc_program(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: TargetOptLevel,
) -> Result<JitProgram, Vec<Diagnostic>> {
    let compiled = orc_backend::compile_orc(&typed, sample_rate, block_size, fast_math, opt_level)
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
    opt_level: TargetOptLevel,
) -> Result<String, Vec<Diagnostic>> {
    orc_backend::emit_optimized_ir(&typed, sample_rate, block_size, fast_math, opt_level)
        .map_err(|diag| vec![diag])
}

#[cfg(feature = "llvm-orc")]
fn emit_targeted_ir(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: TargetConfig,
) -> Result<String, Vec<Diagnostic>> {
    orc_backend::emit_targeted_ir(&typed, sample_rate, block_size, fast_math, &target)
        .map_err(|diag| vec![diag])
}

#[cfg(feature = "llvm-orc")]
fn emit_targeted_object(
    typed: TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: TargetConfig,
) -> Result<AotObjectArtifact, Vec<Diagnostic>> {
    orc_backend::emit_targeted_object(&typed, sample_rate, block_size, fast_math, &target)
        .map_err(|diag| vec![diag])
}

#[cfg(not(feature = "llvm-orc"))]
fn emit_orc_ir(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
    _opt_level: TargetOptLevel,
) -> Result<String, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but onda_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(not(feature = "llvm-orc"))]
fn emit_targeted_ir(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
    _target: TargetConfig,
) -> Result<String, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "LLVM backend is required but onda_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(not(feature = "llvm-orc"))]
fn emit_targeted_object(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
    _target: TargetConfig,
) -> Result<AotObjectArtifact, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "LLVM backend is required but onda_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(not(feature = "llvm-orc"))]
fn build_orc_program(
    _typed: TypedProgram,
    _sample_rate: f32,
    _block_size: usize,
    _fast_math: bool,
    _opt_level: TargetOptLevel,
) -> Result<JitProgram, Vec<Diagnostic>> {
    Err(vec![Diagnostic::internal(
        "ORC backend is required but onda_codegen_llvm was built without 'llvm-orc' feature",
    )])
}

#[cfg(all(test, feature = "llvm-orc"))]
mod tests {
    use super::*;

    use onda_frontend::parse_program;
    use onda_semantics::{analyze_with_options, AnalysisOptions};

    fn typed_program(src: &str) -> TypedProgram {
        let program = parse_program(src).expect("source should parse");
        analyze_with_options(program, AnalysisOptions::default()).expect("source should analyze")
    }

    fn is_missing_target_backend(diags: &[Diagnostic]) -> bool {
        diags.iter().any(|diag| {
            diag.message.contains("LLVMGetTargetFromTriple failed")
                || diag
                    .message
                    .contains("No available targets are compatible with triple")
        })
    }

    #[test]
    fn emits_cross_target_object_when_backend_is_available() {
        let typed = typed_program(
            r#"
outs:
  out1

sample:
  out1 = 0.0
"#,
        );

        let result = lower_to_object_with_options(
            typed,
            CodegenOptions {
                sample_rate: 48_000.0,
                block_size: 128,
                fast_math: false,
                target: TargetConfig::for_triple("aarch64-unknown-linux-gnu"),
            },
        );

        match result {
            Ok(artifact) => {
                assert_eq!(artifact.metadata.target.triple, "aarch64-unknown-linux-gnu");
                assert!(!artifact.object_bytes.is_empty());
            }
            Err(diags) if is_missing_target_backend(&diags) => {
                eprintln!(
                    "skipping cross-target AOT smoke test because the linked LLVM build does not include AArch64"
                );
            }
            Err(diags) => panic!("cross-target AOT smoke test failed: {diags:#?}"),
        }
    }
}
