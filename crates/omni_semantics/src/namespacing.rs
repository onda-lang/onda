use super::*;

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
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    if symbols.contains(name) {
        return Some(name.to_owned());
    }
    if let Some((ns, symbol)) = name.rsplit_once("::") {
        if !namespaces.contains(ns) {
            errors.push(Diagnostic::semantic(
                format!("{context}: unknown namespace '{ns}' in symbol '{name}'"),
                0,
                0,
            ));
        } else {
            errors.push(Diagnostic::semantic(
                format!("{context}: unknown symbol '{symbol}' in namespace '{ns}'"),
                0,
                0,
            ));
        }
    } else {
        errors.push(Diagnostic::semantic(
            format!("{context}: unknown symbol '{name}'"),
            0,
            0,
        ));
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

pub(super) fn qualify_struct_type_name(
    ty_name: &mut String,
    current_ns: &str,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) {
    if ty_name.contains("::") {
        if let Some(resolved) = resolve_qualified_symbol_name(
            ty_name,
            struct_symbols,
            struct_namespaces,
            context,
            errors,
        ) {
            *ty_name = resolved;
        }
        return;
    }
    if let Some(resolved) = resolve_unqualified_symbol_name(ty_name, current_ns, struct_symbols) {
        *ty_name = resolved;
    }
}

pub(super) fn qualify_expr_namespaced_symbols(
    expr: &mut Expr,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
) {
    match expr {
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                qualify_expr_namespaced_symbols(
                    &mut arg.expr,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
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
        Expr::ArrayCtor { spec, init } => {
            if let ArrayElemType::Struct(name) = &mut spec.elem {
                qualify_struct_type_name(
                    name,
                    current_ns,
                    struct_symbols,
                    struct_namespaces,
                    context,
                    errors,
                );
            }
            qualify_expr_namespaced_symbols(
                &mut spec.size,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            if let Some(values) = init {
                for value in values {
                    qualify_expr_namespaced_symbols(
                        value,
                        current_ns,
                        callable_symbols,
                        callable_namespaces,
                        struct_symbols,
                        struct_namespaces,
                        errors,
                        context,
                    );
                }
            }
        }
        Expr::Index { index, .. } => qualify_expr_namespaced_symbols(
            index,
            current_ns,
            callable_symbols,
            callable_namespaces,
            struct_symbols,
            struct_namespaces,
            errors,
            context,
        ),
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            qualify_expr_namespaced_symbols(
                lhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            qualify_expr_namespaced_symbols(
                rhs,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Expr::Call { args, .. } => {
            for arg in args {
                qualify_expr_namespaced_symbols(
                    arg,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Expr::Cast { expr: arg, .. }
        | Expr::UnaryNot { expr: arg }
        | Expr::UnaryBitNot { expr: arg } => {
            qualify_expr_namespaced_symbols(
                arg,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                qualify_expr_namespaced_symbols(
                    value,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
}

pub(super) fn qualify_stmt_namespaced_symbols(
    stmt: &mut Stmt,
    current_ns: &str,
    callable_symbols: &HashSet<String>,
    callable_namespaces: &HashSet<String>,
    struct_symbols: &HashSet<String>,
    struct_namespaces: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
    context: &str,
) {
    with_stmt_diag_context_mut(stmt, |stmt| match stmt {
        Stmt::Const { .. } => {}
        Stmt::Assign { target, expr, .. } => {
            if let AssignTarget::Index { index, .. } = target {
                qualify_expr_namespaced_symbols(
                    index,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
            qualify_expr_namespaced_symbols(
                expr,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => qualify_expr_namespaced_symbols(
            expr,
            current_ns,
            callable_symbols,
            callable_namespaces,
            struct_symbols,
            struct_namespaces,
            errors,
            context,
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
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            for nested in then_branch {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
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
                    struct_symbols,
                    struct_namespaces,
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
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            qualify_expr_namespaced_symbols(
                end,
                current_ns,
                callable_symbols,
                callable_namespaces,
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            if let Some(step_expr) = step {
                qualify_expr_namespaced_symbols(
                    step_expr,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
            for nested in body {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
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
                struct_symbols,
                struct_namespaces,
                errors,
                context,
            );
            for nested in body {
                qualify_stmt_namespaced_symbols(
                    nested,
                    current_ns,
                    callable_symbols,
                    callable_namespaces,
                    struct_symbols,
                    struct_namespaces,
                    errors,
                    context,
                );
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    });
}
