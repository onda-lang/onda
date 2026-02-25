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

pub(crate) fn parse_data_len_instance_base(name: &str) -> Option<&str> {
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
        "PI" | "pi" => Some(std::f32::consts::PI as f64),
        "TWO_PI" | "TWOPI" | "two_pi" | "twopi" => Some((2.0 * std::f32::consts::PI) as f64),
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => {
            Some(options.sample_rate as f64)
        }
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => {
            Some(options.block_size as f64)
        }
        _ => None,
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
            let lhs_value = eval_const_expr_f64(lhs, options, context, errors)?;
            let rhs_value = eval_const_expr_f64(rhs, options, context, errors)?;
            Some(match op {
                BinaryOp::Add => lhs_value + rhs_value,
                BinaryOp::Sub => lhs_value - rhs_value,
                BinaryOp::Mul => lhs_value * rhs_value,
                BinaryOp::Div => lhs_value / rhs_value,
                BinaryOp::Mod => lhs_value % rhs_value,
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
