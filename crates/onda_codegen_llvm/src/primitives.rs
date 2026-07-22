use onda_frontend::PrimitiveType;
use onda_mir::ScalarValue;

pub(crate) fn primitive_type_name(ty: PrimitiveType) -> &'static str {
    ty.name()
}

pub(crate) fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

#[cfg(any(feature = "llvm-orc", test))]
pub(crate) fn append_scalar_value_bytes(out: &mut Vec<u8>, value: ScalarValue, ty: PrimitiveType) {
    match (ty, value) {
        (PrimitiveType::F32, ScalarValue::F32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::F64, ScalarValue::F64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I32, ScalarValue::I32(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::I64, ScalarValue::I64(v)) => out.extend_from_slice(&v.to_ne_bytes()),
        (PrimitiveType::Bool, ScalarValue::Bool(v)) => out.push(if v { 1 } else { 0 }),
        (PrimitiveType::F32, other) => {
            let v = scalar_value_to_f64(other) as f32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::F64, other) => {
            let v = scalar_value_to_f64(other);
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I32, other) => {
            let v = scalar_value_to_f64(other) as i32;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::I64, other) => {
            let v = scalar_value_to_f64(other) as i64;
            out.extend_from_slice(&v.to_ne_bytes());
        }
        (PrimitiveType::Bool, other) => {
            out.push(if scalar_value_to_f64(other) != 0.0 {
                1
            } else {
                0
            });
        }
    }
}

pub(crate) fn scalar_value_to_f64(value: ScalarValue) -> f64 {
    match value {
        ScalarValue::F32(v) => v as f64,
        ScalarValue::F64(v) => v,
        ScalarValue::I32(v) => v as f64,
        ScalarValue::I64(v) => v as f64,
        ScalarValue::Bool(v) => {
            if v {
                1.0
            } else {
                0.0
            }
        }
    }
}
