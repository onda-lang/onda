use super::*;
use crate::orc_backend::lowering_common::{
    lower_stmt_common, SharedNonAssignStmtBackend, SharedStmtBackend,
};

fn merge_branch_flow_map<T: Clone>(
    base: &HashMap<String, T>,
    then_map: HashMap<String, T>,
    else_map: HashMap<String, T>,
) -> HashMap<String, T> {
    let mut merged = base.clone();
    for (name, value) in then_map {
        if else_map.contains_key(&name) {
            merged.insert(name, value);
        }
    }
    merged
}

fn adopt_loop_flow_map<T>(
    base: &HashMap<String, T>,
    mut loop_map: HashMap<String, T>,
) -> HashMap<String, T> {
    loop_map.retain(|name, _| base.contains_key(name));
    loop_map
}

struct OrcNonAssignStmtBackend<'a, 'ctx> {
    ctx: &'a mut LoweringCtx<'ctx>,
    locals: &'a mut HashMap<String, OrcValue>,
    local_aliases: &'a mut HashMap<String, AliasSlot>,
    local_array_aliases: &'a mut HashMap<String, LocalArrayAlias>,
    local_tuples: &'a mut HashMap<String, Vec<PrimitiveType>>,
}

impl SharedNonAssignStmtBackend for OrcNonAssignStmtBackend<'_, '_> {
    type Output = ();

    fn const_stmt_result(&self) -> Self::Output {}

    unsafe fn lower_expr_stmt(&mut self, expr: &Expr) -> Result<Self::Output, Diagnostic> {
        lower_orc_expr_stmt(
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_return_stmt(&mut self, expr: &Expr) -> Result<Self::Output, Diagnostic> {
        lower_orc_return_stmt(expr)
    }

    unsafe fn lower_if_stmt(
        &mut self,
        cond: &Expr,
        then_branch: &[Stmt],
        else_branch: &[Stmt],
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_if_stmt(
            cond,
            then_branch,
            else_branch,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_for_stmt(
        &mut self,
        var: &str,
        step: Option<&Expr>,
        start: &Expr,
        end: &Expr,
        end_inclusive: bool,
        body: &[Stmt],
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_for_stmt(
            var,
            step,
            start,
            end,
            end_inclusive,
            body,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_while_stmt(
        &mut self,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_while_stmt(
            cond,
            body,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_break_stmt(&mut self) -> Result<Self::Output, Diagnostic> {
        lower_orc_break_stmt(self.ctx)
    }

    unsafe fn lower_continue_stmt(&mut self) -> Result<Self::Output, Diagnostic> {
        lower_orc_continue_stmt(self.ctx)
    }
}

impl SharedStmtBackend for OrcNonAssignStmtBackend<'_, '_> {
    unsafe fn lower_var_assign(
        &mut self,
        target_name: &str,
        decl_ty: Option<PrimitiveType>,
        is_typed_decl: bool,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_var_assign(
            target_name,
            decl_ty,
            is_typed_decl,
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_index_assign(
        &mut self,
        base: &str,
        index: &Expr,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_index_assign(
            base,
            index,
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_slice_assign(
        &mut self,
        base: &str,
        start: Option<&Expr>,
        end: Option<&Expr>,
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_slice_assign(
            base,
            start,
            end,
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_tuple_destructure(
        &mut self,
        targets: &[String],
        expr: &Expr,
    ) -> Result<Self::Output, Diagnostic> {
        lower_orc_tuple_destructure(
            targets,
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }
}

pub(super) unsafe fn lower_stmt(
    stmt: &Stmt,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let mut backend = OrcNonAssignStmtBackend {
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    };
    lower_stmt_common(&mut backend, stmt)
}

unsafe fn lower_orc_var_assign(
    name: &str,
    decl_ty: Option<PrimitiveType>,
    is_typed_decl: bool,
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
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
                            let slot = ctx.state_slots.get(&flat_target).ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing state slot for struct field '{flat_target}' in ORC lowering"
                                ))
                            })?;
                            let resolved_arg =
                                resolved_scalar_args.get(scalar_arg_idx).copied().flatten();
                            let value_typed = if let Some(arg_expr) = resolved_arg {
                                let typed = lower_expr(
                                    arg_expr,
                                    ctx,
                                    locals,
                                    local_aliases,
                                    local_array_aliases,
                                    local_tuples,
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
                                let default_value = eval_const_default_expr_typed(
                                    default_expr,
                                    ctx.sample_rate,
                                    ctx.block_size,
                                )?;
                                super::super::expr_common::llvm_const_from_const_default_value(
                                    ctx.context,
                                    ctx.float_ty,
                                    slot.ty,
                                    default_value,
                                )
                            };
                            scalar_arg_idx += 1;
                            LLVMBuildStore(ctx.builder, value_typed, slot.ptr);
                        }
                        TypedFieldType::Struct => {}
                        TypedFieldType::Tuple(ref elem_tys) => {
                            let default_values: Vec<Expr> =
                                if let Some(Expr::Tuple { values, .. }) = &field.default {
                                    values.clone()
                                } else {
                                    elem_tys.iter().map(|_| Expr::number(0.0)).collect()
                                };
                            for (idx, _prim) in elem_tys.iter().enumerate() {
                                let elem_flat = format!("{flat_target}.__{idx}");
                                let slot =
                                    ctx.state_slots.get(&elem_flat).ok_or_else(|| {
                                        Diagnostic::internal(format!(
                                            "missing state slot for struct tuple field '{elem_flat}' in ORC lowering"
                                        ))
                                    })?;
                                let default_expr = default_values
                                    .get(idx)
                                    .cloned()
                                    .unwrap_or(Expr::number(0.0));
                                let typed = lower_expr(
                                    &default_expr,
                                    ctx,
                                    locals,
                                    local_aliases,
                                    local_array_aliases,
                                    local_tuples,
                                )?;
                                let casted =
                                    cast_orc_value_to(ctx, typed, slot.ty, b"ctor_tuple_elem\0");
                                LLVMBuildStore(ctx.builder, casted, slot.ptr);
                            }
                        }
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
                        local_tuples,
                    )?;
                    let data = lower_data_element_ptr(
                        ctx,
                        name,
                        &Expr::int(idx as i64),
                        locals,
                        local_aliases,
                        local_array_aliases,
                        local_tuples,
                    )?;
                    let casted =
                        cast_orc_value_to(ctx, typed, data.elem_ty, b"array_ctor_init_cast\0");
                    LLVMBuildStore(ctx.builder, casted, data.ptr);
                }
            }
            return Ok(());
        } else if ctx.array_struct_len.contains_key(name) {
            let actual_len =
                eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
            let expected_len = ctx.array_struct_len[name];
            if expected_len != actual_len {
                return Err(Diagnostic::internal(format!(
                    "array symbol '{name}' expected array[{expected_len}] but got array[{actual_len}]"
                )));
            }
            return Ok(());
        } else if is_typed_decl {
            if local_aliases.contains_key(name) || local_array_aliases.contains_key(name) {
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
                onda_frontend::ArrayElemType::Primitive(elem_ty) => elem_ty,
                onda_frontend::ArrayElemType::Struct(ref struct_name) => {
                    return Err(Diagnostic::internal(format!(
                        "typed array declaration '{name}: {struct_name}[N]' is not yet supported in ORC lowering"
                    )))
                }
            };
            let len = eval_const_data_size_expr(&spec.size, ctx.sample_rate, ctx.block_size)?;
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
                        local_tuples,
                    )?;
                    let casted = cast_orc_value_to(ctx, typed, elem_ty, b"local_arr_init_cast\0");
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
                name.to_owned(),
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
                    local_tuples,
                )?;
                let data = lower_data_element_ptr(
                    ctx,
                    name,
                    &Expr::int(idx as i64),
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                )?;
                let casted = cast_orc_value_to(ctx, typed, data.elem_ty, b"array_store_cast\0");
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
        let first_typed = lower_expr(
            &values[0],
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )?;
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
                lower_expr(
                    value_expr,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                )?
            };
            let casted = cast_orc_value_to(ctx, typed, elem_ty, b"local_arr_init_cast\0");
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
            name.to_owned(),
            LocalArrayAlias::Primitive {
                base_ptr: ptr,
                len,
                elem_ty,
            },
        );
        return Ok(());
    }

    if let Expr::Tuple { values, .. } = expr {
        if ctx.state_slots.contains_key(&format!("{name}.__0")) {
            for (idx, value_expr) in values.iter().enumerate() {
                let flat_name = format!("{name}.__{idx}");
                let slot = ctx.state_slots.get(&flat_name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing state slot for tuple element '{flat_name}' in ORC init lowering"
                    ))
                })?;
                let typed = lower_expr(
                    value_expr,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                )?;
                let casted = cast_orc_value_to(ctx, typed, slot.ty, b"tuple_init_cast\0");
                LLVMBuildStore(ctx.builder, casted, slot.ptr);
            }
            return Ok(());
        }
        let mut elem_tys = Vec::with_capacity(values.len());
        for (idx, value_expr) in values.iter().enumerate() {
            let typed = lower_expr(
                value_expr,
                ctx,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            )?;
            let flat_name = format!("{name}.__{idx}");
            if let Some(slot) = local_aliases.get(&flat_name) {
                let casted = cast_orc_value_to(ctx, typed, slot.ty, b"tup_local_cast\0");
                LLVMBuildStore(ctx.builder, casted, slot.ptr);
            } else {
                let slot = build_local_slot(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, typed.ty),
                    &format!("v_{flat_name}"),
                )?;
                LLVMBuildStore(ctx.builder, typed.value, slot);
                local_aliases.insert(
                    flat_name,
                    AliasSlot {
                        ptr: slot,
                        ty: typed.ty,
                    },
                );
            }
            elem_tys.push(typed.ty);
        }
        local_tuples.insert(name.to_owned(), elem_tys);
        return Ok(());
    }

    if let Expr::UserCall { name: fn_name, .. } = expr {
        if let Some(ret_ty) = lookup_user_fn_return_type(ctx, fn_name) {
            if let ReturnType::Tuple(ref elem_tys) = ret_ty {
                let elem_tys = elem_tys.clone();
                return lower_orc_tuple_from_call(
                    name,
                    &elem_tys,
                    expr,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                );
            }
        }
    }

    if let Expr::Var { name: src_name, .. } = expr {
        if let Some(elem_tys) = local_tuples.get(src_name).cloned() {
            for (idx, ty) in elem_tys.iter().enumerate() {
                let src_flat = format!("{src_name}.__{idx}");
                let src_slot = local_aliases.get(&src_flat).ok_or_else(|| {
                    Diagnostic::internal(format!("local tuple element '{src_flat}' not found"))
                })?;
                let val = LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, src_slot.ty),
                    src_slot.ptr,
                    b"tup_copy_load\0".as_ptr().cast(),
                );
                let dst_flat = format!("{name}.__{idx}");
                if let Some(dst_slot) = local_aliases.get(&dst_flat) {
                    let casted = cast_orc_value_to(
                        ctx,
                        OrcValue {
                            value: val,
                            ty: *ty,
                        },
                        dst_slot.ty,
                        b"tup_copy_cast\0",
                    );
                    LLVMBuildStore(ctx.builder, casted, dst_slot.ptr);
                } else {
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, *ty),
                        &format!("v_{dst_flat}"),
                    )?;
                    LLVMBuildStore(ctx.builder, val, slot);
                    local_aliases.insert(dst_flat, AliasSlot { ptr: slot, ty: *ty });
                }
            }
            local_tuples.insert(name.to_owned(), elem_tys);
            return Ok(());
        }
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
            local_tuples,
            expr,
            "slice alias assignment",
            None,
        )?;
        local_array_aliases.insert(
            name.to_owned(),
            LocalArrayAlias::Primitive {
                base_ptr: view.base_ptr,
                len: view.len_hint,
                elem_ty: view.elem_ty,
            },
        );
        ctx.array_len_values.insert(name.to_owned(), view.len_val);
        return Ok(());
    }

    if let Some(alias) = local_aliases.get(name) {
        let typed = lower_expr(
            expr,
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )?;
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
                    local_tuples,
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
                            local_tuples,
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

    let typed = lower_expr(
        expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
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
    if ctx.array_base_ptrs.contains_key(name)
        || ctx.array_struct_len.contains_key(name)
        || ctx.const_arrays.contains_key(name)
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
        && !ctx.const_arrays.contains_key(name)
    {
        let target_ty = decl_ty.unwrap_or(typed.ty);
        let slot = build_local_slot(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, target_ty),
            &format!("v_{name}"),
        )?;
        let casted = cast_orc_value_to(ctx, typed, target_ty, b"local_store_new_cast\0");
        LLVMBuildStore(ctx.builder, casted, slot);
        local_aliases.insert(
            name.to_owned(),
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

unsafe fn lower_orc_index_assign(
    base: &str,
    index: &Expr,
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    if base == "outs" {
        if let (Some(meta), Some(arr)) = (ctx.port_index_outs, ctx.out_slot_ptr_array) {
            let typed_val = lower_expr(
                expr,
                ctx,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            )?;
            let idx = lower_array_index_i32(
                ctx,
                index,
                meta.count,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
                true,
            )?;
            let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, meta.elem_ty);
            let slot_ptr_ty = LLVMPointerType(elem_llvm_ty, 0);
            let gep = LLVMBuildGEP2(
                ctx.builder,
                slot_ptr_ty,
                arr,
                [idx].as_mut_ptr(),
                1,
                b"outs_w_slot_gep\0".as_ptr().cast(),
            );
            let slot_ptr = LLVMBuildLoad2(
                ctx.builder,
                slot_ptr_ty,
                gep,
                b"outs_w_slot_ptr\0".as_ptr().cast(),
            );
            let casted = cast_orc_value_to(ctx, typed_val, meta.elem_ty, b"outs_store_cast\0");
            LLVMBuildStore(ctx.builder, casted, slot_ptr);
            return Ok(());
        }
    }

    if ctx.state_slots.contains_key(&format!("{base}.__0")) {
        let typed = lower_expr(
            expr,
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )?;
        if let Expr::Int { value, .. } = index {
            let flat_name = format!("{base}.__{value}");
            let slot = ctx.state_slots.get(&flat_name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "tuple state element '{flat_name}' not found in state slots"
                ))
            })?;
            let casted = cast_orc_value_to(ctx, typed, slot.ty, b"tuple_state_store_cast\0");
            LLVMBuildStore(ctx.builder, casted, slot.ptr);
            return Ok(());
        }
        return Err(Diagnostic::internal(
            "tuple element index must be a compile-time integer constant in ORC lowering",
        ));
    }

    let typed = lower_expr(
        expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
    if ctx.input_arrays.contains_key(base)
        || ctx.param_arrays.contains_key(base)
        || ctx.const_arrays.contains_key(base)
    {
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
            local_tuples,
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
            local_tuples,
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
            local_tuples,
        )?
    };
    let casted = cast_orc_value_to(ctx, typed, data.elem_ty, b"array_store_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(())
}

unsafe fn lower_orc_expr_stmt(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let _ = lower_expr(
        expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
    Ok(())
}

unsafe fn lower_orc_return_stmt(_expr: &Expr) -> Result<(), Diagnostic> {
    Err(Diagnostic::internal(
        "return statement is only valid inside def lowering",
    ))
}

unsafe fn lower_orc_if_stmt(
    cond: &Expr,
    then_branch: &[Stmt],
    else_branch: &[Stmt],
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let cond_value = lower_expr(
        cond,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
    let cond_bool = {
        let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
            cast_orc_value_to(ctx, value, to, name)
        };
        lower_condition_common(cond_value, b"if_cond\0", &mut cast_value)
    };
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let base_locals = locals.clone();
    let base_aliases = local_aliases.clone();
    let base_data_aliases = local_array_aliases.clone();
    let base_tuples = local_tuples.clone();
    let (then_flow, else_flow) = lower_if_stmt_common(
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
            let mut then_tuples = local_tuples.clone();
            for nested in then_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut then_locals,
                    &mut then_aliases,
                    &mut then_data_aliases,
                    &mut then_tuples,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            Ok((then_locals, then_aliases, then_data_aliases, then_tuples))
        },
        || unsafe {
            let ctx = &mut *ctx_ptr;
            let mut else_locals = locals.clone();
            let mut else_aliases = local_aliases.clone();
            let mut else_data_aliases = local_array_aliases.clone();
            let mut else_tuples = local_tuples.clone();
            for nested in else_branch {
                lower_stmt(
                    nested,
                    ctx,
                    &mut else_locals,
                    &mut else_aliases,
                    &mut else_data_aliases,
                    &mut else_tuples,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            Ok((else_locals, else_aliases, else_data_aliases, else_tuples))
        },
    )?;
    *locals = merge_branch_flow_map(&base_locals, then_flow.0, else_flow.0);
    *local_aliases = merge_branch_flow_map(&base_aliases, then_flow.1, else_flow.1);
    *local_array_aliases = merge_branch_flow_map(&base_data_aliases, then_flow.2, else_flow.2);
    *local_tuples = merge_branch_flow_map(&base_tuples, then_flow.3, else_flow.3);
    Ok(())
}

unsafe fn lower_orc_for_stmt(
    var: &str,
    step: Option<&Expr>,
    start: &Expr,
    end: &Expr,
    end_inclusive: bool,
    body: &[Stmt],
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let start_value = lower_expr(
        start,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
    let start_v = cast_orc_value_to(ctx, start_value, PrimitiveType::I32, b"for_start_i32\0");
    let end_value = lower_expr(
        end,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
    let end_v = cast_orc_value_to(ctx, end_value, PrimitiveType::I32, b"for_end_i32\0");
    let step_v = if let Some(step_expr) = step {
        let step_value = lower_expr(
            step_expr,
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )?;
        cast_orc_value_to(ctx, step_value, PrimitiveType::I32, b"for_step_i32\0")
    } else {
        const_i32(ctx.i32_ty, 1)
    };

    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let base_locals = locals.clone();
    let base_aliases = local_aliases.clone();
    let base_data_aliases = local_array_aliases.clone();
    let base_tuples = local_tuples.clone();
    let mut loop_flow = None;
    lower_for_stmt_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        ctx.i32_ty,
        start_v,
        end_v,
        step_v,
        end_inclusive,
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
            let mut loop_tuples = local_tuples.clone();
            loop_locals.insert(
                var.to_owned(),
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
                    &mut loop_tuples,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            let _ = ctx.loop_stack.pop();
            loop_flow = Some((loop_locals, loop_aliases, loop_data_aliases, loop_tuples));
            Ok(())
        },
    )?;
    if let Some((loop_locals, loop_aliases, loop_data_aliases, loop_tuples)) = loop_flow {
        *locals = adopt_loop_flow_map(&base_locals, loop_locals);
        *local_aliases = adopt_loop_flow_map(&base_aliases, loop_aliases);
        *local_array_aliases = adopt_loop_flow_map(&base_data_aliases, loop_data_aliases);
        *local_tuples = adopt_loop_flow_map(&base_tuples, loop_tuples);
    }
    Ok(())
}

unsafe fn lower_orc_while_stmt(
    cond: &Expr,
    body: &[Stmt],
    ctx: &mut LoweringCtx<'_>,
    locals: &mut HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let base_locals = locals.clone();
    let base_aliases = local_aliases.clone();
    let base_data_aliases = local_array_aliases.clone();
    let base_tuples = local_tuples.clone();
    let mut loop_flow = None;
    lower_while_stmt_common(
        ctx.builder,
        ctx.context,
        ctx.fn_ref,
        b"while_cond\0",
        b"while_body\0",
        b"while_end\0",
        || unsafe {
            let ctx = &mut *ctx_ptr;
            let cond_value = lower_expr(
                cond,
                ctx,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            )?;
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
            let mut loop_tuples = local_tuples.clone();
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
                    &mut loop_tuples,
                )?;
                if current_block_terminated(ctx.builder) {
                    break;
                }
            }
            let _ = ctx.loop_stack.pop();
            loop_flow = Some((loop_locals, loop_aliases, loop_data_aliases, loop_tuples));
            Ok(())
        },
    )?;
    if let Some((loop_locals, loop_aliases, loop_data_aliases, loop_tuples)) = loop_flow {
        *locals = adopt_loop_flow_map(&base_locals, loop_locals);
        *local_aliases = adopt_loop_flow_map(&base_aliases, loop_aliases);
        *local_array_aliases = adopt_loop_flow_map(&base_data_aliases, loop_data_aliases);
        *local_tuples = adopt_loop_flow_map(&base_tuples, loop_tuples);
    }
    Ok(())
}

unsafe fn lower_orc_break_stmt(ctx: &mut LoweringCtx<'_>) -> Result<(), Diagnostic> {
    let Some(loop_control) = ctx.loop_stack.last().copied() else {
        return Err(Diagnostic::internal(
            "break statement encountered outside of loop in ORC lowering",
        ));
    };
    LLVMBuildBr(ctx.builder, loop_control.break_bb);
    Ok(())
}

unsafe fn lower_orc_continue_stmt(ctx: &mut LoweringCtx<'_>) -> Result<(), Diagnostic> {
    let Some(loop_control) = ctx.loop_stack.last().copied() else {
        return Err(Diagnostic::internal(
            "continue statement encountered outside of loop in ORC lowering",
        ));
    };
    LLVMBuildBr(ctx.builder, loop_control.continue_bb);
    Ok(())
}

/// Look up the return type of a user function by name, checking both base and mono registries.
unsafe fn lookup_user_fn_return_type(ctx: &LoweringCtx<'_>, fn_name: &str) -> Option<ReturnType> {
    let registry = &*ctx.user_registry;
    if let Some(rt) = registry.mono_return_tys.get(fn_name) {
        return Some(rt.clone());
    }
    if let Some(rt) = registry.base_return_tys.get(fn_name) {
        return Some(rt.clone());
    }
    None
}

/// Lower a tuple-returning user call and store elements as local tuple aliases.
#[allow(clippy::too_many_arguments)]
unsafe fn lower_orc_tuple_from_call(
    dest_name: &str,
    elem_tys: &[PrimitiveType],
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = expr
    else {
        return Err(Diagnostic::internal(
            "lower_orc_tuple_from_call called with non-UserCall expr",
        ));
    };
    let shared = orc_user_call_context(ctx);
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let mut lower_scalar_expr = |arg_expr: &Expr| unsafe {
        lower_expr(
            arg_expr,
            &mut *ctx_ptr,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )
    };
    let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_buffer_arg_signature_in_orc(&*ctx_ptr, local_array_aliases, arg_expr, callee_name)
    };
    let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_array_arg_signature_in_orc(&*ctx_ptr, local_array_aliases, arg_expr, callee_name)
    };
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let mut cast_scalar_arg = |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
        cast_orc_value_to(&*ctx_ptr, value, target_ty, arg_name)
    };
    let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                arg_expr: &Expr,
                                struct_name: &str,
                                by_ref: bool| unsafe {
        lower_struct_call_args_in_orc(
            &mut *ctx_ptr,
            arg_values,
            arg_expr,
            struct_name,
            name,
            by_ref,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )
    };
    let mut lower_struct_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                      arg_expr: &Expr,
                                      struct_name: &str| unsafe {
        lower_struct_array_call_args_in_orc(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_proc_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                    arg_expr: &Expr,
                                    struct_name: &str| unsafe {
        lower_proc_array_call_args_in_orc(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_array_arg =
        |arg_values: &mut Vec<LLVMValueRef>,
         arg_expr: &Expr,
         expected_elem_ty: Option<PrimitiveType>| unsafe {
            lower_array_call_args_in_orc(
                &mut *ctx_ptr,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
                arg_values,
                arg_expr,
                name,
                expected_elem_ty,
            )
        };
    let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
        lower_buffer_call_args_in_orc(
            &mut *ctx_ptr,
            local_array_aliases,
            local_tuples,
            arg_values,
            arg_expr,
            name,
            locals,
            local_aliases,
        )
    };
    let lowered = lower_user_call_common(
        shared,
        name,
        type_args,
        args,
        &mut lower_scalar_expr,
        &mut infer_buffer_arg_signature,
        &mut infer_array_arg_signature,
        "ORC tuple-from-call",
        b"tup_var_call_arg\0",
        &mut cast_scalar_arg,
        &mut lower_struct_arg,
        &mut lower_struct_array_arg,
        &mut lower_proc_array_arg,
        &mut lower_array_arg,
        &mut lower_buffer_arg,
        b"tup_var_call\0",
    )?;
    // Use the actual return type from the monomorphized function, not the
    // pre-looked-up elem_tys which may come from the base (unspecialized) def.
    let elem_tys = match &lowered.ret_ty {
        ReturnType::Tuple(tys) => tys.clone(),
        _ => elem_tys.to_vec(),
    };
    let elem_tys = &elem_tys;
    for (i, elem_ty) in elem_tys.iter().enumerate() {
        let elem_val = LLVMBuildExtractValue(
            ctx.builder,
            lowered.call,
            i as u32,
            b"tup_var_elem\0".as_ptr().cast(),
        );
        let flat_name = format!("{dest_name}.__{i}");
        if let Some(slot) = local_aliases.get(&flat_name) {
            let casted = cast_orc_value_to(
                ctx,
                OrcValue {
                    value: elem_val,
                    ty: *elem_ty,
                },
                slot.ty,
                b"tup_var_cast\0",
            );
            LLVMBuildStore(ctx.builder, casted, slot.ptr);
        } else {
            let slot = build_local_slot(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, *elem_ty),
                &format!("v_{flat_name}"),
            )?;
            LLVMBuildStore(ctx.builder, elem_val, slot);
            local_aliases.insert(
                flat_name,
                AliasSlot {
                    ptr: slot,
                    ty: *elem_ty,
                },
            );
        }
    }
    local_tuples.insert(dest_name.to_string(), elem_tys.to_vec());
    Ok(())
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
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    if ctx.const_arrays.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "cannot assign to const array '{base}' in ORC lowering"
        )));
    }
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
        local_tuples,
        &dst_expr,
        "slice assignment target",
        None,
    )?;
    let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, dst_view.elem_ty);

    if matches!(expr, Expr::Var { .. } | Expr::Slice { .. }) {
        let src_view = lower_orc_array_view(
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
            expr,
            "slice assignment source",
            None,
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

    let typed = lower_expr(
        expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    )?;
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

/// Lower tuple destructuring assignment `(a, b) = expr` in ORC (sample/block) scope.
unsafe fn lower_orc_tuple_destructure(
    targets: &[String],
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &mut HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Tuple { values, .. } => {
            if targets.len() != values.len() {
                return Err(Diagnostic::internal(format!(
                    "tuple destructuring arity mismatch: {} targets, {} elements",
                    targets.len(),
                    values.len()
                )));
            }
            for (target_name, val_expr) in targets.iter().zip(values.iter()) {
                let typed = lower_expr(
                    val_expr,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                )?;
                if let Some(slot) = local_aliases.get(target_name) {
                    let casted = cast_orc_value_to(ctx, typed, slot.ty, b"tup_destr_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                } else {
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, typed.ty),
                        &format!("v_{target_name}"),
                    )?;
                    LLVMBuildStore(ctx.builder, typed.value, slot);
                    local_aliases.insert(
                        target_name.clone(),
                        AliasSlot {
                            ptr: slot,
                            ty: typed.ty,
                        },
                    );
                }
            }
            Ok(())
        }
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            let shared = orc_user_call_context(ctx);
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            let mut lower_scalar_expr = |arg_expr: &Expr| unsafe {
                lower_expr(
                    arg_expr,
                    &mut *ctx_ptr,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    local_tuples,
                )
            };
            let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_buffer_arg_signature_in_orc(
                    &*ctx_ptr,
                    local_array_aliases,
                    arg_expr,
                    callee_name,
                )
            };
            let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_array_arg_signature_in_orc(
                    &*ctx_ptr,
                    local_array_aliases,
                    arg_expr,
                    callee_name,
                )
            };
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            let mut cast_scalar_arg =
                |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
                    cast_orc_value_to(&*ctx_ptr, value, target_ty, arg_name)
                };
            let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                        arg_expr: &Expr,
                                        struct_name: &str,
                                        by_ref: bool| {
                unsafe {
                    lower_struct_call_args_in_orc(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                        by_ref,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        local_tuples,
                    )
                }
            };
            let mut lower_struct_array_arg =
                |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr, struct_name: &str| unsafe {
                    lower_struct_array_call_args_in_orc(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                    )
                };
            let mut lower_proc_array_arg =
                |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr, struct_name: &str| unsafe {
                    lower_proc_array_call_args_in_orc(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                    )
                };
            let mut lower_array_arg =
                |arg_values: &mut Vec<LLVMValueRef>,
                 arg_expr: &Expr,
                 expected_elem_ty: Option<PrimitiveType>| unsafe {
                    lower_array_call_args_in_orc(
                        &mut *ctx_ptr,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        local_tuples,
                        arg_values,
                        arg_expr,
                        name,
                        expected_elem_ty,
                    )
                };
            let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
                lower_buffer_call_args_in_orc(
                    &mut *ctx_ptr,
                    local_array_aliases,
                    local_tuples,
                    arg_values,
                    arg_expr,
                    name,
                    locals,
                    local_aliases,
                )
            };
            let lowered = lower_user_call_common(
                shared,
                name,
                type_args,
                args,
                &mut lower_scalar_expr,
                &mut infer_buffer_arg_signature,
                &mut infer_array_arg_signature,
                "ORC tuple destructure",
                b"tup_call_arg\0",
                &mut cast_scalar_arg,
                &mut lower_struct_arg,
                &mut lower_struct_array_arg,
                &mut lower_proc_array_arg,
                &mut lower_array_arg,
                &mut lower_buffer_arg,
                b"tup_call\0",
            )?;
            let ReturnType::Tuple(elem_tys) = &lowered.ret_ty else {
                return Err(Diagnostic::internal(
                    "expected tuple return from user call in ORC destructure",
                ));
            };
            let elem_tys = elem_tys.clone();
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
                    lowered.call,
                    i as u32,
                    b"tup_elem\0".as_ptr().cast(),
                );
                let elem_orc = OrcValue {
                    value: elem_val,
                    ty: *elem_ty,
                };
                if let Some(slot) = local_aliases.get(target_name) {
                    let casted = cast_orc_value_to(ctx, elem_orc, slot.ty, b"tup_destr_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                } else {
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, *elem_ty),
                        &format!("v_{target_name}"),
                    )?;
                    LLVMBuildStore(ctx.builder, elem_val, slot);
                    local_aliases.insert(
                        target_name.clone(),
                        AliasSlot {
                            ptr: slot,
                            ty: *elem_ty,
                        },
                    );
                }
            }
            Ok(())
        }
        Expr::Var { name: var_name, .. } => {
            // Tuple variable — read each element from local aliases (var.__0, var.__1, ...)
            let elem_tys = local_tuples.get(var_name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "tuple destructuring: '{var_name}' is not a known tuple variable"
                ))
            })?;
            let elem_tys = elem_tys.clone();
            if targets.len() != elem_tys.len() {
                return Err(Diagnostic::internal(format!(
                    "tuple destructuring arity mismatch: {} targets, {} elements",
                    targets.len(),
                    elem_tys.len()
                )));
            }
            for (i, (target_name, elem_ty)) in targets.iter().zip(elem_tys.iter()).enumerate() {
                let alias_key = format!("{var_name}.__{i}");
                let src_slot = local_aliases.get(&alias_key).ok_or_else(|| {
                    Diagnostic::internal(format!("tuple destructure: missing alias '{alias_key}'"))
                })?;
                let val = LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, src_slot.ty),
                    src_slot.ptr,
                    b"tup_destr_ld\0".as_ptr().cast(),
                );
                let elem_orc = OrcValue {
                    value: val,
                    ty: src_slot.ty,
                };
                if let Some(slot) = local_aliases.get(target_name) {
                    let casted = cast_orc_value_to(ctx, elem_orc, slot.ty, b"tup_destr_cast\0");
                    LLVMBuildStore(ctx.builder, casted, slot.ptr);
                } else {
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, *elem_ty),
                        &format!("v_{target_name}"),
                    )?;
                    LLVMBuildStore(ctx.builder, val, slot);
                    local_aliases.insert(
                        target_name.clone(),
                        AliasSlot {
                            ptr: slot,
                            ty: *elem_ty,
                        },
                    );
                }
            }
            Ok(())
        }
        _ => Err(Diagnostic::internal(
            "tuple destructuring requires a tuple literal, tuple variable, or tuple-returning call",
        )),
    }
}
