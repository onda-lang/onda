use super::*;

pub(super) fn infer_stmt_calls(
    stmt: &Stmt,
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, || match stmt {
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                infer_expr_calls(
                    index,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            infer_expr_calls(
                expr,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Stmt::Expr { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::Return { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            infer_expr_calls(
                cond,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            for nested in then_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            for nested in else_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            infer_expr_calls(
                start,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                end,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            if let Some(step_expr) = step {
                infer_expr_calls(
                    step_expr,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            for nested in body {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            infer_expr_calls(
                cond,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            for nested in body {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

fn infer_expr_calls(
    expr: &Expr,
    struct_instances: &HashMap<String, String>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::DataCtor { .. } | Expr::Var(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                infer_expr_calls(
                    value,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Expr::Index { index, .. } => {
            infer_expr_calls(
                index,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_expr_calls(
                lhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
            infer_expr_calls(
                expr,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Logical { lhs, rhs, .. } => {
            infer_expr_calls(
                lhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                infer_expr_calls(
                    arg,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Expr::UserCall { name, args, .. } => {
            if let Some(sig) = fn_signatures.get(name) {
                let resolved = resolve_call_args(
                    args,
                    &sig.params,
                    &sig.defaults,
                    false,
                    false,
                    &format!("function '{name}' call"),
                    errors,
                );
                if let Some(param_kinds) = kinds.get_mut(name) {
                    for (idx, arg) in resolved.into_iter().enumerate() {
                        if let Some(arg) = arg {
                            if let Some(slot) = param_kinds.get_mut(idx) {
                                match arg {
                                    Expr::Var(v) => {
                                        if let Some(struct_name) = struct_instances.get(v) {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else if let Some(buffer_infos) = buffer_bindings.get(v) {
                                            for buffer_info in buffer_infos {
                                                if !slot.saw_buffers.iter().any(|seen| {
                                                    seen.elem_ty == buffer_info.elem_ty
                                                        && seen.channels == buffer_info.channels
                                                }) {
                                                    slot.saw_buffers.push(buffer_info.clone());
                                                }
                                            }
                                        } else {
                                            slot.saw_scalar = true;
                                        }
                                    }
                                    _ => slot.saw_scalar = true,
                                }
                            }
                        }
                    }
                }
            }
            for arg in args {
                infer_expr_calls(
                    &arg.expr,
                    struct_instances,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
    }
}

pub(crate) fn resolve_call_args<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_defaults: &[Option<Expr>],
    forbid_self_named: bool,
    named_only: bool,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Option<&'a Expr>> {
    let mut resolved: Vec<Option<&Expr>> = vec![None; param_names.len()];
    let mut next_pos = 0usize;
    let mut seen_named = HashSet::new();
    let mut saw_named = false;

    for arg in args {
        if let Some(name) = &arg.name {
            saw_named = true;
            if forbid_self_named && name == "self" {
                errors.push(Diagnostic::semantic(
                    format!("{context}: 'self' cannot be passed as a named argument"),
                    0,
                    0,
                ));
                continue;
            }
            if !seen_named.insert(name.clone()) {
                errors.push(Diagnostic::semantic(
                    format!("{context}: duplicate named argument '{name}'"),
                    0,
                    0,
                ));
                continue;
            }
            let Some(idx) = param_names.iter().position(|p| p == name) else {
                errors.push(Diagnostic::semantic(
                    format!("{context}: unknown named argument '{name}'"),
                    0,
                    0,
                ));
                continue;
            };
            if resolved[idx].is_some() {
                errors.push(Diagnostic::semantic(
                    format!("{context}: argument '{name}' provided multiple times"),
                    0,
                    0,
                ));
                continue;
            }
            resolved[idx] = Some(&arg.expr);
        } else {
            if named_only {
                errors.push(Diagnostic::semantic(
                    format!("{context}: positional arguments are not allowed; use named arguments"),
                    0,
                    0,
                ));
                continue;
            }
            if saw_named {
                errors.push(Diagnostic::semantic(
                    format!("{context}: positional arguments must come before named arguments"),
                    0,
                    0,
                ));
                continue;
            }
            while next_pos < resolved.len() && resolved[next_pos].is_some() {
                next_pos += 1;
            }
            if next_pos >= resolved.len() {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context}: too many positional arguments (expected at most {})",
                        param_names.len()
                    ),
                    0,
                    0,
                ));
                continue;
            }
            resolved[next_pos] = Some(&arg.expr);
            next_pos += 1;
        }
    }

    for idx in 0..resolved.len() {
        let has_default = matches!(param_defaults.get(idx), Some(Some(_)));
        if resolved[idx].is_none() && !has_default {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context}: missing required argument '{}'",
                    param_names[idx]
                ),
                0,
                0,
            ));
        }
    }

    resolved
}
