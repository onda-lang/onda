use super::*;

pub(super) fn build_local_slot(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    name: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    unsafe {
        let insert_bb = LLVMGetInsertBlock(builder);
        if insert_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get insertion block for local allocation",
            ));
        }
        let fn_ref = LLVMGetBasicBlockParent(insert_bb);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(
                "failed to get function for local allocation",
            ));
        }
        let entry_bb = LLVMGetEntryBasicBlock(fn_ref);
        if entry_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get entry block for local allocation",
            ));
        }
        let context = LLVMGetTypeContext(elem_ty);
        if context.is_null() {
            return Err(Diagnostic::internal(
                "failed to get LLVM context for local allocation",
            ));
        }
        let alloca_builder = LLVMCreateBuilderInContext(context);
        if alloca_builder.is_null() {
            return Err(Diagnostic::internal(
                "failed to create LLVM builder for local allocation",
            ));
        }
        let first_inst = LLVMGetFirstInstruction(entry_bb);
        if first_inst.is_null() {
            LLVMPositionBuilderAtEnd(alloca_builder, entry_bb);
        } else {
            LLVMPositionBuilderBefore(alloca_builder, first_inst);
        }
        let c_name =
            CString::new(name).map_err(|_| Diagnostic::internal("invalid local variable name"))?;
        let slot = LLVMBuildAlloca(alloca_builder, elem_ty, c_name.as_ptr());
        LLVMDisposeBuilder(alloca_builder);
        if slot.is_null() {
            return Err(Diagnostic::internal(
                "failed to allocate local variable slot",
            ));
        }
        Ok(slot)
    }
}

pub(super) fn build_local_array_slot(
    builder: LLVMBuilderRef,
    elem_ty: LLVMTypeRef,
    len: usize,
    name: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    if len == 0 {
        return Err(Diagnostic::internal(
            "local array declaration requires size greater than zero",
        ));
    }
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "local array declaration size {len} exceeds supported i32 range in ORC lowering"
        )));
    }

    unsafe {
        let insert_bb = LLVMGetInsertBlock(builder);
        if insert_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get insertion block for local array allocation",
            ));
        }
        let fn_ref = LLVMGetBasicBlockParent(insert_bb);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(
                "failed to get function for local array allocation",
            ));
        }
        let entry_bb = LLVMGetEntryBasicBlock(fn_ref);
        if entry_bb.is_null() {
            return Err(Diagnostic::internal(
                "failed to get entry block for local array allocation",
            ));
        }
        let context = LLVMGetTypeContext(elem_ty);
        if context.is_null() {
            return Err(Diagnostic::internal(
                "failed to get LLVM context for local array allocation",
            ));
        }
        let alloca_builder = LLVMCreateBuilderInContext(context);
        if alloca_builder.is_null() {
            return Err(Diagnostic::internal(
                "failed to create LLVM builder for local array allocation",
            ));
        }
        let first_inst = LLVMGetFirstInstruction(entry_bb);
        if first_inst.is_null() {
            LLVMPositionBuilderAtEnd(alloca_builder, entry_bb);
        } else {
            LLVMPositionBuilderBefore(alloca_builder, first_inst);
        }
        let c_name =
            CString::new(name).map_err(|_| Diagnostic::internal("invalid local array name"))?;
        let count = const_i32(LLVMInt32TypeInContext(context), len as i32);
        let slot = LLVMBuildArrayAlloca(alloca_builder, elem_ty, count, c_name.as_ptr());
        LLVMDisposeBuilder(alloca_builder);
        if slot.is_null() {
            return Err(Diagnostic::internal("failed to allocate local array slot"));
        }
        Ok(slot)
    }
}

pub(super) unsafe fn current_block_terminated(builder: LLVMBuilderRef) -> bool {
    let bb = LLVMGetInsertBlock(builder);
    if bb.is_null() {
        return false;
    }
    !LLVMGetBasicBlockTerminator(bb).is_null()
}
