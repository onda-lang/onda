//! Native LLVM lowering for validated Onda MIR.
//!
//! This is the production native backend. It accepts only validated MIR;
//! source-language typing, rewriting, scheduling, and specialization stay
//! above this boundary.

mod function_emitter;
mod host_abi;

use std::ffi::{CStr, CString};
use std::fmt;
use std::mem::ManuallyDrop;
use std::ptr::null_mut;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMGetErrorMessage};
use llvm_sys::orc2::lljit::*;
use llvm_sys::orc2::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{
    LLVMABIAlignmentOfType, LLVMABISizeOfType, LLVMByteOrder, LLVMCreateTargetData,
    LLVMDisposeTargetData, LLVMPointerSize, LLVMStoreSizeOfType, LLVMTargetDataRef,
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets, LLVM_InitializeNativeAsmParser,
    LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMDisposeTargetMachine, LLVMTargetMachineEmitToMemoryBuffer,
};
use llvm_sys::{
    LLVMFastMathAll, LLVMFastMathNone, LLVMIntPredicate, LLVMLinkage, LLVMRealPredicate,
};

use onda_frontend::Diagnostic;
use onda_mir::{
    Block, BufferChannels, CallArgument, FunctionKind, Place, Program, Projection, Rvalue,
    ScalarType, StatementKind, Type, Value,
};

use crate::{
    BufferDescriptorTables, RuntimeAllocator, RuntimeState, TargetOptLevel, UninitRuntimeBuffer,
    UninitializedRuntimeState,
};

use self::host_abi::{abi_const_ptr, abi_mut_ptr, validate_audio_abi, validate_buffer_abi};

const RUNTIME_FAILURE_CONTEXT_INDEX: u32 = 13;
const INIT_ALL_CONTEXT_INDEX: u32 = 16;
const DELEGATE_BATCH_CONTEXT_INDEX: u32 = 17;
const PRINT_BATCH_CONTEXT_INDEX: u32 = 18;
const OUTPUT_SEQUENCE_CONTEXT_INDEX: u32 = 19;

struct OwnedBufferDescriptorTables {
    pointers: Vec<*mut u8>,
    frames: Vec<i32>,
    channels: Vec<i32>,
    sample_rates: Vec<f32>,
}

impl OwnedBufferDescriptorTables {
    fn as_borrowed(&self) -> BufferDescriptorTables<'_> {
        BufferDescriptorTables::new(
            &self.pointers,
            &self.frames,
            &self.channels,
            &self.sample_rates,
        )
    }
}

struct OwnedLlvm<T: Copy> {
    value: T,
    dispose: unsafe extern "C" fn(T),
}

impl<T: Copy> OwnedLlvm<T> {
    fn new(value: T, dispose: unsafe extern "C" fn(T)) -> Self {
        Self { value, dispose }
    }

    fn get(&self) -> T {
        self.value
    }

    fn release(self) -> T {
        ManuallyDrop::new(self).value
    }
}

impl<T: Copy> Drop for OwnedLlvm<T> {
    fn drop(&mut self) {
        unsafe {
            (self.dispose)(self.value);
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// Classification for failures at the validated MIR-to-LLVM boundary.
pub enum MirCodegenErrorKind {
    InvalidMir,
    Unsupported,
    Llvm,
}

#[derive(Debug, Clone, Eq, PartialEq)]
/// A validation, capability-boundary, or LLVM failure from native MIR codegen.
pub struct MirCodegenError {
    pub kind: MirCodegenErrorKind,
    pub message: String,
}

impl fmt::Display for MirCodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for MirCodegenError {}

impl MirCodegenError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: MirCodegenErrorKind::InvalidMir,
            message: message.into(),
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            kind: MirCodegenErrorKind::Unsupported,
            message: message.into(),
        }
    }

    fn llvm(message: impl Into<String>) -> Self {
        Self {
            kind: MirCodegenErrorKind::Llvm,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// LLVM policy options for an already configured MIR program.
///
/// Sample rate and block size come exclusively from [`Program::config`].
pub struct MirCompileOptions {
    pub fast_math: bool,
    pub opt_level: TargetOptLevel,
}

#[derive(Debug, Clone)]
/// Target-machine policy for MIR-native IR and object emission.
pub struct MirTargetOptions {
    pub fast_math: bool,
    pub target: crate::TargetConfig,
}

impl Default for MirTargetOptions {
    fn default() -> Self {
        Self {
            fast_math: false,
            target: crate::TargetConfig::host(),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// Host payload shape for a MIR event entry point.
pub enum MirEventPayloadShape {
    Fixed { byte_size: usize },
    Dynamic,
}

impl Default for MirCompileOptions {
    fn default() -> Self {
        Self {
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        }
    }
}

type NativeProcessFn = unsafe extern "C" fn(
    *mut u8,
    *const u8,
    *const *const u8,
    *const *mut u8,
    u32,
    u32,
    u32,
    *const *mut u8,
    *const i32,
    *const i32,
    *const f32,
    *mut onda_processor_abi::ExecutionOutput,
) -> u32;
type NativeInitFn = unsafe extern "C" fn(
    *const u8,
    *mut u8,
    u32,
    *const *mut u8,
    *const i32,
    *const i32,
    *const f32,
    *mut onda_processor_abi::ExecutionOutput,
) -> u32;
type NativeEventFn = unsafe extern "C" fn(
    *const u8,
    *const u8,
    *mut u8,
    *const *mut u8,
    *const i32,
    *const i32,
    *const f32,
    *mut onda_processor_abi::ExecutionOutput,
) -> u32;

#[derive(Debug)]
struct NativeOrcProcess {
    lljit: LLVMOrcLLJITRef,
    process: NativeProcessFn,
    init: NativeInitFn,
    events: Vec<NativeEventFn>,
}

// SAFETY: construction finishes all LLJIT mutation before this owner is published. The stored
// entrypoints are immutable code addresses whose mutable data is supplied entirely by each
// runtime instance. LLVM permits concurrent execution of compiled code, and the Arc-backed
// MirJitProgram owner prevents LLJIT disposal until no caller can retain an entrypoint.
unsafe impl Send for NativeOrcProcess {}
// SAFETY: shared access performs no LLJIT mutation. Process/init/event calls receive disjoint
// host-owned runtime storage, so synchronization belongs to the exclusive Instance owner rather
// than this immutable executable owner.
unsafe impl Sync for NativeOrcProcess {}

impl Drop for NativeOrcProcess {
    fn drop(&mut self) {
        unsafe {
            let error = LLVMOrcDisposeLLJIT(self.lljit);
            if !error.is_null() {
                let message = LLVMGetErrorMessage(error);
                if !message.is_null() {
                    LLVMDisposeErrorMessage(message);
                }
            }
        }
    }
}

#[derive(Debug)]
/// Executable native code compiled directly from validated Onda MIR.
///
/// Supports scalar and fixed scalar-array storage, runtime slices and buffers,
/// packed parameter/event data, audio and control I/O, const data, user calls,
/// structured control flow, scalar operations, casts, comparisons, and MIR
/// intrinsics. Tuples and structs remain outside this lowering boundary. The
/// external process symbol retains the 12-argument host ABI while the MIR body
/// consumes its validated `(start_frame, frames, flags)` value parameters and
/// supports arbitrary legal block segments.
pub struct MirJitProgram {
    mir: Program,
    layouts: NativeLayouts,
    compiled: NativeOrcProcess,
}

#[derive(Debug, Clone)]
struct RegionLayout {
    offsets: Vec<usize>,
    size: usize,
    alignment: usize,
}

#[derive(Debug, Clone)]
struct EventPayloadLayout {
    fixed_size: Option<usize>,
}

#[derive(Debug, Clone)]
struct NativeLayouts {
    state: RegionLayout,
    params: RegionLayout,
    type_alignments: Vec<usize>,
    type_sizes: Vec<usize>,
    control_offsets: Vec<usize>,
    event_payloads: Vec<EventPayloadLayout>,
    input_bases: Vec<usize>,
    input_count: usize,
    output_bases: Vec<usize>,
    output_count: usize,
}

struct LoweredTypes {
    values: Vec<LLVMTypeRef>,
}

impl LoweredTypes {
    unsafe fn build(context: LLVMContextRef, program: &Program) -> Result<Self, MirCodegenError> {
        let mut values = vec![None; program.types.len()];
        let mut visiting = vec![false; program.types.len()];
        for index in 0..program.types.len() {
            lower_type_recursive(context, program, index, &mut values, &mut visiting)?;
        }
        Ok(Self {
            values: values
                .into_iter()
                .map(|value| value.expect("all validated MIR types are lowered"))
                .collect(),
        })
    }

    fn get(&self, id: onda_mir::TypeId) -> LLVMTypeRef {
        self.values[id.index()]
    }
}

unsafe fn lower_type_recursive(
    context: LLVMContextRef,
    program: &Program,
    index: usize,
    values: &mut [Option<LLVMTypeRef>],
    visiting: &mut [bool],
) -> Result<LLVMTypeRef, MirCodegenError> {
    if let Some(value) = values[index] {
        return Ok(value);
    }
    if visiting[index] {
        return Err(MirCodegenError::unsupported(format!(
            "MIR type {index} is recursively defined"
        )));
    }
    visiting[index] = true;
    let value = match &program.types[index] {
        Type::Scalar(scalar) => llvm_scalar_type(context, *scalar),
        Type::Array { element, len } => {
            let element =
                lower_type_recursive(context, program, element.index(), values, visiting)?;
            LLVMArrayType2(element, u64::from(*len))
        }
        Type::Slice { .. } => {
            let i8_ty = LLVMInt8TypeInContext(context);
            let mut fields = [
                LLVMPointerType(i8_ty, 0),
                LLVMPointerType(i8_ty, 0),
                LLVMInt32TypeInContext(context),
                LLVMInt32TypeInContext(context),
            ];
            LLVMStructTypeInContext(context, fields.as_mut_ptr(), fields.len() as u32, 0)
        }
        Type::Buffer { .. } => {
            let i8_ty = LLVMInt8TypeInContext(context);
            let mut fields = [
                LLVMPointerType(i8_ty, 0),
                LLVMPointerType(i8_ty, 0),
                LLVMInt32TypeInContext(context),
                LLVMInt32TypeInContext(context),
                LLVMFloatTypeInContext(context),
                LLVMInt1TypeInContext(context),
            ];
            LLVMStructTypeInContext(context, fields.as_mut_ptr(), fields.len() as u32, 0)
        }
        Type::BufferSpan { .. } => {
            let i8_ty = LLVMInt8TypeInContext(context);
            let pointer = LLVMPointerType(i8_ty, 0);
            let mut fields = [pointer; 6];
            LLVMStructTypeInContext(context, fields.as_mut_ptr(), fields.len() as u32, 0)
        }
        unsupported => {
            return Err(MirCodegenError::unsupported(format!(
                "cannot lower MIR type {unsupported:?}"
            )));
        }
    };
    visiting[index] = false;
    values[index] = Some(value);
    Ok(value)
}

unsafe fn llvm_scalar_type(context: LLVMContextRef, scalar: onda_mir::ScalarType) -> LLVMTypeRef {
    match scalar {
        onda_mir::ScalarType::F32 => LLVMFloatTypeInContext(context),
        onda_mir::ScalarType::F64 => LLVMDoubleTypeInContext(context),
        onda_mir::ScalarType::I32 => LLVMInt32TypeInContext(context),
        onda_mir::ScalarType::I64 => LLVMInt64TypeInContext(context),
        onda_mir::ScalarType::Bool => LLVMInt1TypeInContext(context),
    }
}

unsafe fn compute_native_layouts(
    program: &Program,
    types: &LoweredTypes,
    data_layout: &str,
) -> Result<NativeLayouts, MirCodegenError> {
    let data_layout_c = CString::new(data_layout)
        .map_err(|_| MirCodegenError::llvm("LLVM data layout contains an interior NUL"))?;
    let target_data = LLVMCreateTargetData(data_layout_c.as_ptr());
    if target_data.is_null() {
        return Err(MirCodegenError::llvm("failed to create LLVM target data"));
    }

    let result = (|| {
        let type_alignments = program
            .types
            .iter()
            .enumerate()
            .map(|(index, _)| {
                usize::try_from(LLVMABIAlignmentOfType(
                    target_data,
                    types.get(onda_mir::TypeId::new(index as u32)),
                ))
                .map_err(|_| MirCodegenError::llvm("LLVM ABI alignment does not fit usize"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let type_sizes = program
            .types
            .iter()
            .enumerate()
            .map(|(index, _)| {
                usize::try_from(LLVMABISizeOfType(
                    target_data,
                    types.get(onda_mir::TypeId::new(index as u32)),
                ))
                .map_err(|_| MirCodegenError::llvm("LLVM ABI type size does not fit usize"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let state_types = program.state.iter().map(|slot| slot.ty).collect::<Vec<_>>();
        let state = aligned_region_layout(&state_types, types, target_data)?;
        let mut control_offsets = Vec::with_capacity(program.interface.control_outputs.len());
        for output in &program.interface.control_outputs {
            control_offsets.push(state.offsets[output.mirror.index()]);
        }

        let param_types = program
            .interface
            .params
            .iter()
            .map(|param| param.ty)
            .collect::<Vec<_>>();
        let params = packed_region_layout(&param_types, types, target_data)?;
        let event_payloads = program
            .interface
            .events
            .iter()
            .map(|event| {
                let mut size = 0usize;
                for parameter in &event.params {
                    let Some(parameter_size) = fixed_payload_type_size(program, parameter.ty)?
                    else {
                        return Ok(EventPayloadLayout { fixed_size: None });
                    };
                    size = size.checked_add(parameter_size).ok_or_else(|| {
                        MirCodegenError::unsupported("event payload size overflow")
                    })?;
                }
                Ok(EventPayloadLayout {
                    fixed_size: Some(size),
                })
            })
            .collect::<Result<Vec<_>, MirCodegenError>>()?;

        let (input_bases, input_count) = interface_port_layout(
            program,
            program.interface.inputs.iter().map(|input| input.ty),
        )?;
        let (output_bases, output_count) = interface_port_layout(
            program,
            program.interface.outputs.iter().map(|output| output.ty),
        )?;

        Ok(NativeLayouts {
            state,
            params,
            type_alignments,
            type_sizes,
            control_offsets,
            event_payloads,
            input_bases,
            input_count,
            output_bases,
            output_count,
        })
    })();
    LLVMDisposeTargetData(target_data);
    result
}

unsafe fn aligned_region_layout(
    ids: &[onda_mir::TypeId],
    types: &LoweredTypes,
    target_data: LLVMTargetDataRef,
) -> Result<RegionLayout, MirCodegenError> {
    let mut offsets = Vec::with_capacity(ids.len());
    let mut size = 0usize;
    let mut region_alignment = 1usize;
    for id in ids {
        let ty = types.get(*id);
        let align = usize::try_from(LLVMABIAlignmentOfType(target_data, ty))
            .map_err(|_| MirCodegenError::llvm("LLVM ABI alignment does not fit usize"))?;
        region_alignment = region_alignment.max(align);
        size = align_up_checked(size, align)?;
        offsets.push(size);
        let item_size = usize::try_from(LLVMABISizeOfType(target_data, ty))
            .map_err(|_| MirCodegenError::llvm("LLVM ABI type size does not fit usize"))?;
        size = size
            .checked_add(item_size)
            .ok_or_else(|| MirCodegenError::unsupported("ABI region size overflow"))?;
    }
    size = align_up_checked(size, region_alignment)?;
    Ok(RegionLayout {
        offsets,
        size,
        alignment: region_alignment,
    })
}

unsafe fn packed_region_layout(
    ids: &[onda_mir::TypeId],
    types: &LoweredTypes,
    target_data: LLVMTargetDataRef,
) -> Result<RegionLayout, MirCodegenError> {
    let mut offsets = Vec::with_capacity(ids.len());
    let mut size = 0usize;
    for id in ids {
        offsets.push(size);
        let item_size = usize::try_from(LLVMStoreSizeOfType(target_data, types.get(*id)))
            .map_err(|_| MirCodegenError::llvm("LLVM store size does not fit usize"))?;
        size = size
            .checked_add(item_size)
            .ok_or_else(|| MirCodegenError::unsupported("packed ABI region size overflow"))?;
    }
    Ok(RegionLayout {
        offsets,
        size,
        alignment: 1,
    })
}

fn align_up_checked(value: usize, align: usize) -> Result<usize, MirCodegenError> {
    if align <= 1 {
        return Ok(value);
    }
    let remainder = value % align;
    if remainder == 0 {
        Ok(value)
    } else {
        value
            .checked_add(align - remainder)
            .ok_or_else(|| MirCodegenError::unsupported("ABI alignment overflow"))
    }
}

fn full_init_clear_ranges(
    program: &Program,
    layouts: &NativeLayouts,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    for (index, slot) in program.state.iter().enumerate() {
        if !slot.pinned {
            continue;
        }
        let start = layouts.state.offsets[index];
        if cursor < start {
            ranges.push(cursor..start);
        }
        let end = start + layouts.type_sizes[slot.ty.index()];
        debug_assert!(cursor <= start && end <= layouts.state.size);
        cursor = end;
    }
    if cursor < layouts.state.size {
        ranges.push(cursor..layouts.state.size);
    }
    ranges
}

fn alignment_at_byte_offset(base_alignment: usize, offset: usize) -> usize {
    if offset == 0 {
        return base_alignment;
    }
    base_alignment.min(1usize << offset.trailing_zeros())
}

fn interface_port_layout(
    program: &Program,
    ids: impl IntoIterator<Item = onda_mir::TypeId>,
) -> Result<(Vec<usize>, usize), MirCodegenError> {
    let mut bases = Vec::new();
    let mut count = 0usize;
    for id in ids {
        bases.push(count);
        let (_, width) = audio_port_shape(program, id).ok_or_else(|| {
            MirCodegenError::unsupported(
                "audio interfaces support only scalars and one-dimensional scalar arrays",
            )
        })?;
        count = count
            .checked_add(width)
            .ok_or_else(|| MirCodegenError::unsupported("audio port count overflow"))?;
    }
    Ok((bases, count))
}

fn audio_port_shape(program: &Program, id: onda_mir::TypeId) -> Option<(ScalarType, usize)> {
    match &program.types[id.index()] {
        Type::Scalar(scalar) => Some((*scalar, 1)),
        Type::Array { element, len } => match program.types[element.index()] {
            Type::Scalar(scalar) => Some((scalar, *len as usize)),
            _ => None,
        },
        _ => None,
    }
}

fn fallback_buffer_byte_count(program: &Program) -> Result<usize, MirCodegenError> {
    program
        .interface
        .buffers
        .iter()
        .try_fold(0, |maximum, buffer| {
            let element_size =
                usize::try_from(scalar_store_size(buffer.element)).map_err(|_| {
                    MirCodegenError::unsupported("buffer scalar size does not fit usize")
                })?;
            Ok(maximum.max(element_size))
        })
}

#[derive(Clone, Copy)]
struct FunctionDecl {
    value: LLVMValueRef,
    ty: LLVMTypeRef,
}

struct ModuleEmitter<'a> {
    program: &'a Program,
    effects: onda_mir::EffectAnalysis,
    ranges: onda_mir::ProgramRangeAnalysis,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    types: &'a LoweredTypes,
    layouts: &'a NativeLayouts,
    fast_math: bool,
    ptr_ty: LLVMTypeRef,
    runtime_context_ty: LLVMTypeRef,
    delegate_batch_ty: LLVMTypeRef,
    print_batch_ty: LLVMTypeRef,
    execution_output_ty: LLVMTypeRef,
    functions: Vec<FunctionDecl>,
    const_globals: Vec<LLVMValueRef>,
    host_alias_scopes: HostAliasScopes,
    range_metadata_kind: u32,
}

#[derive(Clone, Copy)]
struct HostAliasScopes {
    alias_scope_kind: u32,
    noalias_kind: u32,
    invariant_group_kind: u32,
    invariant_group: LLVMValueRef,
    audio_outputs: LLVMValueRef,
    buffer_descriptors: LLVMValueRef,
}

impl<'a> ModuleEmitter<'a> {
    unsafe fn new(
        program: &'a Program,
        context: LLVMContextRef,
        module: LLVMModuleRef,
        types: &'a LoweredTypes,
        layouts: &'a NativeLayouts,
        fast_math: bool,
    ) -> Result<Self, MirCodegenError> {
        let i8_ty = LLVMInt8TypeInContext(context);
        let ptr_ty = LLVMPointerType(i8_ty, 0);
        let i1_ty = LLVMInt1TypeInContext(context);
        let i32_ty = LLVMInt32TypeInContext(context);
        let mut delegate_batch_fields = [ptr_ty, i32_ty, i32_ty, i32_ty, i32_ty];
        let delegate_batch_ty = LLVMStructTypeInContext(
            context,
            delegate_batch_fields.as_mut_ptr(),
            delegate_batch_fields.len() as u32,
            0,
        );
        let print_batch_ty = delegate_batch_ty;
        let mut execution_output_fields = [ptr_ty, ptr_ty, i32_ty];
        let execution_output_ty = LLVMStructTypeInContext(
            context,
            execution_output_fields.as_mut_ptr(),
            execution_output_fields.len() as u32,
            0,
        );
        let mut runtime_fields = [
            ptr_ty, ptr_ty, i32_ty, i32_ty, i32_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty,
            ptr_ty, ptr_ty, i32_ty, ptr_ty, ptr_ty, i1_ty, ptr_ty, ptr_ty, ptr_ty,
        ];
        let runtime_context_ty = LLVMStructTypeInContext(
            context,
            runtime_fields.as_mut_ptr(),
            runtime_fields.len() as u32,
            0,
        );
        let effects = onda_mir::analyze_effects(program);
        let ranges = onda_mir::analyze_program_integer_ranges(program);
        let const_globals = build_const_globals(program, context, module)?;
        let functions =
            declare_functions(program, &effects, &ranges, context, module, types, layouts)?;
        let host_alias_scopes = build_host_alias_scopes(context);
        let range_metadata_kind = LLVMGetMDKindIDInContext(context, c"range".as_ptr(), 5);
        Ok(Self {
            program,
            effects,
            ranges,
            context,
            module,
            types,
            layouts,
            fast_math,
            ptr_ty,
            runtime_context_ty,
            delegate_batch_ty,
            print_batch_ty,
            execution_output_ty,
            functions,
            const_globals,
            host_alias_scopes,
            range_metadata_kind,
        })
    }

    unsafe fn emit(&self) -> Result<(), MirCodegenError> {
        for index in 0..self.program.functions.len() {
            emit_function_body(self, index)?;
        }
        Ok(())
    }
}

unsafe fn declare_functions(
    program: &Program,
    effects: &onda_mir::EffectAnalysis,
    program_ranges: &onda_mir::ProgramRangeAnalysis,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    types: &LoweredTypes,
    layouts: &NativeLayouts,
) -> Result<Vec<FunctionDecl>, MirCodegenError> {
    let void_ty = LLVMVoidTypeInContext(context);
    let i32_ty = LLVMInt32TypeInContext(context);
    let ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let mut declarations = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let function_id = onda_mir::FunctionId::new(index as u32);
        let ranges = program_ranges
            .function(function_id)
            .expect("program range analysis covers every MIR function");
        let (name, fn_ty, internal) = match function.kind {
            FunctionKind::Init => {
                let mut args = [
                    ptr_ty, ptr_ty, i32_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty,
                ];
                (
                    "onda_processor_init".to_owned(),
                    LLVMFunctionType(i32_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    false,
                )
            }
            FunctionKind::Process => {
                let mut args = [
                    ptr_ty, ptr_ty, ptr_ty, ptr_ty, i32_ty, i32_ty, i32_ty, ptr_ty, ptr_ty, ptr_ty,
                    ptr_ty, ptr_ty,
                ];
                (
                    "onda_process".to_owned(),
                    LLVMFunctionType(i32_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    false,
                )
            }
            FunctionKind::Event(event) => {
                let mut args = [
                    ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty,
                ];
                (
                    format!("onda_event_{}", event.raw()),
                    LLVMFunctionType(i32_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    false,
                )
            }
            FunctionKind::User => {
                let result_ty = match function.results.as_slice() {
                    [] => void_ty,
                    [result] => types.get(*result),
                    results => {
                        let mut result_types = results
                            .iter()
                            .map(|result| types.get(*result))
                            .collect::<Vec<_>>();
                        LLVMStructTypeInContext(
                            context,
                            result_types.as_mut_ptr(),
                            result_types.len() as u32,
                            0,
                        )
                    }
                };
                let mut args = Vec::with_capacity(function.params.len() + 1);
                args.push(ptr_ty);
                for param in &function.params {
                    args.push(match param.mode {
                        onda_mir::PassingMode::Value => types.get(param.ty),
                        onda_mir::PassingMode::ReadOnlyReference
                        | onda_mir::PassingMode::ReadWriteReference => ptr_ty,
                    });
                }
                (
                    format!("__onda_mir_fn_{index}"),
                    LLVMFunctionType(result_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    true,
                )
            }
        };
        let name = CString::new(name)
            .map_err(|_| MirCodegenError::llvm("generated LLVM symbol contains a NUL"))?;
        let value = LLVMAddFunction(module, name.as_ptr(), fn_ty);
        if value.is_null() {
            return Err(MirCodegenError::llvm(format!(
                "failed to declare MIR function {index}"
            )));
        }
        if internal {
            LLVMSetLinkage(value, LLVMLinkage::LLVMInternalLinkage);
        }
        let function_effects = effects.function(function_id);
        // MIR has no exceptions, allocation, atomics, or synchronization.
        // Carrying those source-level facts into LLVM is substantially more
        // robust than asking target passes to infer them through the opaque
        // runtime-context pointer.
        for attribute in ["nounwind", "nofree", "nosync"] {
            add_enum_attribute_at_index(
                context,
                value,
                llvm_sys::LLVMAttributeFunctionIndex,
                attribute,
                0,
            )?;
        }
        let writes_failure_context = function_effects.may_fail;
        if function_effects.is_memory_free() && !writes_failure_context {
            add_enum_attribute_at_index(
                context,
                value,
                llvm_sys::LLVMAttributeFunctionIndex,
                "memory",
                0,
            )?;
        } else if function_effects.is_read_only() && !writes_failure_context {
            // LLVM 21 encodes MemoryEffects as two Mod/Ref bits for each of
            // its four memory locations. `0b01` repeated is read-only.
            add_enum_attribute_at_index(
                context,
                value,
                llvm_sys::LLVMAttributeFunctionIndex,
                "memory",
                0b01_01_01_01,
            )?;
        }
        if !function_effects.may_not_return {
            add_enum_attribute_at_index(
                context,
                value,
                llvm_sys::LLVMAttributeFunctionIndex,
                "willreturn",
                0,
            )?;
        }
        match function.attributes.inline {
            onda_mir::InlineHint::Auto => {}
            onda_mir::InlineHint::Always => {
                add_enum_attribute_at_index(
                    context,
                    value,
                    llvm_sys::LLVMAttributeFunctionIndex,
                    "alwaysinline",
                    0,
                )?;
            }
            onda_mir::InlineHint::Never => {
                add_enum_attribute_at_index(
                    context,
                    value,
                    llvm_sys::LLVMAttributeFunctionIndex,
                    "noinline",
                    0,
                )?;
            }
        }
        match function.kind {
            FunctionKind::Init => {
                add_enum_param_attribute(context, value, 1, "readonly")?;
                add_enum_param_attribute(context, value, 2, "noalias")?;
                add_enum_param_attribute(context, value, 3, "noundef")?;
            }
            FunctionKind::Process => {
                add_enum_param_attribute(context, value, 1, "noalias")?;
                for attribute in ["noalias", "readonly"] {
                    add_enum_param_attribute(context, value, 2, attribute)?;
                    add_enum_param_attribute(context, value, 3, attribute)?;
                }
                add_enum_param_attribute(context, value, 4, "noalias")?;
                for parameter in [5_u32, 6, 7] {
                    add_enum_param_attribute(context, value, parameter, "noundef")?;
                }
                for (parameter, llvm_index) in [5_u32, 6, 7].into_iter().enumerate() {
                    let Some(range) =
                        ranges.parameter(onda_mir::ParameterId::new(parameter as u32))
                    else {
                        continue;
                    };
                    add_integer_range_attribute(
                        context,
                        value,
                        llvm_index,
                        analyzed_integer_value_range(range),
                    )?;
                }
            }
            FunctionKind::Event(_) => {
                for parameter in [1_u32, 2] {
                    add_enum_param_attribute(context, value, parameter, "readonly")?;
                }
                add_enum_param_attribute(context, value, 3, "noalias")?;
            }
            FunctionKind::User => {
                if function.results.len() == 1 {
                    if let Some(range) = ranges.result(0) {
                        add_integer_range_attribute(
                            context,
                            value,
                            llvm_sys::LLVMAttributeReturnIndex,
                            analyzed_integer_value_range(range),
                        )?;
                    }
                }
                for (index, parameter) in function.params.iter().enumerate() {
                    // LLVM parameter attributes are one-based, and every MIR
                    // user function has the opaque runtime context in slot 1.
                    let llvm_index = index as u32 + 2;
                    add_enum_param_attribute(context, value, llvm_index, "noundef")?;
                    if parameter.mode == onda_mir::PassingMode::Value {
                        if let Some(range) =
                            ranges.parameter(onda_mir::ParameterId::new(index as u32))
                        {
                            add_integer_range_attribute(
                                context,
                                value,
                                llvm_index,
                                analyzed_integer_value_range(range),
                            )?;
                        }
                        continue;
                    }
                    add_enum_param_attribute(context, value, llvm_index, "nonnull")?;
                    // LLVM 21 replaced the legacy `nocapture` spelling with
                    // the integer `captures(none)` parameter attribute. The
                    // encoded CaptureInfo for `none` is zero.
                    add_enum_attribute_at_index(context, value, llvm_index, "captures", 0)?;
                    add_enum_attribute_at_index(context, value, llvm_index, "align", 1)?;
                    add_enum_attribute_at_index(
                        context,
                        value,
                        llvm_index,
                        "dereferenceable",
                        layouts.type_sizes[parameter.ty.index()] as u64,
                    )?;
                    let parameter_effects = function_effects.parameters[index];
                    if parameter.mode == onda_mir::PassingMode::ReadOnlyReference
                        || !parameter_effects.writes
                    {
                        add_enum_param_attribute(context, value, llvm_index, "readonly")?;
                    } else if parameter_effects.writes && !parameter_effects.reads {
                        add_enum_param_attribute(context, value, llvm_index, "writeonly")?;
                    }
                }
            }
        }
        declarations.push(FunctionDecl { value, ty: fn_ty });
    }
    Ok(declarations)
}

fn analyzed_integer_value_range(range: onda_mir::IntegerRange) -> onda_mir::ValueRange {
    match range.scalar() {
        onda_mir::ScalarType::I32 => onda_mir::ValueRange {
            min: onda_mir::ScalarValue::I32(range.min() as i32),
            max: onda_mir::ScalarValue::I32(range.max() as i32),
        },
        onda_mir::ScalarType::I64 => onda_mir::ValueRange {
            min: onda_mir::ScalarValue::I64(range.min()),
            max: onda_mir::ScalarValue::I64(range.max()),
        },
        _ => unreachable!("integer analysis only produces integer scalar ranges"),
    }
}

fn invariant_value_range(range: onda_mir::IntegerRangeInvariant) -> onda_mir::ValueRange {
    onda_mir::ValueRange {
        min: range.min,
        max: range.max,
    }
}

fn llvm_integer_range_encoding(
    range: onda_mir::ValueRange,
) -> Option<(onda_mir::ScalarType, u64, u64)> {
    match (range.min, range.max) {
        (onda_mir::ScalarValue::I32(min), onda_mir::ScalarValue::I32(max)) => {
            if min == i32::MIN && max == i32::MAX {
                return None;
            }
            Some((
                onda_mir::ScalarType::I32,
                u64::from(min as u32),
                u64::from(max.wrapping_add(1) as u32),
            ))
        }
        (onda_mir::ScalarValue::I64(min), onda_mir::ScalarValue::I64(max)) => {
            if min == i64::MIN && max == i64::MAX {
                return None;
            }
            Some((
                onda_mir::ScalarType::I64,
                min as u64,
                max.wrapping_add(1) as u64,
            ))
        }
        _ => None,
    }
}

unsafe fn add_integer_range_attribute(
    context: LLVMContextRef,
    function: LLVMValueRef,
    index: u32,
    range: onda_mir::ValueRange,
) -> Result<(), MirCodegenError> {
    let Some((scalar, lower, upper)) = llvm_integer_range_encoding(range) else {
        return Ok(());
    };
    let name = "range";
    let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
    if kind == 0 {
        return Err(MirCodegenError::llvm(
            "failed to resolve LLVM 'range' attribute",
        ));
    }
    let bit_width = match scalar {
        onda_mir::ScalarType::I32 => 32,
        onda_mir::ScalarType::I64 => 64,
        _ => unreachable!("range attributes are only emitted for integer scalars"),
    };
    let attribute = LLVMCreateConstantRangeAttribute(context, kind, bit_width, &lower, &upper);
    LLVMAddAttributeAtIndex(function, index, attribute);
    Ok(())
}

unsafe fn add_enum_param_attribute(
    context: LLVMContextRef,
    function: LLVMValueRef,
    parameter_index: u32,
    name: &str,
) -> Result<(), MirCodegenError> {
    add_enum_attribute_at_index(context, function, parameter_index, name, 0)
}

unsafe fn add_enum_attribute_at_index(
    context: LLVMContextRef,
    function: LLVMValueRef,
    index: u32,
    name: &str,
    value: u64,
) -> Result<(), MirCodegenError> {
    let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
    if kind == 0 {
        return Err(MirCodegenError::llvm(format!(
            "failed to resolve LLVM attribute '{name}'"
        )));
    }
    let attribute = LLVMCreateEnumAttribute(context, kind, value);
    LLVMAddAttributeAtIndex(function, index, attribute);
    Ok(())
}

unsafe fn add_enum_callsite_attribute(
    context: LLVMContextRef,
    call: LLVMValueRef,
    index: u32,
    name: &str,
    value: u64,
) -> Result<(), MirCodegenError> {
    let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
    if kind == 0 {
        return Err(MirCodegenError::llvm(format!(
            "failed to resolve LLVM call-site attribute '{name}'"
        )));
    }
    let attribute = LLVMCreateEnumAttribute(context, kind, value);
    LLVMAddCallSiteAttribute(call, index, attribute);
    Ok(())
}

unsafe fn build_const_globals(
    program: &Program,
    context: LLVMContextRef,
    module: LLVMModuleRef,
) -> Result<Vec<LLVMValueRef>, MirCodegenError> {
    let mut globals = Vec::with_capacity(program.const_data.len());
    for (index, data) in program.const_data.iter().enumerate() {
        let element_ty = llvm_scalar_type(context, data.element);
        let array_ty = LLVMArrayType2(element_ty, data.values.len() as u64);
        let mut values = data
            .values
            .iter()
            .map(|value| llvm_scalar_constant(context, *value))
            .collect::<Vec<_>>();
        let initializer = LLVMConstArray2(element_ty, values.as_mut_ptr(), values.len() as u64);
        let name = CString::new(format!("__onda_mir_const_{index}"))
            .map_err(|_| MirCodegenError::llvm("generated const symbol contains a NUL"))?;
        let global = LLVMAddGlobal(module, array_ty, name.as_ptr());
        LLVMSetInitializer(global, initializer);
        LLVMSetGlobalConstant(global, 1);
        LLVMSetLinkage(global, LLVMLinkage::LLVMInternalLinkage);
        globals.push(global);
    }
    Ok(globals)
}

unsafe fn build_host_alias_scopes(context: LLVMContextRef) -> HostAliasScopes {
    // The raw ABI keeps descriptor tables stable and separate from every
    // audio/buffer storage region for the duration of an entry-point call.
    // Express that contract locally so LLVM can hoist invariant collection
    // selections without claiming either program-wide memory invariance or
    // that the descriptor tables are mutually disjoint (read and write
    // pointer tables may legitimately alias). `invariant.group` is tied to
    // the load's pointer SSA value, so a fresh entry-point invocation may bind
    // a different value at the same host address.
    let domain_name = "onda.host_regions";
    let domain_label =
        LLVMMDStringInContext2(context, domain_name.as_ptr().cast(), domain_name.len());
    let domain = self_referential_metadata_node(context, &[domain_label]);

    let output_name = "onda.audio_outputs";
    let output_label =
        LLVMMDStringInContext2(context, output_name.as_ptr().cast(), output_name.len());
    let output_scope = self_referential_metadata_node(context, &[domain, output_label]);

    let descriptor_name = "onda.buffer_descriptors";
    let descriptor_label = LLVMMDStringInContext2(
        context,
        descriptor_name.as_ptr().cast(),
        descriptor_name.len(),
    );
    let descriptor_scope = self_referential_metadata_node(context, &[domain, descriptor_label]);

    let mut output_scopes = [output_scope];
    let output_list =
        LLVMMDNodeInContext2(context, output_scopes.as_mut_ptr(), output_scopes.len());
    let mut descriptor_scopes = [descriptor_scope];
    let descriptor_list = LLVMMDNodeInContext2(
        context,
        descriptor_scopes.as_mut_ptr(),
        descriptor_scopes.len(),
    );
    let invariant_group = LLVMMDNodeInContext2(context, std::ptr::null_mut(), 0);
    HostAliasScopes {
        alias_scope_kind: LLVMGetMDKindIDInContext(context, c"alias.scope".as_ptr(), 11),
        noalias_kind: LLVMGetMDKindIDInContext(context, c"noalias".as_ptr(), 7),
        invariant_group_kind: LLVMGetMDKindIDInContext(context, c"invariant.group".as_ptr(), 15),
        invariant_group: LLVMMetadataAsValue(context, invariant_group),
        audio_outputs: LLVMMetadataAsValue(context, output_list),
        buffer_descriptors: LLVMMetadataAsValue(context, descriptor_list),
    }
}

unsafe fn self_referential_metadata_node(
    context: LLVMContextRef,
    trailing: &[LLVMMetadataRef],
) -> LLVMMetadataRef {
    let mut operands = Vec::with_capacity(trailing.len() + 1);
    operands.push(std::ptr::null_mut());
    operands.extend_from_slice(trailing);
    let node = LLVMMDNodeInContext2(context, operands.as_mut_ptr(), operands.len());
    LLVMReplaceMDNodeOperandWith(LLVMMetadataAsValue(context, node), 0, node);
    node
}

unsafe fn llvm_scalar_constant(
    context: LLVMContextRef,
    value: onda_mir::ScalarValue,
) -> LLVMValueRef {
    match value {
        onda_mir::ScalarValue::F32(value) => {
            LLVMConstReal(LLVMFloatTypeInContext(context), f64::from(value))
        }
        onda_mir::ScalarValue::F64(value) => LLVMConstReal(LLVMDoubleTypeInContext(context), value),
        onda_mir::ScalarValue::I32(value) => {
            LLVMConstInt(LLVMInt32TypeInContext(context), value as u64, 1)
        }
        onda_mir::ScalarValue::I64(value) => {
            LLVMConstInt(LLVMInt64TypeInContext(context), value as u64, 1)
        }
        onda_mir::ScalarValue::Bool(value) => {
            LLVMConstInt(LLVMInt1TypeInContext(context), u64::from(value), 0)
        }
    }
}

fn fused_clamped_index_sources(
    program: &Program,
    function: &onda_mir::Function,
) -> Vec<Option<FusedClampedIndex>> {
    let mut writes = vec![0_usize; function.locals.len()];
    let mut candidates = vec![None; function.locals.len()];
    collect_local_writes(program, &function.body, false, &mut writes, &mut candidates);

    candidates
        .into_iter()
        .enumerate()
        .map(|(local, source)| {
            if writes[local] != 1 {
                return None;
            }
            let source = source?;
            if let onda_mir::Value::Local(source_local) = source {
                if writes[source_local.index()] != 1 {
                    return None;
                }
            }
            let scalar = match source {
                onda_mir::Value::Local(source_local) => {
                    match program.types[function.locals[source_local.index()].ty.index()] {
                        Type::Scalar(scalar) => scalar,
                        _ => return None,
                    }
                }
                onda_mir::Value::Constant(value) => value.ty(),
            };
            matches!(
                scalar,
                onda_mir::ScalarType::F32 | onda_mir::ScalarType::F64
            )
            .then_some(FusedClampedIndex { source, scalar })
        })
        .collect()
}

fn collect_local_writes(
    program: &Program,
    block: &Block,
    inside_loop: bool,
    writes: &mut [usize],
    candidates: &mut [Option<onda_mir::Value>],
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                if destination.projections.is_empty() {
                    if let onda_mir::PlaceBase::Local(local) = destination.base {
                        record_local_write(writes, local, inside_loop);
                        if !inside_loop {
                            candidates[local.index()] = match value {
                                Rvalue::Cast {
                                    value,
                                    to: onda_mir::ScalarType::I32,
                                } => Some(*value),
                                _ => None,
                            };
                        }
                    }
                }
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => {
                for result in results {
                    record_local_write(writes, *result, inside_loop);
                }
                for (parameter, argument) in
                    program.functions[function.index()].params.iter().zip(args)
                {
                    if parameter.mode != onda_mir::PassingMode::ReadWriteReference {
                        continue;
                    }
                    let place = match argument {
                        CallArgument::Place(place) => Some(place),
                        CallArgument::ArrayWindow { array, .. } => Some(array),
                        _ => None,
                    };
                    if let Some(place) = place {
                        if let onda_mir::PlaceBase::Local(local) = place.base {
                            record_local_write(writes, local, inside_loop);
                        }
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_local_writes(program, then_block, inside_loop, writes, candidates);
                collect_local_writes(program, else_block, inside_loop, writes, candidates);
            }
            StatementKind::Loop { body } => {
                collect_local_writes(program, body, true, writes, candidates);
            }
            _ => {}
        }
    }
}

fn record_local_write(writes: &mut [usize], local: onda_mir::LocalId, inside_loop: bool) {
    let writes = &mut writes[local.index()];
    *writes = if inside_loop {
        usize::MAX
    } else {
        writes.saturating_add(1)
    };
}

unsafe fn emit_function_body(
    module: &ModuleEmitter<'_>,
    function_index: usize,
) -> Result<(), MirCodegenError> {
    let function = &module.program.functions[function_index];
    let declaration = module.functions[function_index];
    let builder = LLVMCreateBuilderInContext(module.context);
    if builder.is_null() {
        return Err(MirCodegenError::llvm("failed to create LLVM builder"));
    }
    let prologue_builder = LLVMCreateBuilderInContext(module.context);
    if prologue_builder.is_null() {
        LLVMDisposeBuilder(builder);
        return Err(MirCodegenError::llvm(
            "failed to create LLVM prologue builder",
        ));
    }
    let result = (|| {
        let entry = append_block(module.context, declaration.value, "entry")?;
        LLVMPositionBuilderAtEnd(builder, entry);
        let (runtime_context, fallback_buffer_read, fallback_buffer_write) = match function.kind {
            FunctionKind::User => {
                let runtime_context = LLVMGetParam(declaration.value, 0);
                (
                    runtime_context,
                    load_context_field(
                        module,
                        builder,
                        runtime_context,
                        14,
                        "unbound_buffer_read",
                    )?,
                    load_context_field(
                        module,
                        builder,
                        runtime_context,
                        15,
                        "unbound_buffer_write",
                    )?,
                )
            }
            _ => build_entry_runtime_context(module, declaration.value, function.kind, builder)?,
        };
        // Direct buffer metadata is materialized in the entry block so it is
        // invariant for the duration of an entry-point call. Keep its fallback
        // operands there as well: delegate-batch reset introduces a branch, and
        // values loaded after that branch would not dominate entry-block
        // snapshots inserted by `snapshot_direct_buffer_field`.
        let mut emitter = FunctionEmitter {
            module,
            function,
            ranges: module
                .ranges
                .function(onda_mir::FunctionId::new(function_index as u32))
                .expect("program range analysis covers every MIR function"),
            declaration,
            builder,
            prologue_builder,
            runtime_context,
            locals: Vec::with_capacity(function.locals.len()),
            parameters: Vec::with_capacity(function.params.len()),
            event_parameters: Vec::new(),
            loop_stack: Vec::new(),
            fused_clamped_indices: fused_clamped_index_sources(module.program, function),
            direct_buffer_fields: vec![[None; 6]; module.program.interface.buffers.len()],
            fallback_buffer_read,
            fallback_buffer_write,
        };
        emitter.allocate_storage()?;
        emitter.lower_block(&function.body)?;
        if !current_block_terminated(builder) {
            if !matches!(function.kind, FunctionKind::User) {
                LLVMBuildRet(
                    builder,
                    LLVMConstInt(LLVMInt32TypeInContext(module.context), 0, 0),
                );
            } else if function.results.is_empty() {
                LLVMBuildRetVoid(builder);
            } else {
                return Err(MirCodegenError::invalid(format!(
                    "MIR function {function_index} falls through without returning"
                )));
            }
        }
        Ok(())
    })();
    LLVMDisposeBuilder(prologue_builder);
    LLVMDisposeBuilder(builder);
    result
}

#[derive(Clone, Copy)]
struct PlaceRef {
    ptr: LLVMValueRef,
    ty: onda_mir::TypeId,
    alignment: usize,
}

#[derive(Clone, Copy)]
struct BufferParts {
    read_ptr: LLVMValueRef,
    write_ptr: LLVMValueRef,
    frames: LLVMValueRef,
    channels: LLVMValueRef,
    element: onda_mir::ScalarType,
}

#[derive(Clone, Copy)]
struct SliceParts {
    read_ptr: LLVMValueRef,
    write_ptr: LLVMValueRef,
    len: LLVMValueRef,
    stride_bytes: LLVMValueRef,
    element: onda_mir::ScalarType,
}

#[derive(Clone, Copy)]
struct FusedClampedIndex {
    source: onda_mir::Value,
    scalar: onda_mir::ScalarType,
}

struct FunctionEmitter<'a, 'm> {
    module: &'m ModuleEmitter<'a>,
    function: &'a onda_mir::Function,
    ranges: &'m onda_mir::FunctionRangeAnalysis,
    declaration: FunctionDecl,
    builder: LLVMBuilderRef,
    /// A separate insertion cursor used to materialize stable host-buffer
    /// descriptor fields in the function entry block. Bindings may change
    /// between entry-point calls, but remain fixed for the duration of one
    /// call, so these SSA snapshots cannot become stale.
    prologue_builder: LLVMBuilderRef,
    runtime_context: LLVMValueRef,
    locals: Vec<PlaceRef>,
    parameters: Vec<PlaceRef>,
    event_parameters: Vec<PlaceRef>,
    loop_stack: Vec<(LLVMBasicBlockRef, LLVMBasicBlockRef)>,
    fused_clamped_indices: Vec<Option<FusedClampedIndex>>,
    direct_buffer_fields: Vec<[Option<LLVMValueRef>; 6]>,
    fallback_buffer_read: LLVMValueRef,
    fallback_buffer_write: LLVMValueRef,
}

unsafe fn reset_delegate_batch(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    runtime_context: LLVMValueRef,
) -> Result<(), MirCodegenError> {
    let batch = load_context_field(
        module,
        builder,
        runtime_context,
        DELEGATE_BATCH_CONTEXT_INDEX,
        "delegate_batch",
    )?;
    let present = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntNE,
        batch,
        LLVMConstPointerNull(module.ptr_ty),
        c_name("delegate_batch_present")?.as_ptr(),
    );
    let reset = append_block(
        module.context,
        LLVMGetBasicBlockParent(LLVMGetInsertBlock(builder)),
        "delegate_batch_reset",
    )?;
    let done = append_block(
        module.context,
        LLVMGetBasicBlockParent(LLVMGetInsertBlock(builder)),
        "delegate_batch_reset_done",
    )?;
    LLVMBuildCondBr(builder, present, reset, done);
    LLVMPositionBuilderAtEnd(builder, reset);
    let zero = LLVMConstInt(LLVMInt32TypeInContext(module.context), 0, 0);
    for field in 2..=4 {
        let pointer = LLVMBuildStructGEP2(
            builder,
            module.delegate_batch_ty,
            batch,
            field,
            c_name("delegate_batch_counter")?.as_ptr(),
        );
        LLVMBuildStore(builder, zero, pointer);
    }
    LLVMBuildBr(builder, done);
    LLVMPositionBuilderAtEnd(builder, done);
    Ok(())
}

unsafe fn build_entry_runtime_context(
    module: &ModuleEmitter<'_>,
    function: LLVMValueRef,
    kind: FunctionKind,
    builder: LLVMBuilderRef,
) -> Result<(LLVMValueRef, LLVMValueRef, LLVMValueRef), MirCodegenError> {
    let context = LLVMBuildAlloca(
        builder,
        module.runtime_context_ty,
        c_name("runtime_context")?.as_ptr(),
    );
    let null = LLVMConstPointerNull(module.ptr_ty);
    let zero = LLVMConstInt(LLVMInt32TypeInContext(module.context), 0, 0);
    let false_value = LLVMConstInt(LLVMInt1TypeInContext(module.context), 0, 0);
    let mut fields = [
        null,        // 0: audio inputs
        null,        // 1: audio outputs
        zero,        // 2: frames
        zero,        // 3: input channels
        zero,        // 4: output channels
        null,        // 5: params
        null,        // 6: state
        null,        // 7: buffer reads
        null,        // 8: buffer writes
        null,        // 9: buffer frames
        null,        // 10: buffer channels
        null,        // 11: buffer sample rates
        null,        // 12: event args
        zero,        // 13: runtime failure
        null,        // 14: unbound-buffer read fallback
        null,        // 15: unbound-buffer write fallback
        false_value, // 16: init all
        null,        // 17: delegate batch
        null,        // 18: print batch
        null,        // 19: output sequence
    ];
    let (fallback_read, fallback_write) = allocate_entry_fallback_buffers(module, builder)?;
    fields[14] = fallback_read;
    fields[15] = fallback_write;
    match kind {
        FunctionKind::Init => {
            fields[5] = LLVMGetParam(function, 0);
            fields[6] = LLVMGetParam(function, 1);
            let buffers =
                entry_buffer_descriptor_table(module, builder, LLVMGetParam(function, 3))?;
            fields[7] = buffers;
            fields[8] = buffers;
            for (field, parameter) in (9..=11).zip(4..=6) {
                fields[field] = entry_buffer_descriptor_table(
                    module,
                    builder,
                    LLVMGetParam(function, parameter),
                )?;
            }
            let all_value = LLVMGetParam(function, 2);
            let all = LLVMBuildICmp(
                builder,
                llvm_sys::LLVMIntPredicate::LLVMIntNE,
                all_value,
                LLVMConstInt(LLVMInt32TypeInContext(module.context), 0, 0),
                c_name("init_all")?.as_ptr(),
            );
            fields[INIT_ALL_CONTEXT_INDEX as usize] = all;
        }
        FunctionKind::Process => {
            fields[6] = LLVMGetParam(function, 0);
            fields[5] = LLVMGetParam(function, 1);
            fields[0] = LLVMGetParam(function, 2);
            fields[1] = LLVMGetParam(function, 3);
            fields[2] = LLVMGetParam(function, 4);
            fields[3] = LLVMGetParam(function, 5);
            fields[4] = LLVMGetParam(function, 6);
            let buffers =
                entry_buffer_descriptor_table(module, builder, LLVMGetParam(function, 7))?;
            fields[7] = buffers;
            fields[8] = buffers;
            for (field, parameter) in (9..=11).zip(8..=10) {
                fields[field] = entry_buffer_descriptor_table(
                    module,
                    builder,
                    LLVMGetParam(function, parameter),
                )?;
            }
        }
        FunctionKind::Event(_) => {
            fields[12] = LLVMGetParam(function, 0);
            fields[5] = LLVMGetParam(function, 1);
            fields[6] = LLVMGetParam(function, 2);
            let buffers =
                entry_buffer_descriptor_table(module, builder, LLVMGetParam(function, 3))?;
            fields[7] = buffers;
            fields[8] = buffers;
            for (field, parameter) in (9..=11).zip(4..=6) {
                fields[field] = entry_buffer_descriptor_table(
                    module,
                    builder,
                    LLVMGetParam(function, parameter),
                )?;
            }
        }
        FunctionKind::User => unreachable!(),
    }
    // Materialize the ordinary entry context before inspecting the nullable
    // execution-output pointer. Direct buffer metadata snapshots are inserted
    // in the entry block and must see these stores before that inspection
    // introduces control flow.
    for (index, value) in fields.iter().copied().enumerate() {
        let ptr = LLVMBuildStructGEP2(
            builder,
            module.runtime_context_ty,
            context,
            index as u32,
            c_name("runtime_field")?.as_ptr(),
        );
        LLVMBuildStore(builder, value, ptr);
    }

    let needs_delegate_batch = !module.program.interface.delegates.is_empty();
    let needs_print_batch = !module.program.log_sites.is_empty();
    let (delegate_batch, print_batch, output_sequence) =
        if needs_delegate_batch || needs_print_batch {
            let output = match kind {
                FunctionKind::Init => LLVMGetParam(function, 7),
                FunctionKind::Process => LLVMGetParam(function, 11),
                FunctionKind::Event(_) => LLVMGetParam(function, 7),
                FunctionKind::User => unreachable!(),
            };
            load_execution_output_batches(
                module,
                builder,
                output,
                needs_delegate_batch,
                needs_print_batch,
            )?
        } else {
            (null, null, null)
        };
    for (index, value) in [
        (DELEGATE_BATCH_CONTEXT_INDEX, delegate_batch),
        (PRINT_BATCH_CONTEXT_INDEX, print_batch),
        (OUTPUT_SEQUENCE_CONTEXT_INDEX, output_sequence),
    ] {
        let ptr = LLVMBuildStructGEP2(
            builder,
            module.runtime_context_ty,
            context,
            index,
            c_name("runtime_field")?.as_ptr(),
        );
        LLVMBuildStore(builder, value, ptr);
    }
    if matches!(kind, FunctionKind::Init) {
        let all = fields[INIT_ALL_CONTEXT_INDEX as usize];
        let clear = append_block(module.context, function, "init_clear")?;
        let initialized = append_block(module.context, function, "init_cleared")?;
        LLVMBuildCondBr(builder, all, clear, initialized);

        LLVMPositionBuilderAtEnd(builder, clear);
        // Pinned declarations run on this path and fully overwrite their
        // slots. Clear only the complementary byte ranges so large pinned
        // arrays are not written twice. Padding remains in the clear set,
        // keeping the complete physical state image initialized.
        for range in full_init_clear_ranges(module.program, module.layouts) {
            let destination = byte_offset_ptr(
                module.context,
                builder,
                fields[6],
                range.start,
                "init_clear_range",
            )?;
            LLVMBuildMemSet(
                builder,
                destination,
                LLVMConstInt(LLVMInt8TypeInContext(module.context), 0, 0),
                LLVMConstInt(
                    LLVMInt64TypeInContext(module.context),
                    (range.end - range.start) as u64,
                    0,
                ),
                alignment_at_byte_offset(module.layouts.state.alignment, range.start) as u32,
            );
        }
        LLVMBuildBr(builder, initialized);
        LLVMPositionBuilderAtEnd(builder, initialized);
    }
    Ok((context, fallback_read, fallback_write))
}

unsafe fn allocate_entry_fallback_buffers(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
) -> Result<(LLVMValueRef, LLVMValueRef), MirCodegenError> {
    let byte_count = fallback_buffer_byte_count(module.program)?;
    if byte_count == 0 {
        let null = LLVMConstPointerNull(module.ptr_ty);
        return Ok((null, null));
    }
    let i8_ty = LLVMInt8TypeInContext(module.context);
    let storage_ty = LLVMArrayType2(i8_ty, byte_count as u64);
    let read = LLVMBuildAlloca(builder, storage_ty, c_name("unbound_buffer_read")?.as_ptr());
    let write = LLVMBuildAlloca(
        builder,
        storage_ty,
        c_name("unbound_buffer_write")?.as_ptr(),
    );
    LLVMSetAlignment(read, 8);
    LLVMSetAlignment(write, 8);
    LLVMBuildMemSet(
        builder,
        read,
        LLVMConstInt(i8_ty, 0, 0),
        LLVMConstInt(LLVMInt64TypeInContext(module.context), byte_count as u64, 0),
        8,
    );
    Ok((read, write))
}

unsafe fn load_execution_output_batches(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    output: LLVMValueRef,
    load_delegate: bool,
    load_print: bool,
) -> Result<(LLVMValueRef, LLVMValueRef, LLVMValueRef), MirCodegenError> {
    let null = LLVMConstPointerNull(module.ptr_ty);
    let delegate_slot = LLVMBuildAlloca(
        builder,
        module.ptr_ty,
        c_name("execution_delegate_batch_slot")?.as_ptr(),
    );
    let print_slot = LLVMBuildAlloca(
        builder,
        module.ptr_ty,
        c_name("execution_print_batch_slot")?.as_ptr(),
    );
    let sequence_slot = LLVMBuildAlloca(
        builder,
        module.ptr_ty,
        c_name("execution_sequence_slot")?.as_ptr(),
    );
    LLVMBuildStore(builder, null, delegate_slot);
    LLVMBuildStore(builder, null, print_slot);
    LLVMBuildStore(builder, null, sequence_slot);
    let present = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntNE,
        output,
        null,
        c_name("execution_output_present")?.as_ptr(),
    );
    let load = append_block(
        module.context,
        LLVMGetBasicBlockParent(LLVMGetInsertBlock(builder)),
        "execution_output_load",
    )?;
    let done = append_block(
        module.context,
        LLVMGetBasicBlockParent(LLVMGetInsertBlock(builder)),
        "execution_output_loaded",
    )?;
    LLVMBuildCondBr(builder, present, load, done);
    LLVMPositionBuilderAtEnd(builder, load);
    for (index, slot, enabled) in [
        (0_u32, delegate_slot, load_delegate),
        (1_u32, print_slot, load_print),
    ] {
        if !enabled {
            continue;
        }
        let pointer = LLVMBuildStructGEP2(
            builder,
            module.execution_output_ty,
            output,
            index,
            c_name("execution_output_batch_ptr")?.as_ptr(),
        );
        let batch = LLVMBuildLoad2(
            builder,
            module.ptr_ty,
            pointer,
            c_name("execution_output_batch")?.as_ptr(),
        );
        LLVMBuildStore(builder, batch, slot);
    }
    let sequence = LLVMBuildStructGEP2(
        builder,
        module.execution_output_ty,
        output,
        2,
        c_name("execution_output_sequence_ptr")?.as_ptr(),
    );
    LLVMBuildStore(builder, sequence, sequence_slot);
    LLVMBuildBr(builder, done);
    LLVMPositionBuilderAtEnd(builder, done);
    Ok((
        LLVMBuildLoad2(
            builder,
            module.ptr_ty,
            delegate_slot,
            c_name("execution_delegate_batch")?.as_ptr(),
        ),
        LLVMBuildLoad2(
            builder,
            module.ptr_ty,
            print_slot,
            c_name("execution_print_batch")?.as_ptr(),
        ),
        LLVMBuildLoad2(
            builder,
            module.ptr_ty,
            sequence_slot,
            c_name("execution_sequence")?.as_ptr(),
        ),
    ))
}

unsafe fn entry_buffer_descriptor_table(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    ptr: LLVMValueRef,
) -> Result<LLVMValueRef, MirCodegenError> {
    if module.program.interface.buffers.is_empty() {
        return Ok(ptr);
    }
    let mut parameter_types = [module.ptr_ty];
    let fn_ty = LLVMFunctionType(
        module.ptr_ty,
        parameter_types.as_mut_ptr(),
        parameter_types.len() as u32,
        0,
    );
    let function = ensure_named_function(module.module, "llvm.launder.invariant.group.p0", fn_ty)?;
    Ok(LLVMBuildCall2(
        builder,
        fn_ty,
        function,
        [ptr].as_mut_ptr(),
        1,
        c_name("buffer_descriptor_group")?.as_ptr(),
    ))
}

unsafe fn load_context_field(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    context: LLVMValueRef,
    index: u32,
    name: &str,
) -> Result<LLVMValueRef, MirCodegenError> {
    let ptr = context_field_ptr(module, builder, context, index)?;
    let ty = match index {
        INIT_ALL_CONTEXT_INDEX => LLVMInt1TypeInContext(module.context),
        2..=4 | RUNTIME_FAILURE_CONTEXT_INDEX => LLVMInt32TypeInContext(module.context),
        _ => module.ptr_ty,
    };
    Ok(LLVMBuildLoad2(builder, ty, ptr, c_name(name)?.as_ptr()))
}

unsafe fn context_field_ptr(
    module: &ModuleEmitter<'_>,
    builder: LLVMBuilderRef,
    context: LLVMValueRef,
    index: u32,
) -> Result<LLVMValueRef, MirCodegenError> {
    Ok(LLVMBuildStructGEP2(
        builder,
        module.runtime_context_ty,
        context,
        index,
        c_name("context_field")?.as_ptr(),
    ))
}

unsafe fn byte_offset_ptr(
    context: LLVMContextRef,
    builder: LLVMBuilderRef,
    base: LLVMValueRef,
    offset: usize,
    name: &str,
) -> Result<LLVMValueRef, MirCodegenError> {
    let offset = u64::try_from(offset)
        .map_err(|_| MirCodegenError::unsupported("byte offset does not fit u64"))?;
    let mut index = LLVMConstInt(LLVMInt64TypeInContext(context), offset, 0);
    Ok(LLVMBuildGEP2(
        builder,
        LLVMInt8TypeInContext(context),
        base,
        &mut index,
        1,
        c_name(name)?.as_ptr(),
    ))
}

unsafe fn ensure_named_function(
    module: LLVMModuleRef,
    name: &str,
    ty: LLVMTypeRef,
) -> Result<LLVMValueRef, MirCodegenError> {
    let name = c_name(name)?;
    let existing = LLVMGetNamedFunction(module, name.as_ptr());
    if !existing.is_null() {
        return Ok(existing);
    }
    let function = LLVMAddFunction(module, name.as_ptr(), ty);
    if function.is_null() {
        Err(MirCodegenError::llvm("failed to declare LLVM intrinsic"))
    } else {
        Ok(function)
    }
}

fn c_name(name: &str) -> Result<CString, MirCodegenError> {
    CString::new(name).map_err(|_| MirCodegenError::llvm("LLVM name contains a NUL"))
}

unsafe fn append_block(
    context: LLVMContextRef,
    function: LLVMValueRef,
    name: &str,
) -> Result<LLVMBasicBlockRef, MirCodegenError> {
    Ok(LLVMAppendBasicBlockInContext(
        context,
        function,
        c_name(name)?.as_ptr(),
    ))
}

unsafe fn current_block_terminated(builder: LLVMBuilderRef) -> bool {
    let block = LLVMGetInsertBlock(builder);
    !block.is_null() && !LLVMGetBasicBlockTerminator(block).is_null()
}

fn is_float(scalar: onda_mir::ScalarType) -> bool {
    matches!(
        scalar,
        onda_mir::ScalarType::F32 | onda_mir::ScalarType::F64
    )
}

fn is_integer(scalar: onda_mir::ScalarType) -> bool {
    matches!(
        scalar,
        onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
    )
}

fn scalar_store_size(scalar: onda_mir::ScalarType) -> u64 {
    match scalar {
        onda_mir::ScalarType::F32 | onda_mir::ScalarType::I32 => 4,
        onda_mir::ScalarType::F64 | onda_mir::ScalarType::I64 => 8,
        onda_mir::ScalarType::Bool => 1,
    }
}

fn fixed_payload_type_size(
    program: &Program,
    ty: onda_mir::TypeId,
) -> Result<Option<usize>, MirCodegenError> {
    match &program.types[ty.index()] {
        Type::Scalar(scalar) => Ok(Some(scalar_store_size(*scalar) as usize)),
        Type::Array { element, len } => {
            let Some(element_size) = fixed_payload_type_size(program, *element)? else {
                return Ok(None);
            };
            element_size
                .checked_mul(*len as usize)
                .map(Some)
                .ok_or_else(|| MirCodegenError::unsupported("event payload size overflow"))
        }
        Type::Slice { .. } => Ok(None),
        Type::Buffer { .. } => Err(MirCodegenError::unsupported(
            "buffer values cannot be serialized in event payloads",
        )),
        Type::BufferSpan { .. } => Err(MirCodegenError::unsupported(
            "buffer spans cannot be serialized in event payloads",
        )),
        Type::Tuple(_) | Type::Struct(_) => unreachable!("unsupported type rejected earlier"),
    }
}

fn validate_target_options(options: &MirTargetOptions) -> Result<(), MirCodegenError> {
    if options
        .target
        .triple
        .as_ref()
        .is_some_and(|triple| triple.trim().is_empty())
    {
        return Err(MirCodegenError::invalid(
            "target triple must not be empty when provided",
        ));
    }
    if matches!(&options.target.cpu, crate::TargetCpu::Explicit(cpu) if cpu.trim().is_empty()) {
        return Err(MirCodegenError::invalid(
            "target CPU must not be empty when explicitly provided",
        ));
    }
    if options
        .target
        .features
        .as_ref()
        .is_some_and(|features| features.contains(char::is_whitespace))
    {
        return Err(MirCodegenError::invalid(
            "target features must be a comma-separated LLVM feature string without whitespace",
        ));
    }
    if options
        .target
        .abi_name
        .as_ref()
        .is_some_and(|abi| abi.trim().is_empty())
    {
        return Err(MirCodegenError::invalid(
            "target ABI name must not be empty when provided",
        ));
    }
    Ok(())
}

fn validate_backend_capabilities(program: &Program) -> Result<(), Vec<MirCodegenError>> {
    let mut errors = Vec::new();
    for (index, state) in program.state.iter().enumerate() {
        if type_contains_runtime_descriptor(
            program,
            state.ty,
            &mut vec![false; program.types.len()],
        ) {
            errors.push(MirCodegenError::unsupported(format!(
                "MIR state slot {index} stores a runtime slice or buffer descriptor"
            )));
        }
    }
    for (surface, types) in [
        (
            "input",
            program
                .interface
                .inputs
                .iter()
                .map(|item| item.ty)
                .collect::<Vec<_>>(),
        ),
        (
            "output",
            program
                .interface
                .outputs
                .iter()
                .map(|item| item.ty)
                .collect::<Vec<_>>(),
        ),
        (
            "control output",
            program
                .interface
                .control_outputs
                .iter()
                .map(|item| item.ty)
                .collect::<Vec<_>>(),
        ),
        (
            "parameter",
            program
                .interface
                .params
                .iter()
                .map(|item| item.ty)
                .collect::<Vec<_>>(),
        ),
    ] {
        for (index, ty) in types.into_iter().enumerate() {
            if type_contains_runtime_descriptor(program, ty, &mut vec![false; program.types.len()])
            {
                errors.push(MirCodegenError::unsupported(format!(
                    "MIR {surface} {index} contains a runtime descriptor"
                )));
            }
        }
    }
    for (event_index, event) in program.interface.events.iter().enumerate() {
        for (parameter_index, parameter) in event.params.iter().enumerate() {
            if matches!(program.types[parameter.ty.index()], Type::Buffer { .. }) {
                errors.push(MirCodegenError::unsupported(format!(
                    "MIR event {event_index} parameter {parameter_index} is buffer-typed"
                )));
            }
        }
    }

    for index in 0..program.types.len() {
        if let Err(message) = require_fixed_type(
            program,
            onda_mir::TypeId::new(index as u32),
            &mut vec![false; program.types.len()],
        ) {
            errors.push(MirCodegenError::unsupported(format!(
                "MIR type {index} is outside the native LLVM slice: {message}"
            )));
        }
    }
    for (index, data) in program.const_data.iter().enumerate() {
        if data.values.is_empty() {
            errors.push(MirCodegenError::unsupported(format!(
                "constant-data item {index} is empty"
            )));
        }
    }
    for (function_index, function) in program.functions.iter().enumerate() {
        if matches!(function.kind, FunctionKind::Init | FunctionKind::Event(_))
            && !function.params.is_empty()
        {
            errors.push(MirCodegenError::unsupported(format!(
                "MIR entry function {function_index} has explicit parameters; entry ABI parameters must be represented by MIR places"
            )));
        }
        if !matches!(function.kind, FunctionKind::User) && !function.results.is_empty() {
            errors.push(MirCodegenError::unsupported(format!(
                "MIR entry function {function_index} returns values"
            )));
        }
        for (result_index, result) in function.results.iter().enumerate() {
            if type_contains_runtime_descriptor(
                program,
                *result,
                &mut vec![false; program.types.len()],
            ) {
                errors.push(MirCodegenError::unsupported(format!(
                    "MIR function {function_index} result {result_index} is a runtime descriptor"
                )));
            }
        }
        if matches!(function.kind, FunctionKind::User) {
            for (parameter_index, parameter) in function.params.iter().enumerate() {
                match program.types[parameter.ty.index()] {
                    Type::Slice { .. } if parameter.mode != onda_mir::PassingMode::Value => {
                        errors.push(MirCodegenError::unsupported(format!(
                            "MIR function {function_index} slice parameter {parameter_index} is not passed by value"
                        )));
                    }
                    Type::Buffer { .. } if parameter.mode == onda_mir::PassingMode::Value => {
                        errors.push(MirCodegenError::unsupported(format!(
                            "MIR function {function_index} buffer parameter {parameter_index} is not passed by reference"
                        )));
                    }
                    Type::BufferSpan { .. } if parameter.mode != onda_mir::PassingMode::Value => {
                        errors.push(MirCodegenError::unsupported(format!(
                            "MIR function {function_index} buffer span parameter {parameter_index} is not passed by value"
                        )));
                    }
                    _ => {}
                }
            }
        }
        inspect_block(program, function_index, &function.body, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn optimize_owned_program(
    program: Program,
) -> Result<onda_mir::OptimizedProgram, Vec<MirCodegenError>> {
    let validated = onda_mir::validate_owned(program).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| MirCodegenError::invalid(error.to_string()))
            .collect::<Vec<_>>()
    })?;
    onda_mir::optimize(validated)
        .map(|(program, _stats)| program)
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|error| MirCodegenError::invalid(error.to_string()))
                .collect()
        })
}

fn type_contains_runtime_descriptor(
    program: &Program,
    id: onda_mir::TypeId,
    visiting: &mut [bool],
) -> bool {
    if visiting[id.index()] {
        return false;
    }
    visiting[id.index()] = true;
    let result = match &program.types[id.index()] {
        Type::Slice { .. } | Type::Buffer { .. } | Type::BufferSpan { .. } => true,
        Type::Array { element, .. } => {
            type_contains_runtime_descriptor(program, *element, visiting)
        }
        Type::Tuple(elements) => elements
            .iter()
            .any(|element| type_contains_runtime_descriptor(program, *element, visiting)),
        Type::Struct(_) | Type::Scalar(_) => false,
    };
    visiting[id.index()] = false;
    result
}

fn require_fixed_type(
    program: &Program,
    id: onda_mir::TypeId,
    visiting: &mut [bool],
) -> Result<(), &'static str> {
    if visiting[id.index()] {
        return Err("recursive fixed-size type");
    }
    visiting[id.index()] = true;
    let result = match &program.types[id.index()] {
        Type::Scalar(_) => Ok(()),
        Type::Array { element, .. } => require_fixed_type(program, *element, visiting),
        Type::Tuple(_) => Err("tuples are not implemented"),
        Type::Struct(_) => Err("struct aggregates and field projections are not implemented"),
        Type::Slice { .. } | Type::Buffer { .. } | Type::BufferSpan { .. } => Ok(()),
    };
    visiting[id.index()] = false;
    result
}

fn inspect_block(
    program: &Program,
    function_index: usize,
    block: &Block,
    errors: &mut Vec<MirCodegenError>,
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                inspect_place(function_index, destination, errors);
                inspect_rvalue(function_index, value, errors);
            }
            StatementKind::Call { function, args, .. } => {
                if !program
                    .functions
                    .get(function.index())
                    .is_some_and(|callee| matches!(callee.kind, FunctionKind::User))
                {
                    errors.push(MirCodegenError::unsupported(format!(
                        "MIR function {function_index} calls non-user function {}",
                        function.raw()
                    )));
                }
                for argument in args {
                    match argument {
                        CallArgument::Value(_) => {}
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => {
                            inspect_place(function_index, place, errors)
                        }
                        CallArgument::SliceElement { .. }
                        | CallArgument::SliceWindow { .. }
                        | CallArgument::Buffer(_)
                        | CallArgument::BufferParam(_)
                        | CallArgument::BufferSpan(_) => {}
                    }
                }
            }
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. }
            | StatementKind::PublishDelegate { .. }
            | StatementKind::PublishLog { .. } => {}
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                inspect_block(program, function_index, then_block, errors);
                inspect_block(program, function_index, else_block, errors);
            }
            StatementKind::Loop { body } => inspect_block(program, function_index, body, errors),
            StatementKind::Break | StatementKind::Continue | StatementKind::Return { .. } => {}
        }
    }
}

fn inspect_place(function_index: usize, place: &Place, errors: &mut Vec<MirCodegenError>) {
    if place
        .projections
        .iter()
        .any(|projection| matches!(projection, Projection::Field(_)))
    {
        errors.push(MirCodegenError::unsupported(format!(
            "MIR function {function_index} contains a struct field projection"
        )));
    }
}

fn inspect_rvalue(function_index: usize, value: &Rvalue, errors: &mut Vec<MirCodegenError>) {
    match value {
        Rvalue::Load(place) => inspect_place(function_index, place, errors),
        Rvalue::InitAll
        | Rvalue::Use(_)
        | Rvalue::Unary { .. }
        | Rvalue::Binary { .. }
        | Rvalue::Compare { .. }
        | Rvalue::Cast { .. }
        | Rvalue::Intrinsic { .. }
        | Rvalue::ProcessFrame { .. }
        | Rvalue::InputLoad { .. }
        | Rvalue::OutputLoad { .. }
        | Rvalue::ConstDataLoad { .. }
        | Rvalue::BufferLoad { .. }
        | Rvalue::BufferParamLoad { .. }
        | Rvalue::BufferLen(_)
        | Rvalue::BufferChannels(_)
        | Rvalue::BufferSampleRate(_)
        | Rvalue::BufferIsBound(_)
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::BufferParamSampleRate(_)
        | Rvalue::BufferParamIsBound(_)
        | Rvalue::SliceLoad { .. }
        | Rvalue::SliceLen(_) => {}
        Rvalue::MakeSlice { source, .. } => {
            if let onda_mir::SliceSource::Place(place) = source {
                inspect_place(function_index, place, errors);
            }
        }
    }
}

/// Validates, lowers, optimizes, and JIT-compiles an owned MIR program.
pub fn lower_mir_and_jit(program: Program) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    lower_mir_and_jit_with_options(program, MirCompileOptions::default())
}

/// JIT-compiles MIR with explicit optimization and fast-math policy.
pub fn lower_mir_and_jit_with_options(
    program: Program,
    options: MirCompileOptions,
) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    lower_optimized_mir_and_jit_with_options(optimize_owned_program(program)?, options)
}

/// JIT-compiles MIR that already carries the backend-neutral optimization
/// proof, running only LLVM capability checks and native lowering.
pub fn lower_optimized_mir_and_jit(
    program: onda_mir::OptimizedProgram,
) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    lower_optimized_mir_and_jit_with_options(program, MirCompileOptions::default())
}

/// JIT-compiles optimized MIR with explicit LLVM policy.
pub fn lower_optimized_mir_and_jit_with_options(
    program: onda_mir::OptimizedProgram,
    options: MirCompileOptions,
) -> Result<MirJitProgram, Vec<MirCodegenError>> {
    validate_backend_capabilities(program.as_program())?;
    let program = program.into_program();
    let (compiled, layouts) = compile_native_jit(&program, options).map_err(|error| vec![error])?;
    Ok(MirJitProgram {
        mir: program,
        layouts,
        compiled,
    })
}

/// Emits host-target LLVM IR directly from validated MIR.
pub fn lower_mir_to_llvm_ir(program: &Program) -> Result<String, Vec<MirCodegenError>> {
    lower_mir_to_llvm_ir_with_options(program, MirCompileOptions::default())
}

/// Emits host-target LLVM IR from MIR with explicit code-generation policy.
pub fn lower_mir_to_llvm_ir_with_options(
    program: &Program,
    options: MirCompileOptions,
) -> Result<String, Vec<MirCodegenError>> {
    let program = optimize_owned_program(program.clone())?;
    lower_optimized_mir_to_llvm_ir_with_options(&program, options)
}

/// Emits host-target LLVM IR from backend-neutral optimized MIR.
pub fn lower_optimized_mir_to_llvm_ir(
    program: &onda_mir::OptimizedProgram,
) -> Result<String, Vec<MirCodegenError>> {
    lower_optimized_mir_to_llvm_ir_with_options(program, MirCompileOptions::default())
}

/// Emits host-target LLVM IR from optimized MIR with explicit LLVM policy.
pub fn lower_optimized_mir_to_llvm_ir_with_options(
    program: &onda_mir::OptimizedProgram,
    options: MirCompileOptions,
) -> Result<String, Vec<MirCodegenError>> {
    validate_backend_capabilities(program.as_program())?;
    emit_native_ir(program.as_program(), options).map_err(|error| vec![error])
}

/// Emits optimized LLVM IR for an explicit target directly from validated MIR.
pub fn lower_mir_to_target_llvm_ir(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<String, Vec<MirCodegenError>> {
    let program = optimize_owned_program(program.clone())?;
    lower_optimized_mir_to_target_llvm_ir(&program, options)
}

/// Emits target LLVM IR from backend-neutral optimized MIR.
pub fn lower_optimized_mir_to_target_llvm_ir(
    program: &onda_mir::OptimizedProgram,
    options: &MirTargetOptions,
) -> Result<String, Vec<MirCodegenError>> {
    validate_backend_capabilities(program.as_program())?;
    validate_target_options(options).map_err(|error| vec![error])?;
    emit_targeted_native_ir(program.as_program(), options).map_err(|error| vec![error])
}

/// Emits a target object file directly from validated MIR.
pub fn lower_mir_to_object(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<Vec<u8>, Vec<MirCodegenError>> {
    let program = optimize_owned_program(program.clone())?;
    lower_optimized_mir_to_object(&program, options)
}

/// Emits a target object from backend-neutral optimized MIR.
pub fn lower_optimized_mir_to_object(
    program: &onda_mir::OptimizedProgram,
    options: &MirTargetOptions,
) -> Result<Vec<u8>, Vec<MirCodegenError>> {
    validate_backend_capabilities(program.as_program())?;
    validate_target_options(options).map_err(|error| vec![error])?;
    emit_targeted_native_object(program.as_program(), options).map_err(|error| vec![error])
}

/// Emits a target object and its runtime sidecar metadata directly from MIR.
pub fn lower_mir_to_object_artifact(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<crate::AotObjectArtifact, Vec<MirCodegenError>> {
    let program = optimize_owned_program(program.clone())?;
    lower_optimized_mir_to_object_artifact(&program, options)
}

/// Emits a target object and sidecar metadata from optimized MIR.
pub fn lower_optimized_mir_to_object_artifact(
    program: &onda_mir::OptimizedProgram,
    options: &MirTargetOptions,
) -> Result<crate::AotObjectArtifact, Vec<MirCodegenError>> {
    validate_backend_capabilities(program.as_program())?;
    validate_target_options(options).map_err(|error| vec![error])?;
    emit_targeted_native_object_artifact(program.as_program(), options).map_err(|error| vec![error])
}

impl MirJitProgram {
    pub fn mir(&self) -> &Program {
        &self.mir
    }

    pub fn required_in_channels(&self) -> usize {
        self.layouts.input_count
    }

    pub fn required_out_channels(&self) -> usize {
        self.layouts.output_count
    }

    pub fn buffer_count(&self) -> usize {
        self.mir.interface.buffers.len()
    }

    pub fn state_size_bytes(&self) -> usize {
        self.layouts.state.size
    }

    pub fn state_alignment_bytes(&self) -> usize {
        self.layouts.state.alignment
    }

    pub fn state_byte_offsets(&self) -> &[usize] {
        &self.layouts.state.offsets
    }

    pub fn param_byte_size(&self) -> usize {
        self.layouts.params.size
    }

    pub fn param_byte_offsets(&self) -> &[usize] {
        &self.layouts.params.offsets
    }

    pub fn input_channel_bases(&self) -> &[usize] {
        &self.layouts.input_bases
    }

    pub fn output_channel_bases(&self) -> &[usize] {
        &self.layouts.output_bases
    }

    pub fn control_output_storage_byte_offsets(&self) -> &[usize] {
        &self.layouts.control_offsets
    }

    pub fn control_output_storage_byte_offset(&self, index: usize) -> Option<usize> {
        self.layouts.control_offsets.get(index).copied()
    }

    pub fn event_payload_byte_size(&self, index: usize) -> Option<usize> {
        self.layouts
            .event_payloads
            .get(index)
            .and_then(|layout| layout.fixed_size)
    }

    pub fn event_payload_shape(&self, index: usize) -> Option<MirEventPayloadShape> {
        self.layouts.event_payloads.get(index).map(|layout| {
            layout
                .fixed_size
                .map_or(MirEventPayloadShape::Dynamic, |byte_size| {
                    MirEventPayloadShape::Fixed { byte_size }
                })
        })
    }

    pub fn default_param_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.layouts.params.size);
        for parameter in &self.mir.interface.params {
            append_constant_bytes(&self.mir, &parameter.default, parameter.ty, &mut bytes);
        }
        debug_assert_eq!(bytes.len(), self.layouts.params.size);
        bytes
    }

    pub fn initialize_state(&self, params: &[u8]) -> Result<RuntimeState, Diagnostic> {
        self.initialize_state_with_allocator(params, None)
    }

    pub fn initialize_state_with_allocator(
        &self,
        params: &[u8],
        allocator: Option<RuntimeAllocator>,
    ) -> Result<RuntimeState, Diagnostic> {
        let mut state = self.allocate_state_with_allocator(allocator)?;
        let buffers = self.neutral_buffer_descriptors()?;
        self.initialize_allocated_state(params, &mut state, buffers.as_borrowed(), None)
    }

    fn neutral_buffer_descriptors(&self) -> Result<OwnedBufferDescriptorTables, Diagnostic> {
        let count = self.mir.interface.buffers.len();
        let channels = self
            .mir
            .interface
            .buffers
            .iter()
            .map(|buffer| match buffer.channels {
                BufferChannels::Mono | BufferChannels::Dynamic => Ok(1),
                BufferChannels::Static(channels) => i32::try_from(channels).map_err(|_| {
                    Diagnostic::runtime("buffer channel count does not fit i32", 0, 0)
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(OwnedBufferDescriptorTables {
            pointers: vec![std::ptr::null_mut(); count],
            frames: vec![1; count],
            channels,
            sample_rates: vec![self.mir.config.sample_rate; count],
        })
    }

    pub fn allocate_state_with_allocator(
        &self,
        allocator: Option<RuntimeAllocator>,
    ) -> Result<UninitializedRuntimeState, Diagnostic> {
        let words = self.layouts.state.size.saturating_add(7) / 8;
        let mut state_words = UninitRuntimeBuffer::<u64>::try_new_in(words, allocator)?;
        if !self.layouts.state.size.is_multiple_of(8) {
            // The generated initializer clears exactly the declared state bytes.
            // Initialize the final word first so its trailing allocation padding
            // is valid before the buffer is exposed as u64 storage.
            unsafe { state_words.as_mut_ptr().add(words - 1).write(0) };
        }
        Ok(UninitializedRuntimeState {
            state_words: Some(state_words),
            state_size_bytes: self.layouts.state.size,
        })
    }

    pub fn initialize_allocated_state(
        &self,
        params: &[u8],
        state: &mut UninitializedRuntimeState,
        buffers: BufferDescriptorTables<'_>,
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> Result<RuntimeState, Diagnostic> {
        if params.len() != self.layouts.params.size {
            return Err(Diagnostic::runtime(
                format!(
                    "MIR runtime parameter storage has {} bytes; expected {}",
                    params.len(),
                    self.layouts.params.size
                ),
                0,
                0,
            ));
        }
        if state.state_size_bytes != self.layouts.state.size {
            return Err(Diagnostic::runtime(
                format!(
                    "MIR runtime state storage has {} bytes; expected {}",
                    state.state_size_bytes, self.layouts.state.size
                ),
                0,
                0,
            ));
        }
        validate_buffer_abi(&self.mir, buffers)?;
        let state_words = state
            .state_words
            .as_mut()
            .ok_or_else(|| Diagnostic::internal("state storage was already initialized"))?;
        let status = unsafe {
            (self.compiled.init)(
                abi_const_ptr(params),
                state_words.as_mut_ptr().cast::<u8>(),
                1,
                abi_const_ptr(buffers.pointers),
                abi_const_ptr(buffers.frames),
                abi_const_ptr(buffers.channels),
                abi_const_ptr(buffers.sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        };
        crate::check_execution_status(status)?;
        // SAFETY: full initialization clears all declared state bytes, and the
        // only possible trailing bytes were initialized above.
        let state_words = unsafe {
            state
                .state_words
                .take()
                .expect("validated pending state storage")
                .assume_init()
        };
        Ok(RuntimeState {
            state_words,
            state_size_bytes: self.layouts.state.size,
        })
    }

    pub fn initialize_state_in_place(
        &self,
        params: &[u8],
        state: &mut RuntimeState,
        full: bool,
        buffers: BufferDescriptorTables<'_>,
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> Result<(), Diagnostic> {
        if params.len() != self.layouts.params.size {
            return Err(Diagnostic::runtime(
                format!(
                    "MIR runtime parameter storage has {} bytes; expected {}",
                    params.len(),
                    self.layouts.params.size
                ),
                0,
                0,
            ));
        }
        if state.state_size_bytes != self.layouts.state.size {
            return Err(Diagnostic::runtime(
                format!(
                    "MIR runtime state storage has {} bytes; expected {}",
                    state.state_size_bytes, self.layouts.state.size
                ),
                0,
                0,
            ));
        }
        validate_buffer_abi(&self.mir, buffers)?;
        let status = unsafe {
            (self.compiled.init)(
                abi_const_ptr(params),
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                u32::from(full),
                abi_const_ptr(buffers.pointers),
                abi_const_ptr(buffers.frames),
                abi_const_ptr(buffers.channels),
                abi_const_ptr(buffers.sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        };
        crate::check_execution_status(status)
    }

    /// Validates the process ABI shape before entering generated code.
    ///
    /// # Safety
    ///
    /// Every channel pointer in the input and output tables, and every
    /// non-null pointer in the external-buffer table, must remain valid for
    /// the complete region described by the MIR interface and buffer metadata.
    /// The four external-buffer descriptor tables must remain immutable and
    /// must not overlap state, parameter, audio, or external-buffer sample
    /// storage for the duration of the call.
    /// This method validates channel presence and alignment, but Rust slices
    /// cannot validate the extents, lifetimes, or aliasing of their pointees.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn process_checked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: usize,
        frames: usize,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> Result<(), Diagnostic> {
        let block_size = self.mir.config.block_size as usize;
        if start_frame > block_size || frames > block_size.saturating_sub(start_frame) {
            return Err(Diagnostic::runtime(
                format!(
                    "native MIR process segment [{start_frame}, {}) exceeds the configured {block_size}-frame block",
                    start_frame.saturating_add(frames)
                ),
                0,
                0,
            ));
        }
        if flags & !(onda_mir::PROCESS_FULL_BLOCK as u32) != 0 {
            return Err(Diagnostic::runtime(
                format!(
                    "native MIR process flags {flags:#x} contain bits outside BEGIN_BLOCK/END_BLOCK"
                ),
                0,
                0,
            ));
        }
        validate_audio_abi(&self.mir, in_ptrs, out_ptrs)?;
        self.validate_runtime_regions(state, params)?;
        validate_buffer_abi(
            &self.mir,
            BufferDescriptorTables::new(
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            ),
        )?;
        let start_frame = u32::try_from(start_frame)
            .map_err(|_| Diagnostic::runtime("start frame does not fit u32", 0, 0))?;
        let frames = u32::try_from(frames)
            .map_err(|_| Diagnostic::runtime("frame count does not fit u32", 0, 0))?;
        let status = unsafe {
            (self.compiled.process)(
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                abi_const_ptr(params),
                abi_const_ptr(in_ptrs),
                abi_const_ptr(out_ptrs),
                start_frame,
                frames,
                flags,
                abi_const_ptr(buffer_ptrs),
                abi_const_ptr(buffer_frames),
                abi_const_ptr(buffer_channels),
                abi_const_ptr(buffer_sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        };
        crate::check_execution_status(status)?;
        Ok(())
    }

    /// Executes the public process entry without validating any host regions.
    ///
    /// # Safety
    ///
    /// All slices, state/parameter storage, segment values, flags, and buffer
    /// metadata must satisfy the same invariants enforced by [`Self::process_checked`].
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn process_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: u32,
        frames: u32,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> u32 {
        unsafe {
            (self.compiled.process)(
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                abi_const_ptr(params),
                abi_const_ptr(in_ptrs),
                abi_const_ptr(out_ptrs),
                start_frame,
                frames,
                flags,
                abi_const_ptr(buffer_ptrs),
                abi_const_ptr(buffer_frames),
                abi_const_ptr(buffer_channels),
                abi_const_ptr(buffer_sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        }
    }

    /// Validates event and buffer metadata before entering generated code.
    ///
    /// # Safety
    ///
    /// Every non-null external-buffer pointer must remain valid for the region
    /// described by its frame/channel metadata for the duration of the call.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn trigger_event_by_index(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> Result<(), Diagnostic> {
        let status = unsafe {
            self.trigger_event_by_index_with_status(
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
                output,
            )?
        };
        crate::check_execution_status(status)
    }

    /// Validates event and buffer metadata, then returns the generated execution status.
    /// Validation errors are returned before generated event code is entered.
    ///
    /// # Safety
    ///
    /// Every non-null external-buffer pointer must remain valid for the region
    /// described by its frame/channel metadata for the duration of the call.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn trigger_event_by_index_with_status(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> Result<u32, Diagnostic> {
        let Some(event) = self.compiled.events.get(event_index).copied() else {
            return Ok(0);
        };
        self.validate_event_payload(event_index, payload)?;
        self.validate_runtime_regions(state, params)?;
        validate_buffer_abi(
            &self.mir,
            BufferDescriptorTables::new(
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            ),
        )?;
        let status = unsafe {
            event(
                abi_const_ptr(payload),
                abi_const_ptr(params),
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                abi_const_ptr(buffer_ptrs),
                abi_const_ptr(buffer_frames),
                abi_const_ptr(buffer_channels),
                abi_const_ptr(buffer_sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        };
        Ok(status)
    }

    /// Executes an event entry without validating payload or runtime regions.
    ///
    /// # Safety
    ///
    /// The payload and every host region must match the event and buffer ABI.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn trigger_event_by_index_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
        output: Option<&mut onda_processor_abi::ExecutionOutput>,
    ) -> u32 {
        let Some(event) = self.compiled.events.get(event_index).copied() else {
            return 0;
        };
        unsafe {
            event(
                abi_const_ptr(payload),
                abi_const_ptr(params),
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                abi_const_ptr(buffer_ptrs),
                abi_const_ptr(buffer_frames),
                abi_const_ptr(buffer_channels),
                abi_const_ptr(buffer_sample_rates),
                output.map_or(std::ptr::null_mut(), |output| output as *mut _),
            )
        }
    }

    fn validate_runtime_regions(
        &self,
        state: &RuntimeState,
        params: &[u8],
    ) -> Result<(), Diagnostic> {
        if state.state_size_bytes != self.layouts.state.size
            || state.state_words.len() < self.layouts.state.size.saturating_add(7) / 8
        {
            return Err(Diagnostic::runtime(
                "runtime state storage does not match native MIR layout",
                0,
                0,
            ));
        }
        if params.len() != self.layouts.params.size {
            return Err(Diagnostic::runtime(
                "runtime parameter storage does not match native MIR layout",
                0,
                0,
            ));
        }
        Ok(())
    }

    fn validate_event_payload(&self, event_index: usize, payload: &[u8]) -> Result<(), Diagnostic> {
        if let Some(expected) = self.layouts.event_payloads[event_index].fixed_size {
            if payload.len() == expected {
                return Ok(());
            }
            return Err(Diagnostic::runtime(
                format!(
                    "native MIR event {event_index} payload has {} bytes; expected {expected}",
                    payload.len()
                ),
                0,
                0,
            ));
        }

        let event = &self.mir.interface.events[event_index];
        let mut offset = 0usize;
        for parameter in &event.params {
            match self.mir.types[parameter.ty.index()] {
                Type::Slice { element, .. } => {
                    if payload.len().saturating_sub(offset) < 4 {
                        return Err(Diagnostic::runtime(
                            format!(
                                "native MIR event {event_index} payload is truncated before slice parameter '{}'",
                                parameter.name
                            ),
                            0,
                            0,
                        ));
                    }
                    let len = i32::from_ne_bytes(
                        payload[offset..offset + 4]
                            .try_into()
                            .expect("slice length prefix has four bytes"),
                    );
                    if len < 0 {
                        return Err(Diagnostic::runtime(
                            format!(
                                "native MIR event {event_index} slice parameter '{}' has negative length {len}",
                                parameter.name
                            ),
                            0,
                            0,
                        ));
                    }
                    offset = offset.saturating_add(4);
                    let data_bytes = (len as usize)
                        .checked_mul(scalar_store_size(element) as usize)
                        .filter(|bytes| *bytes <= i32::MAX as usize)
                        .ok_or_else(|| {
                            Diagnostic::runtime(
                                format!(
                                    "native MIR event {event_index} slice parameter '{}' byte extent exceeds i32",
                                    parameter.name
                                ),
                                0,
                                0,
                            )
                        })?;
                    if payload.len().saturating_sub(offset) < data_bytes {
                        return Err(Diagnostic::runtime(
                            format!(
                                "native MIR event {event_index} payload is truncated in slice parameter '{}'; expected {data_bytes} element bytes",
                                parameter.name
                            ),
                            0,
                            0,
                        ));
                    }
                    offset = offset.saturating_add(data_bytes);
                }
                _ => {
                    let bytes = fixed_payload_type_size(&self.mir, parameter.ty)
                        .map_err(|error| Diagnostic::runtime(error.message, 0, 0))?
                        .ok_or_else(|| {
                            Diagnostic::runtime(
                                "native MIR event payload has an unexpected nested dynamic type",
                                0,
                                0,
                            )
                        })?;
                    if payload.len().saturating_sub(offset) < bytes {
                        return Err(Diagnostic::runtime(
                            format!(
                                "native MIR event {event_index} payload is truncated in parameter '{}'; expected {bytes} bytes",
                                parameter.name
                            ),
                            0,
                            0,
                        ));
                    }
                    offset = offset.saturating_add(bytes);
                }
            }
        }
        if offset != payload.len() {
            return Err(Diagnostic::runtime(
                format!(
                    "native MIR event {event_index} expects {offset} payload bytes for its dynamic layout, got {}",
                    payload.len()
                ),
                0,
                0,
            ));
        }
        Ok(())
    }
}

fn append_constant_bytes(
    program: &Program,
    value: &onda_mir::ConstantValue,
    ty: onda_mir::TypeId,
    out: &mut Vec<u8>,
) {
    match (&program.types[ty.index()], value) {
        (Type::Scalar(_), onda_mir::ConstantValue::Scalar(value)) => match value {
            onda_mir::ScalarValue::F32(value) => out.extend_from_slice(&value.to_ne_bytes()),
            onda_mir::ScalarValue::F64(value) => out.extend_from_slice(&value.to_ne_bytes()),
            onda_mir::ScalarValue::I32(value) => out.extend_from_slice(&value.to_ne_bytes()),
            onda_mir::ScalarValue::I64(value) => out.extend_from_slice(&value.to_ne_bytes()),
            onda_mir::ScalarValue::Bool(value) => out.push(u8::from(*value)),
        },
        (Type::Array { element, .. }, onda_mir::ConstantValue::Aggregate(values)) => {
            for value in values {
                append_constant_bytes(program, value, *element, out);
            }
        }
        _ => unreachable!("validated MIR constant matches its declared type"),
    }
}

fn compile_native_jit(
    program: &Program,
    options: MirCompileOptions,
) -> Result<(NativeOrcProcess, NativeLayouts), MirCodegenError> {
    initialize_native_llvm()?;
    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(MirCodegenError::llvm("failed to create LLJIT builder"));
        }
        let builder = OwnedLlvm::new(builder, LLVMOrcDisposeLLJITBuilder);
        let target_builder =
            super::jit_utils::create_aggressive_jit_target_machine_builder(options.opt_level)
                .map_err(codegen_diagnostic)?;
        let target_builder = OwnedLlvm::new(target_builder, LLVMOrcDisposeJITTargetMachineBuilder);
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder.get(), target_builder.release());
        let mut lljit = null_mut();
        let error = LLVMOrcCreateLLJIT(&mut lljit, builder.release());
        if !error.is_null() {
            return Err(codegen_diagnostic(super::jit_utils::llvm_error_to_diag(
                "failed to create native MIR LLJIT",
                error,
            )));
        }
        if lljit.is_null() {
            return Err(MirCodegenError::llvm("LLJIT returned a null instance"));
        }

        let result = (|| {
            let triple = LLVMOrcLLJITGetTripleString(lljit);
            let data_layout = LLVMOrcLLJITGetDataLayoutStr(lljit);
            if triple.is_null() || data_layout.is_null() {
                return Err(MirCodegenError::llvm(
                    "failed to read LLJIT target triple or data layout",
                ));
            }
            let triple = CStr::from_ptr(triple).to_string_lossy().into_owned();
            let data_layout = CStr::from_ptr(data_layout).to_string_lossy().into_owned();
            let (module, context, layouts) =
                build_native_module(program, options.fast_math, &triple, &data_layout)?;
            let context = OwnedLlvm::new(context, LLVMContextDispose);
            let module = OwnedLlvm::new(module, LLVMDisposeModule);
            let target_machine = super::jit_utils::create_host_target_machine(options.opt_level)
                .map_err(codegen_diagnostic)?;
            let target_machine = OwnedLlvm::new(target_machine, LLVMDisposeTargetMachine);
            super::jit_utils::run_default_pass_pipeline(
                module.get(),
                target_machine.get(),
                super::jit_utils::map_opt_level(options.opt_level),
            )
            .map_err(codegen_diagnostic)?;
            verify_module(module.get())?;

            let thread_context =
                super::llvm_helpers::llvm_orc_create_new_thread_safe_context_from_llvm_context(
                    context.release(),
                );
            if thread_context.is_null() {
                return Err(MirCodegenError::llvm(
                    "failed to create ORC thread-safe context for MIR",
                ));
            }
            let thread_context = OwnedLlvm::new(thread_context, LLVMOrcDisposeThreadSafeContext);
            let thread_module =
                LLVMOrcCreateNewThreadSafeModule(module.release(), thread_context.get());
            if thread_module.is_null() {
                return Err(MirCodegenError::llvm(
                    "failed to create ORC thread-safe MIR module",
                ));
            }
            let thread_module = OwnedLlvm::new(thread_module, LLVMOrcDisposeThreadSafeModule);
            drop(thread_context);
            let add_error = LLVMOrcLLJITAddLLVMIRModule(
                lljit,
                LLVMOrcLLJITGetMainJITDylib(lljit),
                thread_module.release(),
            );
            if !add_error.is_null() {
                return Err(codegen_diagnostic(super::jit_utils::llvm_error_to_diag(
                    "failed to add native MIR module to LLJIT",
                    add_error,
                )));
            }

            let process = super::jit_utils::lookup_symbol(lljit, "onda_process", "MIR process")
                .map_err(codegen_diagnostic)?;
            let init = super::jit_utils::lookup_symbol(lljit, "onda_processor_init", "MIR init")
                .map_err(codegen_diagnostic)?;
            let mut events = Vec::with_capacity(program.interface.events.len());
            for index in 0..program.interface.events.len() {
                let address = super::jit_utils::lookup_symbol(
                    lljit,
                    &format!("onda_event_{index}"),
                    "MIR event",
                )
                .map_err(codegen_diagnostic)?;
                events.push(std::mem::transmute::<usize, NativeEventFn>(
                    address as usize,
                ));
            }
            Ok((
                NativeOrcProcess {
                    lljit,
                    process: std::mem::transmute::<usize, NativeProcessFn>(process as usize),
                    init: std::mem::transmute::<usize, NativeInitFn>(init as usize),
                    events,
                },
                layouts,
            ))
        })();
        if result.is_err() {
            super::jit_utils::dispose_lljit_quiet(lljit);
        }
        result
    }
}

fn emit_native_ir(
    program: &Program,
    options: MirCompileOptions,
) -> Result<String, MirCodegenError> {
    initialize_native_llvm()?;
    unsafe {
        let target_machine = super::jit_utils::create_host_target_machine(options.opt_level)
            .map_err(codegen_diagnostic)?;
        let target_machine = OwnedLlvm::new(target_machine, LLVMDisposeTargetMachine);
        let triple = super::jit_utils::target_machine_triple_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let (module, context, _) =
            build_native_module(program, options.fast_math, &triple, &data_layout)?;
        let _context = OwnedLlvm::new(context, LLVMContextDispose);
        let module = OwnedLlvm::new(module, LLVMDisposeModule);
        super::jit_utils::run_default_pass_pipeline(
            module.get(),
            target_machine.get(),
            super::jit_utils::map_opt_level(options.opt_level),
        )
        .map_err(codegen_diagnostic)?;
        verify_module(module.get())?;
        super::jit_utils::llvm_module_to_string(module.get()).map_err(codegen_diagnostic)
    }
}

fn emit_targeted_native_ir(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<String, MirCodegenError> {
    initialize_codegen_targets();
    unsafe {
        let resolved = super::jit_utils::resolve_target_machine_config(&options.target)
            .map_err(codegen_diagnostic)?;
        let target_machine = super::jit_utils::create_target_machine_from_config(&resolved)
            .map_err(codegen_diagnostic)?;
        let target_machine = OwnedLlvm::new(target_machine, LLVMDisposeTargetMachine);
        let triple = super::jit_utils::target_machine_triple_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let (module, context, _) =
            build_native_module(program, options.fast_math, &triple, &data_layout)?;
        let _context = OwnedLlvm::new(context, LLVMContextDispose);
        let module = OwnedLlvm::new(module, LLVMDisposeModule);
        super::jit_utils::run_default_pass_pipeline(
            module.get(),
            target_machine.get(),
            resolved.opt_level,
        )
        .map_err(codegen_diagnostic)?;
        verify_module(module.get())?;
        super::jit_utils::llvm_module_to_string(module.get()).map_err(codegen_diagnostic)
    }
}

fn emit_targeted_native_object(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<Vec<u8>, MirCodegenError> {
    emit_targeted_native_object_parts(program, options).map(|parts| parts.object_bytes)
}

struct TargetedNativeObjectParts {
    object_bytes: Vec<u8>,
    layouts: NativeLayouts,
    triple: String,
    cpu: String,
    features: String,
    data_layout: String,
    pointer_width_bits: u32,
    byte_order: &'static str,
}

fn emit_targeted_native_object_parts(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<TargetedNativeObjectParts, MirCodegenError> {
    initialize_codegen_targets();
    unsafe {
        let resolved = super::jit_utils::resolve_target_machine_config(&options.target)
            .map_err(codegen_diagnostic)?;
        let target_machine = super::jit_utils::create_target_machine_from_config(&resolved)
            .map_err(codegen_diagnostic)?;
        let target_machine = OwnedLlvm::new(target_machine, LLVMDisposeTargetMachine);
        let triple = super::jit_utils::target_machine_triple_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine.get())
            .map_err(codegen_diagnostic)?;
        let (pointer_width_bits, byte_order) = target_data_facts(&data_layout)?;
        let (module, context, layouts) =
            build_native_module(program, options.fast_math, &triple, &data_layout)?;
        let _context = OwnedLlvm::new(context, LLVMContextDispose);
        let module = OwnedLlvm::new(module, LLVMDisposeModule);
        super::jit_utils::run_default_pass_pipeline(
            module.get(),
            target_machine.get(),
            resolved.opt_level,
        )
        .map_err(codegen_diagnostic)?;
        verify_module(module.get())?;
        let object_bytes = emit_object_to_bytes(target_machine.get(), module.get())?;
        Ok(TargetedNativeObjectParts {
            object_bytes,
            layouts,
            triple,
            cpu: resolved.cpu.clone(),
            features: resolved.features.clone(),
            data_layout,
            pointer_width_bits,
            byte_order,
        })
    }
}

fn emit_targeted_native_object_artifact(
    program: &Program,
    options: &MirTargetOptions,
) -> Result<crate::AotObjectArtifact, MirCodegenError> {
    let parts = emit_targeted_native_object_parts(program, options)?;
    validate_relocatable_object(&parts.triple, &parts.object_bytes)?;
    let event_fixed_sizes = parts
        .layouts
        .event_payloads
        .iter()
        .map(|layout| layout.fixed_size)
        .collect::<Vec<_>>();
    let metadata = crate::aot_artifact::build_mir_aot_metadata(
        program,
        crate::mir_metadata::MirMetadataLayoutView {
            state_offsets: &parts.layouts.state.offsets,
            param_offsets: &parts.layouts.params.offsets,
            control_output_offsets: &parts.layouts.control_offsets,
            input_bases: &parts.layouts.input_bases,
            output_bases: &parts.layouts.output_bases,
            event_fixed_sizes: &event_fixed_sizes,
        },
        options.fast_math,
        &options.target,
        parts.triple,
        parts.cpu,
        parts.features,
        parts.data_layout,
        parts.pointer_width_bits,
        parts.byte_order,
        parts.layouts.state.size,
        parts.layouts.state.alignment,
        parts.layouts.params.size,
        parts.layouts.params.alignment,
    )
    .map_err(|error| MirCodegenError::invalid(format!("MIR AOT metadata failed: {error}")))?;
    Ok(crate::AotObjectArtifact {
        object_bytes: parts.object_bytes,
        metadata,
    })
}

unsafe fn target_data_facts(data_layout: &str) -> Result<(u32, &'static str), MirCodegenError> {
    let layout = CString::new(data_layout)
        .map_err(|_| MirCodegenError::llvm("LLVM target data layout contains a NUL byte"))?;
    let target_data = LLVMCreateTargetData(layout.as_ptr());
    if target_data.is_null() {
        return Err(MirCodegenError::llvm(
            "LLVMCreateTargetData returned null for the resolved target layout",
        ));
    }
    let pointer_width_bits = LLVMPointerSize(target_data)
        .checked_mul(8)
        .ok_or_else(|| MirCodegenError::llvm("LLVM pointer width overflowed u32"));
    let byte_order = match LLVMByteOrder(target_data) {
        llvm_sys::target::LLVMByteOrdering::LLVMLittleEndian => "little_endian",
        llvm_sys::target::LLVMByteOrdering::LLVMBigEndian => "big_endian",
    };
    LLVMDisposeTargetData(target_data);
    Ok((pointer_width_bits?, byte_order))
}

fn validate_relocatable_object(triple: &str, bytes: &[u8]) -> Result<(), MirCodegenError> {
    if bytes.is_empty() {
        return Err(MirCodegenError::llvm(
            "native MIR object emission returned an empty object",
        ));
    }
    if !triple.starts_with("wasm32-") && !triple.starts_with("wasm64-") {
        return Ok(());
    }
    const WASM_HEADER: &[u8] = b"\0asm\x01\0\0\0";
    if !bytes.starts_with(WASM_HEADER) {
        return Err(MirCodegenError::llvm(format!(
            "LLVM target '{triple}' did not emit a WebAssembly object"
        )));
    }
    if !wasm_has_custom_section(bytes, b"linking")? {
        return Err(MirCodegenError::llvm(
            "LLVM WebAssembly output is missing the relocatable linking section",
        ));
    }
    Ok(())
}

fn wasm_has_custom_section(bytes: &[u8], expected_name: &[u8]) -> Result<bool, MirCodegenError> {
    let mut cursor = 8;
    while cursor < bytes.len() {
        let section_id = bytes[cursor];
        cursor += 1;
        let section_size = wasm_u32_leb(bytes, &mut cursor)? as usize;
        let section_end = cursor.checked_add(section_size).ok_or_else(|| {
            MirCodegenError::llvm("WebAssembly object section size overflowed usize")
        })?;
        if section_end > bytes.len() {
            return Err(MirCodegenError::llvm(
                "WebAssembly object contains a truncated section",
            ));
        }
        if section_id == 0 {
            let mut payload_cursor = cursor;
            let name_size = wasm_u32_leb(bytes, &mut payload_cursor)? as usize;
            let name_end = payload_cursor.checked_add(name_size).ok_or_else(|| {
                MirCodegenError::llvm("WebAssembly custom-section name overflowed usize")
            })?;
            if name_end > section_end {
                return Err(MirCodegenError::llvm(
                    "WebAssembly object contains a truncated custom-section name",
                ));
            }
            if &bytes[payload_cursor..name_end] == expected_name {
                return Ok(true);
            }
        }
        cursor = section_end;
    }
    Ok(false)
}

fn wasm_u32_leb(bytes: &[u8], cursor: &mut usize) -> Result<u32, MirCodegenError> {
    let mut result = 0_u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*cursor).ok_or_else(|| {
            MirCodegenError::llvm("WebAssembly object contains a truncated LEB128 value")
        })?;
        *cursor += 1;
        if shift == 28 && byte & 0xf0 != 0 {
            return Err(MirCodegenError::llvm(
                "WebAssembly object contains an overflowing u32 LEB128 value",
            ));
        }
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(MirCodegenError::llvm(
        "WebAssembly object contains an invalid u32 LEB128 value",
    ))
}

fn initialize_codegen_targets() {
    super::llvm_helpers::CODEGEN_TARGETS_INIT.get_or_init(|| unsafe {
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmPrinters();
        LLVM_InitializeAllAsmParsers();
    });
}

unsafe fn emit_object_to_bytes(
    target_machine: llvm_sys::target_machine::LLVMTargetMachineRef,
    module: LLVMModuleRef,
) -> Result<Vec<u8>, MirCodegenError> {
    let mut error_message = null_mut();
    let mut memory_buffer = null_mut();
    let status = LLVMTargetMachineEmitToMemoryBuffer(
        target_machine,
        module,
        LLVMCodeGenFileType::LLVMObjectFile,
        &mut error_message,
        &mut memory_buffer,
    );
    if status != 0 {
        let detail = if error_message.is_null() {
            "unknown object emission failure".to_owned()
        } else {
            let detail = CStr::from_ptr(error_message).to_string_lossy().into_owned();
            LLVMDisposeMessage(error_message);
            detail
        };
        return Err(MirCodegenError::llvm(format!(
            "native MIR object emission failed: {detail}"
        )));
    }
    if memory_buffer.is_null() {
        if !error_message.is_null() {
            LLVMDisposeMessage(error_message);
        }
        return Err(MirCodegenError::llvm(
            "native MIR object emission returned a null memory buffer",
        ));
    }
    let start = LLVMGetBufferStart(memory_buffer).cast::<u8>();
    let size = LLVMGetBufferSize(memory_buffer);
    let bytes = if start.is_null() || size == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(start, size).to_vec()
    };
    LLVMDisposeMemoryBuffer(memory_buffer);
    if !error_message.is_null() {
        LLVMDisposeMessage(error_message);
    }
    Ok(bytes)
}

unsafe fn build_native_module(
    program: &Program,
    fast_math: bool,
    target_triple: &str,
    data_layout: &str,
) -> Result<(LLVMModuleRef, LLVMContextRef, NativeLayouts), MirCodegenError> {
    let context = LLVMContextCreate();
    if context.is_null() {
        return Err(MirCodegenError::llvm("failed to create LLVM context"));
    }
    let context = OwnedLlvm::new(context, LLVMContextDispose);
    let module_name = c_name("onda_mir_module")?;
    let module = LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context.get());
    if module.is_null() {
        return Err(MirCodegenError::llvm("failed to create LLVM MIR module"));
    }
    let module = OwnedLlvm::new(module, LLVMDisposeModule);
    let triple = c_name(target_triple)?;
    let layout = c_name(data_layout)?;
    LLVMSetTarget(module.get(), triple.as_ptr());
    LLVMSetDataLayout(module.get(), layout.as_ptr());
    let types = LoweredTypes::build(context.get(), program)?;
    let layouts = compute_native_layouts(program, &types, data_layout)?;
    let emitter = ModuleEmitter::new(
        program,
        context.get(),
        module.get(),
        &types,
        &layouts,
        fast_math,
    )?;
    emitter.emit()?;
    verify_module(module.get())?;
    Ok((module.release(), context.release(), layouts))
}

unsafe fn verify_module(module: LLVMModuleRef) -> Result<(), MirCodegenError> {
    let mut message = null_mut();
    let failed = LLVMVerifyModule(
        module,
        LLVMVerifierFailureAction::LLVMReturnStatusAction,
        &mut message,
    );
    if failed == 0 {
        if !message.is_null() {
            LLVMDisposeMessage(message);
        }
        return Ok(());
    }
    let detail = if message.is_null() {
        "unknown LLVM verifier failure".to_owned()
    } else {
        let detail = CStr::from_ptr(message).to_string_lossy().into_owned();
        LLVMDisposeMessage(message);
        detail
    };
    Err(MirCodegenError::llvm(format!(
        "native MIR LLVM verification failed: {detail}"
    )))
}

fn initialize_native_llvm() -> Result<(), MirCodegenError> {
    let error = super::llvm_helpers::NATIVE_INIT_ERR.get_or_init(|| unsafe {
        if LLVM_InitializeNativeTarget() != 0 {
            return Some("LLVM_InitializeNativeTarget failed".to_owned());
        }
        if LLVM_InitializeNativeAsmPrinter() != 0 {
            return Some("LLVM_InitializeNativeAsmPrinter failed".to_owned());
        }
        if LLVM_InitializeNativeAsmParser() != 0 {
            return Some("LLVM_InitializeNativeAsmParser failed".to_owned());
        }
        None
    });
    match error {
        Some(error) => Err(MirCodegenError::llvm(error.clone())),
        None => Ok(()),
    }
}

fn codegen_diagnostic(diagnostic: Diagnostic) -> MirCodegenError {
    MirCodegenError::llvm(diagnostic.message)
}

#[cfg(test)]
#[path = "mir_native_tests/mod.rs"]
mod tests;
