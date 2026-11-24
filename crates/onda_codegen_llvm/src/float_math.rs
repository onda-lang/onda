pub use onda_processor_abi::FloatMathPolicy;

/// Floating-point presets for LLVM compilation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum FloatMathMode {
    #[default]
    Default = 0,
    Strict = 1,
    Fast = 2,
}

impl FloatMathMode {
    /// Individual permissions to pass alongside the full fast-math switch.
    pub fn policy(self) -> FloatMathPolicy {
        match self {
            Self::Default => FloatMathPolicy {
                reassociate: true,
                contract: true,
                no_signed_zeros: true,
            },
            Self::Strict | Self::Fast => FloatMathPolicy::default(),
        }
    }

    pub fn is_fast(self) -> bool {
        self == Self::Fast
    }
}
