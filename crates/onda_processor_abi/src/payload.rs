//! Recursive message schemas and prepared, allocation-free payload operations.
//!
//! Wire tensors are packed little-endian SoA. Prepared tensors have native
//! scalar alignment, with each dynamic parameter retaining its length prefix.
use serde::{Deserialize, Serialize};

mod layout;
mod shape;
pub use shape::PayloadLeaf;
mod value;
pub use value::{PayloadEncoder, PayloadEncoderWorkspace, PayloadSink, PayloadSource};
mod transfer;
pub use layout::{IntegerDomain, ParameterPlan, PayloadPlan, PayloadTensor, TensorRegion};
pub use transfer::PreparedPayload;

/// Default host-side wire budget for a payload containing dynamic slices.
pub const DEFAULT_DYNAMIC_WIRE_CAPACITY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarEncoding {
    F32,
    F64,
    I32,
    I64,
    Bool,
}

impl ScalarEncoding {
    pub const fn byte_size(self) -> usize {
        match self {
            Self::Bool => 1,
            Self::F32 | Self::I32 => 4,
            Self::F64 | Self::I64 => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PayloadType {
    Scalar {
        encoding: ScalarEncoding,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        integer_range: Option<crate::IntegerRangeMetadata>,
    },
    Tuple {
        elements: Vec<PayloadType>,
    },
    /// A nominal aggregate. Valid schemas contain at least one field.
    Struct {
        name: String,
        fields: Vec<PayloadField>,
    },
    Array {
        element: Box<PayloadType>,
        len: usize,
    },
    Slice {
        element: Box<PayloadType>,
    },
}

impl PayloadType {
    pub fn scalar(encoding: ScalarEncoding) -> Self {
        Self::Scalar {
            encoding,
            integer_range: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadField {
    pub name: String,
    pub ty: PayloadType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PayloadDefault>,
}

/// Exact source scalar spellings avoid rounding i64 and non-finite floats in
/// JSON. Aggregate defaults follow field/element order, independently of SoA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PayloadDefault {
    Scalar(String),
    Aggregate(Vec<PayloadDefault>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadSchema {
    pub params: Vec<PayloadField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    InvalidSchema,
    InvalidValue,
    InvalidRange,
    Overflow,
    NegativeLength,
    Truncated,
    TrailingBytes,
    InsufficientCapacity,
    MisalignedWorkspace,
    InvalidLengths,
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidValue => "payload value does not match its schema",
            Self::InvalidSchema => "invalid payload schema",
            Self::InvalidRange => "invalid payload integer range",
            Self::Overflow => "payload byte extent exceeds i32",
            Self::NegativeLength => "negative payload slice length",
            Self::Truncated => "truncated payload",
            Self::TrailingBytes => "unexpected trailing payload bytes",
            Self::InsufficientCapacity => "insufficient payload workspace or output capacity",
            Self::MisalignedWorkspace => "payload workspace must be aligned to eight bytes",
            Self::InvalidLengths => "payload lengths do not match the schema",
        })
    }
}
impl std::error::Error for PayloadError {}

#[cfg(test)]
mod tests;

impl std::fmt::Display for ScalarEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Bool => "bool",
        })
    }
}
impl std::fmt::Display for PayloadType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scalar { encoding, .. } => write!(f, "{encoding}"),
            Self::Struct { name, .. } => f.write_str(name),
            Self::Array { element, len } => write!(f, "{element}[{len}]"),
            Self::Slice { element } => write!(f, "{element}[]"),
            Self::Tuple { elements } => {
                f.write_str("(")?;
                for (index, element) in elements.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{element}")?;
                }
                f.write_str(")")
            }
        }
    }
}
