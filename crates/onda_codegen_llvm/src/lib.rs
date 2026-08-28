//! LLVM execution and object-code backend for validated Onda MIR.

use std::alloc::Layout;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::ptr::{self, NonNull};
use std::sync::Arc;

use onda_frontend::{Diagnostic, PrimitiveType};
use onda_mir::{IntegerRangeInvariant, ParamControl, ScalarValue, ValueRange};
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
    PROCESSOR_EXECUTION_OK, PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
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

pub fn check_execution_status(status: u32) -> Result<(), Diagnostic> {
    if status == PROCESSOR_EXECUTION_OK {
        Ok(())
    } else {
        Err(Diagnostic::runtime(
            format!("generated Onda code failed a runtime safety check ({status})"),
            0,
            0,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct JitProgram {
    sample_rate: f32,
    block_size: usize,
    inputs: Arc<Vec<DeclaredIo>>,
    outputs: Arc<Vec<DeclaredIo>>,
    control_outputs: Arc<Vec<DeclaredIo>>,
    params: Arc<Vec<DeclaredIo>>,
    events: Arc<Vec<DeclaredEvent>>,
    delegates: Arc<Vec<DeclaredDelegate>>,
    buffers: Arc<Vec<DeclaredBuffer>>,
    buffer_arrays: Arc<Vec<DeclaredBufferArray>>,
    input_index: Arc<HashMap<String, usize>>,
    output_index: Arc<HashMap<String, usize>>,
    control_output_index: Arc<HashMap<String, usize>>,
    param_index: Arc<HashMap<String, usize>>,
    event_index: Arc<HashMap<String, usize>>,
    delegate_index: Arc<HashMap<String, usize>>,
    buffer_index: Arc<HashMap<String, usize>>,
    state_entries: Arc<Vec<DeclaredState>>,
    snapshot_segments: Arc<Vec<StateSnapshotSegment>>,
    snapshot_size_bytes: usize,
    #[cfg(feature = "llvm-orc")]
    compiled: Arc<orc_backend::MirJitProgram>,
}

#[derive(Debug, Clone, Copy)]
struct StateSnapshotSegment {
    snapshot_offset: usize,
    state_offset: usize,
    byte_size: usize,
    element_size: usize,
    integer_range: Option<IntegerRangeInvariant>,
}

#[derive(Clone, Copy)]
pub struct RuntimeAllocator {
    context: *mut c_void,
    alloc: unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize),
}

impl RuntimeAllocator {
    /// Creates a host allocator for instance-owned runtime storage.
    ///
    /// Onda invokes `alloc` only synchronously while creating an instance. Once
    /// instance creation returns, no operation on that instance invokes
    /// `alloc`. Onda may invoke `free` while unwinding failed creation and when
    /// the completed instance is later destroyed.
    ///
    /// # Safety
    ///
    /// `context` and both callbacks must remain valid until every instance
    /// created with this allocator has been destroyed. `alloc` must return
    /// writable storage of at least `size` bytes aligned to `align`, or null on
    /// failure. `free` must accept every non-null allocation returned by
    /// `alloc`, with its original size and alignment.
    ///
    /// `alloc` must be callable on each thread where the host creates an
    /// instance. `free` must be callable on every thread where creation can
    /// fail or an instance can be destroyed, including concurrently when the
    /// host creates or destroys multiple instances at once.
    pub unsafe fn new(
        context: *mut c_void,
        alloc: unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void,
        free: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize),
    ) -> Self {
        Self {
            context,
            alloc,
            free,
        }
    }

    /// Invokes the host allocation callback.
    ///
    /// # Safety
    ///
    /// `size` and `align` must describe a valid non-zero allocation layout,
    /// and the current thread must satisfy the contract given to [`Self::new`].
    pub unsafe fn allocate(self, size: usize, align: usize) -> *mut c_void {
        unsafe { (self.alloc)(self.context, size, align) }
    }

    /// Invokes the host deallocation callback.
    ///
    /// # Safety
    ///
    /// `ptr`, `size`, and `align` must identify a live allocation previously
    /// returned by [`Self::allocate`] through this allocator. The allocation
    /// must not be used again after this call, and the current thread must
    /// satisfy the contract given to [`Self::new`].
    pub unsafe fn deallocate(self, ptr: *mut c_void, size: usize, align: usize) {
        unsafe { (self.free)(self.context, ptr, size, align) }
    }
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

/// Allocated physical state storage that has not completed full processor initialization.
pub struct UninitializedRuntimeState {
    state_words: Option<UninitRuntimeBuffer<u64>>,
    state_size_bytes: usize,
}

pub struct RuntimeBuffer<T: Copy> {
    storage: RuntimeBufferStorage<T>,
}

pub(crate) struct UninitRuntimeBuffer<T: Copy> {
    storage: UninitRuntimeBufferStorage<T>,
}

enum RuntimeBufferStorage<T: Copy> {
    Global(Vec<T>),
    Custom(CustomRuntimeBuffer<T>),
}

enum UninitRuntimeBufferStorage<T: Copy> {
    Global(Vec<MaybeUninit<T>>),
    Custom(CustomRuntimeBuffer<MaybeUninit<T>>),
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

impl<T: Copy> UninitRuntimeBuffer<T> {
    pub(crate) fn try_new_in(
        len: usize,
        allocator: Option<RuntimeAllocator>,
    ) -> Result<Self, Diagnostic> {
        let storage = if let Some(allocator) = allocator {
            UninitRuntimeBufferStorage::Custom(CustomRuntimeBuffer::try_uninit(len, allocator)?)
        } else {
            let mut values = Vec::with_capacity(len);
            // SAFETY: MaybeUninit<T> is valid without initializing its contained T.
            unsafe { values.set_len(len) };
            UninitRuntimeBufferStorage::Global(values)
        };
        Ok(Self { storage })
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut T {
        if self.len() == 0 {
            return ptr::null_mut();
        }
        match &mut self.storage {
            UninitRuntimeBufferStorage::Global(values) => values.as_mut_ptr().cast::<T>(),
            UninitRuntimeBufferStorage::Custom(values) => values.ptr.as_ptr().cast::<T>(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match &self.storage {
            UninitRuntimeBufferStorage::Global(values) => values.len(),
            UninitRuntimeBufferStorage::Custom(values) => values.len,
        }
    }

    /// Converts the buffer after every element has been initialized.
    ///
    /// # Safety
    ///
    /// Every element in the buffer must contain a valid `T`.
    pub(crate) unsafe fn assume_init(self) -> RuntimeBuffer<T> {
        let storage = match self.storage {
            UninitRuntimeBufferStorage::Global(values) => {
                let mut values = ManuallyDrop::new(values);
                // SAFETY: the caller guarantees all elements are initialized, and
                // MaybeUninit<T> has the same layout as T.
                let values = unsafe {
                    Vec::from_raw_parts(
                        values.as_mut_ptr().cast::<T>(),
                        values.len(),
                        values.capacity(),
                    )
                };
                RuntimeBufferStorage::Global(values)
            }
            UninitRuntimeBufferStorage::Custom(values) => {
                let values = ManuallyDrop::new(values);
                RuntimeBufferStorage::Custom(CustomRuntimeBuffer {
                    ptr: values.ptr.cast::<T>(),
                    len: values.len,
                    allocator: values.allocator,
                })
            }
        };
        RuntimeBuffer { storage }
    }
}

impl fmt::Debug for UninitializedRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UninitializedRuntimeState")
            .field(
                "state_words",
                &self.state_words.as_ref().map(UninitRuntimeBuffer::len),
            )
            .field("state_size_bytes", &self.state_size_bytes)
            .finish()
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
    fn try_uninit(
        len: usize,
        allocator: RuntimeAllocator,
    ) -> Result<CustomRuntimeBuffer<MaybeUninit<T>>, Diagnostic> {
        let layout = runtime_array_layout::<MaybeUninit<T>>(len)?;
        let ptr = allocate_custom_runtime_buffer::<MaybeUninit<T>>(allocator, layout)?;
        Ok(CustomRuntimeBuffer {
            ptr,
            len,
            allocator,
        })
    }

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
            self.allocator.deallocate(
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
    let raw = unsafe { allocator.allocate(layout.size(), layout.align()) };
    let Some(ptr) = NonNull::new(raw.cast::<T>()) else {
        return Err(Diagnostic::runtime("runtime allocator returned null", 0, 0));
    };
    if !(ptr.as_ptr() as usize).is_multiple_of(layout.align()) {
        unsafe {
            allocator.deallocate(raw, layout.size(), layout.align());
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
    authored: bool,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_array: bool,
    byte_offset: usize,
    storage_byte_offset: usize,
    integer_range: Option<IntegerRangeInvariant>,
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
    may_write: bool,
}

#[derive(Debug, Clone)]
pub struct DeclaredBufferArray {
    name: String,
    first: usize,
    len: usize,
}

#[derive(Debug, Clone)]
pub struct DeclaredEvent {
    name: String,
    params: Vec<DeclaredEventParam>,
    payload_bytes: Option<usize>,
    payload_min_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct DeclaredEventParam {
    name: String,
    elem_ty: PrimitiveType,
    array_len: usize,
    is_array: bool,
    is_slice: bool,
    byte_offset: Option<usize>,
    default_bytes: Option<Vec<u8>>,
    default_values: Option<Vec<ScalarValue>>,
}

#[derive(Debug, Clone)]
pub struct DeclaredDelegate {
    name: String,
    params: Vec<DeclaredEventParam>,
    payload_bytes: Option<usize>,
    payload_min_bytes: usize,
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
    let mut metadata = mir_metadata::build_mir_program_metadata(
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
    metadata.state_entries.retain(DeclaredState::is_authored);
    Ok(JitProgram {
        sample_rate: compiled.mir().config.sample_rate,
        block_size: compiled.mir().config.block_size as usize,
        input_index: Arc::new(metadata.input_index),
        output_index: Arc::new(metadata.output_index),
        control_output_index: Arc::new(metadata.control_output_index),
        param_index: Arc::new(metadata.param_index),
        event_index: Arc::new(metadata.event_index),
        delegate_index: Arc::new(metadata.delegate_index),
        buffer_index: Arc::new(metadata.buffer_index),
        inputs: Arc::new(metadata.inputs),
        outputs: Arc::new(metadata.outputs),
        control_outputs: Arc::new(metadata.control_outputs),
        params: Arc::new(metadata.params),
        events: Arc::new(metadata.events),
        delegates: Arc::new(metadata.delegates),
        buffers: Arc::new(metadata.buffers),
        buffer_arrays: Arc::new(metadata.buffer_arrays),
        state_entries: Arc::new(metadata.state_entries),
        snapshot_segments: Arc::new(snapshot_segments),
        snapshot_size_bytes,
        compiled: Arc::new(compiled),
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
            integer_range: entry.integer_range,
        })
        .collect()
}

#[cfg(all(test, feature = "llvm-orc"))]
mod tests {
    use super::*;

    use onda_frontend::parse_program;
    use onda_semantics::{analyze_with_options, AnalysisOptions, TypedProgram};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn jit_program_is_send_and_sync() {
        assert_send_sync::<JitProgram>();
    }

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
                None,
            )
        }
        .expect("process should succeed");
        output[0]
    }

    #[test]
    fn native_delegate_batch_preserves_order_and_payloads() {
        let program = lower_and_jit(typed_program(
            r#"
delegates:
  first(value: i32)
  second(value: f32)
event trigger():
  first(7)
  second(2.5)
sample:
  out1 = 0.0
"#,
        ))
        .expect("delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 24];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("event should publish delegates");

        assert_eq!(batch.used_bytes, 24);
        assert_eq!(batch.record_count, 2);
        assert_eq!(batch.overflow_count, 0);
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 4);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 7);
        assert_eq!(u32::from_ne_bytes(storage[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(storage[16..20].try_into().unwrap()), 4);
        assert_eq!(f32::from_ne_bytes(storage[20..24].try_into().unwrap()), 2.5);

        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                99,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("a missing checked event should be a neutral call");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (0, 0, 0)
        );

        batch.used_bytes = 12;
        batch.record_count = 1;
        batch.overflow_count = 3;
        let status = unsafe {
            program.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                99,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("a missing unchecked event should be a neutral call");
        assert_eq!(status, onda_processor_abi::PROCESSOR_EXECUTION_OK);
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (0, 0, 0)
        );
    }

    #[test]
    fn native_delegate_batch_copies_multiple_and_empty_slices() {
        let program = lower_and_jit(typed_program(
            r#"
delegate report(code: i32, values: f32[], tags: i32[])
event trigger(values: f32[], tags: i32[]):
  report(7, values, tags)
sample:
  out1 = 0.0
"#,
        ))
        .expect("dynamic delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut payload = Vec::new();
        payload.extend_from_slice(&2_i32.to_ne_bytes());
        payload.extend_from_slice(&1.25_f32.to_ne_bytes());
        payload.extend_from_slice(&(-2.5_f32).to_ne_bytes());
        payload.extend_from_slice(&3_i32.to_ne_bytes());
        for value in [11_i32, -4, 99] {
            payload.extend_from_slice(&value.to_ne_bytes());
        }
        let mut storage = [0_u8; 40];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("dynamic delegate should publish");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (40, 1, 0)
        );
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 32);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 7);
        assert_eq!(i32::from_ne_bytes(storage[12..16].try_into().unwrap()), 2);
        assert_eq!(
            f32::from_ne_bytes(storage[16..20].try_into().unwrap()),
            1.25
        );
        assert_eq!(
            f32::from_ne_bytes(storage[20..24].try_into().unwrap()),
            -2.5
        );
        assert_eq!(i32::from_ne_bytes(storage[24..28].try_into().unwrap()), 3);
        assert_eq!(i32::from_ne_bytes(storage[28..32].try_into().unwrap()), 11);
        assert_eq!(i32::from_ne_bytes(storage[32..36].try_into().unwrap()), -4);
        assert_eq!(i32::from_ne_bytes(storage[36..40].try_into().unwrap()), 99);

        let empty_payload = [0_u8; 8];
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &empty_payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("empty delegate slices should publish");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (20, 1, 0)
        );
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 12);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 7);
        assert_eq!(i32::from_ne_bytes(storage[12..16].try_into().unwrap()), 0);
        assert_eq!(i32::from_ne_bytes(storage[16..20].try_into().unwrap()), 0);
    }

    #[test]
    fn native_delegate_batch_drops_whole_records_and_keeps_later_records() {
        let program = lower_and_jit(typed_program(
            r#"
delegates:
  large(values: f32[])
  small(value: i32)
event trigger(values: f32[]):
  large(values)
  small(9)
sample:
  out1 = 0.0
"#,
        ))
        .expect("overflow delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut payload = Vec::new();
        payload.extend_from_slice(&2_i32.to_ne_bytes());
        payload.extend_from_slice(&1.0_f32.to_ne_bytes());
        payload.extend_from_slice(&2.0_f32.to_ne_bytes());
        let mut storage = [0_u8; 12];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("overflow does not fail delegate dispatch");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (12, 1, 1)
        );
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 4);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 9);
    }

    #[test]
    fn native_delegate_batch_is_cleared_when_generated_execution_fails() {
        let program = lower_and_jit(typed_program(
            r#"
delegate started()
init:
  observed = 0.0
event trigger(values: f32[]):
  started()
  observed = values[0]
sample:
  out1 = observed
"#,
        ))
        .expect("failing delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 8];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        batch.used_bytes = 7;
        batch.record_count = 3;
        batch.overflow_count = 2;
        let result = unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &0_i32.to_ne_bytes(),
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        };
        assert!(result.is_err(), "out-of-bounds event should fail");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (0, 0, 0)
        );
    }

    #[test]
    fn native_print_batch_is_retained_when_generated_execution_fails() {
        let program = lower_and_jit(typed_program(
            r#"
init:
  observed = 0.0
event trigger(values: f32[]):
  print("before failure", 7)
  observed = values[0]
sample:
  out1 = observed
"#,
        ))
        .expect("failing print source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 12];
        let mut batch = onda_processor_abi::PrintBatch::from_storage(&mut storage);
        batch.used_bytes = 1;
        batch.record_count = 9;
        batch.overflow_count = 4;

        let result = unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &0_i32.to_ne_bytes(),
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: std::ptr::null_mut(),
                    print_batch: &mut batch,
                }),
            )
        };

        assert!(result.is_err(), "out-of-bounds event should fail");
        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (12, 1, 0)
        );
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 4);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 7);
    }

    #[test]
    fn native_print_batch_drops_whole_records_and_keeps_later_records() {
        let program = lower_and_jit(typed_program(
            r#"
event trigger():
  print("large", f64(1.0))
  print("small", true)
sample:
  out1 = 0.0
"#,
        ))
        .expect("overflow print source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 9];
        let mut batch = onda_processor_abi::PrintBatch::from_storage(&mut storage);

        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: std::ptr::null_mut(),
                    print_batch: &mut batch,
                }),
            )
        }
        .expect("event should retain the later small print");

        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (9, 1, 1)
        );
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 1);
        assert_eq!(storage[8], 1);
    }

    #[test]
    fn native_empty_print_publishes_an_empty_record() {
        let program = lower_and_jit(typed_program(
            r#"
event trigger():
  print()
sample:
  out1 = 0.0
"#,
        ))
        .expect("empty print source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 8];
        let mut batch = onda_processor_abi::PrintBatch::from_storage(&mut storage);

        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: std::ptr::null_mut(),
                    print_batch: &mut batch,
                }),
            )
        }
        .expect("empty print should publish");

        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (8, 1, 0)
        );
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_ne_bytes(storage[4..8].try_into().unwrap()), 0);
    }

    #[test]
    fn task_delegate_publications_follow_resumption_and_batch_boundaries() {
        let program = lower_and_jit(typed_program(
            r#"
delegate progress(value: i32)
task worker():
  progress(1)
  yield
  progress(2)
  yield
block:
  await worker()
  sample:
    out1 = 0.0
"#,
        ))
        .expect("task delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let input_ptrs: Vec<*const u8> = Vec::new();
        let mut output = [0.0_f32; 1];
        let output_ptrs = [output.as_mut_ptr().cast::<u8>()];
        let buffer_ptrs: Vec<*mut u8> = Vec::new();
        let buffer_frames: Vec<i32> = Vec::new();
        let buffer_channels: Vec<i32> = Vec::new();
        let buffer_sample_rates: Vec<f32> = Vec::new();
        let mut storage = [0_u8; 12];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);

        for expected in [Some(1_i32), Some(2_i32), None] {
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
                    Some(&mut onda_processor_abi::ExecutionOutput {
                        delegate_batch: &mut batch,
                        print_batch: std::ptr::null_mut(),
                    }),
                )
            }
            .expect("task resumption should process successfully");
            match expected {
                Some(value) => {
                    assert_eq!((batch.used_bytes, batch.record_count), (12, 1));
                    assert_eq!(
                        i32::from_ne_bytes(storage[8..12].try_into().unwrap()),
                        value
                    );
                }
                None => assert_eq!((batch.used_bytes, batch.record_count), (0, 0)),
            }
            assert_eq!(batch.overflow_count, 0);
        }
    }

    #[test]
    fn proc_task_delegates_dispatch_through_parent_routes() {
        let program = lower_and_jit(typed_program(
            r#"
proc Worker:
  delegate progress(value: i32)
  task run():
    progress(3)
    yield
    progress(4)
    yield
  block:
    await run()
    sample:
      out1 = 0.0

delegate observed(value: i32)
init:
  worker = Worker()
when worker.progress(value):
  observed(value)
sample:
  out1 = worker()
"#,
        ))
        .expect("proc task delegate source should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let input_ptrs: Vec<*const u8> = Vec::new();
        let mut output = [0.0_f32; 1];
        let output_ptrs = [output.as_mut_ptr().cast::<u8>()];
        let buffer_ptrs: Vec<*mut u8> = Vec::new();
        let buffer_frames: Vec<i32> = Vec::new();
        let buffer_channels: Vec<i32> = Vec::new();
        let buffer_sample_rates: Vec<f32> = Vec::new();
        let mut storage = [0_u8; 12];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);

        for expected in [Some(3_i32), Some(4_i32), None] {
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
                    Some(&mut onda_processor_abi::ExecutionOutput {
                        delegate_batch: &mut batch,
                        print_batch: std::ptr::null_mut(),
                    }),
                )
            }
            .expect("proc task resumption should process successfully");
            match expected {
                Some(value) => {
                    assert_eq!((batch.used_bytes, batch.record_count), (12, 1));
                    assert_eq!(
                        i32::from_ne_bytes(storage[8..12].try_into().unwrap()),
                        value
                    );
                }
                None => assert_eq!((batch.used_bytes, batch.record_count), (0, 0)),
            }
            assert_eq!(batch.overflow_count, 0);
        }
    }

    #[test]
    fn nested_proc_delegate_promotion_reaches_the_host_once() {
        let program = lower_and_jit(typed_program(
            r#"
proc Child:
  delegate fired(value: i32)
  event trigger(value: i32):
    fired(value)
  sample:
    out1 = 0.0

proc Parent:
  delegate relayed(value: i32)
  init:
    child = Child()
  event trigger(value: i32):
    child.trigger(value)
  when child.fired(value):
    relayed(value)
  sample:
    out1 = child()

delegate observed(value: i32)
init:
  parent = Parent()
event trigger(value: i32):
  parent.trigger(value)
when parent.relayed(value):
  observed(value)
sample:
  out1 = parent()
"#,
        ))
        .expect("nested delegate promotion should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 12];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        let payload = 19_i32.to_ne_bytes();
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("nested delegate promotion should execute");
        assert_eq!(batch.record_count, 1);
        assert_eq!(batch.overflow_count, 0);
        assert_eq!(u32::from_ne_bytes(storage[0..4].try_into().unwrap()), 0);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 19);
    }

    #[test]
    fn delegate_routes_preserve_source_order_depth_first_dispatch_and_instance_identity() {
        let program = lower_and_jit(typed_program(
            r#"
proc Child:
  delegate fired(value: i32)
  event trigger(value: i32):
    fired(value)
  sample:
    out1 = 0.0

delegates:
  observed(slot: i32, value: i32)
  derived(value: i32 = 99)

init:
  left = Child()
  right = Child()

when left.fired(value):
  observed(0, value)
when left.fired(value):
  observed(1, value)
when right.fired(value):
  observed(2, value)
when observed(_, _):
  derived()

event trigger():
  left.trigger(10)
  right.trigger(20)

sample:
  out1 = left() + right()
"#,
        ))
        .expect("distinct delegate routes should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 84];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        unsafe {
            program.trigger_event_by_index(
                &mut state,
                &params,
                0,
                &[],
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("routed delegate dispatch should execute");

        assert_eq!(
            (batch.used_bytes, batch.record_count, batch.overflow_count),
            (84, 6, 0)
        );
        let expected = [
            (0_u32, vec![0_i32, 10]),
            (1, vec![99]),
            (0, vec![1, 10]),
            (1, vec![99]),
            (0, vec![2, 20]),
            (1, vec![99]),
        ];
        let mut cursor = 0usize;
        for (delegate, values) in expected {
            assert_eq!(
                u32::from_ne_bytes(storage[cursor..cursor + 4].try_into().unwrap()),
                delegate
            );
            assert_eq!(
                u32::from_ne_bytes(storage[cursor + 4..cursor + 8].try_into().unwrap()),
                (values.len() * 4) as u32
            );
            cursor += 8;
            for value in values {
                assert_eq!(
                    i32::from_ne_bytes(storage[cursor..cursor + 4].try_into().unwrap()),
                    value
                );
                cursor += 4;
            }
        }
        assert_eq!(cursor, storage.len());
    }

    #[test]
    fn whole_proc_array_subscription_reports_the_actual_index() {
        let program = lower_and_jit(typed_program(
            r#"
proc Child:
  delegate fired(value: i32)
  sample:
    fired(23)
    out1 = 0.0

delegates:
  observed(index: i32, value: i32)
  selected(value: i32)
init:
  children: Child[2] = Child()
when children.fired(index, value):
  observed(index, value)
when children[1].fired(value):
  selected(value)
sample:
  out1 = children[0]() + children[1]()
"#,
        ))
        .expect("whole-array delegate route should lower to JIT");
        let params = program.default_param_bytes();
        let mut state = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut storage = [0_u8; 44];
        let mut batch = onda_processor_abi::DelegateBatch::from_storage(&mut storage);
        let mut output = [0.0_f32; 1];
        let output_ptrs = [output.as_mut_ptr().cast::<u8>()];
        unsafe {
            program.process_checked(
                &mut state,
                &params,
                0,
                1,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &[],
                &output_ptrs,
                &[],
                &[],
                &[],
                &[],
                Some(&mut onda_processor_abi::ExecutionOutput {
                    delegate_batch: &mut batch,
                    print_batch: std::ptr::null_mut(),
                }),
            )
        }
        .expect("whole-array delegate route should publish");
        assert_eq!(batch.record_count, 3);
        assert_eq!(i32::from_ne_bytes(storage[8..12].try_into().unwrap()), 0);
        assert_eq!(i32::from_ne_bytes(storage[12..16].try_into().unwrap()), 23);
        assert_eq!(i32::from_ne_bytes(storage[24..28].try_into().unwrap()), 1);
        assert_eq!(i32::from_ne_bytes(storage[28..32].try_into().unwrap()), 23);
        assert_eq!(u32::from_ne_bytes(storage[32..36].try_into().unwrap()), 1);
        assert_eq!(i32::from_ne_bytes(storage[40..44].try_into().unwrap()), 23);
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
        unsafe { live.bytes_mut() }.fill(0xff);
        program
            .restore_state_snapshot(&params, &mut live, &snapshot, &[], &[], &[], &[])
            .expect("snapshot should restore");
        assert_eq!(live.bytes(), initial.bytes());
    }

    #[test]
    fn snapshot_restore_normalizes_ranged_i32_and_i64_state() {
        let typed = typed_program(
            r#"
init:
  wrapped: i32 = 0 {0..4, wrap}
  clamped: i64 = 10 {10..21}

sample:
  out1 = 0.0
"#,
        );
        let program = lower_and_jit(typed).expect("ranged source should lower to JIT");
        let params = program.default_param_bytes();
        let initial = program
            .initialize_state(&params)
            .expect("state should initialize");
        let mut live = initial
            .try_clone_with_allocator(None)
            .expect("state should clone");
        let mut snapshot = vec![0_u8; program.state_size_bytes()];
        snapshot[0..8].copy_from_slice(&(99_i64).to_le_bytes());
        snapshot[8..12].copy_from_slice(&(-1_i32).to_le_bytes());
        program
            .restore_state_snapshot(&params, &mut live, &snapshot, &[], &[], &[], &[])
            .expect("snapshot should restore");
        program
            .write_state_snapshot(&live, &mut snapshot)
            .expect("normalized state should snapshot");
        assert_eq!(i64::from_le_bytes(snapshot[0..8].try_into().unwrap()), 20);
        assert_eq!(i32::from_le_bytes(snapshot[8..12].try_into().unwrap()), 3);
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
                None,
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
                None,
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
            for bytes in state_bytes[start..end].as_chunks::<4>().0 {
                outputs.push(f32::from_ne_bytes(*bytes));
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
