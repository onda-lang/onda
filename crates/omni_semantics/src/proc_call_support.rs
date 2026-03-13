use std::collections::HashMap;

use omni_frontend::{CallArg, Expr};

use crate::{
    PROC_FIELD_SENTINEL_ARG, PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG,
    PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG,
};

#[derive(Clone)]
pub(crate) struct ProcArrayAliasInfo {
    pub(crate) array_base: String,
    pub(crate) index_expr: Expr,
}

pub(crate) fn split_dot_path(name: &str) -> Option<(&str, &str)> {
    let (base, field) = name.split_once('.')?;
    if base.is_empty() || field.is_empty() {
        return None;
    }
    if field.contains('.') {
        return None;
    }
    Some((base, field))
}

fn prepend_proc_index_alias_args(args: &mut Vec<CallArg>, alias: &ProcArrayAliasInfo) {
    let mut rest = std::mem::take(args);
    rest.retain(|arg| {
        !matches!(
            arg.name.as_deref(),
            Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG)
        )
    });
    let mut rewritten = Vec::<CallArg>::with_capacity(rest.len() + 2);
    rewritten.push(CallArg {
        name: None,
        expr: Expr::var(alias.array_base.clone()),
    });
    rewritten.push(CallArg {
        name: None,
        expr: alias.index_expr.clone(),
    });
    rewritten.extend(rest);
    *args = rewritten;
}

pub(crate) fn rewrite_proc_alias_calls_in_expr(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) {
    match expr {
        Expr::Var { name, .. } => {
            if let Some((base, field)) = split_dot_path(name.as_str()) {
                if let Some(alias) = aliases.get(base) {
                    *expr = Expr::UserCall {
                        loc: Default::default(),
                        name: format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
                        type_args: Vec::new(),
                        args: vec![
                            CallArg {
                                name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                expr: Expr::var(alias.array_base.clone()),
                            },
                            CallArg {
                                name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                expr: alias.index_expr.clone(),
                            },
                            CallArg {
                                name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                                expr: Expr::var(field.to_owned()),
                            },
                        ],
                    };
                }
            }
        }
        Expr::Index { index, .. } => rewrite_proc_alias_calls_in_expr(index, aliases),
        Expr::Slice { start, end, .. } => {
            if let Some(start) = start {
                rewrite_proc_alias_calls_in_expr(start, aliases);
            }
            if let Some(end) = end {
                rewrite_proc_alias_calls_in_expr(end, aliases);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            rewrite_proc_alias_calls_in_expr(&mut spec.size, aliases);
            if let Some(values) = init {
                for value in values {
                    rewrite_proc_alias_calls_in_expr(value, aliases);
                }
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            rewrite_proc_alias_calls_in_expr(lhs, aliases);
            rewrite_proc_alias_calls_in_expr(rhs, aliases);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                rewrite_proc_alias_calls_in_expr(arg, aliases);
            }
        }
        Expr::UserCall { name, args, .. } => {
            for arg in args.iter_mut() {
                rewrite_proc_alias_calls_in_expr(&mut arg.expr, aliases);
            }
            if let Some(alias) = aliases.get(name) {
                *name = PROC_INDEX_CALL_SENTINEL.to_owned();
                prepend_proc_index_alias_args(args, alias);
                return;
            }
            if let Some((base, field)) = split_dot_path(name.as_str()) {
                if let Some(alias) = aliases.get(base) {
                    *name = format!("{PROC_INDEX_CALL_SENTINEL}.{field}");
                    prepend_proc_index_alias_args(args, alias);
                }
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            rewrite_proc_alias_calls_in_expr(inner, aliases);
        }
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                rewrite_proc_alias_calls_in_expr(value, aliases);
            }
        }
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
    }
}

pub(crate) fn rewrite_proc_alias_calls_for_validation(
    expr: &Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Expr {
    let mut rewritten = expr.clone();
    rewrite_proc_alias_calls_in_expr(&mut rewritten, aliases);
    rewritten
}
