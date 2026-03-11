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

pub(crate) fn fork_scope_flow_state(
    known_scalars: &HashSet<String>,
    local_aliases: &LocalAliasTypes,
    local_array_aliases: &HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> ScopeFlowState {
    ScopeFlowState::from_parts(
        known_scalars.clone(),
        local_aliases.clone(),
        local_array_aliases.clone(),
        local_proc_aliases.clone(),
    )
}

pub(crate) fn merge_branch_scope_flow_state(
    known_scalars: &mut HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    then_state: ScopeFlowState,
    else_state: ScopeFlowState,
) {
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
    *local_array_aliases = then_state.local_array_aliases;
    for (k, v) in else_state.local_array_aliases {
        local_array_aliases.entry(k).or_insert(v);
    }
    *local_proc_aliases = then_state.local_proc_aliases;
    for (k, v) in else_state.local_proc_aliases {
        local_proc_aliases.entry(k).or_insert(v);
    }
}

pub(crate) fn adopt_loop_scope_flow_state(
    known_scalars: &HashSet<String>,
    local_aliases: &mut LocalAliasTypes,
    local_array_aliases: &mut HashMap<String, LocalArrayAliasInfo>,
    local_proc_aliases: &mut HashMap<String, ProcArrayAliasInfo>,
    loop_state: ScopeFlowState,
) {
    *local_aliases = loop_state.local_aliases;
    local_aliases.retain(|name, _| known_scalars.contains(name));
    *local_array_aliases = loop_state.local_array_aliases;
    *local_proc_aliases = loop_state.local_proc_aliases;
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
    build_stmt_expr_env(
        build_scope_expr_env(inputs, known_scalars, array_vars, scope),
        inputs.state_scalars,
        inputs.declared_symbols,
        local_aliases,
        local_array_aliases,
        inputs.input_names,
        inputs.output_names,
        inputs.param_names,
    )
}

pub(crate) fn validate_and_infer_stmt_expr_type(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    validate_expr(expr, env.expr_env, errors);
    infer_expr_type_for_semantics_with_local_data(
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
    let Expr::Var(name) = expr else {
        return false;
    };
    env.expr_env.array_vars.contains_key(name)
        || is_declared_data_array_symbol(env.declared_symbols, name)
        || has_declared_buffer_symbol_info(env.declared_symbols, name)
}

pub(crate) fn is_data_like_value_expr(expr: &Expr, env: StmtExprAnalysisEnv<'_>) -> bool {
    matches!(expr, Expr::Slice { .. }) || is_bare_array_ref_expr(expr, env)
}

pub(crate) fn validate_data_like_value_expr(
    expr: &Expr,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
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
    require_bool_type(expr_ty, context, errors);
}

pub(crate) fn require_validated_numeric_stmt_expr(
    expr: &Expr,
    context: &str,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expr_ty = validate_and_infer_stmt_expr_type(expr, env, errors);
    require_numeric_type(expr_ty, context, errors);
}

pub(crate) fn validate_for_loop_step_expr(
    step_expr: Option<&Expr>,
    env: StmtExprAnalysisEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(step_expr) = step_expr {
        require_validated_numeric_stmt_expr(step_expr, "for loop step", env, errors);
        if matches!(step_expr, Expr::Int(0)) || matches!(step_expr, Expr::Number(v) if *v == 0.0) {
            errors.push(Diagnostic::semantic("for loop step cannot be zero", 0, 0));
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
        Expr::Int(v) => Some(*v),
        Expr::Number(v) => {
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
        errors.push(Diagnostic::semantic(
            format!("{keyword} is only allowed inside for/while/loop bodies"),
            0,
            0,
        ));
    }
}
