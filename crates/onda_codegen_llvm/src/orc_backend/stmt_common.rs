use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn lower_if_stmt_common<TThen, TElse, FThen, FElse>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    cond_bool: LLVMValueRef,
    then_bb_name: &[u8],
    else_bb_name: &[u8],
    merge_bb_name: &[u8],
    mut lower_then: FThen,
    mut lower_else: FElse,
) -> Result<(TThen, TElse), Diagnostic>
where
    FThen: FnMut() -> Result<TThen, Diagnostic>,
    FElse: FnMut() -> Result<TElse, Diagnostic>,
{
    let then_bb = LLVMAppendBasicBlockInContext(context, fn_ref, then_bb_name.as_ptr().cast());
    let else_bb = LLVMAppendBasicBlockInContext(context, fn_ref, else_bb_name.as_ptr().cast());
    let merge_bb = LLVMAppendBasicBlockInContext(context, fn_ref, merge_bb_name.as_ptr().cast());

    LLVMBuildCondBr(builder, cond_bool, then_bb, else_bb);

    LLVMPositionBuilderAtEnd(builder, then_bb);
    let then_result = lower_then()?;
    if !current_block_terminated(builder) {
        LLVMBuildBr(builder, merge_bb);
    }

    LLVMPositionBuilderAtEnd(builder, else_bb);
    let else_result = lower_else()?;
    if !current_block_terminated(builder) {
        LLVMBuildBr(builder, merge_bb);
    }

    LLVMPositionBuilderAtEnd(builder, merge_bb);
    Ok((then_result, else_result))
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn lower_while_stmt_common<FCond, FBody>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    cond_bb_name: &[u8],
    body_bb_name: &[u8],
    end_bb_name: &[u8],
    mut lower_cond: FCond,
    mut lower_body: FBody,
) -> Result<(), Diagnostic>
where
    FCond: FnMut() -> Result<LLVMValueRef, Diagnostic>,
    FBody: FnMut(LLVMBasicBlockRef, LLVMBasicBlockRef) -> Result<(), Diagnostic>,
{
    let cond_bb = LLVMAppendBasicBlockInContext(context, fn_ref, cond_bb_name.as_ptr().cast());
    let body_bb = LLVMAppendBasicBlockInContext(context, fn_ref, body_bb_name.as_ptr().cast());
    let end_bb = LLVMAppendBasicBlockInContext(context, fn_ref, end_bb_name.as_ptr().cast());

    LLVMBuildBr(builder, cond_bb);

    LLVMPositionBuilderAtEnd(builder, cond_bb);
    let cond_bool = lower_cond()?;
    LLVMBuildCondBr(builder, cond_bool, body_bb, end_bb);

    LLVMPositionBuilderAtEnd(builder, body_bb);
    lower_body(cond_bb, end_bb)?;
    if !current_block_terminated(builder) {
        LLVMBuildBr(builder, cond_bb);
    }

    LLVMPositionBuilderAtEnd(builder, end_bb);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn lower_for_stmt_common<FBody>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    i32_ty: LLVMTypeRef,
    start_v: LLVMValueRef,
    end_v: LLVMValueRef,
    step_v: LLVMValueRef,
    end_inclusive: bool,
    cond_bb_name: &[u8],
    body_bb_name: &[u8],
    latch_bb_name: &[u8],
    end_bb_name: &[u8],
    diag_context: &str,
    mut lower_body: FBody,
) -> Result<(), Diagnostic>
where
    FBody: FnMut(LLVMValueRef, LLVMBasicBlockRef, LLVMBasicBlockRef) -> Result<(), Diagnostic>,
{
    let preheader_bb = LLVMGetInsertBlock(builder);
    if preheader_bb.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to get {diag_context} preheader block"
        )));
    }

    let cond_bb = LLVMAppendBasicBlockInContext(context, fn_ref, cond_bb_name.as_ptr().cast());
    let body_bb = LLVMAppendBasicBlockInContext(context, fn_ref, body_bb_name.as_ptr().cast());
    let latch_bb = LLVMAppendBasicBlockInContext(context, fn_ref, latch_bb_name.as_ptr().cast());
    let end_bb = LLVMAppendBasicBlockInContext(context, fn_ref, end_bb_name.as_ptr().cast());

    LLVMBuildBr(builder, cond_bb);

    LLVMPositionBuilderAtEnd(builder, cond_bb);
    let loop_i = LLVMBuildPhi(builder, i32_ty, b"for_i\0".as_ptr().cast());
    let mut incoming_vals = [start_v];
    let mut incoming_blocks = [preheader_bb];
    LLVMAddIncoming(
        loop_i,
        incoming_vals.as_mut_ptr(),
        incoming_blocks.as_mut_ptr(),
        1,
    );

    let cmp_pos = LLVMBuildICmp(
        builder,
        if end_inclusive {
            LLVMIntPredicate::LLVMIntSLE
        } else {
            LLVMIntPredicate::LLVMIntSLT
        },
        loop_i,
        end_v,
        b"for_cmp_pos\0".as_ptr().cast(),
    );
    let cmp_neg = LLVMBuildICmp(
        builder,
        if end_inclusive {
            LLVMIntPredicate::LLVMIntSGE
        } else {
            LLVMIntPredicate::LLVMIntSGT
        },
        loop_i,
        end_v,
        b"for_cmp_neg\0".as_ptr().cast(),
    );
    let step_pos = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        step_v,
        const_i32(i32_ty, 0),
        b"for_step_pos\0".as_ptr().cast(),
    );
    let step_neg = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSLT,
        step_v,
        const_i32(i32_ty, 0),
        b"for_step_neg\0".as_ptr().cast(),
    );
    let pos_cond = LLVMBuildAnd(
        builder,
        step_pos,
        cmp_pos,
        b"for_pos_cond\0".as_ptr().cast(),
    );
    let neg_cond = LLVMBuildAnd(
        builder,
        step_neg,
        cmp_neg,
        b"for_neg_cond\0".as_ptr().cast(),
    );
    let cond = LLVMBuildOr(builder, pos_cond, neg_cond, b"for_cond\0".as_ptr().cast());
    LLVMBuildCondBr(builder, cond, body_bb, end_bb);

    LLVMPositionBuilderAtEnd(builder, body_bb);
    lower_body(loop_i, latch_bb, end_bb)?;
    if !current_block_terminated(builder) {
        LLVMBuildBr(builder, latch_bb);
    }

    LLVMPositionBuilderAtEnd(builder, latch_bb);
    let latch_end_bb = LLVMGetInsertBlock(builder);
    if latch_end_bb.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to get {diag_context} latch block"
        )));
    }
    let next_i = LLVMBuildAdd(builder, loop_i, step_v, b"for_i_next\0".as_ptr().cast());
    LLVMBuildBr(builder, cond_bb);
    let mut back_vals = [next_i];
    let mut back_blocks = [latch_end_bb];
    LLVMAddIncoming(loop_i, back_vals.as_mut_ptr(), back_blocks.as_mut_ptr(), 1);

    LLVMPositionBuilderAtEnd(builder, end_bb);
    Ok(())
}

fn slice_llvm_name(prefix: &str, suffix: &str) -> Vec<u8> {
    format!("{prefix}_{suffix}\0").into_bytes()
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn lower_slice_copy_common<FCopy>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    i32_ty: LLVMTypeRef,
    dst_view: CodegenArrayView,
    src_view: CodegenArrayView,
    llvm_prefix: &str,
    backward_diag_context: &str,
    forward_diag_context: &str,
    copy_elem: FCopy,
) -> Result<(), Diagnostic>
where
    FCopy: Fn(LLVMValueRef) -> Result<(), Diagnostic> + Copy,
{
    let copy_len_cmp_name = slice_llvm_name(llvm_prefix, "copy_len_cmp");
    let copy_len_name = slice_llvm_name(llvm_prefix, "copy_len");
    let dst_i8_name = slice_llvm_name(llvm_prefix, "dst_i8");
    let src_i8_name = slice_llvm_name(llvm_prefix, "src_i8");
    let dst_addr_name = slice_llvm_name(llvm_prefix, "dst_addr");
    let src_addr_name = slice_llvm_name(llvm_prefix, "src_addr");
    let copy_len_ptr_name = slice_llvm_name(llvm_prefix, "copy_len_ptr");
    let copy_bytes_name = slice_llvm_name(llvm_prefix, "copy_bytes");
    let src_end_name = slice_llvm_name(llvm_prefix, "src_end");
    let dst_end_name = slice_llvm_name(llvm_prefix, "dst_end");
    let dst_after_src_name = slice_llvm_name(llvm_prefix, "dst_after_src");
    let dst_before_src_end_name = slice_llvm_name(llvm_prefix, "dst_before_src_end");
    let src_before_dst_end_name = slice_llvm_name(llvm_prefix, "src_before_dst_end");
    let overlaps_name = slice_llvm_name(llvm_prefix, "overlaps");
    let backward_copy_name = slice_llvm_name(llvm_prefix, "backward_copy");
    let copy_backward_name = slice_llvm_name(llvm_prefix, "copy_backward");
    let copy_forward_name = slice_llvm_name(llvm_prefix, "copy_forward");
    let copy_merge_name = slice_llvm_name(llvm_prefix, "copy_merge");
    let back_start_name = slice_llvm_name(llvm_prefix, "back_start");
    let back_cond_name = slice_llvm_name(llvm_prefix, "back_cond");
    let back_body_name = slice_llvm_name(llvm_prefix, "back_body");
    let back_latch_name = slice_llvm_name(llvm_prefix, "back_latch");
    let back_end_name = slice_llvm_name(llvm_prefix, "back_end");
    let fwd_cond_name = slice_llvm_name(llvm_prefix, "fwd_cond");
    let fwd_body_name = slice_llvm_name(llvm_prefix, "fwd_body");
    let fwd_latch_name = slice_llvm_name(llvm_prefix, "fwd_latch");
    let fwd_end_name = slice_llvm_name(llvm_prefix, "fwd_end");

    let copy_len = LLVMBuildSelect(
        builder,
        LLVMBuildICmp(
            builder,
            llvm_sys::LLVMIntPredicate::LLVMIntSLT,
            dst_view.len_val,
            src_view.len_val,
            copy_len_cmp_name.as_ptr().cast(),
        ),
        dst_view.len_val,
        src_view.len_val,
        copy_len_name.as_ptr().cast(),
    );
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let intptr_ty = LLVMInt64TypeInContext(context);
    let dst_i8 = LLVMBuildBitCast(
        builder,
        dst_view.base_ptr,
        i8_ptr_ty,
        dst_i8_name.as_ptr().cast(),
    );
    let src_i8 = LLVMBuildBitCast(
        builder,
        src_view.base_ptr,
        i8_ptr_ty,
        src_i8_name.as_ptr().cast(),
    );
    let dst_addr = LLVMBuildPtrToInt(builder, dst_i8, intptr_ty, dst_addr_name.as_ptr().cast());
    let src_addr = LLVMBuildPtrToInt(builder, src_i8, intptr_ty, src_addr_name.as_ptr().cast());
    let elem_bytes = LLVMConstInt(intptr_ty, primitive_type_bytes(dst_view.elem_ty) as u64, 0);
    let copy_len_ptr = LLVMBuildZExt(
        builder,
        copy_len,
        intptr_ty,
        copy_len_ptr_name.as_ptr().cast(),
    );
    let copy_bytes = LLVMBuildMul(
        builder,
        copy_len_ptr,
        elem_bytes,
        copy_bytes_name.as_ptr().cast(),
    );
    let src_end = LLVMBuildAdd(builder, src_addr, copy_bytes, src_end_name.as_ptr().cast());
    let dst_end = LLVMBuildAdd(builder, dst_addr, copy_bytes, dst_end_name.as_ptr().cast());
    let dst_after_src = LLVMBuildICmp(
        builder,
        llvm_sys::LLVMIntPredicate::LLVMIntUGT,
        dst_addr,
        src_addr,
        dst_after_src_name.as_ptr().cast(),
    );
    let dst_before_src_end = LLVMBuildICmp(
        builder,
        llvm_sys::LLVMIntPredicate::LLVMIntULT,
        dst_addr,
        src_end,
        dst_before_src_end_name.as_ptr().cast(),
    );
    let src_before_dst_end = LLVMBuildICmp(
        builder,
        llvm_sys::LLVMIntPredicate::LLVMIntULT,
        src_addr,
        dst_end,
        src_before_dst_end_name.as_ptr().cast(),
    );
    let overlaps = LLVMBuildAnd(
        builder,
        dst_before_src_end,
        src_before_dst_end,
        overlaps_name.as_ptr().cast(),
    );
    let backward_copy = LLVMBuildAnd(
        builder,
        dst_after_src,
        overlaps,
        backward_copy_name.as_ptr().cast(),
    );

    lower_if_stmt_common(
        builder,
        context,
        fn_ref,
        backward_copy,
        copy_backward_name.as_slice(),
        copy_forward_name.as_slice(),
        copy_merge_name.as_slice(),
        || unsafe {
            let start = LLVMBuildSub(
                builder,
                copy_len,
                const_i32(i32_ty, 1),
                back_start_name.as_ptr().cast(),
            );
            lower_for_stmt_common(
                builder,
                context,
                fn_ref,
                i32_ty,
                start,
                const_i32(i32_ty, 0),
                const_i32(i32_ty, -1),
                true,
                back_cond_name.as_slice(),
                back_body_name.as_slice(),
                back_latch_name.as_slice(),
                back_end_name.as_slice(),
                backward_diag_context,
                |loop_i, _, _| copy_elem(loop_i),
            )?;
            Ok(())
        },
        || unsafe {
            lower_for_stmt_common(
                builder,
                context,
                fn_ref,
                i32_ty,
                const_i32(i32_ty, 0),
                copy_len,
                const_i32(i32_ty, 1),
                false,
                fwd_cond_name.as_slice(),
                fwd_body_name.as_slice(),
                fwd_latch_name.as_slice(),
                fwd_end_name.as_slice(),
                forward_diag_context,
                |loop_i, _, _| copy_elem(loop_i),
            )?;
            Ok(())
        },
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn lower_slice_fill_common<FFill>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    i32_ty: LLVMTypeRef,
    dst_view: CodegenArrayView,
    llvm_prefix: &str,
    fill_diag_context: &str,
    fill_elem: FFill,
) -> Result<(), Diagnostic>
where
    FFill: Fn(LLVMValueRef) -> Result<(), Diagnostic> + Copy,
{
    let fill_cond_name = slice_llvm_name(llvm_prefix, "fill_cond");
    let fill_body_name = slice_llvm_name(llvm_prefix, "fill_body");
    let fill_latch_name = slice_llvm_name(llvm_prefix, "fill_latch");
    let fill_end_name = slice_llvm_name(llvm_prefix, "fill_end");

    lower_for_stmt_common(
        builder,
        context,
        fn_ref,
        i32_ty,
        const_i32(i32_ty, 0),
        dst_view.len_val,
        const_i32(i32_ty, 1),
        false,
        fill_cond_name.as_slice(),
        fill_body_name.as_slice(),
        fill_latch_name.as_slice(),
        fill_end_name.as_slice(),
        fill_diag_context,
        |loop_i, _, _| fill_elem(loop_i),
    )?;

    Ok(())
}
