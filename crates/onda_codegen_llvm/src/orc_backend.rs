use std::ffi::{CStr, CString};
use std::ptr::null_mut;

use llvm_sys::core::*;
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::orc2::lljit::*;
use llvm_sys::orc2::*;
use llvm_sys::prelude::*;
use llvm_sys::target::{LLVMCopyStringRepOfTargetData, LLVMDisposeTargetData};
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
use onda_frontend::Diagnostic;
mod jit_utils;
mod llvm_helpers;
mod mir_native;
pub use mir_native::{
    lower_mir_and_jit, lower_mir_and_jit_with_options, lower_mir_to_llvm_ir,
    lower_mir_to_llvm_ir_with_options, lower_mir_to_object, lower_mir_to_object_artifact,
    lower_mir_to_target_llvm_ir, lower_optimized_mir_and_jit,
    lower_optimized_mir_and_jit_with_options, lower_optimized_mir_to_llvm_ir,
    lower_optimized_mir_to_llvm_ir_with_options, lower_optimized_mir_to_object,
    lower_optimized_mir_to_object_artifact, lower_optimized_mir_to_target_llvm_ir, MirCodegenError,
    MirCodegenErrorKind, MirCompileOptions, MirEventPayloadShape, MirJitProgram, MirTargetOptions,
};
