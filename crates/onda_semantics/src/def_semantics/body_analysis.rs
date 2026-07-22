use std::cell::RefCell;

use crate::*;

fn infer_def_slice_alias_info(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    param_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_slice_alias_info(
        base,
        start,
        end,
        declared_symbols,
        None,
        local_array_aliases,
        param_structs,
        struct_defs,
        errors,
    )
}

fn infer_def_data_like_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    param_structs: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_data_like_info(
        expr,
        declared_symbols,
        None,
        local_array_aliases,
        param_structs,
        struct_defs,
        errors,
    )
}

#[derive(Clone, Copy)]
pub(crate) struct DefStmtAnalysisCtx<'a> {
    pub common: ScopeAnalysisCtx<'a>,
    pub locals: &'a HashSet<String>,
    pub declared_symbols: &'a DeclaredSymbolMap,
    pub param_structs: &'a HashMap<String, String>,
    pub proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    pub state_scalars: &'a HashMap<String, PrimitiveType>,
    pub def_return_types: &'a HashMap<String, ReturnType>,
    pub resolved_scalar_locals: &'a RefCell<LocalAliasTypes>,
}

pub(crate) type DefStmtAnalysisState = ScopeFlowState;

pub(crate) fn analyze_def_stmt(
    stmt: &Stmt,
    ctx: DefStmtAnalysisCtx<'_>,
    st: &mut DefStmtAnalysisState,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, |_stmt_diag| {
        let known_scalars = &mut st.known_scalars;
        let local_aliases = &mut st.local_aliases;
        let local_array_aliases = &mut st.local_array_aliases;
        let local_proc_aliases = &mut st.local_proc_aliases;
        let tuple_vars = &mut st.tuple_vars;
        let common = ctx.common;
        let locals = ctx.locals;
        let declared_symbols = ctx.declared_symbols;
        let param_structs = ctx.param_structs;
        let proc_array_roots = ctx.proc_array_roots;
        let state_scalars = ctx.state_scalars;
        let input_names = common.input_names;
        let output_names = common.output_names;
        let param_names = common.param_names;
        let struct_defs = common.struct_defs;
        let def_return_types = ctx.def_return_types;
        let resolved_scalar_locals = ctx.resolved_scalar_locals;
        let options = common.options;
        let empty_data = HashMap::<String, usize>::new();
        // In def analysis, struct-typed parameters (for example `self`) should be
        // visible to expression type inference, including indexed array field reads.
        let struct_instance_ctx = param_structs;
        let empty_outputs = HashSet::<String>::new();
        let array_vars = merged_data_vars_for_runtime(&empty_data, local_array_aliases);
        let expr_inputs = build_scope_analysis_expr_inputs(
            common,
            locals,
            state_scalars,
            declared_symbols,
            param_structs,
            struct_instance_ctx,
            &empty_outputs,
            proc_array_roots,
        );
        macro_rules! stmt_expr_env {
            ($scope:expr) => {
                build_scope_stmt_expr_env_with_tuples(
                    expr_inputs,
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    &array_vars,
                    $scope,
                    tuple_vars,
                )
            };
        }
        macro_rules! scope_expr_env {
            ($scope:expr) => {{
                let mut env = build_scope_expr_env_with_tuples(
                    expr_inputs,
                    known_scalars,
                    local_aliases,
                    &array_vars,
                    $scope,
                    tuple_vars,
                );
                env.local_array_aliases = local_array_aliases;
                env
            }};
        }
        match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign {
                target_loc,
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => with_loc_diag_context(target_loc.as_ref(), |target_diag| match target {
                AssignTarget::Var(name) => {
                    if !matches!(expr, Expr::Index { .. }) {
                        local_proc_aliases.remove(name);
                    }
                    if !validate_block_bound_surface_var_name(
                        name,
                        target_loc.as_ref().into(),
                        scope_expr_env!(ScopeKind::Def),
                        errors,
                    ) {
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        return;
                    }
                    let declared_ty = *decl_ty;
                    if let Expr::ArrayCtor { spec, init, .. } = expr {
                        if *is_typed_decl {
                            if declared_ty.is_some() {
                                with_expr_diag_context(expr, |expr_diag| {
                                    push_semantic(
                                        expr_diag,
                                        errors,
                                        "typed declaration cannot combine scalar type annotation with array constructor",
                                    );
                                });
                                return;
                            }
                            if split_field_path(name, errors).is_some() {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    "typed array declaration target must be a plain variable name",
                                );
                                return;
                            }
                            if known_scalars.contains(name)
                                || local_array_aliases.contains_key(name)
                                || input_names.contains(name)
                                || output_names.contains(name)
                                || param_names.contains(name)
                                || state_scalars.contains_key(name)
                            {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "typed array declaration for '{name}' conflicts with existing symbol"
                                    ),
                                );
                                return;
                            }
                            let size_context =
                                format!("typed array declaration size for symbol '{name}' in def");
                            let Some(size_value) = with_expr_diag_context(&spec.size, |_diag| {
                                eval_data_size_expr(&spec.size, options, &size_context, errors)
                            }) else {
                                return;
                            };
                            match &spec.elem {
                                ArrayElemType::Primitive(elem_ty) => {
                                    local_array_aliases.insert(
                                        name.clone(),
                                        LocalArrayAliasInfo {
                                            len: size_value,
                                            elem_ty: *elem_ty,
                                            elem_struct: None,
                                            writable: true,
                                        },
                                    );
                                    if let Some(values) = init {
                                        if values.len() != size_value {
                                            with_expr_diag_context(expr, |expr_diag| {
                                                push_semantic(
                                                    expr_diag,
                                                    errors,
                                                    format!(
                                                        "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                                        values.len()
                                                    ),
                                                );
                                            });
                                        }
                                        for (idx, value) in
                                            values.iter().take(size_value).enumerate()
                                        {
                                            validate_expr(
                                                value,
                                                scope_expr_env!(ScopeKind::Def),
                                                errors,
                                            );
                                            let value_ty =
                                                infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                                    value,
                                                    state_scalars,
                                                    declared_symbols,
                                                    None,
                                                    local_aliases,
                                                    local_array_aliases,
                                                    locals,
                                                    input_names,
                                                    output_names,
                                                    param_names,
                                                    struct_instance_ctx,
                                                    struct_defs,
                                                    proc_array_roots,
                                                    errors,
                                                );
                                            require_expr_assignable_type(
                                                value,
                                                value_ty,
                                                *elem_ty,
                                                &format!(
                                                    "typed array initializer assignment to '{name}[{idx}]'"
                                                ),
                                                errors,
                                            );
                                        }
                                    }
                                }
                                ArrayElemType::Struct(struct_name) => {
                                    with_expr_diag_context(expr, |expr_diag| {
                                        push_semantic(
                                            expr_diag,
                                            errors,
                                            format!(
                                                "typed array declaration '{name}: {struct_name}[N]' is not yet supported in def blocks"
                                            ),
                                        );
                                    });
                                }
                            }
                            return;
                        } else {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    "array[...] construction is only allowed in init or typed array declarations",
                                );
                            });
                            return;
                        }
                    }
                    if let Expr::ArrayLiteral { values, .. } = expr {
                        if declared_ty.is_some() {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "typed declaration for '{name}' with array literals must use explicit array type syntax like '{name}: T[N] = [...]'"
                                ),
                            );
                            return;
                        }
                        if split_field_path(name, errors).is_some() {
                            push_semantic(
                                target_diag,
                                errors,
                                "array declaration target must be a plain variable name",
                            );
                            return;
                        }
                        if known_scalars.contains(name)
                            || local_aliases.contains_key(name)
                            || local_array_aliases.contains_key(name)
                            || input_names.contains(name)
                            || output_names.contains(name)
                            || param_names.contains(name)
                            || state_scalars.contains_key(name)
                        {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "array declaration for '{name}' conflicts with existing symbol"
                                ),
                            );
                            return;
                        }
                        if values.is_empty() {
                            with_expr_diag_context(expr, |expr_diag| {
                                push_semantic(
                                    expr_diag,
                                    errors,
                                    format!(
                                        "array initializer for symbol '{name}' cannot be empty"
                                    ),
                                );
                            });
                            return;
                        }
                        for value in values {
                            validate_expr(value, scope_expr_env!(ScopeKind::Def), errors);
                        }
                        let inferred_first =
                            infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                &values[0],
                                state_scalars,
                                declared_symbols,
                                None,
                                local_aliases,
                                local_array_aliases,
                                locals,
                                input_names,
                                output_names,
                                param_names,
                                struct_instance_ctx,
                                struct_defs,
                                proc_array_roots,
                                errors,
                            );
                        let elem_ty = untyped_literal_type(&values[0])
                            .or(inferred_first)
                            .unwrap_or(PrimitiveType::F32);
                        for (idx, value) in values.iter().enumerate() {
                            let value_ty =
                                infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                    value,
                                    state_scalars,
                                    declared_symbols,
                                    None,
                                    local_aliases,
                                    local_array_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instance_ctx,
                                    struct_defs,
                                    proc_array_roots,
                                    errors,
                                );
                            require_expr_assignable_type(
                                value,
                                value_ty,
                                elem_ty,
                                &format!("array initializer assignment to '{name}[{idx}]'"),
                                errors,
                            );
                        }
                        local_array_aliases.insert(
                            name.clone(),
                            LocalArrayAliasInfo {
                                len: values.len(),
                                elem_ty,
                                elem_struct: None,
                                writable: true,
                            },
                        );
                        return;
                    }
                    if let Expr::Slice {
                        base, start, end, ..
                    } = expr
                    {
                        if let Some(surface) =
                            dynamic_param_surface_name(base, scope_expr_env!(ScopeKind::Def))
                        {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "dynamic param array '{surface}' is not a first-class value; use '{surface}[i]' directly in block or sample"
                                ),
                            );
                            if let Some(start) = start {
                                validate_expr(start, scope_expr_env!(ScopeKind::Def), errors);
                            }
                            if let Some(end) = end {
                                validate_expr(end, scope_expr_env!(ScopeKind::Def), errors);
                            }
                            return;
                        }
                        if declared_ty.is_some() || *is_typed_decl {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "typed declaration for '{name}' is not supported for slice aliases"
                                ),
                            );
                            return;
                        }
                        if split_field_path(name, errors).is_some() {
                            push_semantic(
                                target_diag,
                                errors,
                                "slice alias target must be a plain variable name",
                            );
                            return;
                        }
                        if known_scalars.contains(name)
                            || local_aliases.contains_key(name)
                            || local_array_aliases.contains_key(name)
                            || input_names.contains(name)
                            || output_names.contains(name)
                            || param_names.contains(name)
                            || state_scalars.contains_key(name)
                        {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "slice alias declaration for '{name}' conflicts with existing symbol"
                                ),
                            );
                            return;
                        }
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        if let Some(alias) = infer_def_slice_alias_info(
                            base,
                            start.as_deref(),
                            end.as_deref(),
                            declared_symbols,
                            local_array_aliases,
                            param_structs,
                            struct_defs,
                            errors,
                        ) {
                            local_array_aliases.insert(name.clone(), alias);
                        }
                        return;
                    }
                    if local_aliases.contains_key(name) {
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            expr,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        require_expr_assignable_type(
                            expr,
                            expr_ty,
                            *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                            &format!("alias assignment to '{name}'"),
                            errors,
                        );
                        let tuple_arity =
                            infer_tracked_tuple_arity(expr, tuple_vars, def_return_types);
                        track_tuple_var_assignment(tuple_vars, name, tuple_arity);
                        known_scalars.insert(name.clone());
                        return;
                    }
                    if local_array_aliases.contains_key(name) {
                        push_semantic(
                            target_diag,
                            errors,
                            format!(
                                "array alias '{name}' must be written using '{name}[index] = value'"
                            ),
                        );
                        return;
                    }
                    if let Some((base, field)) = split_field_path(name, errors) {
                        if declared_ty.is_some() {
                            push_semantic(
                                target_diag,
                                errors,
                                "typed declaration is only supported for plain scalar variables",
                            );
                        }
                        let Some(struct_name) = param_structs.get(base) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "invalid assignment target '{name}' in def block; only struct parameters can be assigned via fields"
                                ),
                            );
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!("unknown struct type '{}'", struct_name),
                            );
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    base, struct_name, field
                                ),
                            );
                            return;
                        };
                        let expr_for_validation =
                            rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                        match field_decl.ty {
                            TypedFieldType::Scalar(prim) => {
                                let expr_error_count_before = errors.len();
                                validate_expr(
                                    &expr_for_validation,
                                    scope_expr_env!(ScopeKind::Def),
                                    errors,
                                );
                                let expr_ty =
                                    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                        &expr_for_validation,
                                        state_scalars,
                                        declared_symbols,
                                        None,
                                        local_aliases,
                                        local_array_aliases,
                                        locals,
                                        input_names,
                                        output_names,
                                        param_names,
                                        struct_instance_ctx,
                                        struct_defs,
                                        proc_array_roots,
                                        errors,
                                    );
                                let had_expr_validation_error =
                                    errors.len() > expr_error_count_before;
                                let suppress_type_mismatch =
                                    is_invalid_placeholder_symbol(declared_symbols, field)
                                        || is_invalid_placeholder_symbol(
                                            declared_symbols,
                                            &format!("{base}.{field}"),
                                        );
                                if !suppress_type_mismatch
                                    && !had_expr_validation_error
                                    && !has_use_before_declaration_error(errors)
                                {
                                    require_expr_assignable_type(
                                        &expr_for_validation,
                                        expr_ty,
                                        prim,
                                        &format!("def assignment to '{}.{}'", base, field),
                                        errors,
                                    );
                                }
                            }
                            TypedFieldType::Array(_) => {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "array field '{}.{}' must be assigned via index syntax",
                                        base, field
                                    ),
                                );
                            }
                            TypedFieldType::Struct => {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "nested struct field '{}.{}' must be assigned via subfields or constructor",
                                        base, field
                                    ),
                                );
                            }
                            TypedFieldType::Tuple(_) => {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "tuple field '{}.{}' must be assigned via index syntax",
                                        base, field
                                    ),
                                );
                            }
                        }
                        return;
                    }
                    if !known_scalars.contains(name)
                        && !local_aliases.contains_key(name)
                        && !local_array_aliases.contains_key(name)
                        && !input_names.contains(name)
                        && !output_names.contains(name)
                        && !param_names.contains(name)
                        && !state_scalars.contains_key(name)
                    {
                        if let Expr::Index { base, index, .. } = expr {
                            if let Some(binding_kind) = classify_def_indexed_binding(
                                base,
                                local_array_aliases,
                                param_structs,
                                proc_array_roots,
                                state_scalars,
                                struct_defs,
                                errors,
                            ) {
                                validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                                let idx_ty =
                                    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                        index,
                                        state_scalars,
                                        declared_symbols,
                                        None,
                                        local_aliases,
                                        local_array_aliases,
                                        locals,
                                        input_names,
                                        output_names,
                                        param_names,
                                        struct_instance_ctx,
                                        struct_defs,
                                        proc_array_roots,
                                        errors,
                                    );
                                require_expr_numeric_type(
                                    index,
                                    idx_ty,
                                    "array index expression",
                                    errors,
                                );
                                match binding_kind {
                                    IndexedBindingKind::ProcArrayAlias => {
                                        local_proc_aliases.insert(
                                            name.clone(),
                                            ProcArrayAliasInfo {
                                                array_base: base.clone(),
                                                index_expr: index.as_ref().clone(),
                                            },
                                        );
                                        return;
                                    }
                                    IndexedBindingKind::StructElementAlias(struct_name) => {
                                        if !add_struct_element_alias_bindings(
                                            name,
                                            &struct_name,
                                            struct_defs,
                                            known_scalars,
                                            local_aliases,
                                            local_array_aliases,
                                            &format!("array alias '{name}' from '{base}[...]'"),
                                            errors,
                                        ) {
                                            return;
                                        }
                                        return;
                                    }
                                    IndexedBindingKind::PrimitiveScalar => {
                                        // Primitive array indexed reads are scalar expressions.
                                        // Allow normal first-assignment local inference to handle:
                                        //   x = arr[idx]
                                    }
                                }
                            }
                        }
                    }
                    if known_scalars.contains(name) && declared_ty.is_some() {
                        push_semantic(
                            target_diag,
                            errors,
                            format!(
                            "typed declaration for '{name}' is only allowed on first assignment"
                        ),
                        );
                    }
                    if local_array_aliases.contains_key(name) {
                        push_semantic(
                            target_diag,
                            errors,
                            format!(
                                "array alias '{name}' must be written using '{name}[index] = value'"
                            ),
                        );
                        return;
                    }
                    if is_builtin_constant_name(name) {
                        push_semantic(
                            target_diag,
                            errors,
                            format!("cannot assign to builtin constant '{name}'"),
                        );
                    }
                    if locals.contains(name) {
                        push_semantic(
                            target_diag,
                            errors,
                            format!("cannot assign to loop variable '{name}'"),
                        );
                    }
                    if input_names.contains(name)
                        || output_names.contains(name)
                        || param_names.contains(name)
                        || state_scalars.contains_key(name)
                    {
                        push_semantic(
                            target_diag,
                            errors,
                            format!("cannot assign to global symbol '{name}' inside def"),
                        );
                    }
                    let expr_error_count_before = errors.len();
                    let expr_for_validation =
                        rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                    validate_expr(
                        &expr_for_validation,
                        scope_expr_env!(ScopeKind::Def),
                        errors,
                    );
                    let had_expr_validation_error = errors.len() > expr_error_count_before;
                    let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                        &expr_for_validation,
                        state_scalars,
                        declared_symbols,
                        None,
                        local_aliases,
                        local_array_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        struct_instance_ctx,
                        struct_defs,
                        proc_array_roots,
                        errors,
                    );
                    let can_track_local = !input_names.contains(name)
                        && !output_names.contains(name)
                        && !param_names.contains(name)
                        && !state_scalars.contains_key(name);
                    let target_ty = if let Some(declared) = declared_ty {
                        declared
                    } else if let Some(existing) = local_aliases.get(name).copied() {
                        existing
                    } else {
                        let untyped_ty = effective_untyped_assignment_type(expr, expr_ty);
                        untyped_ty.unwrap_or(PrimitiveType::F32)
                    };
                    let suppress_type_mismatch =
                        is_invalid_placeholder_symbol(declared_symbols, name);
                    if !suppress_type_mismatch
                        && !had_expr_validation_error
                        && !has_use_before_declaration_error(errors)
                    {
                        require_expr_assignable_type(
                            &expr_for_validation,
                            expr_ty,
                            target_ty,
                            &format!("def assignment to '{name}'"),
                            errors,
                        );
                    }
                    // Track tuple variables (assigned from tuple literal or
                    // tuple-returning call) for indexing validation
                    let tuple_arity = infer_tracked_tuple_arity(expr, tuple_vars, def_return_types);
                    track_tuple_var_assignment(tuple_vars, name, tuple_arity);
                    if can_track_local {
                        local_aliases.entry(name.clone()).or_insert(target_ty);
                        resolved_scalar_locals
                            .borrow_mut()
                            .entry(name.clone())
                            .or_insert(target_ty);
                    }
                    known_scalars.insert(name.clone());
                }
                AssignTarget::Index { base, index } => {
                    if decl_ty.is_some() || generic_decl_ty.is_some() || *is_typed_decl {
                        push_semantic(
                            target_diag,
                            errors,
                            "typed declaration is only supported for plain scalar variables",
                        );
                    }
                    if let Some(name) = io_surface_name(base, scope_expr_env!(ScopeKind::Def)) {
                        push_io_surface_scope_error(errors, target_loc.as_ref().into(), name);
                        validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        return;
                    }
                    if let Some(name) =
                        dynamic_param_surface_name(base, scope_expr_env!(ScopeKind::Def))
                    {
                        push_semantic(
                            target_diag,
                            errors,
                            format!(
                                "dynamic param indexing '{name}[...]' is only allowed in block or sample"
                            ),
                        );
                        validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        return;
                    }
                    if let Some(alias) = local_array_aliases.get(base) {
                        if !alias.writable {
                            push_semantic(
                                target_diag,
                                errors,
                                format!("cannot assign to immutable array alias '{base}'"),
                            );
                            return;
                        }
                        if alias.elem_struct.is_some() {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "indexed assignment target '{base}[...]' has struct elements; assign fields through an alias"
                                ),
                            );
                            return;
                        }
                        validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        let index_ty =
                            infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                index,
                                state_scalars,
                                declared_symbols,
                                None,
                                local_aliases,
                                local_array_aliases,
                                locals,
                                input_names,
                                output_names,
                                param_names,
                                struct_instance_ctx,
                                struct_defs,
                                proc_array_roots,
                                errors,
                            );
                        require_expr_numeric_type(
                            index,
                            index_ty,
                            "array index expression",
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            expr,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        require_expr_assignable_type(
                            expr,
                            expr_ty,
                            alias.elem_ty,
                            "array/buffer write",
                            errors,
                        );
                        return;
                    }
                    if has_declared_buffer_symbol_info(declared_symbols, base) {
                        if is_declared_multichannel_buffer_info(declared_symbols, base) {
                            push_semantic(
                            target_diag,
                            errors,
                            format!(
                                "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                            ),
                        );
                        }
                        validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        let index_ty =
                            infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                index,
                                state_scalars,
                                declared_symbols,
                                None,
                                local_aliases,
                                local_array_aliases,
                                locals,
                                input_names,
                                output_names,
                                param_names,
                                struct_instance_ctx,
                                struct_defs,
                                proc_array_roots,
                                errors,
                            );
                        require_expr_numeric_type(
                            index,
                            index_ty,
                            "array index expression",
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            expr,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        let expected_ty = declared_symbol_scalar_type(declared_symbols, base)
                            .unwrap_or(PrimitiveType::F32);
                        require_expr_assignable_type(
                            expr,
                            expr_ty,
                            expected_ty,
                            "array/buffer write",
                            errors,
                        );
                        return;
                    }
                    if let Some((root, field)) = split_field_path(base, errors) {
                        if let Some(proc_array) = proc_array_roots.get(root) {
                            let Some(field_decl) = resolve_struct_field_decl(
                                &proc_array.proc_name,
                                field,
                                struct_defs,
                            ) else {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "processor array parameter '{}' (type '{}') has no field '{}'",
                                        root, proc_array.proc_name, field
                                    ),
                                );
                                return;
                            };
                            let TypedFieldType::Scalar(expected_ty) = field_decl.ty else {
                                push_semantic(
                                    target_diag,
                                    errors,
                                    format!(
                                        "processor array field '{}.{}' must be scalar for indexed assignment; use an alias for non-scalar fields",
                                        root, field
                                    ),
                                );
                                return;
                            };
                            validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                            validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                            let index_ty =
                                infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                    index,
                                    state_scalars,
                                    declared_symbols,
                                    None,
                                    local_aliases,
                                    local_array_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instance_ctx,
                                    struct_defs,
                                    proc_array_roots,
                                    errors,
                                );
                            require_expr_numeric_type(
                                index,
                                index_ty,
                                "array index expression",
                                errors,
                            );
                            let expr_ty =
                                infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                    expr,
                                    state_scalars,
                                    declared_symbols,
                                    None,
                                    local_aliases,
                                    local_array_aliases,
                                    locals,
                                    input_names,
                                    output_names,
                                    param_names,
                                    struct_instance_ctx,
                                    struct_defs,
                                    proc_array_roots,
                                    errors,
                                );
                            require_expr_assignable_type(
                                expr,
                                expr_ty,
                                expected_ty,
                                "array/buffer write",
                                errors,
                            );
                            return;
                        }
                        let Some(struct_name) = param_structs.get(root) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                "indexed assignment target '{base}[...]' is invalid in def block"
                            ),
                            );
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!("unknown struct type '{}'", struct_name),
                            );
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                            );
                            return;
                        };
                        if !matches!(
                            field_decl.ty,
                            TypedFieldType::Array(_) | TypedFieldType::Tuple(_)
                        ) {
                            push_semantic(
                                target_diag,
                                errors,
                                format!(
                                    "field '{}.{}' is not array or tuple and cannot be indexed",
                                    root, field
                                ),
                            );
                        }
                        validate_expr(index, scope_expr_env!(ScopeKind::Def), errors);
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        let index_ty =
                            infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                index,
                                state_scalars,
                                declared_symbols,
                                None,
                                local_aliases,
                                local_array_aliases,
                                locals,
                                input_names,
                                output_names,
                                param_names,
                                struct_instance_ctx,
                                struct_defs,
                                proc_array_roots,
                                errors,
                            );
                        require_expr_numeric_type(
                            index,
                            index_ty,
                            "array index expression",
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            expr,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        let expected_elem_ty =
                            field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                        require_expr_assignable_type(
                            expr,
                            expr_ty,
                            expected_elem_ty,
                            "array/buffer write",
                            errors,
                        );
                        return;
                    }
                    push_semantic(
                        target_diag,
                        errors,
                        "indexed assignments in def are only allowed for mutable array or buffer references (for example local arrays, array params, buffer params, or array fields on struct params such as 'tmp[i] = x', 'arr[i] = x', 'buf[i] = x', or 'self.buf[i] = x')",
                    );
                }
                AssignTarget::Slice { base, start, end } => {
                    if decl_ty.is_some() || generic_decl_ty.is_some() || *is_typed_decl {
                        push_semantic(
                            target_diag,
                            errors,
                            "typed declaration is only supported for plain scalar variables",
                        );
                    }
                    if let Some(name) = io_surface_name(base, scope_expr_env!(ScopeKind::Def)) {
                        push_io_surface_scope_error(errors, target_loc.as_ref().into(), name);
                        if let Some(start) = start {
                            validate_expr(start, scope_expr_env!(ScopeKind::Def), errors);
                        }
                        if let Some(end) = end {
                            validate_expr(end, scope_expr_env!(ScopeKind::Def), errors);
                        }
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        return;
                    }
                    if let Some(name) =
                        dynamic_param_surface_name(base, scope_expr_env!(ScopeKind::Def))
                    {
                        push_semantic(
                            target_diag,
                            errors,
                            format!(
                                "dynamic param array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
                            ),
                        );
                        if let Some(start) = start {
                            validate_expr(start, scope_expr_env!(ScopeKind::Def), errors);
                        }
                        if let Some(end) = end {
                            validate_expr(end, scope_expr_env!(ScopeKind::Def), errors);
                        }
                        validate_expr(expr, scope_expr_env!(ScopeKind::Def), errors);
                        return;
                    }
                    let Some(target_info) = infer_def_slice_alias_info(
                        base,
                        start.as_ref(),
                        end.as_ref(),
                        declared_symbols,
                        local_array_aliases,
                        param_structs,
                        struct_defs,
                        errors,
                    ) else {
                        return;
                    };
                    if !target_info.writable {
                        push_semantic(
                            target_diag,
                            errors,
                            format!("cannot assign to immutable array alias '{base}'"),
                        );
                        return;
                    }
                    let slice_env = scope_expr_env!(ScopeKind::Def);
                    if let Some(start) = start {
                        validate_expr(start, slice_env, errors);
                        let start_ty =
                            infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                start,
                                state_scalars,
                                declared_symbols,
                                None,
                                local_aliases,
                                local_array_aliases,
                                locals,
                                input_names,
                                output_names,
                                param_names,
                                struct_instance_ctx,
                                struct_defs,
                                proc_array_roots,
                                errors,
                            );
                        require_expr_numeric_type(start, start_ty, "slice start bound", errors);
                    }
                    if let Some(end) = end {
                        validate_expr(end, slice_env, errors);
                        let end_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            end,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        require_expr_numeric_type(end, end_ty, "slice end bound", errors);
                    }
                    let stmt_env = stmt_expr_env!(ScopeKind::Def);
                    if is_data_like_value_expr(expr, stmt_env) {
                        validate_data_like_value_expr(expr, stmt_env, errors);
                        if let Some(src_info) = infer_def_data_like_info(
                            expr,
                            declared_symbols,
                            local_array_aliases,
                            param_structs,
                            struct_defs,
                            errors,
                        ) {
                            require_expr_assignable_type(
                                expr,
                                Some(src_info.elem_ty),
                                target_info.elem_ty,
                                "slice copy assignment",
                                errors,
                            );
                        }
                    } else {
                        validate_expr(expr, slice_env, errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                            expr,
                            state_scalars,
                            declared_symbols,
                            None,
                            local_aliases,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            proc_array_roots,
                            errors,
                        );
                        require_expr_assignable_type(
                            expr,
                            expr_ty,
                            target_info.elem_ty,
                            "slice fill assignment",
                            errors,
                        );
                    }
                }
                AssignTarget::Tuple(targets) => {
                    // Validate the RHS expression
                    analyze_stmt_expr(expr, stmt_expr_env!(ScopeKind::Def), errors);
                    let mut targets_ok = true;
                    for target_name in targets {
                        targets_ok &= validate_block_bound_surface_var_name(
                            target_name,
                            target_loc.as_ref().into(),
                            scope_expr_env!(ScopeKind::Def),
                            errors,
                        );
                    }
                    // Validate destructuring arity against the RHS tuple length
                    let rhs_arity = infer_tracked_tuple_arity(expr, tuple_vars, def_return_types);
                    if let Some(expected) = rhs_arity {
                        if targets.len() != expected {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "tuple destructuring has {} targets but the right-hand side has {} elements",
                                    targets.len(),
                                    expected,
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    if !targets_ok {
                        return;
                    }
                    let destructured_types = match expr {
                        Expr::UserCall { name, .. } => {
                            match def_return_types.get(name.as_str()) {
                                Some(ReturnType::Tuple(types)) => Some(types.clone()),
                                _ => None,
                            }
                        }
                        Expr::Tuple { values, .. } => Some(
                            values
                                .iter()
                                .map(|value| {
                                    let inferred =
                                        infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
                                            value,
                                            state_scalars,
                                            declared_symbols,
                                            None,
                                            local_aliases,
                                            local_array_aliases,
                                            locals,
                                            input_names,
                                            output_names,
                                            param_names,
                                            struct_instance_ctx,
                                            struct_defs,
                                            proc_array_roots,
                                            errors,
                                        );
                                    effective_untyped_assignment_type(value, inferred)
                                        .unwrap_or(PrimitiveType::F32)
                                })
                                .collect(),
                        ),
                        _ => None,
                    };
                    clear_tuple_var_bindings(tuple_vars, targets.iter());
                    // Register each destructured target as a known scalar
                    for (index, target_name) in targets.iter().enumerate() {
                        let target_ty = destructured_types
                            .as_ref()
                            .and_then(|types| types.get(index))
                            .copied()
                            .unwrap_or(PrimitiveType::F32);
                        known_scalars.insert(target_name.clone());
                        local_aliases
                            .entry(target_name.clone())
                            .or_insert(target_ty);
                        resolved_scalar_locals
                            .borrow_mut()
                            .entry(target_name.clone())
                            .or_insert(target_ty);
                    }
                }
            }),
            Stmt::Expr { expr, .. } => {
                let expr = rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                analyze_stmt_expr(&expr, stmt_expr_env!(ScopeKind::Def), errors);
            }
            Stmt::Return { expr, .. } => {
                let expr = rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                analyze_stmt_expr(&expr, stmt_expr_env!(ScopeKind::Def), errors);
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                require_validated_bool_stmt_expr(
                    cond,
                    "if condition",
                    stmt_expr_env!(ScopeKind::Def),
                    errors,
                );
                let mut then_state = fork_scope_flow_state_with_tuples(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                );
                for nested in then_branch {
                    analyze_def_stmt(nested, ctx, &mut then_state, loop_depth, errors);
                }
                let mut else_state = fork_scope_flow_state_with_tuples(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                );
                for nested in else_branch {
                    analyze_def_stmt(nested, ctx, &mut else_state, loop_depth, errors);
                }
                merge_branch_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                    then_state,
                    else_state,
                );
            }
            Stmt::For {
                var,
                step,
                start,
                end,
                body,
                ..
            } => {
                require_validated_numeric_stmt_expr(
                    start,
                    "for loop start bound",
                    stmt_expr_env!(ScopeKind::Def),
                    errors,
                );
                require_validated_numeric_stmt_expr(
                    end,
                    "for loop end bound",
                    stmt_expr_env!(ScopeKind::Def),
                    errors,
                );
                validate_for_loop_step_expr(step.as_ref(), stmt_expr_env!(ScopeKind::Def), errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_state = fork_scope_flow_state_with_tuples(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                );
                let loop_ctx = DefStmtAnalysisCtx {
                    locals: &loop_locals,
                    ..ctx
                };
                for nested in body {
                    analyze_def_stmt(nested, loop_ctx, &mut loop_state, loop_depth + 1, errors);
                }
                adopt_loop_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                    loop_state,
                );
            }
            Stmt::While { cond, body, .. } => {
                require_validated_bool_stmt_expr(
                    cond,
                    "while condition",
                    stmt_expr_env!(ScopeKind::Def),
                    errors,
                );
                let mut loop_state = fork_scope_flow_state_with_tuples(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                );
                for nested in body {
                    analyze_def_stmt(nested, ctx, &mut loop_state, loop_depth + 1, errors);
                }
                adopt_loop_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    tuple_vars,
                    loop_state,
                );
            }
            Stmt::Break { .. } => require_loop_control_context("break", loop_depth, errors),
            Stmt::Continue { .. } => require_loop_control_context("continue", loop_depth, errors),
        }
    });
}
