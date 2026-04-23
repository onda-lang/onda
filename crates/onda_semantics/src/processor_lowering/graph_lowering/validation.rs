use onda_frontend::SourceLoc;

use super::*;

pub(super) fn push_graph_error(
    errors: &mut Vec<Diagnostic>,
    loc: SourceLoc,
    message: impl Into<String>,
) {
    errors.push(Diagnostic::semantic_span(message, loc));
}

pub(super) fn eval_graph_nonnegative_int_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if can_eval_const_expr_exact_int(expr) {
        let value = eval_const_expr_i64_exact(expr, options, context, errors)?;
        if value < 0 {
            push_graph_error(
                errors,
                expr.loc(),
                format!("{context} must be greater than or equal to zero"),
            );
            return None;
        }
        let Ok(value) = usize::try_from(value) else {
            push_graph_error(
                errors,
                expr.loc(),
                format!("{context} exceeds supported range"),
            );
            return None;
        };
        return Some(value);
    }

    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        push_graph_error(errors, expr.loc(), format!("{context} must be finite"));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        push_graph_error(errors, expr.loc(), format!("{context} must be an integer"));
        return None;
    }
    if rounded < 0.0 {
        push_graph_error(
            errors,
            expr.loc(),
            format!("{context} must be greater than or equal to zero"),
        );
        return None;
    }
    Some(rounded as usize)
}

fn eval_graph_static_slice_bound(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let Some(expr) = expr else {
        return Some(if default_to_len { total_len } else { 0 });
    };
    if can_eval_const_expr_exact_int(expr) {
        let raw = eval_const_expr_i64_exact(expr, options, context, errors)?;
        let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
        return Some(adjusted.clamp(0, total_len as i64) as usize);
    }

    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        push_graph_error(errors, expr.loc(), format!("{context} must be finite"));
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        push_graph_error(errors, expr.loc(), format!("{context} must be an integer"));
        return None;
    }
    let raw = rounded as i64;
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    Some(adjusted.clamp(0, total_len as i64) as usize)
}

pub(super) fn eval_graph_static_slice_bounds(
    total_len: usize,
    start: Option<&Expr>,
    end: Option<&Expr>,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<(usize, usize)> {
    let start_idx = eval_graph_static_slice_bound(
        start,
        total_len,
        false,
        options,
        &format!("{context} slice start"),
        errors,
    )?;
    let end_idx = eval_graph_static_slice_bound(
        end,
        total_len,
        true,
        options,
        &format!("{context} slice end"),
        errors,
    )?;
    if end_idx <= start_idx {
        let loc = SourceLoc::spanning(
            start.and_then(|expr| expr.loc().cloned()),
            end.and_then(|expr| expr.loc().cloned()),
        );
        push_graph_error(
            errors,
            loc,
            format!("{context} slice must have positive length"),
        );
        return None;
    }
    Some((start_idx, end_idx))
}

pub(super) fn graph_block_source_error(
    detail: String,
    inferred_param_rate: bool,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let message = if inferred_param_rate {
        format!("{detail}; add @sample to this param edge if sample-rate modulation is intended")
    } else {
        detail
    };
    push_graph_error(errors, loc, message);
}
