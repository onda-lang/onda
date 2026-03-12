use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::mem::{align_of, size_of};
use std::ptr::null_mut;

use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::orc2::lljit::*;
use llvm_sys::orc2::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{
    LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget,
};
use llvm_sys::target_machine::{
    LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine, LLVMDisposeTargetMachine,
    LLVMGetDefaultTargetTriple, LLVMGetHostCPUFeatures, LLVMGetHostCPUName,
    LLVMGetTargetFromTriple, LLVMRelocMode, LLVMTargetMachineRef, LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use llvm_sys::{LLVMFastMathFlags, LLVMFastMathNone, LLVMIntPredicate, LLVMRealPredicate};
use omni_frontend::{
    AssignTarget, BinaryOp, BuiltinFn, CallArg, CallTypeArg, CmpOp, Diagnostic, Expr, LogicalOp,
    PrimitiveType, Stmt,
};
use omni_semantics::{
    internal_names::{PROC_INDEX_BASE_ARG, PROC_INDEX_BUFFER_SELECT_SENTINEL, PROC_INDEX_EXPR_ARG},
    ProcSincStageStateFields, ProcStepOversampleMeta, TypedArrayInfo, TypedBufferChannels,
    TypedEventParamType, TypedFieldType, TypedFnParam, TypedFunction, TypedProgram,
    TypedStructField,
};

use crate::primitives::primitive_type_bytes;

mod array_access;
mod builtin_intrinsics;
mod call_helpers;
mod contexts;
mod def_lowering;
mod expr_common;
mod jit_utils;
mod layout;
mod llvm_helpers;
mod orc_expr_stmt;
mod orc_locals;
mod oversampling;
mod pipeline;
mod pointer_helpers;
mod proc_buffer_refs;
mod proc_ir;
mod process_handle;
mod specialization;
mod stmt_common;
mod user_fn_ir;
mod value_model;
use array_access::*;
use builtin_intrinsics::*;
use call_helpers::*;
use contexts::*;
use def_lowering::*;
use expr_common::*;
use jit_utils::*;
use layout::*;
use llvm_helpers::*;
use orc_expr_stmt::*;
use orc_locals::*;
use oversampling::*;
pub(crate) use pipeline::{compile_orc, emit_optimized_ir};
use pointer_helpers::*;
use proc_buffer_refs::*;
use proc_ir::*;
pub(crate) use process_handle::OrcProcess;
use process_handle::*;
use specialization::*;
use stmt_common::*;
use user_fn_ir::*;
use value_model::*;
