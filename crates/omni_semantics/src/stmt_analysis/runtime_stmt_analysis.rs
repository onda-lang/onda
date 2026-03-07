use super::*;

#[derive(Clone)]
pub(crate) struct ProcArrayAliasInfo {
    pub(crate) array_base: String,
    pub(crate) index_expr: Expr,
}

pub(crate) fn rewrite_proc_alias_calls_for_validation(
    expr: &Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Expr {
    fn rewrite(expr: &mut Expr, aliases: &HashMap<String, ProcArrayAliasInfo>) {
        match expr {
            Expr::Var(name) => {
                if let Some((base, field)) = split_dot_path(name.as_str()) {
                    if let Some(alias) = aliases.get(base) {
                        *expr = Expr::UserCall {
                            name: format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
                            type_args: Vec::new(),
                            args: vec![
                                CallArg {
                                    name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                    expr: Expr::Var(alias.array_base.clone()),
                                },
                                CallArg {
                                    name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                    expr: alias.index_expr.clone(),
                                },
                                CallArg {
                                    name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                    expr: Expr::Var(field.to_owned()),
                                },
                            ],
                        };
                    }
                }
            }
            Expr::Index { index, .. } => rewrite(index, aliases),
            Expr::ArrayCtor { spec, init } => {
                rewrite(&mut spec.size, aliases);
                if let Some(values) = init {
                    for value in values {
                        rewrite(value, aliases);
                    }
                }
            }
            Expr::Compare { lhs, rhs, .. }
            | Expr::Logical { lhs, rhs, .. }
            | Expr::Binary { lhs, rhs, .. } => {
                rewrite(lhs, aliases);
                rewrite(rhs, aliases);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    rewrite(arg, aliases);
                }
            }
            Expr::UserCall { name, args, .. } => {
                for arg in args.iter_mut() {
                    rewrite(&mut arg.expr, aliases);
                }
                if let Some(alias) = aliases.get(name) {
                    let mut rest = std::mem::take(args);
                    rest.retain(|arg| {
                        !matches!(
                            arg.name.as_deref(),
                            Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
                        )
                    });
                    let mut rewritten = Vec::<CallArg>::with_capacity(rest.len() + 2);
                    rewritten.push(CallArg {
                        name: None,
                        expr: Expr::Var(alias.array_base.clone()),
                    });
                    rewritten.push(CallArg {
                        name: None,
                        expr: alias.index_expr.clone(),
                    });
                    rewritten.extend(rest);
                    *args = rewritten;
                    *name = PROC_INDEX_CALL_SENTINEL.to_owned();
                    return;
                }
                if let Some((base, field)) = split_dot_path(name.as_str()) {
                    if let Some(alias) = aliases.get(base) {
                        let mut rest = std::mem::take(args);
                        rest.retain(|arg| {
                            !matches!(
                                arg.name.as_deref(),
                                Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
                            )
                        });
                        let mut rewritten = Vec::<CallArg>::with_capacity(rest.len() + 2);
                        rewritten.push(CallArg {
                            name: None,
                            expr: Expr::Var(alias.array_base.clone()),
                        });
                        rewritten.push(CallArg {
                            name: None,
                            expr: alias.index_expr.clone(),
                        });
                        rewritten.extend(rest);
                        *args = rewritten;
                        *name = format!("{PROC_INDEX_CALL_SENTINEL}.{field}");
                    }
                }
            }
            Expr::Cast { expr: inner, .. }
            | Expr::UnaryNot { expr: inner }
            | Expr::UnaryBitNot { expr: inner } => {
                rewrite(inner, aliases);
            }
            Expr::ArrayLiteral(values) => {
                for value in values {
                    rewrite(value, aliases);
                }
            }
            Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        }
    }

    let mut rewritten = expr.clone();
    rewrite(&mut rewritten, aliases);
    rewritten
}

pub(crate) fn merged_data_vars_for_runtime(
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

pub(crate) struct RuntimeStmtAnalysisCtx<'a> {
    pub scope: ScopeKind,
    pub registration_mode: RuntimeRegistrationMode,
    pub declared_symbols: &'a DeclaredSymbolMap,
    pub state_arrays: &'a HashMap<String, usize>,
    pub state_array_struct_roots: &'a HashMap<String, ArrayStructRootInfo>,
    pub nested_proc_instances: &'a HashMap<String, ProcNestedState>,
    pub struct_instances: &'a HashMap<String, String>,
    pub registration_input_names: &'a HashSet<String>,
    pub registration_output_names: &'a HashSet<String>,
    pub registration_param_names: &'a HashSet<String>,
    pub input_names: &'a HashSet<String>,
    pub output_names: &'a HashSet<String>,
    pub forbidden_assign_names: &'a HashSet<String>,
    pub param_names: &'a HashSet<String>,
    pub struct_defs: &'a HashMap<String, Vec<TypedStructField>>,
    pub fn_signatures: &'a HashMap<String, FnSignature>,
    pub proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    pub options: AnalysisOptions,
}

fn is_proc_event_stmt_call(
    expr: &Expr,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
) -> bool {
    let Expr::UserCall { name, .. } = expr else {
        return false;
    };
    let Some((base, _event_name)) = split_dot_path(name) else {
        return false;
    };
    base == PROC_INDEX_CALL_SENTINEL
        || nested_proc_instances.contains_key(base)
        || proc_array_roots.contains_key(base)
}

#[derive(Clone)]
pub(crate) struct RuntimeStmtAnalysisState {
    pub known_scalars: HashSet<String>,
    pub local_aliases: LocalAliasTypes,
    pub local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub local_proc_aliases: HashMap<String, ProcArrayAliasInfo>,
}

pub(crate) fn analyze_runtime_stmts<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    ctx: &RuntimeStmtAnalysisCtx<'_>,
    state: &mut RuntimeStmtAnalysisState,
    errors: &mut Vec<Diagnostic>,
) {
    debug_assert!(
        matches!(
            (ctx.scope, ctx.registration_mode),
            (ScopeKind::Sample, RuntimeRegistrationMode::None)
                | (ScopeKind::Block, RuntimeRegistrationMode::Block)
                | (ScopeKind::Sample, RuntimeRegistrationMode::Sample)
        ),
        "runtime analysis scope and registration mode must stay aligned"
    );
    analyze_runtime_scope(
        stmts,
        locals,
        state_scalars,
        ctx,
        &mut state.known_scalars,
        &mut state.local_aliases,
        &mut state.local_array_aliases,
        &mut state.local_proc_aliases,
        0,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
fn analyze_runtime_scope<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    ctx: &RuntimeStmtAnalysisCtx<'_>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    let stmts = stmts.into_iter().collect::<Vec<_>>();
    register_scope_state(
        stmts.iter().copied(),
        state_scalars,
        ctx.declared_symbols,
        ctx.state_arrays,
        ctx.state_array_struct_roots,
        ctx.struct_instances,
        ctx.registration_input_names,
        ctx.registration_output_names,
        ctx.registration_param_names,
        ctx.struct_defs,
        ctx.registration_mode,
    );
    known_scalars.extend(state_scalars.keys().cloned());
    for stmt in stmts {
        analyze_runtime_stmt_inner(
            stmt,
            locals,
            state_scalars,
            ctx,
            known_scalars,
            local_aliases,
            local_array_aliases,
            local_proc_aliases,
            loop_depth,
            errors,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_runtime_stmt_inner(
    stmt: &Stmt,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    ctx: &RuntimeStmtAnalysisCtx<'_>,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    let declared_symbols = ctx.declared_symbols;
    let state_arrays = ctx.state_arrays;
    let state_array_struct_roots = ctx.state_array_struct_roots;
    let nested_proc_instances = ctx.nested_proc_instances;
    let struct_instances = ctx.struct_instances;
    let input_names = ctx.input_names;
    let output_names = ctx.output_names;
    let forbidden_assign_names = ctx.forbidden_assign_names;
    let param_names = ctx.param_names;
    let struct_defs = ctx.struct_defs;
    let fn_signatures = ctx.fn_signatures;
    let proc_array_roots = ctx.proc_array_roots;
    let options = ctx.options;

    with_stmt_diag_context(stmt, || {
        let array_vars = merged_data_vars_for_runtime(state_arrays, local_array_aliases);
        let empty_param_structs = HashMap::<String, String>::new();
        let stmt_expr_env = |scope| StmtExprAnalysisEnv {
            expr_env: ExprEnv {
                known_scalars,
                locals,
                outputs: output_names,
                array_vars: &array_vars,
                declared_symbols,
                param_structs: &empty_param_structs,
                struct_instances,
                struct_defs,
                fn_signatures,
                allow_array_ctor: false,
                scope,
            },
            state_scalars,
            declared_symbols,
            local_aliases,
            local_array_aliases,
            input_names,
            output_names,
            param_names,
        };
        match stmt {
            Stmt::Const { .. } => {}
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
                    ctx.scope,
                    expr,
                    known_scalars,
                    local_aliases,
                    local_array_aliases,
                    local_proc_aliases,
                    locals,
                    state_scalars,
                    declared_symbols,
                    state_arrays,
                    state_array_struct_roots,
                    proc_array_roots,
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
                let expr = rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
                if is_proc_event_stmt_call(&expr, nested_proc_instances, proc_array_roots) {
                    if let Expr::UserCall { args, .. } = &expr {
                        for arg in args {
                            analyze_proc_event_arg_expr(
                                &arg.expr,
                                stmt_expr_env(ctx.scope),
                                errors,
                            );
                        }
                    }
                } else {
                    analyze_stmt_expr(&expr, stmt_expr_env(ctx.scope), errors);
                }
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
                let cond = rewrite_proc_alias_calls_for_validation(cond, local_proc_aliases);
                require_validated_bool_stmt_expr(
                    &cond,
                    "if condition",
                    stmt_expr_env(ctx.scope),
                    errors,
                );
                let mut then_known = known_scalars.clone();
                let mut then_aliases = local_aliases.clone();
                let mut then_data_aliases = local_array_aliases.clone();
                let mut then_proc_aliases = local_proc_aliases.clone();
                analyze_runtime_scope(
                    then_branch.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut then_known,
                    &mut then_aliases,
                    &mut then_data_aliases,
                    &mut then_proc_aliases,
                    loop_depth,
                    errors,
                );
                let mut else_known = known_scalars.clone();
                let mut else_aliases = local_aliases.clone();
                let mut else_data_aliases = local_array_aliases.clone();
                let mut else_proc_aliases = local_proc_aliases.clone();
                analyze_runtime_scope(
                    else_branch.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut else_known,
                    &mut else_aliases,
                    &mut else_data_aliases,
                    &mut else_proc_aliases,
                    loop_depth,
                    errors,
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
                let start = rewrite_proc_alias_calls_for_validation(start, local_proc_aliases);
                let end = rewrite_proc_alias_calls_for_validation(end, local_proc_aliases);
                require_validated_numeric_stmt_expr(
                    &start,
                    "for loop start bound",
                    stmt_expr_env(ctx.scope),
                    errors,
                );
                require_validated_numeric_stmt_expr(
                    &end,
                    "for loop end bound",
                    stmt_expr_env(ctx.scope),
                    errors,
                );
                let rewritten_step = step.as_ref().map(|step_expr| {
                    rewrite_proc_alias_calls_for_validation(step_expr, local_proc_aliases)
                });
                validate_for_loop_step_expr(
                    rewritten_step.as_ref(),
                    stmt_expr_env(ctx.scope),
                    errors,
                );
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_array_aliases.clone();
                let mut loop_proc_aliases = local_proc_aliases.clone();
                analyze_runtime_scope(
                    body.iter(),
                    &loop_locals,
                    state_scalars,
                    ctx,
                    &mut loop_known,
                    &mut loop_aliases,
                    &mut loop_data_aliases,
                    &mut loop_proc_aliases,
                    loop_depth + 1,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                let cond = rewrite_proc_alias_calls_for_validation(cond, local_proc_aliases);
                require_validated_bool_stmt_expr(
                    &cond,
                    "while condition",
                    stmt_expr_env(ctx.scope),
                    errors,
                );
                let mut loop_known = known_scalars.clone();
                let mut loop_aliases = local_aliases.clone();
                let mut loop_data_aliases = local_array_aliases.clone();
                let mut loop_proc_aliases = local_proc_aliases.clone();
                analyze_runtime_scope(
                    body.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut loop_known,
                    &mut loop_aliases,
                    &mut loop_data_aliases,
                    &mut loop_proc_aliases,
                    loop_depth + 1,
                    errors,
                );
            }
            Stmt::Break { .. } => require_loop_control_context("break", loop_depth, errors),
            Stmt::Continue { .. } => require_loop_control_context("continue", loop_depth, errors),
        }
    });
}
fn analyze_assign_sample(
    target: &AssignTarget,
    decl_ty: &Option<PrimitiveType>,
    generic_decl_ty: &Option<String>,
    is_typed_decl: bool,
    scope: ScopeKind,
    expr: &Expr,
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    locals: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
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
    let array_vars = merged_data_vars_for_runtime(state_arrays, local_array_aliases);
    match target {
        AssignTarget::Index { base, index } => {
            let expr_for_validation =
                rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
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
                && !has_declared_buffer_symbol_info(declared_symbols, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed assignment target '{base}[...]' is not a array/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_info(declared_symbols, base) {
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
                    declared_symbols,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope,
                },
                errors,
            );
            validate_expr(
                &expr_for_validation,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    array_vars: &array_vars,
                    declared_symbols,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope,
                },
                errors,
            );
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
                struct_instances,
                struct_defs,
                errors,
            );
            require_numeric_type(index_ty, "array index expression", errors);
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
                struct_instances,
                struct_defs,
                errors,
            );
            let expected_ty = local_array_aliases
                .get(base)
                .map(|a| a.elem_ty)
                .or_else(|| declared_symbol_scalar_type(declared_symbols, base))
                .unwrap_or(PrimitiveType::F32);
            require_assignable_type(expr_ty, expected_ty, "array/buffer write", errors);
        }
        AssignTarget::Var(name) => {
            if !matches!(expr, Expr::Index { .. }) {
                local_proc_aliases.remove(name);
            }
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
                                            declared_symbols,
                                            param_structs: &HashMap::new(),
                                            struct_instances,
                                            struct_defs,
                                            fn_signatures,
                                            allow_array_ctor: false,
                                            scope,
                                        },
                                        errors,
                                    );
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
                            declared_symbols,
                            param_structs: &HashMap::new(),
                            struct_instances,
                            struct_defs,
                            fn_signatures,
                            allow_array_ctor: false,
                            scope,
                        },
                        errors,
                    );
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
                    struct_instances,
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
                        declared_symbols,
                        param_structs: &HashMap::new(),
                        struct_instances,
                        struct_defs,
                        fn_signatures,
                        allow_array_ctor: false,
                        scope,
                    },
                    errors,
                );
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
                if let Some(struct_name) = struct_instances.get(base) {
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
                                    declared_symbols,
                                    param_structs: &HashMap::new(),
                                    struct_instances,
                                    struct_defs,
                                    fn_signatures,
                                    allow_array_ctor: false,
                                    scope,
                                },
                                errors,
                            );
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
                        TypedFieldType::Struct => {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "nested struct field '{flat}' must be accessed via subfields or methods"
                                ),
                                0,
                                0,
                            ));
                        }
                    }
                    return;
                }

                let flat = format!("{base}.{field}");
                if !state_scalars.contains_key(&flat)
                    && !state_arrays.contains_key(&flat)
                    && !state_array_struct_roots.contains_key(&flat)
                {
                    errors.push(Diagnostic::semantic(
                        format!("unknown struct instance '{base}'"),
                        0,
                        0,
                    ));
                    return;
                }
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
                    if let Some(binding_kind) = classify_runtime_like_indexed_binding(
                        base,
                        local_array_aliases,
                        state_arrays,
                        state_array_struct_roots,
                        struct_instances,
                        struct_defs,
                        proc_array_roots,
                        errors,
                    ) {
                        validate_expr(
                            index,
                            ExprEnv {
                                known_scalars,
                                locals,
                                outputs: output_names,
                                array_vars: &array_vars,
                                declared_symbols,
                                param_structs: &HashMap::new(),
                                struct_instances,
                                struct_defs,
                                fn_signatures,
                                allow_array_ctor: false,
                                scope,
                            },
                            errors,
                        );
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
                            struct_instances,
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
                                // Primitive array/buffer indexed reads are scalar expressions.
                                // Allow normal first-assignment local inference to handle:
                                //   x = arr[idx]
                            }
                        }
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

            let expr_for_validation =
                rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
            validate_expr(
                &expr_for_validation,
                ExprEnv {
                    known_scalars,
                    locals,
                    outputs: output_names,
                    array_vars: &array_vars,
                    declared_symbols,
                    param_structs: &HashMap::new(),
                    struct_instances,
                    struct_defs,
                    fn_signatures,
                    allow_array_ctor: false,
                    scope,
                },
                errors,
            );
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
                    declared_symbol_scalar_type(declared_symbols, name)
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
