use super::struct_helpers::{
    lower_proc_array_call_args_in_def, lower_struct_array_call_args_in_def,
};
use super::*;

pub(super) unsafe fn lower_def_stmt(
    stmt: &Stmt,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    match stmt {
        Stmt::Const { .. } => Ok(false),
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
            let return_ty = ctx.return_ty.clone();
            match &return_ty {
                ReturnType::Scalar(scalar_ty) => {
                    let value = lower_def_expr(expr, ctx)?;
                    let ret_v = cast_def_value_to(ctx, value, *scalar_ty, b"def_ret_cast\0");
                    LLVMBuildStore(ctx.builder, ret_v, ctx.return_slot);
                }
                ReturnType::Tuple(elem_tys) => {
                    if let Expr::Tuple { values, .. } = expr {
                        if values.len() != elem_tys.len() {
                            return Err(Diagnostic::internal(format!(
                                "tuple return arity mismatch: expected {}, got {}",
                                elem_tys.len(),
                                values.len()
                            )));
                        }
                        let return_llvm_ty = llvm_ty_for_return_type(ctx.context, &return_ty);
                        let mut agg = LLVMGetUndef(return_llvm_ty);
                        for (i, (val_expr, elem_ty)) in
                            values.iter().zip(elem_tys.iter()).enumerate()
                        {
                            let elem_ty = *elem_ty;
                            let val = lower_def_expr(val_expr, ctx)?;
                            let cast_v = cast_def_value_to(ctx, val, elem_ty, b"tup_elem_cast\0");
                            agg = LLVMBuildInsertValue(
                                ctx.builder,
                                agg,
                                cast_v,
                                i as u32,
                                b"tup_ins\0".as_ptr().cast(),
                            );
                        }
                        LLVMBuildStore(ctx.builder, agg, ctx.return_slot);
                    } else {
                        // Handle returning a tuple-returning call or tuple variable
                        let (tuple_val, _) = lower_def_tuple_value(expr, ctx)?;
                        LLVMBuildStore(ctx.builder, tuple_val, ctx.return_slot);
                    }
                }
            }
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
        AssignTarget::Slice { base, start, end } => {
            lower_def_slice_assign(base, start.as_ref(), end.as_ref(), expr, ctx)
        }
        AssignTarget::Tuple(targets) => lower_def_tuple_destructure(targets, expr, ctx),
    }
}

/// Lower a tuple literal expression. Returns the LLVM struct value and element types.
unsafe fn lower_def_tuple_literal(
    values: &[Expr],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<(LLVMValueRef, Vec<PrimitiveType>), Diagnostic> {
    let mut elem_tys = Vec::new();
    let mut elem_vals = Vec::new();
    for val_expr in values {
        let val = lower_def_expr(val_expr, ctx)?;
        elem_tys.push(val.ty);
        elem_vals.push(val.value);
    }
    let mut llvm_elem_tys: Vec<LLVMTypeRef> = elem_tys
        .iter()
        .map(|t| llvm_ty_for_primitive(ctx.context, *t))
        .collect();
    let struct_ty = LLVMStructTypeInContext(
        ctx.context,
        llvm_elem_tys.as_mut_ptr(),
        llvm_elem_tys.len() as u32,
        0,
    );
    let mut agg = LLVMGetUndef(struct_ty);
    for (i, val) in elem_vals.into_iter().enumerate() {
        agg = LLVMBuildInsertValue(
            ctx.builder,
            agg,
            val,
            i as u32,
            b"tup_ins\0".as_ptr().cast(),
        );
    }
    Ok((agg, elem_tys))
}

/// Lower a tuple-returning user call. Reuses the same call infrastructure as
/// regular def-level UserCall lowering.
unsafe fn lower_def_tuple_call(
    name: &str,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<(LLVMValueRef, Vec<PrimitiveType>), Diagnostic> {
    let module = ctx.module;
    let context = ctx.context;
    let float_ty = ctx.float_ty;
    let sample_rate = ctx.sample_rate;
    let block_size = ctx.block_size;
    let fast_math = ctx.fast_math_flags != LLVMFastMathNone;
    let struct_fields = ctx.struct_fields;
    let user_fn_param_names = ctx.user_fn_param_names;
    let user_fn_param_defaults = ctx.user_fn_param_defaults;
    let user_fn_param_kinds = ctx.user_fn_param_kinds;
    let user_fn_param_by_ref = ctx.user_fn_param_by_ref;
    let user_registry = ctx.user_registry as *mut UserFnRegistry;
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let mut lower_scalar_expr =
        |arg_expr: &Expr| unsafe { lower_def_expr(arg_expr, &mut *ctx_ptr) };
    let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_buffer_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
    };
    let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_array_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
    };
    let prepared = prepare_user_call_common(
        name,
        type_args,
        args,
        module,
        context,
        float_ty,
        sample_rate,
        block_size,
        fast_math,
        struct_fields,
        user_fn_param_names,
        user_fn_param_defaults,
        user_fn_param_kinds,
        user_fn_param_by_ref,
        user_registry,
        &mut lower_scalar_expr,
        &mut infer_buffer_arg_signature,
        &mut infer_array_arg_signature,
        "def tuple call",
    )?;
    let ReturnType::Tuple(elem_tys) = &prepared.ret_ty else {
        return Err(Diagnostic::internal(
            "expected tuple return from user call in tuple assignment",
        ));
    };
    let elem_tys = elem_tys.clone();

    let mut arg_values = Vec::new();
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let mut cast_scalar_arg = |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
        cast_def_value_to(&*ctx_ptr, value, target_ty, arg_name)
    };
    let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                arg_expr: &Expr,
                                struct_name: &str,
                                by_ref: bool| unsafe {
        lower_struct_call_args_in_def(
            &mut *ctx_ptr,
            arg_values,
            arg_expr,
            struct_name,
            name,
            by_ref,
        )
    };
    let mut lower_struct_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                      arg_expr: &Expr,
                                      struct_name: &str| unsafe {
        lower_struct_array_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_proc_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                    arg_expr: &Expr,
                                    struct_name: &str| unsafe {
        lower_proc_array_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_array_arg =
        |arg_values: &mut Vec<LLVMValueRef>,
         arg_expr: &Expr,
         expected_elem_ty: Option<PrimitiveType>| unsafe {
            lower_array_call_args_in_def(
                &mut *ctx_ptr,
                arg_values,
                arg_expr,
                name,
                expected_elem_ty,
            )
        };
    let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
        lower_buffer_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, name)
    };
    materialize_user_call_args_common(
        name,
        &prepared,
        &mut arg_values,
        b"tup_call_arg\0",
        "def tuple call",
        &mut cast_scalar_arg,
        &mut lower_struct_arg,
        &mut lower_struct_array_arg,
        &mut lower_proc_array_arg,
        &mut lower_array_arg,
        &mut lower_buffer_arg,
    )?;
    let call = LLVMBuildCall2(
        ctx.builder,
        prepared.fn_ty,
        prepared.fn_ref,
        arg_values.as_mut_ptr(),
        arg_values.len() as u32,
        b"tup_call\0".as_ptr().cast(),
    );
    Ok((call, elem_tys))
}

/// Lower a tuple-valued expression (literal, call, or variable reference).
/// Returns the LLVM struct value and element types.
unsafe fn lower_def_tuple_value(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<(LLVMValueRef, Vec<PrimitiveType>), Diagnostic> {
    match expr {
        Expr::Tuple { values, .. } => lower_def_tuple_literal(values, ctx),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => lower_def_tuple_call(name, type_args, args, ctx),
        Expr::Var { name, .. } => {
            let slot = ctx
                .tuple_slots
                .get(name)
                .ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "unknown tuple variable '{name}' in tuple value lowering"
                    ))
                })?
                .clone();
            let mut llvm_elem_tys: Vec<LLVMTypeRef> = slot
                .elem_tys
                .iter()
                .map(|t| llvm_ty_for_primitive(ctx.context, *t))
                .collect();
            let struct_ty = LLVMStructTypeInContext(
                ctx.context,
                llvm_elem_tys.as_mut_ptr(),
                llvm_elem_tys.len() as u32,
                0,
            );
            let loaded = LLVMBuildLoad2(
                ctx.builder,
                struct_ty,
                slot.ptr,
                b"tup_load\0".as_ptr().cast(),
            );
            Ok((loaded, slot.elem_tys))
        }
        _ => Err(Diagnostic::internal(
            "expected tuple expression, tuple-returning call, or tuple variable",
        )),
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
    if try_lower_def_untyped_array_decl(target_name, expr, ctx)? {
        return Ok(false);
    }

    // Check for tuple expression or tuple-returning call assignment
    if matches!(expr, Expr::Tuple { .. }) || is_tuple_returning_user_call(expr, ctx) {
        let (tuple_val, elem_tys) = lower_def_tuple_value(expr, ctx)?;
        if let Some(existing) = ctx.tuple_slots.get(target_name) {
            // Re-assign to existing tuple slot
            LLVMBuildStore(ctx.builder, tuple_val, existing.ptr);
        } else {
            // Create new tuple slot
            let mut llvm_elem_tys: Vec<LLVMTypeRef> = elem_tys
                .iter()
                .map(|t| llvm_ty_for_primitive(ctx.context, *t))
                .collect();
            let struct_ty = LLVMStructTypeInContext(
                ctx.context,
                llvm_elem_tys.as_mut_ptr(),
                llvm_elem_tys.len() as u32,
                0,
            );
            let slot = build_local_slot(ctx.builder, struct_ty, &format!("v_{target_name}"))?;
            LLVMBuildStore(ctx.builder, tuple_val, slot);
            ctx.tuple_slots.insert(
                target_name.to_owned(),
                DefTupleSlot {
                    ptr: slot,
                    elem_tys,
                },
            );
        }
        return Ok(false);
    }
    if matches!(expr, Expr::Slice { .. }) {
        if ctx.local_slots.contains_key(target_name)
            || ctx.array_ptrs.contains_key(target_name)
            || ctx.local_array_aliases.contains_key(target_name)
        {
            return Err(Diagnostic::internal(format!(
                "slice alias declaration for '{target_name}' conflicts with existing symbol in def lowering"
            )));
        }
        let view = lower_def_array_view(ctx, expr, "slice alias assignment", None)?;
        ctx.local_array_aliases.insert(
            target_name.to_owned(),
            LocalArrayAlias::Primitive {
                base_ptr: view.base_ptr,
                len: view.len_hint,
                elem_ty: view.elem_ty,
            },
        );
        ctx.array_len_values
            .insert(target_name.to_owned(), view.len_val);
        return Ok(false);
    }

    if !ctx.local_slots.contains_key(target_name)
        && !ctx.local_array_aliases.contains_key(target_name)
        && !ctx.array_ptrs.contains_key(target_name)
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
    if ctx.array_ptrs.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "array symbol '{target_name}' must be assigned via index syntax in def lowering"
        )));
    }
    if ctx.local_array_aliases.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "array alias '{target_name}' must be assigned via index syntax in def lowering"
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
    let Expr::ArrayCtor { spec, init, .. } = expr else {
        return Ok(false);
    };

    if !is_typed_decl {
        return Err(Diagnostic::internal(
            "array constructor assignment in def lowering requires typed array declaration syntax",
        ));
    }
    if decl_ty.is_some() {
        return Err(Diagnostic::internal(
            "typed array declaration cannot include scalar declaration type in def lowering",
        ));
    }
    if ctx.local_slots.contains_key(target_name) || ctx.array_ptrs.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "typed array declaration for '{target_name}' conflicts with existing local symbol in def lowering"
        )));
    }

    let elem_ty = match &spec.elem {
        onda_frontend::ArrayElemType::Primitive(elem_ty) => *elem_ty,
        onda_frontend::ArrayElemType::Struct(name) => {
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
    ctx.array_ptrs.insert(target_name.to_owned(), ptr);
    ctx.array_len.insert(target_name.to_owned(), len);
    ctx.array_elem_ty.insert(target_name.to_owned(), elem_ty);
    Ok(true)
}

unsafe fn try_lower_def_untyped_array_decl(
    target_name: &str,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let Expr::ArrayLiteral { values, .. } = expr else {
        return Ok(false);
    };

    if let Some(expected_len) = ctx.array_len.get(target_name).copied() {
        if values.len() != expected_len {
            return Err(Diagnostic::internal(format!(
                "array symbol '{target_name}' initializer expects {expected_len} elements, got {}",
                values.len()
            )));
        }
        for (idx, value_expr) in values.iter().enumerate() {
            let typed = lower_def_expr(value_expr, ctx)?;
            let data = lower_def_data_element_ptr(ctx, target_name, &Expr::int(idx as i64), true)?;
            let casted = cast_def_value_to(ctx, typed, data.elem_ty, b"def_data_store_cast\0");
            LLVMBuildStore(ctx.builder, casted, data.ptr);
        }
        return Ok(true);
    }

    if ctx.local_array_aliases.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "array alias '{target_name}' must be assigned via index syntax in def lowering"
        )));
    }
    if ctx.local_slots.contains_key(target_name) || ctx.array_ptrs.contains_key(target_name) {
        return Err(Diagnostic::internal(format!(
            "array declaration for '{target_name}' conflicts with existing symbol in def lowering"
        )));
    }
    if values.is_empty() {
        return Err(Diagnostic::internal(format!(
            "array initializer for symbol '{target_name}' cannot be empty in def lowering"
        )));
    }

    let first_typed = lower_def_expr(&values[0], ctx)?;
    let elem_ty = first_typed.ty;
    let len = values.len();
    let ptr = build_local_array_slot(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, elem_ty),
        len,
        &format!("d_{target_name}"),
    )?;
    for (idx, value_expr) in values.iter().enumerate() {
        let typed = if idx == 0 {
            first_typed
        } else {
            lower_def_expr(value_expr, ctx)?
        };
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
    ctx.array_ptrs.insert(target_name.to_owned(), ptr);
    ctx.array_len.insert(target_name.to_owned(), len);
    ctx.array_elem_ty.insert(target_name.to_owned(), elem_ty);
    Ok(true)
}

unsafe fn try_bind_struct_data_alias_in_def(
    target_name: &str,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let Expr::Index { base, index, .. } = expr else {
        return Ok(false);
    };

    if let Some(struct_name) = ctx.array_struct_roots.get(base).cloned() {
        let root_len = *ctx.array_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing array[Struct] length metadata for '{base}' in def lowering"
            ))
        })?;
        let raw_index = lower_def_expr(index, ctx)?;
        let index_i32 = cast_def_value_to(
            ctx,
            raw_index,
            PrimitiveType::I32,
            b"def_data_alias_idx_i32\0",
        );
        let clamped = if let Some(runtime_len) = ctx.array_len_values.get(base).copied() {
            clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, index_i32, runtime_len)
        } else {
            clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, root_len)?
        };
        bind_struct_data_element_aliases_in_def(target_name, &struct_name, base, clamped, ctx)?;
        return Ok(true);
    }
    if try_bind_struct_data_slot_aliases_in_def(target_name, base, index, ctx)? {
        return Ok(true);
    }

    if let Some(alias) = ctx.local_array_aliases.get(base).cloned() {
        match alias {
            LocalArrayAlias::Primitive { .. } => return Ok(false),
            LocalArrayAlias::Struct {
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

    if ctx.array_ptrs.contains_key(base) {
        return Ok(false);
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

unsafe fn lower_def_slice_assign(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let dst_expr = Expr::Slice {
        loc: Default::default(),
        base: base.to_owned(),
        start: start.cloned().map(Box::new),
        end: end.cloned().map(Box::new),
    };
    let dst_view = lower_def_array_view(ctx, &dst_expr, "slice assignment target", None)?;
    let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, dst_view.elem_ty);

    if matches!(expr, Expr::Var { .. } | Expr::Slice { .. }) {
        let src_view = lower_def_array_view(ctx, expr, "slice assignment source", None)?;
        let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
        let copy_elem = move |loop_i| unsafe {
            let ctx = &mut *ctx_ptr;
            let src_ptr = build_f32_ptr_offset(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, src_view.elem_ty),
                src_view.base_ptr,
                loop_i,
                b"def_slice_copy_src_ptr\0",
            );
            let src_val = LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, src_view.elem_ty),
                src_ptr,
                b"def_slice_copy_src_val\0".as_ptr().cast(),
            );
            let casted = cast_def_value_to(
                ctx,
                OrcValue {
                    value: src_val,
                    ty: src_view.elem_ty,
                },
                dst_view.elem_ty,
                b"def_slice_copy_cast\0",
            );
            let dst_ptr = build_f32_ptr_offset(
                ctx.builder,
                elem_llvm_ty,
                dst_view.base_ptr,
                loop_i,
                b"def_slice_copy_dst_ptr\0",
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
            "def_slice",
            "def backward slice copy",
            "def forward slice copy",
            copy_elem,
        )?;
        return Ok(false);
    }

    let typed_value = lower_def_expr(expr, ctx)?;
    let fill_value = cast_def_value_to(ctx, typed_value, dst_view.elem_ty, b"def_slice_fill\0");
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let fill_elem = move |loop_i| unsafe {
        let ctx = &mut *ctx_ptr;
        let dst_ptr = build_f32_ptr_offset(
            ctx.builder,
            elem_llvm_ty,
            dst_view.base_ptr,
            loop_i,
            b"def_slice_fill_ptr\0",
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
        "def_slice",
        "def slice fill",
        fill_elem,
    )?;
    Ok(false)
}
unsafe fn lower_def_if_stmt(
    cond: &Expr,
    then_branch: &[Stmt],
    else_branch: &[Stmt],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let cond_value = lower_def_expr(cond, ctx)?;
    let cond_bool = {
        let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
            cast_def_value_to(ctx, value, to, name)
        };
        lower_condition_common(cond_value, b"def_if_cond\0", &mut cast_value)
    };
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let (then_terminated, else_terminated) = lower_if_stmt_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        cond_bool,
        b"def_if_then\0",
        b"def_if_else\0",
        b"def_if_merge\0",
        || unsafe {
            let ctx = &mut *ctx_ptr;
            let mut terminated = false;
            for nested in then_branch {
                if lower_def_stmt(nested, ctx)? {
                    terminated = true;
                    break;
                }
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            Ok(terminated)
        },
        || unsafe {
            let ctx = &mut *ctx_ptr;
            let mut terminated = false;
            for nested in else_branch {
                if lower_def_stmt(nested, ctx)? {
                    terminated = true;
                    break;
                }
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            Ok(terminated)
        },
    )?;
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

    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    lower_for_stmt_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        ctx.i32_ty,
        start_v,
        end_v,
        step_v,
        end_inclusive,
        b"def_for_cond\0",
        b"def_for_body\0",
        b"def_for_latch\0",
        b"def_for_end\0",
        "def for-loop",
        |loop_i, latch_bb, end_bb| unsafe {
            let ctx = &mut *ctx_ptr;
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
            Ok(())
        },
    )?;
    Ok(false)
}

unsafe fn lower_def_while_stmt(
    cond: &Expr,
    body: &[Stmt],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    lower_while_stmt_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        b"def_while_cond\0",
        b"def_while_body\0",
        b"def_while_end\0",
        || unsafe {
            let ctx = &mut *ctx_ptr;
            let cond_value = lower_def_expr(cond, ctx)?;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_def_value_to(ctx, value, to, name)
            };
            Ok(lower_condition_common(
                cond_value,
                b"def_while_cond_bool\0",
                &mut cast_value,
            ))
        },
        |cond_bb, end_bb| unsafe {
            let ctx = &mut *ctx_ptr;
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
            Ok(())
        },
    )?;
    Ok(false)
}

/// Check if an expression is a UserCall that returns a tuple.
unsafe fn is_tuple_returning_user_call(expr: &Expr, ctx: &DefLoweringCtx<'_>) -> bool {
    let Expr::UserCall { name, .. } = expr else {
        return false;
    };
    let registry = &*(ctx.user_registry);
    if let Some(ret_ty) = registry.base_return_tys.get(name) {
        return matches!(ret_ty, ReturnType::Tuple(_));
    }
    false
}

/// Lower tuple destructuring assignment: `(a, b) = expr`
unsafe fn lower_def_tuple_destructure(
    targets: &[String],
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    // Get the tuple value (from a variable, literal, or call)
    let (tuple_val, elem_tys) = if let Expr::Var { name, .. } = expr {
        // Load from an existing tuple variable
        let slot = ctx.tuple_slots.get(name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "unknown tuple variable '{name}' in destructuring assignment"
            ))
        })?;
        let slot_clone = slot.clone();
        let mut llvm_elem_tys: Vec<LLVMTypeRef> = slot_clone
            .elem_tys
            .iter()
            .map(|t| llvm_ty_for_primitive(ctx.context, *t))
            .collect();
        let struct_ty = LLVMStructTypeInContext(
            ctx.context,
            llvm_elem_tys.as_mut_ptr(),
            llvm_elem_tys.len() as u32,
            0,
        );
        let loaded = LLVMBuildLoad2(
            ctx.builder,
            struct_ty,
            slot_clone.ptr,
            b"tup_load\0".as_ptr().cast(),
        );
        (loaded, slot_clone.elem_tys)
    } else {
        lower_def_tuple_value(expr, ctx)?
    };

    if targets.len() != elem_tys.len() {
        return Err(Diagnostic::internal(format!(
            "tuple destructuring arity mismatch: {} targets, {} elements",
            targets.len(),
            elem_tys.len()
        )));
    }

    for (i, (target_name, elem_ty)) in targets.iter().zip(elem_tys.iter()).enumerate() {
        let elem_val = LLVMBuildExtractValue(
            ctx.builder,
            tuple_val,
            i as u32,
            b"tup_elem\0".as_ptr().cast(),
        );
        let elem_orc = OrcValue {
            value: elem_val,
            ty: *elem_ty,
        };
        if let Some(local) = ctx.local_slots.get(target_name).copied() {
            let casted = cast_def_value_to(ctx, elem_orc, local.ty, b"tup_destr_cast\0");
            LLVMBuildStore(ctx.builder, casted, local.ptr);
        } else {
            let slot = build_local_slot(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, *elem_ty),
                &format!("v_{target_name}"),
            )?;
            LLVMBuildStore(ctx.builder, elem_val, slot);
            ctx.local_slots.insert(
                target_name.to_owned(),
                DefLocalSlot {
                    ptr: slot,
                    ty: *elem_ty,
                },
            );
        }
    }
    Ok(false)
}
