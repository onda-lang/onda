use omni_frontend::{BinaryOp, BuiltinFn, CmpOp, Diagnostic, Expr, LogicalOp, PrimitiveType};

use crate::AnalysisOptions;

pub(crate) fn is_builtin_constant_name(name: &str) -> bool {
    matches!(
        name,
        "PI" | "TWO_PI"
            | "TWOPI"
            | "pi"
            | "two_pi"
            | "twopi"
            | "SAMPLE_RATE"
            | "SAMPLERATE"
            | "SR"
            | "sample_rate"
            | "samplerate"
            | "BLOCK_SIZE"
            | "BLOCKSIZE"
            | "BS"
            | "block_size"
            | "blocksize"
    )
}

pub(crate) fn builtin_constant_type(name: &str) -> Option<PrimitiveType> {
    match name {
        "PI" | "pi" | "TWO_PI" | "TWOPI" | "two_pi" | "twopi" => Some(PrimitiveType::F64),
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => {
            Some(PrimitiveType::F32)
        }
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => Some(PrimitiveType::I32),
        _ => None,
    }
}

pub(crate) fn is_builtin_function_name(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "tanh"
            | "atan"
            | "atan2"
            | "exp"
            | "log"
            | "sqrt"
            | "pow"
            | "abs"
            | "fabs"
            | "floor"
            | "ceil"
            | "round"
            | "trunc"
            | "min"
            | "max"
            | "fma"
            | "unsafe_read"
            | "unsafe_write"
    )
}

pub(crate) fn is_builtin_unsafe_data_fn(name: &str) -> bool {
    matches!(name, "unsafe_read" | "unsafe_write")
}

pub(crate) fn is_internal_buffer_2d_fn(name: &str) -> bool {
    matches!(name, "__omni_buffer_read2" | "__omni_buffer_write2")
}

fn split_instance_method_path(name: &str) -> Option<(&str, &str)> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method.is_empty() {
        return None;
    }
    Some((base, method))
}

pub(crate) fn parse_array_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "len" {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "chans" {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn parse_buffer_samplerate_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "samplerate" {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn parse_unsafe_read_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "unsafe_read" {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn parse_unsafe_write_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == "unsafe_write" {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn builtin_arity(func: BuiltinFn) -> usize {
    match func {
        BuiltinFn::Sin
        | BuiltinFn::Cos
        | BuiltinFn::Tan
        | BuiltinFn::Tanh
        | BuiltinFn::Atan
        | BuiltinFn::Exp
        | BuiltinFn::Log
        | BuiltinFn::Sqrt
        | BuiltinFn::Abs
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Trunc => 1,
        BuiltinFn::Pow | BuiltinFn::Atan2 | BuiltinFn::Min | BuiltinFn::Max => 2,
        BuiltinFn::Fma => 3,
    }
}

pub(crate) fn builtin_name(func: BuiltinFn) -> &'static str {
    match func {
        BuiltinFn::Sin => "sin",
        BuiltinFn::Cos => "cos",
        BuiltinFn::Tan => "tan",
        BuiltinFn::Tanh => "tanh",
        BuiltinFn::Atan => "atan",
        BuiltinFn::Atan2 => "atan2",
        BuiltinFn::Exp => "exp",
        BuiltinFn::Log => "log",
        BuiltinFn::Sqrt => "sqrt",
        BuiltinFn::Pow => "pow",
        BuiltinFn::Abs => "abs",
        BuiltinFn::Floor => "floor",
        BuiltinFn::Ceil => "ceil",
        BuiltinFn::Round => "round",
        BuiltinFn::Trunc => "trunc",
        BuiltinFn::Min => "min",
        BuiltinFn::Max => "max",
        BuiltinFn::Fma => "fma",
    }
}

pub(crate) fn is_float_type(ty: PrimitiveType) -> bool {
    matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
}

pub(crate) fn builtin_constant_value_f64(name: &str, options: AnalysisOptions) -> Option<f64> {
    match name {
        "PI" | "pi" => Some(std::f64::consts::PI),
        "TWO_PI" | "TWOPI" | "two_pi" | "twopi" => Some(2.0 * std::f64::consts::PI),
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => {
            Some(options.sample_rate as f64)
        }
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => {
            Some(options.block_size as f64)
        }
        _ => None,
    }
}

fn merge_const_integer_types(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

pub(crate) fn infer_const_expr_type(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number { .. } => Some(PrimitiveType::F32),
        Expr::Int { value: v, .. } => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool { .. } => Some(PrimitiveType::Bool),
        Expr::Var { name, .. } => builtin_constant_type(name).or_else(|| {
            errors.push(Diagnostic::semantic_span(
                format!("{context} uses non-constant symbol '{name}'"),
                expr.loc(),
            ));
            None
        }),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner = infer_const_expr_type(expr, options, context, errors)?;
            merge_const_integer_types(inner, inner).or_else(|| {
                errors.push(Diagnostic::semantic_span(
                    format!(
                        "{context} bitwise not requires integer operand, got {:?}",
                        inner
                    ),
                    expr.loc(),
                ));
                None
            })
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_ty = infer_const_expr_type(lhs, options, context, errors)?;
            let rhs_ty = infer_const_expr_type(rhs, options, context, errors)?;
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => merge_const_integer_types(lhs_ty, rhs_ty).or_else(|| {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context} bitwise expression requires integer operands, got {:?} and {:?}",
                            lhs_ty, rhs_ty
                        ),
                        expr.loc(),
                    ));
                    None
                }),
                _ => {
                    use PrimitiveType::*;
                    match (lhs_ty, rhs_ty) {
                        (F64, I32)
                        | (I32, F64)
                        | (F64, I64)
                        | (I64, F64)
                        | (F64, F32)
                        | (F32, F64)
                        | (F64, F64) => Some(F64),
                        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
                        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
                        (I32, I32) => Some(I32),
                        _ => {
                            errors.push(Diagnostic::semantic_span(
                                format!(
                                    "{context} requires numeric operands, got {:?} and {:?}",
                                    lhs_ty, rhs_ty
                                ),
                                expr.loc(),
                            ));
                            None
                        }
                    }
                }
            }
        }
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must be a compile-time constant expression"),
                expr.loc(),
            ));
            None
        }
    }
}

pub(crate) fn can_eval_const_expr_exact_int(expr: &Expr) -> bool {
    match expr {
        Expr::Int { .. } | Expr::Bool { .. } => true,
        Expr::Number { .. } => false,
        Expr::Var { name, .. } => matches!(
            builtin_constant_type(name),
            Some(PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool)
        ),
        Expr::Cast { to, expr, .. } => {
            matches!(
                to,
                PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool
            ) && can_eval_const_expr_exact_int(expr)
        }
        Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            can_eval_const_expr_exact_int(expr)
        }
        Expr::Logical { lhs, rhs, .. }
        | Expr::Compare { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            can_eval_const_expr_exact_int(lhs) && can_eval_const_expr_exact_int(rhs)
        }
        _ => false,
    }
}

pub(crate) fn eval_const_expr_i64_exact(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<i64> {
    fn push_division_by_zero(
        expr: &Expr,
        context: &str,
        errors: &mut Vec<Diagnostic>,
        op: &str,
    ) -> Option<i64> {
        errors.push(Diagnostic::semantic_span(
            format!("{context} {op} by zero"),
            expr.loc(),
        ));
        None
    }

    match expr {
        Expr::Int { value, .. } => Some(*value),
        Expr::Bool { value, .. } => Some(if *value { 1 } else { 0 }),
        Expr::Var { name, .. } => match builtin_constant_type(name) {
            Some(PrimitiveType::I32) => Some(options.block_size as i64),
            Some(PrimitiveType::I64) => builtin_constant_value_f64(name, options).map(|v| v as i64),
            Some(PrimitiveType::Bool) => {
                Some(if builtin_constant_value_f64(name, options)? != 0.0 {
                    1
                } else {
                    0
                })
            }
            _ => {
                errors.push(Diagnostic::semantic_span(
                    format!("{context} uses non-constant symbol '{name}'"),
                    expr.loc(),
                ));
                None
            }
        },
        Expr::Cast { to, expr, .. } => {
            let value = eval_const_expr_i64_exact(expr, options, context, errors)?;
            Some(match to {
                PrimitiveType::I32 => (value as i32) as i64,
                PrimitiveType::I64 => value,
                PrimitiveType::Bool => {
                    if value != 0 {
                        1
                    } else {
                        0
                    }
                }
                _ => {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context} must evaluate to an integer constant expression"),
                        expr.loc(),
                    ));
                    return None;
                }
            })
        }
        Expr::UnaryNot { expr, .. } => {
            let value = eval_const_expr_i64_exact(expr, options, context, errors)?;
            Some(if value == 0 { 1 } else { 0 })
        }
        Expr::UnaryBitNot { expr, .. } => {
            let ty = infer_const_expr_type(expr, options, context, errors)?;
            let value = eval_const_expr_i64_exact(expr, options, context, errors)?;
            Some(match ty {
                PrimitiveType::I32 => (!(value as i32)) as i64,
                PrimitiveType::I64 => !value,
                _ => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context} bitwise not requires integer operand, got {:?}",
                            ty
                        ),
                        expr.loc(),
                    ));
                    return None;
                }
            })
        }
        Expr::Logical { op, lhs, rhs, .. } => {
            let lhs_value = eval_const_expr_i64_exact(lhs, options, context, errors)?;
            match op {
                LogicalOp::And => {
                    if lhs_value == 0 {
                        Some(0)
                    } else {
                        let rhs_value = eval_const_expr_i64_exact(rhs, options, context, errors)?;
                        Some(if rhs_value != 0 { 1 } else { 0 })
                    }
                }
                LogicalOp::Or => {
                    if lhs_value != 0 {
                        Some(1)
                    } else {
                        let rhs_value = eval_const_expr_i64_exact(rhs, options, context, errors)?;
                        Some(if rhs_value != 0 { 1 } else { 0 })
                    }
                }
            }
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            let lhs_value = eval_const_expr_i64_exact(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_i64_exact(rhs, options, context, errors)?;
            let pred = match op {
                CmpOp::Eq => lhs_value == rhs_value,
                CmpOp::Ne => lhs_value != rhs_value,
                CmpOp::Lt => lhs_value < rhs_value,
                CmpOp::Le => lhs_value <= rhs_value,
                CmpOp::Gt => lhs_value > rhs_value,
                CmpOp::Ge => lhs_value >= rhs_value,
            };
            Some(if pred { 1 } else { 0 })
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let result_ty = infer_const_expr_type(expr, options, context, errors)?;
            let lhs_value = eval_const_expr_i64_exact(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_i64_exact(rhs, options, context, errors)?;
            match result_ty {
                PrimitiveType::I32 => {
                    let lhs = lhs_value as i32;
                    let rhs = rhs_value as i32;
                    Some(match op {
                        BinaryOp::Add => lhs.wrapping_add(rhs) as i64,
                        BinaryOp::Sub => lhs.wrapping_sub(rhs) as i64,
                        BinaryOp::Mul => lhs.wrapping_mul(rhs) as i64,
                        BinaryOp::Div => {
                            if rhs == 0 {
                                return push_division_by_zero(expr, context, errors, "division");
                            }
                            lhs.wrapping_div(rhs) as i64
                        }
                        BinaryOp::Mod => {
                            if rhs == 0 {
                                return push_division_by_zero(expr, context, errors, "modulo");
                            }
                            lhs.wrapping_rem(rhs) as i64
                        }
                        BinaryOp::BitAnd => (lhs & rhs) as i64,
                        BinaryOp::BitOr => (lhs | rhs) as i64,
                        BinaryOp::BitXor => (lhs ^ rhs) as i64,
                        BinaryOp::ShiftLeft => lhs.wrapping_shl(rhs as u32) as i64,
                        BinaryOp::ShiftRight => lhs.wrapping_shr(rhs as u32) as i64,
                    })
                }
                PrimitiveType::I64 => Some(match op {
                    BinaryOp::Add => lhs_value.wrapping_add(rhs_value),
                    BinaryOp::Sub => lhs_value.wrapping_sub(rhs_value),
                    BinaryOp::Mul => lhs_value.wrapping_mul(rhs_value),
                    BinaryOp::Div => {
                        if rhs_value == 0 {
                            return push_division_by_zero(expr, context, errors, "division");
                        }
                        lhs_value.wrapping_div(rhs_value)
                    }
                    BinaryOp::Mod => {
                        if rhs_value == 0 {
                            return push_division_by_zero(expr, context, errors, "modulo");
                        }
                        lhs_value.wrapping_rem(rhs_value)
                    }
                    BinaryOp::BitAnd => lhs_value & rhs_value,
                    BinaryOp::BitOr => lhs_value | rhs_value,
                    BinaryOp::BitXor => lhs_value ^ rhs_value,
                    BinaryOp::ShiftLeft => lhs_value.wrapping_shl(rhs_value as u32),
                    BinaryOp::ShiftRight => lhs_value.wrapping_shr(rhs_value as u32),
                }),
                _ => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context} must evaluate to an integer constant expression, got {:?}",
                            result_ty
                        ),
                        expr.loc(),
                    ));
                    None
                }
            }
        }
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must be a compile-time integer constant expression"),
                expr.loc(),
            ));
            None
        }
    }
}

pub(crate) fn eval_const_expr_f64(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match expr {
        Expr::Number { value: v, .. } => Some(*v),
        Expr::Int { value: v, .. } => Some(*v as f64),
        Expr::Bool { value: v, .. } => Some(if *v { 1.0 } else { 0.0 }),
        Expr::Var { name, .. } => {
            if let Some(value) = builtin_constant_value_f64(name, options) {
                Some(value)
            } else {
                errors.push(Diagnostic::semantic_span(
                    format!("{context} uses non-constant symbol '{name}'"),
                    expr.loc(),
                ));
                None
            }
        }
        Expr::Cast { to, expr, .. } => {
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(match to {
                PrimitiveType::F32 | PrimitiveType::F64 => value,
                PrimitiveType::I32 => (value as i32) as f64,
                PrimitiveType::I64 => (value as i64) as f64,
                PrimitiveType::Bool => {
                    if value != 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            })
        }
        Expr::UnaryNot { expr, .. } => {
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(if value == 0.0 { 1.0 } else { 0.0 })
        }
        Expr::UnaryBitNot { expr, .. } => {
            let ty = infer_const_expr_type(expr, options, context, errors)?;
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(match ty {
                PrimitiveType::I32 => (!(value as i32)) as f64,
                PrimitiveType::I64 => (!(value as i64)) as f64,
                _ => {
                    errors.push(Diagnostic::semantic_span(
                        format!(
                            "{context} bitwise not requires integer operand, got {:?}",
                            ty
                        ),
                        expr.loc(),
                    ));
                    return None;
                }
            })
        }
        Expr::Logical { op, lhs, rhs, .. } => {
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            match op {
                LogicalOp::And => {
                    if lhs_value == 0.0 {
                        Some(0.0)
                    } else {
                        let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
                        Some(if rhs_value != 0.0 { 1.0 } else { 0.0 })
                    }
                }
                LogicalOp::Or => {
                    if lhs_value != 0.0 {
                        Some(1.0)
                    } else {
                        let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
                        Some(if rhs_value != 0.0 { 1.0 } else { 0.0 })
                    }
                }
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let result_ty = infer_const_expr_type(expr, options, context, errors)?;
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
            Some(match op {
                BinaryOp::Add => lhs_value + rhs_value,
                BinaryOp::Sub => lhs_value - rhs_value,
                BinaryOp::Mul => lhs_value * rhs_value,
                BinaryOp::Div => lhs_value / rhs_value,
                BinaryOp::Mod => lhs_value % rhs_value,
                BinaryOp::BitAnd => match result_ty {
                    PrimitiveType::I32 => ((lhs_value as i32) & (rhs_value as i32)) as f64,
                    PrimitiveType::I64 => ((lhs_value as i64) & (rhs_value as i64)) as f64,
                    _ => unreachable!("bitwise expr type must be integer"),
                },
                BinaryOp::BitOr => match result_ty {
                    PrimitiveType::I32 => ((lhs_value as i32) | (rhs_value as i32)) as f64,
                    PrimitiveType::I64 => ((lhs_value as i64) | (rhs_value as i64)) as f64,
                    _ => unreachable!("bitwise expr type must be integer"),
                },
                BinaryOp::BitXor => match result_ty {
                    PrimitiveType::I32 => ((lhs_value as i32) ^ (rhs_value as i32)) as f64,
                    PrimitiveType::I64 => ((lhs_value as i64) ^ (rhs_value as i64)) as f64,
                    _ => unreachable!("bitwise expr type must be integer"),
                },
                BinaryOp::ShiftLeft => match result_ty {
                    PrimitiveType::I32 => ((lhs_value as i32) << (rhs_value as i32)) as f64,
                    PrimitiveType::I64 => ((lhs_value as i64) << (rhs_value as i64)) as f64,
                    _ => unreachable!("bitwise expr type must be integer"),
                },
                BinaryOp::ShiftRight => match result_ty {
                    PrimitiveType::I32 => ((lhs_value as i32) >> (rhs_value as i32)) as f64,
                    PrimitiveType::I64 => ((lhs_value as i64) >> (rhs_value as i64)) as f64,
                    _ => unreachable!("bitwise expr type must be integer"),
                },
            })
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
            let pred = match op {
                CmpOp::Eq => lhs_value == rhs_value,
                CmpOp::Ne => lhs_value != rhs_value,
                CmpOp::Lt => lhs_value < rhs_value,
                CmpOp::Le => lhs_value <= rhs_value,
                CmpOp::Gt => lhs_value > rhs_value,
                CmpOp::Ge => lhs_value >= rhs_value,
            };
            Some(if pred { 1.0 } else { 0.0 })
        }
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must be a compile-time constant expression"),
                expr.loc(),
            ));
            None
        }
    }
}

pub(crate) fn eval_const_bool_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<bool> {
    let ty = infer_const_expr_type(expr, options, context, errors)?;
    if ty != PrimitiveType::Bool {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context} must evaluate to a compile-time bool, got {:?}",
                ty
            ),
            expr.loc(),
        ));
        return None;
    }
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    Some(value != 0.0)
}

pub(crate) fn eval_data_size_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if can_eval_const_expr_exact_int(expr) {
        let value = eval_const_expr_i64_exact(expr, options, context, errors)?;
        if value <= 0 {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must be greater than zero"),
                expr.loc(),
            ));
            return None;
        }
        let Ok(value) = usize::try_from(value) else {
            errors.push(Diagnostic::semantic_span(
                format!("{context} exceeds supported range"),
                expr.loc(),
            ));
            return None;
        };
        return Some(value);
    }

    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        errors.push(Diagnostic::semantic_span(
            format!("{context} must evaluate to a finite numeric value"),
            expr.loc(),
        ));
        return None;
    }

    let truncated = value.trunc();
    if (value - truncated).abs() > 1e-6 {
        errors.push(Diagnostic::semantic_span(
            format!("{context} must evaluate to an integer value"),
            expr.loc(),
        ));
        return None;
    }
    if truncated <= 0.0 {
        errors.push(Diagnostic::semantic_span(
            format!("{context} must be greater than zero"),
            expr.loc(),
        ));
        return None;
    }
    if truncated > usize::MAX as f64 {
        errors.push(Diagnostic::semantic_span(
            format!("{context} exceeds supported range"),
            expr.loc(),
        ));
        return None;
    }

    Some(truncated as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_const_expr_i64_exact_preserves_large_integer_results() {
        let expr = Expr::Binary {
            loc: Default::default(),
            op: BinaryOp::Add,
            lhs: Box::new(Expr::int(9_007_199_254_740_992)),
            rhs: Box::new(Expr::int(1)),
        };
        let mut errors = Vec::new();
        let value = eval_const_expr_i64_exact(
            &expr,
            AnalysisOptions::default(),
            "large integer const",
            &mut errors,
        );
        assert_eq!(value, Some(9_007_199_254_740_993));
        assert!(
            errors.is_empty(),
            "expected exact integer eval to succeed, got {errors:?}"
        );
    }

    #[test]
    fn eval_data_size_expr_preserves_large_i64_consts() {
        let expr = Expr::Cast {
            loc: Default::default(),
            to: PrimitiveType::I64,
            expr: Box::new(Expr::Binary {
                loc: Default::default(),
                op: BinaryOp::Add,
                lhs: Box::new(Expr::int(9_007_199_254_740_992)),
                rhs: Box::new(Expr::int(1)),
            }),
        };
        let expected = usize::try_from(9_007_199_254_740_993_i64).unwrap();
        let mut errors = Vec::new();
        let value =
            eval_data_size_expr(&expr, AnalysisOptions::default(), "array size", &mut errors);
        assert_eq!(value, Some(expected));
        assert!(
            errors.is_empty(),
            "expected exact size eval to succeed, got {errors:?}"
        );
    }
}
