//! Scalar constant operations shared by source evaluation and MIR folding.
//! Operands must already have the same concrete type. Invalid operations,
//! including integer division by zero, return `None`.
use crate::{BinaryOp, CompareOp, Intrinsic, ScalarType, ScalarValue};

// LLVM minimum/maximum propagate NaN and select the appropriate zero sign.
// RangeClamp deliberately uses maxnum/minnum instead, mapping NaN to a bound.
macro_rules! float_extremum {
    ($float:ty, $lhs:expr, $rhs:expr, $minimum:expr) => {{
        let (lhs, rhs) = ($lhs, $rhs);
        if lhs.is_nan() {
            lhs
        } else if rhs.is_nan() {
            rhs
        } else if lhs == 0.0 && rhs == 0.0 {
            <$float>::from_bits(if $minimum {
                lhs.to_bits() | rhs.to_bits()
            } else {
                lhs.to_bits() & rhs.to_bits()
            })
        } else if $minimum {
            lhs.min(rhs)
        } else {
            lhs.max(rhs)
        }
    }};
}

macro_rules! float_intrinsic {
    ($op:expr, $args:expr, $variant:path, $float:ty) => {{
        let value = match ($op, $args) {
            (Intrinsic::Sin, [$variant(x)]) => x.sin(),
            (Intrinsic::Cos, [$variant(x)]) => x.cos(),
            (Intrinsic::Tan, [$variant(x)]) => x.tan(),
            (Intrinsic::Tanh, [$variant(x)]) => x.tanh(),
            (Intrinsic::Atan, [$variant(x)]) => x.atan(),
            (Intrinsic::Atan2, [$variant(x), $variant(y)]) => x.atan2(*y),
            (Intrinsic::Exp, [$variant(x)]) => x.exp(),
            (Intrinsic::Log, [$variant(x)]) => x.ln(),
            (Intrinsic::Sqrt, [$variant(x)]) => x.sqrt(),
            (Intrinsic::Pow, [$variant(x), $variant(y)]) => x.powf(*y),
            (Intrinsic::Abs, [$variant(x)]) => x.abs(),
            (Intrinsic::Floor, [$variant(x)]) => x.floor(),
            (Intrinsic::Ceil, [$variant(x)]) => x.ceil(),
            (Intrinsic::Round, [$variant(x)]) => x.round(),
            (Intrinsic::Trunc, [$variant(x)]) => x.trunc(),
            (Intrinsic::Min, [$variant(x), $variant(y)]) => {
                float_extremum!($float, *x, *y, true)
            }
            (Intrinsic::Max, [$variant(x), $variant(y)]) => {
                float_extremum!($float, *x, *y, false)
            }
            (Intrinsic::Fma, [$variant(x), $variant(y), $variant(z)]) => x.mul_add(*y, *z),
            (Intrinsic::RangeClamp, [$variant(x), $variant(min), $variant(max)]) => {
                x.max(*min).min(*max)
            }
            _ => return None,
        };
        Some($variant(value))
    }};
}

macro_rules! integer_intrinsic {
    ($op:expr, $args:expr, $variant:path) => {{
        let value = match ($op, $args) {
            (Intrinsic::Abs, [$variant(x)]) => x.wrapping_abs(),
            (Intrinsic::Min, [$variant(x), $variant(y)]) => (*x).min(*y),
            (Intrinsic::Max, [$variant(x), $variant(y)]) => (*x).max(*y),
            (Intrinsic::RangeClamp, [$variant(x), $variant(min), $variant(max)]) => {
                (*x).max(*min).min(*max)
            }
            (Intrinsic::RangeWrap, [$variant(x), $variant(min), $variant(max)]) => {
                let lower = i128::from(*min);
                let width = i128::from(*max) - lower + 1;
                if width <= 0 {
                    return None;
                }
                (lower + (i128::from(*x) - lower).rem_euclid(width)) as _
            }
            _ => return None,
        };
        Some($variant(value))
    }};
}

/// Evaluate a builtin on homogeneous concrete operands, without allocation.
pub fn intrinsic(op: Intrinsic, args: &[ScalarValue]) -> Option<ScalarValue> {
    match args.first()? {
        ScalarValue::F32(_) => float_intrinsic!(op, args, ScalarValue::F32, f32),
        ScalarValue::F64(_) => float_intrinsic!(op, args, ScalarValue::F64, f64),
        ScalarValue::I32(_) => integer_intrinsic!(op, args, ScalarValue::I32),
        ScalarValue::I64(_) => integer_intrinsic!(op, args, ScalarValue::I64),
        ScalarValue::Bool(_) => None,
    }
}

pub fn unary(op: crate::UnaryOp, value: ScalarValue) -> Option<ScalarValue> {
    match (op, value) {
        (crate::UnaryOp::Negate, ScalarValue::F32(value)) => Some(ScalarValue::F32(-value)),
        (crate::UnaryOp::Negate, ScalarValue::F64(value)) => Some(ScalarValue::F64(-value)),
        (crate::UnaryOp::Negate, ScalarValue::I32(value)) => {
            Some(ScalarValue::I32(value.wrapping_neg()))
        }
        (crate::UnaryOp::Negate, ScalarValue::I64(value)) => {
            Some(ScalarValue::I64(value.wrapping_neg()))
        }
        (crate::UnaryOp::LogicalNot, ScalarValue::Bool(value)) => Some(ScalarValue::Bool(!value)),
        (crate::UnaryOp::BitNot, ScalarValue::I32(value)) => Some(ScalarValue::I32(!value)),
        (crate::UnaryOp::BitNot, ScalarValue::I64(value)) => Some(ScalarValue::I64(!value)),
        _ => None,
    }
}

macro_rules! fold_integer_binary {
    ($op:expr, $lhs:expr, $rhs:expr, $variant:path) => {{
        let value = match $op {
            BinaryOp::Add => $lhs.wrapping_add($rhs),
            BinaryOp::Subtract => $lhs.wrapping_sub($rhs),
            BinaryOp::Multiply => $lhs.wrapping_mul($rhs),
            BinaryOp::Divide if $rhs != 0 => $lhs.wrapping_div($rhs),
            BinaryOp::Remainder if $rhs != 0 => $lhs.wrapping_rem($rhs),
            BinaryOp::BitAnd => $lhs & $rhs,
            BinaryOp::BitOr => $lhs | $rhs,
            BinaryOp::BitXor => $lhs ^ $rhs,
            BinaryOp::ShiftLeft => $lhs.wrapping_shl($rhs as u32),
            BinaryOp::ShiftRight => $lhs.wrapping_shr($rhs as u32),
            BinaryOp::Divide | BinaryOp::Remainder => return None,
        };
        Some($variant(value))
    }};
}

pub fn binary(op: BinaryOp, lhs: ScalarValue, rhs: ScalarValue) -> Option<ScalarValue> {
    match (lhs, rhs) {
        (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => {
            fold_integer_binary!(op, lhs, rhs, ScalarValue::I32)
        }
        (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => {
            fold_integer_binary!(op, lhs, rhs, ScalarValue::I64)
        }
        (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => Some(ScalarValue::F32(match op {
            BinaryOp::Add => lhs + rhs,
            BinaryOp::Subtract => lhs - rhs,
            BinaryOp::Multiply => lhs * rhs,
            BinaryOp::Divide => lhs / rhs,
            BinaryOp::Remainder => lhs % rhs,
            _ => return None,
        })),
        (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => Some(ScalarValue::F64(match op {
            BinaryOp::Add => lhs + rhs,
            BinaryOp::Subtract => lhs - rhs,
            BinaryOp::Multiply => lhs * rhs,
            BinaryOp::Divide => lhs / rhs,
            BinaryOp::Remainder => lhs % rhs,
            _ => return None,
        })),
        _ => None,
    }
}

macro_rules! compare_values {
    ($op:expr, $lhs:expr, $rhs:expr) => {
        Some(match $op {
            CompareOp::Equal => $lhs == $rhs,
            CompareOp::NotEqual => $lhs != $rhs,
            CompareOp::Less => $lhs < $rhs,
            CompareOp::LessEqual => $lhs <= $rhs,
            CompareOp::Greater => $lhs > $rhs,
            CompareOp::GreaterEqual => $lhs >= $rhs,
        })
    };
}

pub fn compare(op: CompareOp, lhs: ScalarValue, rhs: ScalarValue) -> Option<bool> {
    match (lhs, rhs) {
        (ScalarValue::F32(lhs), ScalarValue::F32(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::F64(lhs), ScalarValue::F64(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::I32(lhs), ScalarValue::I32(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::I64(lhs), ScalarValue::I64(rhs)) => compare_values!(op, lhs, rhs),
        (ScalarValue::Bool(lhs), ScalarValue::Bool(rhs)) => match op {
            CompareOp::Equal => Some(lhs == rhs),
            CompareOp::NotEqual => Some(lhs != rhs),
            _ => None,
        },
        _ => None,
    }
}

/// Numeric casts; MIR expresses bool conversions as comparisons and selects.
pub fn cast(value: ScalarValue, to: ScalarType) -> Option<ScalarValue> {
    macro_rules! cast_from {
        ($value:expr) => {
            Some(match to {
                ScalarType::F32 => ScalarValue::F32($value as f32),
                ScalarType::F64 => ScalarValue::F64($value as f64),
                ScalarType::I32 => ScalarValue::I32($value as i32),
                ScalarType::I64 => ScalarValue::I64($value as i64),
                ScalarType::Bool => return None,
            })
        };
    }
    match value {
        ScalarValue::F32(value) => cast_from!(value),
        ScalarValue::F64(value) => cast_from!(value),
        ScalarValue::I32(value) => cast_from!(value),
        ScalarValue::I64(value) => cast_from!(value),
        ScalarValue::Bool(_) => None,
    }
}
