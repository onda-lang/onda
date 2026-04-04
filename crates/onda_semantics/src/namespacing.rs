use super::*;
use onda_frontend::Span;

fn push_semantic(diag: DiagCtx, errors: &mut Vec<Diagnostic>, message: impl Into<String>) {
    errors.push(diag.semantic(message, 0, 0));
}

pub(super) fn parent_namespace(ns: &str) -> Option<&str> {
    ns.rsplit_once("::").map(|(parent, _)| parent)
}

pub(super) fn namespace_of_symbol(name: &str) -> String {
    let base = name.split('.').next().unwrap_or(name);
    base.rsplit_once("::")
        .map(|(ns, _)| ns.to_owned())
        .unwrap_or_default()
}

pub(super) fn namespace_candidates(current_ns: &str) -> Vec<String> {
    if current_ns.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut cur = Some(current_ns);
    while let Some(ns) = cur {
        out.push(ns.to_owned());
        cur = parent_namespace(ns);
    }
    out.push(String::new());
    out
}

pub(super) fn join_namespace(ns: &str, leaf: &str) -> String {
    if ns.is_empty() {
        leaf.to_owned()
    } else {
        format!("{ns}::{leaf}")
    }
}

pub(super) fn collect_declared_namespaces(symbols: &HashSet<String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for symbol in symbols {
        let mut ns = namespace_of_symbol(symbol);
        while !ns.is_empty() {
            out.insert(ns.clone());
            ns = parent_namespace(&ns).unwrap_or_default().to_owned();
        }
    }
    out
}

pub(super) fn resolve_qualified_symbol_name(
    name: &str,
    symbols: &HashSet<String>,
    namespaces: &HashSet<String>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if symbols.contains(name) {
        return Some(name.to_owned());
    }
    if let Some((ns, symbol)) = name.rsplit_once("::") {
        if !namespaces.contains(ns) {
            push_semantic(
                diag,
                errors,
                format!("{context}: unknown namespace '{ns}' in symbol '{name}'"),
            );
        } else {
            push_semantic(
                diag,
                errors,
                format!("{context}: unknown symbol '{symbol}' in namespace '{ns}'"),
            );
        }
    } else {
        push_semantic(diag, errors, format!("{context}: unknown symbol '{name}'"));
    }
    None
}

pub(super) fn resolve_unqualified_symbol_name(
    name: &str,
    current_ns: &str,
    symbols: &HashSet<String>,
) -> Option<String> {
    for ns in namespace_candidates(current_ns) {
        let candidate = join_namespace(&ns, name);
        if symbols.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn qualify_named_type_name(
    ty_name: &mut String,
    current_ns: &str,
    symbols: &HashSet<String>,
    namespaces: &HashSet<String>,
    context: &str,
    diag: DiagCtx,
    strict_qualified: bool,
    errors: &mut Vec<Diagnostic>,
) {
    let (base_name, suffix) = if let Some(idx) = ty_name.find('<') {
        (&ty_name[..idx], &ty_name[idx..])
    } else {
        (ty_name.as_str(), "")
    };
    if base_name.contains("::") {
        if symbols.contains(base_name) {
            let resolved = base_name.to_owned();
            *ty_name = format!("{resolved}{suffix}");
        } else if strict_qualified {
            if let Some(resolved) =
                resolve_qualified_symbol_name(base_name, symbols, namespaces, context, diag, errors)
            {
                *ty_name = format!("{resolved}{suffix}");
            }
        }
        return;
    }
    if let Some(resolved) = resolve_unqualified_symbol_name(base_name, current_ns, symbols) {
        *ty_name = format!("{resolved}{suffix}");
    }
}

pub(super) fn qualify_struct_type_name(
    ty_name: &mut String,
    current_ns: &str,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    context: &str,
    diag: DiagCtx,
    errors: &mut Vec<Diagnostic>,
) {
    qualify_named_type_name(
        ty_name,
        current_ns,
        struct_symbols,
        struct_namespaces,
        context,
        diag,
        true,
        errors,
    );
}

pub(super) fn qualify_expr_namespaced_symbols(
    expr: &mut Expr,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    nominal_symbols: &HashSet<String>,
    nominal_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
    array_elem_diag: Option<DiagCtx>,
) {
    match expr {
        Expr::UserCall {
            name, args, loc, ..
        } => {
            let diag = DiagCtx::new(*loc);
            for arg in args {
                qualify_expr_namespaced_symbols(
                    &mut arg.expr,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
            if is_builtin_function_name(name)
                || is_internal_buffer_2d_fn(name)
                || name.contains('.')
            {
                return;
            }
            if name.contains("::") {
                if let Some(resolved) = resolve_qualified_symbol_name(
                    name,
                    callable_symbols,
                    callable_namespaces,
                    context,
                    diag,
                    errors,
                ) {
                    *name = resolved;
                }
                return;
            }
            if let Some(resolved) =
                resolve_unqualified_symbol_name(name, current_ns, callable_symbols)
            {
                *name = resolved;
            }
        }
        Expr::ArrayCtor { loc, spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                qualify_named_type_name(
                    name,
                    current_ns,
                    nominal_symbols,
                    nominal_namespaces,
                    context,
                    array_elem_diag.unwrap_or_else(|| DiagCtx::new(*loc)),
                    false,
                    errors,
                );
            }
            qualify_expr_namespaced_symbols(
                &mut spec.size,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            if let Some(values) = init {
                for value in values {
                    qualify_expr_namespaced_symbols(
                        value,
                        current_ns,
                        callable_symbols,
                        callable_namespaces,
                        nominal_symbols,
                        nominal_namespaces,
                        errors,
                        context,
                        None,
                    );
                }
            }
        }
        Expr::Index { index, .. } => qualify_expr_namespaced_symbols(
            index,
            current_ns,
            callable_symbols,
            callable_namespaces,
            nominal_symbols,
            nominal_namespaces,
            errors,
            context,
            None,
        ),
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                qualify_expr_namespaced_symbols(
                    start,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
            if let Some(end) = end {
                qualify_expr_namespaced_symbols(
                    end,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            qualify_expr_namespaced_symbols(
                lhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            qualify_expr_namespaced_symbols(
                rhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                qualify_expr_namespaced_symbols(
                    arg,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
        }
        Expr::Cast { expr: arg, .. }
        | Expr::UnaryNot { expr: arg, .. }
        | Expr::UnaryBitNot { expr: arg, .. } => {
            qualify_expr_namespaced_symbols(
                arg,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                qualify_expr_namespaced_symbols(
                    value,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                qualify_expr_namespaced_symbols(
                    value,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } | Expr::Var { .. } => {}
    }
}

pub(super) fn qualify_stmt_namespaced_symbols(
    stmt: &mut Stmt,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    nominal_symbols: &HashSet<String>,
    nominal_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
) {
    with_stmt_diag_context_mut(stmt, |_diag, stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign {
            target,
            expr,
            typed_decl_ty_loc,
            ..
        } => {
            match target {
                AssignTarget::Index { index, .. } => {
                    qualify_expr_namespaced_symbols(
                        index,
                        current_ns,
                        callable_symbols,
                        callable_namespaces,
                        nominal_symbols,
                        nominal_namespaces,
                        errors,
                        context,
                        None,
                    );
                }
                AssignTarget::Slice { start, end, .. } => {
                    if let Some(start) = start {
                        qualify_expr_namespaced_symbols(
                            start,
                            current_ns,
                            callable_symbols,
                            callable_namespaces,
                            nominal_symbols,
                            nominal_namespaces,
                            errors,
                            context,
                            None,
                        );
                    }
                    if let Some(end) = end {
                        qualify_expr_namespaced_symbols(
                            end,
                            current_ns,
                            callable_symbols,
                            callable_namespaces,
                            nominal_symbols,
                            nominal_namespaces,
                            errors,
                            context,
                            None,
                        );
                    }
                }
                AssignTarget::Var(_) | AssignTarget::Tuple(_) => {}
            }
            let array_elem_diag = if *typed_decl_ty_loc != Span::ZERO {
                Some(DiagCtx::new(*typed_decl_ty_loc))
            } else {
                None
            };
            qualify_expr_namespaced_symbols(
                expr,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                array_elem_diag,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => qualify_expr_namespaced_symbols(
            expr,
            current_ns,
            callable_symbols,
            callable_namespaces,
            nominal_symbols,
            nominal_namespaces,
            errors,
            context,
            None,
        ),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            qualify_expr_namespaced_symbols(
                cond,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            for nested in then_branch {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                );
            }
            for nested in else_branch {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
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
            qualify_expr_namespaced_symbols(
                start,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            qualify_expr_namespaced_symbols(
                end,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            if let Some(step_expr) = step {
                qualify_expr_namespaced_symbols(
                    step_expr,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                    None,
                );
            }
            for nested in body {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                );
            }
        }
        Stmt::While { cond, body, .. } => {
            qualify_expr_namespaced_symbols(
                cond,
                current_ns,
                callable_symbols,
                callable_namespaces,
                nominal_symbols,
                nominal_namespaces,
                errors,
                context,
                None,
            );
            for nested in body {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    nominal_symbols,
                    nominal_namespaces,
                    errors,
                    context,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}
