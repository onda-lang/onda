pub(super) const PRINT_NAME: &str = "print";
pub(super) const PRINT_VALUE_TYPES: &str = "f32 | f64 | i32 | i64 | bool";
pub(super) const PRINT_SIGNATURE: &str = "print(...values: f32 | f64 | i32 | i64 | bool)";
pub(super) const PRINT_LABEL_SIGNATURE: &str =
    "print(label: quoted text, ...values: f32 | f64 | i32 | i64 | bool)";
pub(super) const PRINT_DOCUMENTATION: &str =
    "Publishes one typed, host-facing diagnostic occurrence. The optional label is compile-time quoted text.";
