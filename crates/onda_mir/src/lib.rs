//! Backend-neutral mid-level representation for executable Onda programs.
//!
//! MIR is produced only after parsing, semantic analysis, specialization, and
//! processor/graph lowering have completed. Backends must not need to recover
//! source-level meaning from names or frontend AST nodes.

mod analysis;
mod format;
mod ids;
mod ir;
mod json;
mod messagepack;
mod passes;
mod types;
mod validate;

pub use format::format_program;
pub use ids::*;
pub use ir::*;
pub use json::{
    from_json, from_json_program, from_json_validated, from_json_with_producer_proofs, to_json,
    to_json_optimized, to_json_pretty, to_json_pretty_optimized, to_json_validated, MirJsonError,
};
pub use messagepack::{
    from_messagepack, from_messagepack_with_producer_proofs, to_messagepack,
    to_messagepack_optimized, MirMessagePackError,
};
pub use passes::{canonicalize, optimize, OptimizedProgram, PassStats};
pub use types::*;
pub use validate::{
    validate, validate_owned, validate_owned_with_producer_proofs, validate_with_producer_proofs,
    ValidatedProgram, ValidationError,
};

/// The in-memory MIR schema version.
///
/// JSON consumers must reject versions they do not understand. Compatible
/// additions retain this value; incompatible serialized-schema changes must
/// increment it.
pub const MIR_SCHEMA_VERSION: u32 = 5;

/// Positional ABI indices for the three value parameters of the process entry.
pub const PROCESS_START_FRAME_PARAM_INDEX: usize = 0;
pub const PROCESS_FRAMES_PARAM_INDEX: usize = 1;
pub const PROCESS_FLAGS_PARAM_INDEX: usize = 2;
pub const PROCESS_PARAM_COUNT: usize = 3;

/// Flags accepted by segmented process entry points.
pub const PROCESS_BEGIN_BLOCK: i32 = 1 << 0;
pub const PROCESS_END_BLOCK: i32 = 1 << 1;
pub const PROCESS_FULL_BLOCK: i32 = PROCESS_BEGIN_BLOCK | PROCESS_END_BLOCK;

/// Stable serialized names for the positional process-entry parameters.
pub const PROCESS_PARAM_NAMES: [&str; PROCESS_PARAM_COUNT] = ["start_frame", "frames", "flags"];
pub use analysis::{
    analyze_effects, analyze_integer_ranges, EffectAnalysis, FunctionEffects,
    FunctionRangeAnalysis, IntegerRange, MemoryRegionSet, ReferenceEffects,
};
