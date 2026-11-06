//! Shared activation guards for scalar children and counted processor arrays.

use super::*;

fn projection_active_field(
    declared: &view_catalog::DeclaredProjection,
    shapes: &HashMap<String, ProcLoweringShape>,
) -> Option<String> {
    let edges = &declared.projection.edges;
    let (owner, edge) = declared.projection.parent_and_declaration()?;
    let active = shapes
        .get(owner)?
        .nested_proc_active_fields
        .get(&edge.declaration)?;
    let parent = view_catalog::DeclaredProjection::source_path_for(&edges[..edges.len() - 1]);
    Some(if parent.is_empty() {
        active.clone()
    } else {
        nested_field_name(&parent, active)
    })
}

/// Activation belongs inside a selected wrapper, after its caller evaluated
/// every input. Borrowed calls use the same active slot before entering here.
pub(super) fn begin_projection_slot(
    declared: &view_catalog::DeclaredProjection,
    shapes: &HashMap<String, ProcLoweringShape>,
    call: Expr,
) -> Option<Stmt> {
    declared.projection.edges.last()?.extent?;
    let active = projection_active_field(declared, shapes)?;
    Some(begin_active_slot(
        &active,
        proc_storage::family_coordinate(declared.projection.coordinate_arity())?,
        call,
    ))
}

/// Guard scalar hooks at their shared owner wrapper so independently scheduled
/// parent, direct, and borrowed calls all observe the same activation.
pub(super) fn guard_scalar_hook(
    declared: &view_catalog::DeclaredProjection,
    shapes: &HashMap<String, ProcLoweringShape>,
    finish: bool,
    mut body: Vec<Stmt>,
) -> Vec<Stmt> {
    let edges = &declared.projection.edges;
    let edge = edges.last().expect("declared processor child");
    if edge.extent.is_some() {
        return body;
    }
    let Some(active) = projection_active_field(declared, shapes) else {
        return body;
    };
    let active = format!("self.{active}");
    body.push(Stmt::Assign {
        loc: Default::default(),
        target_loc: Default::default(),
        target: AssignTarget::Index {
            base: active.clone(),
            index: Expr::int(0),
        },
        decl_ty: None,
        generic_decl_ty: None,
        is_typed_decl: false,
        typed_decl_ty_loc: Default::default(),
        expr: Expr::bool(!finish),
    });
    let active = Expr::Index {
        loc: Default::default(),
        base: active,
        index: Box::new(Expr::int(0)),
    };
    vec![Stmt::If {
        loc: Default::default(),
        cond: if finish {
            active
        } else {
            Expr::UnaryNot {
                loc: Default::default(),
                expr: Box::new(active),
            }
        },
        then_branch: body,
        else_branch: Vec::new(),
    }]
}

fn begin_active_slot(active_field: &str, index: Expr, call: Expr) -> Stmt {
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
