use super::*;

pub(super) fn infer_stmt_calls(
    stmt: &Stmt,
    struct_instances: &HashMap<String, String>,
    array_bindings: &mut HashMap<String, InferredArrayParam>,
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
                    array_bindings,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            infer_expr_calls(
                expr,
                struct_instances,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            if let AssignTarget::Var(name) = target {
                if let Some(info) = infer_array_binding_from_assignment(expr) {
                    array_bindings.insert(name.clone(), info);
                }
            }
        }
        Stmt::Expr { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            array_bindings,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::Return { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            array_bindings,
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
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            let mut then_arrays = array_bindings.clone();
            for nested in then_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    &mut then_arrays,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            let mut else_arrays = array_bindings.clone();
            for nested in else_branch {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    &mut else_arrays,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            merge_array_bindings(array_bindings, &then_arrays);
            merge_array_bindings(array_bindings, &else_arrays);
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
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                end,
                struct_instances,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            if let Some(step_expr) = step {
                infer_expr_calls(
                    step_expr,
                    struct_instances,
                    array_bindings,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            let mut loop_arrays = array_bindings.clone();
            for nested in body {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    &mut loop_arrays,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            merge_array_bindings(array_bindings, &loop_arrays);
        }
        Stmt::While { cond, body, .. } => {
            infer_expr_calls(
                cond,
                struct_instances,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            let mut loop_arrays = array_bindings.clone();
            for nested in body {
                infer_stmt_calls(
                    nested,
                    struct_instances,
                    &mut loop_arrays,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            merge_array_bindings(array_bindings, &loop_arrays);
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}

fn infer_expr_calls(
    expr: &Expr,
    struct_instances: &HashMap<String, String>,
    array_bindings: &HashMap<String, InferredArrayParam>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::ArrayCtor { .. } | Expr::Var(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                infer_expr_calls(
                    value,
                    struct_instances,
                    array_bindings,
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
                array_bindings,
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
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                array_bindings,
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
                array_bindings,
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
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                array_bindings,
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
                    array_bindings,
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
                                        } else if let Some(array_info) = array_bindings.get(v) {
                                            if !slot.saw_arrays.iter().any(|seen| {
                                                seen.elem_ty == array_info.elem_ty
                                                    && seen.len == array_info.len
                                            }) {
                                                slot.saw_arrays.push(array_info.clone());
                                            }
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
                    array_bindings,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
    }
}

fn infer_array_binding_from_assignment(expr: &Expr) -> Option<InferredArrayParam> {
    match expr {
        Expr::ArrayLiteral(values) => {
            if values.is_empty() {
                return None;
            }
            let elem_ty = infer_array_literal_elem_ty(&values[0]).unwrap_or(PrimitiveType::F32);
            Some(InferredArrayParam {
                elem_ty,
                len: values.len(),
            })
        }
        Expr::ArrayCtor { spec, .. } => match &spec.elem {
            omni_frontend::ArrayElemType::Primitive(elem_ty) => Some(InferredArrayParam {
                elem_ty: *elem_ty,
                len: 1,
            }),
            omni_frontend::ArrayElemType::Struct(_) => None,
        },
        _ => None,
    }
}

fn infer_array_literal_elem_ty(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var(name) => builtin_constant_type(name),
        _ => None,
    }
}

fn merge_array_bindings(
    dst: &mut HashMap<String, InferredArrayParam>,
    src: &HashMap<String, InferredArrayParam>,
) {
    for (name, src_info) in src {
        dst.entry(name.clone())
            .and_modify(|dst_info| {
                if dst_info.elem_ty == PrimitiveType::F32 && src_info.elem_ty == PrimitiveType::F64
                {
                    dst_info.elem_ty = PrimitiveType::F64;
                }
                dst_info.len = dst_info.len.max(src_info.len);
            })
            .or_insert_with(|| src_info.clone());
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
