use std::collections::HashSet;

use onda_frontend::{
    BinaryOp, BuiltinFn, CmpOp, Diagnostic, Expr, LogicalOp, PrimitiveType, SourceLoc,
    INTERNAL_BUFFER_READ2_FN, INTERNAL_BUFFER_READ3_FN, INTERNAL_BUFFER_READ_CHANNEL_FN,
    INTERNAL_BUFFER_WRITE2_FN, INTERNAL_BUFFER_WRITE3_FN, INTERNAL_BUFFER_WRITE_CHANNEL_FN,
    READ_UNSAFE_FN, WRITE_UNSAFE_FN,
};

use crate::AnalysisOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinConstantValue {
    Pi,
    TwoPi,
    SampleRate,
    HostSampleRate,
    BlockSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinConstant {
    pub name: &'static str,
    pub ty: PrimitiveType,
    pub value: BuiltinConstantValue,
}

pub const BUILTIN_CONSTANTS: &[BuiltinConstant] = &[
    BuiltinConstant {
        name: "PI",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::Pi,
    },
    BuiltinConstant {
        name: "pi",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::Pi,
    },
    BuiltinConstant {
        name: "TWO_PI",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::TwoPi,
    },
    BuiltinConstant {
        name: "TWOPI",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::TwoPi,
    },
    BuiltinConstant {
        name: "two_pi",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::TwoPi,
    },
    BuiltinConstant {
        name: "twopi",
        ty: PrimitiveType::F64,
        value: BuiltinConstantValue::TwoPi,
    },
    BuiltinConstant {
        name: "SAMPLE_RATE",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::SampleRate,
    },
    BuiltinConstant {
        name: "SAMPLERATE",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::SampleRate,
    },
    BuiltinConstant {
        name: "SR",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::SampleRate,
    },
    BuiltinConstant {
        name: "sample_rate",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::SampleRate,
    },
    BuiltinConstant {
        name: "samplerate",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::SampleRate,
    },
    BuiltinConstant {
        name: "HOST_SR",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::HostSampleRate,
    },
    BuiltinConstant {
        name: "HOST_SAMPLE_RATE",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::HostSampleRate,
    },
    BuiltinConstant {
        name: "HOST_SAMPLERATE",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::HostSampleRate,
    },
    BuiltinConstant {
        name: "host_sample_rate",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::HostSampleRate,
    },
    BuiltinConstant {
        name: "host_samplerate",
        ty: PrimitiveType::F32,
        value: BuiltinConstantValue::HostSampleRate,
    },
    BuiltinConstant {
        name: "BLOCK_SIZE",
        ty: PrimitiveType::I32,
        value: BuiltinConstantValue::BlockSize,
    },
    BuiltinConstant {
        name: "BLOCKSIZE",
        ty: PrimitiveType::I32,
        value: BuiltinConstantValue::BlockSize,
    },
    BuiltinConstant {
        name: "BS",
        ty: PrimitiveType::I32,
        value: BuiltinConstantValue::BlockSize,
    },
    BuiltinConstant {
        name: "block_size",
        ty: PrimitiveType::I32,
        value: BuiltinConstantValue::BlockSize,
    },
    BuiltinConstant {
        name: "blocksize",
        ty: PrimitiveType::I32,
        value: BuiltinConstantValue::BlockSize,
    },
];

pub fn builtin_constant(name: &str) -> Option<&'static BuiltinConstant> {
    BUILTIN_CONSTANTS
        .iter()
        .find(|constant| constant.name == name)
}

pub fn builtin_constant_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_CONSTANTS.iter().map(|constant| constant.name)
}

pub(crate) fn host_sample_rate_constant_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_CONSTANTS
        .iter()
        .filter(|constant| constant.value == BuiltinConstantValue::HostSampleRate)
        .map(|constant| constant.name)
}

pub fn is_builtin_constant_name(name: &str) -> bool {
    builtin_constant(name).is_some()
}

pub fn builtin_constant_type(name: &str) -> Option<PrimitiveType> {
    builtin_constant(name).map(|constant| constant.ty)
}

const INTERNAL_BUFFER_2D_FUNCTION_NAMES: &[&str] = &[
    INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_WRITE2_FN,
    INTERNAL_BUFFER_READ_CHANNEL_FN,
    INTERNAL_BUFFER_WRITE_CHANNEL_FN,
    INTERNAL_BUFFER_READ3_FN,
    INTERNAL_BUFFER_WRITE3_FN,
    READ_UNSAFE_FN,
    WRITE_UNSAFE_FN,
];

pub fn public_builtin_function_names() -> impl Iterator<Item = &'static str> {
    BuiltinFn::ALL.into_iter().map(BuiltinFn::name)
}

pub fn is_builtin_function_name(name: &str) -> bool {
    BuiltinFn::from_name(name).is_some()
}

pub fn is_internal_buffer_2d_fn(name: &str) -> bool {
    INTERNAL_BUFFER_2D_FUNCTION_NAMES.contains(&name)
}

pub fn is_builtin_buffer_write_function_name(name: &str) -> bool {
    matches!(
        name,
        INTERNAL_BUFFER_WRITE2_FN
            | INTERNAL_BUFFER_WRITE3_FN
            | INTERNAL_BUFFER_WRITE_CHANNEL_FN
            | WRITE_UNSAFE_FN
    )
}

/// Internal indexing operations that may also use receiver syntax. These stay
/// outside the public builtin registry because their signature is resolved
/// from the receiver's storage shape during semantic analysis.
pub(crate) fn is_unsafe_index_method_name(name: &str) -> bool {
    matches!(name, READ_UNSAFE_FN | WRITE_UNSAFE_FN)
}

pub const ARRAY_LEN_METHOD: &str = "len";
pub const BUFFER_BOUND_METHOD: &str = "bound";
pub const BUFFER_CHANS_METHOD: &str = "chans";
pub const BUFFER_SAMPLERATE_METHOD: &str = "samplerate";

pub const BUILTIN_INSTANCE_METHOD_NAMES: &[&str] = &[
    ARRAY_LEN_METHOD,
    BUFFER_BOUND_METHOD,
    BUFFER_CHANS_METHOD,
    BUFFER_SAMPLERATE_METHOD,
];

pub fn builtin_instance_method_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_INSTANCE_METHOD_NAMES.iter().copied()
}

pub fn is_builtin_instance_method_name(name: &str) -> bool {
    BUILTIN_INSTANCE_METHOD_NAMES.contains(&name)
}

pub(crate) fn builtin_instance_method_return_type(name: &str) -> Option<PrimitiveType> {
    match name {
        ARRAY_LEN_METHOD | BUFFER_CHANS_METHOD => Some(PrimitiveType::I32),
        BUFFER_BOUND_METHOD => Some(PrimitiveType::Bool),
        BUFFER_SAMPLERATE_METHOD => Some(PrimitiveType::F32),
        _ => None,
    }
}

fn split_instance_method_path(name: &str) -> Option<(&str, &str)> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method.is_empty() {
        return None;
    }
    Some((base, method))
}

pub fn parse_array_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == ARRAY_LEN_METHOD {
        Some(base)
    } else {
        None
    }
}

pub fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == BUFFER_CHANS_METHOD {
        Some(base)
    } else {
        None
    }
}

pub fn parse_buffer_bound_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == BUFFER_BOUND_METHOD {
        Some(base)
    } else {
        None
    }
}

pub fn parse_buffer_samplerate_instance_base(name: &str) -> Option<&str> {
    let (base, method) = split_instance_method_path(name)?;
    if method == BUFFER_SAMPLERATE_METHOD {
        Some(base)
    } else {
        None
    }
}

pub(crate) fn builtin_arity(func: BuiltinFn) -> usize {
    func.arity()
}

pub(crate) fn builtin_name(func: BuiltinFn) -> &'static str {
    func.name()
}

pub(crate) fn is_float_type(ty: PrimitiveType) -> bool {
    matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
}

pub(crate) fn builtin_constant_value_f64(name: &str, options: AnalysisOptions) -> Option<f64> {
    match builtin_constant(name)?.value {
        BuiltinConstantValue::Pi => Some(std::f64::consts::PI),
        BuiltinConstantValue::TwoPi => Some(2.0 * std::f64::consts::PI),
        BuiltinConstantValue::SampleRate | BuiltinConstantValue::HostSampleRate => {
            Some(options.sample_rate as f64)
        }
        BuiltinConstantValue::BlockSize => Some(options.block_size as f64),
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

fn merge_const_numeric_types(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) | (F32, I64) | (I64, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

pub(crate) fn infer_const_expr_type(
    expr: &Expr,
    _options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    expr.try_fold(
        |node, children| match node {
            Expr::UnaryBitNot { .. } | Expr::Binary { .. } => node.children(children),
            _ => {}
        },
        |node, children| {
            Ok::<_, ()>(match node {
                Expr::Number { .. } => PrimitiveType::F64,
                Expr::Int { .. } => PrimitiveType::I64,
                Expr::Bool { .. } => PrimitiveType::Bool,
                Expr::Var { name, .. } => builtin_constant_type(name).ok_or_else(|| {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context} uses non-constant symbol '{name}'"),
                        node.loc(),
                    ));
                })?,
                Expr::Cast { to, .. } => *to,
                Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
                    PrimitiveType::Bool
                }
                Expr::UnaryBitNot { expr, .. } => {
                    let inner = children.next().expect("bitwise-not operand type");
                    merge_const_integer_types(inner, inner).ok_or_else(|| {
                        errors.push(Diagnostic::semantic_span(
                            format!(
                                "{context} bitwise not requires integer operand, got {:?}",
                                inner
                            ),
                            expr.loc(),
                        ));
                    })?
                }
                Expr::Binary { op, .. } => {
                    let lhs_ty = children.next().expect("left binary operand type");
                    let rhs_ty = children.next().expect("right binary operand type");
                    match op {
                        BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                        | BinaryOp::ShiftLeft
                        | BinaryOp::ShiftRight => {
                            merge_const_integer_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                errors.push(Diagnostic::semantic_span(
                                    format!(
                                        "{context} bitwise expression requires integer operands, got {:?} and {:?}",
                                        lhs_ty, rhs_ty
                                    ),
                                    node.loc(),
                                ));
                            })?
                        }
                        _ => {
                            merge_const_numeric_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                errors.push(Diagnostic::semantic_span(
                                    format!(
                                        "{context} requires numeric operands, got {:?} and {:?}",
                                        lhs_ty, rhs_ty
                                    ),
                                    node.loc(),
                                ));
                            })?
                        }
                    }
                }
                _ => {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context} must be a compile-time constant expression"),
                        node.loc(),
                    ));
                    return Err(());
                }
            })
        },
    )
    .ok()
}

fn fold_const_expr_exactness(expr: &Expr, mut on_exact: impl FnMut(&Expr)) -> bool {
    expr.try_fold(
        Expr::children,
        |node, children| -> Result<bool, std::convert::Infallible> {
            let mut child = || children.next().expect("const expression exactness child");
            let exact = match node {
                Expr::Int { .. } | Expr::Bool { .. } => true,
                Expr::Var { name, .. } => matches!(
                    builtin_constant_type(name),
                    Some(PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool)
                ),
                Expr::Cast { to, .. } => {
                    matches!(
                        to,
                        PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool
                    ) && child()
                }
                Expr::UnaryNot { .. } | Expr::UnaryBitNot { .. } => child(),
                Expr::Logical { .. } | Expr::Compare { .. } | Expr::Binary { .. } => {
                    child() && child()
                }
                _ => false,
            };
            if exact {
                on_exact(node);
            }
            Ok(exact)
        },
    )
    .unwrap()
}

pub(crate) fn can_eval_const_expr_exact_int(expr: &Expr) -> bool {
    fold_const_expr_exactness(expr, |_| {})
}

pub(crate) fn exact_const_expr_nodes(expr: &Expr) -> HashSet<*const Expr> {
    let mut nodes = HashSet::new();
    fold_const_expr_exactness(expr, |expr| {
        nodes.insert(expr as *const Expr);
    });
    nodes
}

pub(crate) fn eval_const_expr_i64_exact(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<i64> {
    type Eval = Result<(i64, PrimitiveType), Diagnostic>;
    let result = expr
        .try_fold(
            Expr::children,
            |node, children| -> Result<Eval, std::convert::Infallible> {
                let mut child = || children.next().expect("integer constant expression child");
                let diagnostic = |message| Diagnostic::semantic_span(message, node.loc());
                Ok((|| -> Eval {
                    match node {
                    Expr::Int { value, .. } => Ok((*value, PrimitiveType::I64)),
                    Expr::Bool { value, .. } => {
                        Ok((i64::from(*value), PrimitiveType::Bool))
                    }
                    Expr::Var { name, .. } => match builtin_constant_type(name) {
                        Some(PrimitiveType::I32) => {
                            Ok((options.block_size as i64, PrimitiveType::I32))
                        }
                        Some(ty @ (PrimitiveType::I64 | PrimitiveType::Bool)) => {
                            builtin_constant_value_f64(name, options)
                                .map(|value| (value as i64, ty))
                                .ok_or_else(|| {
                                    diagnostic(format!(
                                        "{context} uses non-constant symbol '{name}'"
                                    ))
                                })
                        }
                        _ => Err(diagnostic(format!(
                            "{context} uses non-constant symbol '{name}'"
                        ))),
                    },
                    Expr::Cast { to, expr, .. } => {
                        let (value, _) = child()?;
                        match to {
                            PrimitiveType::I32 => Ok(((value as i32) as i64, *to)),
                            PrimitiveType::I64 => Ok((value, *to)),
                            PrimitiveType::Bool => Ok((i64::from(value != 0), *to)),
                            _ => Err(Diagnostic::semantic_span(
                                format!(
                                    "{context} must evaluate to an integer constant expression"
                                ),
                                expr.loc(),
                            )),
                        }
                    }
                    Expr::UnaryNot { .. } => {
                        let (value, _) = child()?;
                        Ok((i64::from(value == 0), PrimitiveType::Bool))
                    }
                    Expr::UnaryBitNot { expr, .. } => {
                        let (value, ty) = child()?;
                        match ty {
                            PrimitiveType::I32 => Ok(((!(value as i32)) as i64, ty)),
                            PrimitiveType::I64 => Ok((!value, ty)),
                            _ => Err(Diagnostic::semantic_span(
                                format!(
                                    "{context} bitwise not requires integer operand, got {:?}",
                                    ty
                                ),
                                expr.loc(),
                            )),
                        }
                    }
                    Expr::Logical { op, .. } => {
                        let lhs = child();
                        let rhs = child();
                        let (lhs, _) = lhs?;
                        let value = match op {
                            LogicalOp::And if lhs == 0 => false,
                            LogicalOp::Or if lhs != 0 => true,
                            _ => rhs?.0 != 0,
                        };
                        Ok((i64::from(value), PrimitiveType::Bool))
                    }
                    Expr::Compare { op, .. } => {
                        let (lhs, _) = child()?;
                        let (rhs, _) = child()?;
                        let value = match op {
                            CmpOp::Eq => lhs == rhs,
                            CmpOp::Ne => lhs != rhs,
                            CmpOp::Lt => lhs < rhs,
                            CmpOp::Le => lhs <= rhs,
                            CmpOp::Gt => lhs > rhs,
                            CmpOp::Ge => lhs >= rhs,
                        };
                        Ok((i64::from(value), PrimitiveType::Bool))
                    }
                    Expr::Binary { op, .. } => {
                        let (lhs, lhs_ty) = child()?;
                        let (rhs, rhs_ty) = child()?;
                        let bitwise = matches!(
                            op,
                            BinaryOp::BitAnd
                                | BinaryOp::BitOr
                                | BinaryOp::BitXor
                                | BinaryOp::ShiftLeft
                                | BinaryOp::ShiftRight
                        );
                        let ty = if bitwise {
                            merge_const_integer_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                diagnostic(format!(
                                    "{context} bitwise expression requires integer operands, got {:?} and {:?}",
                                    lhs_ty, rhs_ty
                                ))
                            })?
                        } else {
                            merge_const_numeric_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                diagnostic(format!(
                                    "{context} requires numeric operands, got {:?} and {:?}",
                                    lhs_ty, rhs_ty
                                ))
                            })?
                        };
                        if !matches!(ty, PrimitiveType::I32 | PrimitiveType::I64) {
                            return Err(diagnostic(format!(
                                "{context} must evaluate to an integer constant expression, got {:?}",
                                ty
                            )));
                        }
                        let zero_error = |operation| {
                            diagnostic(format!("{context} {operation} by zero"))
                        };
                        let value = match ty {
                            PrimitiveType::I32 => {
                                let lhs = lhs as i32;
                                let rhs = rhs as i32;
                                match op {
                                    BinaryOp::Add => lhs.wrapping_add(rhs) as i64,
                                    BinaryOp::Sub => lhs.wrapping_sub(rhs) as i64,
                                    BinaryOp::Mul => lhs.wrapping_mul(rhs) as i64,
                                    BinaryOp::Div => {
                                        if rhs == 0 {
                                            return Err(zero_error("division"));
                                        }
                                        lhs.wrapping_div(rhs) as i64
                                    }
                                    BinaryOp::Mod => (rhs != 0)
                                        .then(|| lhs.wrapping_rem(rhs) as i64)
                                        .ok_or_else(|| zero_error("modulo"))?,
                                    BinaryOp::BitAnd => (lhs & rhs) as i64,
                                    BinaryOp::BitOr => (lhs | rhs) as i64,
                                    BinaryOp::BitXor => (lhs ^ rhs) as i64,
                                    BinaryOp::ShiftLeft => lhs.wrapping_shl(rhs as u32) as i64,
                                    BinaryOp::ShiftRight => lhs.wrapping_shr(rhs as u32) as i64,
                                }
                            }
                            PrimitiveType::I64 => match op {
                                BinaryOp::Add => lhs.wrapping_add(rhs),
                                BinaryOp::Sub => lhs.wrapping_sub(rhs),
                                BinaryOp::Mul => lhs.wrapping_mul(rhs),
                                BinaryOp::Div => (rhs != 0)
                                    .then(|| lhs.wrapping_div(rhs))
                                    .ok_or_else(|| zero_error("division"))?,
                                BinaryOp::Mod => (rhs != 0)
                                    .then(|| lhs.wrapping_rem(rhs))
                                    .ok_or_else(|| zero_error("modulo"))?,
                                BinaryOp::BitAnd => lhs & rhs,
                                BinaryOp::BitOr => lhs | rhs,
                                BinaryOp::BitXor => lhs ^ rhs,
                                BinaryOp::ShiftLeft => lhs.wrapping_shl(rhs as u32),
                                BinaryOp::ShiftRight => lhs.wrapping_shr(rhs as u32),
                            },
                            _ => unreachable!(),
                        };
                        Ok((value, ty))
                    }
                        _ => Err(diagnostic(format!(
                            "{context} must be a compile-time integer constant expression"
                        ))),
                    }
                })())
            },
        )
        .unwrap();
    match result {
        Ok((value, _)) => Some(value),
        Err(error) => {
            errors.push(error);
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
    type Eval = Result<(f64, PrimitiveType), Diagnostic>;
    let result = expr
        .try_fold(
            Expr::children,
            |node, children| -> Result<Eval, std::convert::Infallible> {
                let mut child = || children.next().expect("constant expression child");
                let diagnostic = |message| Diagnostic::semantic_span(message, node.loc());
                Ok((|| -> Eval {
                    match node {
                        Expr::Number { value, .. } => Ok((*value, PrimitiveType::F64)),
                        Expr::Int { value, .. } => Ok((*value as f64, PrimitiveType::I64)),
                        Expr::Bool { value, .. } => {
                            Ok((if *value { 1.0 } else { 0.0 }, PrimitiveType::Bool))
                        }
                        Expr::Var { name, .. } => builtin_constant_value_f64(name, options)
                            .zip(builtin_constant_type(name))
                            .ok_or_else(|| {
                                diagnostic(format!(
                                    "{context} uses non-constant symbol '{name}'"
                                ))
                            }),
                        Expr::Cast { to, .. } => {
                            let (value, _) = child()?;
                            Ok((
                                match to {
                                    PrimitiveType::F32 | PrimitiveType::F64 => value,
                                    PrimitiveType::I32 => (value as i32) as f64,
                                    PrimitiveType::I64 => (value as i64) as f64,
                                    PrimitiveType::Bool => {
                                        if value != 0.0 { 1.0 } else { 0.0 }
                                    }
                                },
                                *to,
                            ))
                        }
                        Expr::UnaryNot { .. } => {
                            let (value, _) = child()?;
                            Ok((
                                if value == 0.0 { 1.0 } else { 0.0 },
                                PrimitiveType::Bool,
                            ))
                        }
                        Expr::UnaryBitNot { expr, .. } => {
                            let (value, ty) = child()?;
                            match ty {
                                PrimitiveType::I32 => Ok(((!(value as i32)) as f64, ty)),
                                PrimitiveType::I64 => Ok(((!(value as i64)) as f64, ty)),
                                _ => Err(Diagnostic::semantic_span(
                                    format!(
                                        "{context} bitwise not requires integer operand, got {:?}",
                                        ty
                                    ),
                                    expr.loc(),
                                )),
                            }
                        }
                        Expr::Logical { op, .. } => {
                            let lhs = child();
                            let rhs = child();
                            let (lhs, _) = lhs?;
                            let value = match op {
                                LogicalOp::And if lhs == 0.0 => false,
                                LogicalOp::Or if lhs != 0.0 => true,
                                _ => rhs?.0 != 0.0,
                            };
                            Ok((
                                if value { 1.0 } else { 0.0 },
                                PrimitiveType::Bool,
                            ))
                        }
                        Expr::Compare { op, .. } => {
                            let (lhs, _) = child()?;
                            let (rhs, _) = child()?;
                            let value = match op {
                                CmpOp::Eq => lhs == rhs,
                                CmpOp::Ne => lhs != rhs,
                                CmpOp::Lt => lhs < rhs,
                                CmpOp::Le => lhs <= rhs,
                                CmpOp::Gt => lhs > rhs,
                                CmpOp::Ge => lhs >= rhs,
                            };
                            Ok((
                                if value { 1.0 } else { 0.0 },
                                PrimitiveType::Bool,
                            ))
                        }
                        Expr::Binary { op, .. } => {
                            let (lhs, lhs_ty) = child()?;
                            let (rhs, rhs_ty) = child()?;
                            let bitwise = matches!(
                                op,
                                BinaryOp::BitAnd
                                    | BinaryOp::BitOr
                                    | BinaryOp::BitXor
                                    | BinaryOp::ShiftLeft
                                    | BinaryOp::ShiftRight
                            );
                            let ty = if bitwise {
                                merge_const_integer_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                    diagnostic(format!(
                                        "{context} bitwise expression requires integer operands, got {:?} and {:?}",
                                        lhs_ty, rhs_ty
                                    ))
                                })?
                            } else {
                                merge_const_numeric_types(lhs_ty, rhs_ty).ok_or_else(|| {
                                    diagnostic(format!(
                                        "{context} requires numeric operands, got {:?} and {:?}",
                                        lhs_ty, rhs_ty
                                    ))
                                })?
                            };
                            let value = match op {
                                BinaryOp::Add => lhs + rhs,
                                BinaryOp::Sub => lhs - rhs,
                                BinaryOp::Mul => lhs * rhs,
                                BinaryOp::Div => lhs / rhs,
                                BinaryOp::Mod => lhs % rhs,
                                BinaryOp::BitAnd if ty == PrimitiveType::I32 => {
                                    ((lhs as i32) & (rhs as i32)) as f64
                                }
                                BinaryOp::BitAnd => ((lhs as i64) & (rhs as i64)) as f64,
                                BinaryOp::BitOr if ty == PrimitiveType::I32 => {
                                    ((lhs as i32) | (rhs as i32)) as f64
                                }
                                BinaryOp::BitOr => ((lhs as i64) | (rhs as i64)) as f64,
                                BinaryOp::BitXor if ty == PrimitiveType::I32 => {
                                    ((lhs as i32) ^ (rhs as i32)) as f64
                                }
                                BinaryOp::BitXor => ((lhs as i64) ^ (rhs as i64)) as f64,
                                BinaryOp::ShiftLeft if ty == PrimitiveType::I32 => {
                                    (lhs as i32).wrapping_shl(rhs as u32) as f64
                                }
                                BinaryOp::ShiftLeft => {
                                    (lhs as i64).wrapping_shl(rhs as u32) as f64
                                }
                                BinaryOp::ShiftRight if ty == PrimitiveType::I32 => {
                                    (lhs as i32).wrapping_shr(rhs as u32) as f64
                                }
                                BinaryOp::ShiftRight => {
                                    (lhs as i64).wrapping_shr(rhs as u32) as f64
                                }
                            };
                            Ok((value, ty))
                        }
                        _ => Err(diagnostic(format!(
                            "{context} must be a compile-time constant expression"
                        ))),
                    }
                })())
            },
        )
        .unwrap();
    match result {
        Ok((value, _)) => Some(value),
        Err(error) => {
            errors.push(error);
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

pub(crate) const fn primitive_storage_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

pub(crate) const fn max_buffer_static_channels(ty: PrimitiveType) -> usize {
    (i32::MAX as usize) / primitive_storage_bytes(ty)
}

pub(crate) fn validate_buffer_static_channels(
    channels: usize,
    elem_ty: PrimitiveType,
    context: &str,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let maximum = max_buffer_static_channels(elem_ty);
    if channels <= maximum {
        return true;
    }
    errors.push(Diagnostic::semantic_span(
        format!(
            "{context} exceeds the signed i32 buffer byte-extent limit for {} elements; maximum is {maximum}",
            elem_ty.name()
        ),
        loc,
    ));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_index_operations_are_internal_receiver_methods() {
        for name in [
            "read_unsafe",
            "write_unsafe",
            "read_unsafe2",
            "write_unsafe2",
        ] {
            assert!(!is_builtin_function_name(name));
            assert!(!is_builtin_instance_method_name(name));
            assert!(!public_builtin_function_names().any(|builtin| builtin == name));
        }
        assert!(is_unsafe_index_method_name("read_unsafe"));
        assert!(is_unsafe_index_method_name("write_unsafe"));
        assert!(!is_unsafe_index_method_name("read_unsafe2"));
        assert!(!is_unsafe_index_method_name("write_unsafe2"));
    }

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

    #[test]
    fn deep_constant_evaluators_use_heap_backed_traversal() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut integer = Expr::int(0);
                let mut float = Expr::number(0.0);
                for _ in 0..15_000 {
                    integer = Expr::Binary {
                        loc: Default::default(),
                        op: BinaryOp::Add,
                        lhs: Box::new(integer),
                        rhs: Box::new(Expr::int(1)),
                    };
                    float = Expr::Binary {
                        loc: Default::default(),
                        op: BinaryOp::Add,
                        lhs: Box::new(float),
                        rhs: Box::new(Expr::number(0.5)),
                    };
                }

                let mut errors = Vec::new();
                assert_eq!(
                    eval_const_expr_i64_exact(
                        &integer,
                        AnalysisOptions::default(),
                        "deep integer const",
                        &mut errors,
                    ),
                    Some(15_000)
                );
                assert_eq!(
                    eval_const_expr_f64(
                        &float,
                        AnalysisOptions::default(),
                        "deep float const",
                        &mut errors,
                    ),
                    Some(7_500.0)
                );
                assert!(errors.is_empty(), "unexpected diagnostics: {errors:?}");
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn constant_evaluators_do_not_observe_short_circuited_errors() {
        let expr = Expr::Logical {
            loc: Default::default(),
            op: LogicalOp::And,
            lhs: Box::new(Expr::Bool {
                loc: Default::default(),
                value: false,
            }),
            rhs: Box::new(Expr::var("not_a_constant")),
        };
        let mut errors = Vec::new();
        assert_eq!(
            eval_const_expr_i64_exact(
                &expr,
                AnalysisOptions::default(),
                "short circuit",
                &mut errors,
            ),
            Some(0)
        );
        assert_eq!(
            eval_const_expr_f64(
                &expr,
                AnalysisOptions::default(),
                "short circuit",
                &mut errors,
            ),
            Some(0.0)
        );
        assert!(errors.is_empty());
    }
}
