//! Nominal enum information retained at the executable interface seam.
use crate::*;
use onda_processor_abi::payload::PayloadType;
use onda_processor_abi::{
    EnumMemberMetadata, EnumMetadata, IntegerRangeEndpoint, IntegerRangeMetadata,
};

/// Enum conversions are explicit ordinal extraction or checked construction.
pub(crate) fn valid_cast(from: PrimitiveType, to: PrimitiveType) -> bool {
    match (from, to) {
        (PrimitiveType::Enum(_), PrimitiveType::I32) => true,
        (PrimitiveType::Enum(a), PrimitiveType::Enum(b)) => a == b,
        (PrimitiveType::I32 | PrimitiveType::I64, PrimitiveType::Enum(_)) => true,
        (PrimitiveType::Enum(_), _) | (_, PrimitiveType::Enum(_)) => false,
        _ => true,
    }
}

pub(crate) fn metadata(def: &EnumDef) -> EnumMetadata {
    EnumMetadata {
        name: def.name.clone(),
        members: def
            .members
            .iter()
            .map(|member| EnumMemberMetadata {
                name: member.name.clone(),
                label: member
                    .label
                    .clone()
                    .unwrap_or_else(|| display_label(&member.name)),
            })
            .collect(),
    }
}

fn display_label(name: &str) -> String {
    let chars: Vec<_> = name.chars().collect();
    let mut label = String::with_capacity(name.len());
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '_' {
            if !label.is_empty() && !label.ends_with(' ') {
                label.push(' ');
            }
            continue;
        }
        if index > 0
            && ch.is_uppercase()
            && !label.ends_with(' ')
            && (chars[index - 1].is_lowercase()
                || chars[index - 1].is_ascii_digit()
                || (chars[index - 1].is_uppercase()
                    && chars.get(index + 1).is_some_and(|ch| ch.is_lowercase())))
        {
            label.push(' ');
        }
        label.push(if label.is_empty() {
            ch.to_ascii_uppercase()
        } else {
            ch
        });
    }
    label.trim_end().to_owned()
}

pub(crate) fn integer_range(ty: PrimitiveType) -> Option<IntegerRangeMetadata> {
    let PrimitiveType::Enum(ty) = ty else {
        return None;
    };
    Some(IntegerRangeMetadata {
        min: IntegerRangeEndpoint {
            scalar: "i32".into(),
            value: "0".into(),
        },
        max: IntegerRangeEndpoint {
            scalar: "i32".into(),
            value: (ty.count - 1).to_string(),
        },
        mode: "clamp".into(),
    })
}

pub(crate) fn payload_scalar(ty: PrimitiveType, enums: &HashMap<u32, EnumMetadata>) -> PayloadType {
    PayloadType::Scalar {
        encoding: crate::aggregate_layout::scalar_encoding(ty),
        integer_range: integer_range(ty),
        enumeration: match ty {
            PrimitiveType::Enum(ty) => Some(enums[&ty.id].clone()),
            _ => None,
        },
    }
}

pub(crate) fn for_type(program: &TypedProgram, ty: PrimitiveType) -> Option<EnumMetadata> {
    let PrimitiveType::Enum(ty) = ty else {
        return None;
    };
    program
        .enums
        .iter()
        .find(|def| def.resolved == Some(ty))
        .map(metadata)
}

/// The same invariant is used by snapshots and generated scalar operations.
pub(crate) fn storage_range(ty: PrimitiveType) -> Option<onda_mir::IntegerRangeInvariant> {
    let PrimitiveType::Enum(ty) = ty else {
        return None;
    };
    Some(onda_mir::IntegerRangeInvariant {
        min: onda_mir::ScalarValue::I32(0),
        max: onda_mir::ScalarValue::I32(ty.count as i32 - 1),
        mode: onda_mir::IntegerRangeMode::Clamp,
    })
}
