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
    pub state_scalars: &'a HashMap<String, PrimitiveType>,
}

pub(crate) type DefStmtAnalysisState = ScopeFlowState;

pub(crate) fn analyze_def_stmt(
    stmt: &Stmt,
    ctx: DefStmtAnalysisCtx<'_>,
    st: &mut DefStmtAnalysisState,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let known_scalars = &mut st.known_scalars;
        let local_aliases = &mut st.local_aliases;
        let local_array_aliases = &mut st.local_array_aliases;
        let local_proc_aliases = &mut st.local_proc_aliases;
        let common = ctx.common;
        let locals = ctx.locals;
        let declared_symbols = ctx.declared_symbols;
        let param_structs = ctx.param_structs;
        let state_scalars = ctx.state_scalars;
        let input_names = common.input_names;
        let output_names = common.output_names;
        let param_names = common.param_names;
        let struct_defs = common.struct_defs;
        let fn_signatures = common.fn_signatures;
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
        );
        let stmt_expr_env = |scope| {
            build_scope_stmt_expr_env(
                expr_inputs,
                known_scalars,
                local_aliases,
                local_array_aliases,
                &array_vars,
                scope,
            )
        };
        let def_expr_env =
            build_scope_expr_env(expr_inputs, known_scalars, &array_vars, ScopeKind::Def);

        match stmt {
            Stmt::Const { .. } => {}
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => match target {
                AssignTarget::Var(name) => {
                    if !matches!(expr, Expr::Index { .. }) {
                        local_proc_aliases.remove(name);
                    }
                    let declared_ty = *decl_ty;
                    if let Expr::ArrayCtor { spec, init } = expr {
                        if *is_typed_decl {
                            if declared_ty.is_some() {
                                errors.push(Diagnostic::semantic(
                                    "typed declaration cannot combine scalar type annotation with array constructor",
                                    0,
                                    0,
                                ));
                                return;
                            }
                            if split_field_path(name, errors).is_some() {
                                errors.push(Diagnostic::semantic(
                                    "typed array declaration target must be a plain variable name",
                                    0,
                                    0,
                                ));
                                return;
                            }
                            if known_scalars.contains(name)
                                || local_array_aliases.contains_key(name)
                                || input_names.contains(name)
                                || output_names.contains(name)
                                || param_names.contains(name)
                                || state_scalars.contains_key(name)
                            {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "typed array declaration for '{name}' conflicts with existing symbol"
                                    ),
                                    0,
                                    0,
                                ));
                                return;
                            }
                            let size_context =
                                format!("typed array declaration size for symbol '{name}' in def");
                            let Some(size_value) =
                                eval_data_size_expr(&spec.size, options, &size_context, errors)
                            else {
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
                                            errors.push(Diagnostic::semantic(
                                            format!(
                                                "typed array declaration '{name}' initializer expects {size_value} elements, got {}",
                                                values.len()
                                            ),
                                            0,
                                            0,
                                        ));
                                        }
                                        for (idx, value) in
                                            values.iter().take(size_value).enumerate()
                                        {
                                            validate_expr(
                                                value,
                                                build_expr_env(
                                                    known_scalars,
                                                    locals,
                                                    &empty_outputs,
                                                    &array_vars,
                                                    declared_symbols,
                                                    param_structs,
                                                    struct_instance_ctx,
                                                    struct_defs,
                                                    fn_signatures,
                                                    ScopeKind::Def,
                                                ),
                                                errors,
                                            );
                                            let value_ty =
                                                infer_expr_type_for_semantics_with_local_data(
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
                                                    errors,
                                                );
                                            require_assignable_type(
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
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "typed array declaration '{name}: {struct_name}[N]' is not yet supported in def blocks"
                                        ),
                                        0,
                                        0,
                                    ));
                                }
                            }
                            return;
                        } else {
                            errors.push(Diagnostic::semantic(
                                "array[...] construction is only allowed in init or typed array declarations",
                                0,
                                0,
                            ));
                            return;
                        }
                    }
                    if let Expr::ArrayLiteral(values) = expr {
                        if declared_ty.is_some() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "typed declaration for '{name}' with array literals must use explicit array type syntax like '{name}: T[N] = [...]'"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        if split_field_path(name, errors).is_some() {
                            errors.push(Diagnostic::semantic(
                                "array declaration target must be a plain variable name",
                                0,
                                0,
                            ));
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
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "array declaration for '{name}' conflicts with existing symbol"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        if values.is_empty() {
                            errors.push(Diagnostic::semantic(
                                format!("array initializer for symbol '{name}' cannot be empty"),
                                0,
                                0,
                            ));
                            return;
                        }
                        for value in values {
                            validate_expr(value, def_expr_env, errors);
                        }
                        let elem_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        )
                        .unwrap_or(PrimitiveType::F32);
                        for (idx, value) in values.iter().enumerate() {
                            let value_ty = infer_expr_type_for_semantics_with_local_data(
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
                                errors,
                            );
                            require_assignable_type(
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
                    if let Expr::Slice { base, start, end } = expr {
                        if declared_ty.is_some() || *is_typed_decl {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "typed declaration for '{name}' is not supported for slice aliases"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        if split_field_path(name, errors).is_some() {
                            errors.push(Diagnostic::semantic(
                                "slice alias target must be a plain variable name",
                                0,
                                0,
                            ));
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
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "slice alias declaration for '{name}' conflicts with existing symbol"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        validate_expr(expr, def_expr_env, errors);
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
                        validate_expr(expr, def_expr_env, errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                            &format!("alias assignment to '{name}'"),
                            errors,
                        );
                        known_scalars.insert(name.clone());
                        return;
                    }
                    if local_array_aliases.contains_key(name) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "array alias '{name}' must be written using '{name}[index] = value'"
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    if let Some((base, field)) = split_field_path(name, errors) {
                        if declared_ty.is_some() {
                            errors.push(Diagnostic::semantic(
                                "typed declaration is only supported for plain scalar variables",
                                0,
                                0,
                            ));
                        }
                        let Some(struct_name) = param_structs.get(base) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "invalid assignment target '{name}' in def block; only struct parameters can be assigned via fields"
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    base, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let expr_for_validation =
                            rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                        match field_decl.ty {
                            TypedFieldType::Scalar(prim) => {
                                let expr_error_count_before = errors.len();
                                validate_expr(&expr_for_validation, def_expr_env, errors);
                                let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                                    errors,
                                );
                                let had_expr_validation_error =
                                    errors.len() > expr_error_count_before;
                                let suppress_type_mismatch =
                                    is_invalid_placeholder_symbol(&declared_symbols, field)
                                        || is_invalid_placeholder_symbol(
                                            &declared_symbols,
                                            &format!("{base}.{field}"),
                                        );
                                if !suppress_type_mismatch
                                    && !had_expr_validation_error
                                    && !has_use_before_declaration_error(errors)
                                {
                                    require_assignable_type(
                                        expr_ty,
                                        prim,
                                        &format!("def assignment to '{}.{}'", base, field),
                                        errors,
                                    );
                                }
                            }
                            TypedFieldType::Array(_) => {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "array field '{}.{}' must be assigned via index syntax",
                                        base, field
                                    ),
                                    0,
                                    0,
                                ));
                            }
                            TypedFieldType::Struct => {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "nested struct field '{}.{}' must be assigned via subfields or constructor",
                                        base, field
                                    ),
                                    0,
                                    0,
                                ));
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
                        if let Expr::Index { base, index } = expr {
                            if let Some(binding_kind) = classify_def_indexed_binding(
                                base,
                                local_array_aliases,
                                param_structs,
                                state_scalars,
                                struct_defs,
                                errors,
                            ) {
                                validate_expr(index, def_expr_env, errors);
                                let idx_ty = infer_expr_type_for_semantics_with_local_data(
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
                                    errors,
                                );
                                require_numeric_type(idx_ty, "array index expression", errors);
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
                        errors.push(Diagnostic::semantic(
                            format!(
                            "typed declaration for '{name}' is only allowed on first assignment"
                        ),
                            0,
                            0,
                        ));
                    }
                    if local_array_aliases.contains_key(name) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "array alias '{name}' must be written using '{name}[index] = value'"
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    if is_builtin_constant_name(name) {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to builtin constant '{name}'"),
                            0,
                            0,
                        ));
                    }
                    if locals.contains(name) {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to loop variable '{name}'"),
                            0,
                            0,
                        ));
                    }
                    if input_names.contains(name)
                        || output_names.contains(name)
                        || param_names.contains(name)
                        || state_scalars.contains_key(name)
                    {
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to global symbol '{name}' inside def"),
                            0,
                            0,
                        ));
                    }
                    let expr_error_count_before = errors.len();
                    let expr_for_validation =
                        rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                    validate_expr(&expr_for_validation, def_expr_env, errors);
                    let had_expr_validation_error = errors.len() > expr_error_count_before;
                    let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                        expr_ty.unwrap_or(PrimitiveType::F32)
                    };
                    let suppress_type_mismatch =
                        is_invalid_placeholder_symbol(&declared_symbols, name);
                    if !suppress_type_mismatch
                        && !had_expr_validation_error
                        && !has_use_before_declaration_error(errors)
                    {
                        require_assignable_type(
                            expr_ty,
                            target_ty,
                            &format!("def assignment to '{name}'"),
                            errors,
                        );
                    }
                    if can_track_local {
                        local_aliases.entry(name.clone()).or_insert(target_ty);
                    }
                    known_scalars.insert(name.clone());
                }
                AssignTarget::Index { base, index } => {
                    if decl_ty.is_some() || generic_decl_ty.is_some() || *is_typed_decl {
                        errors.push(Diagnostic::semantic(
                            "typed declaration is only supported for plain scalar variables",
                            0,
                            0,
                        ));
                    }
                    if let Some(alias) = local_array_aliases.get(base) {
                        if !alias.writable {
                            errors.push(Diagnostic::semantic(
                                format!("cannot assign to immutable array alias '{base}'"),
                                0,
                                0,
                            ));
                            return;
                        }
                        if alias.elem_struct.is_some() {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "indexed assignment target '{base}[...]' has struct elements; assign fields through an alias"
                                ),
                                0,
                                0,
                            ));
                            return;
                        }
                        validate_expr(index, def_expr_env, errors);
                        validate_expr(expr, def_expr_env, errors);
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_numeric_type(index_ty, "array index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            alias.elem_ty,
                            "array/buffer write",
                            errors,
                        );
                        return;
                    }
                    if has_declared_buffer_symbol_info(&declared_symbols, base) {
                        if is_declared_multichannel_buffer_info(&declared_symbols, base) {
                            errors.push(Diagnostic::semantic(
                            format!(
                                "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                            ),
                            0,
                            0,
                        ));
                        }
                        validate_expr(index, def_expr_env, errors);
                        validate_expr(expr, def_expr_env, errors);
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_numeric_type(index_ty, "array index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        let expected_ty = declared_symbol_scalar_type(&declared_symbols, base)
                            .unwrap_or(PrimitiveType::F32);
                        require_assignable_type(expr_ty, expected_ty, "array/buffer write", errors);
                        return;
                    }
                    if let Some((root, field)) = split_field_path(base, errors) {
                        let Some(struct_name) = param_structs.get(root) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                "indexed assignment target '{base}[...]' is invalid in def block"
                            ),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(fields) = struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "field '{}.{}' is not array and cannot be indexed",
                                    root, field
                                ),
                                0,
                                0,
                            ));
                        }
                        validate_expr(index, def_expr_env, errors);
                        validate_expr(expr, def_expr_env, errors);
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_numeric_type(index_ty, "array index expression", errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        let expected_elem_ty =
                            field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                        require_assignable_type(
                            expr_ty,
                            expected_elem_ty,
                            "array/buffer write",
                            errors,
                        );
                        return;
                    }
                    errors.push(Diagnostic::semantic(
                        "indexed assignments in def are only allowed for mutable array or buffer references (for example local arrays, array params, buffer params, or array fields on struct params such as 'tmp[i] = x', 'arr[i] = x', 'buf[i] = x', or 'self.buf[i] = x')",
                        0,
                        0,
                    ));
                }
                AssignTarget::Slice { base, start, end } => {
                    if decl_ty.is_some() || generic_decl_ty.is_some() || *is_typed_decl {
                        errors.push(Diagnostic::semantic(
                            "typed declaration is only supported for plain scalar variables",
                            0,
                            0,
                        ));
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
                        errors.push(Diagnostic::semantic(
                            format!("cannot assign to immutable array alias '{base}'"),
                            0,
                            0,
                        ));
                        return;
                    }
                    let slice_env = build_scope_expr_env(
                        expr_inputs,
                        known_scalars,
                        &array_vars,
                        ScopeKind::Def,
                    );
                    if let Some(start) = start {
                        validate_expr(start, slice_env, errors);
                        let start_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_numeric_type(start_ty, "slice start bound", errors);
                    }
                    if let Some(end) = end {
                        validate_expr(end, slice_env, errors);
                        let end_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_numeric_type(end_ty, "slice end bound", errors);
                    }
                    let stmt_env = build_scope_stmt_expr_env(
                        expr_inputs,
                        known_scalars,
                        local_aliases,
                        local_array_aliases,
                        &array_vars,
                        ScopeKind::Def,
                    );
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
                            require_assignable_type(
                                Some(src_info.elem_ty),
                                target_info.elem_ty,
                                "slice copy assignment",
                                errors,
                            );
                        }
                    } else {
                        validate_expr(expr, slice_env, errors);
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
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
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            target_info.elem_ty,
                            "slice fill assignment",
                            errors,
                        );
                    }
                }
            },
            Stmt::Expr { expr, .. } => {
                let expr = rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                analyze_stmt_expr(&expr, stmt_expr_env(ScopeKind::Def), errors);
            }
            Stmt::Return { expr, .. } => {
                let expr = rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                analyze_stmt_expr(&expr, stmt_expr_env(ScopeKind::Def), errors);
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
                    stmt_expr_env(ScopeKind::Def),
                    errors,
                );
                let mut then_state = fork_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                );
                for nested in then_branch {
                    analyze_def_stmt(nested, ctx, &mut then_state, loop_depth, errors);
                }
                let mut else_state = fork_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                );
                for nested in else_branch {
                    analyze_def_stmt(nested, ctx, &mut else_state, loop_depth, errors);
                }
                merge_branch_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
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
                    stmt_expr_env(ScopeKind::Def),
                    errors,
                );
                require_validated_numeric_stmt_expr(
                    end,
                    "for loop end bound",
                    stmt_expr_env(ScopeKind::Def),
                    errors,
                );
                validate_for_loop_step_expr(step.as_ref(), stmt_expr_env(ScopeKind::Def), errors);
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_state = fork_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
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
                    loop_state,
                );
            }
            Stmt::While { cond, body, .. } => {
                require_validated_bool_stmt_expr(
                    cond,
                    "while condition",
                    stmt_expr_env(ScopeKind::Def),
                    errors,
                );
                let mut loop_state = fork_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                );
                for nested in body {
                    analyze_def_stmt(nested, ctx, &mut loop_state, loop_depth + 1, errors);
                }
                adopt_loop_scope_flow_state(
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    loop_state,
                );
            }
            Stmt::Break { .. } => require_loop_control_context("break", loop_depth, errors),
            Stmt::Continue { .. } => require_loop_control_context("continue", loop_depth, errors),
        }
    });
}
