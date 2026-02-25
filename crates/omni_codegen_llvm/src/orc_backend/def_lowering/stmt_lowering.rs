use super::*;

pub(super) unsafe fn lower_def_stmt(
    stmt: &Stmt,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl,
            expr,
            ..
        } => lower_def_assign_stmt(target, *decl_ty, *is_typed_decl, expr, ctx),
        Stmt::Expr { expr, .. } => {
            let _ = lower_def_expr(expr, ctx)?;
            Ok(false)
        }
        Stmt::Return { expr, .. } => {
            let value = lower_def_expr(expr, ctx)?;
            let ret_v = cast_def_value_to(ctx, value, ctx.return_ty, b"def_ret_cast\0");
            LLVMBuildStore(ctx.builder, ret_v, ctx.return_slot);
            LLVMBuildBr(ctx.builder, ctx.return_block);
            Ok(true)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => lower_def_if_stmt(cond, then_branch, else_branch, ctx),
        Stmt::For {
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            ..
        } => lower_def_for_stmt(var, step.as_ref(), start, end, *end_inclusive, body, ctx),
        Stmt::While { cond, body, .. } => lower_def_while_stmt(cond, body, ctx),
        Stmt::Break { .. } => {
            let Some(loop_control) = ctx.loop_stack.last().copied() else {
                return Err(Diagnostic::internal(
                    "break statement encountered outside of loop in def lowering",
                ));
            };
            LLVMBuildBr(ctx.builder, loop_control.break_bb);
            Ok(false)
        }
        Stmt::Continue { .. } => {
            let Some(loop_control) = ctx.loop_stack.last().copied() else {
                return Err(Diagnostic::internal(
                    "continue statement encountered outside of loop in def lowering",
                ));
            };
            LLVMBuildBr(ctx.builder, loop_control.continue_bb);
            Ok(false)
        }
    }
}

unsafe fn lower_def_assign_stmt(
    target: &AssignTarget,
    decl_ty: Option<PrimitiveType>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    match target {
        AssignTarget::Var(target_name) => {
            lower_def_var_assign(target_name, decl_ty, is_typed_decl, expr, ctx)
        }
        AssignTarget::Index { base, index } => lower_def_index_assign(base, index, expr, ctx),
    }
}

unsafe fn lower_def_var_assign(
    target_name: &str,
    decl_ty: Option<PrimitiveType>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    if try_lower_def_typed_array_decl(target_name, decl_ty, is_typed_decl, expr, ctx)? {
        return Ok(false);
    }

    if !ctx.local_slots.contains_key(target_name)
        && !ctx.local_data_aliases.contains_key(target_name)
        && !ctx.data_ptrs.contains_key(target_name)
        && try_bind_struct_data_alias_in_def(target_name, expr, ctx)?
    {
        return Ok(false);
    }

    let typed_value = lower_def_expr(expr, ctx)?;
    if let Some(local) = ctx.local_slots.get(target_name).copied() {
        let casted = cast_def_value_to(ctx, typed_value, local.ty, b"def_store_cast\0");
        LLVMBuildStore(ctx.builder, casted, local.ptr);
        return Ok(false);
    }
    if ctx.data_ptrs.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "Data symbol '{target_name}' must be assigned via index syntax in def lowering"
        )));
    }
    if ctx.local_data_aliases.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "Data alias '{target_name}' must be assigned via index syntax in def lowering"
        )));
    }
    let target_ty = decl_ty.unwrap_or(typed_value.ty);
    let slot = build_local_slot(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, target_ty),
        &format!("v_{target_name}"),
    )?;
    let casted = cast_def_value_to(ctx, typed_value, target_ty, b"def_store_new_cast\0");
    LLVMBuildStore(ctx.builder, casted, slot);
    ctx.local_slots.insert(
        target_name.to_owned(),
        DefLocalSlot {
            ptr: slot,
            ty: target_ty,
        },
    );
    Ok(false)
}

unsafe fn try_lower_def_typed_array_decl(
    target_name: &str,
    decl_ty: Option<PrimitiveType>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let Expr::DataCtor { spec, init } = expr else {
        return Ok(false);
    };

    if !is_typed_decl {
        return Err(Diagnostic::internal(
            "Data constructor assignment in def lowering requires typed array declaration syntax",
        ));
    }
    if decl_ty.is_some() {
        return Err(Diagnostic::internal(
            "typed array declaration cannot include scalar declaration type in def lowering",
        ));
    }
    if ctx.local_slots.contains_key(target_name) || ctx.data_ptrs.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "typed array declaration for '{target_name}' conflicts with existing local symbol in def lowering"
        )));
    }

    let elem_ty = match &spec.elem {
        omni_frontend::DataElemType::Primitive(elem_ty) => *elem_ty,
        omni_frontend::DataElemType::Struct(name) => {
            return Err(Diagnostic::internal(format!(
                "typed array declaration '{target_name}: {name}[N]' is not yet supported in def lowering"
            )))
        }
    };
    let len = eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
    let ptr = build_local_array_slot(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, elem_ty),
        len,
        &format!("d_{target_name}"),
    )?;
    if let Some(values) = init {
        if values.len() != len {
            return Err(Diagnostic::internal(format!(
                "typed array declaration '{target_name}' initializer expects {len} elements, got {}",
                values.len()
            )));
        }
        for (idx, value_expr) in values.iter().enumerate() {
            let typed = lower_def_expr(value_expr, ctx)?;
            let casted = cast_def_value_to(ctx, typed, elem_ty, b"def_local_arr_init_cast\0");
            let idx_v = LLVMConstInt(ctx.i32_ty, idx as u64, 0);
            let elem_ptr = build_f32_ptr_offset(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, elem_ty),
                ptr,
                idx_v,
                b"def_local_arr_init_ptr\0",
            );
            LLVMBuildStore(ctx.builder, casted, elem_ptr);
        }
    }
    ctx.data_ptrs.insert(target_name.to_owned(), ptr);
    ctx.data_len.insert(target_name.to_owned(), len);
    ctx.data_elem_ty.insert(target_name.to_owned(), elem_ty);
    Ok(true)
}

unsafe fn try_bind_struct_data_alias_in_def(
    target_name: &str,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let Expr::Index { base, index } = expr else {
        return Ok(false);
    };

    if let Some(struct_name) = ctx.data_struct_roots.get(base).cloned() {
        let root_len = *ctx.data_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data[Struct] length metadata for '{base}' in def lowering"
            ))
        })?;
        let raw_index = lower_def_expr(index, ctx)?;
        let index_i32 = cast_def_value_to(
            ctx,
            raw_index,
            PrimitiveType::I32,
            b"def_data_alias_idx_i32\0",
        );
        let clamped = clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, root_len)?;
        bind_struct_data_element_aliases_in_def(target_name, &struct_name, base, clamped, ctx)?;
        return Ok(true);
    }

    if let Some(alias) = ctx.local_data_aliases.get(base).cloned() {
        match alias {
            LocalDataAlias::Primitive { .. } => {
                return Err(Diagnostic::internal(format!(
                    "local alias binding '{target_name} = {base}[...]' is not supported for primitive arrays in def lowering; use direct indexed access"
                )));
            }
            LocalDataAlias::Struct {
                root_base,
                elem_struct,
                len,
                start_index,
            } => {
                let raw_index = lower_def_expr(index, ctx)?;
                let index_i32 = cast_def_value_to(
                    ctx,
                    raw_index,
                    PrimitiveType::I32,
                    b"def_data_alias_local_idx_i32\0",
                );
                let clamped_local = clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, len)?;
                let global_index = LLVMBuildAdd(
                    ctx.builder,
                    start_index,
                    clamped_local,
                    b"def_data_alias_global_idx\0".as_ptr().cast(),
                );
                bind_struct_data_element_aliases_in_def(
                    target_name,
                    &elem_struct,
                    &root_base,
                    global_index,
                    ctx,
                )?;
                return Ok(true);
            }
        }
    }

    if ctx.data_ptrs.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "local alias binding '{target_name} = {base}[...]' is not supported for primitive arrays in def lowering; use direct indexed access"
        )));
    }

    Ok(false)
}

unsafe fn lower_def_index_assign(
    base: &str,
    index: &Expr,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let typed_value = lower_def_expr(expr, ctx)?;
    let data = lower_def_data_element_ptr(ctx, base, index, true)?;
    let value = cast_def_value_to(ctx, typed_value, data.elem_ty, b"def_data_store_cast\0");
    LLVMBuildStore(ctx.builder, value, data.ptr);
    Ok(false)
}

unsafe fn lower_def_if_stmt(
    cond: &Expr,
    then_branch: &[Stmt],
    else_branch: &[Stmt],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let cond_value = lower_def_expr(cond, ctx)?;
    let cond_bool = lower_def_condition(ctx, cond_value, b"def_if_cond\0");

    let then_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_if_then\0".as_ptr().cast());
    let else_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_if_else\0".as_ptr().cast());
    let merge_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_if_merge\0".as_ptr().cast());

    LLVMBuildCondBr(ctx.builder, cond_bool, then_bb, else_bb);

    LLVMPositionBuilderAtEnd(ctx.builder, then_bb);
    let mut then_terminated = false;
    for nested in then_branch {
        if lower_def_stmt(nested, ctx)? {
            then_terminated = true;
            break;
        }
        if current_block_terminated(ctx.builder) {
            break;
        }
    }
    if !current_block_terminated(ctx.builder) {
        LLVMBuildBr(ctx.builder, merge_bb);
    }

    LLVMPositionBuilderAtEnd(ctx.builder, else_bb);
    let mut else_terminated = false;
    for nested in else_branch {
        if lower_def_stmt(nested, ctx)? {
            else_terminated = true;
            break;
        }
        if current_block_terminated(ctx.builder) {
            break;
        }
    }
    if !current_block_terminated(ctx.builder) {
        LLVMBuildBr(ctx.builder, merge_bb);
    }

    LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
    if then_terminated && else_terminated {
        LLVMBuildBr(ctx.builder, ctx.return_block);
        return Ok(true);
    }
    Ok(false)
}

unsafe fn lower_def_for_stmt(
    var: &str,
    step: Option<&Expr>,
    start: &Expr,
    end: &Expr,
    end_inclusive: bool,
    body: &[Stmt],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let preheader_bb = LLVMGetInsertBlock(ctx.builder);
    if preheader_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get def for-loop preheader block",
        ));
    }

    let cond_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_for_cond\0".as_ptr().cast());
    let body_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_for_body\0".as_ptr().cast());
    let latch_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_for_latch\0".as_ptr().cast());
    let end_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_for_end\0".as_ptr().cast());

    let start_value = lower_def_expr(start, ctx)?;
    let start_v = cast_def_value_to(ctx, start_value, PrimitiveType::I32, b"def_for_start_i32\0");
    let end_value = lower_def_expr(end, ctx)?;
    let end_v = cast_def_value_to(ctx, end_value, PrimitiveType::I32, b"def_for_end_i32\0");
    let step_v = if let Some(step_expr) = step {
        let step_value = lower_def_expr(step_expr, ctx)?;
        cast_def_value_to(ctx, step_value, PrimitiveType::I32, b"def_for_step_i32\0")
    } else {
        const_i32(ctx.i32_ty, 1)
    };

    LLVMBuildBr(ctx.builder, cond_bb);

    LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
    let loop_i = LLVMBuildPhi(ctx.builder, ctx.i32_ty, b"def_for_i\0".as_ptr().cast());
    let mut incoming_vals = [start_v];
    let mut incoming_blocks = [preheader_bb];
    LLVMAddIncoming(
        loop_i,
        incoming_vals.as_mut_ptr(),
        incoming_blocks.as_mut_ptr(),
        1,
    );

    let cmp_pos = LLVMBuildICmp(
        ctx.builder,
        if end_inclusive {
            LLVMIntPredicate::LLVMIntSLE
        } else {
            LLVMIntPredicate::LLVMIntSLT
        },
        loop_i,
        end_v,
        b"def_for_cmp_pos\0".as_ptr().cast(),
    );
    let cmp_neg = LLVMBuildICmp(
        ctx.builder,
        if end_inclusive {
            LLVMIntPredicate::LLVMIntSGE
        } else {
            LLVMIntPredicate::LLVMIntSGT
        },
        loop_i,
        end_v,
        b"def_for_cmp_neg\0".as_ptr().cast(),
    );
    let step_pos = LLVMBuildICmp(
        ctx.builder,
        LLVMIntPredicate::LLVMIntSGT,
        step_v,
        const_i32(ctx.i32_ty, 0),
        b"def_for_step_pos\0".as_ptr().cast(),
    );
    let step_neg = LLVMBuildICmp(
        ctx.builder,
        LLVMIntPredicate::LLVMIntSLT,
        step_v,
        const_i32(ctx.i32_ty, 0),
        b"def_for_step_neg\0".as_ptr().cast(),
    );
    let pos_cond = LLVMBuildAnd(
        ctx.builder,
        step_pos,
        cmp_pos,
        b"def_for_pos_cond\0".as_ptr().cast(),
    );
    let neg_cond = LLVMBuildAnd(
        ctx.builder,
        step_neg,
        cmp_neg,
        b"def_for_neg_cond\0".as_ptr().cast(),
    );
    let cond = LLVMBuildOr(
        ctx.builder,
        pos_cond,
        neg_cond,
        b"def_for_cond\0".as_ptr().cast(),
    );
    LLVMBuildCondBr(ctx.builder, cond, body_bb, end_bb);

    LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
    let old_binding = ctx.local_slots.get(var).copied();
    let loop_slot = build_local_slot(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, PrimitiveType::I32),
        &format!("loop_{var}"),
    )?;
    ctx.local_slots.insert(
        var.to_owned(),
        DefLocalSlot {
            ptr: loop_slot,
            ty: PrimitiveType::I32,
        },
    );
    LLVMBuildStore(ctx.builder, loop_i, loop_slot);

    ctx.loop_stack.push(LoopControl {
        break_bb: end_bb,
        continue_bb: latch_bb,
    });
    for nested in body {
        let _ = lower_def_stmt(nested, ctx)?;
        if current_block_terminated(ctx.builder) {
            break;
        }
    }
    let _ = ctx.loop_stack.pop();

    if let Some(binding) = old_binding {
        ctx.local_slots.insert(var.to_owned(), binding);
    } else {
        ctx.local_slots.remove(var);
    }

    if !current_block_terminated(ctx.builder) {
        LLVMBuildBr(ctx.builder, latch_bb);
    }

    LLVMPositionBuilderAtEnd(ctx.builder, latch_bb);
    let latch_end_bb = LLVMGetInsertBlock(ctx.builder);
    if latch_end_bb.is_null() {
        return Err(Diagnostic::internal(
            "failed to get def for-loop latch block",
        ));
    }
    let next_i = LLVMBuildAdd(
        ctx.builder,
        loop_i,
        step_v,
        b"def_for_i_next\0".as_ptr().cast(),
    );
    LLVMBuildBr(ctx.builder, cond_bb);
    let mut back_vals = [next_i];
    let mut back_blocks = [latch_end_bb];
    LLVMAddIncoming(loop_i, back_vals.as_mut_ptr(), back_blocks.as_mut_ptr(), 1);

    LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
    Ok(false)
}

unsafe fn lower_def_while_stmt(
    cond: &Expr,
    body: &[Stmt],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let cond_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_while_cond\0".as_ptr().cast());
    let body_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_while_body\0".as_ptr().cast());
    let end_bb =
        LLVMAppendBasicBlockInContext(ctx.context, ctx.fn_ref, b"def_while_end\0".as_ptr().cast());

    LLVMBuildBr(ctx.builder, cond_bb);

    LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
    let cond_value = lower_def_expr(cond, ctx)?;
    let cond_bool = lower_def_condition(ctx, cond_value, b"def_while_cond_bool\0");
    LLVMBuildCondBr(ctx.builder, cond_bool, body_bb, end_bb);

    LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
    ctx.loop_stack.push(LoopControl {
        break_bb: end_bb,
        continue_bb: cond_bb,
    });
    for nested in body {
        let _ = lower_def_stmt(nested, ctx)?;
        if current_block_terminated(ctx.builder) {
            break;
        }
    }
    let _ = ctx.loop_stack.pop();

    if !current_block_terminated(ctx.builder) {
        LLVMBuildBr(ctx.builder, cond_bb);
    }

    LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
    Ok(false)
}
