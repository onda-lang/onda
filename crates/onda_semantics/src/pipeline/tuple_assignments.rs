//! Capture a checked tuple once, then give each destination ordinary scalar
//! assignment semantics. Checked component types preserve numeric context.
use super::*;

pub(super) fn scalarize_tuple_assignments(
    body: &mut Vec<Stmt>,
    rewrite_stores: &mut impl FnMut(&mut Vec<Stmt>),
) {
    fn visit(body: &mut Vec<Stmt>, counter: &mut usize, rewrite: &mut impl FnMut(&mut Vec<Stmt>)) {
        let mut output = Vec::with_capacity(body.len());
        for mut statement in std::mem::take(body) {
            match &mut statement {
                Stmt::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    visit(then_branch, counter, rewrite);
                    visit(else_branch, counter, rewrite);
                }
                Stmt::For { body, .. } | Stmt::While { body, .. } => visit(body, counter, rewrite),
                _ => {}
            }
            let Stmt::Assign {
                target: AssignTarget::Tuple(targets),
                decl_ty: Some(DeclType::Tuple(types)),
                loc,
                target_loc,
                expr,
                ..
            } = &statement
            else {
                output.push(statement);
                continue;
            };
            let name = format!("__onda_tuple_assignment_{}", *counter);
            *counter += 1;
            let capture = Stmt::Assign {
                loc: loc.clone(),
                target_loc: target_loc.clone(),
                target: AssignTarget::Var(name.clone()),
                decl_ty: Some(DeclType::Tuple(types.clone())),
                generic_decl_ty: None,
                is_typed_decl: false,
                typed_decl_ty_loc: Default::default(),
                expr: expr.clone(),
            };
            let mut stores = Vec::new();
            output.push(capture);
            for (index, (target, ty)) in targets.iter().zip(types).enumerate() {
                let Some(target) = target.binding() else {
                    continue;
                };
                stores.push(Stmt::Assign {
                    loc: loc.clone(),
                    target_loc: target_loc.clone(),
                    target: AssignTarget::Var(target.to_owned()),
                    decl_ty: Some(DeclType::Scalar(*ty)),
                    generic_decl_ty: None,
                    is_typed_decl: false,
                    typed_decl_ty_loc: Default::default(),
                    expr: Expr::Index {
                        loc: loc.clone(),
                        base: name.clone(),
                        index: Box::new(Expr::int(index as i64)),
                    },
                });
            }
            rewrite(&mut stores);
            output.extend(stores);
        }
        *body = output;
    }
    visit(body, &mut 0, rewrite_stores);
}
