use std::sync::OnceLock;

#[cfg(test)]
use llvm_sys::core::*;
use llvm_sys::orc2::LLVMOrcThreadSafeContextRef;
use llvm_sys::prelude::*;
#[cfg(test)]
use llvm_sys::{LLVMFastMathAll, LLVMFastMathFlags, LLVMFastMathNone, LLVMRealPredicate};

#[cfg(test)]
use onda_frontend::{Diagnostic, PrimitiveType};

#[cfg(test)]
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

#[cfg(test)]
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
pub(super) static CODEGEN_TARGETS_INIT: OnceLock<()> = OnceLock::new();

extern "C" {
    #[link_name = "LLVMOrcCreateNewThreadSafeContextFromLLVMContext"]
    pub(super) fn llvm_orc_create_new_thread_safe_context_from_llvm_context(
        Ctx: LLVMContextRef,
    ) -> LLVMOrcThreadSafeContextRef;
}

#[cfg(test)]
pub(super) fn fast_math_flags(enabled: bool) -> LLVMFastMathFlags {
    if enabled {
        LLVMFastMathAll
    } else {
        LLVMFastMathNone
    }
}

#[cfg(test)]
pub(super) unsafe fn set_fast_math_flags(inst: LLVMValueRef, flags: LLVMFastMathFlags) {
    if flags != LLVMFastMathNone {
        LLVMSetFastMathFlags(inst, flags);
    }
}

#[cfg(test)]
pub(super) unsafe fn set_fast_math_for_primitive(
    inst: LLVMValueRef,
    ty: PrimitiveType,
    flags: LLVMFastMathFlags,
) {
    if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
        set_fast_math_flags(inst, flags);
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
