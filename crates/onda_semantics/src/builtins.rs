use onda_frontend::{
    BinaryOp, BuiltinFn, Diagnostic, Expr, PrimitiveType, SourceLoc, INTERNAL_BUFFER_READ2_FN,
    INTERNAL_BUFFER_READ3_FN, INTERNAL_BUFFER_READ_CHANNEL_FN, INTERNAL_BUFFER_WRITE2_FN,
    INTERNAL_BUFFER_WRITE3_FN, INTERNAL_BUFFER_WRITE_CHANNEL_FN, READ_UNSAFE_FN, WRITE_UNSAFE_FN,
};

use crate::const_scalar::eval_const_scalar;
use crate::expr_typing::merge_numeric_types_without_diagnostics as merge_const_numeric_types;
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

/// Types materialized scalar expressions using the ordinary arithmetic and
/// intrinsic rules. Const references and calls are resolved before this check.
pub(crate) fn infer_const_expr_type(
    expr: &Expr,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<PrimitiveType> {
    expr.try_fold(
        |node, children| match node {
            Expr::UnaryBitNot { .. } | Expr::Binary { .. } | Expr::Call { .. } => node.children(children),
            _ => {}
        },
        |node, children| {
            let pure = match node {
                Expr::Number { .. } | Expr::Int { .. } | Expr::Var { .. } => true,
                Expr::UnaryBitNot { .. } | Expr::Binary { .. } | Expr::Call { .. } =>
                    children.as_slice().iter().all(|(_, pure)| *pure),
                _ => false,
            };
            let ty = match node {
                Expr::Number { .. } => PrimitiveType::F64,
                Expr::Int { .. } => PrimitiveType::I64,
                Expr::Bool { .. } => PrimitiveType::Bool,
                Expr::Var { name, .. } => builtin_constant_type(name).ok_or_else(|| {
                    errors.push(Diagnostic::semantic_span(
                        format!("{context} uses non-constant symbol '{name}'"),
                        node.loc(),
                    ));
                })?,
                Expr::Index { base, .. } => {
                    errors.push(Diagnostic::semantic_span(format!("{context} uses non-constant array '{base}'"), node.loc()));
                    return Err(());
                }
                Expr::UserCall { name, .. } => {
                    errors.push(Diagnostic::semantic_span(format!("{context} uses unknown const def '{name}'"), node.loc()));
                    return Err(());
                }
                Expr::Call { func, .. } => {
                    let (types, pure): (Vec<_>, Vec<_>) = children.unzip();
                    let types = crate::expr_typing::adapt_numeric_argument_types_from_purity(&types, &pure);
                    crate::intrinsic_result_type(*func, &types).ok_or_else(|| {
                        errors.push(Diagnostic::semantic_span(format!("{context}: builtin '{}' has incompatible argument types", builtin_name(*func)), node.loc()));
                    })?
                }
                Expr::Cast { to, .. } => *to,
                Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
                    PrimitiveType::Bool
                }
                Expr::UnaryBitNot { expr, .. } => {
                    let (inner, _) = children.next().expect("bitwise-not operand type");
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
                    let (lhs_ty, lhs_pure) = children.next().expect("left binary operand type");
                    let (rhs_ty, rhs_pure) = children.next().expect("right binary operand type");
                    let (lhs_ty, rhs_ty) = crate::expr_typing::adapt_binary_types_from_purity(lhs_ty, rhs_ty, lhs_pure, rhs_pure);
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
            };
            Ok::<_, ()>((ty, pure))
        },
    )
    .ok()
    .map(|(ty, _)| ty)
}

fn fold_const_expr_exactness(
    expr: &Expr,
    constants: Option<&crate::decl_symbols::DeclaredSymbolMap>,
) -> (bool, bool) {
    // Keep constant availability separate from integer results: an explicit
    // integer cast may consume a constant floating expression without losing
    // its integer result type or exact arithmetic in the surrounding tree.
    expr.try_fold(
        Expr::children,
        |node, children| -> Result<(bool, bool), std::convert::Infallible> {
            let mut child = || children.next().expect("const expression exactness child");
            let (constant, exact) = match node {
                Expr::Int { .. } | Expr::Bool { .. } => (true, true),
                Expr::Number { .. } => (true, false),
                Expr::Var { name, .. } => {
                    let ty = builtin_constant_type(name).or_else(|| {
                        match constants.and_then(|symbols| symbols.get(name)) {
                            Some(crate::decl_symbols::DeclaredSymbolInfo::Constant {
                                ty, ..
                            }) => Some(*ty),
                            _ => None,
                        }
                    });
                    (
                        ty.is_some(),
                        matches!(
                            ty,
                            Some(PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool)
                        ),
                    )
                }
                Expr::Index { base, .. } => {
                    let (_, index_exact) = child();
                    let ty = match constants.and_then(|symbols| symbols.get(base)) {
                        Some(crate::decl_symbols::DeclaredSymbolInfo::ConstArray { elem_ty }) => {
                            Some(*elem_ty)
                        }
                        _ => None,
                    };
                    (
                        ty.is_some() && index_exact,
                        matches!(
                            ty,
                            Some(PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool)
                        ) && index_exact,
                    )
                }
                Expr::Cast { to, .. } => {
                    let (constant, _) = child();
                    (
                        constant,
                        constant
                            && matches!(
                                to,
                                PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::Bool
                            ),
                    )
                }
                Expr::UnaryNot { .. } | Expr::UnaryBitNot { .. } => child(),
                Expr::Logical { .. } | Expr::Compare { .. } | Expr::Binary { .. } => {
                    let (lhs_const, lhs_exact) = child();
                    let (rhs_const, rhs_exact) = child();
                    (lhs_const && rhs_const, lhs_exact && rhs_exact)
                }
                Expr::Call { func, .. } => {
                    let (constant, integers) = children.fold(
                        (true, true),
                        |(constant, integers), (arg_const, arg_int)| {
                            (constant && arg_const, integers && arg_int)
                        },
                    );
                    (
                        constant,
                        integers
                            && matches!(
                                func,
                                BuiltinFn::Abs
                                    | BuiltinFn::Min
                                    | BuiltinFn::Max
                                    | BuiltinFn::RangeClamp
                                    | BuiltinFn::RangeWrap
                            ),
                    )
                }
                _ => (false, false),
            };
            Ok((constant, exact))
        },
    )
    .unwrap()
}

pub(crate) fn can_eval_const_expr_exact_int(expr: &Expr) -> bool {
    fold_const_expr_exactness(expr, None).1
}

pub(crate) fn can_eval_const_expr_with_symbols(
    expr: &Expr,
    constants: &crate::decl_symbols::DeclaredSymbolMap,
) -> bool {
    fold_const_expr_exactness(expr, Some(constants)).0
}

/// Proves the same exact integer forms before deferred constants have values.
pub(crate) fn can_eval_const_expr_exact_int_with_symbols(
    expr: &Expr,
    constants: &crate::decl_symbols::DeclaredSymbolMap,
) -> bool {
    fold_const_expr_exactness(expr, Some(constants)).1
}

pub(crate) fn eval_const_expr_i64_exact(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<i64> {
    let value = eval_const_scalar(expr, None, options, context, errors)?;
    match value {
        crate::TypedConstValue::I32(value) => Some(i64::from(value)),
        crate::TypedConstValue::I64(value) => Some(value),
        crate::TypedConstValue::Bool(value) => Some(i64::from(value)),
        _ => {
            errors.push(Diagnostic::semantic_span(
                format!("{context} must evaluate to an integer constant expression"),
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
    eval_const_scalar(expr, None, options, context, errors).map(crate::TypedConstValue::to_f64)
}

/// Const slices accept exact integers and finite, integral floating bounds.
/// All bounds convert to i32 before normalization, as in runtime lowering.
pub(crate) fn eval_const_slice_integer(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<i64> {
    let value = eval_const_scalar(expr, None, options, context, errors)?;
    if matches!(
        value,
        crate::TypedConstValue::F32(_) | crate::TypedConstValue::F64(_)
    ) {
        let float = value.to_f64();
        if !float.is_finite() {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression must be finite"),
                expr.loc(),
            ));
            return None;
        }
        if (float - float.round()).abs() > 1e-6 {
            errors.push(Diagnostic::semantic_span(
                format!("{context}: expression is not a compile-time integer"),
                expr.loc(),
            ));
            return None;
        }
    }
    coerce_slice_integer(value).or_else(|| {
        errors.push(Diagnostic::semantic_span(
            format!("{context}: slice bound requires numeric type"),
            expr.loc(),
        ));
        None
    })
}

pub(crate) fn coerce_slice_integer(value: crate::TypedConstValue) -> Option<i64> {
    let onda_mir::ScalarValue::I32(value) = onda_mir::constant_eval::cast(
        crate::mir_scalar::mir_scalar(value),
        onda_mir::ScalarType::I32,
    )?
    else {
        unreachable!()
    };
    Some(i64::from(value))
}

pub(crate) fn eval_const_bool_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<bool> {
    let value = eval_const_scalar(expr, None, options, context, errors)?;
    if let crate::TypedConstValue::Bool(value) = value {
        Some(value)
    } else {
        errors.push(Diagnostic::semantic_span(
            format!(
                "{context} must evaluate to a compile-time bool, got {:?}",
                value.primitive_type()
            ),
            expr.loc(),
        ));
        None
    }
}

pub(crate) fn eval_data_size_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    if matches!(
        expr,
        Expr::Bool { .. }
            | Expr::Compare { .. }
            | Expr::Logical { .. }
            | Expr::UnaryNot { .. }
            | Expr::Cast {
                to: PrimitiveType::Bool,
                ..
            }
    ) {
        errors.push(Diagnostic::semantic_span(
            format!("{context} requires an integer size, got Bool"),
            expr.loc(),
        ));
        return None;
    }
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

/// Array lengths and indices use i32, including lengths read only as metadata.
/// Zero is valid for a slice; storage-size evaluation checks positivity first.
pub(crate) fn checked_array_length(
    len: usize,
    context: &str,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> Option<i32> {
    i32::try_from(len).ok().or_else(|| {
        errors.push(Diagnostic::semantic_span(
            format!("{context} exceeds i32::MAX array length"),
            loc,
        ));
        None
    })
}

pub(crate) fn eval_array_size_expr(
    expr: &Expr,
    options: AnalysisOptions,
    context: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let len = eval_data_size_expr(expr, options, context, errors)?;
    checked_array_length(len, context, expr.loc(), errors)?;
    Some(len)
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
    use onda_frontend::LogicalOp;

    #[test]
    fn deeply_nested_literal_builtins_infer_and_evaluate_on_a_worker_stack() {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let mut expr = Expr::Binary {
                    loc: Default::default(),
                    op: BinaryOp::Add,
                    lhs: Box::new(Expr::int(i64::from(i32::MAX))),
                    rhs: Box::new(Expr::int(1)),
                };
                for _ in 0..15_000 {
                    expr = Expr::Call {
                        loc: Default::default(),
                        func: BuiltinFn::Abs,
                        args: vec![expr],
                    };
                }
                let mut visits = 0;
                let literals = crate::expr_typing::scalar_const_kinds(&expr, |node, children| {
                    visits += 1;
                    node.children(children);
                });
                let mut errors = Vec::new();
                let inferred = infer_const_expr_type(&expr, "deep literals", &mut errors);
                let evaluated = eval_const_scalar(
                    &expr,
                    Some(PrimitiveType::I32),
                    AnalysisOptions::default(),
                    "deep literals",
                    &mut errors,
                );
                // This synthetic tree is much deeper than parsed builtin calls.
                // Release its owned argument vectors without recursive AST drop.
                while let Expr::Call { mut args, .. } = expr {
                    expr = args.pop().unwrap();
                }
                assert_eq!(visits, 15_003);
                assert_eq!(literals.len(), visits);
                assert_eq!(inferred, Some(PrimitiveType::I64));
                assert_eq!(evaluated, Some(crate::TypedConstValue::I32(i32::MIN)));
                assert!(errors.is_empty(), "{errors:?}");
            })
            .unwrap()
            .join()
            .unwrap();
    }

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
