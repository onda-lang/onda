use std::fmt;

use crate::{OptimizedProgram, Program, ValidatedProgram, ValidationError};

#[derive(Debug)]
pub enum MirJsonError {
    Json(serde_json::Error),
    Invalid(Vec<ValidationError>),
}

impl fmt::Display for MirJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
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

impl std::error::Error for MirJsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<serde_json::Error> for MirJsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn require_valid(program: &Program) -> Result<(), MirJsonError> {
    crate::validate(program).map_err(MirJsonError::Invalid)
}

/// Encodes the versioned machine-readable MIR boundary.
pub fn to_json(program: &Program) -> Result<String, MirJsonError> {
    require_valid(program)?;
    Ok(serde_json::to_string(program)?)
}

/// Encodes readable, versioned MIR for tooling and checked-in fixtures.
pub fn to_json_pretty(program: &Program) -> Result<String, MirJsonError> {
    require_valid(program)?;
    Ok(serde_json::to_string_pretty(program)?)
}

/// Encodes MIR that already carries a validation proof, including trusted
/// producer proofs for unchecked operations.
pub fn to_json_validated(program: &ValidatedProgram) -> Result<String, MirJsonError> {
    Ok(serde_json::to_string(program.as_program())?)
}

/// Encodes optimized producer MIR without discarding its validation proof.
pub fn to_json_optimized(program: &OptimizedProgram) -> Result<String, MirJsonError> {
    Ok(serde_json::to_string(program.as_program())?)
}

/// Encodes readable optimized producer MIR for diagnostics and fixtures.
pub fn to_json_pretty_optimized(program: &OptimizedProgram) -> Result<String, MirJsonError> {
    Ok(serde_json::to_string_pretty(program.as_program())?)
}

/// Decodes machine-readable MIR and retains proof that all backend-neutral
/// schema and executable-safety invariants hold.
///
/// Backends may still reject unsupported capabilities during legalization.
pub fn from_json(json: &str) -> Result<ValidatedProgram, MirJsonError> {
    from_json_validated(json)
}

/// Decodes machine-readable MIR and retains proof that the backend-neutral
/// validator accepted it.
pub fn from_json_validated(json: &str) -> Result<ValidatedProgram, MirJsonError> {
    let program = serde_json::from_str(json)?;
    crate::validate_owned(program).map_err(MirJsonError::Invalid)
}

/// Decodes serialized MIR emitted by a trusted producer and retains its
/// unchecked-bounds proof.
///
/// # Safety
///
/// The serialized program must come from a producer that proved every
/// `BoundsMode::Unchecked` operation for all executions reaching it.
pub unsafe fn from_json_with_producer_proofs(json: &str) -> Result<ValidatedProgram, MirJsonError> {
    let program = serde_json::from_str(json)?;
    unsafe { crate::validate_owned_with_producer_proofs(program) }.map_err(MirJsonError::Invalid)
}

/// Compatibility helper for callers that need an owned raw schema value after
/// validation. Prefer [`from_json`] so the validation proof is not discarded.
pub fn from_json_program(json: &str) -> Result<Program, MirJsonError> {
    Ok(from_json(json)?.into_program())
}

#[cfg(test)]
mod tests {
    use crate::{
        process_function_params, validate, AccessMode, BoundsMode, CompileConfig, Function,
        FunctionAttributes, FunctionId, FunctionKind, FunctionOrigin, InlineHint, Local, LocalId,
        Place, PlaceBase, Program, Rvalue, ScalarType, ScalarValue, SliceSource, SourceSpan,
        StateId, StatePersistence, StateSlot, Statement, StatementKind, Type, TypeId, Value,
        MIR_SCHEMA_VERSION,
    };

    #[test]
    fn json_round_trip_preserves_valid_program() {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 128,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::I32));
        let mut process = empty_function("onda_process", FunctionKind::Process);
        process.params = process_function_params(TypeId::new(0));
        program.functions = vec![empty_function("onda_init", FunctionKind::Init), process];

        let json = super::to_json_pretty(&program).expect("MIR should encode");
        assert!(json.contains(&format!("\"schema_version\": {MIR_SCHEMA_VERSION}")));
        assert!(json.contains("\"kind\": \"process\""));
        let decoded = super::from_json(&json).expect("MIR should decode");
        assert_eq!(decoded.as_program(), &program);
        validate(&decoded).expect("decoded MIR should remain valid");
    }

    #[test]
    fn scalar_json_is_lossless_across_javascript_number_boundaries() {
        let values = [
            ScalarValue::I64(i64::MIN),
            ScalarValue::I64(i64::MAX),
            ScalarValue::F32(f32::from_bits(0xffc0_1234)),
            ScalarValue::F64(f64::from_bits(0x7ff8_0000_0000_1234)),
            ScalarValue::F64(f64::INFINITY),
            ScalarValue::F64(f64::NEG_INFINITY),
            ScalarValue::F64(-0.0),
        ];

        let json = serde_json::to_string(&values).expect("scalar values should encode");
        assert!(json.contains("\"value\":\"-9223372036854775808\""));
        assert!(json.contains("\"value\":\"9223372036854775807\""));
        assert!(json.contains("\"value\":\"0xffc01234\""));
        assert!(json.contains("\"value\":\"0x7ff8000000001234\""));
        assert!(json.contains("\"value\":\"0x7ff0000000000000\""));
        assert!(json.contains("\"value\":\"0xfff0000000000000\""));

        let decoded: Vec<ScalarValue> =
            serde_json::from_str(&json).expect("scalar values should decode");
        for (actual, expected) in decoded.iter().zip(values) {
            match (actual, expected) {
                (ScalarValue::F32(actual), ScalarValue::F32(expected)) => {
                    assert_eq!(actual.to_bits(), expected.to_bits());
                }
                (ScalarValue::F64(actual), ScalarValue::F64(expected)) => {
                    assert_eq!(actual.to_bits(), expected.to_bits());
                }
                _ => assert_eq!(*actual, expected),
            }
        }

        let unsafe_numeric_i64 = r#"{"type":"i64","value":9007199254740992}"#;
        assert!(serde_json::from_str::<ScalarValue>(unsafe_numeric_i64).is_err());
    }

    #[test]
    fn json_boundary_rejects_invalid_programs_in_both_directions() {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 128,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.functions = vec![empty_function("onda_init", FunctionKind::Init)];

        assert!(matches!(
            super::to_json(&program),
            Err(super::MirJsonError::Invalid(_))
        ));
        let json = serde_json::to_string(&program).expect("raw invalid fixture should encode");
        assert!(matches!(
            super::from_json(&json),
            Err(super::MirJsonError::Invalid(_))
        ));
    }

    #[test]
    fn current_schema_json_preserves_explicit_identity_attributes_and_slice_bounds() {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.extend([
            Type::Scalar(ScalarType::I32),
            Type::Scalar(ScalarType::F32),
            Type::Array {
                element: TypeId::new(1),
                len: 4,
            },
            Type::Slice {
                element: ScalarType::F32,
                access: AccessMode::ReadOnly,
            },
        ]);
        program.state.extend([
            StateSlot {
                name: "meter_storage".to_owned(),
                ty: TypeId::new(1),
                persistence: StatePersistence::ControlMirror,
            },
            StateSlot {
                name: "values".to_owned(),
                ty: TypeId::new(2),
                persistence: StatePersistence::Snapshot,
            },
        ]);
        program
            .interface
            .control_outputs
            .push(crate::ControlOutput {
                name: "meter".to_owned(),
                ty: TypeId::new(1),
                mirror: StateId::new(0),
            });
        let mut process = empty_function("onda_process", FunctionKind::Process);
        process.attributes = FunctionAttributes {
            origin: FunctionOrigin::CompilerGenerated,
            inline: InlineHint::Always,
        };
        process.params = process_function_params(TypeId::new(0));
        process.locals.push(Local {
            name: Some("view".to_owned()),
            ty: TypeId::new(3),
        });
        process.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Place(Place {
                        base: PlaceBase::State(StateId::new(1)),
                        projections: Vec::new(),
                    }),
                    start: Value::Constant(ScalarValue::I32(4)),
                    len: Value::Constant(ScalarValue::I32(0)),
                    bounds: BoundsMode::Trap,
                    access: AccessMode::ReadOnly,
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions = vec![empty_function("onda_init", FunctionKind::Init), process];

        let json = super::to_json_pretty(&program).expect("current-schema MIR should encode");
        assert!(json.contains("\"mirror\": 0"));
        assert!(json.contains("\"control_mirror\""));
        assert!(json.contains("\"origin\": \"compiler_generated\""));
        assert!(json.contains("\"inline\": \"always\""));
        assert!(json.contains("\"bounds\": \"trap\""));
        let decoded = super::from_json(&json).expect("current-schema MIR should decode");
        assert_eq!(decoded.as_program(), &program);
    }

    #[test]
    fn json_rejects_an_unknown_schema_version() {
        let mut program = Program::new(
            CompileConfig {
                sample_rate: 48_000.0,
                block_size: 64,
            },
            FunctionId::new(0),
            FunctionId::new(1),
        );
        program.types.push(Type::Scalar(ScalarType::I32));
        let mut process = empty_function("onda_process", FunctionKind::Process);
        process.params = process_function_params(TypeId::new(0));
        program.functions = vec![empty_function("onda_init", FunctionKind::Init), process];
        let unknown_version = MIR_SCHEMA_VERSION + 1;
        program.schema_version = unknown_version;

        let json = serde_json::to_string(&program).expect("raw old-schema fixture should encode");
        let error = super::from_json(&json).expect_err("unknown schema must not decode");
        assert!(
            matches!(error, super::MirJsonError::Invalid(errors) if errors
            .iter()
            .any(|error| error.message.contains(&format!("schema version {unknown_version}"))))
        );
    }

    fn empty_function(name: &str, kind: FunctionKind) -> Function {
        Function {
            name: name.to_owned(),
            kind,
            attributes: crate::FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: Vec::new(),
            body: crate::Block::default(),
            source: SourceSpan::UNKNOWN,
        }
    }
}
