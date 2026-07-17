use crate::{StructId, TypeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    F32,
    F64,
    I32,
    I64,
    Bool,
}

impl ScalarType {
    pub const ALL: [Self; 5] = [Self::F32, Self::F64, Self::I32, Self::I64, Self::Bool];

    pub const fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Bool => "bool",
        }
    }

    pub const fn is_numeric(self) -> bool {
        !matches!(self, Self::Bool)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferChannels {
    Mono,
    Static(u32),
    Dynamic,
}

/// A logical MIR type. It deliberately contains no ABI size, alignment, or
/// pointer-width information.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Type {
    Scalar(ScalarType),
    Tuple(Vec<TypeId>),
    Array {
        element: TypeId,
        len: u32,
    },
    Struct(StructId),
    Slice {
        element: ScalarType,
        access: AccessMode,
    },
    Buffer {
        element: ScalarType,
        channels: BufferChannels,
        access: AccessMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ScalarValue {
    F32(#[serde(with = "json_f32")] f32),
    F64(#[serde(with = "json_f64")] f64),
    I32(i32),
    I64(#[serde(with = "json_i64")] i64),
    Bool(bool),
}

impl ScalarValue {
    pub const fn ty(self) -> ScalarType {
        match self {
            Self::F32(_) => ScalarType::F32,
            Self::F64(_) => ScalarType::F64,
            Self::I32(_) => ScalarType::I32,
            Self::I64(_) => ScalarType::I64,
            Self::Bool(_) => ScalarType::Bool,
        }
    }
}

/// JSON has no lossless representation for all MIR scalar values. In
/// particular, JavaScript numbers cannot represent arbitrary `i64` values and
/// JSON numbers cannot represent infinities or NaNs. Keep the readable JSON
/// shape of `ScalarValue`, but serialize `i64` as a decimal string and encode
/// non-finite floats by their exact IEEE bit pattern.
mod json_i64 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i64, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

mod json_f32 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Number(f32),
        Bits(String),
    }

    pub fn serialize<S>(value: &f32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if value.is_finite() {
            serializer.serialize_f32(*value)
        } else {
            serializer.serialize_str(&format!("0x{:08x}", value.to_bits()))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<f32, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Repr::deserialize(deserializer)? {
            Repr::Number(value) => Ok(value),
            Repr::Bits(bits) => {
                let digits = bits
                    .strip_prefix("0x")
                    .ok_or_else(|| D::Error::custom("f32 bit pattern must start with '0x'"))?;
                if digits.len() != 8 {
                    return Err(D::Error::custom(
                        "f32 bit pattern must contain exactly 8 hexadecimal digits",
                    ));
                }
                u32::from_str_radix(digits, 16)
                    .map(f32::from_bits)
                    .map_err(D::Error::custom)
            }
        }
    }
}

mod json_f64 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Number(f64),
        Bits(String),
    }

    pub fn serialize<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if value.is_finite() {
            serializer.serialize_f64(*value)
        } else {
            serializer.serialize_str(&format!("0x{:016x}", value.to_bits()))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<f64, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Repr::deserialize(deserializer)? {
            Repr::Number(value) => Ok(value),
            Repr::Bits(bits) => {
                let digits = bits
                    .strip_prefix("0x")
                    .ok_or_else(|| D::Error::custom("f64 bit pattern must start with '0x'"))?;
                if digits.len() != 16 {
                    return Err(D::Error::custom(
                        "f64 bit pattern must contain exactly 16 hexadecimal digits",
                    ));
                }
                u64::from_str_radix(digits, 16)
                    .map(f64::from_bits)
                    .map_err(D::Error::custom)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ConstantValue {
    Scalar(ScalarValue),
    Aggregate(Vec<ConstantValue>),
}

/// Inclusive finite range metadata for a numeric scalar interface value.
///
/// Both endpoints must have the interface scalar type, `min <= max`, and
/// boolean ranges are invalid. A declared default must lie within the range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ValueRange {
    pub min: ScalarValue,
    pub max: ScalarValue,
}
