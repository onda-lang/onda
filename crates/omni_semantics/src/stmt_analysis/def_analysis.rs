use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_def_stmt(
    stmt: &Stmt,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    param_structs: &HashMap<String, String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let empty_data = HashMap::<String, usize>::new();
        // In def analysis, struct-typed parameters (for example `self`) should be
        // visible to expression type inference, including indexed array field reads.
        let struct_instance_ctx = param_structs;
        let empty_outputs = HashSet::<String>::new();
        let array_vars = merged_data_vars_for_sample(&empty_data, local_array_aliases);

        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => match target {
                AssignTarget::Var(name) => {
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
                                                ExprEnv {
                                                    known_scalars,
                                                    locals,
                                                    outputs: &empty_outputs,
                                                    array_vars: &array_vars,
                                                    param_structs,
                                                    struct_instances: struct_instance_ctx,
                                                    struct_defs,
                                                    fn_signatures,
                                                    allow_array_ctor: false,
                                                    scope: ScopeKind::Def,
                                                },
                                                errors,
                                            );
                                            let value_ty =
                                                infer_expr_type_for_semantics_with_local_data(
                                                    value,
                                                    state_scalars,
                                                    None,
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
                            validate_expr(
                                value,
                                ExprEnv {
                                    known_scalars,
                                    locals,
                                    outputs: &empty_outputs,
                                    array_vars: &array_vars,
                                    param_structs,
                                    struct_instances: struct_instance_ctx,
                                    struct_defs,
                                    fn_signatures,
                                    allow_array_ctor: false,
                                    scope: ScopeKind::Def,
                                },
                                errors,
                            );
                        }
                        let elem_ty = infer_expr_type_for_semantics_with_local_data(
                            &values[0],
                            state_scalars,
                            None,
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
                                None,
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
                    if local_aliases.contains_key(name) {
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let expr_ty = infer_expr_type_for_semantics_with_local_data(
                            expr,
                            state_scalars,
                            None,
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
                        match field_decl.ty {
                            TypedFieldType::Scalar(prim) => {
                                let expr_error_count_before = errors.len();
                                validate_expr(
                                    expr,
                                    ExprEnv {
                                        known_scalars,
                                        locals,
                                        outputs: &empty_outputs,
                                        array_vars: &array_vars,
                                        param_structs,
                                        struct_instances: struct_instance_ctx,
                                        struct_defs,
                                        fn_signatures,
                                        allow_array_ctor: false,
                                        scope: ScopeKind::Def,
                                    },
                                    errors,
                                );
                                let expr_ty = infer_expr_type_for_semantics_with_local_data(
                                    expr,
                                    state_scalars,
                                    None,
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
                                let suppress_type_mismatch = get_declared_symbol_type(
                                    state_scalars,
                                    field,
                                    DECLARED_INVALID_PLACEHOLDER_PREFIX,
                                )
                                .is_some()
                                    || get_declared_symbol_type(
                                        state_scalars,
                                        &format!("{base}.{field}"),
                                        DECLARED_INVALID_PLACEHOLDER_PREFIX,
                                    )
                                    .is_some();
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
                            let mut is_scalar_data_base = false;
                            let mut array_struct_elem_struct: Option<String> = None;

                            if let Some(alias) = local_array_aliases.get(base) {
                                if let Some(elem_struct) = &alias.elem_struct {
                                    array_struct_elem_struct = Some(elem_struct.clone());
                                } else {
                                    is_scalar_data_base = true;
                                }
                            }

                            if !is_scalar_data_base && array_struct_elem_struct.is_none() {
                                if let Some((root, field)) = split_field_path(base, errors) {
                                    if let Some(struct_name) = param_structs.get(root) {
                                        if let Some(fields) = struct_defs.get(struct_name) {
                                            if let Some(field_decl) =
                                                fields.iter().find(|f| f.name == field)
                                            {
                                                if matches!(field_decl.ty, TypedFieldType::Array(_))
                                                {
                                                    if let Some(elem_struct) =
                                                        &field_decl.array_elem_struct
                                                    {
                                                        array_struct_elem_struct =
                                                            Some(elem_struct.clone());
                                                    } else {
                                                        is_scalar_data_base = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if is_scalar_data_base || array_struct_elem_struct.is_some() {
                                validate_expr(
                                    index,
                                    ExprEnv {
                                        known_scalars,
                                        locals,
                                        outputs: &empty_outputs,
                                        array_vars: &array_vars,
                                        param_structs,
                                        struct_instances: struct_instance_ctx,
                                        struct_defs,
                                        fn_signatures,
                                        allow_array_ctor: false,
                                        scope: ScopeKind::Def,
                                    },
                                    errors,
                                );
                                let idx_ty = infer_expr_type_for_semantics_with_local_data(
                                    index,
                                    state_scalars,
                                    None,
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
                                if is_scalar_data_base {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "local alias binding '{name} = {base}[...]' is not supported for primitive arrays; use direct indexed access"
                                        ),
                                        0,
                                        0,
                                    ));
                                } else if let Some(struct_name) = array_struct_elem_struct {
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
                                }
                                return;
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
                    validate_expr(
                        expr,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs: &empty_outputs,
                            array_vars: &array_vars,
                            param_structs,
                            struct_instances: struct_instance_ctx,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Def,
                        },
                        errors,
                    );
                    let had_expr_validation_error = errors.len() > expr_error_count_before;
                    let expr_ty = infer_expr_type_for_semantics_with_local_data(
                        expr,
                        state_scalars,
                        None,
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
                    let suppress_type_mismatch = get_declared_symbol_type(
                        state_scalars,
                        name,
                        DECLARED_INVALID_PLACEHOLDER_PREFIX,
                    )
                    .is_some();
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
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
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
                            None,
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
                    if has_declared_buffer_symbol(known_scalars, base) {
                        if is_declared_multichannel_buffer_symbol(known_scalars, base) {
                            errors.push(Diagnostic::semantic(
                            format!(
                                "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                            ),
                            0,
                            0,
                        ));
                        }
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
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
                            None,
                            local_array_aliases,
                            locals,
                            input_names,
                            output_names,
                            param_names,
                            struct_instance_ctx,
                            struct_defs,
                            errors,
                        );
                        let expected_ty = get_declared_symbol_type(
                            state_scalars,
                            base,
                            DECLARED_BUFFER_ELEM_TYPE_PREFIX,
                        )
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
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: &empty_outputs,
                                array_vars: &array_vars,
                                param_structs,
                                struct_instances: struct_instance_ctx,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Def,
                            },
                            errors,
                        );
                        let index_ty = infer_expr_type_for_semantics_with_local_data(
                            index,
                            state_scalars,
                            None,
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
                            None,
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
                        "indexed assignments in def are only allowed for local typed arrays or array fields on struct parameters (for example 'tmp[i] = x' or 'self.buf[i] = x')",
                        0,
                        0,
                    ));
                }
            },
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics_with_local_data(
                    expr,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);
                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_array_aliases.clone();
                for nested in then_branch {
                    analyze_def_stmt(
                        nested,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth,
                        errors,
                    );
                }
                let mut else_known = known_scalars.clone();
                let mut else_aliases = local_aliases.clone();
                let mut else_data_aliases = local_array_aliases.clone();
                for nested in else_branch {
                    analyze_def_stmt(
                        nested,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth,
                        errors,
                    );
                }
                let mut merged = known_scalars.clone();
                for name in &then_known {
                    if else_known.contains(name) {
                        merged.insert(name.clone());
                    }
                }
                *known_scalars = merged;
                *local_aliases = then_aliases;
                local_aliases.extend(else_aliases);
                local_aliases.retain(|name, _| known_scalars.contains(name));
                *local_array_aliases = then_data_aliases;
                for (k, v) in else_data_aliases {
                    local_array_aliases.entry(k).or_insert(v);
                }
            }
            Stmt::For {
                var,
                step,
                start,
                end,
                body,
                ..
            } => {
                validate_expr(
                    start,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                if let Some(step_expr) = step {
                    validate_expr(
                        step_expr,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs: &empty_outputs,
                            array_vars: &array_vars,
                            param_structs,
                            struct_instances: struct_instance_ctx,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Def,
                        },
                        errors,
                    );
                }
                let start_ty = infer_expr_type_for_semantics_with_local_data(
                    start,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_numeric_type(start_ty, "for loop start bound", errors);
                let end_ty = infer_expr_type_for_semantics_with_local_data(
                    end,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_numeric_type(end_ty, "for loop end bound", errors);
                if let Some(step_expr) = step {
                    let step_ty = infer_expr_type_for_semantics_with_local_data(
                        step_expr,
                        state_scalars,
                        None,
                        local_array_aliases,
                        locals,
                        input_names,
                        output_names,
                        param_names,
                        struct_instance_ctx,
                        struct_defs,
                        errors,
                    );
                    require_numeric_type(step_ty, "for loop step", errors);
                    if matches!(step_expr, Expr::Int(0))
                        || matches!(step_expr, Expr::Number(v) if *v == 0.0)
                    {
                        errors.push(Diagnostic::semantic("for loop step cannot be zero", 0, 0));
                    }
                }
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_array_aliases.clone();
                for nested in body {
                    analyze_def_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth + 1,
                        errors,
                    );
                }
                loop_aliases.retain(|name, _| known_scalars.contains(name));
                *local_aliases = loop_aliases;
                *local_array_aliases = loop_data_aliases;
            }
            Stmt::While { cond, body, .. } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: &empty_outputs,
                        array_vars: &array_vars,
                        param_structs,
                        struct_instances: struct_instance_ctx,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Def,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics_with_local_data(
                    cond,
                    state_scalars,
                    None,
                    local_array_aliases,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instance_ctx,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "while condition", errors);

                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_array_aliases.clone();
                for nested in body {
                    analyze_def_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        locals,
                        param_structs,
                        state_scalars,
                        input_names,
                        output_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth + 1,
                        errors,
                    );
                }
                loop_aliases.retain(|name, _| known_scalars.contains(name));
                *local_aliases = loop_aliases;
                *local_array_aliases = loop_data_aliases;
            }
            Stmt::Break { .. } => {
                if loop_depth == 0 {
                    errors.push(Diagnostic::semantic(
                        "break is only allowed inside for/while/loop bodies",
                        0,
                        0,
                    ));
                }
            }
            Stmt::Continue { .. } => {
                if loop_depth == 0 {
                    errors.push(Diagnostic::semantic(
                        "continue is only allowed inside for/while/loop bodies",
                        0,
                        0,
                    ));
                }
            }
        }
    });
}
