use std::fmt;

use crate::{OptimizedProgram, Program, ValidatedProgram, ValidationError};

#[derive(Debug)]
pub enum MirMessagePackError {
    Encode(rmp_serde::encode::Error),
    Decode(rmp_serde::decode::Error),
    Invalid(Vec<ValidationError>),
}

impl fmt::Display for MirMessagePackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::Invalid(errors) => {
                write!(formatter, "invalid MIR")?;
                for error in errors {
                    write!(formatter, "\n{error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for MirMessagePackError {}

/// Encodes ordinary checked MIR with named MessagePack fields.
pub fn to_messagepack(program: &Program) -> Result<Vec<u8>, MirMessagePackError> {
    crate::validate(program).map_err(MirMessagePackError::Invalid)?;
    rmp_serde::to_vec_named(program).map_err(MirMessagePackError::Encode)
}

/// Encodes optimized producer MIR for the compact production backend boundary.
pub fn to_messagepack_optimized(
    program: &OptimizedProgram,
) -> Result<Vec<u8>, MirMessagePackError> {
    rmp_serde::to_vec_named(program.as_program()).map_err(MirMessagePackError::Encode)
}

/// Decodes MessagePack supplied by an untrusted caller. Unchecked bounds are
/// rejected by ordinary MIR validation.
pub fn from_messagepack(bytes: &[u8]) -> Result<ValidatedProgram, MirMessagePackError> {
    let program = rmp_serde::from_slice(bytes).map_err(MirMessagePackError::Decode)?;
    crate::validate_owned(program).map_err(MirMessagePackError::Invalid)
}

/// Decodes MessagePack emitted by a trusted Onda MIR producer.
///
/// # Safety
///
/// The serialized program must come from a producer that proved every
/// `BoundsMode::Unchecked` operation is in bounds for all executions reaching
/// it. Every declared `IntegerRangeInvariant` must contain every value
/// observable from that storage, including values supplied by callers or
/// restored from external state. Every pinned state slot must be fully
/// overwritten on every successful full-initialization path before it can be
/// observed.
pub unsafe fn from_messagepack_with_producer_proofs(
    bytes: &[u8],
) -> Result<ValidatedProgram, MirMessagePackError> {
    let program = rmp_serde::from_slice(bytes).map_err(MirMessagePackError::Decode)?;
    unsafe { crate::validate_owned_with_producer_proofs(program) }
        .map_err(MirMessagePackError::Invalid)
}

#[cfg(test)]
mod tests {
    use crate::{
        process_function_params, CompileConfig, Function, FunctionAttributes, FunctionId,
        FunctionKind, Program, SourceSpan, Type, TypeId,
    };

    fn checked_program() -> Program {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(crate::ScalarType::I32));
        let function = |name: &str, kind| Function {
            name: name.to_owned(),
            kind,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: crate::Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        let mut process = function("process", FunctionKind::Process);
        process.params = process_function_params(TypeId::new(0));
        program.functions = vec![function("init", FunctionKind::Init), process];
        program
    }

    #[test]
    fn checked_messagepack_round_trip_retains_validation() {
        let program = checked_program();
        let bytes = super::to_messagepack(&program).expect("checked MIR should encode");
        assert!(bytes.len() < serde_json::to_vec(&program).unwrap().len());
        let decoded = super::from_messagepack(&bytes).expect("checked MIR should decode");
        assert_eq!(decoded.as_program(), &program);
    }
}
