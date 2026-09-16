//! The logical MIR interface projects into the compiler-free payload schema.
use crate::{ConstantValue, Program, ScalarType, ScalarValue, Type, TypeId};
use onda_processor_abi::payload::{
    PayloadDefault, PayloadError, PayloadField, PayloadSchema, PayloadType, ScalarEncoding,
};
use std::collections::HashSet;

impl Program {
    pub fn payload_schema<'a>(
        &self,
        params: impl IntoIterator<Item = (&'a str, TypeId, Option<&'a ConstantValue>)>,
    ) -> Result<PayloadSchema, PayloadError> {
        let mut active = HashSet::new();
        let params = params
            .into_iter()
            .map(|(name, ty, default)| {
                Ok(PayloadField {
                    name: name.to_owned(),
                    ty: self.payload_type(ty, &mut active)?,
                    default: default.map(payload_default),
                })
            })
            .collect::<Result<_, PayloadError>>()?;
        Ok(PayloadSchema { params })
    }

    fn payload_type(
        &self,
        id: TypeId,
        active: &mut HashSet<TypeId>,
    ) -> Result<PayloadType, PayloadError> {
        if !active.insert(id) {
            return Err(PayloadError::InvalidSchema);
        }
        let result = match self
            .types
            .get(id.index())
            .ok_or(PayloadError::InvalidSchema)?
        {
            Type::Scalar(scalar) => PayloadType::scalar(payload_scalar(*scalar)),
            Type::Tuple(elements) => PayloadType::Tuple {
                elements: elements
                    .iter()
                    .map(|ty| self.payload_type(*ty, active))
                    .collect::<Result<_, _>>()?,
            },
            Type::Array { element, len } => PayloadType::Array {
                element: Box::new(self.payload_type(*element, active)?),
                len: *len as usize,
            },
            Type::Struct(id) => {
                let structure = self
                    .structs
                    .get(id.index())
                    .ok_or(PayloadError::InvalidSchema)?;
                let fields = structure
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(PayloadField {
                            name: field.name.clone(),
                            ty: self.payload_type(field.ty, active)?,
                            default: None,
                        })
                    })
                    .collect::<Result<_, PayloadError>>()?;
                PayloadType::Struct {
                    name: structure.name.clone(),
                    fields,
                }
            }
            Type::Slice { element, .. } => PayloadType::Slice {
                element: Box::new(PayloadType::scalar(payload_scalar(*element))),
            },
            Type::Buffer { .. } | Type::BufferSpan { .. } => {
                return Err(PayloadError::InvalidSchema)
            }
        };
        active.remove(&id);
        Ok(result)
    }
}

pub fn payload_scalar(scalar: ScalarType) -> ScalarEncoding {
    match scalar {
        ScalarType::F32 => ScalarEncoding::F32,
        ScalarType::F64 => ScalarEncoding::F64,
        ScalarType::I32 => ScalarEncoding::I32,
        ScalarType::I64 => ScalarEncoding::I64,
        ScalarType::Bool => ScalarEncoding::Bool,
    }
}

fn payload_default(value: &ConstantValue) -> PayloadDefault {
    match value {
        ConstantValue::Aggregate(values) => {
            PayloadDefault::Aggregate(values.iter().map(payload_default).collect())
        }
        ConstantValue::Scalar(value) => PayloadDefault::Scalar(match value {
            ScalarValue::F32(value) => value.to_string(),
            ScalarValue::F64(value) => value.to_string(),
            ScalarValue::I32(value) => value.to_string(),
            ScalarValue::I64(value) => value.to_string(),
            ScalarValue::Bool(value) => value.to_string(),
        }),
    }
}
