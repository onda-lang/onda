use super::*;

pub(crate) fn rewrite_binding_expr(expr: &mut Expr, names: &HashMap<String, String>) {
    match expr {
        Expr::Var { name, .. } => {
            rewrite_binding_path(name, names);
        }
        Expr::Index { base, index, .. } => {
            rewrite_binding_path(base, names);
            rewrite_binding_expr(index, names);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            rewrite_binding_path(base, names);
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                rewrite_binding_expr(coordinate, names);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                rewrite_binding_expr(value, names);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_binding_expr(&mut spec.size, names);
            if let Some(values) = init {
                for value in values {
                    rewrite_binding_expr(value, names);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_binding_expr(lhs, names);
            rewrite_binding_expr(rhs, names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_binding_expr(arg, names);
            }
        }
        Expr::UserCall { name, args, .. } => {
            rewrite_binding_callable(name, names);
            for arg in args {
                rewrite_binding_expr(&mut arg.expr, names);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            rewrite_binding_expr(expr, names);
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

pub(crate) fn rewrite_binding_path(name: &mut String, names: &HashMap<String, String>) {
    let mut prefix = name.as_str();
    loop {
        if let Some(replacement) = names.get(prefix) {
            *name = format!("{replacement}{}", &name[prefix.len()..]);
            return;
        }
        let Some((parent, _)) = prefix.rsplit_once('.') else {
            return;
        };
        prefix = parent;
    }
}

pub(crate) fn rewrite_binding_callable(name: &mut String, names: &HashMap<String, String>) {
    rewrite_binding_path(name, names);
}

pub(crate) fn rewrite_binding_target(target: &mut AssignTarget, names: &HashMap<String, String>) {
    match target {
        AssignTarget::Var(name) => {
            rewrite_binding_path(name, names);
        }
        AssignTarget::Index { base, .. } | AssignTarget::IndexedMember { base, .. } => {
            rewrite_binding_path(base, names);
        }
        AssignTarget::Slice { base, .. } => {
            rewrite_binding_path(base, names);
        }
        AssignTarget::Tuple(values) => {
            for name in values.iter_mut().filter_map(|target| target.binding_mut()) {
                if let Some(replacement) = names.get(name) {
                    *name = replacement.clone();
                }
            }
        }
    }
    target.visit_selectors_mut(|selector| rewrite_binding_expr(selector, names));
}

pub(crate) fn rewrite_binding_stmts(
    stmts: &mut [Stmt],
    names: &HashMap<String, String>,
    task_locals: &HashSet<String>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Const { decl, .. } => rewrite_binding_expr(&mut decl.expr, names),
            Stmt::Assign {
                target,
                decl_ty,
                generic_decl_ty,
                is_typed_decl,
                expr,
                ..
            } => {
                let declared_task_local =
                    matches!(target, AssignTarget::Var(name) if task_locals.contains(name));
                rewrite_binding_target(target, names);
                rewrite_binding_expr(expr, names);
                if declared_task_local {
                    *decl_ty = None;
                    *generic_decl_ty = None;
                    *is_typed_decl = false;
                }
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
                rewrite_binding_expr(expr, names)
            }
            Stmt::Print { values, .. } => {
                for value in values {
                    rewrite_binding_expr(value, names);
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                rewrite_binding_expr(cond, names);
                rewrite_binding_stmts(then_branch, names, task_locals);
                rewrite_binding_stmts(else_branch, names, task_locals);
            }
            Stmt::For {
                var,
                start,
                end,
                step,
                body,
                ..
            } => {
                if let Some(replacement) = names.get(var) {
                    *var = replacement.clone();
                }
                rewrite_binding_expr(start, names);
                rewrite_binding_expr(end, names);
                if let Some(step) = step {
                    rewrite_binding_expr(step, names);
                }
                rewrite_binding_stmts(body, names, task_locals);
            }
            Stmt::While { cond, body, .. } => {
                rewrite_binding_expr(cond, names);
                rewrite_binding_stmts(body, names, task_locals);
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
}

pub(crate) fn collect_expr_uses(expr: &Expr, uses: &mut HashSet<String>) {
    match expr {
        Expr::Var { name, .. } => {
            uses.insert(name.clone());
        }
        Expr::Index { base, index, .. } => {
            uses.insert(base.clone());
            collect_expr_uses(index, uses);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            uses.insert(base.clone());
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                collect_expr_uses(coordinate, uses);
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                collect_expr_uses(value, uses);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            collect_expr_uses(&spec.size, uses);
            if let Some(values) = init {
                for value in values {
                    collect_expr_uses(value, uses);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            collect_expr_uses(lhs, uses);
            collect_expr_uses(rhs, uses);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Expr::UserCall { name, args, .. } => {
            collect_callable_receiver_use(name, uses);
            for arg in args {
                collect_expr_uses(&arg.expr, uses);
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            collect_expr_uses(expr, uses)
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

pub(crate) fn collect_callable_receiver_use(name: &str, uses: &mut HashSet<String>) {
    let Some((receiver, _)) = name.rsplit_once('.') else {
        return;
    };
    uses.insert(receiver.to_owned());
    if let Some(root) = receiver.split('.').next() {
        uses.insert(root.to_owned());
    }
}

/// Names read or written by executable statements, including projected stores.
/// This is conservative storage liveness: replacing an aggregate uses its place.
pub(crate) fn collect_stmt_uses(stmts: &[Stmt], uses: &mut HashSet<String>) {
    let mut pending = stmts.iter().collect::<Vec<_>>();
    while let Some(stmt) = pending.pop() {
        match stmt {
            Stmt::Assign { target, expr, .. } => {
                collect_expr_uses(expr, uses);
                match target {
                    AssignTarget::Var(name) => {
                        uses.insert(name.clone());
                    }
                    AssignTarget::Index { base, .. } | AssignTarget::IndexedMember { base, .. } => {
                        uses.insert(base.clone());
                    }
                    AssignTarget::Slice { base, .. } => {
                        uses.insert(base.clone());
                    }
                    AssignTarget::Tuple(names) => uses.extend(
                        names
                            .iter()
                            .filter_map(|name| name.binding())
                            .map(str::to_owned),
                    ),
                }
                target.visit_selectors(|selector| collect_expr_uses(selector, uses));
            }
            Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => collect_expr_uses(expr, uses),
            Stmt::Const { decl, .. } => collect_expr_uses(&decl.expr, uses),
            Stmt::Print { values, .. } => {
                for value in values {
                    collect_expr_uses(value, uses);
                }
            }
            Stmt::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                collect_expr_uses(cond, uses);
                pending.extend(then_branch);
                pending.extend(else_branch);
            }
            Stmt::For {
                start,
                end,
                step,
                body,
                ..
            } => {
                collect_expr_uses(start, uses);
                collect_expr_uses(end, uses);
                if let Some(step) = step {
                    collect_expr_uses(step, uses);
                }
                pending.extend(body);
            }
            Stmt::While { cond, body, .. } => {
                collect_expr_uses(cond, uses);
                pending.extend(body);
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
    uses.extend(
        uses.iter()
            .filter_map(|name| name.split_once('.').map(|(root, _)| root.to_owned()))
            .collect::<Vec<_>>(),
    );
}
