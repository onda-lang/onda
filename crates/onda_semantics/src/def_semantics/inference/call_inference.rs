use super::*;
use crate::{PROC_INDEX_BASE_ARG, PROC_INDEX_BUFFER_SELECT_SENTINEL, PROC_INDEX_EXPR_ARG};
use onda_frontend::SourceLoc;

pub(super) fn infer_stmt_calls(
    stmt: &Stmt,
    struct_instances: &HashMap<String, String>,
    struct_array_roots: &HashMap<String, String>,
    proc_array_roots: &HashMap<String, InferredProcArrayParam>,
    array_bindings: &mut HashMap<String, InferredArrayParam>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    with_stmt_diag_context(stmt, |_diag| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                infer_expr_calls(
                    index,
                    struct_instances,
                    struct_array_roots,
                    proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
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
            struct_array_roots,
            proc_array_roots,
            array_bindings,
            buffer_bindings,
            fn_signatures,
            kinds,
            errors,
        ),
        Stmt::Return { expr, .. } => infer_expr_calls(
            expr,
            struct_instances,
            struct_array_roots,
            proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                end,
                struct_instances,
                struct_array_roots,
                proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
    struct_array_roots: &HashMap<String, String>,
    proc_array_roots: &HashMap<String, InferredProcArrayParam>,
    array_bindings: &HashMap<String, InferredArrayParam>,
    buffer_bindings: &HashMap<String, Vec<InferredBufferParam>>,
    fn_signatures: &HashMap<String, FnSignature>,
    kinds: &mut HashMap<String, Vec<InferredFnParam>>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Number { .. }
        | Expr::Int { .. }
        | Expr::Bool { .. }
        | Expr::ArrayCtor { .. }
        | Expr::Var { .. } => {}
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                infer_expr_calls(
                    value,
                    struct_instances,
                    struct_array_roots,
                    proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                infer_expr_calls(
                    start,
                    struct_instances,
                    struct_array_roots,
                    proc_array_roots,
                    array_bindings,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
            if let Some(end) = end {
                infer_expr_calls(
                    end,
                    struct_instances,
                    struct_array_roots,
                    proc_array_roots,
                    array_bindings,
                    buffer_bindings,
                    fn_signatures,
                    kinds,
                    errors,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
            infer_expr_calls(
                lhs,
                struct_instances,
                struct_array_roots,
                proc_array_roots,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                struct_array_roots,
                proc_array_roots,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            infer_expr_calls(
                expr,
                struct_instances,
                struct_array_roots,
                proc_array_roots,
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
                struct_array_roots,
                proc_array_roots,
                array_bindings,
                buffer_bindings,
                fn_signatures,
                kinds,
                errors,
            );
            infer_expr_calls(
                rhs,
                struct_instances,
                struct_array_roots,
                proc_array_roots,
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
                    struct_array_roots,
                    proc_array_roots,
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
                let resolved = resolve_call_args_at(
                    args,
                    &sig.params,
                    &sig.defaults,
                    false,
                    false,
                    &format!("function '{name}' call"),
                    expr.loc(),
                    errors,
                );
                if let Some(param_kinds) = kinds.get_mut(name) {
                    for (idx, arg) in resolved.into_iter().enumerate() {
                        if let Some(arg) = arg {
                            if let Some(slot) = param_kinds.get_mut(idx) {
                                match arg {
                                    Expr::Var { name: v, .. } => {
                                        if let Some(struct_name) = struct_instances.get(v) {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else if let Some(proc_array_info) =
                                            proc_array_roots.get(v)
                                        {
                                            if !slot.saw_proc_arrays.iter().any(|seen| {
                                                seen.proc_name == proc_array_info.proc_name
                                                    && seen.len == proc_array_info.len
                                            }) {
                                                slot.saw_proc_arrays.push(proc_array_info.clone());
                                            }
                                        } else if let Some(struct_name) = struct_array_roots.get(v)
                                        {
                                            if !slot
                                                .saw_struct_arrays
                                                .iter()
                                                .any(|seen| seen.struct_name == *struct_name)
                                            {
                                                slot.saw_struct_arrays.push(
                                                    InferredStructArrayParam {
                                                        struct_name: struct_name.clone(),
                                                    },
                                                );
                                            }
                                        } else if let Some(array_info) = array_bindings.get(v) {
                                            if !slot.saw_arrays.iter().any(|seen| {
                                                seen.elem_ty == array_info.elem_ty
                                                    && seen.len == array_info.len
                                            }) {
                                                slot.saw_arrays.push(array_info.clone());
                                            }
                                        } else if let Some(buffer_infos) = buffer_bindings.get(v) {
                                            for buffer_info in buffer_infos {
                                                push_buffer_observation(
                                                    slot,
                                                    buffer_info.clone(),
                                                    true,
                                                );
                                            }
                                        } else if let Some(struct_name) = sig
                                            .param_types
                                            .get(idx)
                                            .and_then(|ty| ty.as_ref())
                                            .and_then(|ty| match ty {
                                                FnParamType::Struct(name) => Some(name),
                                                _ => None,
                                            })
                                        {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else {
                                            slot.saw_scalar = true;
                                        }
                                    }
                                    Expr::Index { base, .. } => {
                                        if let Some(struct_name) = struct_array_roots.get(base) {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else if let Some(struct_name) = sig
                                            .param_types
                                            .get(idx)
                                            .and_then(|ty| ty.as_ref())
                                            .and_then(|ty| match ty {
                                                FnParamType::Struct(name) => Some(name),
                                                _ => None,
                                            })
                                        {
                                            slot.saw_structs.insert(struct_name.clone());
                                        } else {
                                            slot.saw_scalar = true;
                                        }
                                    }
                                    Expr::UserCall {
                                        name: selector_name,
                                        args: selector_args,
                                        ..
                                    } if selector_name == PROC_INDEX_BUFFER_SELECT_SENTINEL => {
                                        let mut saw_any_buffer = false;
                                        let mut saw_invalid_slot = false;
                                        for selector_arg in selector_args {
                                            if selector_arg.name.as_deref()
                                                == Some(PROC_INDEX_BASE_ARG)
                                                || selector_arg.name.as_deref()
                                                    == Some(PROC_INDEX_EXPR_ARG)
                                            {
                                                continue;
                                            }
                                            let Expr::Var { name: v, .. } = &selector_arg.expr
                                            else {
                                                saw_invalid_slot = true;
                                                continue;
                                            };
                                            if let Some(buffer_infos) = buffer_bindings.get(v) {
                                                saw_any_buffer = true;
                                                for buffer_info in buffer_infos {
                                                    push_buffer_observation(
                                                        slot,
                                                        buffer_info.clone(),
                                                        true,
                                                    );
                                                }
                                            } else {
                                                saw_invalid_slot = true;
                                            }
                                        }
                                        if !saw_any_buffer || saw_invalid_slot {
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
                    struct_array_roots,
                    proc_array_roots,
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
        Expr::ArrayLiteral { values, .. } => {
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
            onda_frontend::ArrayElemType::Primitive(elem_ty) => Some(InferredArrayParam {
                elem_ty: *elem_ty,
                len: 1,
            }),
            onda_frontend::ArrayElemType::Struct(_) => None,
        },
        Expr::Slice { .. } => None,
        _ => None,
    }
}

fn infer_array_literal_elem_ty(expr: &Expr) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value: v, .. } => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Var { name, .. } => builtin_constant_type(name),
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
    resolve_call_args_at(
        args,
        param_names,
        param_defaults,
        forbid_self_named,
        named_only,
        context,
        SourceLoc::ZERO,
        errors,
    )
}

pub(crate) fn resolve_call_args_at<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_defaults: &[Option<Expr>],
    forbid_self_named: bool,
    named_only: bool,
    context: &str,
    loc: SourceLoc,
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
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: 'self' cannot be passed as a named argument"),
                    loc,
                ));
                continue;
            }
            if !seen_named.insert(name.clone()) {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: duplicate named argument '{name}'"),
                    loc,
                ));
                continue;
            }
            let Some(idx) = param_names.iter().position(|p| p == name) else {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: unknown named argument '{name}'"),
                    loc,
                ));
                continue;
            };
            if resolved[idx].is_some() {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: argument '{name}' provided multiple times"),
                    loc,
                ));
                continue;
            }
            resolved[idx] = Some(&arg.expr);
        } else {
            if named_only {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: positional arguments are not allowed; use named arguments"),
                    loc,
                ));
                continue;
            }
            if saw_named {
                errors.push(Diagnostic::semantic_span(
                    format!("{context}: positional arguments must come before named arguments"),
                    loc,
                ));
                continue;
            }
            while next_pos < resolved.len() && resolved[next_pos].is_some() {
                next_pos += 1;
            }
            if next_pos >= resolved.len() {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "{context}: too many positional arguments (expected at most {})",
                        param_names.len()
                    ),
                    loc,
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
            errors.push(Diagnostic::semantic_span(
                format!(
                    "{context}: missing required argument '{}'",
                    param_names[idx]
                ),
                loc,
            ));
        }
    }

    resolved
}
