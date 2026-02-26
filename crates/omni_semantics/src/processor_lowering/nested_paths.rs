use super::*;

pub(super) fn nested_field_name(var: &str, field: &str) -> String {
    format!("{var}__{field}")
}

pub(super) fn nested_init_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_init")
}

pub(super) fn nested_step_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_step")
}

pub(super) fn nested_event_fn_name(owner_proc: &str, nested_var: &str, event_name: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_event_{event_name}")
}

pub(super) fn nested_block_pre_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_block_pre")
}

pub(super) fn nested_block_post_fn_name(owner_proc: &str, nested_var: &str) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_block_post")
}

pub(super) fn nested_call_out_fn_name(
    owner_proc: &str,
    nested_var: &str,
    out_idx: usize,
) -> String {
    format!("{owner_proc}.__proc_nested_{nested_var}_call_out{out_idx}")
}

pub(super) fn rewrite_nested_field_paths_in_expr(
    expr: &mut Expr,
    nested_fields: &HashMap<String, HashSet<String>>,
) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(fields) = nested_fields.get(base) {
                    if fields.contains(field) {
                        *name = format!("self.{}", nested_field_name(base, field));
                    }
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(fields) = nested_fields.get(root) {
                    if fields.contains(field) {
                        *base = format!("self.{}", nested_field_name(root, field));
                    }
                }
            }
            rewrite_nested_field_paths_in_expr(index, nested_fields);
        }
        Expr::DataCtor { spec, init } => {
            rewrite_nested_field_paths_in_expr(&mut spec.size, nested_fields);
            if let Some(values) = init {
                for value in values {
                    rewrite_nested_field_paths_in_expr(value, nested_fields);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_nested_field_paths_in_expr(lhs, nested_fields);
            rewrite_nested_field_paths_in_expr(rhs, nested_fields);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_nested_field_paths_in_expr(arg, nested_fields);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                rewrite_nested_field_paths_in_expr(&mut arg.expr, nested_fields);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            rewrite_nested_field_paths_in_expr(inner, nested_fields)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                rewrite_nested_field_paths_in_expr(value, nested_fields);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(super) fn rewrite_nested_field_paths_in_stmt(
    stmt: &mut Stmt,
    nested_fields: &HashMap<String, HashSet<String>>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(fields) = nested_fields.get(base) {
                            if fields.contains(field) {
                                *name = format!("self.{}", nested_field_name(base, field));
                            }
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(fields) = nested_fields.get(root) {
                            if fields.contains(field) {
                                *base = format!("self.{}", nested_field_name(root, field));
                            }
                        }
                    }
                    rewrite_nested_field_paths_in_expr(index, nested_fields);
                }
            }
            rewrite_nested_field_paths_in_expr(expr, nested_fields);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            rewrite_nested_field_paths_in_expr(expr, nested_fields)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_nested_field_paths_in_expr(cond, nested_fields);
            for s in then_branch {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
            for s in else_branch {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            rewrite_nested_field_paths_in_expr(start, nested_fields);
            rewrite_nested_field_paths_in_expr(end, nested_fields);
            if let Some(step_expr) = step {
                rewrite_nested_field_paths_in_expr(step_expr, nested_fields);
            }
            for s in body {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
        }
        Stmt::While { cond, body, .. } => {
            rewrite_nested_field_paths_in_expr(cond, nested_fields);
            for s in body {
                rewrite_nested_field_paths_in_stmt(s, nested_fields);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn remap_nested_symbols_in_expr(expr: &mut Expr, remap: &HashMap<String, String>) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if let Some(mapped) = remap.get(base) {
                    *name = format!("{mapped}.{field}");
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if let Some(mapped) = remap.get(root) {
                    *base = format!("{mapped}.{field}");
                }
            }
            remap_nested_symbols_in_expr(index, remap);
        }
        Expr::DataCtor { spec, init } => {
            remap_nested_symbols_in_expr(&mut spec.size, remap);
            if let Some(values) = init {
                for value in values {
                    remap_nested_symbols_in_expr(value, remap);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            remap_nested_symbols_in_expr(lhs, remap);
            remap_nested_symbols_in_expr(rhs, remap);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                remap_nested_symbols_in_expr(arg, remap);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args {
                remap_nested_symbols_in_expr(&mut arg.expr, remap);
            }
            if let Some(mapped) = remap.get(name) {
                *name = mapped.clone();
            } else if let Some(raw) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                if let Some(mapped) = remap.get(raw) {
                    *name = format!("{PROC_FIELD_SENTINEL_PREFIX}{mapped}");
                }
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            remap_nested_symbols_in_expr(inner, remap)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                remap_nested_symbols_in_expr(value, remap);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(super) fn remap_nested_symbols_in_stmt(stmt: &mut Stmt, remap: &HashMap<String, String>) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if let Some(mapped) = remap.get(base) {
                            *name = format!("{mapped}.{field}");
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if let Some(mapped) = remap.get(root) {
                            *base = format!("{mapped}.{field}");
                        }
                    }
                    remap_nested_symbols_in_expr(index, remap);
                }
            }
            remap_nested_symbols_in_expr(expr, remap);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            remap_nested_symbols_in_expr(expr, remap)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            remap_nested_symbols_in_expr(cond, remap);
            for s in then_branch {
                remap_nested_symbols_in_stmt(s, remap);
            }
            for s in else_branch {
                remap_nested_symbols_in_stmt(s, remap);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            remap_nested_symbols_in_expr(start, remap);
            remap_nested_symbols_in_expr(end, remap);
            if let Some(step_expr) = step {
                remap_nested_symbols_in_expr(step_expr, remap);
            }
            for s in body {
                remap_nested_symbols_in_stmt(s, remap);
            }
        }
        Stmt::While { cond, body, .. } => {
            remap_nested_symbols_in_expr(cond, remap);
            for s in body {
                remap_nested_symbols_in_stmt(s, remap);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

pub(super) fn prefix_self_fields_in_expr(
    expr: &mut Expr,
    prefix: &str,
    nested_field_names: &HashSet<String>,
) {
    match expr {
        Expr::Var(name) => {
            if let Some((base, field)) = split_simple_field_path(name) {
                if base == "self" && nested_field_names.contains(field) {
                    *name = format!("self.{}", nested_field_name(prefix, field));
                }
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_simple_field_path(base) {
                if root == "self" && nested_field_names.contains(field) {
                    *base = format!("self.{}", nested_field_name(prefix, field));
                }
            }
            prefix_self_fields_in_expr(index, prefix, nested_field_names);
        }
        Expr::DataCtor { spec, init } => {
            prefix_self_fields_in_expr(&mut spec.size, prefix, nested_field_names);
            if let Some(values) = init {
                for value in values {
                    prefix_self_fields_in_expr(value, prefix, nested_field_names);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            prefix_self_fields_in_expr(lhs, prefix, nested_field_names);
            prefix_self_fields_in_expr(rhs, prefix, nested_field_names);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                prefix_self_fields_in_expr(arg, prefix, nested_field_names);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                prefix_self_fields_in_expr(&mut arg.expr, prefix, nested_field_names);
            }
        }
        Expr::Cast { expr: inner, .. } | Expr::UnaryNot { expr: inner } => {
            prefix_self_fields_in_expr(inner, prefix, nested_field_names)
        }
        Expr::ArrayLiteral(values) => {
            for value in values {
                prefix_self_fields_in_expr(value, prefix, nested_field_names);
            }
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
    }
}

pub(super) fn prefix_self_fields_in_stmt(
    stmt: &mut Stmt,
    prefix: &str,
    nested_field_names: &HashSet<String>,
) {
    match stmt {
        Stmt::Assign { target, expr, .. } => {
            match target {
                AssignTarget::Var(name) => {
                    if let Some((base, field)) = split_simple_field_path(name) {
                        if base == "self" && nested_field_names.contains(field) {
                            *name = format!("self.{}", nested_field_name(prefix, field));
                        }
                    }
                }
                AssignTarget::Index { base, index } => {
                    if let Some((root, field)) = split_simple_field_path(base) {
                        if root == "self" && nested_field_names.contains(field) {
                            *base = format!("self.{}", nested_field_name(prefix, field));
                        }
                    }
                    prefix_self_fields_in_expr(index, prefix, nested_field_names);
                }
            }
            prefix_self_fields_in_expr(expr, prefix, nested_field_names);
        }
        Stmt::Expr { expr, .. } | Stmt::Return { expr, .. } => {
            prefix_self_fields_in_expr(expr, prefix, nested_field_names)
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            prefix_self_fields_in_expr(cond, prefix, nested_field_names);
            for s in then_branch {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
            for s in else_branch {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
        }
        Stmt::For {
            start,
            end,
            step,
            body,
            ..
        } => {
            prefix_self_fields_in_expr(start, prefix, nested_field_names);
            prefix_self_fields_in_expr(end, prefix, nested_field_names);
            if let Some(step_expr) = step {
                prefix_self_fields_in_expr(step_expr, prefix, nested_field_names);
            }
            for s in body {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
        }
        Stmt::While { cond, body, .. } => {
            prefix_self_fields_in_expr(cond, prefix, nested_field_names);
            for s in body {
                prefix_self_fields_in_stmt(s, prefix, nested_field_names);
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}
