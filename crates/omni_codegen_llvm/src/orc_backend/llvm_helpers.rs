use std::sync::OnceLock;

use llvm_sys::core::*;
use llvm_sys::orc2::LLVMOrcThreadSafeContextRef;
use llvm_sys::prelude::*;
use llvm_sys::{LLVMFastMathAll, LLVMFastMathFlags, LLVMFastMathNone, LLVMRealPredicate};

use omni_frontend::{Diagnostic, PrimitiveType};

pub(super) unsafe fn set_internal_alwaysinline(
    fn_ref: LLVMValueRef,
    context: LLVMContextRef,
) -> Result<(), Diagnostic> {
    LLVMSetLinkage(fn_ref, llvm_sys::LLVMLinkage::LLVMInternalLinkage);
    let alwaysinline_name = b"alwaysinline";
    let kind =
        LLVMGetEnumAttributeKindForName(alwaysinline_name.as_ptr().cast(), alwaysinline_name.len());
    if kind == 0 {
        return Err(Diagnostic::internal(
            "failed to resolve LLVM enum attribute kind for 'alwaysinline'",
        ));
    }
    let attr = LLVMCreateEnumAttribute(context, kind, 0);
    LLVMAddAttributeAtIndex(fn_ref, llvm_sys::LLVMAttributeFunctionIndex, attr);
    Ok(())
}

pub(super) unsafe fn add_enum_param_attribute(
    fn_ref: LLVMValueRef,
    context: LLVMContextRef,
    param_index_1_based: u32,
    name: &[u8],
) -> Result<(), Diagnostic> {
    let kind = LLVMGetEnumAttributeKindForName(name.as_ptr().cast(), name.len());
    if kind == 0 {
        return Err(Diagnostic::internal(format!(
            "failed to resolve LLVM enum attribute kind for '{}'",
            String::from_utf8_lossy(name)
        )));
    }
    let attr = LLVMCreateEnumAttribute(context, kind, 0);
    LLVMAddAttributeAtIndex(fn_ref, param_index_1_based, attr);
    Ok(())
}

pub(super) static NATIVE_INIT_ERR: OnceLock<Option<String>> = OnceLock::new();

extern "C" {
    pub fn LLVMOrcCreateNewThreadSafeContextFromLLVMContext(
        Ctx: LLVMContextRef,
    ) -> LLVMOrcThreadSafeContextRef;
}

pub(super) fn fast_math_flags(enabled: bool) -> LLVMFastMathFlags {
    if enabled {
        LLVMFastMathAll
    } else {
        LLVMFastMathNone
    }
}

pub(super) unsafe fn set_fast_math_flags(inst: LLVMValueRef, flags: LLVMFastMathFlags) {
    if flags != LLVMFastMathNone {
        LLVMSetFastMathFlags(inst, flags);
    }
}

pub(super) unsafe fn set_fast_math_for_primitive(
    inst: LLVMValueRef,
    ty: PrimitiveType,
    flags: LLVMFastMathFlags,
) {
    if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
        set_fast_math_flags(inst, flags);
    }
}

pub(super) unsafe fn build_fadd_fast(
    builder: LLVMBuilderRef,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFAdd(builder, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}

pub(super) unsafe fn build_fsub_fast(
    builder: LLVMBuilderRef,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFSub(builder, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}

pub(super) unsafe fn build_fmul_fast(
    builder: LLVMBuilderRef,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFMul(builder, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}

pub(super) unsafe fn build_fdiv_fast(
    builder: LLVMBuilderRef,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFDiv(builder, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}

pub(super) unsafe fn build_frem_fast(
    builder: LLVMBuilderRef,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFRem(builder, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}

pub(super) unsafe fn build_fcmp_fast(
    builder: LLVMBuilderRef,
    pred: LLVMRealPredicate,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    name: &[u8],
    flags: LLVMFastMathFlags,
) -> LLVMValueRef {
    let inst = LLVMBuildFCmp(builder, pred, lhs, rhs, name.as_ptr().cast());
    set_fast_math_flags(inst, flags);
    inst
}
