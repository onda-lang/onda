//! Scalar representations and operators at the AST/MIR boundary.
use crate::TypedConstValue;
use onda_frontend::{BinaryOp as AstBinaryOp, BuiltinFn, CmpOp, PrimitiveType};
use onda_mir::{BinaryOp as MirBinaryOp, CompareOp, Intrinsic, ScalarType, ScalarValue};

pub(crate) fn mir_scalar(value: TypedConstValue) -> ScalarValue {
    match value {
        TypedConstValue::F32(value) => ScalarValue::F32(value),
        TypedConstValue::F64(value) => ScalarValue::F64(value),
        TypedConstValue::I32(value) => ScalarValue::I32(value),
        TypedConstValue::I64(value) => ScalarValue::I64(value),
        TypedConstValue::Bool(value) => ScalarValue::Bool(value),
    }
}

pub(crate) fn scalar_type(ty: PrimitiveType) -> ScalarType {
    match ty {
        PrimitiveType::F32 => ScalarType::F32,
        PrimitiveType::F64 => ScalarType::F64,
        PrimitiveType::I32 => ScalarType::I32,
        PrimitiveType::I64 => ScalarType::I64,
        PrimitiveType::Bool => ScalarType::Bool,
    }
}

pub(crate) fn source_scalar_type(ty: ScalarType) -> PrimitiveType {
    match ty {
        ScalarType::F32 => PrimitiveType::F32,
        ScalarType::F64 => PrimitiveType::F64,
        ScalarType::I32 => PrimitiveType::I32,
        ScalarType::I64 => PrimitiveType::I64,
        ScalarType::Bool => PrimitiveType::Bool,
    }
}

pub(crate) fn map_binary(op: AstBinaryOp) -> MirBinaryOp {
    match op {
        AstBinaryOp::Add => MirBinaryOp::Add,
        AstBinaryOp::Sub => MirBinaryOp::Subtract,
        AstBinaryOp::Mul => MirBinaryOp::Multiply,
        AstBinaryOp::Div => MirBinaryOp::Divide,
        AstBinaryOp::Mod => MirBinaryOp::Remainder,
        AstBinaryOp::BitAnd => MirBinaryOp::BitAnd,
        AstBinaryOp::BitOr => MirBinaryOp::BitOr,
        AstBinaryOp::BitXor => MirBinaryOp::BitXor,
        AstBinaryOp::ShiftLeft => MirBinaryOp::ShiftLeft,
        AstBinaryOp::ShiftRight => MirBinaryOp::ShiftRight,
    }
}

pub(crate) fn map_compare(op: CmpOp) -> CompareOp {
    match op {
        CmpOp::Eq => CompareOp::Equal,
        CmpOp::Ne => CompareOp::NotEqual,
        CmpOp::Lt => CompareOp::Less,
        CmpOp::Le => CompareOp::LessEqual,
        CmpOp::Gt => CompareOp::Greater,
        CmpOp::Ge => CompareOp::GreaterEqual,
    }
}

pub(crate) fn typed_scalar(value: ScalarValue) -> TypedConstValue {
    match value {
        ScalarValue::F32(value) => TypedConstValue::F32(value),
        ScalarValue::F64(value) => TypedConstValue::F64(value),
        ScalarValue::I32(value) => TypedConstValue::I32(value),
        ScalarValue::I64(value) => TypedConstValue::I64(value),
        ScalarValue::Bool(value) => TypedConstValue::Bool(value),
    }
}

pub(crate) fn map_intrinsic(function: BuiltinFn) -> Option<Intrinsic> {
    Some(match function {
        BuiltinFn::Sin => Intrinsic::Sin,
        BuiltinFn::Cos => Intrinsic::Cos,
        BuiltinFn::Tan => Intrinsic::Tan,
        BuiltinFn::Tanh => Intrinsic::Tanh,
        BuiltinFn::Atan => Intrinsic::Atan,
        BuiltinFn::Atan2 => Intrinsic::Atan2,
        BuiltinFn::Exp => Intrinsic::Exp,
        BuiltinFn::Log => Intrinsic::Log,
        BuiltinFn::Sqrt => Intrinsic::Sqrt,
        BuiltinFn::Pow => Intrinsic::Pow,
        BuiltinFn::Abs => Intrinsic::Abs,
        BuiltinFn::Floor => Intrinsic::Floor,
        BuiltinFn::Ceil => Intrinsic::Ceil,
        BuiltinFn::Round => Intrinsic::Round,
        BuiltinFn::Trunc => Intrinsic::Trunc,
        BuiltinFn::Min => Intrinsic::Min,
        BuiltinFn::Max => Intrinsic::Max,
        BuiltinFn::Fma => Intrinsic::Fma,
        BuiltinFn::RangeClamp => Intrinsic::RangeClamp,
        BuiltinFn::RangeWrap => Intrinsic::RangeWrap,
        BuiltinFn::BindingCountClamp
        | BuiltinFn::BindingRangeClamp
        | BuiltinFn::BindingRangeInclusiveClamp
        | BuiltinFn::BindingCountWrap
        | BuiltinFn::BindingRangeWrap
        | BuiltinFn::BindingRangeInclusiveWrap => return None,
    })
}
