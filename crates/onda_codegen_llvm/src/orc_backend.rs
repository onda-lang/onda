#[cfg(test)]
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
#[cfg(test)]
use std::mem::{align_of, size_of};
use std::ptr::null_mut;

#[cfg(test)]
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::orc2::lljit::*;
use llvm_sys::orc2::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{LLVMCopyStringRepOfTargetData, LLVMDisposeTargetData};
#[cfg(test)]
use llvm_sys::target::{
    LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
};
#[cfg(test)]
use llvm_sys::target_machine::LLVMDisposeTargetMachine;
#[cfg(test)]
use llvm_sys::target_machine::{LLVMCodeGenFileType, LLVMTargetMachineEmitToMemoryBuffer};
use llvm_sys::target_machine::{
    LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout, LLVMCreateTargetMachine,
    LLVMCreateTargetMachineOptions, LLVMCreateTargetMachineWithOptions,
    LLVMDisposeTargetMachineOptions, LLVMGetDefaultTargetTriple, LLVMGetHostCPUFeatures,
    LLVMGetHostCPUName, LLVMGetTargetFromTriple, LLVMGetTargetMachineTriple,
    LLVMNormalizeTargetTriple, LLVMRelocMode, LLVMTargetMachineOptionsSetABI,
    LLVMTargetMachineOptionsSetCPU, LLVMTargetMachineOptionsSetCodeGenOptLevel,
    LLVMTargetMachineOptionsSetCodeModel, LLVMTargetMachineOptionsSetFeatures,
    LLVMTargetMachineOptionsSetRelocMode, LLVMTargetMachineRef, LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
#[cfg(test)]
use llvm_sys::{LLVMFastMathFlags, LLVMFastMathNone, LLVMIntPredicate, LLVMRealPredicate};
use onda_frontend::Diagnostic;
#[cfg(test)]
use onda_frontend::{
    AssignTarget, BinaryOp, BuiltinFn, CallArg, CallTypeArg, CmpOp, Expr, LogicalOp, PrimitiveType,
    Stmt, INTERNAL_BUFFER_READ2_FN, INTERNAL_BUFFER_WRITE2_FN,
};
#[cfg(test)]
use onda_semantics::{
    builtins::{
        builtin_constant, builtin_constant_type, is_builtin_buffer_2d_unsafe_fn,
        is_builtin_unsafe_data_fn, parse_array_len_instance_base, parse_buffer_chans_instance_base,
        parse_buffer_samplerate_instance_base, parse_unsafe_read2_instance_base,
        parse_unsafe_read_instance_base, parse_unsafe_write2_instance_base,
        parse_unsafe_write_instance_base, BuiltinConstantValue, UNSAFE_READ2_FN, UNSAFE_READ_FN,
        UNSAFE_WRITE2_FN, UNSAFE_WRITE_FN,
    },
    internal_names::{
        sanitize_runtime_symbol_component, PROC_INDEX_BASE_ARG, PROC_INDEX_BUFFER_SELECT_SENTINEL,
        PROC_INDEX_EXPR_ARG,
    },
    ProcSincStageStateFields, ProcStepOversampleMeta, ReturnType, TypedArrayInfo,
    TypedBufferChannels, TypedConstValue, TypedEventParamType, TypedFieldType, TypedFnParam,
    TypedFunction, TypedProgram, TypedStructField,
};

#[cfg(test)]
use crate::primitives::primitive_type_bytes;

#[cfg(test)]
mod array_access;
#[cfg(test)]
mod builtin_intrinsics;
#[cfg(test)]
mod call_helpers;
#[cfg(test)]
mod contexts;
#[cfg(test)]
mod def_lowering;
#[cfg(test)]
mod expr_common;
mod jit_utils;
#[cfg(test)]
mod layout;
mod llvm_helpers;
#[cfg(test)]
mod lowering_common;
mod mir_native;
#[cfg(test)]
mod orc_expr_stmt;
#[cfg(test)]
mod orc_locals;
#[cfg(test)]
mod oversampling;
#[cfg(test)]
mod pipeline;
#[cfg(test)]
mod pointer_helpers;
#[cfg(test)]
mod proc_buffer_refs;
#[cfg(test)]
mod proc_ir;
#[cfg(test)]
mod process_handle;
#[cfg(test)]
mod specialization;
#[cfg(test)]
mod stmt_common;
#[cfg(test)]
mod user_fn_ir;
#[cfg(test)]
mod value_model;
#[cfg(test)]
use array_access::*;
#[cfg(test)]
use builtin_intrinsics::*;
#[cfg(test)]
use call_helpers::*;
#[cfg(test)]
use contexts::*;
#[cfg(test)]
use def_lowering::*;
#[cfg(test)]
use expr_common::*;
#[cfg(test)]
use jit_utils::*;
#[cfg(test)]
use layout::*;
#[cfg(test)]
use llvm_helpers::*;
#[cfg(test)]
pub(crate) use mir_native::emit_optimized_mir_artifacts;
pub use mir_native::{
    lower_mir_and_jit, lower_mir_and_jit_with_options, lower_mir_to_llvm_ir,
    lower_mir_to_llvm_ir_with_options, lower_mir_to_object, lower_mir_to_object_artifact,
    lower_mir_to_target_llvm_ir, lower_optimized_mir_and_jit,
    lower_optimized_mir_and_jit_with_options, lower_optimized_mir_to_llvm_ir,
    lower_optimized_mir_to_llvm_ir_with_options, lower_optimized_mir_to_object,
    lower_optimized_mir_to_object_artifact, lower_optimized_mir_to_target_llvm_ir, MirCodegenError,
    MirCodegenErrorKind, MirCompileOptions, MirEventPayloadShape, MirJitProgram, MirTargetOptions,
};
#[cfg(test)]
use orc_expr_stmt::*;
#[cfg(test)]
use orc_locals::*;
#[cfg(test)]
use oversampling::*;
#[cfg(test)]
pub(crate) use pipeline::{compile_orc, emit_legacy_artifacts};
#[cfg(test)]
use pointer_helpers::*;
#[cfg(test)]
use proc_buffer_refs::*;
#[cfg(test)]
use proc_ir::*;
#[cfg(test)]
pub(crate) use process_handle::OrcProcess;
#[cfg(test)]
use process_handle::*;
#[cfg(test)]
use specialization::*;
#[cfg(test)]
use stmt_common::*;
#[cfg(test)]
use user_fn_ir::*;
#[cfg(test)]
use value_model::*;
