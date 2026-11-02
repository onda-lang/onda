//! Counted lifecycle work for columnar processor arrays.

use super::*;
use crate::proc_call_rewrite::dynamic_proc_array_buffer_call_args;

pub(super) fn begin_active_slot(active_field: &str, index: Expr, call: Expr) -> Stmt {
    Stmt::If {
        loc: Default::default(),
        cond: Expr::UnaryNot {
            loc: Default::default(),
            expr: Box::new(Expr::Index {
                loc: Default::default(),
                base: format!("self.{active_field}"),
                index: Box::new(index.clone()),
            }),
        },
        then_branch: vec![
            Stmt::Expr {
                loc: Default::default(),
                expr: call,
            },
            Stmt::Assign {
                loc: Default::default(),
                target_loc: Default::default(),
                target: AssignTarget::Index {
                    base: format!("self.{active_field}"),
                    index,
                },
                decl_ty: None,
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: Expr::bool(true),
            },
        ],
        else_branch: Vec::new(),
    }
}

fn slot_loop(var: String, len: usize, body: Vec<Stmt>) -> Stmt {
    Stmt::For {
        loc: Default::default(),
        var,
        var_ty: PrimitiveType::I32,
        step: None,
        start: Expr::int(0),
        end: Expr::int(len as i64),
        end_inclusive: false,
        body,
    }
}

pub(super) fn reset_active_slots(active_field: &str, len: usize) -> Stmt {
    let var = "__onda_proc_reset_slot".to_owned();
    slot_loop(
        var.clone(),
        len,
        vec![Stmt::Assign {
            loc: Default::default(),
            target_loc: Default::default(),
            target: AssignTarget::Index {
                base: format!("self.{active_field}"),
                index: Expr::var(var),
            },
            decl_ty: None,
            generic_decl_ty: None,
            is_typed_decl: false,
            typed_decl_ty_loc: Default::default(),
            expr: Expr::bool(false),
        }],
    )
}

pub(super) fn finish_active_slots(
    owner: &str,
    array_base: &str,
    active_field: &str,
    slots: &ProcArrayDeclaration,
    raw_slots: &ProcArrayDeclaration,
    instances: &HashMap<String, ProcCallInstance>,
    proc_api: &HashMap<String, ProcApi>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Stmt> {
    let slot_instances = slots
        .binding_names()
        .zip(raw_slots.binding_names())
        .map(|(slot, raw)| {
            instances
                .get(raw)
                .cloned()
                .map(|instance| (slot.clone(), instance))
        })
        .collect::<Option<Vec<_>>>()?;
    let (first_slot, first) = slot_instances.first()?;
    let api = proc_api.get(&first.proc_name)?;
    let var = "__onda_proc_finish_slot".to_owned();
    let index = Expr::var(var.clone());
    let array_base = if array_base.starts_with("self.") {
        array_base.to_owned()
    } else {
        format!("self.{array_base}")
    };
    let mut args = vec![CallArg {
        name: None,
        expr: Expr::Index {
            loc: Default::default(),
            base: array_base.clone(),
            index: Box::new(index.clone()),
        },
    }];
    args.extend(dynamic_proc_array_buffer_call_args(
        &slot_instances,
        api,
        &array_base,
        &index,
        errors,
    ));
    let mut name = format!("{}{}", first.proc_name, PROC_BLOCK_POST_FN_SUFFIX);
    proc_storage::route_dynamic_family_call(
        &mut name,
        &mut args,
        nested_block_post_fn_name(owner, first_slot),
        &slot_instances,
        slots.len(),
        &index,
        IndexAccess::Unchecked,
    );
    let call = Stmt::Expr {
        loc: Default::default(),
        expr: Expr::UserCall {
            loc: Default::default(),
            name,
            type_args: Vec::new(),
            args,
        },
    };
    Some(slot_loop(
        var,
        slots.len(),
        vec![Stmt::If {
            loc: Default::default(),
            cond: Expr::Index {
                loc: Default::default(),
                base: format!("self.{active_field}"),
                index: Box::new(index),
            },
            then_branch: vec![call],
            else_branch: Vec::new(),
        }],
    ))
}
