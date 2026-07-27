use crate::{StructId, TypeId};
use serde::{Deserialize, Serialize};

/// Largest integer magnitude that host-facing `f64` parameter controls can
/// represent exactly.
pub const MAX_EXACT_HOST_CONTROL_INTEGER: i64 = (1_i64 << 53) - 1;

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
    /// SuperCollider-style `lincurve` curvature. Values near zero are linear;
    /// negative values bend toward the maximum and positive values toward the
    /// minimum.
    pub curve: Option<f64>,
    pub unit: Option<String>,
    pub step: Option<ScalarValue>,
    /// Number of equal intervals between the inclusive range endpoints.
    pub step_count: Option<u32>,
}

/// A validated, prepared scalar parameter domain for host-control use.
///
/// This combines the numeric range with its normalization, presentation, and
/// discrete-grid metadata. Hosts should prepare one domain per parameter and
/// reuse it for plain/normalized conversions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamDomain<'a> {
    scalar: ScalarType,
    minimum: f64,
    maximum: f64,
    scale: ParamScale,
    curve: Option<f64>,
    unit: Option<&'a str>,
    step: Option<f64>,
    step_count: Option<u32>,
}

impl<'a> ParamDomain<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scalar: ScalarType,
        minimum: f64,
        maximum: f64,
        scale: ParamScale,
        curve: Option<f64>,
        unit: Option<&'a str>,
        step: Option<f64>,
        step_count: Option<u32>,
    ) -> Option<Self> {
        if !matches!(
            scalar,
            ScalarType::F32 | ScalarType::F64 | ScalarType::I32 | ScalarType::I64
        ) || !minimum.is_finite()
            || !maximum.is_finite()
            || minimum >= maximum
            || curve.is_some_and(|curve| !curve.is_finite())
            || unit.is_some_and(|unit| unit.contains('\0'))
            || step.is_some_and(|step| !step.is_finite() || step <= 0.0)
            || step.is_some() != step_count.is_some()
            || step_count == Some(0)
        {
            return None;
        }
        if !host_control_value_fits_scalar(scalar, minimum)
            || !host_control_value_fits_scalar(scalar, maximum)
            || step.is_some_and(|step| !host_control_value_fits_scalar(scalar, step))
            || (scalar == ScalarType::I64
                && maximum - minimum > MAX_EXACT_HOST_CONTROL_INTEGER as f64)
        {
            return None;
        }
        if scale == ParamScale::Log
            && (!matches!(scalar, ScalarType::F32 | ScalarType::F64)
                || minimum <= 0.0
                || curve.is_some()
                || step.is_some())
        {
            return None;
        }
        if matches!(scalar, ScalarType::I32 | ScalarType::I64) && step.is_none() {
            return None;
        }
        if let (Some(step), Some(step_count)) = (step, step_count) {
            if validated_step_count(scalar, minimum, maximum, step) != Some(step_count) {
                return None;
            }
        }
        Some(Self {
            scalar,
            minimum,
            maximum,
            scale,
            curve,
            unit,
            step,
            step_count,
        })
    }

    fn from_control(range: ValueRange, control: &'a ParamControl) -> Option<Self> {
        let scalar = range.min.ty();
        if range.max.ty() != scalar || control.step.is_some_and(|step| step.ty() != scalar) {
            return None;
        }
        Self::new(
            scalar,
            range.min.as_f64(),
            range.max.as_f64(),
            control.scale,
            control.curve,
            control.unit.as_deref(),
            control.step.map(ScalarValue::as_f64),
            control.step_count,
        )
    }

    pub const fn scalar(self) -> ScalarType {
        self.scalar
    }

    pub const fn minimum(self) -> f64 {
        self.minimum
    }

    pub const fn maximum(self) -> f64 {
        self.maximum
    }

    pub const fn scale(self) -> ParamScale {
        self.scale
    }

    pub const fn scale_name(self) -> &'static str {
        match self.scale {
            ParamScale::Linear => "linear",
            ParamScale::Log => "log",
        }
    }

    pub const fn curve(self) -> Option<f64> {
        self.curve
    }

    pub const fn unit(self) -> Option<&'a str> {
        self.unit
    }

    pub const fn step(self) -> Option<f64> {
        self.step
    }

    pub const fn step_count(self) -> Option<u32> {
        self.step_count
    }

    pub fn constrain_plain(self, plain: f64) -> f64 {
        let clamped = if plain.is_nan() {
            self.minimum
        } else {
            plain.clamp(self.minimum, self.maximum)
        };
        let Some(step) = self.step else {
            return clamped;
        };
        let snapped = self.minimum + ((clamped - self.minimum) / step).round() * step;
        snapped.clamp(self.minimum, self.maximum)
    }

    pub fn normalized_to_plain(self, normalized: f64) -> f64 {
        let normalized = if normalized.is_nan() {
            0.0
        } else {
            normalized.clamp(0.0, 1.0)
        };
        if normalized == 0.0 {
            return self.minimum;
        }
        if normalized == 1.0 {
            return self.maximum;
        }
        let plain = match (self.curve, self.scale) {
            (Some(curve), ParamScale::Linear) => linear_unit_to_plain(
                self.minimum,
                self.maximum,
                curve_normalized_to_unit(curve, normalized),
            ),
            (None, ParamScale::Linear) => {
                linear_unit_to_plain(self.minimum, self.maximum, normalized)
            }
            (None, ParamScale::Log) => {
                let log_min = self.minimum.ln();
                (log_min + normalized * (self.maximum.ln() - log_min)).exp()
            }
            (Some(_), ParamScale::Log) => {
                unreachable!("validated control cannot mix log and curve")
            }
        };
        self.constrain_plain(plain)
    }

    pub fn plain_to_normalized(self, plain: f64) -> f64 {
        let plain = self.constrain_plain(plain);
        if plain == self.minimum {
            return 0.0;
        }
        if plain == self.maximum {
            return 1.0;
        }
        match (self.curve, self.scale) {
            (Some(curve), ParamScale::Linear) => curve_unit_to_normalized(
                curve,
                linear_plain_to_unit(self.minimum, self.maximum, plain),
            ),
            (None, ParamScale::Linear) => linear_plain_to_unit(self.minimum, self.maximum, plain),
            (None, ParamScale::Log) => {
                let log_min = self.minimum.ln();
                (plain.ln() - log_min) / (self.maximum.ln() - log_min)
            }
            (Some(_), ParamScale::Log) => {
                unreachable!("validated control cannot mix log and curve")
            }
        }
        .clamp(0.0, 1.0)
    }
}

fn host_control_value_fits_scalar(scalar: ScalarType, value: f64) -> bool {
    match scalar {
        ScalarType::F32 => (value as f32) as f64 == value,
        ScalarType::F64 => true,
        ScalarType::I32 => {
            value.fract() == 0.0 && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)
        }
        ScalarType::I64 => {
            value.fract() == 0.0 && value.abs() <= MAX_EXACT_HOST_CONTROL_INTEGER as f64
        }
        ScalarType::Bool => false,
    }
}

#[doc(hidden)]
pub fn validated_step_count(
    scalar: ScalarType,
    minimum: f64,
    maximum: f64,
    step: f64,
) -> Option<u32> {
    if !minimum.is_finite()
        || !maximum.is_finite()
        || !step.is_finite()
        || minimum >= maximum
        || step <= 0.0
    {
        return None;
    }
    match scalar {
        ScalarType::F32 | ScalarType::F64 => {
            let intervals = (maximum - minimum) / step;
            if !intervals.is_finite() {
                return None;
            }
            let rounded = intervals.round();
            if rounded < 1.0 || rounded > f64::from(u32::MAX) {
                return None;
            }
            let count = rounded as u32;
            float_grid_value_matches(scalar, minimum, maximum, step, count).then_some(count)
        }
        ScalarType::I32 | ScalarType::I64 => {
            if minimum.fract() != 0.0 || maximum.fract() != 0.0 || step.fract() != 0.0 {
                return None;
            }
            let width = maximum - minimum;
            let intervals = width / step;
            if !width.is_finite()
                || !intervals.is_finite()
                || intervals.fract() != 0.0
                || intervals < 1.0
                || intervals > f64::from(u32::MAX)
            {
                return None;
            }
            Some(intervals as u32)
        }
        ScalarType::Bool => None,
    }
}

#[doc(hidden)]
pub fn value_is_on_step_grid(
    scalar: ScalarType,
    minimum: f64,
    value: f64,
    step: f64,
    step_count: u32,
) -> bool {
    if !value.is_finite() || !step.is_finite() || step <= 0.0 {
        return false;
    }
    match scalar {
        ScalarType::F32 | ScalarType::F64 => {
            let index = (value - minimum) / step;
            if !index.is_finite() {
                return false;
            }
            let rounded = index.round();
            if rounded < 0.0 || rounded > f64::from(step_count) {
                return false;
            }
            float_grid_value_matches(scalar, minimum, value, step, rounded as u32)
        }
        ScalarType::I32 | ScalarType::I64 => {
            value.fract() == 0.0
                && minimum.fract() == 0.0
                && step.fract() == 0.0
                && (value - minimum) % step == 0.0
        }
        ScalarType::Bool => false,
    }
}

fn float_grid_value_matches(
    scalar: ScalarType,
    minimum: f64,
    expected: f64,
    step: f64,
    index: u32,
) -> bool {
    let scaled_step = step * f64::from(index);
    let reconstructed = minimum + scaled_step;
    if !reconstructed.is_finite() {
        return false;
    }
    if scalar == ScalarType::F32 {
        return (reconstructed as f32).to_bits() == (expected as f32).to_bits();
    }

    const ROUNDING_ULPS: f64 = 8.0;
    const MAX_ERROR_IN_STEPS: f64 = 0.125;
    let scale = minimum
        .abs()
        .max(expected.abs())
        .max(scaled_step.abs())
        .max(f64::MIN_POSITIVE);
    let rounding_tolerance = ROUNDING_ULPS * f64::EPSILON * scale;
    let grid_tolerance = MAX_ERROR_IN_STEPS * step;
    (reconstructed - expected).abs() <= rounding_tolerance.min(grid_tolerance)
}

fn linear_unit_to_plain(minimum: f64, maximum: f64, unit: f64) -> f64 {
    let width = maximum - minimum;
    if width.is_finite() {
        minimum + unit * width
    } else {
        (1.0 - unit) * minimum + unit * maximum
    }
}

fn linear_plain_to_unit(minimum: f64, maximum: f64, plain: f64) -> f64 {
    let width = maximum - minimum;
    if width.is_finite() {
        (plain - minimum) / width
    } else {
        let scale = minimum.abs().max(maximum.abs());
        ((plain / scale) - (minimum / scale)) / ((maximum / scale) - (minimum / scale))
    }
}

impl ParamControl {
    pub fn domain(&self, range: ValueRange) -> Option<ParamDomain<'_>> {
        ParamDomain::from_control(range, self)
    }
}

const CURVE_LINEAR_EPSILON: f64 = 0.001;

fn curve_normalized_to_unit(curve: f64, normalized: f64) -> f64 {
    if curve.abs() < CURVE_LINEAR_EPSILON {
        normalized
    } else if curve > 0.0 {
        1.0 - curve_normalized_to_unit(-curve, 1.0 - normalized)
    } else {
        (curve * normalized).exp_m1() / curve.exp_m1()
    }
}

fn curve_unit_to_normalized(curve: f64, unit: f64) -> f64 {
    if curve.abs() < CURVE_LINEAR_EPSILON {
        unit
    } else if curve > 0.0 {
        1.0 - curve_unit_to_normalized(-curve, 1.0 - unit)
    } else {
        (unit * curve.exp_m1()).ln_1p() / curve
    }
}

#[cfg(test)]
mod param_control_tests {
    use super::*;

    #[test]
    fn prepares_an_already_decoded_domain() {
        let domain = ParamDomain::new(
            ScalarType::F64,
            0.0,
            1.0,
            ParamScale::Linear,
            None,
            Some("dB"),
            Some(0.25),
            Some(4),
        )
        .expect("valid decoded domain");

        assert_eq!(domain.unit(), Some("dB"));
        assert_eq!(domain.normalized_to_plain(0.5), 0.5);
        assert_eq!(domain.plain_to_normalized(0.5), 0.5);
        assert_eq!(domain.constrain_plain(0.7), 0.75);
    }

    #[test]
    fn maps_linear_and_logarithmic_domains() {
        let linear = ParamControl::default();
        let linear_range = ValueRange {
            min: ScalarValue::F64(20.0),
            max: ScalarValue::F64(20_000.0),
        };
        let linear = linear
            .domain(linear_range)
            .expect("valid linear control domain");
        assert_eq!(linear.normalized_to_plain(0.5), 10_010.0);
        assert_eq!(linear.plain_to_normalized(10_010.0), 0.5);

        let log_control = ParamControl {
            scale: ParamScale::Log,
            ..ParamControl::default()
        };
        let log = log_control
            .domain(linear_range)
            .expect("valid logarithmic control domain");
        let midpoint = log.normalized_to_plain(0.5);
        assert!((midpoint - (20.0_f64 * 20_000.0).sqrt()).abs() < 1.0e-10);
        assert!((log.plain_to_normalized(midpoint) - 0.5).abs() < 1.0e-12);

        let wide_range = ValueRange {
            min: ScalarValue::F64(1.0e-300),
            max: ScalarValue::F64(1.0e300),
        };
        let wide_log_control = ParamControl {
            scale: ParamScale::Log,
            ..ParamControl::default()
        };
        let wide_log = wide_log_control
            .domain(wide_range)
            .expect("valid wide logarithmic control domain");
        let wide_midpoint = wide_log.normalized_to_plain(0.5);
        assert!((wide_midpoint - 1.0).abs() < 1.0e-12);
        assert!((wide_log.plain_to_normalized(1.0) - 0.5).abs() < 1.0e-12);

        let unit_range = ValueRange {
            min: ScalarValue::F64(0.0),
            max: ScalarValue::F64(1.0),
        };
        let inverse_curve_control = ParamControl {
            curve: Some(-4.0),
            ..ParamControl::default()
        };
        let inverse_curve = inverse_curve_control
            .domain(unit_range)
            .expect("valid inverse curve domain");
        let curved_midpoint = inverse_curve.normalized_to_plain(0.5);
        let expected = (-2.0_f64).exp_m1() / (-4.0_f64).exp_m1();
        assert!((curved_midpoint - expected).abs() < 1.0e-12);
        assert!((inverse_curve.plain_to_normalized(curved_midpoint) - 0.5).abs() < 1.0e-12);

        let forward_curve_control = ParamControl {
            curve: Some(4.0),
            ..ParamControl::default()
        };
        let forward_curve = forward_curve_control
            .domain(unit_range)
            .expect("valid forward curve domain");
        assert!((forward_curve.normalized_to_plain(0.5) + curved_midpoint - 1.0).abs() < 1.0e-12);

        let curved_step_control = ParamControl {
            curve: Some(-4.0),
            step: Some(ScalarValue::F64(0.25)),
            step_count: Some(4),
            ..ParamControl::default()
        };
        let curved_step = curved_step_control
            .domain(unit_range)
            .expect("valid stepped curve domain");
        assert_eq!(curved_step.normalized_to_plain(0.5), 1.0);
    }

    #[test]
    fn maps_wide_linear_domains_without_overflow() {
        let domain = ParamDomain::new(
            ScalarType::F64,
            -1.0e308,
            1.0e308,
            ParamScale::Linear,
            None,
            None,
            None,
            None,
        )
        .expect("valid wide linear domain");
        assert_eq!(domain.normalized_to_plain(0.5), 0.0);
        assert_eq!(domain.plain_to_normalized(0.0), 0.5);

        let curved = ParamDomain::new(
            ScalarType::F64,
            -1.0e308,
            1.0e308,
            ParamScale::Linear,
            Some(-4.0),
            None,
            None,
            None,
        )
        .expect("valid wide curved domain");
        let midpoint = curved.normalized_to_plain(0.5);
        assert!(midpoint.is_finite());
        assert!((curved.plain_to_normalized(midpoint) - 0.5).abs() < 1.0e-12);
    }

    #[test]
    fn validates_float_grids_at_storage_precision() {
        assert_eq!(
            validated_step_count(ScalarType::F32, 0.0, 100_000.0, 0.1_f32 as f64),
            Some(1_000_000)
        );
        assert_eq!(
            validated_step_count(ScalarType::F64, 0.0, 0.3, 0.1),
            Some(3)
        );
        assert_eq!(
            validated_step_count(ScalarType::F32, 0.0, 100_000.5, 1.0),
            None
        );
        assert!(!value_is_on_step_grid(
            ScalarType::F32,
            0.0,
            50_000.5,
            1.0,
            100_000,
        ));
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
        let domain = control.domain(range).expect("valid stepped domain");
        assert_eq!(domain.constrain_plain(-1.0), 0.0);
        assert_eq!(domain.constrain_plain(3.2), 4.0);
        assert_eq!(domain.normalized_to_plain(0.3), 4.0);
        assert_eq!(domain.plain_to_normalized(3.2), 0.4);
    }
}
