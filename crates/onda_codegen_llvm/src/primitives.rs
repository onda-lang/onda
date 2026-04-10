use onda_frontend::PrimitiveType;
use onda_semantics::TypedConstValue;

pub(crate) fn primitive_type_name(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "bool",
    }
}

pub(crate) fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

pub(crate) fn append_typed_const_bytes(
    out: &mut Vec<u8>,
    value: TypedConstValue,
    ty: PrimitiveType,
) {
    match (ty, value) {
        (PrimitiveType::F32, TypedConstValue::F32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::F64, TypedConstValue::F64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I32, TypedConstValue::I32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I64, TypedConstValue::I64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::Bool, TypedConstValue::Bool(v)) => out.push(if v { 1 } else { 0 }),
        (PrimitiveType::F32, other) => {
            let v = typed_const_to_f64(other) as f32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::F64, other) => {
            let v = typed_const_to_f64(other);
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I32, other) => {
            let v = typed_const_to_f64(other) as i32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I64, other) => {
            let v = typed_const_to_f64(other) as i64;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::Bool, other) => {
            out.push(if typed_const_to_f64(other) != 0.0 {
                1
            } else {
                0
            });
        }
    }
}

pub(crate) fn typed_const_to_f64(value: TypedConstValue) -> f64 {
    match value {
        TypedConstValue::F32(v) => v as f64,
        TypedConstValue::F64(v) => v,
        TypedConstValue::I32(v) => v as f64,
        TypedConstValue::I64(v) => v as f64,
        TypedConstValue::Bool(v) => {
            if v {
                1.0
            } else {
                0.0
            }
        }
    }
}
