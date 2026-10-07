//! Shared scalar generic inference for overload selection and specialization.
use crate::*;

#[derive(Debug)]
pub(super) struct NumericConstraintError {
    pub(super) message: String,
    pub(super) location: SourceLoc,
}

pub(super) fn resolve_numeric_constraints(
    constraints: &[(PrimitiveType, bool, &Expr)],
    explicit: Option<PrimitiveType>,
    symbols: &DeclaredSymbolMap,
) -> Result<PrimitiveType, NumericConstraintError> {
    let error = |message, expr: &Expr| NumericConstraintError {
        message,
        location: expr.loc(),
    };
    let exact = constraints
        .iter()
        .find_map(|(ty, exact, _)| exact.then_some(*ty));
    if let Some(exact) = exact {
        if let Some((other, _, expr)) = constraints
            .iter()
            .find(|(ty, is_exact, _)| *is_exact && *ty != exact)
        {
            return Err(error(
                format!(
                    "has incompatible exact argument types {} and {}",
                    exact.name(),
                    other.name()
                ),
                expr,
            ));
        }
    }

    let merge = |current: Option<PrimitiveType>, ty: PrimitiveType, expr: &Expr| match current {
        None => Ok(Some(ty)),
        Some(current) => merge_inferred_return_types(current, ty)
            .map(Some)
            .ok_or_else(|| {
                error(
                    format!(
                        "has incompatible argument types {} and {}",
                        current.name(),
                        ty.name()
                    ),
                    expr,
                )
            }),
    };
    let target = if let Some(target) = explicit.or(exact) {
        target
    } else {
        // Concrete operands supply the width; contextual operands supply only
        // their numeric family. Apply binding defaults when all are contextual.
        let contextual = constraints
            .iter()
            .map(|(_, exact, expr)| !exact && is_contextual_numeric_expr(expr, symbols))
            .collect::<Vec<_>>();
        let mut concrete = None;
        for ((ty, _, expr), contextual) in constraints.iter().zip(&contextual) {
            if !contextual {
                concrete = merge(concrete, *ty, expr)?;
            }
        }
        let default_float =
            concrete.is_none() && constraints.iter().any(|(ty, _, _)| is_float_type(*ty));
        let mut target = if default_float {
            Some(PrimitiveType::F32)
        } else {
            concrete
        };
        for ((ty, _, expr), contextual) in constraints.iter().zip(&contextual) {
            if !contextual || default_float {
                continue;
            }
            let ty = match concrete {
                Some(concrete) => crate::expr_typing::adapt_contextual_numeric_type(*ty, concrete),
                None => effective_untyped_assignment_type(expr, Some(*ty), symbols).unwrap_or(*ty),
            };
            target = merge(target, ty, expr)?;
        }
        target.unwrap_or(PrimitiveType::F32)
    };
    if !target.is_numeric() {
        return Err(NumericConstraintError {
            message: "inferred as bool, but generic type arguments must be numeric (f32, f64, i32, or i64)"
                .to_owned(),
            location: constraints.first().map_or(SourceLoc::default(), |(_, _, expr)| expr.loc()),
        });
    }
    for (actual, exact, expr) in constraints {
        let compatible = if *exact {
            *actual == target
        } else {
            can_assign_expr_to_type(expr, *actual, target, symbols)
        };
        if !compatible {
            return Err(error(
                format!(
                    "resolves to {}, but argument has type {} and cannot be implicitly converted",
                    target.name(),
                    actual.name()
                ),
                expr,
            ));
        }
    }
    Ok(target)
}
