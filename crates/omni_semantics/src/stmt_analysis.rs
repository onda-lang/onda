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
