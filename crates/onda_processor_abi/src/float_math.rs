/// Independent permissions for native floating-point optimization. The default
/// preserves operation order, separate rounding, NaNs, infinities and signed zero.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FloatMathPolicy {
    /// Allow changing the association of arithmetic operations (for reductions).
    pub reassociate: bool,
    /// Allow replacing multiply followed by add with a fused operation.
    pub contract: bool,
    /// Ignore the sign of zero. LLVM needs this as well as reassociation for
    /// many vector reductions; it is deliberately a separate permission.
    pub no_signed_zeros: bool,
}
