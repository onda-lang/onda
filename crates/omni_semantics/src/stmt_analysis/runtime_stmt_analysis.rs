use super::*;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(crate) fn runtime_symbol_root(name: &str) -> &str {
    name.split('.').next().unwrap_or(name)
}

fn infer_runtime_slice_alias_info(
    base: &str,
    start: Option<&Expr>,
    end: Option<&Expr>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_slice_alias_info(
        base,
        start,
        end,
        declared_symbols,
        Some(state_arrays),
        local_array_aliases,
        struct_instances,
        struct_defs,
        errors,
    )
}

fn infer_runtime_data_like_info(
    expr: &Expr,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    struct_instances: &HashMap<String, String>,
    struct_defs: &HashMap<String, Vec<TypedStructField>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<LocalArrayAliasInfo> {
    infer_scope_data_like_info(
        expr,
        declared_symbols,
        Some(state_arrays),
        local_array_aliases,
        struct_instances,
        struct_defs,
        errors,
    )
}

pub(crate) struct RuntimeStmtAnalysisCtx<'a> {
    pub common: ScopeAnalysisCtx<'a>,
    pub registration_mode: RuntimeRegistrationMode,
    pub declared_symbols: &'a DeclaredSymbolMap,
    pub state_arrays: &'a HashMap<String, usize>,
    pub state_array_struct_roots: &'a HashMap<String, ArrayStructRootInfo>,
    pub nested_proc_instances: &'a HashMap<String, ProcNestedState>,
    pub struct_instances: &'a HashMap<String, String>,
    pub registration_input_names: &'a HashSet<String>,
    pub registration_output_names: &'a HashSet<String>,
    pub registration_param_names: &'a HashSet<String>,
    pub forbidden_assign_names: &'a HashSet<String>,
    pub proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    pub event_policy: Option<EventStmtPolicy<'a>>,
}

#[derive(Clone, Copy)]
pub(crate) struct EventStmtPolicy<'a> {
    pub init_writable_roots: &'a HashSet<String>,
    pub immutable_roots: &'a HashSet<String>,
    pub input_names: &'a HashSet<String>,
    pub output_names: &'a HashSet<String>,
    pub scalar_param_names: &'a HashSet<String>,
    pub array_param_names: &'a HashSet<String>,
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

pub(crate) type RuntimeStmtAnalysisState = ScopeFlowState;

fn has_local_binding_root(
    root: &str,
    locals: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> bool {
    fn has_prefixed_root<'a>(mut names: impl Iterator<Item = &'a String>, root: &str) -> bool {
        names.any(|name| {
            name.strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('.'))
        })
    }

    locals.contains(root)
        || local_aliases.contains_key(root)
        || local_array_aliases.contains_key(root)
        || local_proc_aliases.contains_key(root)
        || has_prefixed_root(local_aliases.keys(), root)
        || has_prefixed_root(local_array_aliases.keys(), root)
}

fn validate_event_assign_target_restrictions(
    target_loc: SourceLoc,
    target: &AssignTarget,
    locals: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &HashMap<String, ProcArrayAliasInfo>,
    event_policy: EventStmtPolicy<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let (base, indexed) = match target {
        AssignTarget::Var(name) => (name.as_str(), false),
        AssignTarget::Index { base, .. } => (base.as_str(), true),
        AssignTarget::Slice { base, .. } => (base.as_str(), true),
    };
    let root = runtime_symbol_root(base);

    if event_policy.scalar_param_names.contains(root) {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to immutable event parameter '{}'", root),
            target_loc,
        ));
        return;
    }
    if event_policy.array_param_names.contains(root) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "cannot assign to immutable event array parameter '{}'",
                root
            ),
            target_loc,
        ));
        return;
    }
    if event_policy.input_names.contains(root) {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to input symbol '{}' in event handler", root),
            target_loc,
        ));
        return;
    }
    if event_policy.output_names.contains(root) {
        errors.push(Diagnostic::semantic_span(
            format!("cannot assign to output symbol '{}' in event handler", root),
            target_loc,
        ));
        return;
    }
    if event_policy.immutable_roots.contains(root) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "event handlers can only write init-root state; '{}' is not init-root",
                root
            ),
            target_loc,
        ));
        return;
    }

    if indexed || base.contains('.') {
        let is_local_root = has_local_binding_root(
            root,
            locals,
            local_aliases,
            local_array_aliases,
            local_proc_aliases,
        );
        if !event_policy.init_writable_roots.contains(root) && !is_local_root {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "event handlers can only write init-root state; '{}' is not init-root",
                    root
                ),
                target_loc,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_runtime_stmt_analysis_ctx<'a>(
    common: ScopeAnalysisCtx<'a>,
    registration_mode: RuntimeRegistrationMode,
    declared_symbols: &'a DeclaredSymbolMap,
    state_arrays: &'a HashMap<String, usize>,
    state_array_struct_roots: &'a HashMap<String, ArrayStructRootInfo>,
    nested_proc_instances: &'a HashMap<String, ProcNestedState>,
    proc_array_roots: &'a HashMap<String, ProcNestedArrayState>,
    struct_instances: &'a HashMap<String, String>,
    registration_input_names: &'a HashSet<String>,
    registration_output_names: &'a HashSet<String>,
    registration_param_names: &'a HashSet<String>,
    forbidden_assign_names: &'a HashSet<String>,
    event_policy: Option<EventStmtPolicy<'a>>,
) -> RuntimeStmtAnalysisCtx<'a> {
    RuntimeStmtAnalysisCtx {
        common,
        registration_mode,
        declared_symbols,
        state_arrays,
        state_array_struct_roots,
        nested_proc_instances,
        proc_array_roots,
        struct_instances,
        registration_input_names,
        registration_output_names,
        registration_param_names,
        forbidden_assign_names,
        event_policy,
    }
}

fn build_runtime_stmt_analysis_state(
    known_scalars: HashSet<String>,
    local_aliases: LocalAliasTypes,
    local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
) -> RuntimeStmtAnalysisState {
    ScopeFlowState::new(known_scalars, local_aliases, local_array_aliases)
}

fn build_runtime_stmt_expr_env<'a>(
    expr_inputs: ScopeExprInputs<'a>,
    flow_state: &'a RuntimeStmtAnalysisState,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
) -> StmtExprAnalysisEnv<'a> {
    build_scope_stmt_expr_env(
        expr_inputs,
        &flow_state.known_scalars,
        &flow_state.local_aliases,
        &flow_state.local_array_aliases,
        array_vars,
        scope,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_runtime_scope_stmts<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    common: ScopeAnalysisCtx<'a>,
    registration_mode: RuntimeRegistrationMode,
    locals: &HashSet<String>,
    known_scalars: HashSet<String>,
    local_aliases: LocalAliasTypes,
    local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    struct_instances: &HashMap<String, String>,
    forbidden_assign_names: &HashSet<String>,
    event_policy: Option<EventStmtPolicy<'a>>,
    errors: &mut Vec<Diagnostic>,
) {
    let mut state_scalars = state_scalars.clone();
    let ctx = build_runtime_stmt_analysis_ctx(
        common,
        registration_mode,
        declared_symbols,
        state_arrays,
        state_array_struct_roots,
        nested_proc_instances,
        proc_array_roots,
        struct_instances,
        common.input_names,
        common.output_names,
        common.param_names,
        forbidden_assign_names,
        event_policy,
    );
    let mut state =
        build_runtime_stmt_analysis_state(known_scalars, local_aliases, local_array_aliases);
    analyze_runtime_stmts(stmts, locals, &mut state_scalars, &ctx, &mut state, errors);
}

pub(crate) fn build_known_scalars_from_state(
    base_names: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
) -> HashSet<String> {
    let mut known = base_names.clone();
    known.extend(state_scalars.keys().cloned());
    known
}

pub(crate) fn extend_known_scalars<'a>(
    known_scalars: &mut HashSet<String>,
    names: impl IntoIterator<Item = &'a String>,
) {
    known_scalars.extend(names.into_iter().cloned());
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn register_and_analyze_runtime_scope<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    common: ScopeAnalysisCtx<'a>,
    registration_mode: RuntimeRegistrationMode,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    struct_instances: &HashMap<String, String>,
    registration_input_names: &HashSet<String>,
    registration_output_names: &HashSet<String>,
    registration_param_names: &HashSet<String>,
    runtime_locals: &HashSet<String>,
    runtime_known_scalars: HashSet<String>,
    runtime_local_aliases: LocalAliasTypes,
    runtime_local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    runtime_forbidden_assign_names: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    let ctx = build_runtime_stmt_analysis_ctx(
        common,
        registration_mode,
        declared_symbols,
        state_arrays,
        state_array_struct_roots,
        nested_proc_instances,
        proc_array_roots,
        struct_instances,
        registration_input_names,
        registration_output_names,
        registration_param_names,
        runtime_forbidden_assign_names,
        None,
    );
    let mut state = build_runtime_stmt_analysis_state(
        runtime_known_scalars,
        runtime_local_aliases,
        runtime_local_array_aliases,
    );
    analyze_runtime_stmts(
        stmts,
        runtime_locals,
        state_scalars,
        &ctx,
        &mut state,
        errors,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_runtime_events(
    typed_events: &[TypedEvent],
    event_known_scalars_seed: &HashSet<String>,
    event_array_alias_seed: &HashMap<String, LocalArrayAliasInfo>,
    event_immutable_param_seed: &HashSet<String>,
    init_writable_roots: &HashSet<String>,
    immutable_event_roots: &HashSet<String>,
    validation_input_names: &HashSet<String>,
    validation_output_names: &HashSet<String>,
    state_scalars: &HashMap<String, PrimitiveType>,
    declared_symbols: &DeclaredSymbolMap,
    state_arrays: &HashMap<String, usize>,
    state_array_struct_roots: &HashMap<String, ArrayStructRootInfo>,
    nested_proc_instances: &HashMap<String, ProcNestedState>,
    proc_array_roots: &HashMap<String, ProcNestedArrayState>,
    struct_instances: &HashMap<String, String>,
    common: ScopeAnalysisCtx<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let empty_runtime_inputs = HashSet::<String>::new();
    let empty_runtime_outputs = HashSet::<String>::new();
    let runtime_loop_vars = HashSet::<String>::new();
    let empty_proc_array_roots = HashMap::<String, ProcNestedArrayState>::new();

    for event in typed_events {
        let mut scalar_event_params = HashSet::<String>::new();
        let mut array_event_params = HashSet::<String>::new();
        let mut event_known_scalars = event_known_scalars_seed.clone();
        let mut event_local_aliases = LocalAliasTypes::new();
        let mut event_local_data_aliases = event_array_alias_seed.clone();
        for param in &event.params {
            match param.ty {
                TypedEventParamType::Scalar(ty) => {
                    scalar_event_params.insert(param.name.clone());
                    event_known_scalars.insert(param.name.clone());
                    event_local_aliases.insert(param.name.clone(), ty);
                }
                TypedEventParamType::Array { elem, len } => {
                    array_event_params.insert(param.name.clone());
                    event_local_data_aliases.insert(
                        param.name.clone(),
                        LocalArrayAliasInfo {
                            len,
                            elem_ty: elem,
                            elem_struct: None,
                            writable: false,
                        },
                    );
                }
                TypedEventParamType::Slice { elem } => {
                    array_event_params.insert(param.name.clone());
                    event_local_data_aliases.insert(
                        param.name.clone(),
                        LocalArrayAliasInfo {
                            len: 1,
                            elem_ty: elem,
                            elem_struct: None,
                            writable: false,
                        },
                    );
                }
            }
        }

        let mut event_param_immutable = event_immutable_param_seed.clone();
        event_param_immutable.extend(scalar_event_params.iter().cloned());

        let event_common = ScopeAnalysisCtx {
            policy: ScopePolicy::Event,
            input_names: &empty_runtime_inputs,
            output_names: &empty_runtime_outputs,
            param_names: &event_param_immutable,
            struct_defs: common.struct_defs,
            fn_signatures: common.fn_signatures,
            options: common.options,
        };
        let event_policy = EventStmtPolicy {
            init_writable_roots,
            immutable_roots: immutable_event_roots,
            input_names: validation_input_names,
            output_names: validation_output_names,
            scalar_param_names: &scalar_event_params,
            array_param_names: &array_event_params,
        };
        analyze_runtime_scope_stmts(
            event.body.iter(),
            event_common,
            RuntimeRegistrationMode::None,
            &runtime_loop_vars,
            event_known_scalars,
            event_local_aliases,
            event_local_data_aliases,
            state_scalars,
            declared_symbols,
            state_arrays,
            state_array_struct_roots,
            nested_proc_instances,
            if proc_array_roots.is_empty() {
                &empty_proc_array_roots
            } else {
                proc_array_roots
            },
            struct_instances,
            validation_output_names,
            Some(event_policy),
            errors,
        );
    }
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
            (ctx.common.scope_kind(), ctx.registration_mode),
            (ScopeKind::Sample, RuntimeRegistrationMode::None)
                | (ScopeKind::Block, RuntimeRegistrationMode::Block)
                | (ScopeKind::Sample, RuntimeRegistrationMode::Sample)
        ),
        "runtime analysis scope and registration mode must stay aligned"
    );
    analyze_runtime_scope(stmts, locals, state_scalars, ctx, state, 0, errors);
}

#[allow(clippy::too_many_arguments)]
fn analyze_runtime_scope<'a>(
    stmts: impl IntoIterator<Item = &'a Stmt>,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    ctx: &RuntimeStmtAnalysisCtx<'_>,
    state: &mut RuntimeStmtAnalysisState,
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
        ctx.common.struct_defs,
        ctx.registration_mode,
    );
    state.known_scalars.extend(state_scalars.keys().cloned());
    for stmt in stmts {
        analyze_runtime_stmt_inner(stmt, locals, state_scalars, ctx, state, loop_depth, errors);
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_runtime_stmt_inner(
    stmt: &Stmt,
    locals: &HashSet<String>,
    state_scalars: &mut HashMap<String, PrimitiveType>,
    ctx: &RuntimeStmtAnalysisCtx<'_>,
    state: &mut RuntimeStmtAnalysisState,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    let common = ctx.common;
    let declared_symbols = ctx.declared_symbols;
    let state_arrays = ctx.state_arrays;
    let state_array_struct_roots = ctx.state_array_struct_roots;
    let nested_proc_instances = ctx.nested_proc_instances;
    let struct_instances = ctx.struct_instances;
    let input_names = common.input_names;
    let output_names = common.output_names;
    let forbidden_assign_names = ctx.forbidden_assign_names;
    let param_names = common.param_names;
    let struct_defs = common.struct_defs;
    let fn_signatures = common.fn_signatures;
    let proc_array_roots = ctx.proc_array_roots;
    let options = common.options;
    let scope = common.scope_kind();

    with_stmt_diag_context(stmt, |diag| {
        let array_vars = merged_data_vars_for_runtime(state_arrays, &state.local_array_aliases);
        let empty_param_structs = HashMap::<String, String>::new();
        let expr_inputs = build_scope_analysis_expr_inputs(
            common,
            locals,
            state_scalars,
            declared_symbols,
            &empty_param_structs,
            struct_instances,
            output_names,
        );
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
            } => {
                if let Some(event_policy) = ctx.event_policy {
                    validate_event_assign_target_restrictions(
                        target_loc.as_ref().into(),
                        target,
                        locals,
                        &state.local_aliases,
                        &state.local_array_aliases,
                        &state.local_proc_aliases,
                        event_policy,
                        errors,
                    );
                }
                analyze_assign_sample(
                    target_loc.as_ref().into(),
                    target,
                    decl_ty,
                    generic_decl_ty,
                    *is_typed_decl,
                    scope,
                    expr,
                    &mut state.known_scalars,
                    &mut state.local_aliases,
                    &mut state.local_array_aliases,
                    &mut state.local_proc_aliases,
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
                let expr = rewrite_proc_alias_calls_for_validation(expr, &state.local_proc_aliases);
                if is_proc_event_stmt_call(&expr, nested_proc_instances, proc_array_roots) {
                    if let Expr::UserCall { args, .. } = &expr {
                        for arg in args {
                            analyze_proc_event_arg_expr(
                                &arg.expr,
                                build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                                errors,
                            );
                        }
                    }
                } else {
                    analyze_stmt_expr(
                        &expr,
                        build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                        errors,
                    );
                }
            }
            Stmt::Return { .. } => {
                push_semantic(diag, errors, "return is only allowed inside def blocks");
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let cond = rewrite_proc_alias_calls_for_validation(cond, &state.local_proc_aliases);
                require_validated_bool_stmt_expr(
                    &cond,
                    "if condition",
                    build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                    errors,
                );
                let mut then_state = state.clone();
                analyze_runtime_scope(
                    then_branch.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut then_state,
                    loop_depth,
                    errors,
                );
                let mut else_state = state.clone();
                analyze_runtime_scope(
                    else_branch.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut else_state,
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
                let start =
                    rewrite_proc_alias_calls_for_validation(start, &state.local_proc_aliases);
                let end = rewrite_proc_alias_calls_for_validation(end, &state.local_proc_aliases);
                require_validated_numeric_stmt_expr(
                    &start,
                    "for loop start bound",
                    build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                    errors,
                );
                require_validated_numeric_stmt_expr(
                    &end,
                    "for loop end bound",
                    build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                    errors,
                );
                let rewritten_step = step.as_ref().map(|step_expr| {
                    rewrite_proc_alias_calls_for_validation(step_expr, &state.local_proc_aliases)
                });
                validate_for_loop_step_expr(
                    rewritten_step.as_ref(),
                    build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                    errors,
                );
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone());
                let mut loop_state = state.clone();
                analyze_runtime_scope(
                    body.iter(),
                    &loop_locals,
                    state_scalars,
                    ctx,
                    &mut loop_state,
                    loop_depth + 1,
                    errors,
                );
            }
            Stmt::While { cond, body, .. } => {
                let cond = rewrite_proc_alias_calls_for_validation(cond, &state.local_proc_aliases);
                require_validated_bool_stmt_expr(
                    &cond,
                    "while condition",
                    build_runtime_stmt_expr_env(expr_inputs, state, &array_vars, scope),
                    errors,
                );
                let mut loop_state = state.clone();
                analyze_runtime_scope(
                    body.iter(),
                    locals,
                    state_scalars,
                    ctx,
                    &mut loop_state,
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
    target_loc: SourceLoc,
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
    let empty_param_structs = HashMap::<String, String>::new();
    let expr_inputs = ScopeExprInputs {
        locals,
        state_scalars,
        declared_symbols,
        param_structs: &empty_param_structs,
        struct_instances,
        input_names,
        output_names,
        param_names,
        struct_defs,
        fn_signatures,
        expr_outputs: output_names,
    };
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
    let scope_expr_env = build_scope_expr_env(expr_inputs, known_scalars, &array_vars, scope);
    macro_rules! target_error {
        ($message:expr $(,)?) => {
            errors.push(Diagnostic::semantic_span($message, target_loc))
        };
    }
    match target {
        AssignTarget::Index { base, index } => {
            let expr_for_validation =
                rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
            if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                target_error!("typed declaration is only supported for plain scalar variables",);
            }
            if state_array_struct_roots.contains_key(base) {
                target_error!(
                    format!(
                        "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                    ),
                );
                return;
            }
            if let Some(alias) = local_array_aliases.get(base) {
                if !alias.writable {
                    target_error!(format!("cannot assign to immutable array alias '{base}'"),);
                    return;
                }
                if alias.elem_struct.is_some() {
                    target_error!(
                        format!(
                            "indexed assignment target '{base}[...]' is array[Struct, N]; assign fields through an alias (for example 'x = {base}[i]; x.field = ...')"
                        ),
                    );
                    return;
                }
            }
            if !state_arrays.contains_key(base)
                && !local_array_aliases.contains_key(base)
                && !has_declared_buffer_symbol_info(declared_symbols, base)
            {
                target_error!(format!(
                    "indexed assignment target '{base}[...]' is not a array/buffer symbol"
                ),);
            } else if is_declared_multichannel_buffer_info(declared_symbols, base) {
                target_error!(
                    format!(
                        "indexed assignment target '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                );
            }
            validate_expr(index, scope_expr_env, errors);
            validate_expr(&expr_for_validation, scope_expr_env, errors);
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
            require_expr_numeric_type(index, index_ty, "array index expression", errors);
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
            require_expr_assignable_type(expr, expr_ty, expected_ty, "array/buffer write", errors);
        }
        AssignTarget::Slice { base, start, end } => {
            let expr_for_validation =
                rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
            if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                target_error!("typed declaration is only supported for plain scalar variables",);
            }
            let Some(target_info) = infer_runtime_slice_alias_info(
                base,
                start.as_ref(),
                end.as_ref(),
                declared_symbols,
                state_arrays,
                local_array_aliases,
                struct_instances,
                struct_defs,
                errors,
            ) else {
                return;
            };
            if !target_info.writable {
                target_error!(format!("cannot assign to immutable array alias '{base}'"),);
                return;
            }
            if let Some(start) = start {
                validate_expr(start, stmt_expr_env(scope).expr_env, errors);
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
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_numeric_type(start, start_ty, "slice start bound", errors);
            }
            if let Some(end) = end {
                validate_expr(end, stmt_expr_env(scope).expr_env, errors);
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
                    struct_instances,
                    struct_defs,
                    errors,
                );
                require_expr_numeric_type(end, end_ty, "slice end bound", errors);
            }
            if is_data_like_value_expr(&expr_for_validation, stmt_expr_env(scope)) {
                validate_data_like_value_expr(&expr_for_validation, stmt_expr_env(scope), errors);
                if let Some(src_info) = infer_runtime_data_like_info(
                    &expr_for_validation,
                    declared_symbols,
                    state_arrays,
                    local_array_aliases,
                    struct_instances,
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
                validate_expr(&expr_for_validation, stmt_expr_env(scope).expr_env, errors);
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
                require_expr_assignable_type(
                    expr,
                    expr_ty,
                    target_info.elem_ty,
                    "slice fill assignment",
                    errors,
                );
            }
        }
        AssignTarget::Var(name) => {
            if !matches!(expr, Expr::Index { .. }) {
                local_proc_aliases.remove(name);
            }
            if locals.contains(name) {
                target_error!(format!("cannot assign to loop variable '{name}'"),);
            }
            if is_builtin_constant_name(name) {
                target_error!(format!("cannot assign to builtin constant '{name}'"),);
            }
            if forbidden_assign_names.contains(name) {
                target_error!(format!("cannot assign to output symbol '{name}' in block"),);
            }
            if let Expr::ArrayCtor { spec, init, .. } = expr {
                if is_typed_decl {
                    if decl_ty.is_some() {
                        target_error!(
                            "typed declaration cannot combine scalar type annotation with array constructor",
                        );
                        return;
                    }
                    if split_field_path(name, errors).is_some() {
                        target_error!(
                            "typed array declaration target must be a plain variable name",
                        );
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
                        target_error!(format!(
                            "typed array declaration for '{name}' conflicts with existing symbol"
                        ),);
                        return;
                    }
                    let size_context =
                        format!("typed array declaration size for symbol '{name}' in sample");
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
                                for (idx, value) in values.iter().take(size_value).enumerate() {
                                    validate_expr(value, scope_expr_env, errors);
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
                                        "typed array declaration '{name}: {struct_name}[N]' is not yet supported in sample/block"
                                    ),
                                );
                            });
                        }
                    }
                    return;
                }
            }
            if let Expr::ArrayLiteral { values, .. } = expr {
                if decl_ty.is_some() {
                    target_error!(
                        format!(
                            "typed declaration for '{name}' with array literals must use explicit array type syntax like '{name}: T[N] = [...]'"
                        ),
                    );
                    return;
                }
                if split_field_path(name, errors).is_some() {
                    target_error!("array declaration target must be a plain variable name",);
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
                    target_error!(format!(
                        "array declaration for '{name}' conflicts with existing symbol"
                    ),);
                    return;
                }
                if values.is_empty() {
                    with_expr_diag_context(expr, |expr_diag| {
                        push_semantic(
                            expr_diag,
                            errors,
                            format!("array initializer for symbol '{name}' cannot be empty"),
                        );
                    });
                    return;
                }
                for value in values {
                    validate_expr(value, scope_expr_env, errors);
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
                if decl_ty.is_some() || generic_decl_ty.is_some() || is_typed_decl {
                    target_error!(format!(
                        "typed declaration for '{name}' is not supported for slice aliases"
                    ),);
                    return;
                }
                if split_field_path(name, errors).is_some() {
                    target_error!("slice alias target must be a plain variable name",);
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
                    target_error!(format!(
                        "slice alias declaration for '{name}' conflicts with existing symbol"
                    ),);
                    return;
                }
                validate_expr(expr, scope_expr_env, errors);
                if let Some(alias) = infer_runtime_slice_alias_info(
                    base,
                    start.as_deref(),
                    end.as_deref(),
                    declared_symbols,
                    state_arrays,
                    local_array_aliases,
                    struct_instances,
                    struct_defs,
                    errors,
                ) {
                    local_array_aliases.insert(name.clone(), alias);
                }
                return;
            }

            if local_aliases.contains_key(name) {
                if matches!(expr, Expr::ArrayCtor { .. }) {
                    target_error!("array[...] construction is only allowed in init",);
                }
                if let Expr::UserCall { name: ctor, .. } = expr {
                    if struct_defs.contains_key(ctor) {
                        target_error!("struct construction is only allowed in init",);
                    }
                }
                validate_expr(expr, scope_expr_env, errors);
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
                require_expr_assignable_type(
                    expr,
                    expr_ty,
                    *local_aliases.get(name).unwrap_or(&PrimitiveType::F32),
                    &format!("alias assignment to '{name}'"),
                    errors,
                );
                known_scalars.insert(name.clone());
                return;
            }
            if local_array_aliases.contains_key(name) {
                target_error!(format!(
                    "array alias '{name}' must be written using '{name}[index] = value'"
                ),);
                return;
            }

            if let Some((base, field)) = split_field_path(name, errors) {
                if let Some(struct_name) = struct_instances.get(base) {
                    let Some(fields) = struct_defs.get(struct_name) else {
                        return;
                    };
                    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                        target_error!(format!("struct '{}' has no field '{}'", struct_name, field));
                        return;
                    };
                    let flat = format!("{base}.{field}");
                    match field_decl.ty {
                        TypedFieldType::Scalar(prim) => {
                            if !state_scalars.contains_key(&flat) {
                                target_error!(format!(
                                    "struct field '{flat}' must be initialized in init"
                                ));
                            }
                            validate_expr(expr, scope_expr_env, errors);
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
                            require_expr_assignable_type(
                                expr,
                                expr_ty,
                                prim,
                                &format!("sample assignment to '{flat}'"),
                                errors,
                            );
                        }
                        TypedFieldType::Array(_) => {
                            target_error!(format!(
                                "array field '{flat}' must be accessed with index syntax"
                            ));
                        }
                        TypedFieldType::Struct => {
                            target_error!(
                                format!(
                                    "nested struct field '{flat}' must be accessed via subfields or methods"
                                )
                            );
                        }
                    }
                    return;
                }

                let flat = format!("{base}.{field}");
                if !state_scalars.contains_key(&flat)
                    && !state_arrays.contains_key(&flat)
                    && !state_array_struct_roots.contains_key(&flat)
                {
                    target_error!(format!("unknown struct instance '{base}'"));
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
                if let Expr::Index { base, index, .. } = expr {
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
                        validate_expr(index, scope_expr_env, errors);
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
                        require_expr_numeric_type(index, idx_ty, "array index expression", errors);
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
                target_error!(format!(
                    "cannot assign to immutable symbol '{name}' in sample block"
                ),);
            }
            if struct_instances.contains_key(name) {
                target_error!(format!(
                    "struct instance '{name}' cannot be assigned in sample"
                ),);
            }
            if matches!(expr, Expr::ArrayCtor { .. }) {
                target_error!("array[...] construction is only allowed in init",);
            }
            if let Expr::UserCall { name: ctor, .. } = expr {
                if struct_defs.contains_key(ctor) {
                    target_error!("struct construction is only allowed in init",);
                }
            }
            if state_arrays.contains_key(name) || state_array_struct_roots.contains_key(name) {
                target_error!(format!(
                    "array symbol '{name}' must be written using '{name}[index] = value'"
                ),);
            }
            if local_array_aliases.contains_key(name) {
                target_error!(format!(
                    "array alias '{name}' must be written using '{name}[index] = value'"
                ),);
            }
            if let Some(declared_ty) = *decl_ty {
                if output_names.contains(name) || local_aliases.contains_key(name) {
                    target_error!(format!(
                        "typed declaration for '{name}' is only allowed on first assignment"
                    ),);
                } else if let Some(existing_ty) = state_scalars.get(name).copied() {
                    if existing_ty != declared_ty {
                        target_error!(
                            format!(
                                "typed declaration for '{name}' conflicts with existing state type {:?}",
                                existing_ty
                            ),
                        );
                    }
                }
            }

            let expr_for_validation =
                rewrite_proc_alias_calls_for_validation(expr, local_proc_aliases);
            validate_expr(&expr_for_validation, scope_expr_env, errors);
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
                require_expr_assignable_type(
                    expr,
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
