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

fn builtin_constant_value_f64(name: &str, options: AnalysisOptions) -> Option<f64> {
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

fn infer_const_expr_type(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::Var(name) => builtin_constant_type(name).or_else(|| {
            errors.push(Diagnostic::semantic(
                format!("{context} uses non-constant symbol '{name}'"),
                0,
                0,
            ));
            None
        }),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr } => {
            let inner = infer_const_expr_type(expr, options, context, errors)?;
            merge_const_integer_types(inner, inner).or_else(|| {
                errors.push(Diagnostic::semantic(
                    format!("{context} bitwise not requires integer operand, got {:?}", inner),
                    0,
                    0,
                ));
                None
            })
        }
        Expr::Binary { op, lhs, rhs } => {
            let lhs_ty = infer_const_expr_type(lhs, options, context, errors)?;
            let rhs_ty = infer_const_expr_type(rhs, options, context, errors)?;
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => merge_const_integer_types(lhs_ty, rhs_ty).or_else(|| {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{context} bitwise expression requires integer operands, got {:?} and {:?}",
                            lhs_ty, rhs_ty
                        ),
                        0,
                        0,
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
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "{context} requires numeric operands, got {:?} and {:?}",
                                    lhs_ty, rhs_ty
                                ),
                                0,
                                0,
                            ));
                            None
                        }
                    }
                }
            }
        }
        _ => {
            errors.push(Diagnostic::semantic(
                format!("{context} must be a compile-time constant expression"),
                0,
                0,
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
        Expr::Number(v) => Some(*v as f64),
        Expr::Int(v) => Some(*v as f64),
        Expr::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        Expr::Var(name) => {
            if let Some(value) = builtin_constant_value_f64(name, options) {
                Some(value)
            } else {
                errors.push(Diagnostic::semantic(
                    format!("{context} uses non-constant symbol '{name}'"),
                    0,
                    0,
                ));
                None
            }
        }
        Expr::Cast { to, expr } => {
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
        Expr::UnaryNot { expr } => {
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(if value == 0.0 { 1.0 } else { 0.0 })
        }
        Expr::UnaryBitNot { expr } => {
            let ty = infer_const_expr_type(expr, options, context, errors)?;
            let value = eval_const_expr_f64(expr, options, context, errors)?;
            Some(match ty {
                PrimitiveType::I32 => (!(value as i32)) as f64,
                PrimitiveType::I64 => (!(value as i64)) as f64,
                _ => {
                    errors.push(Diagnostic::semantic(
                        format!("{context} bitwise not requires integer operand, got {:?}", ty),
                        0,
                        0,
                    ));
                    return None;
                }
            })
        }
        Expr::Logical { op, lhs, rhs } => {
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
        Expr::Binary { op, lhs, rhs } => {
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
        Expr::Compare { op, lhs, rhs } => {
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
            errors.push(Diagnostic::semantic(
                format!("{context} must be a compile-time constant expression"),
                0,
                0,
            ));
            None
        }
    }
}

pub(crate) fn eval_data_size_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let value = eval_const_expr_f64(expr, options, context, errors)?;
    if !value.is_finite() {
        errors.push(Diagnostic::semantic(
            format!("{context} must evaluate to a finite numeric value"),
            0,
            0,
        ));
        return None;
    }

    let truncated = value.trunc();
    if (value - truncated).abs() > 1e-6 {
        errors.push(Diagnostic::semantic(
            format!("{context} must evaluate to an integer value"),
            0,
            0,
        ));
        return None;
    }
    if truncated <= 0.0 {
        errors.push(Diagnostic::semantic(
            format!("{context} must be greater than zero"),
            0,
            0,
        ));
        return None;
    }
    if truncated > usize::MAX as f64 {
        errors.push(Diagnostic::semantic(
            format!("{context} exceeds supported range"),
            0,
            0,
        ));
        return None;
    }

    Some(truncated as usize)
}
