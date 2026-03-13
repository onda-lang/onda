use super::*;

pub(super) unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Const { .. } => Ok(()),
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
                                .map(|f| Some(f.default.clone().unwrap_or(Expr::number(0.0))))
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
                                                local_array_aliases,
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
                                    TypedFieldType::Struct => {}
                                    TypedFieldType::Array(_) => {
                                        if !ctx.array_base_ptrs.contains_key(&flat_target)
                                            && !ctx.array_struct_len.contains_key(&flat_target)
                                        {
                                            return Err(Diagnostic::internal(format!(
                                                "missing array symbol '{flat_target}' in ORC lowering"
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

                if let Expr::ArrayCtor { spec, init, .. } = expr {
                    if let Some(&expected_len) = ctx.array_len.get(name) {
                        // State array: verify size and write init values if present.
                        let actual_len =
                            eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                        if expected_len != actual_len {
                            return Err(Diagnostic::internal(format!(
                                "array symbol '{name}' expected array[{expected_len}] but got array[{actual_len}]"
                            )));
                        }
                        if let Some(values) = init {
                            if values.len() != expected_len {
                                return Err(Diagnostic::internal(format!(
                                    "array symbol '{name}' initializer expects {expected_len} elements, got {}",
                                    values.len()
                                )));
                            }
                            for (idx, value_expr) in values.iter().enumerate() {
                                let typed = lower_expr(
                                    value_expr,
                                    ctx,
                                    locals,
                                    local_aliases,
                                    local_array_aliases,
                                )?;
                                let data = lower_data_element_ptr(
                                    ctx,
                                    name,
                                    &Expr::int(idx as i64),
                                    locals,
                                    local_aliases,
                                    local_array_aliases,
                                )?;
                                let casted = cast_orc_value_to(
                                    ctx,
                                    typed,
                                    data.elem_ty,
                                    b"array_ctor_init_cast\0",
                                );
                                LLVMBuildStore(ctx.builder, casted, data.ptr);
                            }
                        }
                        return Ok(());
                    } else if ctx.array_struct_len.contains_key(name) {
                        // Struct array: size check only, init values not supported here.
                        let actual_len =
                            eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
                        let expected_len = ctx.array_struct_len[name];
                        if expected_len != actual_len {
                            return Err(Diagnostic::internal(format!(
                                "array symbol '{name}' expected array[{expected_len}] but got array[{actual_len}]"
                            )));
                        }
                        return Ok(());
                    } else if *is_typed_decl {
                        // Local typed array declaration (not a state array).
                        if local_aliases.contains_key(name)
                            || local_array_aliases.contains_key(name)
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
                            omni_frontend::ArrayElemType::Primitive(elem_ty) => elem_ty,
                            omni_frontend::ArrayElemType::Struct(ref struct_name) => {
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
                                    local_array_aliases,
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
                        local_array_aliases.insert(
                            name.clone(),
                            LocalArrayAlias::Primitive {
                                base_ptr: ptr,
                                len,
                                elem_ty,
                            },
                        );
                        return Ok(());
                    } else {
                        return Err(Diagnostic::internal(format!(
                            "array constructor assigned to non-array symbol '{name}'"
                        )));
                    }
                }

                if let Expr::ArrayLiteral { values, .. } = expr {
                    if ctx.array_struct_len.contains_key(name) {
                        return Err(Diagnostic::internal(format!(
                            "array[Struct] symbol '{name}' must be assigned via indexed field writes"
                        )));
                    }
                    if let Some(expected_len) = ctx.array_len.get(name).copied() {
                        if values.len() != expected_len {
                            return Err(Diagnostic::internal(format!(
                                "array symbol '{name}' initializer expects {expected_len} elements, got {}",
                                values.len()
                            )));
                        }
                        for (idx, value_expr) in values.iter().enumerate() {
                            let typed = lower_expr(
                                value_expr,
                                ctx,
                                locals,
                                local_aliases,
                                local_array_aliases,
                            )?;
                            let data = lower_data_element_ptr(
                                ctx,
                                name,
                                &Expr::int(idx as i64),
                                locals,
                                local_aliases,
                                local_array_aliases,
                            )?;
                            let casted =
                                cast_orc_value_to(ctx, typed, data.elem_ty, b"array_store_cast\0");
                            LLVMBuildStore(ctx.builder, casted, data.ptr);
                        }
                        return Ok(());
                    }
                    if local_array_aliases.contains_key(name) {
                        return Err(Diagnostic::internal(format!(
                            "array alias '{name}' must be assigned via index syntax in ORC lowering"
                        )));
                    }
                    if local_aliases.contains_key(name)
                        || locals.contains_key(name)
                        || ctx.out_slots.contains_key(name)
                        || ctx.state_slots.contains_key(name)
                        || ctx.param_byte_offset.contains_key(name)
                        || ctx.input_index.contains_key(name)
                        || ctx.buffer_index.contains_key(name)
                    {
                        return Err(Diagnostic::internal(format!(
                            "array declaration for '{name}' conflicts with existing symbol in ORC lowering"
                        )));
                    }
                    if values.is_empty() {
                        return Err(Diagnostic::internal(format!(
                            "array initializer for symbol '{name}' cannot be empty in ORC lowering"
                        )));
                    }
                    let first_typed =
                        lower_expr(&values[0], ctx, locals, local_aliases, local_array_aliases)?;
                    let elem_ty = first_typed.ty;
                    let len = values.len();
                    let ptr = build_local_array_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, elem_ty),
                        len,
                        &format!("d_{name}"),
                    )?;
                    for (idx, value_expr) in values.iter().enumerate() {
                        let typed = if idx == 0 {
                            first_typed
                        } else {
                            lower_expr(value_expr, ctx, locals, local_aliases, local_array_aliases)?
                        };
                        let casted =
                            cast_orc_value_to(ctx, typed, elem_ty, b"local_arr_init_cast\0");
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
                    local_array_aliases.insert(
                        name.clone(),
                        LocalArrayAlias::Primitive {
                            base_ptr: ptr,
                            len,
                            elem_ty,
                        },
                    );
                    return Ok(());
                }
                if matches!(expr, Expr::Slice { .. }) {
                    if local_array_aliases.contains_key(name) {
                        return Err(Diagnostic::internal(format!(
                            "array alias '{name}' must be assigned via index syntax in ORC lowering"
                        )));
                    }
                    if local_aliases.contains_key(name)
                        || locals.contains_key(name)
                        || ctx.out_slots.contains_key(name)
                        || ctx.state_slots.contains_key(name)
                        || ctx.param_byte_offset.contains_key(name)
                        || ctx.input_index.contains_key(name)
                        || ctx.buffer_index.contains_key(name)
                    {
                        return Err(Diagnostic::internal(format!(
                            "slice alias declaration for '{name}' conflicts with existing symbol in ORC lowering"
                        )));
                    }
                    let view = lower_orc_array_view(
                        ctx,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        expr,
                        "slice alias assignment",
                    )?;
                    local_array_aliases.insert(
                        name.clone(),
                        LocalArrayAlias::Primitive {
                            base_ptr: view.base_ptr,
                            len: view.len_hint,
                            elem_ty: view.elem_ty,
                        },
                    );
                    ctx.array_len_values.insert(name.clone(), view.len_val);
                    return Ok(());
                }

                if let Some(alias) = local_aliases.get(name) {
                    let typed = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
                    let value = cast_orc_value_to(ctx, typed, alias.ty, b"alias_store_cast\0");
                    LLVMBuildStore(ctx.builder, value, alias.ptr);
                    return Ok(());
                }
                if local_array_aliases.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "array alias '{name}' must be assigned via index syntax"
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
                    && !local_array_aliases.contains_key(name)
                    && !ctx.out_slots.contains_key(name)
                    && !ctx.state_slots.contains_key(name)
                    && !ctx.param_byte_offset.contains_key(name)
                    && !ctx.input_index.contains_key(name)
                    && !ctx.array_base_ptrs.contains_key(name)
                    && !ctx.array_struct_len.contains_key(name)
                    && !ctx.buffer_index.contains_key(name)
                {
                    if let Expr::Index { base, index, .. } = expr {
                        if let Some(struct_name) = ctx.array_struct_roots.get(base).cloned() {
                            let root_len = *ctx.array_struct_len.get(base).ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing array[Struct] length metadata for '{base}'"
                                ))
                            })?;
                            let root_index = lower_clamped_data_index(
                                ctx,
                                index,
                                root_len,
                                locals,
                                local_aliases,
                                local_array_aliases,
                            )?;
                            bind_struct_data_element_aliases(
                                name,
                                &struct_name,
                                base,
                                root_index,
                                ctx,
                                local_aliases,
                                local_array_aliases,
                            )?;
                            return Ok(());
                        }

                        if let Some(alias) = local_array_aliases.get(base).cloned() {
                            match alias {
                                LocalArrayAlias::Primitive { .. } => {}
                                LocalArrayAlias::Struct {
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
                                        local_array_aliases,
                                    )?;
                                    let global_idx = LLVMBuildAdd(
                                        ctx.builder,
                                        start_index,
                                        local_idx,
                                        b"array_alias_global_idx\0".as_ptr().cast(),
                                    );
                                    bind_struct_data_element_aliases(
                                        name,
                                        &elem_struct,
                                        &root_base,
                                        global_idx,
                                        ctx,
                                        local_aliases,
                                        local_array_aliases,
                                    )?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                }

                let typed = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
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
                if ctx.array_base_ptrs.contains_key(name) || ctx.array_struct_len.contains_key(name)
                {
                    return Err(Diagnostic::internal(format!(
                        "array symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if ctx.buffer_index.contains_key(name) {
                    return Err(Diagnostic::internal(format!(
                        "buffer symbol '{name}' must be assigned via index syntax"
                    )));
                }
                if !locals.contains_key(name)
                    && !local_array_aliases.contains_key(name)
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
                let typed = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
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
                        local_array_aliases,
                        true,
                    )?
                } else if ctx.buffer_index.contains_key(base) {
                    lower_buffer_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        true,
                    )?
                } else {
                    lower_data_element_ptr(
                        ctx,
                        base,
                        index,
                        locals,
                        local_aliases,
                        local_array_aliases,
                    )?
                };
                let casted = cast_orc_value_to(ctx, typed, data.elem_ty, b"array_store_cast\0");
                LLVMBuildStore(ctx.builder, casted, data.ptr);
                Ok(())
            }
            AssignTarget::Slice { base, start, end } => lower_orc_slice_assign(
                base,
                start.as_ref(),
                end.as_ref(),
                expr,
                ctx,
                locals,
                local_aliases,
                local_array_aliases,
            ),
        },
        Stmt::Expr { expr, .. } => {
            let _ = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
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
            let cond_value = lower_expr(cond, ctx, locals, local_aliases, local_array_aliases)?;
            let cond_bool = {
                let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                    cast_orc_value_to(ctx, value, to, name)
                };
                lower_condition_common(cond_value, b"if_cond\0", &mut cast_value)
            };
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            lower_if_stmt_common(
                ctx.builder,
                ctx.context,
                ctx.fn_ref,
                cond_bool,
                b"if_then\0",
                b"if_else\0",
                b"if_merge\0",
                || unsafe {
                    let ctx = &mut *ctx_ptr;
                    let mut then_locals = locals.clone();
                    let mut then_aliases = local_aliases.clone();
                    let mut then_data_aliases = local_array_aliases.clone();
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
                    Ok(())
                },
                || unsafe {
                    let ctx = &mut *ctx_ptr;
                    let mut else_locals = locals.clone();
                    let mut else_aliases = local_aliases.clone();
                    let mut else_data_aliases = local_array_aliases.clone();
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
                    Ok(())
                },
            )?;
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
            let start_value = lower_expr(start, ctx, locals, local_aliases, local_array_aliases)?;
            let start_v =
                cast_orc_value_to(ctx, start_value, PrimitiveType::I32, b"for_start_i32\0");
            let end_value = lower_expr(end, ctx, locals, local_aliases, local_array_aliases)?;
            let end_v = cast_orc_value_to(ctx, end_value, PrimitiveType::I32, b"for_end_i32\0");
            let step_v = if let Some(step_expr) = step {
                let step_value =
                    lower_expr(step_expr, ctx, locals, local_aliases, local_array_aliases)?;
                cast_orc_value_to(ctx, step_value, PrimitiveType::I32, b"for_step_i32\0")
            } else {
                const_i32(ctx.i32_ty, 1)
            };

            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            lower_for_stmt_common(
                ctx.builder,
                ctx.context,
                ctx.fn_ref,
                ctx.i32_ty,
                start_v,
                end_v,
                step_v,
                *end_inclusive,
                b"for_cond\0",
                b"for_body\0",
                b"for_latch\0",
                b"for_end\0",
                "for-loop lowering",
                |loop_i, latch_bb, end_bb| unsafe {
                    let ctx = &mut *ctx_ptr;
                    let mut loop_locals = locals.clone();
                    let mut loop_aliases = local_aliases.clone();
                    let mut loop_data_aliases = local_array_aliases.clone();
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
                    Ok(())
                },
            )?;
            Ok(())
        }
        Stmt::While { cond, body, .. } => {
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            lower_while_stmt_common(
                ctx.builder,
                ctx.context,
                ctx.fn_ref,
                b"while_cond\0",
                b"while_body\0",
                b"while_end\0",
                || unsafe {
                    let ctx = &mut *ctx_ptr;
                    let cond_value =
                        lower_expr(cond, ctx, locals, local_aliases, local_array_aliases)?;
                    let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                        cast_orc_value_to(ctx, value, to, name)
                    };
                    Ok(lower_condition_common(
                        cond_value,
                        b"while_cond_bool\0",
                        &mut cast_value,
                    ))
                },
                |cond_bb, end_bb| unsafe {
                    let ctx = &mut *ctx_ptr;
                    let mut loop_locals = locals.clone();
                    let mut loop_aliases = local_aliases.clone();
                    let mut loop_data_aliases = local_array_aliases.clone();
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
                    Ok(())
                },
            )?;
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

unsafe fn lower_orc_slice_assign(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
) -> Result<(), Diagnostic> {
    let dst_expr = Expr::Slice {
        loc: Default::default(),
        base: base.to_owned(),
        start: start.cloned().map(Box::new),
        end: end.cloned().map(Box::new),
    };
    let dst_view = lower_orc_array_view(
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        &dst_expr,
        "slice assignment target",
    )?;
    let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, dst_view.elem_ty);

    if matches!(expr, Expr::Var { .. } | Expr::Slice { .. }) {
        let src_view = lower_orc_array_view(
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            expr,
            "slice assignment source",
        )?;
        let ctx_ptr: *mut LoweringCtx<'_> = ctx;
        let copy_elem = move |loop_i| unsafe {
            let ctx = &mut *ctx_ptr;
            let src_ptr = build_f32_ptr_offset(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, src_view.elem_ty),
                src_view.base_ptr,
                loop_i,
                b"slice_copy_src_ptr\0",
            );
            let src_val = LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, src_view.elem_ty),
                src_ptr,
                b"slice_copy_src_val\0".as_ptr().cast(),
            );
            let casted = cast_orc_value_to(
                ctx,
                OrcValue {
                    value: src_val,
                    ty: src_view.elem_ty,
                },
                dst_view.elem_ty,
                b"slice_copy_cast\0",
            );
            let dst_ptr = build_f32_ptr_offset(
                ctx.builder,
                elem_llvm_ty,
                dst_view.base_ptr,
                loop_i,
                b"slice_copy_dst_ptr\0",
            );
            LLVMBuildStore(ctx.builder, casted, dst_ptr);
            Ok(())
        };
        lower_slice_copy_common(
            ctx.builder,
            ctx.context,
            ctx.fn_ref,
            ctx.i32_ty,
            dst_view,
            src_view,
            "slice",
            "orc backward slice copy",
            "orc forward slice copy",
            copy_elem,
        )?;
        return Ok(());
    }

    let typed = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
    let fill_value = cast_orc_value_to(ctx, typed, dst_view.elem_ty, b"slice_fill\0");
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let fill_elem = move |loop_i| unsafe {
        let ctx = &mut *ctx_ptr;
        let dst_ptr = build_f32_ptr_offset(
            ctx.builder,
            elem_llvm_ty,
            dst_view.base_ptr,
            loop_i,
            b"slice_fill_ptr\0",
        );
        LLVMBuildStore(ctx.builder, fill_value, dst_ptr);
        Ok(())
    };
    lower_slice_fill_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        ctx.i32_ty,
        dst_view,
        "slice",
        "orc slice fill",
        fill_elem,
    )?;
    Ok(())
}
