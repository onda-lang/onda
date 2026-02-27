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
