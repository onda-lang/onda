use super::*;

pub(crate) fn analyze_init_stmt(
    stmt: &Stmt,
    init_default_ty: Option<PrimitiveType>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
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
        let array_vars = merged_data_vars_for_sample(state_arrays, local_array_aliases);
        match stmt {
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => analyze_assign_init(
                target,
                decl_ty,
                generic_decl_ty,
                *is_typed_decl,
                expr,
                init_default_ty,
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
                param_names,
                struct_defs,
                fn_signatures,
                options,
                errors,
            ),
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
                        scope: ScopeKind::Init,
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
                    struct_instances,
                    struct_defs,
                    errors,
                );
            }
            Stmt::Return { .. } => errors.push(Diagnostic::semantic(
                "return is only allowed inside def blocks",
                0,
                0,
            )),
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
                        scope: ScopeKind::Init,
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
                require_bool_type(cond_ty, "if condition", errors);

                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_array_aliases.clone();
                let mut then_scalars = state_scalars.clone();
                let mut then_data = state_arrays.clone();
                let mut then_data_struct_roots = state_array_struct_roots.clone();
                let mut then_structs = struct_instances.clone();
                for nested in then_branch {
                    analyze_init_stmt(
                        nested,
                        init_default_ty,
                        &mut then_known,
                        &mut then_aliases,
                        &mut then_data_aliases,
                        locals,
                        &mut then_scalars,
                        &mut then_data,
                        &mut then_data_struct_roots,
                        &mut then_structs,
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
                let mut else_scalars = state_scalars.clone();
                let mut else_data = state_arrays.clone();
                let mut else_data_struct_roots = state_array_struct_roots.clone();
                let mut else_structs = struct_instances.clone();
                for nested in else_branch {
                    analyze_init_stmt(
                        nested,
                        init_default_ty,
                        &mut else_known,
                        &mut else_aliases,
                        &mut else_data_aliases,
                        locals,
                        &mut else_scalars,
                        &mut else_data,
                        &mut else_data_struct_roots,
                        &mut else_structs,
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

                state_scalars.extend(then_scalars);
                state_scalars.extend(else_scalars);
                for (k, v) in then_data {
                    state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in then_data_struct_roots {
                    state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in else_data {
                    state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in else_data_struct_roots {
                    state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in then_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                for (k, v) in else_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                known_scalars.extend(state_scalars.keys().cloned());
                local_aliases.extend(then_aliases);
                local_aliases.extend(else_aliases);
                for (k, v) in then_data_aliases {
                    local_array_aliases.entry(k).or_insert(v);
                }
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
                        outputs: output_names,
                        array_vars: &array_vars,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope: ScopeKind::Init,
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
                        scope: ScopeKind::Init,
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
                            scope: ScopeKind::Init,
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
                let mut loop_scalars = state_scalars.clone();
                let mut loop_data = state_arrays.clone();
                let mut loop_data_struct_roots = state_array_struct_roots.clone();
                let mut loop_structs = struct_instances.clone();
                for nested in body {
                    analyze_init_stmt(
                        nested,
                        init_default_ty,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        &loop_locals,
                        &mut loop_scalars,
                        &mut loop_data,
                        &mut loop_data_struct_roots,
                        &mut loop_structs,
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
                state_scalars.extend(loop_scalars);
                for (k, v) in loop_data {
                    state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in loop_data_struct_roots {
                    state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in loop_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                known_scalars.extend(state_scalars.keys().cloned());
                *local_aliases = loop_aliases;
                *local_array_aliases = loop_data_aliases;
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
                        scope: ScopeKind::Init,
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
                let mut loop_scalars = state_scalars.clone();
                let mut loop_data = state_arrays.clone();
                let mut loop_data_struct_roots = state_array_struct_roots.clone();
                let mut loop_structs = struct_instances.clone();
                for nested in body {
                    analyze_init_stmt(
                        nested,
                        init_default_ty,
                        &mut loop_known,
                        &mut loop_aliases,
                        &mut loop_data_aliases,
                        locals,
                        &mut loop_scalars,
                        &mut loop_data,
                        &mut loop_data_struct_roots,
                        &mut loop_structs,
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
                state_scalars.extend(loop_scalars);
                for (k, v) in loop_data {
                    state_arrays.entry(k).or_insert(v);
                }
                for (k, v) in loop_data_struct_roots {
                    state_array_struct_roots.entry(k).or_insert(v);
                }
                for (k, v) in loop_structs {
                    struct_instances.entry(k).or_insert(v);
                }
                known_scalars.extend(state_scalars.keys().cloned());
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
#[allow(clippy::too_many_arguments)]
fn analyze_assign_init(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    expr: &Expr,
    init_default_ty: Option<PrimitiveType>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    input_names: &HashSet<String>,
    output_names: &HashSet<String>,
    param_names: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let array_vars = merged_data_vars_for_sample(state_arrays, local_array_aliases);
    match target {
        AssignTarget::Index { base, index } => {
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
            if decl_ty.is_some() || generic_decl_ty.is_some() {
                errors.push(Diagnostic::semantic(
                    "typed declaration is only supported for plain scalar variables",
                    0,
                    0,
                ));
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
                    scope: ScopeKind::Init,
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
                    scope: ScopeKind::Init,
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
            let declared_ty = if let Some(declared) = *decl_ty {
                Some(declared)
            } else if let Some(param) = generic_decl_ty {
                errors.push(Diagnostic::semantic(
                    format!(
                        "generic typed declaration for '{name}: {param}' is only supported in init blocks of specialized generic processors"
                    ),
                    0,
                    0,
                ));
                None
            } else {
                None
            };
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
            if input_names.contains(name)
                || output_names.contains(name)
                || param_names.contains(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign to '{name}' in init block"),
                    0,
                    0,
                ));
            }

            if local_aliases.contains_key(name) {
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
                        scope: ScopeKind::Init,
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
                analyze_struct_field_init_assign(
                    base,
                    field,
                    expr,
                    known_scalars,
                    locals,
                    state_scalars,
                    state_arrays,
                    state_array_struct_roots,
                    struct_instances,
                    output_names,
                    struct_defs,
                    fn_signatures,
                    options,
                    errors,
                );
                return;
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
                if state_arrays.contains_key(name) || state_array_struct_roots.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("array symbol '{name}' can only be initialized once"),
                        0,
                        0,
                    ));
                    return;
                }
                if state_scalars.contains_key(name) || struct_instances.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{name}' already used with a different state type"),
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
                            scope: ScopeKind::Init,
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

                state_scalars.insert(
                    declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                    elem_ty,
                );
                state_arrays.insert(name.clone(), values.len());
                known_scalars.insert(name.clone());
                return;
            }

            if let Expr::UserCall {
                name: struct_name,
                type_args,
                args,
                ..
            } = expr
            {
                if struct_defs.contains_key(struct_name) {
                    if !type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct '{}' is not generic and cannot take type arguments",
                                struct_name
                            ),
                            0,
                            0,
                        ));
                    }
                    if declared_ty.is_some() {
                        errors.push(Diagnostic::semantic(
                            "typed declaration cannot be used with struct constructor assignment",
                            0,
                            0,
                        ));
                        return;
                    }
                    analyze_struct_ctor_init_assign(
                        name,
                        struct_name,
                        args,
                        known_scalars,
                        locals,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        output_names,
                        struct_defs,
                        fn_signatures,
                        options,
                        errors,
                    );
                    return;
                }
            }

            if let Expr::ArrayCtor { spec, init } = expr {
                if declared_ty.is_some() {
                    errors.push(Diagnostic::semantic(
                        "typed declaration cannot be used with array[...] constructor assignment",
                        0,
                        0,
                    ));
                    return;
                }
                let context = format!("array constructor for symbol '{name}'");
                let size_context = format!("array constructor size for symbol '{name}'");
                if init.is_some() && !is_typed_decl {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "array constructor for symbol '{name}' does not support inline array initializers"
                        ),
                        0,
                        0,
                    ));
                }
                let Some(size_value) =
                    eval_data_size_expr(&spec.size, options, &size_context, errors)
                else {
                    return;
                };
                if state_arrays.contains_key(name) || state_array_struct_roots.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("array symbol '{name}' can only be initialized once"),
                        0,
                        0,
                    ));
                    return;
                }
                if state_scalars.contains_key(name) || struct_instances.contains_key(name) {
                    errors.push(Diagnostic::semantic(
                        format!("symbol '{name}' already used with a different state type"),
                        0,
                        0,
                    ));
                    return;
                }
                match &spec.elem {
                    ArrayElemType::Primitive(elem_ty) => {
                        state_scalars.insert(
                            declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, name),
                            *elem_ty,
                        );
                        state_arrays.insert(name.clone(), size_value);
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
                                        scope: ScopeKind::Init,
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
                        if !register_data_struct_root(
                            name,
                            struct_name,
                            size_value,
                            struct_defs,
                            &context,
                            state_scalars,
                            state_arrays,
                            state_array_struct_roots,
                            errors,
                        ) {
                            return;
                        }
                    }
                }
                return;
            }

            if !state_arrays.contains_key(name)
                && !state_array_struct_roots.contains_key(name)
                && !state_scalars.contains_key(name)
                && !struct_instances.contains_key(name)
                && !input_names.contains(name)
                && !output_names.contains(name)
                && !param_names.contains(name)
                && !local_aliases.contains_key(name)
                && !local_array_aliases.contains_key(name)
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
                                scope: ScopeKind::Init,
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

            if state_arrays.contains_key(name) || state_array_struct_roots.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to array symbol '{name}'"),
                    0,
                    0,
                ));
            }
            if struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("cannot assign scalar expression to struct instance '{name}'"),
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
                    scope: ScopeKind::Init,
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
            let target_ty = match (declared_ty, state_scalars.get(name).copied()) {
                (Some(declared), Some(existing)) if declared != existing => {
                    errors.push(Diagnostic::semantic(format!("typed declaration for '{name}' conflicts with existing state type {:?}", existing), 0, 0));
                    existing
                }
                (Some(declared), _) => declared,
                (None, Some(existing)) => existing,
                (None, None) => init_default_ty.or(expr_ty).unwrap_or(PrimitiveType::F32),
            };
            require_assignable_type(
                expr_ty,
                target_ty,
                &format!("init assignment to '{name}'"),
                errors,
            );
            state_scalars.insert(name.clone(), target_ty);
            known_scalars.insert(name.clone());
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn analyze_struct_ctor_init_assign(
    target: &str,
    struct_name: &str,
    args: &[CallArg],
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &mut HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    _options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if struct_instances.contains_key(target) {
        errors.push(Diagnostic::semantic(
            format!("struct instance '{target}' can only be initialized once"),
            0,
            0,
        ));
        return;
    }
    if state_scalars.contains_key(target)
        || state_arrays.contains_key(target)
        || state_array_struct_roots.contains_key(target)
    {
        errors.push(Diagnostic::semantic(
            format!("symbol '{target}' already used with a different state type"),
            0,
            0,
        ));
        return;
    }

    let Some(fields) = struct_defs.get(struct_name) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct '{struct_name}'"),
            0,
            0,
        ));
        return;
    };

    let scalar_param_names = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.name.clone())
        .collect::<Vec<_>>();
    let scalar_defaults = fields
        .iter()
        .filter(|f| matches!(f.ty, TypedFieldType::Scalar(_)))
        .map(|f| f.default.clone().or(Some(Expr::Number(0.0))))
        .collect::<Vec<_>>();

    let resolved = resolve_call_args(
        args,
        &scalar_param_names,
        &scalar_defaults,
        false,
        false,
        &format!("struct constructor '{struct_name}'"),
        errors,
    );

    let mut scalar_idx = 0usize;
    for field in fields {
        let flat = format!("{target}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                if let Some(arg) = resolved[scalar_idx] {
                    validate_expr(
                        arg,
                        ExprEnv {
                            known_scalars,
                            locals,
                            outputs,
                            array_vars: state_arrays,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope: ScopeKind::Init,
                        },
                        errors,
                    );
                    let arg_ty = infer_expr_type_for_semantics(
                        arg,
                        state_scalars,
                        None,
                        locals,
                        &HashSet::new(),
                        outputs,
                        &HashSet::new(),
                        struct_instances,
                        struct_defs,
                        errors,
                    );
                    require_assignable_type(
                        arg_ty,
                        prim,
                        &format!("struct constructor field '{flat}'"),
                        errors,
                    );
                }
                scalar_idx += 1;
                state_scalars.insert(flat.clone(), prim);
                known_scalars.insert(flat);
            }
            TypedFieldType::Array(len) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let context =
                        format!("struct constructor field '{flat}' array element '{elem_struct}'");
                    if !register_data_struct_root(
                        &flat,
                        elem_struct,
                        len,
                        struct_defs,
                        &context,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        continue;
                    }
                } else {
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                    );
                    state_arrays.entry(flat).or_insert(len);
                }
            }
        }
    }

    struct_instances.insert(target.to_owned(), struct_name.to_owned());
}

#[allow(clippy::too_many_arguments)]
fn analyze_struct_field_init_assign(
    base: &str,
    field: &str,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    state_arrays: &mut HashMap<String, usize>,
    state_array_struct_roots: &mut HashMap<String, ArrayStructRootInfo>,
    struct_instances: &HashMap<String, String>,
    outputs: &HashSet<String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    fn_signatures: &HashMap<String, FnSignature>,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(struct_name) = struct_instances.get(base) else {
        errors.push(Diagnostic::semantic(
            format!("unknown struct instance '{base}'"),
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
            format!("struct '{}' has no field '{}'", struct_name, field),
            0,
            0,
        ));
        return;
    };

    let flat = format!("{base}.{field}");
    match field_decl.ty {
        TypedFieldType::Scalar(prim) => {
            if matches!(expr, Expr::ArrayCtor { .. }) {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' is scalar and cannot be assigned array[...]"),
                    0,
                    0,
                ));
                return;
            }
            validate_expr(
                expr,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs,
                    array_vars: state_arrays,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope: ScopeKind::Init,
                },
                errors,
            );
            let expr_ty = infer_expr_type_for_semantics(
                expr,
                state_scalars,
                None,
                locals,
                &HashSet::new(),
                outputs,
                &HashSet::new(),
                struct_instances,
                struct_defs,
                errors,
            );
            require_assignable_type(
                expr_ty,
                prim,
                &format!("struct field init '{flat}'"),
                errors,
            );
            state_scalars.insert(flat.clone(), prim);
            known_scalars.insert(flat);
        }
        TypedFieldType::Array(expected_len) => {
            let Expr::ArrayCtor { spec, .. } = expr else {
                errors.push(Diagnostic::semantic(
                    format!("field '{flat}' requires array[{expected_len}] initialization"),
                    0,
                    0,
                ));
                return;
            };
            let context = format!("array constructor for '{flat}'");
            let size_context = format!("array constructor size for '{flat}'");
            let Some(actual_len) = eval_data_size_expr(&spec.size, options, &size_context, errors)
            else {
                return;
            };
            if actual_len != expected_len {
                errors.push(Diagnostic::semantic(
                    format!(
                        "field '{flat}' requires array[{expected_len}] but got array[{actual_len}]"
                    ),
                    0,
                    0,
                ));
                return;
            }
            match (&field_decl.array_elem_struct, &spec.elem) {
                (None, ArrayElemType::Primitive(elem_ty)) => {
                    let expected_elem_ty = field_decl.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    if expected_elem_ty != *elem_ty {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "field '{flat}' expects array[{:?}, N] but constructor uses array[{:?}, N]",
                                expected_elem_ty, elem_ty
                            ),
                            0,
                            0,
                        ));
                        return;
                    }
                    state_scalars.insert(
                        declared_type_key(DECLARED_DATA_ELEM_TYPE_PREFIX, &flat),
                        expected_elem_ty,
                    );
                    state_arrays.entry(flat).or_insert(expected_len);
                }
                (None, ArrayElemType::Struct(name)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects primitive array but constructor uses struct element type '{name}'"
                        ),
                        0,
                        0,
                    ));
                }
                (Some(expected_struct), ArrayElemType::Struct(actual_struct))
                    if expected_struct == actual_struct =>
                {
                    if !register_data_struct_root(
                        &flat,
                        expected_struct,
                        expected_len,
                        struct_defs,
                        &context,
                        state_scalars,
                        state_arrays,
                        state_array_struct_roots,
                        errors,
                    ) {
                        return;
                    }
                }
                (Some(expected_struct), ArrayElemType::Struct(actual_struct)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects array[{expected_struct}, N] but constructor uses array[{actual_struct}, N]"
                        ),
                        0,
                        0,
                    ));
                }
                (Some(expected_struct), ArrayElemType::Primitive(other)) => {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "field '{flat}' expects array[{expected_struct}, N] but constructor uses primitive element type {:?}",
                            other
                        ),
                        0,
                        0,
                    ));
                }
            }
        }
    }
}
