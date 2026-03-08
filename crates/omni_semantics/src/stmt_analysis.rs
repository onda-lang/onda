use std::collections::{HashMap, HashSet};

use crate::*;

mod def_analysis;
mod indexed_binding;
mod init_analysis;
mod runtime_stmt_analysis;
pub(crate) use def_analysis::*;
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
