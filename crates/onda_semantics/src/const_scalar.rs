//! Type-preserving evaluation of materialized scalar constant expressions.
//!
//! Source declarations may select a numeric context for literal arithmetic.
//! Boolean conditions have no such context: their operands follow the same
//! literal adaptation, casts, and concrete operations as runtime expressions.
use onda_frontend::{BinaryOp, Diagnostic, Expr, LogicalOp, PrimitiveType};
use onda_mir::{constant_eval, ScalarValue, UnaryOp};

use crate::builtins::{
    builtin_arity, builtin_constant_type, builtin_constant_value_f64, builtin_name, is_float_type,
};
use crate::expr_typing::{
    adapt_binary_types_from_purity, adapt_numeric_argument_types_from_purity,
    intrinsic_result_type, merge_numeric_types_without_diagnostics, scalar_const_kinds,
};
use crate::mir_scalar::{map_binary, map_compare, scalar_type, source_scalar_type, typed_scalar};
use crate::{AnalysisOptions, TypedConstValue};

#[derive(Clone, Copy)]
enum Value<'a> {
    Scalar(ScalarValue, PrimitiveType),
    Literal(&'a Expr, PrimitiveType),
}

impl Value<'_> {
    fn scalar(value: ScalarValue) -> Self {
        Self::Scalar(value, source_scalar_type(value.ty()))
    }

    fn ty(self) -> PrimitiveType {
        match self {
            Self::Scalar(_, ty) => ty,
            Self::Literal(_, ty) => ty,
        }
    }

    fn pure(self) -> bool {
        matches!(self, Self::Literal(..))
    }

    fn resolve(
        self,
        target: Option<PrimitiveType>,
        options: AnalysisOptions,
        context: &str,
    ) -> Result<ScalarValue, Diagnostic> {
        let value = match self {
            Self::Scalar(scalar, _) => scalar,
            Self::Literal(expr, _) => evaluate(expr, target, options, context)?,
        };
        Ok(target.map_or(value, |ty| cast(value, ty).expect("numeric operand cast")))
    }
}

// MIR represents casts involving bool with comparisons/selects. Source const
// evaluation uses their scalar equivalents while sharing all numeric casts.
fn cast(value: ScalarValue, to: PrimitiveType) -> Option<ScalarValue> {
    if let PrimitiveType::Enum(ty) = to {
        let ScalarValue::I64(value) = constant_eval::cast(value, onda_mir::ScalarType::I64)? else {
            return None;
        };
        return Some(ScalarValue::I32(
            value.clamp(0, i64::from(ty.count) - 1) as i32
        ));
    }
    if to == PrimitiveType::Bool {
        return Some(ScalarValue::Bool(match value {
            ScalarValue::F32(value) => value != 0.0,
            ScalarValue::F64(value) => value != 0.0,
            ScalarValue::I32(value) => value != 0,
            ScalarValue::I64(value) => value != 0,
            ScalarValue::Bool(value) => value,
        }));
    }
    let value = match value {
        ScalarValue::Bool(value) => ScalarValue::I32(i32::from(value)),
        value => value,
    };
    constant_eval::cast(value, scalar_type(to))
}

enum Frame<'a> {
    Eval(&'a Expr, Option<PrimitiveType>),
    Finish(&'a Expr),
    Call(&'a Expr, usize, Option<PrimitiveType>),
    Logical(LogicalOp, &'a Expr),
}

pub(crate) fn eval_const_scalar(
    expr: &Expr,
    numeric_context: Option<PrimitiveType>,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<TypedConstValue> {
    debug_assert!(numeric_context.is_none_or(|ty| ty != PrimitiveType::Bool));
    let result = evaluate(expr, numeric_context, options, context);
    match result {
        Ok(value) => {
            let ty = match expr {
                Expr::Int {
                    const_ty: Some(ty), ..
                }
                | Expr::Cast { to: ty, .. } => Some(*ty),
                _ => numeric_context,
            };
            Some(match ty {
                Some(PrimitiveType::Enum(ty)) => TypedConstValue::Enum {
                    ty,
                    ordinal: value.as_f64() as i32,
                },
                _ => typed_scalar(value),
            })
        }
        Err(error) => {
            errors.push(error);
            None
        }
    }
}

fn evaluate(
    expr: &Expr,
    numeric_context: Option<PrimitiveType>,
    options: AnalysisOptions,
    context: &str,
) -> Result<ScalarValue, Diagnostic> {
    // A destination provides context only for literal arithmetic. Concrete
    // operands and explicit casts select their own operation types, just as in MIR.
    let constants = scalar_const_kinds(expr, Expr::children);
    let literal_type = |node: &Expr| {
        constants
            .get(&(node as *const Expr))
            .and_then(|kind| kind.literal_type())
    };
    let literal_root = literal_type(expr).is_some();
    let numeric_context = numeric_context.or_else(|| literal_type(expr)).filter(|ty| {
        literal_root
            && (matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
                || (!matches!(expr, Expr::Int { .. } | Expr::Var { .. })
                    && literal_type(expr)
                        .is_some_and(|ty| matches!(ty, PrimitiveType::I32 | PrimitiveType::I64))))
    });
    // In mixed trees every literal operand is deferred. In literal trees the
    // context is already known and selects the floating arithmetic width.
    let mut frames = vec![Frame::Eval(expr, numeric_context)];
    let mut values = Vec::<Value<'_>>::new();
    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Eval(node, mut target) => {
                let diagnostic = |message| Diagnostic::semantic_span(message, node.loc());
                if !literal_root {
                    if let Some(ty) = literal_type(node) {
                        values.push(Value::Literal(node, ty));
                        continue;
                    }
                }
                // Integer-only literal subtrees use full integer precision
                // before their result converts into a floating context.
                if target.is_some_and(|ty| matches!(ty, PrimitiveType::F32 | PrimitiveType::F64))
                    && literal_type(node)
                        .is_some_and(|ty| matches!(ty, PrimitiveType::I32 | PrimitiveType::I64))
                {
                    target = Some(PrimitiveType::I64);
                }
                let scalar = match node {
                    Expr::Number { value, .. } => ScalarValue::F64(*value),
                    Expr::Int { value, .. } => ScalarValue::I64(*value),
                    Expr::Bool { value, .. } => ScalarValue::Bool(*value),
                    Expr::Var { name, .. } => {
                        let Some((value, ty)) = builtin_constant_value_f64(name, options)
                            .zip(builtin_constant_type(name))
                        else {
                            return Err(diagnostic(format!(
                                "{context} uses non-constant symbol '{name}'"
                            )));
                        };
                        cast(ScalarValue::F64(value), ty).expect("builtin constant cast")
                    }
                    Expr::Logical { op, lhs, rhs, .. } => {
                        frames.push(Frame::Logical(*op, rhs));
                        frames.push(Frame::Eval(lhs, None));
                        continue;
                    }
                    Expr::Compare { lhs, rhs, .. } => {
                        frames.push(Frame::Finish(node));
                        frames.push(Frame::Eval(rhs, None));
                        frames.push(Frame::Eval(lhs, None));
                        continue;
                    }
                    Expr::Binary { lhs, rhs, .. } => {
                        frames.push(Frame::Finish(node));
                        frames.push(Frame::Eval(rhs, target));
                        frames.push(Frame::Eval(lhs, target));
                        continue;
                    }
                    Expr::Cast { expr, .. } => {
                        frames.push(Frame::Finish(node));
                        frames.push(Frame::Eval(expr, None));
                        continue;
                    }
                    Expr::Call { func, args, .. } => {
                        let arity = builtin_arity(*func);
                        if args.len() != arity {
                            return Err(diagnostic(format!(
                                "{context}: builtin '{}' expects {arity} argument(s), got {}",
                                builtin_name(*func),
                                args.len()
                            )));
                        }
                        frames.push(Frame::Call(node, values.len(), target));
                        frames.extend(args.iter().rev().map(|arg| Frame::Eval(arg, target)));
                        continue;
                    }
                    Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
                        frames.push(Frame::Finish(node));
                        frames.push(Frame::Eval(expr, target));
                        continue;
                    }
                    _ => {
                        return Err(diagnostic(format!(
                            "{context} must be a compile-time constant expression"
                        )))
                    }
                };
                let scalar = if let Some(target) = target {
                    cast(scalar, target).expect("numeric context cast")
                } else {
                    scalar
                };
                if let Expr::Int {
                    const_ty: Some(ty), ..
                } = node
                {
                    values.push(Value::Scalar(
                        cast(scalar, *ty).expect("typed literal"),
                        *ty,
                    ));
                } else {
                    values.push(Value::scalar(scalar));
                }
            }
            Frame::Logical(op, rhs) => {
                let lhs = values.pop().expect("left logical operand");
                let ScalarValue::Bool(value) = lhs.resolve(None, options, context)? else {
                    return Err(Diagnostic::semantic_span(
                        format!("{context} requires boolean operands"),
                        rhs.loc(),
                    ));
                };
                if (op == LogicalOp::And && !value) || (op == LogicalOp::Or && value) {
                    values.push(lhs);
                } else {
                    // The result is the right operand; evaluating it does not
                    // require re-evaluating the left subtree or any unused path.
                    frames.push(Frame::Eval(rhs, None));
                }
            }
            Frame::Call(node, base, target) => {
                let Expr::Call { func, .. } = node else {
                    unreachable!()
                };
                let args = values.drain(base..).collect::<Vec<_>>();
                let pure = args.iter().map(|value| value.pure()).collect::<Vec<_>>();
                let types = args.iter().map(|value| value.ty()).collect::<Vec<_>>();
                let adapted = adapt_numeric_argument_types_from_purity(&types, &pure);
                let mut ty = intrinsic_result_type(*func, &adapted).ok_or_else(|| {
                    Diagnostic::semantic_span(
                        format!(
                            "{context}: builtin '{}' has incompatible argument types",
                            builtin_name(*func)
                        ),
                        node.loc(),
                    )
                })?;
                // A pure floating builtin follows the declaration's literal
                // context even when its arguments are exact integer literals.
                if matches!(ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    ty = target.unwrap_or(ty);
                }
                let args = args
                    .iter()
                    .map(|arg| arg.resolve(Some(ty), options, context))
                    .collect::<Result<Vec<_>, _>>()?;
                let scalar = crate::mir_scalar::map_intrinsic(*func)
                    .and_then(|intrinsic| constant_eval::intrinsic(intrinsic, &args))
                    .ok_or_else(|| {
                        Diagnostic::semantic_span(
                            format!(
                                "{context}: builtin '{}' cannot be evaluated as {}",
                                builtin_name(*func),
                                ty.name()
                            ),
                            node.loc(),
                        )
                    })?;
                values.push(Value::scalar(scalar));
            }
            Frame::Finish(node) => {
                let diagnostic = |message| Diagnostic::semantic_span(message, node.loc());
                let rhs = values.pop().expect("constant operand");
                let value = match node {
                    Expr::Cast { to, .. } => {
                        if !crate::enum_support::valid_cast(rhs.ty(), *to) {
                            return Err(diagnostic(format!(
                                "{context}: enum conversion requires an integer ordinal; use i32(value) to extract it"
                            )));
                        }
                        Value::Scalar(
                            cast(rhs.resolve(None, options, context)?, *to).expect("scalar cast"),
                            *to,
                        )
                    }
                    Expr::UnaryNot { .. } | Expr::UnaryBitNot { .. } => {
                        if matches!(rhs.ty(), PrimitiveType::Enum(_)) {
                            return Err(diagnostic(format!(
                                "{context} has an invalid enum operand"
                            )));
                        }
                        let op = if matches!(node, Expr::UnaryNot { .. }) {
                            UnaryOp::LogicalNot
                        } else {
                            UnaryOp::BitNot
                        };
                        Value::scalar(
                            constant_eval::unary(op, rhs.resolve(None, options, context)?)
                                .ok_or_else(|| {
                                    diagnostic(format!("{context} has an invalid unary operand"))
                                })?,
                        )
                    }
                    Expr::Binary { .. } | Expr::Compare { .. } => {
                        let lhs = values.pop().expect("left constant operand");
                        if matches!(lhs.ty(), PrimitiveType::Enum(_))
                            && matches!(node, Expr::Compare { op, .. } if !matches!(op, onda_frontend::CmpOp::Eq | onda_frontend::CmpOp::Ne))
                        {
                            return Err(diagnostic(format!(
                                "{context}: enums only support == and != comparisons"
                            )));
                        }
                        let (lhs_ty, rhs_ty) = adapt_binary_types_from_purity(
                            lhs.ty(),
                            rhs.ty(),
                            lhs.pure(),
                            rhs.pure(),
                        );
                        let ty = if matches!(node, Expr::Compare { .. })
                            && lhs_ty == rhs_ty
                            && matches!(lhs_ty, PrimitiveType::Bool | PrimitiveType::Enum(_))
                        {
                            lhs_ty
                        } else if let Some(ty) = numeric_context.filter(|ty| {
                            is_float_type(*ty) && literal_type(node).is_some_and(is_float_type)
                        }) {
                            // Wide integer literal intermediates convert before
                            // participating in the selected floating arithmetic.
                            ty
                        } else {
                            merge_numeric_types_without_diagnostics(lhs_ty, rhs_ty)
                                .ok_or_else(|| {
                                    diagnostic(format!(
                                        "{context} requires numeric operands, got {lhs_ty:?} and {rhs_ty:?}"
                                    ))
                                })?
                        };
                        let left = lhs.resolve(Some(ty), options, context)?;
                        let right = rhs.resolve(Some(ty), options, context)?;
                        let scalar = match node {
                            Expr::Binary { op, .. } => {
                                constant_eval::binary(map_binary(*op), left, right).ok_or_else(
                                    || {
                                        let reason = match op {
                                            BinaryOp::Div
                                                if matches!(
                                                    ty,
                                                    PrimitiveType::I32 | PrimitiveType::I64
                                                ) =>
                                            {
                                                "division by zero"
                                            }
                                            BinaryOp::Mod
                                                if matches!(
                                                    ty,
                                                    PrimitiveType::I32 | PrimitiveType::I64
                                                ) =>
                                            {
                                                "modulo by zero"
                                            }
                                            _ => "has incompatible numeric operands",
                                        };
                                        diagnostic(format!("{context} {reason}"))
                                    },
                                )?
                            }
                            Expr::Compare { op, .. } => ScalarValue::Bool(
                                constant_eval::compare(map_compare(*op), left, right).ok_or_else(
                                    || {
                                        diagnostic(format!(
                                            "{context} has incompatible comparison operands"
                                        ))
                                    },
                                )?,
                            ),
                            _ => unreachable!(),
                        };
                        Value::scalar(scalar)
                    }
                    _ => unreachable!(),
                };
                values.push(value);
            }
        }
    }
    values
        .pop()
        .expect("constant result")
        .resolve(None, options, context)
}
