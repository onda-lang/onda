use std::collections::{HashMap, HashSet};

use crate::*;

mod alias_support;
mod common;
mod indexed_binding;
mod init_analysis;
mod runtime_stmt_analysis;
pub(crate) use alias_support::*;
pub(crate) use common::*;
pub(crate) use indexed_binding::*;
pub(crate) use init_analysis::*;
pub(crate) use runtime_stmt_analysis::*;

#[derive(Clone, Copy)]
pub(crate) struct StmtExprAnalysisEnv<'a> {
    pub(crate) expr_env: ExprEnv<'a>,
    pub(crate) state_scalars: &'a HashMap<String, PrimitiveType>,
    pub(crate) declared_symbols: &'a DeclaredSymbolMap,
    pub(crate) local_aliases: &'a LocalAliasTypes,
    pub(crate) local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    pub(crate) input_names: &'a HashSet<String>,
    pub(crate) output_names: &'a HashSet<String>,
    pub(crate) param_names: &'a HashSet<String>,
}

#[derive(Clone)]
pub(crate) struct ScopeFlowState {
    pub(crate) known_scalars: HashSet<String>,
    pub(crate) local_aliases: LocalAliasTypes,
    pub(crate) local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    pub(crate) local_proc_aliases: HashMap<String, ProcArrayAliasInfo>,
    pub(crate) tuple_vars: HashMap<String, usize>,
}

impl ScopeFlowState {
    pub(crate) fn from_parts(
        known_scalars: HashSet<String>,
        local_aliases: LocalAliasTypes,
        local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
        local_proc_aliases: HashMap<String, ProcArrayAliasInfo>,
    ) -> Self {
        Self {
            known_scalars,
            local_aliases,
            local_array_aliases,
            local_proc_aliases,
            tuple_vars: HashMap::new(),
        }
    }

    pub(crate) fn new(
        known_scalars: HashSet<String>,
        local_aliases: LocalAliasTypes,
        local_array_aliases: HashMap<String, LocalArrayAliasInfo>,
    ) -> Self {
        Self::from_parts(
            known_scalars,
            local_aliases,
            local_array_aliases,
            HashMap::new(),
        )
    }
}

pub(crate) fn fork_scope_flow_state_with_tuples(
    known_scalars: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &HashMap<String, ProcArrayAliasInfo>,
    tuple_vars: &HashMap<String, usize>,
) -> ScopeFlowState {
    let mut st = ScopeFlowState::from_parts(
        known_scalars.clone(),
        local_aliases.clone(),
        local_array_aliases.clone(),
        local_proc_aliases.clone(),
    );
    st.tuple_vars = tuple_vars.clone();
    st
}

pub(crate) fn merge_branch_scope_flow_state(
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    tuple_vars: &mut HashMap<String, usize>,
    then_state: ScopeFlowState,
    else_state: ScopeFlowState,
) {
    let base_array_aliases = local_array_aliases.clone();
    let base_proc_aliases = local_proc_aliases.clone();
    let mut merged = known_scalars.clone();
    for name in &then_state.known_scalars {
        if else_state.known_scalars.contains(name) {
            merged.insert(name.clone());
        }
    }
    *known_scalars = merged;
    *local_aliases = then_state.local_aliases;
    local_aliases.extend(else_state.local_aliases);
    local_aliases.retain(|name, _| known_scalars.contains(name));
    let then_array_names = then_state
        .local_array_aliases
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let else_array_names = else_state
        .local_array_aliases
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    *local_array_aliases = then_state.local_array_aliases;
    for (k, v) in else_state.local_array_aliases {
        local_array_aliases.entry(k).or_insert(v);
    }
    local_array_aliases.retain(|name, _| {
        base_array_aliases.contains_key(name)
            || (then_array_names.contains(name) && else_array_names.contains(name))
    });
    let then_proc_names = then_state
        .local_proc_aliases
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    let else_proc_names = else_state
        .local_proc_aliases
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    *local_proc_aliases = then_state.local_proc_aliases;
    for (k, v) in else_state.local_proc_aliases {
        local_proc_aliases.entry(k).or_insert(v);
    }
    local_proc_aliases.retain(|name, _| {
        base_proc_aliases.contains_key(name)
            || (then_proc_names.contains(name) && else_proc_names.contains(name))
    });
    *tuple_vars = then_state
        .tuple_vars
        .into_iter()
        .filter_map(|(name, arity)| {
            else_state
                .tuple_vars
                .get(&name)
                .filter(|other_arity| **other_arity == arity && known_scalars.contains(&name))
                .map(|_| (name, arity))
        })
        .collect();
}

pub(crate) fn adopt_loop_scope_flow_state(
    known_scalars: &HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    tuple_vars: &mut HashMap<String, usize>,
    loop_state: ScopeFlowState,
) {
    let base_array_aliases = local_array_aliases.clone();
    let base_proc_aliases = local_proc_aliases.clone();
    *local_aliases = loop_state.local_aliases;
    local_aliases.retain(|name, _| known_scalars.contains(name));
    *local_array_aliases = loop_state.local_array_aliases;
    local_array_aliases.retain(|name, _| base_array_aliases.contains_key(name));
    *local_proc_aliases = loop_state.local_proc_aliases;
    local_proc_aliases.retain(|name, _| base_proc_aliases.contains_key(name));
    *tuple_vars = loop_state.tuple_vars;
    tuple_vars.retain(|name, _| known_scalars.contains(name));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_stmt_expr_env<'a>(
    expr_env: ExprEnv<'a>,
    state_scalars: &'a HashMap<String, PrimitiveType>,
    declared_symbols: &'a DeclaredSymbolMap,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    input_names: &'a HashSet<String>,
    output_names: &'a HashSet<String>,
    param_names: &'a HashSet<String>,
) -> StmtExprAnalysisEnv<'a> {
    StmtExprAnalysisEnv {
        expr_env,
        state_scalars,
        declared_symbols,
        local_aliases,
        local_array_aliases,
        input_names,
        output_names,
        param_names,
    }
}

pub(crate) fn build_scope_stmt_expr_env<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
) -> StmtExprAnalysisEnv<'a> {
    let mut expr_env =
        build_scope_expr_env(inputs, known_scalars, local_aliases, array_vars, scope);
    expr_env.local_array_aliases = local_array_aliases;
    build_stmt_expr_env(
        expr_env,
        inputs.state_scalars,
        inputs.declared_symbols,
        local_aliases,
        local_array_aliases,
        inputs.input_names,
        inputs.output_names,
        inputs.param_names,
    )
}

pub(crate) fn build_scope_expr_env_with_tuples<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
    tuple_vars: &'a HashMap<String, usize>,
) -> ExprEnv<'a> {
    let mut env = build_scope_expr_env(inputs, known_scalars, local_aliases, array_vars, scope);
    env.tuple_vars = tuple_vars;
    env
}

pub(crate) fn build_scope_stmt_expr_env_with_tuples<'a>(
    inputs: ScopeExprInputs<'a>,
    known_scalars: &'a HashSet<String>,
    local_aliases: &'a LocalAliasTypes,
    local_array_aliases: &'a HashMap<String, LocalArrayAliasInfo>,
    array_vars: &'a HashMap<String, usize>,
    scope: ScopeKind,
    tuple_vars: &'a HashMap<String, usize>,
) -> StmtExprAnalysisEnv<'a> {
    let mut expr_env = build_scope_expr_env_with_tuples(
        inputs,
        known_scalars,
        local_aliases,
        array_vars,
        scope,
        tuple_vars,
    );
    expr_env.local_array_aliases = local_array_aliases;
    build_stmt_expr_env(
        expr_env,
        inputs.state_scalars,
        inputs.declared_symbols,
        local_aliases,
        local_array_aliases,
        inputs.input_names,
        inputs.output_names,
        inputs.param_names,
    )
}

pub(crate) fn infer_tracked_tuple_arity(
    expr: &Expr,
    tuple_vars: &HashMap<String, usize>,
    fn_return_types: &HashMap<String, ReturnType>,
) -> Option<usize> {
    match expr {
        Expr::Tuple { values, .. } => Some(values.len()),
        Expr::UserCall { name, .. } => match fn_return_types.get(name.as_str()) {
            Some(ReturnType::Tuple(elem_tys)) => Some(elem_tys.len()),
            _ => None,
        },
        Expr::Var { name, .. } => tuple_vars.get(name).copied(),
        _ => None,
    }
}

pub(crate) fn track_tuple_var_assignment(
    tuple_vars: &mut HashMap<String, usize>,
    name: &str,
    tuple_arity: Option<usize>,
) {
    if let Some(arity) = tuple_arity {
        tuple_vars.insert(name.to_owned(), arity);
    } else {
        tuple_vars.remove(name);
    }
}

pub(crate) fn clear_tuple_var_bindings<'a>(
    tuple_vars: &mut HashMap<String, usize>,
    names: impl IntoIterator<Item = &'a String>,
) {
    for name in names {
        tuple_vars.remove(name);
    }
}

pub(crate) fn validate_and_infer_stmt_expr_type(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    validate_expr(expr, env.expr_env, errors);
    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
        expr,
        env.state_scalars,
        env.declared_symbols,
        None,
        env.local_aliases,
        env.local_array_aliases,
        env.expr_env.locals,
        env.input_names,
        env.output_names,
        env.param_names,
        env.expr_env.struct_instances,
        env.expr_env.struct_defs,
        env.expr_env.proc_array_roots,
        errors,
    )
}

pub(crate) fn analyze_stmt_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let _ = validate_and_infer_stmt_expr_type(expr, env, errors);
}

fn is_bare_array_ref_expr(expr: &Expr, env: StmtExprAnalysisEnv<'_>) -> bool {
    let Expr::Var { name, .. } = expr else {
        return false;
    };
    env.expr_env.array_vars.contains_key(name)
        || is_declared_data_array_symbol(env.declared_symbols, name)
        || has_declared_buffer_symbol_info(env.declared_symbols, name)
        || env.expr_env.proc_array_roots.contains_key(name)
}

pub(crate) fn is_data_like_value_expr(expr: &Expr, env: StmtExprAnalysisEnv<'_>) -> bool {
    matches!(expr, Expr::Slice { .. }) || is_bare_array_ref_expr(expr, env)
}

pub(crate) fn validate_data_like_value_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(name) = dynamic_param_surface_value_name(expr, env.expr_env) {
        errors.push(Diagnostic::semantic_span(
            format!(
                "dynamic param array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
            ),
            expr.loc(),
        ));
        return;
    }
    if let Some(name) = io_surface_value_name(expr, env.expr_env) {
        if env.expr_env.io_surface_access_allowed {
            errors.push(Diagnostic::semantic_span(
                format!(
                    "I/O array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
                ),
                expr.loc(),
            ));
        } else {
            push_io_surface_scope_error(errors, expr.loc(), name);
        }
        return;
    }
    if is_bare_array_ref_expr(expr, env) {
        return;
    }
    validate_expr(expr, env.expr_env, errors);
}

pub(crate) fn analyze_proc_event_arg_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if is_data_like_value_expr(expr, env) {
        validate_data_like_value_expr(expr, env, errors);
        return;
    }
    analyze_stmt_expr(expr, env, errors);
}

pub(crate) fn require_validated_bool_stmt_expr(
    expr: &Expr,
    context: &str,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_ty = validate_and_infer_stmt_expr_type(expr, env, errors);
    require_expr_bool_type(expr, expr_ty, context, errors);
}

pub(crate) fn require_validated_numeric_stmt_expr(
    expr: &Expr,
    context: &str,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_ty = validate_and_infer_stmt_expr_type(expr, env, errors);
    require_expr_numeric_type(expr, expr_ty, context, errors);
}

pub(crate) fn validate_for_loop_step_expr(
    step_expr: Option<&Expr>,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(step_expr) = step_expr {
        require_validated_numeric_stmt_expr(step_expr, "for loop step", env, errors);
        if matches!(step_expr, Expr::Int { value: 0, .. })
            || matches!(step_expr, Expr::Number { value: v, .. } if *v == 0.0)
        {
            errors.push(Diagnostic::semantic_span(
                "for loop step cannot be zero",
                step_expr.loc(),
            ));
        }
    }
}

pub(crate) fn infer_static_slice_len_hint(
    total_len: Option<usize>,
    start: Option<&Expr>,
    end: Option<&Expr>,
) -> usize {
    let Some(total_len) = total_len else {
        return 1;
    };
    let start = normalize_static_slice_bound(start, total_len, false);
    let end = normalize_static_slice_bound(end, total_len, true);
    end.saturating_sub(start).max(1)
}

fn normalize_static_slice_bound(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
) -> usize {
    let Some(expr) = expr else {
        return if default_to_len { total_len } else { 0 };
    };
    let raw =
        const_slice_bound_i64(expr).unwrap_or(if default_to_len { total_len as i64 } else { 0 });
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    adjusted.clamp(0, total_len as i64) as usize
}

fn const_slice_bound_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int { value: v, .. } => Some(*v),
        Expr::Number { value: v, .. } => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn require_loop_control_context(
    keyword: &str,
    loop_depth: usize,
    errors: &mut Vec<Diagnostic>,
) {
    if loop_depth == 0 {
        errors.push(Diagnostic::semantic_span(
            format!("{keyword} is only allowed inside for/while/loop bodies"),
            None::<SourceLoc>,
        ));
    }
}
