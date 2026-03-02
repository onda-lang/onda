use super::*;

pub(crate) fn merged_data_vars_for_sample(
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
) -> HashMap<String, usize> {
    let mut merged = state_arrays.clone();
    for (name, alias) in local_array_aliases {
        if alias.elem_struct.is_none() {
            merged.insert(name.clone(), alias.len);
        }
    }
    merged
}

pub(crate) fn seed_top_level_array_aliases(
    aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    arrays: &HashMap<String, TypedArrayInfo>,
    writable: bool,
) {
    for (name, info) in arrays {
        aliases.insert(
            name.clone(),
            LocalArrayAliasInfo {
                len: info.len,
                elem_ty: info.elem_ty,
                elem_struct: None,
                writable,
            },
        );
    }
}

pub(crate) fn analyze_sample_stmt(
    stmt: &Stmt,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_assign_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || {
        let array_vars = merged_data_vars_for_sample(state_arrays, local_array_aliases);
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                analyze_assign_sample(
                    target,
                    decl_ty,
                    generic_decl_ty,
                    *is_typed_decl,
                    expr,
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    locals,
                    state_scalars,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    input_names,
                    output_names,
                    forbidden_assign_names,
                    param_names,
                    struct_defs,
                    fn_signatures,
                    options,
                    errors,
                );
            }
            Stmt::Expr { expr, .. } => {
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let _ = infer_expr_type_for_semantics(
                    expr,
                    state_scalars,
                    None,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { .. } => {
                errors.push(Diagnostic::semantic(
                    "return is only allowed inside def blocks",
                    0,
                    0,
                ));
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
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                let cond_ty = infer_expr_type_for_semantics(
                    cond,
                    state_scalars,
                    None,
                    locals,
                    input_names,
                    output_names,
                    param_names,
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "if condition", errors);
                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_array_aliases.clone();
                for nested in then_branch {
                    analyze_sample_stmt(
                        nested,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
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
                    analyze_sample_stmt(
                        nested,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth,
                        errors,
                    );
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
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                validate_expr(
                    end,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
                    },
                    errors,
                );
                if let Some(step_expr) = step {
                    validate_expr(
                        step_expr,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs: output_names,
                            array_vars: &array_vars,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Sample,
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
                    struct_instances,
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
                    struct_instances,
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
                        struct_instances,
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
                    analyze_sample_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth + 1,
                        errors,
                    );
                }
            }
            Stmt::While { cond, body, .. } => {
                validate_expr(
                    cond,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
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
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_bool_type(cond_ty, "while condition", errors);
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_array_aliases.clone();
                for nested in body {
                    analyze_sample_stmt(
                        nested,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        locals,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        input_names,
                        output_names,
                        forbidden_assign_names,
                        param_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        loop_depth + 1,
                        errors,
                    );
                }
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
fn analyze_assign_sample(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    forbidden_assign_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let array_vars = merged_data_vars_for_sample(state_arrays, local_array_aliases);
    match target {
        AssignTarget::Index { base, index } => {
            if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
            }
            if state_array_struct_roots.contains_key(base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                    0,
                    0,
                ));
                return;
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
                            "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                        0,
                        0,
                    ));
                    return;
                }
            }
            if !state_arrays.contains_key(base)
                && !local_array_aliases.contains_key(base)
                && !has_declared_buffer_symbol(known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed assignment target '{base}[...]' is not a array/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(known_scalars, base) {
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
                    outputs: output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Sample,
                },
                errors,
            );
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Sample,
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
                struct_instances,
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
                struct_instances,
                struct_defs,
                errors,
            );
            let expected_ty = local_array_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_DATA_ELEM_TYPE_PREFIX)
                })
                .or_else(|| {
                    get_declared_symbol_type(state_scalars, base, DECLARED_BUFFER_ELEM_TYPE_PREFIX)
                })
                .unwrap_or(PrimitiveType::F32);
            require_assignable_type(expr_ty, expected_ty, "array/buffer write", errors);
        }
        AssignTarget::Var(name) => {
            if locals.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to loop variable '{name}'"),
                    0,
                    0,
                ));
            }
            if is_builtin_constant_name(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to builtin constant '{name}'"),
                    0,
                    0,
                ));
            }
            if forbidden_assign_names.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to output symbol '{name}' in block"),
                    0,
                    0,
                ));
            }
            if let Expr::ArrayCtor { spec, init } = expr {
                if is_typed_decl {
                    if decl_ty.is_some() {
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
                        || local_aliases.contains_key(name)
                        || local_array_aliases.contains_key(name)
                        || state_scalars.contains_key(name)
                        || state_arrays.contains_key(name)
                        || state_array_struct_roots.contains_key(name)
                        || struct_instances.contains_key(name)
                        || input_names.contains(name)
                        || output_names.contains(name)
                        || param_names.contains(name)
                    {
                        errors.push(Diagnostic::semantic(
                            format!("typed array declaration for '{name}' conflicts with existing symbol"),
                            0,
                            0,
                        ));
                        return;
                    }
                    let size_context =
                        format!("typed array declaration size for symbol '{name}' in sample");
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
                                for (idx, value) in values.iter().take(size_value).enumerate() {
                                    validate_expr(
                                        value,
                                        ExprEnv {
                                            known_scalars,
                                            locals,
                                            outputs: output_names,
                                            array_vars: &array_vars,
                                            param_structs: &HashMap::new(),
                                            struct_instances,
                                            struct_defs,
                                            fn_signatures,
                                            allow_array_ctor: false,
                                            scope: ScopeKind::Sample,
                                        },
                                        errors,
                                    );
                                    let value_ty = infer_expr_type_for_semantics_with_local_data(
                                        value,
                                        state_scalars,
                                        None,
                                        local_array_aliases,
                                        locals,
                                        input_names,
                                        output_names,
                                        param_names,
                                        struct_instances,
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
                                    "typed array declaration '{name}: {struct_name}[N]' is not yet supported in sample/block"
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    return;
                }
            }
            if let Expr::ArrayLiteral(values) = expr {
                if decl_ty.is_some() {
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
                    || state_scalars.contains_key(name)
                    || state_arrays.contains_key(name)
                    || state_array_struct_roots.contains_key(name)
                    || struct_instances.contains_key(name)
                    || input_names.contains(name)
                    || output_names.contains(name)
                    || param_names.contains(name)
                {
                    errors.push(Diagnostic::semantic(
                        format!("array declaration for '{name}' conflicts with existing symbol"),
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
                            outputs: output_names,
                            array_vars: &array_vars,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Sample,
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
                    struct_instances,
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
                        struct_instances,
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
                if matches!(expr, Expr::ArrayCtor { .. }) {
                    errors.push(Diagnostic::semantic(
                        "array[...] construction is only allowed in init",
                        0,
                        0,
                    ));
                }
                if let Expr::UserCall { name: ctor, .. } = expr {
                    if struct_defs.contains_key(ctor) {
                        errors.push(Diagnostic::semantic(
                            "struct construction is only allowed in init",
                            0,
                            0,
                        ));
                    }
                }
                validate_expr(
                    expr,
                    ExprEnv {
                        known_scalars,
                        locals,
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Sample,
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
                    struct_instances,
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
                    format!("array alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                let Some(struct_name) = struct_instances.get(base) else {
                    errors.push(Diagnostic::semantic(
                        format!("unknown struct instance '{base}'"),
                        0,
                        0,
                    ));
                    return;
                };
                let Some(fields) = struct_defs.get(struct_name) else {
                    return;
                };
                let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                    errors.push(Diagnostic::semantic(
                        format!("struct '{}' has no field '{}'", struct_name, field),
                        0,
                        0,
                    ));
                    return;
                };
                let flat = format!("{base}.{field}");
                match field_decl.ty {
                    TypedFieldType::Scalar(prim) => {
                        if !state_scalars.contains_key(&flat) {
                            errors.push(Diagnostic::semantic(
                                format!("struct field '{flat}' must be initialized in init"),
                                0,
                                0,
                            ));
                        }
                        validate_expr(
                            expr,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: output_names,
                                array_vars: &array_vars,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Sample,
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
                            struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_assignable_type(
                            expr_ty,
                            prim,
                            &format!("sample assignment to '{flat}'"),
                            errors,
                        );
                    }
                    TypedFieldType::Array(_) => {
                        errors.push(Diagnostic::semantic(
                            format!("array field '{flat}' must be accessed with index syntax"),
                            0,
                            0,
                        ));
                    }
                }
                return;
            }

            if !output_names.contains(name)
                && !state_scalars.contains_key(name)
                && !state_arrays.contains_key(name)
                && !local_array_aliases.contains_key(name)
                && !state_array_struct_roots.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !param_names.contains(name)
            {
                if let Expr::Index { base, index } = expr {
                    let mut is_scalar_data_base = state_arrays.contains_key(base);
                    let mut array_struct_elem_struct = state_array_struct_roots
                        .get(base)
                        .map(|r| r.struct_name.clone());
                    if let Some(alias) = local_array_aliases.get(base) {
                        if let Some(elem_struct) = &alias.elem_struct {
                            array_struct_elem_struct = Some(elem_struct.clone());
                        } else {
                            is_scalar_data_base = true;
                        }
                    }
                    if !is_scalar_data_base && array_struct_elem_struct.is_none() {
                        if let Some((root, field)) = split_field_path(base, errors) {
                            if let Some(struct_name) = struct_instances.get(root) {
                                if let Some(fields) = struct_defs.get(struct_name) {
                                    if let Some(field_decl) =
                                        fields.iter().find(|f| f.name == field)
                                    {
                                        if matches!(field_decl.ty, TypedFieldType::Array(_)) {
                                            if let Some(elem_struct) = &field_decl.array_elem_struct
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
                                outputs: output_names,
                                array_vars: &array_vars,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope: ScopeKind::Sample,
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
                            struct_instances,
                            struct_defs,
                            errors,
                        );
                        require_numeric_type(idx_ty, "array index expression", errors);
                        if let Some(struct_name) = array_struct_elem_struct {
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
                        // Primitive array/buffer indexed reads are scalar expressions.
                        // Allow normal first-assignment local inference to handle:
                        //   x = arr[idx]
                    }
                }
            }

            if input_names.contains(name) || param_names.contains(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to immutable symbol '{name}' in sample block"),
                    0,
                    0,
                ));
            }
            if struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct instance '{name}' cannot be assigned in sample"),
                    0,
                    0,
                ));
            }
            if matches!(expr, Expr::ArrayCtor { .. }) {
                errors.push(Diagnostic::semantic(
                    "array[...] construction is only allowed in init",
                    0,
                    0,
                ));
            }
            if let Expr::UserCall { name: ctor, .. } = expr {
                if struct_defs.contains_key(ctor) {
                    errors.push(Diagnostic::semantic(
                        "struct construction is only allowed in init",
                        0,
                        0,
                    ));
                }
            }
            if state_arrays.contains_key(name) || state_array_struct_roots.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("array symbol '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
            }
            if local_array_aliases.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("array alias '{name}' must be written using '{name}[index] = value'"),
                    0,
                    0,
                ));
            }
            if let Some(declared_ty) = *decl_ty {
                if output_names.contains(name) || local_aliases.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "typed declaration for '{name}' is only allowed on first assignment"
                        ),
                        0,
                        0,
                    ));
                } else if let Some(existing_ty) = state_scalars.get(name).copied() {
                    if existing_ty != declared_ty {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "typed declaration for '{name}' conflicts with existing state type {:?}",
                                existing_ty
                            ),
                            0,
                            0,
                        ));
                    }
                }
            }

            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    array_vars: &array_vars,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Sample,
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
                struct_instances,
                struct_defs,
                errors,
            );
            let can_track_local = !output_names.contains(name)
                && !state_scalars.contains_key(name)
                && !state_arrays.contains_key(name)
                && !state_array_struct_roots.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !param_names.contains(name)
                && !local_array_aliases.contains_key(name)
                && !locals.contains(name)
                && !is_builtin_constant_name(name);
            let target_ty = if output_names.contains(name) {
                Some(
                    get_declared_symbol_type(state_scalars, name, DECLARED_OUTPUT_TYPE_PREFIX)
                        .unwrap_or(PrimitiveType::F32),
                )
            } else if let Some(existing) = state_scalars.get(name).copied() {
                Some(existing)
            } else if let Some(existing) = local_aliases.get(name).copied() {
                Some(existing)
            } else if let Some(declared) = *decl_ty {
                Some(declared)
            } else {
                Some(expr_ty.unwrap_or(PrimitiveType::F32))
            };
            if let Some(target_ty) = target_ty {
                require_assignable_type(
                    expr_ty,
                    target_ty,
                    &format!("sample assignment to '{name}'"),
                    errors,
                );
                if can_track_local {
                    local_aliases.entry(name.clone()).or_insert(target_ty);
                }
            }

            if output_names.contains(name) || can_track_local {
                known_scalars.insert(name.clone());
            }
        }
    }
}
