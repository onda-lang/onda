use std::collections::HashMap;

use onda_frontend::{CallArg, Expr};

use crate::{
    IndexAccess, PROC_FIELD_SENTINEL_ARG, PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG,
    PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG, PROC_INDEX_UNCHECKED_ARG,
};

#[derive(Clone)]
pub(crate) struct ProcArrayAliasInfo {
    pub(crate) array_base: String,
    pub(crate) index_expr: Expr,
    pub(crate) access: IndexAccess,
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
            Some(PROC_INDEX_BASE_ARG) | Some(PROC_INDEX_EXPR_ARG) | Some(PROC_INDEX_UNCHECKED_ARG)
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
    if alias.access == IndexAccess::Unchecked {
        rewritten.push(CallArg {
            name: Some(PROC_INDEX_UNCHECKED_ARG.to_owned()),
            expr: Expr::int(1),
        });
    }
    rewritten.extend(rest);
    *args = rewritten;
}

fn rewrite_proc_alias_calls_in_expr_impl(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    rewrite_var_fields: bool,
) {
    expr.visit_mut_postorder(|expr| match expr {
        Expr::Var { name, .. } => {
            if rewrite_var_fields {
                if let Some((base, field)) = split_dot_path(name.as_str()) {
                    if let Some(alias) = aliases.get(base) {
                        let mut args = vec![
                            CallArg {
                                name: Some(PROC_INDEX_BASE_ARG.to_owned()),
                                expr: Expr::var(alias.array_base.clone()),
                            },
                            CallArg {
                                name: Some(PROC_INDEX_EXPR_ARG.to_owned()),
                                expr: alias.index_expr.clone(),
                            },
                        ];
                        if alias.access == IndexAccess::Unchecked {
                            args.push(CallArg {
                                name: Some(PROC_INDEX_UNCHECKED_ARG.to_owned()),
                                expr: Expr::int(1),
                            });
                        }
                        args.push(CallArg {
                            name: Some(PROC_FIELD_SENTINEL_ARG.to_owned()),
                            expr: Expr::var(field.to_owned()),
                        });
                        *expr = Expr::UserCall {
                            loc: Default::default(),
                            name: format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}"),
                            type_args: Vec::new(),
                            args,
                        };
                    }
                }
            }
        }
        Expr::UserCall { name, args, .. } => {
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
        _ => {}
    });
}

pub(crate) fn rewrite_proc_alias_calls_in_expr(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) {
    rewrite_proc_alias_calls_in_expr_impl(expr, aliases, true);
}

pub(crate) fn rewrite_proc_alias_call_sites_in_expr(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) {
    rewrite_proc_alias_calls_in_expr_impl(expr, aliases, false);
}

pub(crate) fn rewrite_proc_alias_calls_for_validation(
    expr: &Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Expr {
    let mut rewritten = expr.clone();
    rewrite_proc_alias_calls_in_expr(&mut rewritten, aliases);
    rewritten
}
