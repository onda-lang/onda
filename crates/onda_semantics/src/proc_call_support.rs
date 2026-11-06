use std::collections::HashMap;

use onda_frontend::{AssignTarget, CallArg, Expr, Stmt};

use crate::proc_state_rewrite::{ProcArrayDeclarations, ProcCallInstance};
use crate::{
    IndexAccess, PROC_CACHED_FIELD_ARG, PROC_FIELD_SENTINEL_PREFIX, PROC_INDEX_BASE_ARG,
    PROC_INDEX_CALL_SENTINEL, PROC_INDEX_EXPR_ARG, PROC_INDEX_UNCHECKED_ARG,
};

#[derive(Clone)]
pub(crate) struct ProcArrayAliasInfo {
    pub(crate) array_base: String,
    pub(crate) index_expr: Expr,
    pub(crate) access: IndexAccess,
}

/// Forward the selected processor's validation metadata through ordinary aliases.
pub(crate) fn proc_array_alias_from_expr<T>(
    expr: &Expr,
    arrays: &HashMap<String, T>,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Option<ProcArrayAliasInfo> {
    if let Some(source) = crate::index_access::indexed_read_source(expr) {
        return arrays
            .contains_key(source.base)
            .then(|| ProcArrayAliasInfo {
                array_base: source.base.to_owned(),
                index_expr: source.index.clone(),
                access: source.access,
            });
    }
    match expr {
        Expr::Var { name, .. } => aliases.get(name).cloned(),
        _ => None,
    }
}

pub(crate) fn has_cached_proc_field_marker(arguments: &[CallArg]) -> bool {
    arguments.iter().any(|argument| {
        argument.name.as_deref() == Some(PROC_CACHED_FIELD_ARG)
            && matches!(argument.expr, Expr::Bool { value: true, .. })
    })
}

/// Alias expansion copies its selector into later calls. Freeze that selector
/// where the alias is created so subsequent mutations and calls cannot change
/// the selected processor or evaluate a side effect twice.
pub(crate) fn capture_proc_alias_selectors(
    statements: &mut Vec<Stmt>,
    proc_array_slots: &ProcArrayDeclarations,
) {
    const PREFIX: &str = "__onda_proc_alias_selector_";
    if proc_array_slots.is_empty() {
        return;
    }

    fn capture(statements: &mut Vec<Stmt>, arrays: &ProcArrayDeclarations, next: &mut usize) {
        let mut captured = Vec::with_capacity(statements.len());
        for mut statement in std::mem::take(statements) {
            match &mut statement {
                Stmt::Assign {
                    target: AssignTarget::Var(_),
                    expr,
                    ..
                } => {
                    if let Some(source) = crate::index_access::indexed_read_source(expr) {
                        let is_captured = matches!(source.index, Expr::Var { name, .. } if name.starts_with(PREFIX));
                        if !is_captured
                            && !matches!(source.index, Expr::Int { .. } | Expr::Number { .. })
                            && crate::proc_call_rewrite::resolve_proc_array_base_key(
                                source.base,
                                arrays,
                            )
                            .is_some()
                        {
                            let name = format!("{PREFIX}{next}");
                            *next += 1;
                            let base = source.base.to_owned();
                            let index = source.index.clone();
                            let access = source.access;
                            let loc = expr.loc();
                            captured.push(Stmt::Assign {
                                loc: loc.clone().into(),
                                target_loc: loc.clone().into(),
                                target: AssignTarget::Var(name.clone()),
                                decl_ty: None,
                                generic_decl_ty: None,
                                is_typed_decl: false,
                                typed_decl_ty_loc: Default::default(),
                                expr: index,
                            });
                            *expr = crate::index_access::indexed_read_expr(
                                base,
                                Expr::var(name),
                                access,
                                loc.into(),
                            );
                        }
                    }
                }
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    capture(then_branch, arrays, next);
                    capture(else_branch, arrays, next);
                }
                Stmt::For { body, .. } | Stmt::While { body, .. } => capture(body, arrays, next),
                _ => {}
            }
            captured.push(statement);
        }
        *statements = captured;
    }

    let mut next = 0;
    for statement in statements.iter() {
        statement.visit_statements(|statement| {
            if let Stmt::Assign {
                target: AssignTarget::Var(name),
                ..
            } = statement
            {
                if let Some(index) = name
                    .strip_prefix(PREFIX)
                    .and_then(|index| index.parse::<usize>().ok())
                {
                    next = next.max(index.saturating_add(1));
                }
            }
        });
    }
    capture(statements, proc_array_slots, &mut next);
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

pub(crate) fn rewrite_proc_alias_call_sites_in_expr(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    instances: &HashMap<String, ProcCallInstance>,
    arrays: &ProcArrayDeclarations,
) {
    rewrite_proc_alias_calls(expr, aliases, |receiver, field| {
        instances.contains_key(&format!("{receiver}.{field}"))
            || aliases
                .get(receiver)
                .and_then(|alias| arrays.get(&alias.array_base))
                .and_then(|array| array.first_binding())
                .is_some_and(|source| instances.contains_key(&format!("{source}.{field}")))
    });
}

fn rewrite_proc_alias_calls(
    expr: &mut Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
    is_projection: impl Fn(&str, &str) -> bool,
) {
    expr.visit_mut_postorder(|expr| {
        let Expr::UserCall { name, args, .. } = expr else {
            return;
        };
        if let Some(alias) = name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .and_then(|receiver| aliases.get(receiver))
        {
            *name = format!("{PROC_FIELD_SENTINEL_PREFIX}{PROC_INDEX_CALL_SENTINEL}");
            prepend_proc_index_alias_args(args, alias);
            return;
        }
        if let Some(alias) = aliases.get(name) {
            *name = PROC_INDEX_CALL_SENTINEL.to_owned();
            prepend_proc_index_alias_args(args, alias);
            return;
        }
        if let Some((base, field)) = split_dot_path(name.as_str()) {
            if !is_projection(base, field) {
                if let Some(alias) = aliases.get(base) {
                    *name = format!("{PROC_INDEX_CALL_SENTINEL}.{field}");
                    prepend_proc_index_alias_args(args, alias);
                }
            }
        }
    });
}

pub(crate) fn rewrite_proc_alias_calls_for_validation(
    expr: &Expr,
    aliases: &HashMap<String, ProcArrayAliasInfo>,
) -> Expr {
    let mut rewritten = expr.clone();
    // Nominal view fields already carry their declared types. Only adapt the
    // callable surface; a legacy indexed-field sentinel loses those facts.
    rewrite_proc_alias_calls(&mut rewritten, aliases, |_, _| false);
    rewritten
}
