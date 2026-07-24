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

    pub fn as_f64(self) -> f64 {
        match self {
            Self::F32(value) => value as f64,
            Self::F64(value) => value,
            Self::I32(value) => value as f64,
            Self::I64(value) => value as f64,
            Self::Bool(value) => u8::from(value) as f64,
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

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamScale {
    #[default]
    Linear,
    Log,
}

/// Host-facing behavior for a top-level parameter.
///
/// The range remains on [`crate::Param`] because it is also used by runtime
/// clamping. This structure describes how normalized host values map to that
/// plain range and, when present, its legal discrete grid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamControl {
    pub scale: ParamScale,
    pub unit: Option<String>,
    pub step: Option<ScalarValue>,
    /// Number of equal intervals between the inclusive range endpoints.
    pub step_count: Option<u32>,
}

impl ParamControl {
    pub fn constrain_plain(&self, range: ValueRange, plain: f64) -> f64 {
        let min = range.min.as_f64();
        let max = range.max.as_f64();
        let clamped = if plain.is_nan() {
            min
        } else {
            plain.clamp(min, max)
        };
        let Some(step) = self.step else {
            return clamped;
        };
        let step = step.as_f64();
        let snapped = min + ((clamped - min) / step).round() * step;
        snapped.clamp(min, max)
    }

    pub fn normalized_to_plain(&self, range: ValueRange, normalized: f64) -> f64 {
        let normalized = if normalized.is_nan() {
            0.0
        } else {
            normalized.clamp(0.0, 1.0)
        };
        let min = range.min.as_f64();
        let max = range.max.as_f64();
        if normalized == 0.0 {
            return min;
        }
        if normalized == 1.0 {
            return max;
        }
        let plain = match self.scale {
            ParamScale::Linear => min + normalized * (max - min),
            ParamScale::Log => min * (max / min).powf(normalized),
        };
        self.constrain_plain(range, plain)
    }

    pub fn plain_to_normalized(&self, range: ValueRange, plain: f64) -> f64 {
        let plain = self.constrain_plain(range, plain);
        let min = range.min.as_f64();
        let max = range.max.as_f64();
        if plain == min {
            return 0.0;
        }
        if plain == max {
            return 1.0;
        }
        match self.scale {
            ParamScale::Linear => (plain - min) / (max - min),
            ParamScale::Log => (plain / min).ln() / (max / min).ln(),
        }
        .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod param_control_tests {
    use super::*;

    #[test]
    fn maps_linear_and_logarithmic_domains() {
        let linear = ParamControl::default();
        let linear_range = ValueRange {
            min: ScalarValue::F64(20.0),
            max: ScalarValue::F64(20_000.0),
        };
        assert_eq!(linear.normalized_to_plain(linear_range, 0.5), 10_010.0);
        assert_eq!(linear.plain_to_normalized(linear_range, 10_010.0), 0.5);

        let log = ParamControl {
            scale: ParamScale::Log,
            ..ParamControl::default()
        };
        let midpoint = log.normalized_to_plain(linear_range, 0.5);
        assert!((midpoint - (20.0_f64 * 20_000.0).sqrt()).abs() < 1.0e-10);
        assert!((log.plain_to_normalized(linear_range, midpoint) - 0.5).abs() < 1.0e-12);
    }

    #[test]
    fn clamps_and_snaps_stepped_domains() {
        let control = ParamControl {
            step: Some(ScalarValue::I32(2)),
            step_count: Some(5),
            ..ParamControl::default()
        };
        let range = ValueRange {
            min: ScalarValue::I32(0),
            max: ScalarValue::I32(10),
        };
        assert_eq!(control.constrain_plain(range, -1.0), 0.0);
        assert_eq!(control.constrain_plain(range, 3.2), 4.0);
        assert_eq!(control.normalized_to_plain(range, 0.3), 4.0);
        assert_eq!(control.plain_to_normalized(range, 3.2), 0.4);
    }
}
