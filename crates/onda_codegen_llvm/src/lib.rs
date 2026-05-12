use std::alloc::Layout;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
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
    state_entries: Arc<Vec<DeclaredState>>,
    #[cfg(feature = "llvm-orc")]
    compiled: Arc<orc_backend::OrcProcess>,
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
    if (ptr.as_ptr() as usize) % layout.align() != 0 {
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
    byte_offset: usize,
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
        state_entries: Arc::new(metadata.state_entries),
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

    fn run_one_sample(src: &str) -> f32 {
        let program = lower_and_jit(typed_program(src)).expect("source should lower to JIT");
        let params = vec![0_u8; program.param_byte_size()];
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
        program
            .process_checked(
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
            .expect("process should succeed");
        output[0]
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

        let ir = lower_to_llvm_ir_with_options(typed, CompileOptions::default())
            .expect("const array program should lower to LLVM IR");
        assert!(ir.contains("__onda_const_array_Table"));
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

        let artifact = lower_to_object_with_options(typed, CodegenOptions::default())
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
        let base_artifact = lower_to_object_with_options(base, CodegenOptions::default())
            .expect("base AOT object should emit");
        let const_artifact = lower_to_object_with_options(with_const, CodegenOptions::default())
            .expect("const AOT object should emit");

        assert!(!const_artifact.object_bytes.is_empty());
        assert_eq!(
            const_artifact.metadata.runtime.state_size_bytes,
            base_artifact.metadata.runtime.state_size_bytes
        );
    }
}
