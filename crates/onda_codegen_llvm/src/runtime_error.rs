//! Allocation-free validation failures. Generated execution statuses are separate.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSurface {
    Input,
    Output,
    Buffer,
    Parameter,
    ControlOutput,
    Snapshot,
}

impl fmt::Display for RuntimeSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Buffer => "buffer",
            Self::Parameter => "parameter",
            Self::ControlOutput => "control output",
            Self::Snapshot => "state snapshot",
        })
    }
}

/// An owned error with no heap storage or dependency on an instance's lifetime.
/// Formatting writes to the supplied formatter; conversion to a compiler-style
/// diagnostic is an explicit, allocating host operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    Uninitialized,
    Invalidated,
    UnknownIndex {
        surface: RuntimeSurface,
        index: usize,
    },
    MissingBinding {
        surface: RuntimeSurface,
        index: usize,
    },
    SizeMismatch {
        surface: RuntimeSurface,
        index: usize,
        expected: usize,
        actual: usize,
    },
    Misaligned {
        surface: RuntimeSurface,
        index: usize,
        alignment: usize,
    },
    InvalidShape {
        surface: RuntimeSurface,
        index: usize,
    },
    InvalidSampleRate,
    InvalidElementType {
        index: usize,
    },
    InvalidSegment {
        start: usize,
        frames: usize,
        block_size: usize,
    },
    InvalidFlags {
        flags: u32,
    },
    NoControlDomain {
        index: usize,
    },
    InvalidStorage,
    BackendUnavailable,
    SizeOverflow,
}

impl RuntimeError {
    /// Negative C API status. Nonnegative codes are reserved for generated execution.
    pub const fn code(self) -> i32 {
        match self {
            Self::Uninitialized => -5,
            Self::Invalidated => -6,
            Self::UnknownIndex { .. } => -7,
            Self::MissingBinding { .. } => -8,
            Self::SizeMismatch { .. } => -9,
            Self::Misaligned { .. } => -10,
            Self::InvalidShape { .. } => -11,
            Self::InvalidSampleRate => -12,
            Self::InvalidElementType { .. } => -13,
            Self::InvalidSegment { .. } => -14,
            Self::InvalidFlags { .. } => -15,
            Self::NoControlDomain { .. } => -16,
            Self::InvalidStorage => -17,
            Self::BackendUnavailable => -18,
            Self::SizeOverflow => -19,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Uninitialized => f.write_str("instance requires full initialization"),
            Self::Invalidated => f.write_str(
                "instance state is invalid after execution failure; initialize or restore it",
            ),
            Self::UnknownIndex { surface, index } => write!(f, "unknown {surface} index {index}"),
            Self::MissingBinding { surface, index } => {
                write!(f, "required {surface} {index} is not bound")
            }
            Self::SizeMismatch {
                surface,
                index,
                expected,
                actual,
            } => write!(
                f,
                "{surface} {index} expects {expected} bytes, got {actual}"
            ),
            Self::Misaligned {
                surface,
                index,
                alignment,
            } => write!(f, "{surface} {index} requires {alignment}-byte alignment"),
            Self::InvalidShape { surface, index } => {
                write!(f, "invalid shape for {surface} {index}")
            }
            Self::InvalidSampleRate => f.write_str("sample rate must be finite and positive"),
            Self::InvalidElementType { index } => write!(f, "buffer {index} element type mismatch"),
            Self::InvalidSegment {
                start,
                frames,
                block_size,
            } => write!(
                f,
                "segment start {start} and frame count {frames} exceed block size {block_size}"
            ),
            Self::InvalidFlags { flags } => write!(f, "unknown process flags {flags:#x}"),
            Self::NoControlDomain { index } => {
                write!(f, "parameter {index} has no numeric control domain")
            }
            Self::InvalidStorage => {
                f.write_str("runtime storage does not match the compiled program")
            }
            Self::BackendUnavailable => f.write_str("native execution backend is unavailable"),
            Self::SizeOverflow => f.write_str("size exceeds the runtime representation"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<RuntimeError> for onda_frontend::Diagnostic {
    fn from(error: RuntimeError) -> Self {
        Self::runtime(error.to_string(), 0, 0)
    }
}
