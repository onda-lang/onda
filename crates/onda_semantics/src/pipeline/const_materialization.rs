//! Substitute constants in code selected by ordinary compiler reachability.
//! Array payloads are requested by the interpreter only when a value is read.
use super::*;

pub(super) fn materialize_const_expr(
    expr: &mut Expr,
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(value) = const_interpreter::materialize_expression(expr, artifacts, options, errors)
    {
        *expr = value;
    }
}

pub(super) fn materialize_runtime_stmts(
    stmts: &mut [Stmt],
    artifacts: &SemanticConstArtifacts,
    options: AnalysisOptions,
    errors: &mut Vec<Diagnostic>,
    metadata_options: AnalysisOptions,
) {
    for stmt in stmts {
        stmt.visit_exprs_mut(|expr| {
            if let Some((value, _)) = const_interpreter::materialize_expression_with_facts(
                expr,
                artifacts,
                options,
                Some(metadata_options),
                errors,
            ) {
                *expr = value;
            }
        });
        stmt.visit_statements_mut(|stmt| {
            if let Stmt::For {
                step: Some(step),
                var_ty,
                ..
            } = stmt
            {
                crate::stmt_analysis::validate_constant_loop_step(step, *var_ty, options, errors);
            }
        });
    }
}
