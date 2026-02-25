use super::*;

pub(super) unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_data_aliases: &mut HashMap<String, LocalDataAlias>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Assign {
            target,
            decl_ty,
            is_typed_decl,
            expr,
            ..
        } => match target {
            AssignTarget::Var(name) => {
                if ctx.allow_struct_ctor {
                    if let Expr::UserCall {
                        name: struct_name,
                        args,
                        ..
                    } = expr
                    {
                        if let Some(fields) = ctx.struct_fields.get(struct_name).cloned() {
                            let scalar_param_names = fields
                                .iter()
                                .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                .map(|f| f.name.clone())
                                .collect::<Vec<_>>();
                            let scalar_defaults = fields
                                .iter()
                                .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                .map(|f| Some(f.default.clone().unwrap_or(Expr::Number(0.0))))
                                .collect::<Vec<_>>();
                            let resolved_scalar_args = resolve_call_args_codegen(
                                args,
                                &scalar_param_names,
                                &scalar_defaults,
                                false,
                                &format!("struct constructor '{struct_name}' in ORC init lowering"),
                            )?;
                            let mut scalar_arg_idx = 0usize;
                            for field in &fields {
                                let flat_target = format!("{name}.{}", field.name);
                                match field.ty {
                                    TypedFieldType::Scalar(_) => {
                                        let slot = ctx.state_slots.get(&flat_target).ok_or_else(
                                            || {
                                                Diagnostic::internal(format!(
                                                    "missing state slot for struct field '{flat_target}' in ORC lowering"
                                                ))
                                            },
                                        )?;
                                        let resolved_arg = resolved_scalar_args
                                            .get(scalar_arg_idx)
                                            .copied()
                                            .flatten();
                                        let value_typed = if let Some(arg_expr) = resolved_arg {
                                            let typed = lower_expr(
                                                arg_expr,
                                                ctx,
                                                locals,
                                                local_aliases,
                                                local_data_aliases,
                                            )?;
                                            cast_orc_value_to(ctx, typed, slot.ty, b"ctor_arg\0")
                                        } else {
                                            let default_expr = scalar_defaults
                                                .get(scalar_arg_idx)
                                                .and_then(|d| d.as_ref())
                                                .ok_or_else(|| {
                                                    Diagnostic::internal(format!(
                                                        "struct constructor '{struct_name}' missing default for field '{}'",
                                                        field.name
                                                    ))
                                                })?;
                                            let default_value = eval_const_default_expr(
                                                default_expr,
                                                ctx.sample_rate,
                                                ctx.block_size,
                                            )?;
                                            match slot.ty {
                                                PrimitiveType::F32 => LLVMConstReal(
                                                    ctx.float_ty,
                                                    default_value as f64,
                                                ),
                                                PrimitiveType::F64 => LLVMConstReal(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::F64,
                                                    ),
                                                    default_value as f64,
                                                ),
                                                PrimitiveType::I32 => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::I32,
                                                    ),
                                                    (default_value as i32) as u64,
                                                    1,
                                                ),
                                                PrimitiveType::I64 => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::I64,
                                                    ),
                                                    (default_value as i64) as u64,
                                                    1,
                                                ),
                                                PrimitiveType::Bool => LLVMConstInt(
                                                    llvm_ty_for_primitive(
                                                        ctx.context,
                                                        PrimitiveType::Bool,
                                                    ),
                                                    if default_value != 0.0 { 1 } else { 0 },
                                                    0,
                                                ),
                                            }
                                        };
                                        scalar_arg_idx += 1;
                                        LLVMBuildStore(ctx.builder, value_typed, slot.ptr);
                                    }
                                    TypedFieldType::Data(_) => {
                                        if !ctx.data_base_ptrs.contains_key(&flat_target)
                                            && !ctx.data_struct_len.contains_key(&flat_target)
                                        {
                                            return Err(Diagnostic::internal(format!(
                                                "missing Data symbol '{flat_target}' in ORC lowering"
                                            )));
                                        }
                                    }
                                }
                            }
                            if scalar_arg_idx
                                != fields
                                    .iter()
                                    .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
                                    .count()
                            {
                                return Err(Diagnostic::internal(format!(
                                    "struct constructor '{struct_name}' scalar field mapping mismatch"
                                )));
                            }
                            return Ok(());
                        }
                    }
                }

                if let Expr::DataCtor { spec, init } = expr {
                    let expected_len = if let Some(len) = ctx.data_len.get(name) {
                        *len
                    } else if let Some(len) = ctx.data_struct_len.get(name) {
                        *len
                    } else if *is_typed_decl {
                        if local_aliases.contains_key(name) || local_data_aliases.contains_key(name)
                        {
                            return Err(Diagnostic::internal(format!(
                                "typed array declaration for '{name}' conflicts with existing local symbol in ORC lowering"
                            )));
                        }
                        if locals.contains_key(name)
                            || ctx.out_slots.contains_key(name)
                            || ctx.state_slots.contains_key(name)
                            || ctx.param_byte_offset.contains_key(name)
                            || ctx.input_index.contains_key(name)
                            || ctx.buffer_index.contains_key(name)
                        {
                            return Err(Diagnostic::internal(format!(
                                "typed array declaration for '{name}' conflicts with existing symbol in ORC lowering"
                            )));
                        }
                        let elem_ty = match spec.elem {
                            omni_frontend::DataElemType::Primitive(elem_ty) => elem_ty,
                            omni_frontend::DataElemType::Struct(ref struct_name) => {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{name}: {struct_name}[N]' is not yet supported in ORC lowering"
                                )))
                            }
                        };
                        let len =
                            eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                        let ptr = build_local_array_slot(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, elem_ty),
                            len,
                            &format!("d_{name}"),
                        )?;
                        if let Some(values) = init {
                            if values.len() != len {
                                return Err(Diagnostic::internal(format!(
                                    "typed array declaration '{name}' initializer expects {len} elements, got {}",
                                    values.len()
                                )));
                            }
                            for (idx, value_expr) in values.iter().enumerate() {
                                let typed = lower_expr(
                                    value_expr,
                                    ctx,
                                    locals,
                                    local_aliases,
                                    local_data_aliases,
                                )?;
                                let casted = cast_orc_value_to(
                                    ctx,
                                    typed,
                                    elem_ty,
                                    b"local_arr_init_cast\0",
                                );
                                let idx_v = LLVMConstInt(ctx.i32_ty, idx as u64, 0);
                                let elem_ptr = build_f32_ptr_offset(
                                    ctx.builder,
                                    llvm_ty_for_primitive(ctx.context, elem_ty),
                                    ptr,
                                    idx_v,
                                    b"local_arr_init_ptr\0",
                                );
                                LLVMBuildStore(ctx.builder, casted, elem_ptr);
                            }
                        }
                        local_data_aliases.insert(
                            name.clone(),
                            LocalDataAlias::Primitive {
                                base_ptr: ptr,
                                len,
                                elem_ty,
                            },
                        );
                        return Ok(());
                    } else {
                        return Err(Diagnostic::internal(format!(
                            "Data constructor assigned to non-Data symbol '{name}'"
                        )));
                    };
                    let actual_len =
                        eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                    if expected_len != actual_len {
                        return Err(Diagnostic::internal(format!(
                            "Data symbol '{name}' expected Data[{expected_len}] but got Data[{actual_len}]"
                        )));
                    }
                    return Ok(());
                }

                if let Some(alias) = local_aliases.get(name) {
                    let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                    let value = cast_orc_value_to(ctx, typed, alias.ty, b"alias_store_cast\0");
                    LLVMBuildStore(ctx.builder, value, alias.ptr);
                    return Ok(());
                }
                if local_data_aliases.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "Data alias '{name}' must be assigned via index syntax"
                    )));
                }
                if ctx.input_arrays.contains_key(name)
                    || ctx.param_arrays.contains_key(name)
                    || ctx.output_arrays.contains_key(name)
                {
                    return Err(Diagnostic::internal(format!(
                        "top-level array symbol '{name}' must be assigned via index syntax in ORC lowering"
                    )));
                }
                if ctx.buffer_index.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "buffer symbol '{name}' must be assigned via index syntax in ORC lowering"
                    )));
                }

                if !locals.contains_key(name)
                    && !local_aliases.contains_key(name)
                    && !local_data_aliases.contains_key(name)
                    && !ctx.out_slots.contains_key(name)
                    && !ctx.state_slots.contains_key(name)
                    && !ctx.param_byte_offset.contains_key(name)
                    && !ctx.input_index.contains_key(name)
                    && !ctx.data_base_ptrs.contains_key(name)
                    && !ctx.data_struct_len.contains_key(name)
                    && !ctx.buffer_index.contains_key(name)
                {
                    if let Expr::Index { base, index } = expr {
                        if let Some(struct_name) = ctx.data_struct_roots.get(base).cloned() {
                            let root_len = *ctx.data_struct_len.get(base).ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing Data[Struct] length metadata for '{base}'"
                                ))
                            })?;
                            let root_index = lower_clamped_data_index(
                                ctx,
                                index,
                                root_len,
                                locals,
                                local_aliases,
                                local_data_aliases,
                            )?;
                            bind_struct_data_element_aliases(
                                name,
                                &struct_name,
                                base,
                                root_index,
                                ctx,
                                local_aliases,
                                local_data_aliases,
                            )?;
                            return Ok(());
                        }

                        if let Some(alias) = local_data_aliases.get(base).cloned() {
                            match alias {
                                LocalDataAlias::Primitive { .. } => {
                                    return Err(Diagnostic::internal(format!(
                                        "local alias binding '{name} = {base}[...]' is not supported for primitive arrays in ORC lowering; use direct indexed access"
                                    )));
                                }
                                LocalDataAlias::Struct {
                                    root_base,
                                    elem_struct,
                                    len,
                                    start_index,
                                } => {
                                    let local_idx = lower_clamped_data_index(
                                        ctx,
                                        index,
                                        len,
                                        locals,
                                        local_aliases,
                                        local_data_aliases,
                                    )?;
                                    let global_idx = LLVMBuildAdd(
                                        ctx.builder,
                                        start_index,
                                        local_idx,
                                        b"data_alias_global_idx\0".as_ptr().cast(),
                                    );
                                    bind_struct_data_element_aliases(
                                        name,
                                        &elem_struct,
                                        &root_base,
                                        global_idx,
                                        ctx,
                                        local_aliases,
                                        local_data_aliases,
                                    )?;
                                }
                            }
                            return Ok(());
                        }

                        if ctx.data_base_ptrs.contains_key(base) {
                            return Err(Diagnostic::internal(format!(
                                "local alias binding '{name} = {base}[...]' is not supported for primitive arrays in ORC lowering; use direct indexed access"
                            )));
                        }
                    }
                }

                let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                if let Some(slot) = ctx.out_slots.get(name) {
                    let casted = cast_orc_value_to(ctx, typed, slot.ty, b"out_store_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                    return Ok(());
                }
                if let Some(slot) = ctx.state_slots.get(name) {
                    let casted = cast_orc_value_to(ctx, typed, slot.ty, b"state_store_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                    return Ok(());
                }
                if ctx.data_base_ptrs.contains_key(name) || ctx.data_struct_len.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "Data symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if ctx.buffer_index.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "buffer symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if !locals.contains_key(name)
                    && !local_data_aliases.contains_key(name)
                    && !ctx.input_index.contains_key(name)
                    && !ctx.param_byte_offset.contains_key(name)
                    && !ctx.input_arrays.contains_key(name)
                    && !ctx.param_arrays.contains_key(name)
                    && !ctx.output_arrays.contains_key(name)
                {
                    let target_ty = decl_ty.unwrap_or(typed.ty);
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, target_ty),
                        &format!("v_{name}"),
                    )?;
                    let casted =
                        cast_orc_value_to(ctx, typed, target_ty, b"local_store_new_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot);
                    local_aliases.insert(
                        name.clone(),
                        AliasSlot {
                            ptr: slot,
                            ty: target_ty,
                        },
                    );
                    return Ok(());
                }
                Err(Diagnostic::internal(format!(
                    "unknown assignment target '{name}' in ORC lowering"
                )))
            }
            AssignTarget::Index { base, index } => {
                let typed = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
                if ctx.input_arrays.contains_key(base) || ctx.param_arrays.contains_key(base) {
                    return Err(Diagnostic::internal(format!(
                        "cannot assign to immutable top-level array '{base}' in ORC lowering"
                    )));
                }
                let data = if ctx.output_arrays.contains_key(base) {
                    lower_output_array_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                        true,
                    )?
                } else if ctx.buffer_index.contains_key(base) {
                    lower_buffer_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                        true,
                    )?
                } else {
                    lower_data_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_data_aliases,
                    )?
                };
                let casted = cast_orc_value_to(ctx, typed, data.elem_ty, b"data_store_cast\0");
                LLVMBuildStore(ctx.builder, casted, data.ptr);
                Ok(())
            }
        },
        Stmt::Expr { expr, .. } => {
            let _ = lower_expr(expr, ctx, locals, local_aliases, local_data_aliases)?;
            Ok(())
        }
        Stmt::Return { .. } => Err(Diagnostic::internal(
            "return statement is only valid inside def lowering",
        )),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let cond_value = lower_expr(cond, ctx, locals, local_aliases, local_data_aliases)?;
            let cond_bool = lower_orc_condition(ctx, cond_value, b"if_cond\0");

            let then_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_then\0".as_ptr().cast(),
            );
            let else_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_else\0".as_ptr().cast(),
            );
            let merge_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"if_merge\0".as_ptr().cast(),
            );

            LLVMBuildCondBr(ctx.builder, cond_bool, then_bb, else_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, then_bb);
            let mut then_locals = locals.clone();
            let mut then_aliases = local_aliases.clone();
            let mut then_data_aliases = local_data_aliases.clone();
            for nested in then_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut then_locals,
                    &mut then_aliases,
                    &mut then_data_aliases,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            if !current_block_terminated(ctx.builder) {
                LLVMBuildBr(ctx.builder, merge_bb);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, else_bb);
            let mut else_locals = locals.clone();
            let mut else_aliases = local_aliases.clone();
            let mut else_data_aliases = local_data_aliases.clone();
            for nested in else_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut else_locals,
                    &mut else_aliases,
                    &mut else_data_aliases,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            if !current_block_terminated(ctx.builder) {
                LLVMBuildBr(ctx.builder, merge_bb);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, merge_bb);
            Ok(())
        }
        Stmt::For {
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            ..
        } => {
            let preheader_bb = LLVMGetInsertBlock(ctx.builder);
            if preheader_bb.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get current block for for-loop lowering",
                ));
            }

            let cond_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_cond\0".as_ptr().cast(),
            );
            let body_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_body\0".as_ptr().cast(),
            );
            let latch_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_latch\0".as_ptr().cast(),
            );
            let end_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"for_end\0".as_ptr().cast(),
            );

            let start_value = lower_expr(start, ctx, locals, local_aliases, local_data_aliases)?;
            let start_v =
                cast_orc_value_to(ctx, start_value, PrimitiveType::I32, b"for_start_i32\0");
            let end_value = lower_expr(end, ctx, locals, local_aliases, local_data_aliases)?;
            let end_v = cast_orc_value_to(ctx, end_value, PrimitiveType::I32, b"for_end_i32\0");
            let step_v = if let Some(step_expr) = step {
                let step_value =
                    lower_expr(step_expr, ctx, locals, local_aliases, local_data_aliases)?;
                cast_orc_value_to(ctx, step_value, PrimitiveType::I32, b"for_step_i32\0")
            } else {
                const_i32(ctx.i32_ty, 1)
            };

            LLVMBuildBr(ctx.builder, cond_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
            let loop_i = LLVMBuildPhi(ctx.builder, ctx.i32_ty, b"for_i\0".as_ptr().cast());
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
                if *end_inclusive {
                    LLVMIntPredicate::LLVMIntSLE
                } else {
                    LLVMIntPredicate::LLVMIntSLT
                },
                loop_i,
                end_v,
                b"for_cmp_pos\0".as_ptr().cast(),
            );
            let cmp_neg = LLVMBuildICmp(
                ctx.builder,
                if *end_inclusive {
                    LLVMIntPredicate::LLVMIntSGE
                } else {
                    LLVMIntPredicate::LLVMIntSGT
                },
                loop_i,
                end_v,
                b"for_cmp_neg\0".as_ptr().cast(),
            );
            let step_pos = LLVMBuildICmp(
                ctx.builder,
                LLVMIntPredicate::LLVMIntSGT,
                step_v,
                const_i32(ctx.i32_ty, 0),
                b"for_step_pos\0".as_ptr().cast(),
            );
            let step_neg = LLVMBuildICmp(
                ctx.builder,
                LLVMIntPredicate::LLVMIntSLT,
                step_v,
                const_i32(ctx.i32_ty, 0),
                b"for_step_neg\0".as_ptr().cast(),
            );
            let pos_cond = LLVMBuildAnd(
                ctx.builder,
                step_pos,
                cmp_pos,
                b"for_pos_cond\0".as_ptr().cast(),
            );
            let neg_cond = LLVMBuildAnd(
                ctx.builder,
                step_neg,
                cmp_neg,
                b"for_neg_cond\0".as_ptr().cast(),
            );
            let cond = LLVMBuildOr(
                ctx.builder,
                pos_cond,
                neg_cond,
                b"for_cond\0".as_ptr().cast(),
            );
            LLVMBuildCondBr(ctx.builder, cond, body_bb, end_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
            let mut loop_locals = locals.clone();
            let mut loop_aliases = local_aliases.clone();
            let mut loop_data_aliases = local_data_aliases.clone();
            loop_locals.insert(
                var.clone(),
                OrcValue {
                    value: loop_i,
                    ty: PrimitiveType::I32,
                },
            );
            ctx.loop_stack.push(LoopControl {
                break_bb: end_bb,
                continue_bb: latch_bb,
            });
            for nested in body {
                lower_stmt(
                    nested,
                    ctx,
                    &mut loop_locals,
                    &mut loop_aliases,
                    &mut loop_data_aliases,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            let _ = ctx.loop_stack.pop();
            if !current_block_terminated(ctx.builder) {
                LLVMBuildBr(ctx.builder, latch_bb);
            }
            LLVMPositionBuilderAtEnd(ctx.builder, latch_bb);
            let latch_end_bb = LLVMGetInsertBlock(ctx.builder);
            if latch_end_bb.is_null() {
                return Err(Diagnostic::internal("failed to get for-loop latch block"));
            }
            let next_i = LLVMBuildAdd(ctx.builder, loop_i, step_v, b"for_i_next\0".as_ptr().cast());
            LLVMBuildBr(ctx.builder, cond_bb);
            let mut back_vals = [next_i];
            let mut back_blocks = [latch_end_bb];
            LLVMAddIncoming(loop_i, back_vals.as_mut_ptr(), back_blocks.as_mut_ptr(), 1);

            LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
            Ok(())
        }
        Stmt::While { cond, body, .. } => {
            let cond_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"while_cond\0".as_ptr().cast(),
            );
            let body_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"while_body\0".as_ptr().cast(),
            );
            let end_bb = LLVMAppendBasicBlockInContext(
                ctx.context,
                ctx.fn_ref,
                b"while_end\0".as_ptr().cast(),
            );

            LLVMBuildBr(ctx.builder, cond_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, cond_bb);
            let cond_value = lower_expr(cond, ctx, locals, local_aliases, local_data_aliases)?;
            let cond_bool = lower_orc_condition(ctx, cond_value, b"while_cond_bool\0");
            LLVMBuildCondBr(ctx.builder, cond_bool, body_bb, end_bb);

            LLVMPositionBuilderAtEnd(ctx.builder, body_bb);
            let mut loop_locals = locals.clone();
            let mut loop_aliases = local_aliases.clone();
            let mut loop_data_aliases = local_data_aliases.clone();
            ctx.loop_stack.push(LoopControl {
                break_bb: end_bb,
                continue_bb: cond_bb,
            });
            for nested in body {
                lower_stmt(
                    nested,
                    ctx,
                    &mut loop_locals,
                    &mut loop_aliases,
                    &mut loop_data_aliases,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            let _ = ctx.loop_stack.pop();
            if !current_block_terminated(ctx.builder) {
                LLVMBuildBr(ctx.builder, cond_bb);
            }

            LLVMPositionBuilderAtEnd(ctx.builder, end_bb);
            Ok(())
        }
        Stmt::Break { .. } => {
            let Some(loop_control) = ctx.loop_stack.last().copied() else {
                return Err(Diagnostic::internal(
                    "break statement encountered outside of loop in ORC lowering",
                ));
            };
            LLVMBuildBr(ctx.builder, loop_control.break_bb);
            Ok(())
        }
        Stmt::Continue { .. } => {
            let Some(loop_control) = ctx.loop_stack.last().copied() else {
                return Err(Diagnostic::internal(
                    "continue statement encountered outside of loop in ORC lowering",
                ));
            };
            LLVMBuildBr(ctx.builder, loop_control.continue_bb);
            Ok(())
        }
    }
}
