//! Native LLVM lowering for validated Onda MIR.
//!
//! This is the production native backend. It accepts only validated MIR;
//! source-language typing, rewriting, scheduling, and specialization stay
//! above this boundary.

mod host_abi;

use std::ffi::{CStr, CString};
use std::fmt;
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
    Block, CallArgument, FunctionKind, Place, Program, Projection, Rvalue, ScalarType,
    StatementKind, Type,
};

use crate::{RuntimeAllocator, RuntimeBuffer, RuntimeState, TargetOptLevel};

use self::host_abi::{abi_const_ptr, abi_mut_ptr, validate_audio_abi, validate_buffer_abi};

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
);
type NativeInitFn = unsafe extern "C" fn(*const u8, *mut u8);
type NativeEventFn = unsafe extern "C" fn(
    *const u8,
    *const u8,
    *mut u8,
    *const *mut u8,
    *const i32,
    *const i32,
    *const f32,
);

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
/// external process symbol retains the 11-argument host ABI while the MIR body
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
                LLVMInt32TypeInContext(context),
                LLVMInt32TypeInContext(context),
            ];
            LLVMStructTypeInContext(context, fields.as_mut_ptr(), fields.len() as u32, 0)
        }
        Type::Buffer { .. } => {
            let i8_ty = LLVMInt8TypeInContext(context);
            let mut fields = [
                LLVMPointerType(i8_ty, 0),
                LLVMInt32TypeInContext(context),
                LLVMInt32TypeInContext(context),
                LLVMFloatTypeInContext(context),
            ];
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

#[derive(Clone, Copy)]
struct FunctionDecl {
    value: LLVMValueRef,
    ty: LLVMTypeRef,
}

struct ModuleEmitter<'a> {
    program: &'a Program,
    effects: onda_mir::EffectAnalysis,
    context: LLVMContextRef,
    module: LLVMModuleRef,
    types: &'a LoweredTypes,
    layouts: &'a NativeLayouts,
    fast_math: bool,
    ptr_ty: LLVMTypeRef,
    runtime_context_ty: LLVMTypeRef,
    functions: Vec<FunctionDecl>,
    const_globals: Vec<LLVMValueRef>,
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
        let i32_ty = LLVMInt32TypeInContext(context);
        let mut runtime_fields = [
            ptr_ty, ptr_ty, i32_ty, i32_ty, i32_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty,
            ptr_ty,
        ];
        let runtime_context_ty = LLVMStructTypeInContext(
            context,
            runtime_fields.as_mut_ptr(),
            runtime_fields.len() as u32,
            0,
        );
        let effects = onda_mir::analyze_effects(program);
        let const_globals = build_const_globals(program, context, module)?;
        let functions =
            declare_functions(program, &effects, context, module, types, layouts, ptr_ty)?;
        Ok(Self {
            program,
            effects,
            context,
            module,
            types,
            layouts,
            fast_math,
            ptr_ty,
            runtime_context_ty,
            functions,
            const_globals,
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
    context: LLVMContextRef,
    module: LLVMModuleRef,
    types: &LoweredTypes,
    layouts: &NativeLayouts,
    ptr_ty: LLVMTypeRef,
) -> Result<Vec<FunctionDecl>, MirCodegenError> {
    let void_ty = LLVMVoidTypeInContext(context);
    let i32_ty = LLVMInt32TypeInContext(context);
    let mut declarations = Vec::with_capacity(program.functions.len());
    for (index, function) in program.functions.iter().enumerate() {
        let (name, fn_ty, internal) = match function.kind {
            FunctionKind::Init => {
                let mut args = [ptr_ty, ptr_ty];
                (
                    "onda_init".to_owned(),
                    LLVMFunctionType(void_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    false,
                )
            }
            FunctionKind::Process => {
                let mut args = [
                    ptr_ty, ptr_ty, ptr_ty, ptr_ty, i32_ty, i32_ty, i32_ty, ptr_ty, ptr_ty, ptr_ty,
                    ptr_ty,
                ];
                (
                    "onda_process".to_owned(),
                    LLVMFunctionType(void_ty, args.as_mut_ptr(), args.len() as u32, 0),
                    false,
                )
            }
            FunctionKind::Event(event) => {
                let mut args = [ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty, ptr_ty];
                (
                    format!("onda_event_{}", event.raw()),
                    LLVMFunctionType(void_ty, args.as_mut_ptr(), args.len() as u32, 0),
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
        let function_effects = effects.function(onda_mir::FunctionId::new(index as u32));
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
        if function_effects.is_memory_free() {
            add_enum_attribute_at_index(
                context,
                value,
                llvm_sys::LLVMAttributeFunctionIndex,
                "memory",
                0,
            )?;
        } else if function_effects.is_read_only() {
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
        if !function_effects.may_not_return && !function_effects.may_trap {
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
                let ranges = onda_mir::analyze_integer_ranges(
                    program,
                    onda_mir::FunctionId::new(index as u32),
                );
                for (parameter, llvm_index) in [5_u32, 6, 7].into_iter().enumerate() {
                    let Some(range) =
                        ranges.parameter(onda_mir::ParameterId::new(parameter as u32))
                    else {
                        continue;
                    };
                    if range.scalar() != onda_mir::ScalarType::I32
                        || range.min() < 0
                        || range.max() >= i64::from(i32::MAX)
                    {
                        continue;
                    }
                    add_i32_range_attribute(
                        context,
                        value,
                        llvm_index,
                        range.min() as u64,
                        (range.max() + 1) as u64,
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
                for (index, parameter) in function.params.iter().enumerate() {
                    // LLVM parameter attributes are one-based, and every MIR
                    // user function has the opaque runtime context in slot 1.
                    let llvm_index = index as u32 + 2;
                    add_enum_param_attribute(context, value, llvm_index, "noundef")?;
                    if parameter.mode == onda_mir::PassingMode::Value {
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

unsafe fn add_i32_range_attribute(
    context: LLVMContextRef,
    function: LLVMValueRef,
    index: u32,
    lower: u64,
    upper: u64,
) -> Result<(), MirCodegenError> {
    let name = "range";
    let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
    if kind == 0 {
        return Err(MirCodegenError::llvm(
            "failed to resolve LLVM 'range' attribute",
        ));
    }
    let attribute = LLVMCreateConstantRangeAttribute(context, kind, 32, &lower, &upper);
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
    let result = (|| {
        let entry = append_block(module.context, declaration.value, "entry")?;
        LLVMPositionBuilderAtEnd(builder, entry);
        let runtime_context = match function.kind {
            FunctionKind::User => LLVMGetParam(declaration.value, 0),
            _ => build_entry_runtime_context(module, declaration.value, function.kind, builder)?,
        };

        let mut emitter = FunctionEmitter {
            module,
            function,
            declaration,
            builder,
            runtime_context,
            locals: Vec::with_capacity(function.locals.len()),
            parameters: Vec::with_capacity(function.params.len()),
            event_parameters: Vec::new(),
            loop_stack: Vec::new(),
            fused_clamped_indices: fused_clamped_index_sources(module.program, function),
        };
        emitter.allocate_storage()?;
        emitter.lower_block(&function.body)?;
        if !current_block_terminated(builder) {
            if function.results.is_empty() {
                LLVMBuildRetVoid(builder);
            } else {
                return Err(MirCodegenError::invalid(format!(
                    "MIR function {function_index} falls through without returning"
                )));
            }
        }
        Ok(())
    })();
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
    ptr: LLVMValueRef,
    frames: LLVMValueRef,
    channels: LLVMValueRef,
    element: onda_mir::ScalarType,
}

#[derive(Clone, Copy)]
struct SliceParts {
    ptr: LLVMValueRef,
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
    declaration: FunctionDecl,
    builder: LLVMBuilderRef,
    runtime_context: LLVMValueRef,
    locals: Vec<PlaceRef>,
    parameters: Vec<PlaceRef>,
    event_parameters: Vec<PlaceRef>,
    loop_stack: Vec<(LLVMBasicBlockRef, LLVMBasicBlockRef)>,
    fused_clamped_indices: Vec<Option<FusedClampedIndex>>,
}

impl FunctionEmitter<'_, '_> {
    unsafe fn allocate_storage(&mut self) -> Result<(), MirCodegenError> {
        for (index, local) in self.function.locals.iter().enumerate() {
            let name = c_name(&format!("local_{index}"))?;
            let ty = self.module.types.get(local.ty);
            let ptr = LLVMBuildAlloca(self.builder, ty, name.as_ptr());
            self.locals.push(PlaceRef {
                ptr,
                ty: local.ty,
                alignment: self.module.layouts.type_alignments[local.ty.index()],
            });
        }
        match self.function.kind {
            FunctionKind::Process => {
                for (parameter, context_field) in
                    self.function.params.iter().zip([2_u32, 3_u32, 4_u32])
                {
                    self.parameters.push(PlaceRef {
                        ptr: context_field_ptr(
                            self.module,
                            self.builder,
                            self.runtime_context,
                            context_field,
                        )?,
                        ty: parameter.ty,
                        alignment: self.module.layouts.type_alignments[parameter.ty.index()],
                    });
                }
            }
            FunctionKind::User => {
                for (index, parameter) in self.function.params.iter().enumerate() {
                    let incoming = LLVMGetParam(self.declaration.value, (index + 1) as u32);
                    match parameter.mode {
                        onda_mir::PassingMode::Value => {
                            let ty = self.module.types.get(parameter.ty);
                            let name = c_name(&format!("param_{index}"))?;
                            let ptr = LLVMBuildAlloca(self.builder, ty, name.as_ptr());
                            LLVMBuildStore(self.builder, incoming, ptr);
                            self.parameters.push(PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: self.module.layouts.type_alignments
                                    [parameter.ty.index()],
                            });
                        }
                        onda_mir::PassingMode::ReadOnlyReference
                        | onda_mir::PassingMode::ReadWriteReference => {
                            self.parameters.push(PlaceRef {
                                ptr: incoming,
                                ty: parameter.ty,
                                alignment: 1,
                            });
                        }
                    }
                }
            }
            FunctionKind::Event(event) => self.allocate_event_parameters(event)?,
            FunctionKind::Init => {}
        }
        Ok(())
    }

    unsafe fn allocate_event_parameters(
        &mut self,
        event: onda_mir::EventId,
    ) -> Result<(), MirCodegenError> {
        let payload = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            11,
            "event_payload",
        )?;
        let i8_ty = LLVMInt8TypeInContext(self.module.context);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let mut offset = LLVMConstInt(i32_ty, 0, 0);
        let parameters = &self.module.program.interface.events[event.index()].params;
        self.event_parameters.reserve(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            match self.module.program.types[parameter.ty.index()] {
                Type::Slice { element, .. } => {
                    let len_ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [offset].as_mut_ptr(),
                        1,
                        c_name("event_slice_len_ptr")?.as_ptr(),
                    );
                    let len = LLVMBuildLoad2(
                        self.builder,
                        i32_ty,
                        len_ptr,
                        c_name("event_slice_len")?.as_ptr(),
                    );
                    LLVMSetAlignment(len, 1);
                    let data_offset = LLVMBuildAdd(
                        self.builder,
                        offset,
                        LLVMConstInt(i32_ty, 4, 0),
                        c_name("event_slice_data_offset")?.as_ptr(),
                    );
                    let data_ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [data_offset].as_mut_ptr(),
                        1,
                        c_name("event_slice_data")?.as_ptr(),
                    );
                    let stride = LLVMConstInt(i32_ty, scalar_store_size(element), 0);
                    let descriptor =
                        self.build_slice_descriptor(parameter.ty, data_ptr, len, stride)?;
                    let name = c_name(&format!("event_slice_{index}"))?;
                    let ptr = LLVMBuildAlloca(
                        self.builder,
                        self.module.types.get(parameter.ty),
                        name.as_ptr(),
                    );
                    LLVMBuildStore(self.builder, descriptor, ptr);
                    self.event_parameters.push(PlaceRef {
                        ptr,
                        ty: parameter.ty,
                        alignment: self.module.layouts.type_alignments[parameter.ty.index()],
                    });
                    let data_bytes = LLVMBuildMul(
                        self.builder,
                        len,
                        stride,
                        c_name("event_slice_data_bytes")?.as_ptr(),
                    );
                    offset = LLVMBuildAdd(
                        self.builder,
                        data_offset,
                        data_bytes,
                        c_name("event_payload_next")?.as_ptr(),
                    );
                }
                _ => {
                    let ptr = LLVMBuildGEP2(
                        self.builder,
                        i8_ty,
                        payload,
                        [offset].as_mut_ptr(),
                        1,
                        c_name("event_parameter")?.as_ptr(),
                    );
                    self.event_parameters.push(PlaceRef {
                        ptr,
                        ty: parameter.ty,
                        alignment: 1,
                    });
                    let size = fixed_payload_type_size(self.module.program, parameter.ty)?
                        .ok_or_else(|| {
                            MirCodegenError::invalid(
                                "event payload has an unexpected nested dynamic type",
                            )
                        })?;
                    offset = LLVMBuildAdd(
                        self.builder,
                        offset,
                        LLVMConstInt(i32_ty, size as u64, 0),
                        c_name("event_payload_next")?.as_ptr(),
                    );
                }
            }
        }
        Ok(())
    }

    unsafe fn lower_block(&mut self, block: &Block) -> Result<bool, MirCodegenError> {
        for statement in &block.statements {
            if current_block_terminated(self.builder) {
                return Ok(true);
            }
            self.lower_statement(&statement.kind)?;
        }
        Ok(current_block_terminated(self.builder))
    }

    unsafe fn lower_statement(&mut self, statement: &StatementKind) -> Result<(), MirCodegenError> {
        match statement {
            StatementKind::Assign { destination, value } => {
                let destination = self.lower_place(destination)?;
                let value = self.lower_rvalue(value, destination.ty)?;
                self.store(destination, value);
            }
            StatementKind::Call {
                results,
                function,
                args,
            } => self.lower_call(results, *function, args)?,
            StatementKind::OutputStore {
                output,
                element,
                bounds,
                frame,
                value,
            } => self.lower_output_store(*output, *element, *bounds, *frame, *value)?,
            StatementKind::ControlOutputStore {
                output,
                element,
                bounds,
                value,
            } => self.lower_control_output_store(*output, *element, *bounds, *value)?,
            StatementKind::BufferStore {
                buffer,
                channel,
                index,
                value,
                bounds,
            } => self.lower_buffer_store(*buffer, *channel, *index, *value, *bounds)?,
            StatementKind::BufferParamStore {
                parameter,
                channel,
                index,
                value,
                bounds,
            } => self.lower_buffer_param_store(*parameter, *channel, *index, *value, *bounds)?,
            StatementKind::SliceStore {
                slice,
                index,
                value,
                bounds,
            } => self.lower_slice_store(*slice, *index, *value, *bounds)?,
            StatementKind::SliceFill { destination, value } => {
                self.lower_slice_fill(*destination, *value)?;
            }
            StatementKind::SliceCopy {
                destination,
                source,
            } => self.lower_slice_copy(*destination, *source)?,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            } => self.lower_if(*condition, then_block, else_block)?,
            StatementKind::Loop { body } => self.lower_loop(body)?,
            StatementKind::Break => {
                let Some((break_target, _)) = self.loop_stack.last().copied() else {
                    return Err(MirCodegenError::invalid("break outside a MIR loop"));
                };
                LLVMBuildBr(self.builder, break_target);
            }
            StatementKind::Continue => {
                let Some((_, continue_target)) = self.loop_stack.last().copied() else {
                    return Err(MirCodegenError::invalid("continue outside a MIR loop"));
                };
                LLVMBuildBr(self.builder, continue_target);
            }
            StatementKind::Return { values } => self.lower_return(values)?,
        }
        Ok(())
    }

    unsafe fn lower_if(
        &mut self,
        condition: onda_mir::Value,
        then_block: &Block,
        else_block: &Block,
    ) -> Result<(), MirCodegenError> {
        let condition = self.lower_value(condition)?;
        let then_bb = append_block(self.module.context, self.declaration.value, "if_then")?;
        let else_bb = append_block(self.module.context, self.declaration.value, "if_else")?;
        let merge_bb = append_block(self.module.context, self.declaration.value, "if_merge")?;
        LLVMBuildCondBr(self.builder, condition, then_bb, else_bb);

        LLVMPositionBuilderAtEnd(self.builder, then_bb);
        let then_terminated = self.lower_block(then_block)?;
        if !then_terminated {
            LLVMBuildBr(self.builder, merge_bb);
        }

        LLVMPositionBuilderAtEnd(self.builder, else_bb);
        let else_terminated = self.lower_block(else_block)?;
        if !else_terminated {
            LLVMBuildBr(self.builder, merge_bb);
        }

        LLVMPositionBuilderAtEnd(self.builder, merge_bb);
        if then_terminated && else_terminated {
            LLVMBuildUnreachable(self.builder);
        }
        Ok(())
    }

    unsafe fn lower_loop(&mut self, body: &Block) -> Result<(), MirCodegenError> {
        let body_bb = append_block(self.module.context, self.declaration.value, "loop_body")?;
        let exit_bb = append_block(self.module.context, self.declaration.value, "loop_exit")?;
        LLVMBuildBr(self.builder, body_bb);
        LLVMPositionBuilderAtEnd(self.builder, body_bb);
        self.loop_stack.push((exit_bb, body_bb));
        let terminated = self.lower_block(body)?;
        self.loop_stack.pop();
        if !terminated {
            LLVMBuildBr(self.builder, body_bb);
        }
        LLVMPositionBuilderAtEnd(self.builder, exit_bb);
        Ok(())
    }

    unsafe fn lower_return(&mut self, values: &[onda_mir::Value]) -> Result<(), MirCodegenError> {
        match values {
            [] => {
                LLVMBuildRetVoid(self.builder);
            }
            [value] => {
                let value = self.lower_value(*value)?;
                LLVMBuildRet(self.builder, value);
            }
            values => {
                let result_ty = LLVMGetReturnType(self.declaration.ty);
                let mut aggregate = LLVMGetUndef(result_ty);
                for (index, value) in values.iter().enumerate() {
                    let value = self.lower_value(*value)?;
                    aggregate = LLVMBuildInsertValue(
                        self.builder,
                        aggregate,
                        value,
                        index as u32,
                        c_name("return_value")?.as_ptr(),
                    );
                }
                LLVMBuildRet(self.builder, aggregate);
            }
        }
        Ok(())
    }

    unsafe fn lower_call(
        &mut self,
        results: &[onda_mir::LocalId],
        function: onda_mir::FunctionId,
        args: &[CallArgument],
    ) -> Result<(), MirCodegenError> {
        let callee = &self.module.program.functions[function.index()];
        let declaration = self.module.functions[function.index()];
        let mut llvm_args = Vec::with_capacity(args.len() + 1);
        let mut reference_alignments = Vec::with_capacity(args.len());
        llvm_args.push(self.runtime_context);
        for (index, (argument, parameter)) in args.iter().zip(&callee.params).enumerate() {
            let (lowered, reference_alignment) = match parameter.mode {
                onda_mir::PassingMode::Value => (
                    match argument {
                        CallArgument::Value(value) => self.lower_value(*value)?,
                        CallArgument::Place(place) => {
                            let place = self.lower_place(place)?;
                            self.load(place)
                        }
                        CallArgument::Buffer(buffer) => {
                            self.build_external_buffer_descriptor(*buffer, parameter.ty)?
                        }
                        _ => {
                            return Err(MirCodegenError::unsupported(format!(
                                "MIR call argument {index} cannot be passed by value"
                            )));
                        }
                    },
                    None,
                ),
                onda_mir::PassingMode::ReadOnlyReference
                | onda_mir::PassingMode::ReadWriteReference => {
                    let place = match argument {
                        CallArgument::Place(place) => self.lower_place(place)?,
                        CallArgument::SliceElement {
                            slice,
                            index,
                            bounds,
                        } => {
                            let (ptr, _) = self.slice_element_ptr(*slice, *index, *bounds)?;
                            PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: 1,
                            }
                        }
                        CallArgument::ArrayWindow {
                            array,
                            start,
                            bounds,
                        } => self.array_window_ptr(array, *start, *bounds, parameter.ty)?,
                        CallArgument::SliceWindow {
                            slice,
                            start,
                            bounds,
                        } => self.slice_window_ptr(*slice, *start, *bounds, parameter.ty)?,
                        CallArgument::Buffer(buffer) => {
                            let descriptor =
                                self.build_external_buffer_descriptor(*buffer, parameter.ty)?;
                            let ptr = LLVMBuildAlloca(
                                self.builder,
                                self.module.types.get(parameter.ty),
                                c_name("buffer_argument")?.as_ptr(),
                            );
                            LLVMBuildStore(self.builder, descriptor, ptr);
                            PlaceRef {
                                ptr,
                                ty: parameter.ty,
                                alignment: self.module.layouts.type_alignments
                                    [parameter.ty.index()],
                            }
                        }
                        _ => {
                            return Err(MirCodegenError::unsupported(format!(
                                "MIR reference argument {index} is not a place"
                            )));
                        }
                    };
                    (place.ptr, Some(place.alignment))
                }
            };
            reference_alignments.push(reference_alignment);
            llvm_args.push(lowered);
        }
        let call = LLVMBuildCall2(
            self.builder,
            declaration.ty,
            declaration.value,
            llvm_args.as_mut_ptr(),
            llvm_args.len() as u32,
            c_name(if results.is_empty() { "" } else { "call" })?.as_ptr(),
        );
        for (index, (alignment, parameter)) in reference_alignments
            .into_iter()
            .zip(&callee.params)
            .enumerate()
        {
            let Some(alignment) = alignment else {
                continue;
            };
            let llvm_index = index as u32 + 2;
            add_enum_callsite_attribute(
                self.module.context,
                call,
                llvm_index,
                "align",
                alignment as u64,
            )?;
            add_enum_callsite_attribute(self.module.context, call, llvm_index, "nonnull", 0)?;
            add_enum_callsite_attribute(
                self.module.context,
                call,
                llvm_index,
                "dereferenceable",
                self.module.layouts.type_sizes[parameter.ty.index()] as u64,
            )?;
            let parameter_effects = self.module.effects.function(function).parameters[index];
            if !parameter_effects.writes {
                add_enum_callsite_attribute(self.module.context, call, llvm_index, "readonly", 0)?;
            } else if !parameter_effects.reads {
                add_enum_callsite_attribute(self.module.context, call, llvm_index, "writeonly", 0)?;
            }
        }
        match results {
            [] => {}
            [result] => self.store(self.locals[result.index()], call),
            results => {
                for (index, result) in results.iter().enumerate() {
                    let value = LLVMBuildExtractValue(
                        self.builder,
                        call,
                        index as u32,
                        c_name("call_result")?.as_ptr(),
                    );
                    self.store(self.locals[result.index()], value);
                }
            }
        }
        Ok(())
    }

    unsafe fn array_window_ptr(
        &mut self,
        array: &Place,
        start: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        parameter_ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array {
            element: parameter_element,
            len: required_len,
        } = self.module.program.types[parameter_ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "array-window call target is not a fixed-array reference",
            ));
        };
        let array = self.lower_place(array)?;
        let Type::Array {
            element: source_element,
            len: source_len,
        } = self.module.program.types[array.ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "array-window source is not a fixed array",
            ));
        };
        if !self
            .module
            .program
            .types_equivalent(source_element, parameter_element)
            || source_len < required_len
        {
            return Err(MirCodegenError::invalid(
                "array-window source does not contain the required parameter shape",
            ));
        }
        let start = self.lower_value(start)?;
        let max_start = source_len - required_len;
        let start = self.normalize_fixed_window_start(start, max_start, bounds)?;
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let ptr = LLVMBuildGEP2(
            self.builder,
            self.module.types.get(array.ty),
            array.ptr,
            [zero, start].as_mut_ptr(),
            2,
            c_name("array_window")?.as_ptr(),
        );
        Ok(PlaceRef {
            ptr,
            ty: parameter_ty,
            alignment: array
                .alignment
                .min(self.module.layouts.type_alignments[parameter_element.index()]),
        })
    }

    unsafe fn slice_window_ptr(
        &mut self,
        slice: onda_mir::Value,
        start: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        parameter_ty: onda_mir::TypeId,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array {
            element,
            len: required_len,
        } = self.module.program.types[parameter_ty.index()]
        else {
            return Err(MirCodegenError::invalid(
                "slice-window call target is not a fixed-array reference",
            ));
        };
        let Type::Scalar(element) = self.module.program.types[element.index()] else {
            return Err(MirCodegenError::invalid(
                "slice-window fixed-array parameter element is not scalar",
            ));
        };
        let parts = self.slice_parts(slice)?;
        if parts.element != element {
            return Err(MirCodegenError::invalid(
                "slice-window element type does not match fixed-array parameter",
            ));
        }
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let required = LLVMConstInt(i32_ty, u64::from(required_len), 0);
        if !matches!(bounds, onda_mir::BoundsMode::Unchecked) {
            let unit_stride = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntEQ,
                parts.stride_bytes,
                LLVMConstInt(i32_ty, scalar_store_size(element), 0),
                c_name("slice_window_unit_stride")?.as_ptr(),
            );
            let too_short = LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSLT,
                parts.len,
                required,
                c_name("slice_window_too_short")?.as_ptr(),
            );
            let invalid = LLVMBuildOr(
                self.builder,
                LLVMBuildNot(
                    self.builder,
                    unit_stride,
                    c_name("slice_window_noncontiguous")?.as_ptr(),
                ),
                too_short,
                c_name("slice_window_invalid_shape")?.as_ptr(),
            );
            self.emit_trap_if(invalid, "slice_window_shape_ok")?;
        }
        let max_start = LLVMBuildSub(
            self.builder,
            parts.len,
            required,
            c_name("slice_window_max_start")?.as_ptr(),
        );
        let start = self.lower_value(start)?;
        let start = self.normalize_dynamic_window_start(start, max_start, bounds)?;
        Ok(PlaceRef {
            ptr: self.slice_ptr_at_index(parts, start, "slice_window")?,
            ty: parameter_ty,
            alignment: 1,
        })
    }

    unsafe fn normalize_fixed_window_start(
        &mut self,
        start: LLVMValueRef,
        max_start: u32,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        self.normalize_dynamic_window_start(
            start,
            LLVMConstInt(
                LLVMInt32TypeInContext(self.module.context),
                u64::from(max_start),
                0,
            ),
            bounds,
        )
    }

    unsafe fn normalize_dynamic_window_start(
        &mut self,
        start: LLVMValueRef,
        max_start: LLVMValueRef,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match bounds {
            onda_mir::BoundsMode::Unchecked => Ok(start),
            onda_mir::BoundsMode::Clamp => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("window_start_below")?.as_ptr(),
                );
                let low = LLVMBuildSelect(
                    self.builder,
                    below,
                    zero,
                    start,
                    c_name("window_start_low")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    low,
                    max_start,
                    c_name("window_start_above")?.as_ptr(),
                );
                Ok(LLVMBuildSelect(
                    self.builder,
                    above,
                    max_start,
                    low,
                    c_name("window_start_clamped")?.as_ptr(),
                ))
            }
            onda_mir::BoundsMode::Trap => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("window_start_negative")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start,
                    max_start,
                    c_name("window_start_out_of_range")?.as_ptr(),
                );
                let invalid = LLVMBuildOr(
                    self.builder,
                    below,
                    above,
                    c_name("window_start_invalid")?.as_ptr(),
                );
                self.emit_trap_if(invalid, "window_start_ok")?;
                Ok(start)
            }
        }
    }

    unsafe fn build_external_buffer_descriptor(
        &mut self,
        buffer: onda_mir::BufferId,
        ty: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let parts = self.external_buffer_parts(buffer)?;
        let sample_rate = self.lower_external_buffer_metadata(buffer, 3)?;
        let mut descriptor = LLVMGetUndef(self.module.types.get(ty));
        for (index, value) in [parts.ptr, parts.frames, parts.channels, sample_rate]
            .into_iter()
            .enumerate()
        {
            descriptor = LLVMBuildInsertValue(
                self.builder,
                descriptor,
                value,
                index as u32,
                c_name("buffer_descriptor")?.as_ptr(),
            );
        }
        Ok(descriptor)
    }

    unsafe fn lower_rvalue(
        &mut self,
        rvalue: &Rvalue,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        match rvalue {
            Rvalue::Use(value) => self.lower_value(*value),
            Rvalue::Load(place) => {
                let place = self.lower_place(place)?;
                Ok(self.load(place))
            }
            Rvalue::Unary { op, operand } => self.lower_unary(*op, *operand),
            Rvalue::Binary { op, lhs, rhs } => self.lower_binary(*op, *lhs, *rhs),
            Rvalue::Compare { op, lhs, rhs } => self.lower_compare(*op, *lhs, *rhs),
            Rvalue::Cast { value, to } => self.lower_cast(*value, *to),
            Rvalue::Intrinsic { intrinsic, args } => self.lower_intrinsic(*intrinsic, args),
            Rvalue::ProcessFrame { offset } => self.lower_process_frame(*offset),
            Rvalue::InputLoad {
                input,
                element,
                bounds,
                frame,
            } => self.lower_input_load(*input, *element, *bounds, *frame),
            Rvalue::OutputLoad {
                output,
                element,
                bounds,
                frame,
            } => self.lower_output_load(*output, *element, *bounds, *frame),
            Rvalue::BufferLoad {
                buffer,
                channel,
                index,
                bounds,
            } => self.lower_buffer_load(*buffer, *channel, *index, *bounds),
            Rvalue::BufferParamLoad {
                parameter,
                channel,
                index,
                bounds,
            } => self.lower_buffer_param_load(*parameter, *channel, *index, *bounds),
            Rvalue::BufferLen(buffer) => self.lower_external_buffer_metadata(*buffer, 1),
            Rvalue::BufferChannels(buffer) => self.lower_external_buffer_metadata(*buffer, 2),
            Rvalue::BufferSampleRate(buffer) => self.lower_external_buffer_metadata(*buffer, 3),
            Rvalue::BufferParamLen(parameter) => self.lower_buffer_param_metadata(*parameter, 1),
            Rvalue::BufferParamChannels(parameter) => {
                self.lower_buffer_param_metadata(*parameter, 2)
            }
            Rvalue::BufferParamSampleRate(parameter) => {
                self.lower_buffer_param_metadata(*parameter, 3)
            }
            Rvalue::ConstDataLoad {
                data,
                index,
                bounds,
            } => self.lower_const_data_load(*data, *index, *bounds),
            Rvalue::MakeSlice {
                source,
                start,
                len,
                bounds,
                access: _,
            } => self.lower_make_slice(source, *start, *len, *bounds, expected),
            Rvalue::SliceLoad {
                slice,
                index,
                bounds,
            } => self.lower_slice_load(*slice, *index, *bounds),
            Rvalue::SliceLen(slice) => self.lower_slice_len(*slice),
        }
    }

    unsafe fn lower_process_frame(
        &mut self,
        offset: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let offset = self.lower_value(offset)?;
        let start_frame = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            2,
            "process_start_frame",
        )?;
        let frames = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            3,
            "process_frames",
        )?;
        // One unsigned comparison covers both sides of the signed range:
        // negative offsets become large unsigned values. This shape also
        // lets LLVM fold the check directly into canonical 0..frames loops.
        let valid = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULT,
            offset,
            frames,
            c_name("process_frame_valid")?.as_ptr(),
        );
        let ok = append_block(
            self.module.context,
            self.declaration.value,
            "process_frame_ok",
        )?;
        let trap = append_block(
            self.module.context,
            self.declaration.value,
            "process_frame_trap",
        )?;
        LLVMBuildCondBr(self.builder, valid, ok, trap);
        LLVMPositionBuilderAtEnd(self.builder, trap);
        let void_ty = LLVMVoidTypeInContext(self.module.context);
        let trap_ty = LLVMFunctionType(void_ty, null_mut(), 0, 0);
        let trap_fn = ensure_named_function(self.module.module, "llvm.trap", trap_ty)?;
        LLVMBuildCall2(
            self.builder,
            trap_ty,
            trap_fn,
            null_mut(),
            0,
            c_name("")?.as_ptr(),
        );
        LLVMBuildUnreachable(self.builder);
        LLVMPositionBuilderAtEnd(self.builder, ok);
        Ok(LLVMBuildAdd(
            self.builder,
            start_frame,
            offset,
            c_name("process_frame")?.as_ptr(),
        ))
    }

    unsafe fn lower_value(
        &mut self,
        value: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        Ok(match value {
            onda_mir::Value::Local(local) => self.load(self.locals[local.index()]),
            onda_mir::Value::Constant(value) => llvm_scalar_constant(self.module.context, value),
        })
    }

    unsafe fn lower_unary(
        &mut self,
        op: onda_mir::UnaryOp,
        operand: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(operand)?;
        let operand = self.lower_value(operand)?;
        let name = c_name("unary")?;
        Ok(match op {
            onda_mir::UnaryOp::Negate if is_float(scalar) => {
                let value = LLVMBuildFNeg(self.builder, operand, name.as_ptr());
                self.set_fast_math(value);
                value
            }
            onda_mir::UnaryOp::Negate => LLVMBuildNeg(self.builder, operand, name.as_ptr()),
            onda_mir::UnaryOp::LogicalNot => LLVMBuildNot(self.builder, operand, name.as_ptr()),
            onda_mir::UnaryOp::BitNot => LLVMBuildNot(self.builder, operand, name.as_ptr()),
        })
    }

    unsafe fn lower_binary(
        &mut self,
        op: onda_mir::BinaryOp,
        lhs: onda_mir::Value,
        rhs: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(lhs)?;
        let lhs = self.lower_value(lhs)?;
        let rhs = self.lower_value(rhs)?;
        let name = c_name("binary")?;
        let value = if is_float(scalar) {
            match op {
                onda_mir::BinaryOp::Add => LLVMBuildFAdd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Subtract => {
                    LLVMBuildFSub(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::Multiply => {
                    LLVMBuildFMul(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::Divide => LLVMBuildFDiv(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Remainder => {
                    LLVMBuildFRem(self.builder, lhs, rhs, name.as_ptr())
                }
                _ => {
                    return Err(MirCodegenError::invalid(
                        "bitwise MIR operation has floating-point operands",
                    ));
                }
            }
        } else {
            match op {
                onda_mir::BinaryOp::Add => LLVMBuildAdd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Subtract => LLVMBuildSub(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Multiply => LLVMBuildMul(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::Divide | onda_mir::BinaryOp::Remainder => {
                    self.lower_signed_division_or_remainder(op, scalar, lhs, rhs)?
                }
                onda_mir::BinaryOp::BitAnd => LLVMBuildAnd(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::BitOr => LLVMBuildOr(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::BitXor => LLVMBuildXor(self.builder, lhs, rhs, name.as_ptr()),
                onda_mir::BinaryOp::ShiftLeft => {
                    let rhs = self.mask_shift_count(scalar, rhs)?;
                    LLVMBuildShl(self.builder, lhs, rhs, name.as_ptr())
                }
                onda_mir::BinaryOp::ShiftRight => {
                    let rhs = self.mask_shift_count(scalar, rhs)?;
                    LLVMBuildAShr(self.builder, lhs, rhs, name.as_ptr())
                }
            }
        };
        if is_float(scalar) {
            self.set_fast_math(value);
        }
        Ok(value)
    }

    unsafe fn mask_shift_count(
        &self,
        scalar: onda_mir::ScalarType,
        rhs: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = match scalar {
            onda_mir::ScalarType::I32 => 32_u64,
            onda_mir::ScalarType::I64 => 64_u64,
            _ => {
                return Err(MirCodegenError::invalid(
                    "shift operation requires an i32 or i64 operand",
                ));
            }
        };
        let ty = llvm_scalar_type(self.module.context, scalar);
        Ok(LLVMBuildAnd(
            self.builder,
            rhs,
            LLVMConstInt(ty, bits - 1, 0),
            c_name("masked_shift_count")?.as_ptr(),
        ))
    }

    unsafe fn lower_signed_division_or_remainder(
        &mut self,
        op: onda_mir::BinaryOp,
        scalar: onda_mir::ScalarType,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = match scalar {
            onda_mir::ScalarType::I32 => 32_u32,
            onda_mir::ScalarType::I64 => 64_u32,
            _ => {
                return Err(MirCodegenError::invalid(
                    "integer division requires an i32 or i64 operand",
                ));
            }
        };
        let ty = llvm_scalar_type(self.module.context, scalar);
        let zero = LLVMConstNull(ty);
        let divisor_is_zero = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            rhs,
            zero,
            c_name("division_by_zero")?.as_ptr(),
        );
        self.emit_trap_if(divisor_is_zero, "division_nonzero")?;

        // LLVM makes signed MIN / -1 and MIN % -1 poison. MIR instead uses
        // two's-complement wrapping division semantics: quotient MIN,
        // remainder zero.
        let min = LLVMConstInt(ty, 1_u64 << (bits - 1), 0);
        let minus_one = LLVMConstInt(ty, u64::MAX, 1);
        let lhs_is_min = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            lhs,
            min,
            c_name("division_lhs_is_min")?.as_ptr(),
        );
        let rhs_is_minus_one = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            rhs,
            minus_one,
            c_name("division_rhs_is_minus_one")?.as_ptr(),
        );
        let overflow = LLVMBuildAnd(
            self.builder,
            lhs_is_min,
            rhs_is_minus_one,
            c_name("division_overflow")?.as_ptr(),
        );
        let one = LLVMConstInt(ty, 1, 0);
        let safe_rhs = LLVMBuildSelect(
            self.builder,
            overflow,
            one,
            rhs,
            c_name("division_safe_rhs")?.as_ptr(),
        );
        let raw = match op {
            onda_mir::BinaryOp::Divide => {
                LLVMBuildSDiv(self.builder, lhs, safe_rhs, c_name("division")?.as_ptr())
            }
            onda_mir::BinaryOp::Remainder => {
                LLVMBuildSRem(self.builder, lhs, safe_rhs, c_name("remainder")?.as_ptr())
            }
            _ => unreachable!("only division and remainder use this lowering"),
        };
        Ok(LLVMBuildSelect(
            self.builder,
            overflow,
            if matches!(op, onda_mir::BinaryOp::Divide) {
                min
            } else {
                zero
            },
            raw,
            c_name("division_result")?.as_ptr(),
        ))
    }

    unsafe fn emit_trap_if(
        &mut self,
        should_trap: LLVMValueRef,
        ok_name: &str,
    ) -> Result<(), MirCodegenError> {
        let ok = append_block(self.module.context, self.declaration.value, ok_name)?;
        let trap = append_block(self.module.context, self.declaration.value, "trap")?;
        LLVMBuildCondBr(self.builder, should_trap, trap, ok);
        LLVMPositionBuilderAtEnd(self.builder, trap);
        let void_ty = LLVMVoidTypeInContext(self.module.context);
        let trap_ty = LLVMFunctionType(void_ty, null_mut(), 0, 0);
        let trap_fn = ensure_named_function(self.module.module, "llvm.trap", trap_ty)?;
        LLVMBuildCall2(
            self.builder,
            trap_ty,
            trap_fn,
            null_mut(),
            0,
            c_name("")?.as_ptr(),
        );
        LLVMBuildUnreachable(self.builder);
        LLVMPositionBuilderAtEnd(self.builder, ok);
        Ok(())
    }

    unsafe fn lower_compare(
        &mut self,
        op: onda_mir::CompareOp,
        lhs: onda_mir::Value,
        rhs: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(lhs)?;
        let lhs = self.lower_value(lhs)?;
        let rhs = self.lower_value(rhs)?;
        if is_float(scalar) {
            let predicate = match op {
                onda_mir::CompareOp::Equal => LLVMRealPredicate::LLVMRealOEQ,
                onda_mir::CompareOp::NotEqual => LLVMRealPredicate::LLVMRealUNE,
                onda_mir::CompareOp::Less => LLVMRealPredicate::LLVMRealOLT,
                onda_mir::CompareOp::LessEqual => LLVMRealPredicate::LLVMRealOLE,
                onda_mir::CompareOp::Greater => LLVMRealPredicate::LLVMRealOGT,
                onda_mir::CompareOp::GreaterEqual => LLVMRealPredicate::LLVMRealOGE,
            };
            let value = LLVMBuildFCmp(self.builder, predicate, lhs, rhs, c_name("fcmp")?.as_ptr());
            self.set_fast_math(value);
            Ok(value)
        } else {
            let predicate = match op {
                onda_mir::CompareOp::Equal => LLVMIntPredicate::LLVMIntEQ,
                onda_mir::CompareOp::NotEqual => LLVMIntPredicate::LLVMIntNE,
                onda_mir::CompareOp::Less => LLVMIntPredicate::LLVMIntSLT,
                onda_mir::CompareOp::LessEqual => LLVMIntPredicate::LLVMIntSLE,
                onda_mir::CompareOp::Greater => LLVMIntPredicate::LLVMIntSGT,
                onda_mir::CompareOp::GreaterEqual => LLVMIntPredicate::LLVMIntSGE,
            };
            Ok(LLVMBuildICmp(
                self.builder,
                predicate,
                lhs,
                rhs,
                c_name("icmp")?.as_ptr(),
            ))
        }
    }

    unsafe fn lower_cast(
        &mut self,
        value: onda_mir::Value,
        to: onda_mir::ScalarType,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let from = self.scalar_type_of_value(value)?;
        let value = self.lower_value(value)?;
        if from == to {
            return Ok(value);
        }
        let to_ty = llvm_scalar_type(self.module.context, to);
        let name = c_name("cast")?;
        Ok(match (from, to) {
            (onda_mir::ScalarType::F32, onda_mir::ScalarType::F64) => {
                LLVMBuildFPExt(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::F64, onda_mir::ScalarType::F32) => {
                LLVMBuildFPTrunc(self.builder, value, to_ty, name.as_ptr())
            }
            (from, to) if is_float(from) && is_integer(to) => {
                let from_suffix = if from == onda_mir::ScalarType::F64 {
                    "f64"
                } else {
                    "f32"
                };
                let to_suffix = if to == onda_mir::ScalarType::I64 {
                    "i64"
                } else {
                    "i32"
                };
                let intrinsic_name = format!("llvm.fptosi.sat.{to_suffix}.{from_suffix}");
                let from_ty = llvm_scalar_type(self.module.context, from);
                let mut parameter_types = [from_ty];
                let fn_ty = LLVMFunctionType(to_ty, parameter_types.as_mut_ptr(), 1, 0);
                let function = ensure_named_function(self.module.module, &intrinsic_name, fn_ty)?;
                LLVMBuildCall2(
                    self.builder,
                    fn_ty,
                    function,
                    [value].as_mut_ptr(),
                    1,
                    name.as_ptr(),
                )
            }
            (from, to) if is_integer(from) && is_float(to) => {
                LLVMBuildSIToFP(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::I32, onda_mir::ScalarType::I64) => {
                LLVMBuildSExt(self.builder, value, to_ty, name.as_ptr())
            }
            (onda_mir::ScalarType::I64, onda_mir::ScalarType::I32) => {
                LLVMBuildTrunc(self.builder, value, to_ty, name.as_ptr())
            }
            _ => {
                return Err(MirCodegenError::invalid(format!(
                    "unsupported validated numeric cast {from:?} to {to:?}"
                )));
            }
        })
    }

    unsafe fn lower_intrinsic(
        &mut self,
        intrinsic: onda_mir::Intrinsic,
        args: &[onda_mir::Value],
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let scalar = self.scalar_type_of_value(args[0])?;
        let mut lowered = args
            .iter()
            .map(|value| self.lower_value(*value))
            .collect::<Result<Vec<_>, _>>()?;
        if intrinsic == onda_mir::Intrinsic::RangeClamp {
            if matches!(
                scalar,
                onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
            ) {
                let lower = self.lower_integer_intrinsic(
                    onda_mir::Intrinsic::Max,
                    scalar,
                    &mut vec![lowered[0], lowered[1]],
                )?;
                return self.lower_integer_intrinsic(
                    onda_mir::Intrinsic::Min,
                    scalar,
                    &mut vec![lower, lowered[2]],
                );
            }
            let suffix = if scalar == onda_mir::ScalarType::F64 {
                "f64"
            } else {
                "f32"
            };
            let scalar_ty = llvm_scalar_type(self.module.context, scalar);
            let lower = self.lower_binary_float_intrinsic(
                &format!("llvm.maxnum.{suffix}"),
                scalar_ty,
                lowered[0],
                lowered[1],
                "range_clamp_lower",
            )?;
            return self.lower_binary_float_intrinsic(
                &format!("llvm.minnum.{suffix}"),
                scalar_ty,
                lower,
                lowered[2],
                "range_clamp_upper",
            );
        }
        if matches!(
            scalar,
            onda_mir::ScalarType::I32 | onda_mir::ScalarType::I64
        ) {
            return self.lower_integer_intrinsic(intrinsic, scalar, &mut lowered);
        }
        let suffix = if scalar == onda_mir::ScalarType::F64 {
            "f64"
        } else {
            "f32"
        };
        let base = match intrinsic {
            onda_mir::Intrinsic::Sin => "llvm.sin",
            onda_mir::Intrinsic::Cos => "llvm.cos",
            onda_mir::Intrinsic::Tan => {
                if suffix == "f64" {
                    "tan"
                } else {
                    "tanf"
                }
            }
            onda_mir::Intrinsic::Tanh => {
                if suffix == "f64" {
                    "tanh"
                } else {
                    "tanhf"
                }
            }
            onda_mir::Intrinsic::Atan => {
                if suffix == "f64" {
                    "atan"
                } else {
                    "atanf"
                }
            }
            onda_mir::Intrinsic::Atan2 => {
                if suffix == "f64" {
                    "atan2"
                } else {
                    "atan2f"
                }
            }
            onda_mir::Intrinsic::Exp => "llvm.exp",
            onda_mir::Intrinsic::Log => "llvm.log",
            onda_mir::Intrinsic::Sqrt => "llvm.sqrt",
            onda_mir::Intrinsic::Pow => "llvm.pow",
            onda_mir::Intrinsic::Abs => "llvm.fabs",
            onda_mir::Intrinsic::Floor => "llvm.floor",
            onda_mir::Intrinsic::Ceil => "llvm.ceil",
            onda_mir::Intrinsic::Round => "llvm.round",
            onda_mir::Intrinsic::Trunc => "llvm.trunc",
            onda_mir::Intrinsic::Min => "llvm.minimum",
            onda_mir::Intrinsic::Max => "llvm.maximum",
            onda_mir::Intrinsic::Fma => "llvm.fma",
            onda_mir::Intrinsic::RangeClamp => {
                unreachable!("range clamp lowers before ordinary float intrinsics")
            }
        };
        let name = if base.starts_with("llvm.") {
            format!("{base}.{suffix}")
        } else {
            base.to_owned()
        };
        let scalar_ty = llvm_scalar_type(self.module.context, scalar);
        let mut parameter_types = vec![scalar_ty; lowered.len()];
        let fn_ty = LLVMFunctionType(
            scalar_ty,
            parameter_types.as_mut_ptr(),
            parameter_types.len() as u32,
            0,
        );
        let function = ensure_named_function(self.module.module, &name, fn_ty)?;
        let call = LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            lowered.as_mut_ptr(),
            lowered.len() as u32,
            c_name("intrinsic")?.as_ptr(),
        );
        self.set_fast_math(call);
        Ok(call)
    }

    unsafe fn lower_integer_intrinsic(
        &mut self,
        intrinsic: onda_mir::Intrinsic,
        scalar: onda_mir::ScalarType,
        lowered: &mut Vec<LLVMValueRef>,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let bits = if scalar == onda_mir::ScalarType::I64 {
            64
        } else {
            32
        };
        let scalar_ty = llvm_scalar_type(self.module.context, scalar);
        let (name, extra_bool) = match intrinsic {
            onda_mir::Intrinsic::Abs => (format!("llvm.abs.i{bits}"), true),
            onda_mir::Intrinsic::Min => (format!("llvm.smin.i{bits}"), false),
            onda_mir::Intrinsic::Max => (format!("llvm.smax.i{bits}"), false),
            _ => {
                return Err(MirCodegenError::invalid(format!(
                    "integer MIR intrinsic {intrinsic:?} is not supported by validation"
                )));
            }
        };
        let mut parameter_types = vec![scalar_ty; lowered.len()];
        if extra_bool {
            parameter_types.push(LLVMInt1TypeInContext(self.module.context));
            lowered.push(LLVMConstInt(
                LLVMInt1TypeInContext(self.module.context),
                0,
                0,
            ));
        }
        let fn_ty = LLVMFunctionType(
            scalar_ty,
            parameter_types.as_mut_ptr(),
            parameter_types.len() as u32,
            0,
        );
        let function = ensure_named_function(self.module.module, &name, fn_ty)?;
        Ok(LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            lowered.as_mut_ptr(),
            lowered.len() as u32,
            c_name("integer_intrinsic")?.as_ptr(),
        ))
    }

    unsafe fn lower_input_load(
        &mut self,
        input: onda_mir::InputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.inputs[input.index()].ty,
            self.module.layouts.input_bases[input.index()],
            element,
            bounds,
        )?;
        self.load_audio_sample(0, scalar, port, frame)
    }

    unsafe fn lower_output_load(
        &mut self,
        output: onda_mir::OutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.outputs[output.index()].ty,
            self.module.layouts.output_bases[output.index()],
            element,
            bounds,
        )?;
        self.load_audio_sample(1, scalar, port, frame)
    }

    unsafe fn lower_output_store(
        &mut self,
        output: onda_mir::OutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        frame: onda_mir::Value,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let (scalar, port) = self.interface_port(
            self.module.program.interface.outputs[output.index()].ty,
            self.module.layouts.output_bases[output.index()],
            element,
            bounds,
        )?;
        let value = self.lower_value(value)?;
        let ptr = self.audio_sample_ptr(1, scalar, port, frame)?;
        LLVMBuildStore(self.builder, value, ptr);
        Ok(())
    }

    unsafe fn lower_control_output_store(
        &mut self,
        output: onda_mir::ControlOutputId,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let descriptor = &self.module.program.interface.control_outputs[output.index()];
        let state_ptr = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            6,
            "state_ptr",
        )?;
        let ptr = byte_offset_ptr(
            self.module.context,
            self.builder,
            state_ptr,
            self.module.layouts.control_offsets[output.index()],
            "control_output",
        )?;
        let mut place = PlaceRef {
            ptr,
            ty: descriptor.ty,
            alignment: self.module.layouts.type_alignments[descriptor.ty.index()],
        };
        if let Some(index) = element {
            place = self.project_array(place, index, bounds)?;
        }
        let value = self.lower_value(value)?;
        self.store(place, value);
        Ok(())
    }

    unsafe fn load_audio_sample(
        &mut self,
        context_field: u32,
        scalar: onda_mir::ScalarType,
        port: LLVMValueRef,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let ptr = self.audio_sample_ptr(context_field, scalar, port, frame)?;
        Ok(LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, scalar),
            ptr,
            c_name("audio_load")?.as_ptr(),
        ))
    }

    unsafe fn audio_sample_ptr(
        &mut self,
        context_field: u32,
        scalar: onda_mir::ScalarType,
        port: LLVMValueRef,
        frame: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let ports = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            context_field,
            "audio_ports",
        )?;
        let port_ptr = LLVMBuildGEP2(
            self.builder,
            self.module.ptr_ty,
            ports,
            [port].as_mut_ptr(),
            1,
            c_name("audio_port_ptr")?.as_ptr(),
        );
        let channel = LLVMBuildLoad2(
            self.builder,
            self.module.ptr_ty,
            port_ptr,
            c_name("audio_channel")?.as_ptr(),
        );
        // Segmented process MIR already computes the logical I/O frame as
        // `start_frame + local_frame`. Host pointers address the full block,
        // so the ABI start must not be added again here.
        let logical_frame = self.lower_value(frame)?;
        Ok(LLVMBuildGEP2(
            self.builder,
            llvm_scalar_type(self.module.context, scalar),
            channel,
            [logical_frame].as_mut_ptr(),
            1,
            c_name("audio_sample_ptr")?.as_ptr(),
        ))
    }

    unsafe fn interface_port(
        &mut self,
        ty: onda_mir::TypeId,
        base: usize,
        element: Option<onda_mir::Value>,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(onda_mir::ScalarType, LLVMValueRef), MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        match &self.module.program.types[ty.index()] {
            Type::Scalar(scalar) => Ok((*scalar, LLVMConstInt(i32_ty, base as u64, 0))),
            Type::Array { element: item, len } => {
                let Type::Scalar(scalar) = self.module.program.types[item.index()] else {
                    return Err(MirCodegenError::unsupported(
                        "nested arrays are not audio interface values",
                    ));
                };
                let element = element.ok_or_else(|| {
                    MirCodegenError::invalid("array audio interface access has no element index")
                })?;
                let index =
                    self.lower_fixed_index(element, usize::try_from(*len).unwrap(), bounds)?;
                let port = LLVMBuildAdd(
                    self.builder,
                    LLVMConstInt(i32_ty, base as u64, 0),
                    index,
                    c_name("audio_port")?.as_ptr(),
                );
                Ok((scalar, port))
            }
            _ => Err(MirCodegenError::unsupported(
                "unsupported audio interface aggregate",
            )),
        }
    }

    unsafe fn lower_external_buffer_metadata(
        &mut self,
        buffer: onda_mir::BufferId,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (context_field, element_ty, name) = match descriptor_field {
            0 => (7, self.module.ptr_ty, "buffer_ptr"),
            1 => (
                8,
                LLVMInt32TypeInContext(self.module.context),
                "buffer_frames",
            ),
            2 => (
                9,
                LLVMInt32TypeInContext(self.module.context),
                "buffer_channels",
            ),
            3 => (
                10,
                LLVMFloatTypeInContext(self.module.context),
                "buffer_sample_rate",
            ),
            _ => return Err(MirCodegenError::invalid("invalid buffer descriptor field")),
        };
        let values = load_context_field(
            self.module,
            self.builder,
            self.runtime_context,
            context_field,
            name,
        )?;
        let index = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            buffer.raw() as u64,
            0,
        );
        let ptr = LLVMBuildGEP2(
            self.builder,
            element_ty,
            values,
            [index].as_mut_ptr(),
            1,
            c_name(name)?.as_ptr(),
        );
        Ok(LLVMBuildLoad2(
            self.builder,
            element_ty,
            ptr,
            c_name(name)?.as_ptr(),
        ))
    }

    unsafe fn lower_buffer_param_metadata(
        &mut self,
        parameter: onda_mir::ParameterId,
        descriptor_field: u32,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let descriptor = self.load(self.parameters[parameter.index()]);
        if descriptor_field == 2 {
            let ty = self.function.params[parameter.index()].ty;
            let Type::Buffer { channels, .. } = self.module.program.types[ty.index()] else {
                return Err(MirCodegenError::invalid(
                    "buffer metadata operation uses a non-buffer parameter",
                ));
            };
            match channels {
                onda_mir::BufferChannels::Mono => {
                    return Ok(LLVMConstInt(
                        LLVMInt32TypeInContext(self.module.context),
                        1,
                        0,
                    ));
                }
                onda_mir::BufferChannels::Static(channels) => {
                    return Ok(LLVMConstInt(
                        LLVMInt32TypeInContext(self.module.context),
                        channels as u64,
                        0,
                    ));
                }
                onda_mir::BufferChannels::Dynamic => {}
            }
        }
        Ok(LLVMBuildExtractValue(
            self.builder,
            descriptor,
            descriptor_field,
            c_name("buffer_metadata")?.as_ptr(),
        ))
    }

    unsafe fn external_buffer_parts(
        &mut self,
        buffer: onda_mir::BufferId,
    ) -> Result<BufferParts, MirCodegenError> {
        let descriptor = &self.module.program.interface.buffers[buffer.index()];
        let ptr = self.lower_external_buffer_metadata(buffer, 0)?;
        let frames = self.lower_external_buffer_metadata(buffer, 1)?;
        let runtime_channels = self.lower_external_buffer_metadata(buffer, 2)?;
        let channels = match descriptor.channels {
            onda_mir::BufferChannels::Mono => {
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 1, 0)
            }
            onda_mir::BufferChannels::Static(_) | onda_mir::BufferChannels::Dynamic => {
                runtime_channels
            }
        };
        Ok(BufferParts {
            ptr,
            frames,
            channels,
            element: descriptor.element,
        })
    }

    unsafe fn buffer_param_parts(
        &mut self,
        parameter: onda_mir::ParameterId,
    ) -> Result<BufferParts, MirCodegenError> {
        let ty = self.function.params[parameter.index()].ty;
        let Type::Buffer { element, .. } = self.module.program.types[ty.index()] else {
            return Err(MirCodegenError::invalid(
                "buffer operation uses a non-buffer parameter",
            ));
        };
        let descriptor = self.load(self.parameters[parameter.index()]);
        let ptr =
            LLVMBuildExtractValue(self.builder, descriptor, 0, c_name("buffer_ptr")?.as_ptr());
        let frames = LLVMBuildExtractValue(
            self.builder,
            descriptor,
            1,
            c_name("buffer_frames")?.as_ptr(),
        );
        let channels = self.lower_buffer_param_metadata(parameter, 2)?;
        Ok(BufferParts {
            ptr,
            frames,
            channels,
            element,
        })
    }

    unsafe fn buffer_element_ptr(
        &mut self,
        parts: BufferParts,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let index = self.lower_value(index)?;
        let total_len = LLVMBuildMul(
            self.builder,
            parts.frames,
            parts.channels,
            c_name("buffer_total_len")?.as_ptr(),
        );
        let flat = if let Some(channel) = channel {
            let channel = self.lower_value(channel)?;
            let frame_offset = LLVMBuildMul(
                self.builder,
                index,
                parts.channels,
                c_name("buffer_frame_offset")?.as_ptr(),
            );
            LLVMBuildAdd(
                self.builder,
                frame_offset,
                channel,
                c_name("buffer_flat_index")?.as_ptr(),
            )
        } else {
            index
        };
        let flat = if bounds == onda_mir::BoundsMode::Clamp {
            self.clamp_dynamic_index(flat, total_len)?
        } else {
            self.apply_dynamic_bounds(flat, total_len, bounds)?
        };
        Ok(LLVMBuildGEP2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            parts.ptr,
            [flat].as_mut_ptr(),
            1,
            c_name("buffer_element")?.as_ptr(),
        ))
    }

    unsafe fn lower_buffer_load(
        &mut self,
        buffer: onda_mir::BufferId,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let parts = self.external_buffer_parts(buffer)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            ptr,
            c_name("buffer_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, 1);
        Ok(load)
    }

    unsafe fn lower_buffer_param_load(
        &mut self,
        parameter: onda_mir::ParameterId,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let parts = self.buffer_param_parts(parameter)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, parts.element),
            ptr,
            c_name("buffer_param_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, 1);
        Ok(load)
    }

    unsafe fn lower_buffer_store(
        &mut self,
        buffer: onda_mir::BufferId,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let parts = self.external_buffer_parts(buffer)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn lower_buffer_param_store(
        &mut self,
        parameter: onda_mir::ParameterId,
        channel: Option<onda_mir::Value>,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let parts = self.buffer_param_parts(parameter)?;
        let ptr = self.buffer_element_ptr(parts, channel, index, bounds)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn slice_parts(
        &mut self,
        slice: onda_mir::Value,
    ) -> Result<SliceParts, MirCodegenError> {
        let onda_mir::Value::Local(local) = slice else {
            return Err(MirCodegenError::invalid("slice value is not a local"));
        };
        let ty = self.function.locals[local.index()].ty;
        let Type::Slice { element, .. } = self.module.program.types[ty.index()] else {
            return Err(MirCodegenError::invalid(
                "slice operation uses a non-slice value",
            ));
        };
        let descriptor = self.lower_value(slice)?;
        Ok(SliceParts {
            ptr: LLVMBuildExtractValue(self.builder, descriptor, 0, c_name("slice_ptr")?.as_ptr()),
            len: LLVMBuildExtractValue(self.builder, descriptor, 1, c_name("slice_len")?.as_ptr()),
            stride_bytes: LLVMBuildExtractValue(
                self.builder,
                descriptor,
                2,
                c_name("slice_stride_bytes")?.as_ptr(),
            ),
            element,
        })
    }

    unsafe fn slice_ptr_at_index(
        &self,
        parts: SliceParts,
        index: LLVMValueRef,
        name: &str,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let byte_offset = LLVMBuildMul(
            self.builder,
            index,
            parts.stride_bytes,
            c_name(&format!("{name}_byte_offset"))?.as_ptr(),
        );
        Ok(LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            parts.ptr,
            [byte_offset].as_mut_ptr(),
            1,
            c_name(name)?.as_ptr(),
        ))
    }

    unsafe fn slice_element_ptr(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(LLVMValueRef, onda_mir::ScalarType), MirCodegenError> {
        let parts = self.slice_parts(slice)?;
        let index = self.lower_value(index)?;
        let index = self.apply_dynamic_bounds(index, parts.len, bounds)?;
        let ptr = self.slice_ptr_at_index(parts, index, "slice_element")?;
        Ok((ptr, parts.element))
    }

    unsafe fn lower_make_slice(
        &mut self,
        source: &onda_mir::SliceSource,
        start: onda_mir::Value,
        len: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let Type::Slice { element, .. } = self.module.program.types[expected.index()] else {
            return Err(MirCodegenError::invalid(
                "make-slice destination is not slice-typed",
            ));
        };
        let start = self.lower_value(start)?;
        let element_size = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            scalar_store_size(element),
            0,
        );
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let (base_ptr, stride_bytes, source_len) = match source {
            onda_mir::SliceSource::Place(place) => {
                let place = self.lower_place(place)?;
                match self.module.program.types[place.ty.index()] {
                    Type::Array { len, .. } => {
                        let zero = LLVMConstInt(i32_ty, 0, 0);
                        (
                            LLVMBuildGEP2(
                                self.builder,
                                self.module.types.get(place.ty),
                                place.ptr,
                                [zero, zero].as_mut_ptr(),
                                2,
                                c_name("slice_array_base")?.as_ptr(),
                            ),
                            element_size,
                            LLVMConstInt(i32_ty, u64::from(len), 0),
                        )
                    }
                    Type::Slice { .. } => {
                        let descriptor = self.load(place);
                        (
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                0,
                                c_name("slice_base")?.as_ptr(),
                            ),
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                2,
                                c_name("slice_source_stride")?.as_ptr(),
                            ),
                            LLVMBuildExtractValue(
                                self.builder,
                                descriptor,
                                1,
                                c_name("slice_source_len")?.as_ptr(),
                            ),
                        )
                    }
                    _ => {
                        return Err(MirCodegenError::invalid(
                            "make-slice place source is neither array nor slice",
                        ));
                    }
                }
            }
            onda_mir::SliceSource::Buffer { buffer, channel } => {
                let parts = self.external_buffer_parts(*buffer)?;
                return self.make_buffer_slice(parts, *channel, start, len, bounds, expected);
            }
            onda_mir::SliceSource::BufferParam { parameter, channel } => {
                let parts = self.buffer_param_parts(*parameter)?;
                return self.make_buffer_slice(parts, *channel, start, len, bounds, expected);
            }
            onda_mir::SliceSource::ConstData(data) => {
                let descriptor = &self.module.program.const_data[data.index()];
                let array_ty = LLVMArrayType2(
                    llvm_scalar_type(self.module.context, descriptor.element),
                    descriptor.values.len() as u64,
                );
                let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
                (
                    LLVMBuildGEP2(
                        self.builder,
                        array_ty,
                        self.module.const_globals[data.index()],
                        [zero, zero].as_mut_ptr(),
                        2,
                        c_name("slice_const_base")?.as_ptr(),
                    ),
                    element_size,
                    LLVMConstInt(i32_ty, descriptor.values.len() as u64, 0),
                )
            }
        };
        let len = self.lower_value(len)?;
        let (start, len) = self.normalize_slice_range(start, len, source_len, bounds)?;
        let start_byte_offset = LLVMBuildMul(
            self.builder,
            start,
            stride_bytes,
            c_name("slice_start_byte_offset")?.as_ptr(),
        );
        let ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            base_ptr,
            [start_byte_offset].as_mut_ptr(),
            1,
            c_name("slice_start")?.as_ptr(),
        );
        self.build_slice_descriptor(expected, ptr, len, stride_bytes)
    }

    unsafe fn make_buffer_slice(
        &mut self,
        parts: BufferParts,
        channel: Option<onda_mir::Value>,
        start: LLVMValueRef,
        len: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
        expected: onda_mir::TypeId,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let element_size = LLVMConstInt(
            LLVMInt32TypeInContext(self.module.context),
            scalar_store_size(parts.element),
            0,
        );
        let (base_offset, stride_bytes) = if let Some(channel) = channel {
            let channel = self.lower_value(channel)?;
            let channel = self.clamp_dynamic_index(channel, parts.channels)?;
            let channel_offset = LLVMBuildMul(
                self.builder,
                channel,
                element_size,
                c_name("buffer_slice_channel_offset")?.as_ptr(),
            );
            let stride = LLVMBuildMul(
                self.builder,
                parts.channels,
                element_size,
                c_name("buffer_slice_stride")?.as_ptr(),
            );
            (channel_offset, stride)
        } else {
            (
                LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0),
                element_size,
            )
        };
        let len = self.lower_value(len)?;
        let (start, len) = self.normalize_slice_range(start, len, parts.frames, bounds)?;
        let start_offset = LLVMBuildMul(
            self.builder,
            start,
            stride_bytes,
            c_name("buffer_slice_checked_start_offset")?.as_ptr(),
        );
        let offset = LLVMBuildAdd(
            self.builder,
            base_offset,
            start_offset,
            c_name("buffer_slice_checked_offset")?.as_ptr(),
        );
        let ptr = LLVMBuildGEP2(
            self.builder,
            LLVMInt8TypeInContext(self.module.context),
            parts.ptr,
            [offset].as_mut_ptr(),
            1,
            c_name("buffer_slice_checked_start")?.as_ptr(),
        );
        self.build_slice_descriptor(expected, ptr, len, stride_bytes)
    }

    unsafe fn normalize_slice_range(
        &mut self,
        start: LLVMValueRef,
        len: LLVMValueRef,
        source_len: LLVMValueRef,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(LLVMValueRef, LLVMValueRef), MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match bounds {
            onda_mir::BoundsMode::Unchecked => Ok((start, len)),
            onda_mir::BoundsMode::Clamp => {
                let start_below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("slice_start_below")?.as_ptr(),
                );
                let start_low = LLVMBuildSelect(
                    self.builder,
                    start_below,
                    zero,
                    start,
                    c_name("slice_start_low")?.as_ptr(),
                );
                let start_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start_low,
                    source_len,
                    c_name("slice_start_above")?.as_ptr(),
                );
                let start = LLVMBuildSelect(
                    self.builder,
                    start_above,
                    source_len,
                    start_low,
                    c_name("slice_start_clamped")?.as_ptr(),
                );
                let len_below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    len,
                    zero,
                    c_name("slice_len_below")?.as_ptr(),
                );
                let len_low = LLVMBuildSelect(
                    self.builder,
                    len_below,
                    zero,
                    len,
                    c_name("slice_len_low")?.as_ptr(),
                );
                let remaining = LLVMBuildSub(
                    self.builder,
                    source_len,
                    start,
                    c_name("slice_remaining")?.as_ptr(),
                );
                let len_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len_low,
                    remaining,
                    c_name("slice_len_above")?.as_ptr(),
                );
                let len = LLVMBuildSelect(
                    self.builder,
                    len_above,
                    remaining,
                    len_low,
                    c_name("slice_len_clamped")?.as_ptr(),
                );
                Ok((start, len))
            }
            onda_mir::BoundsMode::Trap => {
                let start_negative = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    start,
                    zero,
                    c_name("slice_start_negative")?.as_ptr(),
                );
                let len_negative = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    len,
                    zero,
                    c_name("slice_len_negative")?.as_ptr(),
                );
                let start_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    start,
                    source_len,
                    c_name("slice_start_out_of_range")?.as_ptr(),
                );
                let remaining = LLVMBuildSub(
                    self.builder,
                    source_len,
                    start,
                    c_name("slice_trap_remaining")?.as_ptr(),
                );
                let len_above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len,
                    remaining,
                    c_name("slice_len_out_of_range")?.as_ptr(),
                );
                let invalid_start = LLVMBuildOr(
                    self.builder,
                    start_negative,
                    start_above,
                    c_name("slice_invalid_start")?.as_ptr(),
                );
                let invalid_len = LLVMBuildOr(
                    self.builder,
                    len_negative,
                    len_above,
                    c_name("slice_invalid_len")?.as_ptr(),
                );
                let invalid = LLVMBuildOr(
                    self.builder,
                    invalid_start,
                    invalid_len,
                    c_name("slice_invalid_range")?.as_ptr(),
                );
                self.emit_trap_if(invalid, "slice_range_ok")?;
                Ok((start, len))
            }
        }
    }

    unsafe fn build_slice_descriptor(
        &self,
        ty: onda_mir::TypeId,
        ptr: LLVMValueRef,
        len: LLVMValueRef,
        stride_bytes: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let mut descriptor = LLVMGetUndef(self.module.types.get(ty));
        descriptor = LLVMBuildInsertValue(
            self.builder,
            descriptor,
            ptr,
            0,
            c_name("slice_with_ptr")?.as_ptr(),
        );
        descriptor = LLVMBuildInsertValue(
            self.builder,
            descriptor,
            len,
            1,
            c_name("slice_with_len")?.as_ptr(),
        );
        Ok(LLVMBuildInsertValue(
            self.builder,
            descriptor,
            stride_bytes,
            2,
            c_name("slice_with_stride")?.as_ptr(),
        ))
    }

    unsafe fn lower_slice_load(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let (ptr, element) = self.slice_element_ptr(slice, index, bounds)?;
        let load = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, element),
            ptr,
            c_name("slice_load")?.as_ptr(),
        );
        LLVMSetAlignment(load, 1);
        Ok(load)
    }

    unsafe fn lower_slice_len(
        &mut self,
        slice: onda_mir::Value,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        Ok(self.slice_parts(slice)?.len)
    }

    unsafe fn lower_slice_store(
        &mut self,
        slice: onda_mir::Value,
        index: onda_mir::Value,
        value: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<(), MirCodegenError> {
        let (ptr, _) = self.slice_element_ptr(slice, index, bounds)?;
        let value = self.lower_value(value)?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn lower_slice_fill(
        &mut self,
        destination: onda_mir::Value,
        value: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let destination = self.slice_parts(destination)?;
        let value = self.lower_value(value)?;
        let preheader = LLVMGetInsertBlock(self.builder);
        let condition = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_condition",
        )?;
        let body = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_body",
        )?;
        let exit = append_block(
            self.module.context,
            self.declaration.value,
            "slice_fill_exit",
        )?;
        LLVMBuildBr(self.builder, condition);

        LLVMPositionBuilderAtEnd(self.builder, condition);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let index = LLVMBuildPhi(self.builder, i32_ty, c_name("slice_fill_index")?.as_ptr());
        let zero = LLVMConstInt(i32_ty, 0, 0);
        LLVMAddIncoming(index, [zero].as_mut_ptr(), [preheader].as_mut_ptr(), 1);
        let in_bounds = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            index,
            destination.len,
            c_name("slice_fill_in_bounds")?.as_ptr(),
        );
        LLVMBuildCondBr(self.builder, in_bounds, body, exit);

        LLVMPositionBuilderAtEnd(self.builder, body);
        let ptr = self.slice_ptr_at_index(destination, index, "slice_fill_element")?;
        let store = LLVMBuildStore(self.builder, value, ptr);
        LLVMSetAlignment(store, 1);
        let next = LLVMBuildAdd(
            self.builder,
            index,
            LLVMConstInt(i32_ty, 1, 0),
            c_name("slice_fill_next")?.as_ptr(),
        );
        LLVMBuildBr(self.builder, condition);
        LLVMAddIncoming(index, [next].as_mut_ptr(), [body].as_mut_ptr(), 1);
        LLVMPositionBuilderAtEnd(self.builder, exit);
        Ok(())
    }

    unsafe fn lower_slice_copy(
        &mut self,
        destination: onda_mir::Value,
        source: onda_mir::Value,
    ) -> Result<(), MirCodegenError> {
        let destination = self.slice_parts(destination)?;
        let source = self.slice_parts(source)?;
        if destination.element != source.element {
            return Err(MirCodegenError::invalid(
                "slice copy source and destination element types differ",
            ));
        }
        let destination_shorter = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            destination.len,
            source.len,
            c_name("slice_copy_destination_shorter")?.as_ptr(),
        );
        let len = LLVMBuildSelect(
            self.builder,
            destination_shorter,
            destination.len,
            source.len,
            c_name("slice_copy_len")?.as_ptr(),
        );
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let empty = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            len,
            zero,
            c_name("slice_copy_empty")?.as_ptr(),
        );
        let nonempty = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_nonempty",
        )?;
        let merge = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_merge",
        )?;
        LLVMBuildCondBr(self.builder, empty, merge, nonempty);
        LLVMPositionBuilderAtEnd(self.builder, nonempty);

        let element_size = scalar_store_size(destination.element);
        let element_size_i32 = LLVMConstInt(i32_ty, element_size, 0);
        let destination_contiguous = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            destination.stride_bytes,
            element_size_i32,
            c_name("slice_copy_destination_contiguous")?.as_ptr(),
        );
        let source_contiguous = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            source.stride_bytes,
            element_size_i32,
            c_name("slice_copy_source_contiguous")?.as_ptr(),
        );
        let contiguous = LLVMBuildAnd(
            self.builder,
            destination_contiguous,
            source_contiguous,
            c_name("slice_copy_contiguous")?.as_ptr(),
        );
        let contiguous_block = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_memmove",
        )?;
        let strided_block = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_strided",
        )?;
        LLVMBuildCondBr(self.builder, contiguous, contiguous_block, strided_block);

        LLVMPositionBuilderAtEnd(self.builder, contiguous_block);
        let i64_ty = LLVMInt64TypeInContext(self.module.context);
        let len_i64 = LLVMBuildZExt(
            self.builder,
            len,
            i64_ty,
            c_name("slice_copy_len_i64")?.as_ptr(),
        );
        let byte_count = LLVMBuildMul(
            self.builder,
            len_i64,
            LLVMConstInt(i64_ty, element_size, 0),
            c_name("slice_copy_bytes")?.as_ptr(),
        );
        LLVMBuildMemMove(self.builder, destination.ptr, 1, source.ptr, 1, byte_count);
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, strided_block);
        let intptr_ty = LLVMInt64TypeInContext(self.module.context);
        let destination_address = LLVMBuildPtrToInt(
            self.builder,
            destination.ptr,
            intptr_ty,
            c_name("slice_copy_destination_address")?.as_ptr(),
        );
        let source_address = LLVMBuildPtrToInt(
            self.builder,
            source.ptr,
            intptr_ty,
            c_name("slice_copy_source_address")?.as_ptr(),
        );
        let same_stride = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntEQ,
            destination.stride_bytes,
            source.stride_bytes,
            c_name("slice_copy_same_stride")?.as_ptr(),
        );
        let one = LLVMConstInt(i32_ty, 1, 0);
        let last_index = LLVMBuildSub(
            self.builder,
            len,
            one,
            c_name("slice_copy_last_index")?.as_ptr(),
        );
        let last_index_i64 = LLVMBuildZExt(
            self.builder,
            last_index,
            intptr_ty,
            c_name("slice_copy_last_index_i64")?.as_ptr(),
        );
        let destination_stride_i64 = LLVMBuildZExt(
            self.builder,
            destination.stride_bytes,
            intptr_ty,
            c_name("slice_copy_destination_stride_i64")?.as_ptr(),
        );
        let source_stride_i64 = LLVMBuildZExt(
            self.builder,
            source.stride_bytes,
            intptr_ty,
            c_name("slice_copy_source_stride_i64")?.as_ptr(),
        );
        let destination_last_offset = LLVMBuildMul(
            self.builder,
            last_index_i64,
            destination_stride_i64,
            c_name("slice_copy_destination_last_offset")?.as_ptr(),
        );
        let source_last_offset = LLVMBuildMul(
            self.builder,
            last_index_i64,
            source_stride_i64,
            c_name("slice_copy_source_last_offset")?.as_ptr(),
        );
        let element_size_i64 = LLVMConstInt(intptr_ty, element_size, 0);
        let destination_end = LLVMBuildAdd(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                destination_address,
                destination_last_offset,
                c_name("slice_copy_destination_last")?.as_ptr(),
            ),
            element_size_i64,
            c_name("slice_copy_destination_end")?.as_ptr(),
        );
        let source_end = LLVMBuildAdd(
            self.builder,
            LLVMBuildAdd(
                self.builder,
                source_address,
                source_last_offset,
                c_name("slice_copy_source_last")?.as_ptr(),
            ),
            element_size_i64,
            c_name("slice_copy_source_end")?.as_ptr(),
        );
        let destination_before_source = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            destination_end,
            source_address,
            c_name("slice_copy_destination_before_source")?.as_ptr(),
        );
        let source_before_destination = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntULE,
            source_end,
            destination_address,
            c_name("slice_copy_source_before_destination")?.as_ptr(),
        );
        let disjoint = LLVMBuildOr(
            self.builder,
            destination_before_source,
            source_before_destination,
            c_name("slice_copy_disjoint")?.as_ptr(),
        );
        let directional_safe = LLVMBuildOr(
            self.builder,
            same_stride,
            disjoint,
            c_name("slice_copy_directional_safe")?.as_ptr(),
        );
        let unsupported_overlap = LLVMBuildNot(
            self.builder,
            directional_safe,
            c_name("slice_copy_unequal_stride_overlap")?.as_ptr(),
        );
        // A general unequal-stride overlap needs temporary storage. Dynamic
        // stack allocation is not acceptable in realtime code, so the
        // deterministic backend contract rejects that rare shape. Equal
        // strides retain memmove directionality; disjoint unequal strides use
        // the normal forward loop.
        self.emit_trap_if(unsupported_overlap, "slice_copy_strided_safe")?;
        let copy_backward = LLVMBuildAnd(
            self.builder,
            LLVMBuildNot(
                self.builder,
                disjoint,
                c_name("slice_copy_overlaps")?.as_ptr(),
            ),
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntUGT,
                destination_address,
                source_address,
                c_name("slice_copy_destination_after_source")?.as_ptr(),
            ),
            c_name("slice_copy_backward")?.as_ptr(),
        );
        let backward = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_backward",
        )?;
        let forward = append_block(
            self.module.context,
            self.declaration.value,
            "slice_copy_forward",
        )?;
        LLVMBuildCondBr(self.builder, copy_backward, backward, forward);

        LLVMPositionBuilderAtEnd(self.builder, backward);
        self.emit_slice_copy_loop(destination, source, len, true)?;
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, forward);
        self.emit_slice_copy_loop(destination, source, len, false)?;
        LLVMBuildBr(self.builder, merge);

        LLVMPositionBuilderAtEnd(self.builder, merge);
        Ok(())
    }

    unsafe fn emit_slice_copy_loop(
        &mut self,
        destination: SliceParts,
        source: SliceParts,
        len: LLVMValueRef,
        backward: bool,
    ) -> Result<(), MirCodegenError> {
        let preheader = LLVMGetInsertBlock(self.builder);
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let one = LLVMConstInt(i32_ty, 1, 0);
        // Compute the initial value in the preheader so the PHI remains the
        // first instruction in its block, as required by LLVM IR.
        let initial = if backward {
            LLVMBuildSub(
                self.builder,
                len,
                one,
                c_name("slice_copy_backward_start")?.as_ptr(),
            )
        } else {
            zero
        };
        let condition = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_condition"
            } else {
                "slice_copy_forward_condition"
            },
        )?;
        let body = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_body"
            } else {
                "slice_copy_forward_body"
            },
        )?;
        let exit = append_block(
            self.module.context,
            self.declaration.value,
            if backward {
                "slice_copy_backward_exit"
            } else {
                "slice_copy_forward_exit"
            },
        )?;
        LLVMBuildBr(self.builder, condition);

        LLVMPositionBuilderAtEnd(self.builder, condition);
        let index = LLVMBuildPhi(
            self.builder,
            i32_ty,
            c_name(if backward {
                "slice_copy_backward_index"
            } else {
                "slice_copy_forward_index"
            })?
            .as_ptr(),
        );
        LLVMAddIncoming(index, [initial].as_mut_ptr(), [preheader].as_mut_ptr(), 1);
        let in_bounds = if backward {
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSGE,
                index,
                zero,
                c_name("slice_copy_backward_in_bounds")?.as_ptr(),
            )
        } else {
            LLVMBuildICmp(
                self.builder,
                LLVMIntPredicate::LLVMIntSLT,
                index,
                len,
                c_name("slice_copy_forward_in_bounds")?.as_ptr(),
            )
        };
        LLVMBuildCondBr(self.builder, in_bounds, body, exit);

        LLVMPositionBuilderAtEnd(self.builder, body);
        self.copy_slice_element(destination, source, index)?;
        let next = if backward {
            LLVMBuildSub(
                self.builder,
                index,
                one,
                c_name("slice_copy_backward_next")?.as_ptr(),
            )
        } else {
            LLVMBuildAdd(
                self.builder,
                index,
                one,
                c_name("slice_copy_forward_next")?.as_ptr(),
            )
        };
        LLVMBuildBr(self.builder, condition);
        let body_block = LLVMGetInsertBlock(self.builder);
        LLVMAddIncoming(index, [next].as_mut_ptr(), [body_block].as_mut_ptr(), 1);
        LLVMPositionBuilderAtEnd(self.builder, exit);
        Ok(())
    }

    unsafe fn copy_slice_element(
        &self,
        destination: SliceParts,
        source: SliceParts,
        index: LLVMValueRef,
    ) -> Result<(), MirCodegenError> {
        let source_ptr = self.slice_ptr_at_index(source, index, "slice_copy_source")?;
        let destination_ptr =
            self.slice_ptr_at_index(destination, index, "slice_copy_destination")?;
        let value = LLVMBuildLoad2(
            self.builder,
            llvm_scalar_type(self.module.context, source.element),
            source_ptr,
            c_name("slice_copy_value")?.as_ptr(),
        );
        LLVMSetAlignment(value, 1);
        let store = LLVMBuildStore(self.builder, value, destination_ptr);
        LLVMSetAlignment(store, 1);
        Ok(())
    }

    unsafe fn apply_dynamic_bounds(
        &mut self,
        index: LLVMValueRef,
        len: LLVMValueRef,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        match mode {
            onda_mir::BoundsMode::Unchecked => Ok(index),
            onda_mir::BoundsMode::Clamp => {
                let positive = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    len,
                    zero,
                    c_name("dynamic_len_positive")?.as_ptr(),
                );
                let empty = LLVMBuildNot(
                    self.builder,
                    positive,
                    c_name("dynamic_len_empty")?.as_ptr(),
                );
                // Clamp selects the nearest existing element. An empty
                // runtime sequence has no such element, so it must trap rather
                // than fabricate an access to index zero.
                self.emit_trap_if(empty, "dynamic_clamp_nonempty")?;
                self.clamp_dynamic_index(index, len)
            }
            onda_mir::BoundsMode::Trap => {
                let in_bounds = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    index,
                    len,
                    c_name("dynamic_index_in_bounds")?.as_ptr(),
                );
                let ok = append_block(
                    self.module.context,
                    self.declaration.value,
                    "dynamic_bounds_ok",
                )?;
                let trap = append_block(
                    self.module.context,
                    self.declaration.value,
                    "dynamic_bounds_trap",
                )?;
                LLVMBuildCondBr(self.builder, in_bounds, ok, trap);
                LLVMPositionBuilderAtEnd(self.builder, trap);
                let void_ty = LLVMVoidTypeInContext(self.module.context);
                let trap_ty = LLVMFunctionType(void_ty, null_mut(), 0, 0);
                let trap_fn = ensure_named_function(self.module.module, "llvm.trap", trap_ty)?;
                LLVMBuildCall2(
                    self.builder,
                    trap_ty,
                    trap_fn,
                    null_mut(),
                    0,
                    c_name("")?.as_ptr(),
                );
                LLVMBuildUnreachable(self.builder);
                LLVMPositionBuilderAtEnd(self.builder, ok);
                Ok(index)
            }
        }
    }

    unsafe fn clamp_dynamic_index(
        &self,
        index: LLVMValueRef,
        len: LLVMValueRef,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let below = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSLT,
            index,
            zero,
            c_name("dynamic_index_below")?.as_ptr(),
        );
        let low = LLVMBuildSelect(
            self.builder,
            below,
            zero,
            index,
            c_name("dynamic_index_low")?.as_ptr(),
        );
        let one = LLVMConstInt(i32_ty, 1, 0);
        let max = LLVMBuildSub(
            self.builder,
            len,
            one,
            c_name("dynamic_index_max")?.as_ptr(),
        );
        let above = LLVMBuildICmp(
            self.builder,
            LLVMIntPredicate::LLVMIntSGT,
            low,
            max,
            c_name("dynamic_index_above")?.as_ptr(),
        );
        Ok(LLVMBuildSelect(
            self.builder,
            above,
            max,
            low,
            c_name("dynamic_index_clamped")?.as_ptr(),
        ))
    }

    unsafe fn lower_const_data_load(
        &mut self,
        data: onda_mir::ConstDataId,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let descriptor = &self.module.program.const_data[data.index()];
        let index = self.lower_fixed_index(index, descriptor.values.len(), bounds)?;
        let element_ty = llvm_scalar_type(self.module.context, descriptor.element);
        let array_ty = LLVMArrayType2(element_ty, descriptor.values.len() as u64);
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let mut indices = [zero, index];
        let ptr = LLVMBuildInBoundsGEP2(
            self.builder,
            array_ty,
            self.module.const_globals[data.index()],
            indices.as_mut_ptr(),
            2,
            c_name("const_data_ptr")?.as_ptr(),
        );
        Ok(LLVMBuildLoad2(
            self.builder,
            element_ty,
            ptr,
            c_name("const_data")?.as_ptr(),
        ))
    }

    unsafe fn lower_place(&mut self, place: &Place) -> Result<PlaceRef, MirCodegenError> {
        let mut lowered = match place.base {
            onda_mir::PlaceBase::Local(local) => self.locals[local.index()],
            onda_mir::PlaceBase::Parameter(parameter) => self.parameters[parameter.index()],
            onda_mir::PlaceBase::State(state) => {
                let state_ptr = load_context_field(
                    self.module,
                    self.builder,
                    self.runtime_context,
                    6,
                    "state_ptr",
                )?;
                PlaceRef {
                    ptr: byte_offset_ptr(
                        self.module.context,
                        self.builder,
                        state_ptr,
                        self.module.layouts.state.offsets[state.index()],
                        "state_slot",
                    )?,
                    ty: self.module.program.state[state.index()].ty,
                    alignment: self.module.layouts.type_alignments
                        [self.module.program.state[state.index()].ty.index()],
                }
            }
            onda_mir::PlaceBase::Param(param) => {
                let params_ptr = load_context_field(
                    self.module,
                    self.builder,
                    self.runtime_context,
                    5,
                    "params_ptr",
                )?;
                let offset = self.module.layouts.params.offsets[param.index()];
                PlaceRef {
                    ptr: byte_offset_ptr(
                        self.module.context,
                        self.builder,
                        params_ptr,
                        offset,
                        "param_slot",
                    )?,
                    ty: self.module.program.interface.params[param.index()].ty,
                    // Parameter storage is a packed byte ABI. Even when an
                    // individual offset is naturally aligned, the host-owned
                    // base pointer itself has no alignment guarantee beyond 1.
                    alignment: 1,
                }
            }
            onda_mir::PlaceBase::EventParam(parameter) => {
                let FunctionKind::Event(_) = self.function.kind else {
                    return Err(MirCodegenError::invalid(
                        "event parameter place appears outside an event handler",
                    ));
                };
                self.event_parameters[parameter.index()]
            }
        };
        for projection in &place.projections {
            match projection {
                Projection::Index { index, bounds } => {
                    lowered = self.project_array(lowered, *index, *bounds)?;
                }
                Projection::Field(_) => {
                    return Err(MirCodegenError::unsupported(
                        "struct field projections are not in the native MIR slice",
                    ));
                }
            }
        }
        Ok(lowered)
    }

    unsafe fn project_array(
        &mut self,
        place: PlaceRef,
        index: onda_mir::Value,
        bounds: onda_mir::BoundsMode,
    ) -> Result<PlaceRef, MirCodegenError> {
        let Type::Array { element, len } = self.module.program.types[place.ty.index()] else {
            return Err(MirCodegenError::invalid(
                "index projection base is not an array",
            ));
        };
        let index = self.lower_fixed_index(index, len as usize, bounds)?;
        let zero = LLVMConstInt(LLVMInt32TypeInContext(self.module.context), 0, 0);
        let mut indices = [zero, index];
        let ptr = LLVMBuildGEP2(
            self.builder,
            self.module.types.get(place.ty),
            place.ptr,
            indices.as_mut_ptr(),
            2,
            c_name("array_element")?.as_ptr(),
        );
        Ok(PlaceRef {
            ptr,
            ty: element,
            alignment: place
                .alignment
                .min(self.module.layouts.type_alignments[element.index()]),
        })
    }

    unsafe fn lower_fixed_index(
        &mut self,
        index: onda_mir::Value,
        len: usize,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        if mode == onda_mir::BoundsMode::Clamp {
            if let onda_mir::Value::Local(local) = index {
                if let Some(fused) = self.fused_clamped_indices[local.index()] {
                    if let Some(index) = self.lower_fused_clamped_index(fused, len)? {
                        return Ok(index);
                    }
                }
            }
        }
        let index = self.lower_value(index)?;
        self.apply_bounds(index, len, mode)
    }

    unsafe fn lower_fused_clamped_index(
        &mut self,
        fused: FusedClampedIndex,
        len: usize,
    ) -> Result<Option<LLVMValueRef>, MirCodegenError> {
        let max = len
            .checked_sub(1)
            .ok_or_else(|| MirCodegenError::invalid("fixed array has no clampable element"))?;
        let max_i32 = i32::try_from(max).map_err(|_| {
            MirCodegenError::invalid("fixed array index exceeds the i32 MIR domain")
        })?;
        // Every integer through 2^24 is exactly representable in f32. Beyond
        // that boundary, retaining the generic saturating-cast-plus-integer-
        // clamp path avoids rounding the upper bound to an invalid i32 value.
        if fused.scalar == onda_mir::ScalarType::F32 && max_i32 > (1_i32 << 24) {
            return Ok(None);
        }

        let value = self.lower_value(fused.source)?;
        let scalar_ty = llvm_scalar_type(self.module.context, fused.scalar);
        let suffix = if fused.scalar == onda_mir::ScalarType::F64 {
            "f64"
        } else {
            "f32"
        };
        let zero = LLVMConstReal(scalar_ty, 0.0);
        // Every i32 is exactly representable in f64, and the f32 case above
        // explicitly stays within that format's consecutive-integer range.
        let max_float = LLVMConstReal(scalar_ty, f64::from(max_i32));
        // The numeric min/max intrinsics choose the numeric operand when the
        // other is NaN. Consequently this maps every NaN and -infinity to
        // zero, +infinity to the last element, and leaves a finite in-range
        // value ready for poison-free truncation toward zero.
        let maxnum = self.lower_binary_float_intrinsic(
            &format!("llvm.maxnum.{suffix}"),
            scalar_ty,
            value,
            zero,
            "index_nonnegative",
        )?;
        let clamped = self.lower_binary_float_intrinsic(
            &format!("llvm.minnum.{suffix}"),
            scalar_ty,
            maxnum,
            max_float,
            "index_float_clamped",
        )?;
        Ok(Some(LLVMBuildFPToSI(
            self.builder,
            clamped,
            LLVMInt32TypeInContext(self.module.context),
            c_name("index_cast")?.as_ptr(),
        )))
    }

    unsafe fn lower_binary_float_intrinsic(
        &self,
        name: &str,
        scalar_ty: LLVMTypeRef,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
        result_name: &str,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let mut parameter_types = [scalar_ty, scalar_ty];
        let fn_ty = LLVMFunctionType(scalar_ty, parameter_types.as_mut_ptr(), 2, 0);
        let function = ensure_named_function(self.module.module, name, fn_ty)?;
        let call = LLVMBuildCall2(
            self.builder,
            fn_ty,
            function,
            [lhs, rhs].as_mut_ptr(),
            2,
            c_name(result_name)?.as_ptr(),
        );
        Ok(call)
    }

    unsafe fn apply_bounds(
        &mut self,
        index: LLVMValueRef,
        len: usize,
        mode: onda_mir::BoundsMode,
    ) -> Result<LLVMValueRef, MirCodegenError> {
        let i32_ty = LLVMInt32TypeInContext(self.module.context);
        let zero = LLVMConstInt(i32_ty, 0, 0);
        let max = LLVMConstInt(i32_ty, (len - 1) as u64, 0);
        match mode {
            onda_mir::BoundsMode::Unchecked => Ok(index),
            onda_mir::BoundsMode::Clamp => {
                let below = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSLT,
                    index,
                    zero,
                    c_name("index_below")?.as_ptr(),
                );
                let low = LLVMBuildSelect(
                    self.builder,
                    below,
                    zero,
                    index,
                    c_name("index_low")?.as_ptr(),
                );
                let above = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntSGT,
                    low,
                    max,
                    c_name("index_above")?.as_ptr(),
                );
                Ok(LLVMBuildSelect(
                    self.builder,
                    above,
                    max,
                    low,
                    c_name("index_clamped")?.as_ptr(),
                ))
            }
            onda_mir::BoundsMode::Trap => {
                let in_bounds = LLVMBuildICmp(
                    self.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    index,
                    LLVMConstInt(i32_ty, len as u64, 0),
                    c_name("index_in_bounds")?.as_ptr(),
                );
                let ok = append_block(self.module.context, self.declaration.value, "bounds_ok")?;
                let trap =
                    append_block(self.module.context, self.declaration.value, "bounds_trap")?;
                LLVMBuildCondBr(self.builder, in_bounds, ok, trap);
                LLVMPositionBuilderAtEnd(self.builder, trap);
                let void_ty = LLVMVoidTypeInContext(self.module.context);
                let trap_ty = LLVMFunctionType(void_ty, null_mut(), 0, 0);
                let trap_fn = ensure_named_function(self.module.module, "llvm.trap", trap_ty)?;
                LLVMBuildCall2(
                    self.builder,
                    trap_ty,
                    trap_fn,
                    null_mut(),
                    0,
                    c_name("")?.as_ptr(),
                );
                LLVMBuildUnreachable(self.builder);
                LLVMPositionBuilderAtEnd(self.builder, ok);
                Ok(index)
            }
        }
    }

    fn scalar_type_of_value(
        &self,
        value: onda_mir::Value,
    ) -> Result<onda_mir::ScalarType, MirCodegenError> {
        match value {
            onda_mir::Value::Constant(value) => Ok(value.ty()),
            onda_mir::Value::Local(local) => {
                let ty = self.function.locals[local.index()].ty;
                match self.module.program.types[ty.index()] {
                    Type::Scalar(scalar) => Ok(scalar),
                    _ => Err(MirCodegenError::invalid(format!(
                        "local {} is not scalar",
                        local.raw()
                    ))),
                }
            }
        }
    }

    unsafe fn load(&self, place: PlaceRef) -> LLVMValueRef {
        let load = LLVMBuildLoad2(
            self.builder,
            self.module.types.get(place.ty),
            place.ptr,
            c"load".as_ptr(),
        );
        LLVMSetAlignment(load, place.alignment as u32);
        load
    }

    unsafe fn store(&self, place: PlaceRef, value: LLVMValueRef) {
        let store = LLVMBuildStore(self.builder, value, place.ptr);
        LLVMSetAlignment(store, place.alignment as u32);
    }

    unsafe fn set_fast_math(&self, instruction: LLVMValueRef) {
        let flags = if self.module.fast_math {
            LLVMFastMathAll
        } else {
            LLVMFastMathNone
        };
        if flags != LLVMFastMathNone {
            LLVMSetFastMathFlags(instruction, flags);
        }
    }
}

unsafe fn build_entry_runtime_context(
    module: &ModuleEmitter<'_>,
    function: LLVMValueRef,
    kind: FunctionKind,
    builder: LLVMBuilderRef,
) -> Result<LLVMValueRef, MirCodegenError> {
    let context = LLVMBuildAlloca(
        builder,
        module.runtime_context_ty,
        c_name("runtime_context")?.as_ptr(),
    );
    let null = LLVMConstPointerNull(module.ptr_ty);
    let zero = LLVMConstInt(LLVMInt32TypeInContext(module.context), 0, 0);
    let mut fields = [
        null, null, zero, zero, zero, null, null, null, null, null, null, null,
    ];
    match kind {
        FunctionKind::Init => {
            fields[5] = LLVMGetParam(function, 0);
            fields[6] = LLVMGetParam(function, 1);
        }
        FunctionKind::Process => {
            fields[6] = LLVMGetParam(function, 0);
            fields[5] = LLVMGetParam(function, 1);
            fields[0] = LLVMGetParam(function, 2);
            fields[1] = LLVMGetParam(function, 3);
            fields[2] = LLVMGetParam(function, 4);
            fields[3] = LLVMGetParam(function, 5);
            fields[4] = LLVMGetParam(function, 6);
            fields[7] = LLVMGetParam(function, 7);
            fields[8] = LLVMGetParam(function, 8);
            fields[9] = LLVMGetParam(function, 9);
            fields[10] = LLVMGetParam(function, 10);
        }
        FunctionKind::Event(_) => {
            fields[11] = LLVMGetParam(function, 0);
            fields[5] = LLVMGetParam(function, 1);
            fields[6] = LLVMGetParam(function, 2);
            fields[7] = LLVMGetParam(function, 3);
            fields[8] = LLVMGetParam(function, 4);
            fields[9] = LLVMGetParam(function, 5);
            fields[10] = LLVMGetParam(function, 6);
        }
        FunctionKind::User => unreachable!(),
    }
    for (index, value) in fields.into_iter().enumerate() {
        let ptr = LLVMBuildStructGEP2(
            builder,
            module.runtime_context_ty,
            context,
            index as u32,
            c_name("runtime_field")?.as_ptr(),
        );
        LLVMBuildStore(builder, value, ptr);
    }
    Ok(context)
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
        2..=4 => LLVMInt32TypeInContext(module.context),
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
        Type::Slice { .. } | Type::Buffer { .. } => true,
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
        Type::Slice { .. } | Type::Buffer { .. } => Ok(()),
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
                        | CallArgument::Buffer(_) => {}
                    }
                }
            }
            StatementKind::OutputStore { .. }
            | StatementKind::ControlOutputStore { .. }
            | StatementKind::BufferStore { .. }
            | StatementKind::BufferParamStore { .. }
            | StatementKind::SliceStore { .. }
            | StatementKind::SliceFill { .. }
            | StatementKind::SliceCopy { .. } => {}
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
        Rvalue::Use(_)
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
        | Rvalue::BufferParamLen(_)
        | Rvalue::BufferParamChannels(_)
        | Rvalue::BufferParamSampleRate(_)
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
        let words = self.layouts.state.size.saturating_add(7) / 8;
        let mut state_words = RuntimeBuffer::try_from_elem_in(words, 0_u64, allocator)?;
        unsafe {
            (self.compiled.init)(
                abi_const_ptr(params),
                abi_mut_ptr(state_words.as_mut_slice()).cast::<u8>(),
            );
        }
        Ok(RuntimeState {
            state_words,
            state_size_bytes: self.layouts.state.size,
        })
    }

    /// Validates the process ABI shape before entering generated code.
    ///
    /// # Safety
    ///
    /// Every channel pointer in the input and output tables, and every
    /// non-null pointer in the external-buffer table, must remain valid for
    /// the complete region described by the MIR interface and buffer metadata.
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
            buffer_ptrs,
            buffer_frames,
            buffer_channels,
            buffer_sample_rates,
        )?;
        let start_frame = u32::try_from(start_frame)
            .map_err(|_| Diagnostic::runtime("start frame does not fit u32", 0, 0))?;
        let frames = u32::try_from(frames)
            .map_err(|_| Diagnostic::runtime("frame count does not fit u32", 0, 0))?;
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
            );
        }
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
    ) {
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
            );
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
    ) -> Result<(), Diagnostic> {
        let Some(event) = self.compiled.events.get(event_index).copied() else {
            return Ok(());
        };
        self.validate_event_payload(event_index, payload)?;
        self.validate_runtime_regions(state, params)?;
        validate_buffer_abi(
            &self.mir,
            buffer_ptrs,
            buffer_frames,
            buffer_channels,
            buffer_sample_rates,
        )?;
        unsafe {
            event(
                abi_const_ptr(payload),
                abi_const_ptr(params),
                abi_mut_ptr(state.state_words.as_mut_slice()).cast::<u8>(),
                abi_const_ptr(buffer_ptrs),
                abi_const_ptr(buffer_frames),
                abi_const_ptr(buffer_channels),
                abi_const_ptr(buffer_sample_rates),
            );
        }
        Ok(())
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
    ) {
        let Some(event) = self.compiled.events.get(event_index).copied() else {
            return;
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
            );
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
        let target_builder =
            super::jit_utils::create_aggressive_jit_target_machine_builder(options.opt_level)
                .map_err(codegen_diagnostic)?;
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder, target_builder);
        let mut lljit = null_mut();
        let error = LLVMOrcCreateLLJIT(&mut lljit, builder);
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
            let target_machine = super::jit_utils::create_host_target_machine(options.opt_level)
                .map_err(codegen_diagnostic)?;
            let optimize = super::jit_utils::run_default_pass_pipeline(
                module,
                target_machine,
                super::jit_utils::map_opt_level(options.opt_level),
            )
            .map_err(codegen_diagnostic);
            LLVMDisposeTargetMachine(target_machine);
            if let Err(error) = optimize {
                LLVMDisposeModule(module);
                LLVMContextDispose(context);
                return Err(error);
            }
            verify_module(module)?;

            let thread_context =
                super::llvm_helpers::llvm_orc_create_new_thread_safe_context_from_llvm_context(
                    context,
                );
            if thread_context.is_null() {
                LLVMDisposeModule(module);
                LLVMContextDispose(context);
                return Err(MirCodegenError::llvm(
                    "failed to create ORC thread-safe context for MIR",
                ));
            }
            let thread_module = LLVMOrcCreateNewThreadSafeModule(module, thread_context);
            if thread_module.is_null() {
                LLVMDisposeModule(module);
                LLVMOrcDisposeThreadSafeContext(thread_context);
                return Err(MirCodegenError::llvm(
                    "failed to create ORC thread-safe MIR module",
                ));
            }
            let add_error = LLVMOrcLLJITAddLLVMIRModule(
                lljit,
                LLVMOrcLLJITGetMainJITDylib(lljit),
                thread_module,
            );
            if !add_error.is_null() {
                LLVMOrcDisposeThreadSafeModule(thread_module);
                return Err(codegen_diagnostic(super::jit_utils::llvm_error_to_diag(
                    "failed to add native MIR module to LLJIT",
                    add_error,
                )));
            }

            let process = super::jit_utils::lookup_symbol(lljit, "onda_process", "MIR process")
                .map_err(codegen_diagnostic)?;
            let init = super::jit_utils::lookup_symbol(lljit, "onda_init", "MIR init")
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
        let result = (|| {
            let triple = super::jit_utils::target_machine_triple_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let (module, context, _) =
                build_native_module(program, options.fast_math, &triple, &data_layout)?;
            let result = (|| {
                super::jit_utils::run_default_pass_pipeline(
                    module,
                    target_machine,
                    super::jit_utils::map_opt_level(options.opt_level),
                )
                .map_err(codegen_diagnostic)?;
                verify_module(module)?;
                super::jit_utils::llvm_module_to_string(module).map_err(codegen_diagnostic)
            })();
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            result
        })();
        LLVMDisposeTargetMachine(target_machine);
        result
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
        let result = (|| {
            let triple = super::jit_utils::target_machine_triple_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let (module, context, _) =
                build_native_module(program, options.fast_math, &triple, &data_layout)?;
            let result = (|| {
                super::jit_utils::run_default_pass_pipeline(
                    module,
                    target_machine,
                    resolved.opt_level,
                )
                .map_err(codegen_diagnostic)?;
                verify_module(module)?;
                super::jit_utils::llvm_module_to_string(module).map_err(codegen_diagnostic)
            })();
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            result
        })();
        LLVMDisposeTargetMachine(target_machine);
        result
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
        let result = (|| {
            let triple = super::jit_utils::target_machine_triple_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let data_layout = super::jit_utils::target_machine_data_layout_string(target_machine)
                .map_err(codegen_diagnostic)?;
            let (pointer_width_bits, byte_order) = target_data_facts(&data_layout)?;
            let (module, context, layouts) =
                build_native_module(program, options.fast_math, &triple, &data_layout)?;
            let result = (|| {
                super::jit_utils::run_default_pass_pipeline(
                    module,
                    target_machine,
                    resolved.opt_level,
                )
                .map_err(codegen_diagnostic)?;
                verify_module(module)?;
                let object_bytes = emit_object_to_bytes(target_machine, module)?;
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
            })();
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            result
        })();
        LLVMDisposeTargetMachine(target_machine);
        result
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
    let module_name = c_name("onda_mir_module")?;
    let module = LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
    if module.is_null() {
        LLVMContextDispose(context);
        return Err(MirCodegenError::llvm("failed to create LLVM MIR module"));
    }
    let result = (|| {
        let triple = c_name(target_triple)?;
        let layout = c_name(data_layout)?;
        LLVMSetTarget(module, triple.as_ptr());
        LLVMSetDataLayout(module, layout.as_ptr());
        let types = LoweredTypes::build(context, program)?;
        let layouts = compute_native_layouts(program, &types, data_layout)?;
        let emitter = ModuleEmitter::new(program, context, module, &types, &layouts, fast_math)?;
        emitter.emit()?;
        verify_module(module)?;
        Ok(layouts)
    })();
    match result {
        Ok(layouts) => Ok((module, context, layouts)),
        Err(error) => {
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            Err(error)
        }
    }
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
mod tests {
    use super::*;

    use onda_frontend::{parse_program, parse_program_file};
    use onda_semantics::{
        analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions, TypedProgram,
    };

    fn source_program(source: &str, block_size: usize) -> (TypedProgram, Program) {
        let parsed = parse_program(source).expect("source should parse");
        let typed = analyze_with_options(
            parsed,
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size,
            },
        )
        .expect("source should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("source should lower to MIR")
            .into_program();
        (typed, mir)
    }

    fn trusted_optimized(
        program: Program,
    ) -> Result<onda_mir::OptimizedProgram, Vec<MirCodegenError>> {
        let validated = unsafe { onda_mir::validate_owned_with_producer_proofs(program) }.map_err(
            |errors| {
                errors
                    .into_iter()
                    .map(|error| MirCodegenError::invalid(error.to_string()))
                    .collect::<Vec<_>>()
            },
        )?;
        onda_mir::optimize(validated)
            .map(|(program, _)| program)
            .map_err(|errors| {
                errors
                    .into_iter()
                    .map(|error| MirCodegenError::invalid(error.to_string()))
                    .collect()
            })
    }

    fn lower_mir_and_jit(program: Program) -> Result<MirJitProgram, Vec<MirCodegenError>> {
        lower_optimized_mir_and_jit(trusted_optimized(program)?)
    }

    fn lower_mir_and_jit_with_options(
        program: Program,
        options: MirCompileOptions,
    ) -> Result<MirJitProgram, Vec<MirCodegenError>> {
        lower_optimized_mir_and_jit_with_options(trusted_optimized(program)?, options)
    }

    fn lower_mir_to_llvm_ir_with_options(
        program: &Program,
        options: MirCompileOptions,
    ) -> Result<String, Vec<MirCodegenError>> {
        lower_optimized_mir_to_llvm_ir_with_options(&trusted_optimized(program.clone())?, options)
    }

    fn lower_mir_to_target_llvm_ir(
        program: &Program,
        options: &MirTargetOptions,
    ) -> Result<String, Vec<MirCodegenError>> {
        lower_optimized_mir_to_target_llvm_ir(&trusted_optimized(program.clone())?, options)
    }

    fn lower_mir_to_object(
        program: &Program,
        options: &MirTargetOptions,
    ) -> Result<Vec<u8>, Vec<MirCodegenError>> {
        lower_optimized_mir_to_object(&trusted_optimized(program.clone())?, options)
    }

    fn lower_mir_to_object_artifact(
        program: &Program,
        options: &MirTargetOptions,
    ) -> Result<crate::AotObjectArtifact, Vec<MirCodegenError>> {
        lower_optimized_mir_to_object_artifact(&trusted_optimized(program.clone())?, options)
    }

    trait CheckedHostCalls {
        #[allow(clippy::too_many_arguments)]
        fn test_process_checked(
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
        ) -> Result<(), Diagnostic>;

        #[allow(clippy::too_many_arguments)]
        fn test_trigger_event_by_index(
            &self,
            state: &mut RuntimeState,
            params: &[u8],
            event_index: usize,
            payload: &[u8],
            buffer_ptrs: &[*mut u8],
            buffer_frames: &[i32],
            buffer_channels: &[i32],
            buffer_sample_rates: &[f32],
        ) -> Result<(), Diagnostic>;
    }

    impl CheckedHostCalls for MirJitProgram {
        fn test_process_checked(
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
        ) -> Result<(), Diagnostic> {
            unsafe {
                self.process_checked(
                    state,
                    params,
                    start_frame,
                    frames,
                    flags,
                    in_ptrs,
                    out_ptrs,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                )
            }
        }

        fn test_trigger_event_by_index(
            &self,
            state: &mut RuntimeState,
            params: &[u8],
            event_index: usize,
            payload: &[u8],
            buffer_ptrs: &[*mut u8],
            buffer_frames: &[i32],
            buffer_channels: &[i32],
            buffer_sample_rates: &[f32],
        ) -> Result<(), Diagnostic> {
            unsafe {
                self.trigger_event_by_index(
                    state,
                    params,
                    event_index,
                    payload,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                )
            }
        }
    }

    fn collect_onda_examples(directory: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("example directory should be readable") {
            let path = entry.expect("example entry should be readable").path();
            if path.is_dir() {
                collect_onda_examples(&path, paths);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "onda")
            {
                paths.push(path);
            }
        }
    }

    fn run_native_outputs(source: &str, block_size: usize) -> Vec<Vec<f32>> {
        let (_, mir) = source_program(source, block_size);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("native MIR LLVM backend should compile");
        let params = native.default_param_bytes();
        let mut state = native
            .initialize_state(&params)
            .expect("native state should initialize");
        let mut outputs = vec![vec![0.0_f32; block_size]; native.required_out_channels()];
        let output_ptrs = outputs
            .iter_mut()
            .map(|output| output.as_mut_ptr().cast::<u8>())
            .collect::<Vec<_>>();
        let inputs: [*const u8; 0] = [];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                block_size,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &inputs,
                &output_ptrs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .expect("native process should run");
        outputs
    }

    #[test]
    fn split_zero_frame_and_flag_segments_execute_expected_hooks() {
        let source = r#"
ins:
  in1 = 0.0

init:
  value = 0.0

block:
  value = value + 1.0
  sample:
    out1 = value + in1
  value = value + 10.0
"#;
        let block_size = 8;
        let (_, mir) = source_program(source, block_size);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        let native_params = native.default_param_bytes();
        let mut native_state = native.initialize_state(&native_params).unwrap();
        let mut native_output = vec![-99.0_f32; block_size];
        let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
        let input = [0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let inputs = [input.as_ptr().cast::<u8>()];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];

        macro_rules! run_segment {
            ($start:expr, $frames:expr, $flags:expr) => {{
                native
                    .test_process_checked(
                        &mut native_state,
                        &native_params,
                        $start,
                        $frames,
                        $flags,
                        &inputs,
                        &native_outputs,
                        &buffers,
                        &metadata_i32,
                        &metadata_i32,
                        &metadata_f32,
                    )
                    .unwrap();
            }};
        }

        run_segment!(0, 3, onda_mir::PROCESS_BEGIN_BLOCK as u32);
        run_segment!(3, 0, 0);
        run_segment!(3, 5, onda_mir::PROCESS_END_BLOCK as u32);
        assert_eq!(native_output, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);

        // Zero-frame calls still run independently gated hooks. Exercise all
        // four legal flag combinations without imposing positional rules.
        run_segment!(0, 0, 0);
        run_segment!(0, 0, onda_mir::PROCESS_BEGIN_BLOCK as u32);
        run_segment!(block_size, 0, onda_mir::PROCESS_END_BLOCK as u32);
        run_segment!(4, 0, onda_mir::PROCESS_FULL_BLOCK as u32);

        native_output.fill(-99.0);
        run_segment!(0, block_size, 0);
        assert_eq!(
            native_output,
            [33.0, 34.0, 35.0, 36.0, 37.0, 38.0, 39.0, 40.0]
        );
    }

    #[test]
    fn checked_process_rejects_out_of_block_segments_and_unknown_flags() {
        let (_, mir) = source_program("sample:\n  out1 = 0.0\n", 8);
        let native = lower_mir_and_jit(mir).unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let mut output = [0.0_f32; 8];
        let outputs = [output.as_mut_ptr().cast::<u8>()];
        let inputs: [*const u8; 0] = [];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        let mut run = |start_frame, frames, flags| {
            native.test_process_checked(
                &mut state,
                &params,
                start_frame,
                frames,
                flags,
                &inputs,
                &outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
        };
        assert!(run(9, 0, 0).unwrap_err().message.contains("exceeds"));
        assert!(run(7, 2, 0).unwrap_err().message.contains("exceeds"));
        assert!(run(0, 0, 4)
            .unwrap_err()
            .message
            .contains("outside BEGIN_BLOCK/END_BLOCK"));
    }

    #[test]
    fn checked_process_rejects_null_and_misaligned_audio_channels() {
        let (_, mir) = source_program("ins:\n  in1 = 0.0\n\nsample:\n  out1 = in1\n", 8);
        let native = lower_mir_and_jit(mir).unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let input = [0.0_f32; 8];
        let mut output = [0.0_f32; 8];
        let valid_inputs = [input.as_ptr().cast::<u8>()];
        let valid_outputs = [output.as_mut_ptr().cast::<u8>()];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        let mut run = |inputs: &[*const u8], outputs: &[*mut u8]| {
            native.test_process_checked(
                &mut state,
                &params,
                0,
                8,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                inputs,
                outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
        };

        let null_inputs = [std::ptr::null()];
        assert!(run(&null_inputs, &valid_outputs)
            .unwrap_err()
            .message
            .contains("input channel 0 (`in1`) pointer is null"));

        let null_outputs = [std::ptr::null_mut()];
        assert!(run(&valid_inputs, &null_outputs)
            .unwrap_err()
            .message
            .contains("output channel 0 (`out1`) pointer is null"));

        let mut aligned_storage = [0_u32; 9];
        let misaligned_outputs = [aligned_storage.as_mut_ptr().cast::<u8>().wrapping_add(1)];
        assert!(run(&valid_inputs, &misaligned_outputs)
            .unwrap_err()
            .message
            .contains("requires 4-byte alignment"));
    }

    #[test]
    fn real_sine_source_executes_through_mir_llvm() {
        let outputs = run_native_outputs(
            include_str!("../../../../examples/foundations/sine.onda"),
            16,
        );
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].iter().all(|sample| sample.is_finite()));
        assert!(outputs[0].iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn real_const_array_source_executes_through_mir_llvm() {
        let outputs = run_native_outputs(
            include_str!("../../../../examples/foundations/const_harmonic_bank.onda"),
            8,
        );
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn all_checked_in_examples_lower_through_validated_mir_to_target_llvm() {
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut paths = Vec::new();
        collect_onda_examples(&examples, &mut paths);
        paths.sort();
        assert_eq!(
            paths.len(),
            46,
            "the canonical example sweep changed; review new or removed programs"
        );

        let mut failures = Vec::new();
        for path in paths {
            let result = (|| {
                let parsed = parse_program_file(&path)
                    .map_err(|diagnostics| format!("parse failed: {diagnostics:?}"))?;
                let typed = analyze_with_options(
                    parsed,
                    AnalysisOptions {
                        sample_rate: 48_000.0,
                        block_size: 64,
                    },
                )
                .map_err(|diagnostics| format!("analysis failed: {diagnostics:?}"))?;
                let mir = lower_program_to_optimized_mir(&typed)
                    .map_err(|diagnostics| format!("MIR lowering failed: {diagnostics:?}"))?;
                let mut target = crate::TargetConfig::host();
                target.opt_level = TargetOptLevel::O0;
                lower_optimized_mir_to_target_llvm_ir(
                    &mir,
                    &MirTargetOptions {
                        fast_math: false,
                        target,
                    },
                )
                .map(|_| ())
                .map_err(|diagnostics| format!("LLVM lowering failed: {diagnostics:?}"))
            })();
            if let Err(error) = result {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "MIR-native LLVM example sweep failed:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn real_event_and_user_call_source_executes_through_mir_llvm() {
        let block_size = 8;
        let (_, mir) = source_program(
            include_str!("../../../../examples/foundations/simple_events.onda"),
            block_size,
        );
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("native MIR LLVM backend should compile");
        assert_eq!(
            native.event_payload_shape(0),
            Some(MirEventPayloadShape::Fixed { byte_size: 8 })
        );
        let native_params = native.default_param_bytes();
        let mut native_state = native.initialize_state(&native_params).unwrap();
        let mut payload = Vec::new();
        payload.extend_from_slice(&330.0_f32.to_ne_bytes());
        payload.extend_from_slice(&0.5_f32.to_ne_bytes());
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        native
            .test_trigger_event_by_index(
                &mut native_state,
                &native_params,
                0,
                &payload,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();

        let mut native_output = vec![0.0_f32; block_size];
        let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
        let inputs: [*const u8; 0] = [];
        native
            .test_process_checked(
                &mut native_state,
                &native_params,
                0,
                block_size,
                3,
                &inputs,
                &native_outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();
        assert!(native_output.iter().all(|sample| sample.is_finite()));
        assert!(native_output.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn source_input_and_value_call_produces_expected_samples() {
        let source = r#"
params:
  gain = 0.75 { 0.0, 1.0 }

def shape(x: f32, amount: f32) -> f32:
  if (x < 0.0):
    return -x * amount
  return x * amount

sample:
  out1 = shape(in1, gain)
"#;
        let block_size = 8;
        let (_, mir) = source_program(source, block_size);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        let native_params = native.default_param_bytes();
        let mut native_state = native.initialize_state(&native_params).unwrap();
        let input = [-1.0_f32, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0];
        let inputs = [input.as_ptr().cast::<u8>()];
        let mut native_output = vec![0.0_f32; block_size];
        let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        native
            .test_process_checked(
                &mut native_state,
                &native_params,
                0,
                block_size,
                3,
                &inputs,
                &native_outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();
        assert_eq!(
            native_output,
            [0.75, 0.375, 0.1875, 0.0, 0.1875, 0.375, 0.5625, 0.75]
        );
    }

    #[test]
    fn source_fixed_array_parameter_produces_expected_samples() {
        let source = r#"
params:
  weights: f32[3] = [0.25, 0.5, 1.0]

sample:
  out1 = weights[0] + weights[1] + weights[2]
"#;
        let outputs = run_native_outputs(source, 4);
        assert_eq!(outputs, [vec![1.75; 4]]);
    }

    #[test]
    fn array_window_accepts_equivalent_duplicate_element_type_ids() {
        let i32_ty = onda_mir::TypeId::new(0);
        let source_element = onda_mir::TypeId::new(1);
        let parameter_element = onda_mir::TypeId::new(2);
        let source_array = onda_mir::TypeId::new(3);
        let parameter_array = onda_mir::TypeId::new(4);
        let mut program = Program::new(
            onda_mir::CompileConfig {
                sample_rate: 48_000.0,
                block_size: 8,
            },
            onda_mir::FunctionId::new(0),
            onda_mir::FunctionId::new(1),
        );
        program.types = vec![
            Type::Scalar(onda_mir::ScalarType::I32),
            Type::Scalar(onda_mir::ScalarType::F32),
            Type::Scalar(onda_mir::ScalarType::F32),
            Type::Array {
                element: source_element,
                len: 4,
            },
            Type::Array {
                element: parameter_element,
                len: 2,
            },
        ];
        program.state.push(onda_mir::StateSlot {
            name: "source".to_owned(),
            ty: source_array,
            persistence: onda_mir::StatePersistence::Snapshot,
        });

        let empty_function = |name: &str, kind| onda_mir::Function {
            name: name.to_owned(),
            kind,
            attributes: onda_mir::FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: onda_mir::Block::default(),
            source: onda_mir::SourceSpan::UNKNOWN,
        };
        let init = empty_function("onda_init", FunctionKind::Init);
        let mut process = empty_function("onda_process", FunctionKind::Process);
        process.params = onda_mir::process_function_params(i32_ty);
        process.body.statements.push(onda_mir::Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: onda_mir::FunctionId::new(2),
                args: vec![CallArgument::ArrayWindow {
                    array: onda_mir::Place {
                        base: onda_mir::PlaceBase::State(onda_mir::StateId::new(0)),
                        projections: Vec::new(),
                    },
                    start: onda_mir::Value::Constant(onda_mir::ScalarValue::I32(1)),
                    bounds: onda_mir::BoundsMode::Trap,
                }],
            },
            source: onda_mir::SourceSpan::UNKNOWN,
        });
        let mut callee = empty_function("consume_window", FunctionKind::User);
        callee.params.push(onda_mir::FunctionParam {
            name: "window".to_owned(),
            ty: parameter_array,
            mode: onda_mir::PassingMode::ReadOnlyReference,
        });
        program.functions = vec![init, process, callee];

        onda_mir::validate(&program)
            .expect("duplicate scalar type IDs are structurally equivalent in MIR");
        lower_mir_to_llvm_ir_with_options(
            &program,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("LLVM should legalize a structurally equivalent array window");
    }

    #[test]
    fn source_intrinsic_set_executes_through_mir_llvm() {
        let source = r#"
sample:
  x = f32(0.25)
  out1 = sin(x) + cos(x) + tan(x) + tanh(x) + atan(x) + atan2(x, f32(0.5)) + exp(x) + log(f32(1.0) + x) + sqrt(x) + pow(x, f32(2.0)) + abs(-x) + floor(x) + ceil(x) + round(x) + trunc(x) + min(x, f32(0.5)) + max(x, f32(0.5)) + fma(x, f32(2.0), f32(0.125))
"#;
        let outputs = run_native_outputs(source, 2);
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0][0].is_finite());
        assert_eq!(outputs[0][0], outputs[0][1]);
    }

    #[test]
    fn runtime_slice_source_produces_expected_samples() {
        let source = r#"
const Table: f32[3] = [0.25, 0.5, 1.0]

def sum_edges(values: f32[]) -> f32:
  return values[0] + values[values.len() - 1]

sample:
  out1 = sum_edges(Table)
"#;
        let outputs = run_native_outputs(source, 4);
        assert_eq!(outputs, [vec![1.25; 4]]);
    }

    #[test]
    fn runtime_buffer_and_forwarded_buffer_parameter_execute_expected_behavior() {
        let source = r#"
outs:
  out1

buffers:
  table: f32

def touch(buf: buffer[f32], index: i32):
  view = buf[:]
  value = buf[index] + view[index] - view[index]
  unsafe_write(buf, index, value + 1.0)
  return value + f32(buf.len()) + f32(buf.chans()) + buf.samplerate()

def forward(buf: buffer[f32], index: i32):
  return touch(buf, index)

sample:
  out1 = forward(table, 0)
"#;
        let block_size = 4;
        let (_, mir) = source_program(source, block_size);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        assert_eq!(native.buffer_count(), 1);

        let native_params = native.default_param_bytes();
        let mut native_state = native.initialize_state(&native_params).unwrap();
        let mut native_buffer = [2.0_f32, 3.0, 4.0, 5.0];
        let native_buffers = [native_buffer.as_mut_ptr().cast::<u8>()];
        let buffer_frames = [4_i32];
        let buffer_channels = [1_i32];
        let buffer_sample_rates = [100.0_f32];
        let mut native_output = [0.0_f32; 4];
        let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
        let inputs: [*const u8; 0] = [];

        native
            .test_process_checked(
                &mut native_state,
                &native_params,
                0,
                block_size,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &inputs,
                &native_outputs,
                &native_buffers,
                &buffer_frames,
                &buffer_channels,
                &buffer_sample_rates,
            )
            .unwrap();

        assert_eq!(native_output, [107.0, 108.0, 109.0, 110.0]);
        assert_eq!(native_buffer, [6.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn dynamic_slice_event_payload_executes_and_is_validated() {
        let source = r#"
outs:
  out1

init:
  data: f32[4] = [0.0, 0.0, 0.0, 0.0]
  total = 0.0

events:
  fill(values: f32[]):
    data[:] = 0.0
    data[:] = values[:4]
    total = data[0] + data[1] + data[2] + data[3]

sample:
  out1 = total
"#;
        let (_, mir) = source_program(source, 1);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        assert_eq!(native.event_payload_byte_size(0), None);
        assert_eq!(
            native.event_payload_shape(0),
            Some(MirEventPayloadShape::Dynamic)
        );

        let native_params = native.default_param_bytes();
        let mut native_state = native.initialize_state(&native_params).unwrap();
        let mut payload = Vec::new();
        payload.extend_from_slice(&4_i32.to_ne_bytes());
        for value in [10.0_f32, 20.0, 30.0, 40.0] {
            payload.extend_from_slice(&value.to_ne_bytes());
        }
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        native
            .test_trigger_event_by_index(
                &mut native_state,
                &native_params,
                0,
                &payload,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();

        let mut native_output = [0.0_f32; 1];
        let native_outputs = [native_output.as_mut_ptr().cast::<u8>()];
        let inputs: [*const u8; 0] = [];
        native
            .test_process_checked(
                &mut native_state,
                &native_params,
                0,
                1,
                onda_mir::PROCESS_FULL_BLOCK as u32,
                &inputs,
                &native_outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();
        assert_eq!(native_output, [100.0]);

        let invalid_payloads = [
            vec![0_u8; 2],
            (-1_i32).to_ne_bytes().to_vec(),
            {
                let mut truncated = 2_i32.to_ne_bytes().to_vec();
                truncated.extend_from_slice(&1.0_f32.to_ne_bytes());
                truncated
            },
            {
                let mut trailing = 0_i32.to_ne_bytes().to_vec();
                trailing.push(0);
                trailing
            },
        ];
        for invalid in invalid_payloads {
            assert!(native
                .test_trigger_event_by_index(
                    &mut native_state,
                    &native_params,
                    0,
                    &invalid,
                    &buffers,
                    &metadata_i32,
                    &metadata_i32,
                    &metadata_f32,
                )
                .is_err());
        }
        let oversized = i32::MAX.to_ne_bytes();
        let error = native
            .test_trigger_event_by_index(
                &mut native_state,
                &native_params,
                0,
                &oversized,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .expect_err("dynamic event slice byte extent must fit i32");
        assert!(error.message.contains("byte extent exceeds i32"));
    }

    #[test]
    fn control_output_uses_target_aligned_state_storage() {
        let source = r#"
kouts:
  meter: f64

block:
  meter = f64(3.25)
"#;
        let (_, mir) = source_program(source, 4);
        let native = lower_mir_and_jit_with_options(
            mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        let params = native.default_param_bytes();
        let mut state = native.initialize_state(&params).unwrap();
        let inputs: [*const u8; 0] = [];
        let outputs: [*mut u8; 0] = [];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        native
            .test_process_checked(
                &mut state,
                &params,
                0,
                4,
                3,
                &inputs,
                &outputs,
                &buffers,
                &metadata_i32,
                &metadata_i32,
                &metadata_f32,
            )
            .unwrap();
        let offset = native.control_output_storage_byte_offset(0).unwrap();
        assert_eq!(native.control_output_storage_byte_offsets(), &[offset]);
        assert!(native.state_byte_offsets().contains(&offset));
        assert_eq!(offset % std::mem::align_of::<f64>(), 0);
        let value = f64::from_ne_bytes(state.bytes()[offset..offset + 8].try_into().unwrap());
        assert!((value - 3.25).abs() < f64::EPSILON);
    }

    #[test]
    fn integer_and_nan_edge_semantics_execute_without_llvm_poison() {
        let shifted = run_native_outputs(
            r#"
params:
  count: i32 = 32

sample:
  out1 = f32((i32(1) << count) + (i32(-8) >> count))
"#,
            1,
        );
        assert_eq!(shifted[0], [-7.0]);

        let divided = run_native_outputs(
            r#"
params:
  divisor: i32 = -1

sample:
  minimum = i32(-2147483647) - i32(1)
  out1 = f32(minimum / divisor)
  out2 = f32(minimum % divisor)
"#,
            1,
        );
        assert_eq!(divided[0], [-2_147_483_648.0]);
        assert_eq!(divided[1], [0.0]);

        let nan = run_native_outputs(
            r#"
params:
  value = 0.0

sample:
  invalid = value / value
  out1 = 0.0
  if (invalid != invalid):
    out1 = f32(i32(invalid)) + 1.0
"#,
            1,
        );
        assert_eq!(nan[0], [1.0]);
    }

    #[test]
    fn ranged_params_map_nan_to_minimum_with_and_without_fast_math() {
        let (_, mir) = source_program(
            r#"
params:
  value = 0.5 {-1.0, 1.0}

sample:
  out1 = value
"#,
            1,
        );
        let inputs: [*const u8; 0] = [];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        for fast_math in [false, true] {
            let native = lower_mir_and_jit_with_options(
                mir.clone(),
                MirCompileOptions {
                    fast_math,
                    opt_level: TargetOptLevel::O3,
                },
            )
            .expect("ranged-param source should compile");
            let mut params = native.default_param_bytes();
            params[..4].copy_from_slice(&f32::NAN.to_ne_bytes());
            let mut state = native
                .initialize_state(&params)
                .expect("ranged-param state should initialize");
            let mut output = [0.0_f32];
            let outputs = [output.as_mut_ptr().cast::<u8>()];
            native
                .test_process_checked(
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
                .expect("ranged-param process should run");
            assert_eq!(output, [-1.0]);
        }
    }

    #[test]
    fn numeric_edge_lowering_has_explicit_llvm_semantics() {
        let (_, mir) = source_program(
            r#"
params:
  count: i32 = 32
  divisor: i32 = -1
  value = 0.0

sample:
  shifted = i32(1) << count
  divided = shifted / divisor
  invalid = value / value
  converted = i32(invalid)
  if (invalid != invalid):
    out1 = f32(divided + converted)
"#,
            1,
        );
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("numeric edge MIR should emit LLVM IR");
        assert!(ir.contains("and i32"));
        assert!(ir.contains(", 31"));
        assert!(ir.contains("fcmp une float"));
        assert!(ir.contains("@llvm.fptosi.sat.i32.f32"));
        assert!(ir.contains("@llvm.trap"));
        assert!(ir.contains("sdiv i32"));
    }

    #[test]
    fn fused_float_cast_and_fixed_clamp_preserve_edge_semantics() {
        let (_, mir) = source_program(
            r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index = 0.0

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
            1,
        );
        let cases = [
            (f32::NAN, 10.0),
            (f32::from_bits(0x7f80_0001), 10.0),
            (f32::from_bits(0xff80_0001), 10.0),
            (f32::NEG_INFINITY, 10.0),
            (-3.5, 10.0),
            (-0.5, 10.0),
            (-0.0, 10.0),
            (0.999, 10.0),
            (1.0, 20.0),
            (1.999, 20.0),
            (2.0, 30.0),
            (2.999, 30.0),
            (f32::MAX, 30.0),
            (f32::INFINITY, 30.0),
        ];
        let inputs: [*const u8; 0] = [];
        let buffers: [*mut u8; 0] = [];
        let metadata_i32: [i32; 0] = [];
        let metadata_f32: [f32; 0] = [];
        for fast_math in [false, true] {
            let native = lower_mir_and_jit_with_options(
                mir.clone(),
                MirCompileOptions {
                    fast_math,
                    opt_level: TargetOptLevel::O3,
                },
            )
            .expect("fused fixed-index source should compile");
            for (index, expected) in cases {
                let mut params = native.default_param_bytes();
                params[..4].copy_from_slice(&index.to_ne_bytes());
                let mut state = native
                    .initialize_state(&params)
                    .expect("fixed-index state should initialize");
                let mut output = [0.0_f32];
                let outputs = [output.as_mut_ptr().cast::<u8>()];
                native
                    .test_process_checked(
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
                    .expect("fixed-index edge case should process");
                assert_eq!(
                    output[0], expected,
                    "unexpected output for index {index:?} with fast_math={fast_math}"
                );
            }
        }
    }

    #[test]
    fn optimized_fixed_float_index_fuses_saturation_with_clamp() {
        let (_, mir) = source_program(
            r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index = 0.0

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
            1,
        );
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("fused fixed-index source should emit LLVM IR");
        assert!(ir.contains("@llvm.maxnum.f32"));
        assert!(ir.contains("@llvm.minnum.f32"));
        assert!(ir.contains("fptosi float"));
        assert!(
            !ir.contains("@llvm.fptosi.sat.i32.f32"),
            "dead standalone saturation should disappear after fixed-index fusion"
        );

        let (_, wide_mir) = source_program(
            r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

params:
  index: f64 = f64(0.0)

def lookup(index: f64):
  return Table[index]

sample:
  out1 = lookup(index)
"#,
            1,
        );
        let wide_ir = lower_mir_to_llvm_ir_with_options(
            &wide_mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("f64 fixed-index source should emit LLVM IR");
        assert!(wide_ir.contains("@llvm.maxnum.f64"));
        assert!(wide_ir.contains("@llvm.minnum.f64"));
        assert!(wide_ir.contains("fptosi double"));
        assert!(!wide_ir.contains("@llvm.fptosi.sat.i32.f64"));
    }

    #[test]
    fn fused_index_provenance_counts_read_write_call_arguments() {
        let (_, mut mir) = source_program(
            r#"
const Table: f32[3] = [10.0, 20.0, 30.0]

def lookup(index):
  return Table[index]

sample:
  out1 = lookup(1.0)
"#,
            1,
        );
        let (function_index, cast_local, source_local, source) = mir
            .functions
            .iter()
            .enumerate()
            .find_map(|(function_index, function)| {
                function.body.statements.iter().find_map(|statement| {
                    let StatementKind::Assign {
                        destination,
                        value:
                            Rvalue::Cast {
                                value: onda_mir::Value::Local(source_local),
                                to: onda_mir::ScalarType::I32,
                            },
                    } = &statement.kind
                    else {
                        return None;
                    };
                    let onda_mir::PlaceBase::Local(cast_local) = destination.base else {
                        return None;
                    };
                    destination.projections.is_empty().then_some((
                        function_index,
                        cast_local,
                        *source_local,
                        statement.source,
                    ))
                })
            })
            .expect("lookup MIR should contain a float-to-i32 index cast");
        assert!(
            fused_clamped_index_sources(&mir, &mir.functions[function_index])[cast_local.index()]
                .is_some()
        );

        let source_ty = mir.functions[function_index].locals[source_local.index()].ty;
        let mut mutator = mir.functions[function_index].clone();
        mutator.name = "__test_mutate_index_source".to_owned();
        mutator.params = vec![onda_mir::FunctionParam {
            name: "value".to_owned(),
            ty: source_ty,
            mode: onda_mir::PassingMode::ReadWriteReference,
        }];
        mutator.results.clear();
        mutator.locals.clear();
        mutator.body = Block::default();
        let mutator_id = onda_mir::FunctionId::new(mir.functions.len() as u32);
        mir.functions.push(mutator);
        mir.functions[function_index]
            .body
            .statements
            .push(onda_mir::Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function: mutator_id,
                    args: vec![CallArgument::Place(Place::local(source_local))],
                },
                source,
            });
        assert!(
            fused_clamped_index_sources(&mir, &mir.functions[function_index])[cast_local.index()]
                .is_none(),
            "a mutable-reference call must invalidate immutable cast provenance"
        );
    }

    #[test]
    fn raw_checked_buffer_abi_rejects_wrapping_extents_and_bad_rates() {
        let (_, mir) = source_program(
            r#"
buffers:
  data: f64[]
sample:
  out1 = 0.0
"#,
            1,
        );
        let mut storage = [0_u64; 1];
        let pointer = storage.as_mut_ptr().cast::<u8>();
        let pointers = [pointer];
        let overflow = validate_buffer_abi(&mir, &pointers, &[i32::MAX], &[2], &[48_000.0])
            .expect_err("wrapping buffer element count must be rejected");
        assert!(overflow.message.contains("exceeds i32"));

        let byte_overflow =
            validate_buffer_abi(&mir, &pointers, &[i32::MAX / 8 + 1], &[1], &[48_000.0])
                .expect_err("f64 byte extent must fit i32 even when element count does");
        assert!(byte_overflow.message.contains("byte extent"));

        for sample_rate in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let error = validate_buffer_abi(&mir, &pointers, &[1], &[1], &[sample_rate])
                .expect_err("invalid sample rate metadata must be rejected");
            assert!(error.message.contains("finite positive sample rate"));
        }
    }

    #[test]
    fn clamped_buffer_accesses_omit_empty_range_guards() {
        let (_, mir) = source_program(
            r#"
buffers:
  data: f32
sample:
  out1 = data[0]
"#,
            1,
        );
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("buffer read MIR should emit LLVM IR");
        assert!(ir.contains("buffer_total_len"));
        assert!(ir.contains("dynamic_index_clamped"));
        assert!(!ir.contains("dynamic_len_positive"));
        assert!(!ir.contains("dynamic_clamp_nonempty"));
    }

    #[test]
    fn raw_checked_buffer_abi_requires_nonempty_bound_buffers() {
        let (_, mir) = source_program(
            r#"
buffers:
  data: f32
sample:
  out1 = 0.0
"#,
            1,
        );
        let mut storage = [0_u32; 1];
        let pointer = storage.as_mut_ptr().cast::<u8>();
        validate_buffer_abi(&mir, &[pointer], &[1], &[1], &[48_000.0])
            .expect("positive, non-null buffer binding should be accepted");

        let null = std::ptr::null_mut();
        for (pointer, frames, channels) in
            [(null, 0, 0), (pointer, 0, 0), (pointer, 1, 0), (null, 1, 1)]
        {
            let error = validate_buffer_abi(&mir, &[pointer], &[frames], &[channels], &[48_000.0])
                .expect_err("raw processor ABI requires every declared buffer to be bound");
            assert!(error.message.contains("must be bound"));
        }

        let error = validate_buffer_abi(&mir, &[null], &[0], &[0], &[f32::NAN])
            .expect_err("invalid sample-rate metadata must be rejected");
        assert!(error.message.contains("finite positive sample rate"));

        let error = validate_buffer_abi(&mir, &[pointer], &[1], &[2], &[48_000.0])
            .expect_err("non-empty bindings must honor declared channel constraints");
        assert!(
            error.message.contains("requires 1 channels"),
            "{}",
            error.message
        );

        let mut aligned_storage = [0_u32; 2];
        let misaligned = aligned_storage.as_mut_ptr().cast::<u8>().wrapping_add(1);
        let error = validate_buffer_abi(&mir, &[misaligned], &[1], &[1], &[48_000.0])
            .expect_err("non-empty bindings must honor scalar alignment");
        assert!(error.message.contains("requires 4-byte alignment"));
    }

    #[test]
    fn raw_abi_uses_null_pointers_for_absent_surfaces() {
        assert!(abi_const_ptr::<u8>(&[]).is_null());
        assert!(abi_mut_ptr::<u8>(&mut []).is_null());

        let values = [1_u8];
        let mut mutable_values = [1_u8];
        assert_eq!(abi_const_ptr(&values), values.as_ptr());
        assert_eq!(
            abi_mut_ptr(&mut mutable_values),
            mutable_values.as_mut_ptr()
        );
    }

    #[test]
    fn physical_state_region_size_is_rounded_to_its_alignment() {
        let (_, mir) = source_program(
            r#"
init:
  wide: f64 = 1.0
  narrow: f32 = 2.0

sample:
  wide = wide + f64(narrow)
  out1 = f32(wide)
"#,
            1,
        );
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("mixed-alignment state should JIT");
        assert_eq!(native.state_byte_offsets(), &[0, 8]);
        assert_eq!(native.state_alignment_bytes(), 8);
        assert_eq!(native.state_size_bytes(), 16);
        assert_eq!(
            native.state_size_bytes() % native.state_alignment_bytes(),
            0
        );
        let mut target = crate::TargetConfig::host();
        target.opt_level = TargetOptLevel::O0;
        let artifact = lower_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: false,
                target,
            },
        )
        .expect("mixed-alignment AOT artifact should emit");
        assert_eq!(
            artifact.metadata.runtime.state_size_bytes,
            native.state_size_bytes()
        );
        assert_eq!(
            artifact.metadata.runtime.state_align_bytes,
            native.state_alignment_bytes()
        );
    }

    #[test]
    fn overlapping_slice_copy_is_memmove_safe_without_dynamic_alloca() {
        let source = r#"
sample:
  forward = [1.0, 2.0, 3.0, 4.0]
  forward[1:4] = forward[0:3]
  backward = [1.0, 2.0, 3.0, 4.0]
  backward[0:3] = backward[1:4]
  out1 = forward[0] + forward[1] * 10.0 + forward[2] * 100.0 + forward[3] * 1000.0
  out2 = backward[0] + backward[1] * 10.0 + backward[2] * 100.0 + backward[3] * 1000.0
"#;
        let outputs = run_native_outputs(source, 1);
        assert_eq!(outputs[0], [3211.0]);
        assert_eq!(outputs[1], [4432.0]);

        let (_, mir) = source_program(source, 1);
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("slice copy MIR should emit LLVM IR");
        assert!(ir.contains("@llvm.memmove"));
        assert!(ir.contains("slice_copy_unequal_stride_overlap"));
        assert!(!ir.contains("slice_copy_temporary"));
        assert!(
            !ir.lines()
                .any(|line| line.contains("alloca i8") && line.contains(", i")),
            "slice copy must not introduce a runtime-sized stack allocation"
        );
    }

    #[test]
    fn function_inline_hints_control_o3_helper_shape() {
        let (_, mut mir) = source_program(
            r#"
ins:
  in1
params:
  amount = 0.25

def shape(x: f32, amount: f32):
  return (x + amount) * (x - amount)

sample:
  out1 = shape(in1, amount)
"#,
            64,
        );
        let function_index = mir
            .functions
            .iter()
            .position(|function| {
                matches!(function.kind, FunctionKind::User) && function.name.contains("shape")
            })
            .expect("source helper should lower to a MIR user function");
        let symbol = format!("@__onda_mir_fn_{function_index}");

        mir.functions[function_index].attributes.inline = onda_mir::InlineHint::Never;
        let never_ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("noinline helper should emit");
        assert!(never_ir.contains("noinline"));
        assert!(never_ir
            .lines()
            .any(|line| { line.contains("call") && line.contains(&symbol) }));

        mir.functions[function_index].attributes.inline = onda_mir::InlineHint::Always;
        let always_ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        )
        .expect("alwaysinline helper should emit");
        assert!(!always_ir
            .lines()
            .any(|line| { line.contains("call") && line.contains(&symbol) }));
        assert!(!always_ir
            .lines()
            .any(|line| { line.starts_with("define internal") && line.contains(&symbol) }));
    }

    #[test]
    fn redundant_zero_state_initialization_does_not_reappear_in_llvm() {
        let (_, mir) = source_program(
            r#"
init:
  value = 0.0

sample:
  out1 = value
"#,
            1,
        );
        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("zero-initialized state should emit LLVM IR");
        let init = ir
            .split("define void @onda_init")
            .nth(1)
            .and_then(|tail| tail.split("\n}").next())
            .expect("onda_init definition");
        assert!(
            !init.contains("state_slot"),
            "pre-zeroed state must not receive a redundant generated store:\n{init}"
        );
    }

    #[test]
    fn packed_parameters_and_reference_calls_carry_sound_alignment_facts() {
        let (_, mir) = source_program(
            r#"
ins:
  in1
params:
  tag: i32 = 0
  gain: f64 = 0.5

def identity(value: f32):
  return value

sample:
  value = in1
  out1 = f32(gain) + identity(value) + f32(tag)
"#,
            1,
        );
        let mut mir = mir;
        let function_index = mir
            .functions
            .iter()
            .position(|function| {
                matches!(function.kind, FunctionKind::User) && function.name.contains("identity")
            })
            .expect("identity helper should lower");
        mir.functions[function_index].params[0].mode = onda_mir::PassingMode::ReadOnlyReference;
        fn rewrite_reference_call(block: &mut Block, target: onda_mir::FunctionId) -> bool {
            for statement in &mut block.statements {
                match &mut statement.kind {
                    StatementKind::Call { function, args, .. } if *function == target => {
                        let local = match &args[0] {
                            CallArgument::Value(onda_mir::Value::Local(local)) => *local,
                            _ => panic!("identity argument should be a local value"),
                        };
                        args[0] = CallArgument::Place(Place::local(local));
                        return true;
                    }
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        if rewrite_reference_call(then_block, target)
                            || rewrite_reference_call(else_block, target)
                        {
                            return true;
                        }
                    }
                    StatementKind::Loop { body } => {
                        if rewrite_reference_call(body, target) {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            false
        }
        let target = onda_mir::FunctionId::new(function_index as u32);
        assert!(
            mir.functions
                .iter_mut()
                .any(|function| rewrite_reference_call(&mut function.body, target)),
            "identity call should be rewritten to a reference argument"
        );
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("packed parameter MIR should JIT");
        assert_eq!(native.param_byte_offsets(), &[0, 4]);
        assert_eq!(native.param_byte_size(), 12);

        let ir = lower_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("reference parameter MIR should emit LLVM IR");
        assert!(ir
            .lines()
            .any(|line| { line.contains("load double") && line.contains("align 1") }));
        let reference_definition = ir
            .lines()
            .find(|line| line.starts_with("define internal") && line.contains("@__onda_mir_fn_"))
            .expect("reference-taking user function definition");
        for fact in [
            "captures(none)",
            "nonnull",
            "readonly",
            "align 1",
            "dereferenceable(4)",
        ] {
            assert!(
                reference_definition.contains(fact),
                "missing reference ABI fact '{fact}' in {reference_definition}"
            );
        }
    }

    #[test]
    fn target_configured_ir_and_object_have_native_entry_abi() {
        let (_, mir) = source_program(
            include_str!("../../../../examples/foundations/sine.onda"),
            4,
        );
        let mut target = crate::TargetConfig::host();
        target.opt_level = TargetOptLevel::O0;
        let options = MirTargetOptions {
            fast_math: false,
            target,
        };
        let ir = lower_mir_to_target_llvm_ir(&mir, &options).expect("MIR should emit LLVM IR");
        let object = lower_mir_to_object(&mir, &options).expect("MIR should emit an object file");
        assert!(!object.is_empty());
        assert!(ir.contains("target triple ="));
        assert!(ir.contains("target datalayout ="));
        assert!(ir.contains("define void @onda_process("));
        let signature = ir
            .lines()
            .find(|line| line.contains("define void @onda_process("))
            .expect("process definition");
        assert_eq!(signature.matches("ptr").count(), 8);
        assert_eq!(signature.matches("i32 noundef range(i32").count(), 3);
    }

    #[test]
    fn object_artifact_sidecar_matches_native_control_and_event_layouts() {
        let (_, mir) = source_program(
            r#"
kouts { meter: f64 }

init { held = 1.25 }

block { meter = f64(held) }

events {
  fixed(head: f32[2] = [0.25, 0.5], stamp: i64 = i64(7)) {
    held = head[0] + f32(stamp)
  }

  dynamic(head: f32[2], tail: f32[], stamp: i64) {
    held = head[0] + tail[0] + f32(stamp)
  }
}
"#,
            64,
        );
        let native = lower_mir_and_jit_with_options(
            mir.clone(),
            MirCompileOptions {
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .expect("MIR should JIT for layout comparison");
        let mut target = crate::TargetConfig::host();
        target.opt_level = TargetOptLevel::O0;
        let artifact = lower_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: false,
                target,
            },
        )
        .expect("MIR object and sidecar should emit");

        assert!(!artifact.object_bytes.is_empty());
        assert_eq!(artifact.metadata.format, crate::PROCESSOR_ARTIFACT_FORMAT);
        assert_eq!(artifact.metadata.abi_version, crate::PROCESSOR_ABI_VERSION);
        assert_eq!(artifact.metadata.artifact_kind, "relocatable_object");
        assert_eq!(artifact.metadata.backend, "llvm");
        assert_eq!(
            artifact.metadata.mir_schema_version,
            onda_mir::MIR_SCHEMA_VERSION
        );
        assert_eq!(
            artifact.metadata.format_version,
            crate::AOT_METADATA_FORMAT_VERSION
        );
        assert_eq!(artifact.metadata.compile.sample_rate, 48_000.0);
        assert_eq!(artifact.metadata.compile.block_size, 64);
        assert_eq!(artifact.metadata.exports.init, "onda_init");
        assert_eq!(artifact.metadata.exports.process, "onda_process");
        assert_eq!(artifact.metadata.target.pointer_model, "native_address");
        assert_eq!(artifact.metadata.target.calling_convention, "c");
        assert!(artifact.metadata.target.pointer_width_bits >= 32);
        assert!(!artifact.metadata.target.data_layout.is_empty());
        assert!(matches!(
            artifact.metadata.integration.profile,
            crate::aot_artifact::AotIntegrationProfile::NativeRelocatableObject { .. }
        ));
        assert_eq!(
            artifact.metadata.exports.events,
            ["onda_event_0", "onda_event_1"]
        );
        assert_eq!(
            artifact.metadata.runtime.state_size_bytes,
            native.state_size_bytes()
        );
        assert_eq!(
            artifact.metadata.runtime.state_align_bytes,
            native.state_alignment_bytes()
        );
        assert!(artifact.metadata.runtime.param_align_bytes >= 1);
        assert_eq!(artifact.metadata.runtime.state_initialization, "zeroed");
        assert_eq!(
            artifact.metadata.runtime.snapshot_format_version,
            crate::AOT_SNAPSHOT_FORMAT_VERSION
        );
        assert_eq!(
            artifact.metadata.runtime.snapshot_byte_order,
            "little_endian"
        );
        assert_eq!(
            artifact.metadata.runtime.snapshot_restore_base,
            "post_init_physical_state_image"
        );
        assert!(
            artifact.metadata.runtime.snapshot_size_bytes
                <= artifact.metadata.runtime.state_size_bytes
        );

        let meter = artifact
            .metadata
            .metadata
            .control_outputs
            .first()
            .expect("meter sidecar descriptor");
        assert_eq!(meter.name, "meter");
        assert_eq!(meter.type_repr, "f64");
        assert_eq!(meter.slot_offset, 0);
        assert_eq!(meter.byte_offset, Some(0));
        assert_eq!(meter.byte_size, 8);
        assert_eq!(
            meter.state_byte_offset,
            native.control_output_storage_byte_offset(0)
        );

        let fixed = &artifact.metadata.metadata.events[0];
        assert_eq!(fixed.name, "fixed");
        assert_eq!(fixed.payload_size_bytes, native.event_payload_byte_size(0));
        assert_eq!(fixed.payload_size_bytes, Some(16));
        assert_eq!(fixed.params[0].type_repr, "f32[2]");
        assert_eq!(fixed.params[0].byte_offset, Some(0));
        assert_eq!(fixed.params[0].byte_size, Some(8));
        assert!(fixed.params[0].has_default);
        assert_eq!(
            fixed.params[0].default_reprs,
            Some(vec!["0.25".to_owned(), "0.5".to_owned()])
        );
        assert_eq!(fixed.params[1].type_repr, "i64");
        assert_eq!(fixed.params[1].byte_offset, Some(8));
        assert_eq!(fixed.params[1].byte_size, Some(8));
        assert!(fixed.params[1].has_default);
        assert_eq!(fixed.params[1].default_reprs, Some(vec!["7".to_owned()]));

        let dynamic = &artifact.metadata.metadata.events[1];
        assert_eq!(dynamic.name, "dynamic");
        assert_eq!(
            dynamic.payload_size_bytes,
            native.event_payload_byte_size(1)
        );
        assert_eq!(dynamic.payload_size_bytes, None);
        assert_eq!(dynamic.params[0].byte_offset, Some(0));
        assert_eq!(dynamic.params[0].byte_size, Some(8));
        assert_eq!(dynamic.params[1].type_repr, "f32[]");
        assert!(dynamic.params[1].is_slice);
        assert_eq!(dynamic.params[1].byte_offset, Some(8));
        assert_eq!(dynamic.params[1].byte_size, None);
        assert_eq!(dynamic.params[2].byte_offset, None);
        assert_eq!(dynamic.params[2].byte_size, Some(8));
    }

    #[test]
    fn wasm_aot_artifact_is_relocatable_and_declares_linker_contract() {
        let (_, mir) = source_program(
            r#"
init { phase = 0.0 }
sample { out1 = phase }
"#,
            64,
        );
        let artifact = lower_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: false,
                target: crate::TargetConfig::for_triple("wasm32-unknown-unknown"),
            },
        )
        .expect("LLVM should emit a relocatable wasm32 processor object");

        assert!(artifact.object_bytes.starts_with(b"\0asm\x01\0\0\0"));
        assert!(artifact
            .object_bytes
            .windows(b"linking".len())
            .any(|window| window == b"linking"));
        assert_eq!(artifact.metadata.target.pointer_width_bits, 32);
        assert_eq!(artifact.metadata.target.byte_order, "little_endian");
        assert_eq!(
            artifact.metadata.target.pointer_model,
            "linear_memory_offset"
        );
        assert!(matches!(
            artifact.metadata.integration.profile,
            crate::aot_artifact::AotIntegrationProfile::WebassemblyRelocatableObject {
                no_entry: true,
                export_memory: true,
                ..
            }
        ));
        assert_eq!(
            artifact.metadata.integration.required_symbols,
            ["onda_init", "onda_process"]
        );
    }

    #[test]
    fn aot_snapshot_manifest_maps_persistent_segments_only() {
        let i32_ty = onda_mir::TypeId::new(0);
        let f32_ty = onda_mir::TypeId::new(1);
        let f64_ty = onda_mir::TypeId::new(2);
        let mut mir = Program::new(
            onda_mir::CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            onda_mir::FunctionId::new(0),
            onda_mir::FunctionId::new(1),
        );
        mir.types = vec![
            Type::Scalar(onda_mir::ScalarType::I32),
            Type::Scalar(onda_mir::ScalarType::F32),
            Type::Scalar(onda_mir::ScalarType::F64),
        ];
        mir.state = vec![
            onda_mir::StateSlot {
                name: "phase".to_owned(),
                ty: f32_ty,
                persistence: onda_mir::StatePersistence::Snapshot,
            },
            onda_mir::StateSlot {
                name: "meter".to_owned(),
                ty: f64_ty,
                persistence: onda_mir::StatePersistence::ControlMirror,
            },
            onda_mir::StateSlot {
                name: "$scratch".to_owned(),
                ty: i32_ty,
                persistence: onda_mir::StatePersistence::InstanceScratch,
            },
            onda_mir::StateSlot {
                name: "history".to_owned(),
                ty: f64_ty,
                persistence: onda_mir::StatePersistence::Snapshot,
            },
        ];
        mir.interface.control_outputs.push(onda_mir::ControlOutput {
            name: "meter".to_owned(),
            ty: f64_ty,
            mirror: onda_mir::StateId::new(1),
        });
        let empty_function = |name: &str, kind| onda_mir::Function {
            name: name.to_owned(),
            kind,
            attributes: onda_mir::FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: onda_mir::Block::default(),
            source: onda_mir::SourceSpan::UNKNOWN,
        };
        let init = empty_function("onda_init", FunctionKind::Init);
        let mut process = empty_function("onda_process", FunctionKind::Process);
        process.params = onda_mir::process_function_params(i32_ty);
        mir.functions = vec![init, process];

        let mut target = crate::TargetConfig::host();
        target.opt_level = TargetOptLevel::O0;
        let artifact = lower_mir_to_object_artifact(
            &mir,
            &MirTargetOptions {
                fast_math: false,
                target,
            },
        )
        .expect("state snapshot manifest should emit");

        assert_eq!(artifact.metadata.runtime.snapshot_size_bytes, 12);
        assert_eq!(
            artifact.metadata.runtime.snapshot_byte_order,
            "little_endian"
        );
        assert_eq!(
            artifact.metadata.runtime.snapshot_restore_base,
            "post_init_physical_state_image"
        );
        let states = &artifact.metadata.metadata.states;
        assert_eq!(
            states
                .iter()
                .map(|state| state.name.as_str())
                .collect::<Vec<_>>(),
            ["phase", "history"]
        );
        assert_eq!(states[0].type_repr, "f32");
        assert_eq!(states[0].element_size_bytes, 4);
        assert_eq!(states[0].packed_snapshot_byte_offset, 0);
        assert_eq!(states[0].physical_state_byte_offset, 0);
        assert_eq!(states[0].byte_size, 4);
        assert_eq!(states[1].type_repr, "f64");
        assert_eq!(states[1].element_size_bytes, 8);
        assert_eq!(states[1].packed_snapshot_byte_offset, 4);
        assert_ne!(
            states[1].packed_snapshot_byte_offset,
            states[1].physical_state_byte_offset
        );
        assert_eq!(states[1].byte_size, 8);
        assert!(states
            .iter()
            .all(|state| state.name != "meter" && state.name != "$scratch"));
    }
}
