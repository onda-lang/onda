//! Counted initialization for a constructor broadcast without per-slot values.

use super::*;

pub(super) fn select_constructor(
    statements: &mut [Stmt],
    owner: &str,
    family_source: &str,
    index: usize,
    len: usize,
) {
    let Some(Stmt::Expr {
        expr: Expr::UserCall { name, args, .. },
        ..
    }) = statements.last_mut()
    else {
        return;
    };
    *name = nested_init_fn_name(owner, family_source);
    proc_storage::select_family_coordinate(
        args,
        &Expr::int(index as i64),
        len,
        IndexAccess::Unchecked,
    );
}

pub(super) fn top_level_setup(array: &str, len: usize, mut body: Vec<Stmt>) -> Stmt {
    let variable = "__onda_proc_constructor_index";
    for statement in &mut body {
        match statement {
            Stmt::Assign {
                target: AssignTarget::Index { base, index },
                ..
            } if base
                .strip_prefix(array)
                .is_some_and(|suffix| suffix.starts_with('.')) =>
            {
                *index = Expr::var(variable);
            }
            Stmt::Expr {
                expr: Expr::UserCall { args, .. },
                ..
            } => {
                if let Some(CallArg {
                    expr: Expr::Index { base, index, .. },
                    ..
                }) = args.first_mut()
                {
                    if base == array {
                        **index = Expr::var(variable);
                    }
                }
            }
            _ => {}
        }
    }
    Stmt::For {
        loc: Default::default(),
        var: variable.to_owned(),
        var_ty: PrimitiveType::I32,
        step: None,
        start: Expr::int(0),
        end: Expr::int(len as i64),
        end_inclusive: false,
        body,
    }
}

pub(super) fn is_uniform(init: &Option<Vec<Expr>>, arrays: &HashSet<String>) -> bool {
    let Some(values) = init else { return true };
    let [Expr::UserCall { args, .. }] = values.as_slice() else {
        return false;
    };
    !args.iter().any(|arg| {
        arg.expr.walk().any(|node| match node {
            Expr::ArrayLiteral { .. } => true,
            Expr::Var { name, .. } => arrays.contains(name),
            _ => false,
        })
    })
}

pub(super) fn broadcast_setup(
    owner: &str,
    array_base: &str,
    slot: &str,
    len: usize,
    mut body: Vec<Stmt>,
) -> Stmt {
    let var = "__onda_proc_constructor_index";
    let alias = "__onda_proc_constructor_view";
    for statement in &mut body {
        match statement {
            Stmt::Assign {
                target: AssignTarget::Var(target),
                ..
            } => {
                let relative = target.strip_prefix("self.").unwrap_or(target);
                if let Some(field) = relative.strip_prefix(&format!("{slot}__")) {
                    *target = format!("{alias}.{field}");
                }
            }
            Stmt::Expr {
                expr: Expr::UserCall { name, args, .. },
                ..
            } if *name == nested_init_fn_name(owner, slot) => {
                proc_storage::select_family_coordinate(
                    args,
                    &Expr::var(var),
                    len,
                    IndexAccess::Unchecked,
                );
            }
            _ => {}
        }
    }
    body.insert(
        0,
        Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Var(alias.to_owned()),
            decl_ty: None,
            generic_decl_ty: None,
            is_typed_decl: false,
            typed_decl_ty_loc: Default::default(),
            expr: Expr::Index {
                loc: Default::default(),
                base: format!("self.{array_base}"),
                index: Box::new(Expr::var(var)),
            },
        },
    );
    Stmt::For {
        loc: Default::default(),
        var: var.to_owned(),
        var_ty: PrimitiveType::I32,
        step: None,
        start: Expr::int(0),
        end: Expr::int(len as i64),
        end_inclusive: false,
        body,
    }
}
