//! LLVM execution and object-code backend for validated Onda MIR.

use std::alloc::Layout;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
#[cfg(feature = "llvm-orc")]
use std::rc::Rc;
use std::sync::Arc;

use onda_frontend::{Diagnostic, PrimitiveType};
use onda_mir::{ParamControl, ScalarValue, ValueRange};
pub use onda_mir::{ParamDomain, ParamScale, ScalarType as ParamScalarType};

mod aot_artifact;
#[cfg(any(feature = "llvm-orc", test))]
mod mir_metadata;
#[cfg(feature = "llvm-orc")]
mod orc_backend;
mod primitives;
mod runtime_metadata;
mod runtime_validation;
mod target_config;

pub use aot_artifact::{
    AotMetadata, AotObjectArtifact, AotStateMetadata, AOT_METADATA_FORMAT_VERSION,
    AOT_SNAPSHOT_FORMAT_VERSION, PROCESSOR_ABI_VERSION, PROCESSOR_ARTIFACT_FORMAT,
};
#[cfg(feature = "llvm-orc")]
pub use orc_backend::{
    lower_mir_and_jit, lower_mir_and_jit_with_options, lower_mir_to_llvm_ir,
    lower_mir_to_llvm_ir_with_options, lower_mir_to_object, lower_mir_to_object_artifact,
    lower_mir_to_target_llvm_ir, lower_optimized_mir_and_jit,
    lower_optimized_mir_and_jit_with_options, lower_optimized_mir_to_llvm_ir,
    lower_optimized_mir_to_llvm_ir_with_options, lower_optimized_mir_to_object,
    lower_optimized_mir_to_object_artifact, lower_optimized_mir_to_target_llvm_ir, MirCodegenError,
    MirCodegenErrorKind, MirCompileOptions, MirEventPayloadShape, MirJitProgram, MirTargetOptions,
};
pub use target_config::{
    TargetCodeModel, TargetConfig, TargetCpu, TargetOptLevel, TargetRelocMode,
};

#[derive(Debug, Clone)]
pub struct JitProgram {
    sample_rate: f32,
    block_size: usize,
    inputs: Arc<Vec<DeclaredIo>>,
    outputs: Arc<Vec<DeclaredIo>>,
    control_outputs: Arc<Vec<DeclaredIo>>,
    params: Arc<Vec<DeclaredIo>>,
    events: Arc<Vec<DeclaredEvent>>,
    buffers: Arc<Vec<DeclaredBuffer>>,
    input_index: Arc<HashMap<String, usize>>,
    output_index: Arc<HashMap<String, usize>>,
    control_output_index: Arc<HashMap<String, usize>>,
    param_index: Arc<HashMap<String, usize>>,
    event_index: Arc<HashMap<String, usize>>,
    buffer_index: Arc<HashMap<String, usize>>,
    state_entries: Arc<Vec<DeclaredState>>,
    snapshot_segments: Arc<Vec<StateSnapshotSegment>>,
    snapshot_size_bytes: usize,
    #[cfg(feature = "llvm-orc")]
    compiled: Rc<orc_backend::MirJitProgram>,
}

#[derive(Debug, Clone, Copy)]
struct StateSnapshotSegment {
    snapshot_offset: usize,
    state_offset: usize,
    byte_size: usize,
    element_size: usize,
}

#[derive(Clone, Copy)]
pub struct RuntimeAllocator {
    pub context: *mut c_void,
    pub alloc: unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void,
    pub free: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize),
}

impl fmt::Debug for RuntimeAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeAllocator")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct RuntimeState {
    pub(crate) state_words: RuntimeBuffer<u64>,
    pub(crate) state_size_bytes: usize,
}

pub struct RuntimeBuffer<T: Copy> {
    storage: RuntimeBufferStorage<T>,
}

enum RuntimeBufferStorage<T: Copy> {
    Global(Vec<T>),
    Custom(CustomRuntimeBuffer<T>),
}

struct CustomRuntimeBuffer<T: Copy> {
    ptr: NonNull<T>,
    len: usize,
    allocator: RuntimeAllocator,
}

impl<T: Copy> RuntimeBuffer<T> {
    pub fn from_vec(vec: Vec<T>) -> Self {
        Self {
            storage: RuntimeBufferStorage::Global(vec),
        }
    }

    pub fn try_from_elem_in(
        len: usize,
        value: T,
        allocator: Option<RuntimeAllocator>,
    ) -> Result<Self, Diagnostic> {
        let Some(allocator) = allocator else {
            return Ok(Self::from_vec(vec![value; len]));
        };
        let buffer = CustomRuntimeBuffer::try_from_elem(len, value, allocator)?;
        Ok(Self {
            storage: RuntimeBufferStorage::Custom(buffer),
        })
    }

    pub fn try_from_slice_in(
        values: &[T],
        allocator: Option<RuntimeAllocator>,
    ) -> Result<Self, Diagnostic> {
        let Some(allocator) = allocator else {
            return Ok(Self::from_vec(values.to_vec()));
        };
        let buffer = CustomRuntimeBuffer::try_from_slice(values, allocator)?;
        Ok(Self {
            storage: RuntimeBufferStorage::Custom(buffer),
        })
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    pub fn as_slice(&self) -> &[T] {
        match &self.storage {
            RuntimeBufferStorage::Global(values) => values.as_slice(),
            RuntimeBufferStorage::Custom(values) => values.as_slice(),
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        match &mut self.storage {
            RuntimeBufferStorage::Global(values) => values.as_mut_slice(),
            RuntimeBufferStorage::Custom(values) => values.as_mut_slice(),
        }
    }

    pub fn as_ptr(&self) -> *const T {
        self.as_slice().as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.as_mut_slice().as_mut_ptr()
    }
}

impl<T: Copy> Default for RuntimeBuffer<T> {
    fn default() -> Self {
        Self::from_vec(Vec::new())
    }
}

impl<T: Copy> Deref for RuntimeBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T: Copy> DerefMut for RuntimeBuffer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for RuntimeBuffer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl<T: Copy> CustomRuntimeBuffer<T> {
    fn try_from_elem(
        len: usize,
        value: T,
        allocator: RuntimeAllocator,
    ) -> Result<Self, Diagnostic> {
        let layout = runtime_array_layout::<T>(len)?;
        let ptr = allocate_custom_runtime_buffer::<T>(allocator, layout)?;
        if len > 0 {
            for idx in 0..len {
                unsafe {
                    ptr::write(ptr.as_ptr().add(idx), value);
                }
            }
        }
        Ok(Self {
            ptr,
            len,
            allocator,
        })
    }

    fn try_from_slice(values: &[T], allocator: RuntimeAllocator) -> Result<Self, Diagnostic> {
        let layout = runtime_array_layout::<T>(values.len())?;
        let ptr = allocate_custom_runtime_buffer::<T>(allocator, layout)?;
        if !values.is_empty() {
            unsafe {
                ptr::copy_nonoverlapping(values.as_ptr(), ptr.as_ptr(), values.len());
            }
        }
        Ok(Self {
            ptr,
            len: values.len(),
            allocator,
        })
    }

    fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T: Copy> Drop for CustomRuntimeBuffer<T> {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        let Ok(layout) = Layout::array::<T>(self.len) else {
            return;
        };
        unsafe {
            (self.allocator.free)(
                self.allocator.context,
                self.ptr.as_ptr().cast::<c_void>(),
                layout.size(),
                layout.align(),
            );
        }
    }
}

fn runtime_array_layout<T>(len: usize) -> Result<Layout, Diagnostic> {
    Layout::array::<T>(len).map_err(|_| {
        Diagnostic::runtime("runtime allocation layout exceeds addressable size", 0, 0)
    })
}

fn allocate_custom_runtime_buffer<T>(
    allocator: RuntimeAllocator,
    layout: Layout,
) -> Result<NonNull<T>, Diagnostic> {
    if layout.size() == 0 {
        return Ok(NonNull::dangling());
    }
    let raw = unsafe { (allocator.alloc)(allocator.context, layout.size(), layout.align()) };
    let Some(ptr) = NonNull::new(raw.cast::<T>()) else {
        return Err(Diagnostic::runtime("runtime allocator returned null", 0, 0));
    };
    if !(ptr.as_ptr() as usize).is_multiple_of(layout.align()) {
        unsafe {
            (allocator.free)(allocator.context, raw, layout.size(), layout.align());
        }
        return Err(Diagnostic::runtime(
            "runtime allocator returned misaligned memory",
            0,
            0,
        ));
    }
    Ok(ptr)
}

#[derive(Debug, Clone)]
pub struct DeclaredState {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_array: bool,
    byte_offset: usize,
    storage_byte_offset: usize,
}

#[derive(Debug, Clone)]
pub struct DeclaredIo {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_array: bool,
    slot_offset: usize,
    byte_offset: usize,
    state_byte_offset: Option<usize>,
    default_values: Option<Vec<ScalarValue>>,
    default_bytes: Option<Vec<u8>>,
    range: Option<ValueRange>,
    control: Option<ParamControl>,
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
    access: onda_mir::AccessMode,
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
    is_array: bool,
    is_slice: bool,
    byte_offset: usize,
    default_bytes: Option<Vec<u8>>,
    default_values: Option<Vec<ScalarValue>>,
}

/// Compiles validated MIR into the full runtime-facing JIT program contract.
#[cfg(feature = "llvm-orc")]
pub fn jit_program_from_mir(program: onda_mir::Program) -> Result<JitProgram, Vec<Diagnostic>> {
    jit_program_from_mir_with_options(program, MirCompileOptions::default())
}

/// Compiles validated MIR with explicit backend policy while retaining MIR as
/// the sole semantic input to native code generation and runtime metadata.
#[cfg(feature = "llvm-orc")]
pub fn jit_program_from_mir_with_options(
    program: onda_mir::Program,
    options: MirCompileOptions,
) -> Result<JitProgram, Vec<Diagnostic>> {
    let compiled =
        orc_backend::lower_mir_and_jit_with_options(program, options).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| Diagnostic::internal(format!("MIR LLVM lowering failed: {error}")))
                .collect::<Vec<_>>()
        })?;
    wrap_mir_orc_program(compiled)
}

/// Compiles backend-neutral optimized MIR without repeating validation or MIR
/// optimization.
#[cfg(feature = "llvm-orc")]
pub fn jit_program_from_optimized_mir(
    program: onda_mir::OptimizedProgram,
) -> Result<JitProgram, Vec<Diagnostic>> {
    jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default())
}

/// Compiles optimized MIR with explicit backend policy.
#[cfg(feature = "llvm-orc")]
pub fn jit_program_from_optimized_mir_with_options(
    program: onda_mir::OptimizedProgram,
    options: MirCompileOptions,
) -> Result<JitProgram, Vec<Diagnostic>> {
    let compiled = orc_backend::lower_optimized_mir_and_jit_with_options(program, options)
        .map_err(mir_codegen_diagnostics)?;
    wrap_mir_orc_program(compiled)
}

#[cfg(feature = "llvm-orc")]
fn mir_codegen_diagnostics(errors: Vec<MirCodegenError>) -> Vec<Diagnostic> {
    errors
        .into_iter()
        .map(|error| Diagnostic::internal(format!("MIR LLVM lowering failed: {error}")))
        .collect()
}

#[cfg(feature = "llvm-orc")]
fn wrap_mir_orc_program(compiled: MirJitProgram) -> Result<JitProgram, Vec<Diagnostic>> {
    let event_fixed_sizes = (0..compiled.mir().interface.events.len())
        .map(|index| compiled.event_payload_byte_size(index))
        .collect::<Vec<_>>();
    let metadata = mir_metadata::build_mir_program_metadata(
        compiled.mir(),
        mir_metadata::MirMetadataLayoutView {
            state_offsets: compiled.state_byte_offsets(),
            param_offsets: compiled.param_byte_offsets(),
            control_output_offsets: compiled.control_output_storage_byte_offsets(),
            input_bases: compiled.input_channel_bases(),
            output_bases: compiled.output_channel_bases(),
            event_fixed_sizes: &event_fixed_sizes,
        },
    )
    .map_err(|error| {
        vec![Diagnostic::internal(format!(
            "MIR runtime metadata failed: {error}"
        ))]
    })?;

    let snapshot_segments = build_snapshot_segments(&metadata.state_entries);
    let snapshot_size_bytes = snapshot_segments
        .last()
        .map_or(0, |segment| segment.snapshot_offset + segment.byte_size);
    Ok(JitProgram {
        sample_rate: compiled.mir().config.sample_rate,
        block_size: compiled.mir().config.block_size as usize,
        input_index: Arc::new(metadata.input_index),
        output_index: Arc::new(metadata.output_index),
        control_output_index: Arc::new(metadata.control_output_index),
        param_index: Arc::new(metadata.param_index),
        event_index: Arc::new(metadata.event_index),
        buffer_index: Arc::new(metadata.buffer_index),
        inputs: Arc::new(metadata.inputs),
        outputs: Arc::new(metadata.outputs),
        control_outputs: Arc::new(metadata.control_outputs),
        params: Arc::new(metadata.params),
        events: Arc::new(metadata.events),
        buffers: Arc::new(metadata.buffers),
        state_entries: Arc::new(metadata.state_entries),
        snapshot_segments: Arc::new(snapshot_segments),
        snapshot_size_bytes,
        compiled: Rc::new(compiled),
    })
}

#[cfg(feature = "llvm-orc")]
fn build_snapshot_segments(entries: &[DeclaredState]) -> Vec<StateSnapshotSegment> {
    entries
        .iter()
        .map(|entry| StateSnapshotSegment {
            snapshot_offset: entry.byte_offset,
            state_offset: entry.storage_byte_offset,
            byte_size: entry.byte_size(),
            element_size: primitives::primitive_type_bytes(entry.elem_ty),
        })
        .collect()
}

#[cfg(all(test, feature = "llvm-orc"))]
mod tests {
    use super::*;

    use onda_frontend::parse_program;
    use onda_semantics::{analyze_with_options, AnalysisOptions, TypedProgram};

    #[derive(Debug, Clone, Copy)]
    struct SourceCompileOptions {
        sample_rate: f32,
        block_size: usize,
        fast_math: bool,
        opt_level: TargetOptLevel,
    }

    impl Default for SourceCompileOptions {
        fn default() -> Self {
            Self {
                sample_rate: 48_000.0,
                block_size: 512,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            }
        }
    }

    #[derive(Debug, Clone)]
    struct SourceCodegenOptions {
        sample_rate: f32,
        block_size: usize,
        fast_math: bool,
        target: TargetConfig,
    }

    impl Default for SourceCodegenOptions {
        fn default() -> Self {
            Self {
                sample_rate: 48_000.0,
                block_size: 512,
                fast_math: false,
                target: TargetConfig::host(),
            }
        }
    }

    fn lower_typed_program(
        typed: &TypedProgram,
    ) -> Result<onda_mir::OptimizedProgram, Vec<Diagnostic>> {
        onda_semantics::lower_program_to_optimized_mir(typed).map_err(|errors| {
            errors
                .into_iter()
                .map(|error| Diagnostic::internal(format!("MIR lowering failed: {error}")))
                .collect()
        })
    }

    fn validate_source_config(
        typed: &TypedProgram,
        sample_rate: f32,
        block_size: usize,
    ) -> Result<(), Vec<Diagnostic>> {
        if typed.analysis_options.sample_rate.to_bits() == sample_rate.to_bits()
            && typed.analysis_options.block_size == block_size
        {
            return Ok(());
        }
        Err(vec![Diagnostic::internal(format!(
            "MIR compile configuration must match semantic analysis: analyzed at {} Hz / {} frames, requested {} Hz / {} frames",
            typed.analysis_options.sample_rate,
            typed.analysis_options.block_size,
            sample_rate,
            block_size,
        ))])
    }

    fn lower_and_jit(typed: TypedProgram) -> Result<JitProgram, Vec<Diagnostic>> {
        let options = SourceCompileOptions {
            sample_rate: typed.analysis_options.sample_rate,
            block_size: typed.analysis_options.block_size,
            ..SourceCompileOptions::default()
        };
        lower_and_jit_with_options(typed, options)
    }

    fn lower_and_jit_with_options(
        typed: TypedProgram,
        options: SourceCompileOptions,
    ) -> Result<JitProgram, Vec<Diagnostic>> {
        validate_source_config(&typed, options.sample_rate, options.block_size)?;
        let mir = lower_typed_program(&typed)?;
        jit_program_from_optimized_mir_with_options(
            mir,
            MirCompileOptions {
                fast_math: options.fast_math,
                opt_level: options.opt_level,
            },
        )
    }

    fn lower_to_llvm_ir_with_options(
        typed: TypedProgram,
        options: SourceCompileOptions,
    ) -> Result<String, Vec<Diagnostic>> {
        validate_source_config(&typed, options.sample_rate, options.block_size)?;
        let mir = lower_typed_program(&typed)?;
        lower_optimized_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: options.fast_math,
                opt_level: options.opt_level,
            },
        )
        .map_err(mir_codegen_diagnostics)
    }

    fn lower_to_object_with_options(
        typed: TypedProgram,
        options: SourceCodegenOptions,
    ) -> Result<AotObjectArtifact, Vec<Diagnostic>> {
        validate_source_config(&typed, options.sample_rate, options.block_size)?;
        let mir = lower_typed_program(&typed)?;
        lower_optimized_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: options.fast_math,
                target: options.target,
            },
        )
        .map_err(mir_codegen_diagnostics)
    }

    fn typed_program(src: &str) -> TypedProgram {
        let program = parse_program(src).expect("source should parse");
        analyze_with_options(program, AnalysisOptions::default()).expect("source should analyze")
    }

    fn typed_program_with_options(src: &str, sample_rate: f32, block_size: usize) -> TypedProgram {
        let program = parse_program(src).expect("source should parse");
        analyze_with_options(
            program,
            AnalysisOptions {
                sample_rate,
                block_size,
            },
        )
        .expect("source should analyze")
    }

    fn run_one_sample(src: &str) -> f32 {
        run_one_sample_with_options(src, 48_000.0, 512)
    }

    fn run_one_sample_with_options(src: &str, sample_rate: f32, block_size: usize) -> f32 {
        let program = lower_and_jit_with_options(
            typed_program_with_options(src, sample_rate, block_size),
            SourceCompileOptions {
                sample_rate,
                block_size,
                ..SourceCompileOptions::default()
            },
        )
        .expect("source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let input_ptrs: Vec<*const u8> = Vec::new();
        let mut output = vec![0.0_f32; 1];
        let output_ptrs = [output.as_mut_ptr().cast::<u8>()];
        let buffer_ptrs: Vec<*mut u8> = Vec::new();
        let buffer_frames: Vec<i32> = Vec::new();
        let buffer_channels: Vec<i32> = Vec::new();
        let buffer_sample_rates: Vec<f32> = Vec::new();
        unsafe {
            program.process_checked(
                &mut state,
                &params,
                0,
                1,
                1 | 2,
                &input_ptrs,
                &output_ptrs,
                &buffer_ptrs,
                &buffer_frames,
                &buffer_channels,
                &buffer_sample_rates,
            )
        }
        .expect("process should succeed");
        output[0]
    }
    #[test]
    fn convenience_jit_uses_the_analyzed_program_configuration() {
        let typed = typed_program_with_options("sample:\n  out1 = SR\n", 44_100.0, 64);
        let program = lower_and_jit(typed).expect("configured source should lower");
        assert_eq!(program.sample_rate(), 44_100.0);
        assert_eq!(program.block_size(), 64);
    }

    #[test]
    fn snapshots_pack_only_persistent_state_and_restore_scratch_to_initial_values() {
        let source = r#"
proc Voice:
  init:
    phase = 0.0

  sample:
    phase = phase + 1.0
    out1 = phase

def step_at(voices, index: i32):
  return voices[index]()

def relay(voices, index: i32):
  return step_at(voices, index)

init:
  voices: Voice[2] = Voice()

sample:
  out1 = relay(voices, 0)
"#;
        let typed = typed_program(source);
        let mir = onda_semantics::lower_program_to_optimized_mir(&typed)
            .expect("source should lower to MIR");
        assert!(mir
            .as_program()
            .state
            .iter()
            .any(|slot| { slot.persistence == onda_mir::StatePersistence::InstanceScratch }));

        let program = lower_and_jit(typed).expect("source should lower to JIT");
        let params = program.default_param_bytes();
        let initial = program
            .initialize_state(&params)
            .expect("state should initialize");
        assert_eq!(program.physical_state_size_bytes(), initial.byte_size());
        assert!(program.state_size_bytes() < initial.byte_size());

        let mut snapshot = vec![0_u8; program.state_size_bytes()];
        program
            .write_state_snapshot(&initial, &mut snapshot)
            .expect("snapshot should pack");
        let mut live = initial
            .try_clone_with_allocator(None)
            .expect("state should clone");
        live.bytes_mut().fill(0xff);
        program
            .restore_state_snapshot(&mut live, &initial, &snapshot)
            .expect("snapshot should restore");
        assert_eq!(live.bytes(), initial.bytes());
    }

    #[test]
    fn optimized_mir_preserves_vectorization_for_stateless_sample_loops() {
        let scenarios = [
            r#"
ins:
  in1
params:
  gain = 0.75
sample:
  out1 = in1 * gain
"#,
            r#"
ins:
  in1
params:
  amount = 0.35
def shape(x: f32, a: f32):
  return (x + a) * (x - a)
sample:
  out1 = shape(shape(in1, amount), amount * 0.5)
"#,
        ];
        for source in scenarios {
            let typed = typed_program_with_options(source, 48_000.0, 64);
            let ir = lower_to_llvm_ir_with_options(
                typed,
                SourceCompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 64,
                    opt_level: TargetOptLevel::O3,
                    ..SourceCompileOptions::default()
                },
            )
            .expect("stateless sample loop should lower to optimized LLVM IR");
            assert!(ir.contains("vector.body"));
            assert!(ir.contains("llvm.loop.isvectorized"));
            assert!(ir.contains("ptr noalias readonly"));
        }
    }

    #[test]
    fn mir_orc_compatibility_program_runs_through_jit_contract() {
        let source = r#"
params:
  gain = 0.75 { 0.0, 1.0 }

sample:
  out1 = gain
"#;
        let program = lower_and_jit_with_options(
            typed_program(source),
            SourceCompileOptions {
                opt_level: TargetOptLevel::O0,
                ..SourceCompileOptions::default()
            },
        )
        .expect("MIR ORC source should lower through the compatibility contract");
        assert_eq!(program.required_in_channels(), 0);
        assert_eq!(program.required_out_channels(), 1);
        assert_eq!(program.param_slot_count(), 1);
        assert_eq!(program.param_name(0), Some("gain"));

        let params = program.default_param_bytes();
        assert_eq!(params, 0.75_f32.to_ne_bytes());
        let mut state = program.initialize_state(&params).unwrap();
        let inputs: [*const u8; 0] = [];
        let mut output = [0.0_f32; 1];
        let outputs = [output.as_mut_ptr().cast::<u8>()];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        unsafe {
            program.process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &inputs,
                &outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
        }
        .unwrap();
        assert!((output[0] - 0.75).abs() < 1.0e-6);
    }

    #[test]
    fn mir_orc_rejects_codegen_config_different_from_semantic_config() {
        let diagnostics = lower_and_jit_with_options(
            typed_program("sample:\n  out1 = SR\n"),
            SourceCompileOptions {
                block_size: 4,
                ..SourceCompileOptions::default()
            },
        )
        .expect_err("mismatched compile config must fail closed");
        assert!(diagnostics[0]
            .message
            .contains("must match semantic analysis"));
    }

    fn run_one_block_control_outputs(src: &str) -> Vec<f32> {
        let program = lower_and_jit(typed_program(src)).expect("source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let input_ptrs: Vec<*const u8> = Vec::new();
        let output_ptrs: Vec<*mut u8> = Vec::new();
        let buffer_ptrs: Vec<*mut u8> = Vec::new();
        let buffer_frames: Vec<i32> = Vec::new();
        let buffer_channels: Vec<i32> = Vec::new();
        let buffer_sample_rates: Vec<f32> = Vec::new();
        unsafe {
            program.process_checked(
                &mut state,
                &params,
                0,
                1,
                1 | 2,
                &input_ptrs,
                &output_ptrs,
                &buffer_ptrs,
                &buffer_frames,
                &buffer_channels,
                &buffer_sample_rates,
            )
        }
        .expect("process should succeed");

        let state_bytes = unsafe {
            std::slice::from_raw_parts(
                state.state_words.as_ptr().cast::<u8>(),
                state.state_size_bytes,
            )
        };
        let mut outputs = Vec::new();
        for index in 0..program.control_output_count() {
            let byte_len = program
                .control_output_type_bytes(index)
                .expect("control output should have byte size");
            let start = program
                .control_output_storage_byte_offset(index)
                .expect("control output should have storage");
            let end = start + byte_len;
            for bytes in state_bytes[start..end].chunks_exact(std::mem::size_of::<f32>()) {
                outputs.push(f32::from_ne_bytes(
                    bytes.try_into().expect("control output should be f32"),
                ));
            }
        }
        outputs
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
    fn dynamic_kins_indexing_runs() {
        let output = run_one_sample(
            r#"
kins:
  kin1 = 0.25
  kin2 = 0.75

sample:
  out1 = kins[0] + kins[1]
"#,
        );

        assert!((output - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn dynamic_kouts_indexing_runs() {
        let outputs = run_one_block_control_outputs(
            r#"
kouts 2

block:
  kouts[0] = 0.25
  kouts[1] = 0.75
"#,
        );

        assert_eq!(outputs.len(), 2);
        assert!((outputs[0] - 0.25).abs() < 1.0e-6);
        assert!((outputs[1] - 0.75).abs() < 1.0e-6);
    }

    #[test]
    fn control_output_array_state_offsets_match_runtime_layout() {
        let outputs = run_one_block_control_outputs(
            r#"
kouts {
  meter: f32[2]
}

block {
  meter[0] = 0.25
  meter[1] = 0.75
}
"#,
        );

        assert_eq!(outputs.len(), 2);
        assert!((outputs[0] - 0.25).abs() < 1.0e-6);
        assert!((outputs[1] - 0.75).abs() < 1.0e-6);
    }

    #[test]
    fn nested_block_rate_proc_operator_runs() {
        let outputs = run_one_block_control_outputs(
            r#"
proc Meter {
  kouts { kout1 }

  block:
    kout1 = 2.0
}

proc Outer {
  kouts { kout1 }

  init:
    meter = Meter()

  block:
    kout1 = meter()
}

kouts { meter }

init:
  outer = Outer()

block:
  meter = outer()
"#,
        );

        assert_eq!(outputs.len(), 1);
        assert!((outputs[0] - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn mixed_dynamic_outs_and_kouts_indexing_runs() {
        let output = run_one_sample(
            r#"
outs 1
kouts 2

block:
  kouts[1] = 0.75
  sample:
    outs[0] = 0.25
"#,
        );

        assert!((output - 0.25).abs() < 1.0e-6);
    }

    #[test]
    fn emits_cross_target_object_when_backend_is_available() {
        let typed = typed_program_with_options(
            r#"
outs:
  out1

sample:
  out1 = 0.0
"#,
            48_000.0,
            128,
        );

        let result = lower_to_object_with_options(
            typed,
            SourceCodegenOptions {
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

    #[test]
    fn lowers_const_array_read_to_llvm_ir() {
        let typed = typed_program(
            r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

params:
  idx: i32 = 1

outs:
  out1

sample:
  out1 = Table[idx]
"#,
        );

        let ir = lower_to_llvm_ir_with_options(typed, SourceCompileOptions::default())
            .expect("const array program should lower to LLVM IR");
        assert!(ir.contains("__onda_mir_const_0"));
    }

    #[test]
    fn oversampled_proc_local_defs_specialize_with_effective_sample_rate() {
        let source = r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = SR / f32(BS)
  outs:
    out1
  sample 2:
    out1 = cached

outs:
  out1
init:
  voice = Voice(gain = 2.0)
sample:
  out1 = voice()
"#;
        let typed = typed_program_with_options(source, 48_000.0, 4);
        let mir = onda_semantics::lower_program_to_optimized_mir(&typed)
            .expect("oversampled proc-local bind hook should lower to MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert_eq!(
            dump.matches("f32(24000.0)").count(),
            1,
            "effective SR / host BS should fold to 24000"
        );
    }

    #[test]
    fn oversampled_proc_local_consts_specialize_with_effective_sample_rate() {
        let source = r#"
proc Voice:
  const Frames = SR
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    const LocalFrames = SR
    cached = Frames + LocalFrames
  outs:
    out1
  sample 2:
    out1 = cached

outs:
  out1
init:
  voice = Voice(gain = 2.0)
sample:
  out1 = voice()
"#;
        let typed = typed_program_with_options(source, 48_000.0, 4);
        let mir = onda_semantics::lower_program_to_optimized_mir(&typed)
            .expect("oversampled proc-local constants should lower to MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert_eq!(
            dump.matches("f32(192000.0)").count(),
            1,
            "two effective-rate constants should fold to 192000"
        );
    }

    #[test]
    fn host_sr_builtin_stays_host_when_proc_sr_is_oversampled() {
        let source = r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = SR + HOST_SR + HOST_SAMPLE_RATE + HOST_SAMPLERATE + host_sample_rate + host_samplerate
  outs:
    out1
  sample 2:
    out1 = cached

outs:
  out1
init:
  voice = Voice(gain = 2.0)
sample:
  out1 = voice()
"#;
        let typed = typed_program_with_options(source, 48_000.0, 4);
        let mir = onda_semantics::lower_program_to_optimized_mir(&typed)
            .expect("HOST_SR aliases should lower to MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert_eq!(
            dump.matches("f32(336000.0)").count(),
            1,
            "effective SR and five host-rate aliases should fold to 336000 total:\n{dump}"
        );
    }

    #[test]
    fn const_array_slice_copy_source_runs() {
        let output = run_one_sample(
            r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

outs:
  out1

sample:
  dst = [0.0, 0.0, 0.0]
  dst[:] = Table[:]
  out1 = dst[0] + dst[1] + dst[2]
"#,
        );

        assert!((output - 1.75).abs() < 1.0e-6);
    }

    #[test]
    fn const_array_len_runs() {
        let output = run_one_sample(
            r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

outs:
  out1

sample:
  out1 = f32(Table.len())
"#,
        );

        assert!((output - 3.0).abs() < 1.0e-6);
    }

    #[test]
    fn const_array_readonly_def_arg_runs() {
        let output = run_one_sample(
            r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

def sum_edges(arr: f32[]):
  return arr[0] + arr[arr.len() - 1]

outs:
  out1

sample:
  out1 = sum_edges(Table)
"#,
        );

        assert!((output - 1.25).abs() < 1.0e-6);
    }

    #[test]
    fn const_scalar_from_const_def_runs() {
        let output = run_one_sample(
            r#"
const def gain(x: f64) -> f64:
  return x * 2.0 + 0.25

const Gain = gain(0.5)

outs:
  out1

sample:
  out1 = f32(Gain)
"#,
        );

        assert!((output - 1.25).abs() < 1.0e-6);
    }

    #[test]
    fn namespaced_const_array_runs() {
        let output = run_one_sample(
            r#"
namespace LUT:
  const Table: f32[3] = [0.25, 0.5, 1.0]

namespace Picked = LUT

outs:
  out1

sample:
  out1 = Picked::Table[1] + LUT::Table[2]
"#,
        );

        assert!((output - 1.5).abs() < 1.0e-6);
    }

    #[test]
    fn namespaced_array_returning_const_def_runs() {
        let output = run_one_sample(
            r#"
namespace LUT<N = 3>:
  const def ramp() -> f32[N]:
    values: f32[N]
    for i in 0..N:
      values[i] = f32(i) * 0.5
    return values

  const Table: f32[N] = ramp()

outs:
  out1

sample:
  out1 = LUT<3>::Table[2]
"#,
        );

        assert!((output - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn namespaced_const_arrays_emit_aot_object() {
        let typed = typed_program(
            r#"
namespace LUT<N = 3>:
  const def ramp() -> f32[N]:
    values: f32[N]
    for i in 0..N:
      values[i] = f32(i) + 0.25
    return values

  const Table: f32[N] = ramp()

outs:
  out1

sample:
  out1 = LUT<3>::Table[2]
"#,
        );

        let artifact = lower_to_object_with_options(typed, SourceCodegenOptions::default())
            .expect("namespaced const array AOT object should emit");
        assert!(!artifact.object_bytes.is_empty());
    }

    #[test]
    fn const_array_bytes_do_not_increase_orc_state_size() {
        let base = lower_and_jit(typed_program(
            r#"
outs:
  out1

sample:
  out1 = 0.0
"#,
        ))
        .expect("base program should lower");
        let values = (0..256)
            .map(|idx| format!("{}.0", idx))
            .collect::<Vec<_>>()
            .join(", ");
        let with_const_src = format!(
            r#"
const Table: f32[256] = [{values}]

outs:
  out1

sample:
  out1 = Table[0]
"#
        );
        let with_const =
            lower_and_jit(typed_program(&with_const_src)).expect("const program should lower");
        let params = Vec::<u8>::new();
        let base_state = base
            .initialize_state(&params)
            .expect("base state should initialize");
        let const_state = with_const
            .initialize_state(&params)
            .expect("const state should initialize");

        assert_eq!(const_state.state_size_bytes, base_state.state_size_bytes);
    }

    #[test]
    fn const_array_bytes_do_not_increase_aot_state_size() {
        let base = typed_program(
            r#"
outs:
  out1

sample:
  out1 = 0.0
"#,
        );
        let values = (0..256)
            .map(|idx| format!("{}.0", idx))
            .collect::<Vec<_>>()
            .join(", ");
        let with_const_src = format!(
            r#"
const Table: f32[256] = [{values}]

outs:
  out1

sample:
  out1 = Table[0]
"#
        );
        let with_const = typed_program(&with_const_src);
        let base_artifact = lower_to_object_with_options(base, SourceCodegenOptions::default())
            .expect("base AOT object should emit");
        let const_artifact =
            lower_to_object_with_options(with_const, SourceCodegenOptions::default())
                .expect("const AOT object should emit");

        assert!(!const_artifact.object_bytes.is_empty());
        assert_eq!(
            const_artifact.metadata.runtime.state_size_bytes,
            base_artifact.metadata.runtime.state_size_bytes
        );
    }

    #[test]
    fn aot_control_output_metadata_includes_state_offsets() {
        let typed = typed_program(
            r#"
kouts {
  meter: f64
}

init {
  held = 1.0
}

block {
  meter = f64(held)
}
"#,
        );

        let artifact = lower_to_object_with_options(typed, SourceCodegenOptions::default())
            .expect("control-output AOT object should emit");
        let meter = artifact
            .metadata
            .metadata
            .control_outputs
            .first()
            .expect("control output metadata should exist");

        assert_eq!(meter.name, "meter");
        assert_eq!(meter.byte_offset, Some(0));
        let state_offset = meter
            .state_byte_offset
            .expect("control output should expose state byte offset");
        assert!(state_offset < artifact.metadata.runtime.state_size_bytes);
        assert_eq!(
            state_offset % std::mem::align_of::<f64>(),
            0,
            "physical control mirror must retain target ABI alignment"
        );
    }
}
