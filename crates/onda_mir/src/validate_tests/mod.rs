use crate::{
    AccessMode, Buffer, BufferChannels, BufferId, BufferRef, CallArgument, CompareOp,
    CompileConfig, ConstantValue, Delegate, DelegateParam, Event, EventId, EventParam, FieldId,
    Function, FunctionId, FunctionKind, FunctionParam, Intrinsic, Local, LocalId, Output, OutputId,
    Param, ParamId, PassingMode, Place, PlaceBase, Program, Projection, Rvalue, ScalarType,
    ScalarValue, SliceSource, SourceSpan, StatePersistence, StateSlot, Statement, StatementKind,
    StructField, StructType, Type, TypeId, Value,
};

fn function(name: &str, kind: FunctionKind) -> Function {
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

// Test-specific types start after the canonical process ABI's i32 type.
fn test_type(index: u32) -> TypeId {
    TypeId::new(index + 1)
}

fn empty_program() -> Program {
    let mut program = Program::new(
        CompileConfig {
            sample_rate: 48_000.0,
            block_size: 128,
        },
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types.push(Type::Scalar(ScalarType::I32));
    let mut process = function("onda_process", FunctionKind::Process);
    process.params = crate::process_function_params(TypeId::new(0));
    program.functions = vec![function("onda_init", FunctionKind::Init), process];
    program
}

fn validate_producer(program: &Program) -> Result<(), Vec<super::ValidationError>> {
    // SAFETY: producer fixtures in this module deliberately exercise
    // unchecked operations alongside the structural invariant under test.
    unsafe { super::validate_with_producer_proofs(program) }
}

#[test]
fn accepts_well_formed_empty_program() {
    assert!(super::validate(&empty_program()).is_ok());
}

#[test]
fn delegate_fixed_arrays_require_primitive_elements() {
    let mut program = empty_program();
    program.structs.push(StructType {
        name: "Payload".to_owned(),
        fields: Vec::new(),
    });
    program.types.extend([
        Type::Struct(crate::StructId::new(0)),
        Type::Array {
            element: test_type(0),
            len: 2,
        },
    ]);
    program.interface.delegates.push(Delegate {
        name: "invalid".to_owned(),
        params: vec![DelegateParam {
            name: "values".to_owned(),
            ty: test_type(1),
        }],
    });

    let errors = super::validate(&program)
        .expect_err("delegate arrays with aggregate elements must fail validation");
    assert!(errors.iter().any(|error| error
        .message
        .contains("fixed array element must be a primitive scalar")));
}

#[test]
fn pinned_state_requires_a_trusted_full_initialization_proof() {
    let mut program = empty_program();
    program.state.push(StateSlot {
        name: "cursor".to_owned(),
        ty: TypeId::new(0),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: true,
        integer_range: None,
    });
    program.functions[0].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::State(crate::StateId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program)
        .expect_err("ordinary MIR cannot assert the pinned initialization invariant");
    assert!(errors.iter().any(|error| error
        .message
        .contains("pinned initialization requires trusted producer validation")));

    // SAFETY: the init entry above overwrites the complete pinned scalar
    // before returning on its only successful path.
    unsafe { super::validate_with_producer_proofs(&program) }
        .expect("trusted producer validation should retain the proof");
}

#[test]
fn logical_type_equivalence_is_recursive_but_structs_are_nominal() {
    let mut program = empty_program();
    program.structs = vec![
        StructType {
            name: "Left".to_owned(),
            fields: vec![StructField {
                name: "value".to_owned(),
                ty: TypeId::new(1),
            }],
        },
        StructType {
            name: "Right".to_owned(),
            fields: vec![StructField {
                name: "value".to_owned(),
                ty: TypeId::new(2),
            }],
        },
    ];
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: TypeId::new(1),
            len: 2,
        },
        Type::Array {
            element: TypeId::new(2),
            len: 2,
        },
        Type::Tuple(vec![TypeId::new(3)]),
        Type::Tuple(vec![TypeId::new(4)]),
        Type::Struct(crate::StructId::new(0)),
        Type::Struct(crate::StructId::new(1)),
    ]);

    assert!(program.types_equivalent(TypeId::new(3), TypeId::new(4)));
    assert!(program.types_equivalent(TypeId::new(5), TypeId::new(6)));
    assert!(!program.types_equivalent(TypeId::new(7), TypeId::new(8)));
}

#[test]
fn rejects_block_size_outside_signed_process_abi() {
    let mut program = empty_program();
    program.config.block_size = i32::MAX as u32 + 1;

    let errors = super::validate(&program).expect_err("oversized block should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("2147483647")));
}

#[test]
fn validates_numeric_interface_ranges_and_defaults() {
    let mut valid = empty_program();
    valid.types.push(Type::Scalar(ScalarType::F32));
    valid.interface.inputs.push(crate::Input {
        name: "gain".to_owned(),
        ty: test_type(0),
        default: Some(ConstantValue::Scalar(ScalarValue::F32(0.5))),
        range: Some(crate::ValueRange {
            min: ScalarValue::F32(0.0),
            max: ScalarValue::F32(1.0),
        }),
    });
    super::validate(&valid).expect("finite ordered range containing its default is valid");

    let mut invalid = empty_program();
    invalid.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Scalar(ScalarType::Bool),
    ]);
    invalid.interface.inputs.push(crate::Input {
        name: "reversed".to_owned(),
        ty: test_type(0),
        default: Some(ConstantValue::Scalar(ScalarValue::F32(1.5))),
        range: Some(crate::ValueRange {
            min: ScalarValue::F32(2.0),
            max: ScalarValue::F32(1.0),
        }),
    });
    invalid.interface.params.extend([
        Param {
            name: "non_finite".to_owned(),
            ty: test_type(0),
            default: ConstantValue::Scalar(ScalarValue::F32(0.0)),
            range: Some(crate::ValueRange {
                min: ScalarValue::F32(f32::NEG_INFINITY),
                max: ScalarValue::F32(1.0),
            }),
            control: crate::ParamControl::default(),
        },
        Param {
            name: "boolean".to_owned(),
            ty: test_type(1),
            default: ConstantValue::Scalar(ScalarValue::Bool(false)),
            range: Some(crate::ValueRange {
                min: ScalarValue::Bool(false),
                max: ScalarValue::Bool(true),
            }),
            control: crate::ParamControl::default(),
        },
        Param {
            name: "outside".to_owned(),
            ty: TypeId::new(0),
            default: ConstantValue::Scalar(ScalarValue::I32(3)),
            range: Some(crate::ValueRange {
                min: ScalarValue::I32(0),
                max: ScalarValue::I32(2),
            }),
            control: crate::ParamControl::default(),
        },
    ]);

    let errors = super::validate(&invalid).expect_err("invalid ranges must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("input 'reversed' range minimum is greater")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("parameter 'non_finite' range endpoints must be finite")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("parameter 'boolean' range is not supported for bool")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("parameter 'outside' default is outside")));
}

#[test]
fn validates_parameter_control_metadata() {
    let mut valid = empty_program();
    valid.types.push(Type::Scalar(ScalarType::F32));
    valid.interface.params.push(Param {
        name: "cutoff".to_owned(),
        ty: test_type(0),
        default: ConstantValue::Scalar(ScalarValue::F32(440.0)),
        range: Some(crate::ValueRange {
            min: ScalarValue::F32(20.0),
            max: ScalarValue::F32(20_000.0),
        }),
        control: crate::ParamControl {
            scale: crate::ParamScale::Log,
            curve: None,
            unit: Some("Hz".to_owned()),
            step: None,
            step_count: None,
        },
    });
    super::validate(&valid).expect("valid logarithmic control metadata should pass");

    let mut non_finite_curve = valid.clone();
    non_finite_curve.interface.params[0].control.scale = crate::ParamScale::Linear;
    non_finite_curve.interface.params[0].control.curve = Some(f64::NAN);
    let errors =
        super::validate(&non_finite_curve).expect_err("non-finite curve metadata must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("curve must be finite")));

    let mut mixed_log_curve = valid.clone();
    mixed_log_curve.interface.params[0].control.curve = Some(-4.0);
    let errors =
        super::validate(&mixed_log_curve).expect_err("logarithmic curve metadata must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("cannot combine logarithmic scale with curve")));

    let mut invalid = empty_program();
    invalid.types.push(Type::Scalar(ScalarType::I32));
    invalid.interface.params.push(Param {
        name: "mode".to_owned(),
        ty: test_type(0),
        default: ConstantValue::Scalar(ScalarValue::I32(4)),
        range: Some(crate::ValueRange {
            min: ScalarValue::I32(0),
            max: ScalarValue::I32(10),
        }),
        control: crate::ParamControl {
            step: Some(ScalarValue::I32(2)),
            step_count: Some(4),
            ..crate::ParamControl::default()
        },
    });
    let errors = super::validate(&invalid).expect_err("wrong step count must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("step_count does not match the range and step")));

    let mut inexact_i64 = empty_program();
    inexact_i64.types.push(Type::Scalar(ScalarType::I64));
    inexact_i64.interface.params.push(Param {
        name: "wide".to_owned(),
        ty: test_type(0),
        default: ConstantValue::Scalar(ScalarValue::I64(9_007_199_254_740_992)),
        range: Some(crate::ValueRange {
            min: ScalarValue::I64(9_007_199_254_740_992),
            max: ScalarValue::I64(9_007_199_254_741_002),
        }),
        control: crate::ParamControl {
            step: Some(ScalarValue::I64(1)),
            step_count: Some(10),
            ..crate::ParamControl::default()
        },
    });
    let errors = super::validate(&inexact_i64).expect_err("inexact i64 control range must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("must fit the exact host integer range")));
}

#[test]
fn rejects_noncanonical_process_parameter_count() {
    let mut program = empty_program();
    program.functions[1].params.pop();

    let errors = super::validate(&program).expect_err("short process signature should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("exactly 3 parameters (start_frame, frames, flags)")));
}

#[test]
fn rejects_noncanonical_process_parameter_names_modes_and_types() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.functions[1].params[0].name = "frames".to_owned();
    program.functions[1].params[1].mode = PassingMode::ReadOnlyReference;
    program.functions[1].params[2].ty = test_type(0);

    let errors = super::validate(&program).expect_err("invalid process signature should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("must be named 'start_frame'")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("'frames' must use value")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("'flags' must have type i32")));
}

#[test]
fn rejects_process_results() {
    let mut program = empty_program();
    program.functions[1].results.push(TypeId::new(0));

    let errors = super::validate(&program).expect_err("process result should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("must not return values")));
}

#[test]
fn rejects_explicit_init_and_event_entry_signatures() {
    let mut program = empty_program();
    program.functions[0].params.push(FunctionParam {
        integer_range: None,
        name: "hidden".to_owned(),
        ty: TypeId::new(0),
        mode: PassingMode::Value,
    });
    program.functions[0].results.push(TypeId::new(0));

    let handler_id = FunctionId::new(2);
    let mut handler = function("onda_event::tick", FunctionKind::Event(EventId::new(0)));
    handler.params.push(FunctionParam {
        integer_range: None,
        name: "payload".to_owned(),
        ty: TypeId::new(0),
        mode: PassingMode::Value,
    });
    handler.results.push(TypeId::new(0));
    program.interface.events.push(Event {
        name: "tick".to_owned(),
        params: Vec::new(),
        handler: handler_id,
    });
    program.functions.push(handler);

    let errors = super::validate(&program).expect_err("explicit entry signatures must be rejected");
    assert!(errors.iter().any(|error| error
        .message
        .contains("init entry point must not declare function parameters")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("init entry point must not return")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("event 'tick' handler must not declare function parameters")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("event 'tick' handler must not return")));
}

#[test]
fn rejects_unowned_entry_role_functions() {
    let mut program = empty_program();
    let handler_id = FunctionId::new(2);
    program.interface.events.push(Event {
        name: "tick".to_owned(),
        params: Vec::new(),
        handler: handler_id,
    });
    program.functions.push(function(
        "onda_event::tick",
        FunctionKind::Event(EventId::new(0)),
    ));
    program
        .functions
        .push(function("extra_init", FunctionKind::Init));
    program.functions.push(function(
        "extra_event",
        FunctionKind::Event(EventId::new(0)),
    ));
    let mut extra_process = function("extra_process", FunctionKind::Process);
    extra_process.params = crate::process_function_params(TypeId::new(0));
    program.functions.push(extra_process);

    let errors = super::validate(&program).expect_err("entry roles must be one-to-one");
    assert!(errors.iter().any(|error| error
        .message
        .contains("init-kind function is not the program init entry point")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("event-kind function is not the registered handler")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("process-kind function is not the program process entry point")));
}

#[test]
fn rejects_recursive_user_function_call_cycles() {
    let mut program = empty_program();
    let mut first = function("first", FunctionKind::User);
    first.body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(3),
            args: Vec::new(),
        },
        source: SourceSpan::UNKNOWN,
    });
    let mut second = function("second", FunctionKind::User);
    second.body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: Vec::new(),
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.extend([first, second]);

    let errors = super::validate(&program).expect_err("recursive MIR should fail");
    assert!(errors.iter().any(|error| {
        error.function == Some(FunctionId::new(2))
            && error
                .message
                .contains("recursive call cycle is not realtime-safe: first -> second -> first")
    }));
}

#[test]
fn rejects_result_function_with_empty_fallthrough_body() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let mut callee = function("partial", FunctionKind::User);
    callee.results.push(test_type(0));
    program.functions.push(callee);

    let errors = super::validate(&program).expect_err("result fallthrough should fail");
    assert!(errors.iter().any(|error| {
        error.function == Some(FunctionId::new(2))
            && error.message.contains("falls through without returning")
    }));
}

#[test]
fn rejects_result_function_when_only_one_if_branch_returns() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let mut callee = function("partial", FunctionKind::User);
    callee.results.push(test_type(0));
    callee.body.statements.push(Statement {
        kind: StatementKind::If {
            condition: Value::Constant(ScalarValue::Bool(true)),
            then_block: crate::Block {
                statements: vec![Statement {
                    kind: StatementKind::Return {
                        values: vec![Value::Constant(ScalarValue::F32(1.0))],
                    },
                    source: SourceSpan::UNKNOWN,
                }],
            },
            else_block: crate::Block::default(),
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    let errors = super::validate(&program).expect_err("partial branch return should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("falls through without returning")));
}

#[test]
fn accepts_result_function_when_both_if_branches_return() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let return_block = |value| crate::Block {
        statements: vec![Statement {
            kind: StatementKind::Return {
                values: vec![Value::Constant(ScalarValue::F32(value))],
            },
            source: SourceSpan::UNKNOWN,
        }],
    };
    let mut callee = function("total", FunctionKind::User);
    callee.results.push(test_type(0));
    callee.body.statements.push(Statement {
        kind: StatementKind::If {
            condition: Value::Constant(ScalarValue::Bool(true)),
            then_block: return_block(1.0),
            else_block: return_block(2.0),
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    super::validate(&program).expect("complete branch returns should validate");
}

#[test]
fn result_function_infinite_loop_has_no_reachable_fallthrough() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let mut callee = function("spin", FunctionKind::User);
    callee.results.push(test_type(0));
    callee.body.statements.push(Statement {
        kind: StatementKind::Loop {
            body: crate::Block {
                statements: vec![Statement {
                    kind: StatementKind::Continue,
                    source: SourceSpan::UNKNOWN,
                }],
            },
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    super::validate(&program).expect("non-terminating loop cannot fall through");
}

#[test]
fn reachable_loop_break_requires_a_following_return() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let mut callee = function("breaks", FunctionKind::User);
    callee.results.push(test_type(0));
    callee.body.statements.push(Statement {
        kind: StatementKind::Loop {
            body: crate::Block {
                statements: vec![Statement {
                    kind: StatementKind::Break,
                    source: SourceSpan::UNKNOWN,
                }],
            },
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    let errors = super::validate(&program).expect_err("loop break should reach fallthrough");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("falls through without returning")));
}

#[test]
fn no_result_function_may_fall_through() {
    let mut program = empty_program();
    program
        .functions
        .push(function("observe", FunctionKind::User));
    super::validate(&program).expect("no-result function may fall through");
}

#[test]
fn rejects_loop_control_outside_loop() {
    let mut program = empty_program();
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Break,
        source: SourceSpan::UNKNOWN,
    });
    let errors = super::validate(&program).expect_err("invalid MIR should be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("outside a loop")));
}

#[test]
fn rejects_wrong_entry_point_kind() {
    let mut program = empty_program();
    program.functions[1].kind = FunctionKind::User;
    let errors = super::validate(&program).expect_err("invalid MIR should be rejected");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("process entry point")));
}

#[test]
fn rejects_mistyped_multi_value_call_destination() {
    let mut program = empty_program();
    program
        .types
        .extend([Type::Scalar(ScalarType::F32), Type::Scalar(ScalarType::I32)]);
    let mut callee = function("pair", FunctionKind::User);
    callee.results = vec![test_type(0), test_type(1)];
    program.functions.push(callee);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        },
        Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        },
    ]);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Call {
            results: vec![LocalId::new(0), LocalId::new(1)],
            function: FunctionId::new(2),
            args: Vec::<CallArgument>::new(),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("mistyped call result should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("result 1") && error.message.contains("i32")));
}

#[test]
fn rejects_mistyped_return_component() {
    let mut program = empty_program();
    program
        .types
        .extend([Type::Scalar(ScalarType::F32), Type::Scalar(ScalarType::I32)]);
    let mut callee = function("pair", FunctionKind::User);
    callee.results = vec![test_type(0), test_type(1)];
    callee.body.statements.push(Statement {
        kind: StatementKind::Return {
            values: vec![
                Value::Constant(ScalarValue::F32(1.0)),
                Value::Constant(ScalarValue::F32(2.0)),
            ],
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    let errors = super::validate(&program).expect_err("mistyped return should fail");
    assert!(errors.iter().any(|error| {
        error.message.contains("return value 1") && error.message.contains("i32")
    }));
}

#[test]
fn rejects_mistyped_assignment() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("mistyped assignment should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("assignment value")));
}

#[test]
fn rejects_mistyped_call_argument() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    let mut callee = function("consume", FunctionKind::User);
    callee.params.push(FunctionParam {
        integer_range: None,
        name: "value".to_owned(),
        ty: test_type(0),
        mode: PassingMode::Value,
    });
    program.functions.push(callee);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: vec![CallArgument::Value(Value::Constant(ScalarValue::I32(1)))],
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("mistyped call argument should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("argument 0")));
}

#[test]
fn accepts_reference_call_argument_into_a_slice() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Scalar(ScalarType::I32),
        Type::Array {
            element: test_type(0),
            len: 2,
        },
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
    ]);
    program.state.push(StateSlot {
        integer_range: None,
        name: "values".to_owned(),
        ty: test_type(2),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
    });
    let mut callee = function("update", FunctionKind::User);
    callee.params.push(FunctionParam {
        integer_range: None,
        name: "value".to_owned(),
        ty: test_type(0),
        mode: PassingMode::ReadWriteReference,
    });
    program.functions.push(callee);
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("view".to_owned()),
        ty: test_type(3),
    });
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Place(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    }),
                    start: Value::Constant(ScalarValue::I32(0)),
                    len: Value::Constant(ScalarValue::I32(2)),
                    bounds: crate::BoundsMode::Unchecked,
                    access: AccessMode::ReadWrite,
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::SliceElement {
                    slice: Value::Local(LocalId::new(0)),
                    index: Value::Constant(ScalarValue::I32(1)),
                    bounds: crate::BoundsMode::Clamp,
                }],
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    validate_producer(&program).expect("slice element should satisfy scalar reference");
}

#[test]
fn rejects_mistyped_output_frame_and_value() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.outputs.push(Output {
        name: "out1".to_owned(),
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::OutputStore {
            output: OutputId::new(0),
            element: None,
            bounds: crate::BoundsMode::Unchecked,
            frame: Value::Constant(ScalarValue::F32(0.0)),
            value: Value::Constant(ScalarValue::I32(1)),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("mistyped output store should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("audio frame")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("output store value")));
}

#[test]
fn validates_explicit_output_loads() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.outputs.push(Output {
        name: "out1".to_owned(),
        ty: test_type(0),
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("frame".to_owned()),
        ty: TypeId::new(0),
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("current".to_owned()),
        ty: test_type(0),
    });
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::ProcessFrame {
                    offset: Value::Constant(ScalarValue::I32(0)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::OutputLoad {
                    output: OutputId::new(0),
                    element: None,
                    bounds: crate::BoundsMode::Unchecked,
                    frame: Value::Local(LocalId::new(0)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    validate_producer(&program).expect("well-typed output load should validate");

    let Rvalue::OutputLoad { frame, .. } = (match &mut program.functions[1].body.statements[1].kind
    {
        StatementKind::Assign { value, .. } => value,
        _ => unreachable!(),
    }) else {
        unreachable!()
    };
    *frame = Value::Constant(ScalarValue::F32(0.0));
    let errors = super::validate(&program).expect_err("mistyped output frame should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("audio frame")));
}

#[test]
fn rejects_audio_io_without_process_capability_or_canonical_frame() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.inputs.push(crate::Input {
        name: "in1".to_owned(),
        ty: test_type(0),
        default: None,
        range: None,
    });
    program.interface.outputs.push(Output {
        name: "out1".to_owned(),
        ty: test_type(0),
    });
    program.functions[0].locals.push(Local {
        integer_range: None,
        name: Some("sample".to_owned()),
        ty: test_type(0),
    });
    program.functions[0].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::InputLoad {
                input: crate::InputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                frame: Value::Constant(ScalarValue::I32(0)),
            },
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::OutputStore {
            output: OutputId::new(0),
            element: None,
            bounds: crate::BoundsMode::Unchecked,
            frame: Value::Constant(ScalarValue::I32(-1)),
            value: Value::Constant(ScalarValue::F32(0.0)),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("unsafe audio I/O must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("only valid in the process entry point")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("must be dominated by process_frame")));
}

#[test]
fn read_write_reference_call_invalidates_process_frame_provenance() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.outputs.push(Output {
        name: "out1".to_owned(),
        ty: test_type(0),
    });

    let mut mutator = function("mutate_frame", FunctionKind::User);
    mutator.params.push(FunctionParam {
        integer_range: None,
        name: "frame".to_owned(),
        ty: TypeId::new(0),
        mode: PassingMode::ReadWriteReference,
    });
    mutator.body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Parameter(crate::ParameterId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::I32(i32::MAX))),
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(mutator);

    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("frame".to_owned()),
        ty: TypeId::new(0),
    });
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::ProcessFrame {
                    offset: Value::Constant(ScalarValue::I32(0)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::Place(Place::local(LocalId::new(0)))],
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::OutputStore {
                output: OutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                frame: Value::Local(LocalId::new(0)),
                value: Value::Constant(ScalarValue::F32(0.0)),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    let errors = super::validate(&program)
        .expect_err("a read-write call may replace a checked process frame");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("must be dominated by process_frame")));
}

#[test]
fn boolean_comparisons_only_support_equality() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::Bool));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("comparison".to_owned()),
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Compare {
                op: CompareOp::Less,
                lhs: Value::Constant(ScalarValue::Bool(false)),
                rhs: Value::Constant(ScalarValue::Bool(true)),
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("relational boolean comparison must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("boolean comparisons only support")));

    let StatementKind::Assign {
        value: Rvalue::Compare { op, .. },
        ..
    } = &mut program.functions[1].body.statements[0].kind
    else {
        unreachable!()
    };
    *op = CompareOp::Equal;
    super::validate(&program).expect("boolean equality should remain valid");
}

#[test]
fn rejects_interface_array_default_with_wrong_shape() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: test_type(0),
            len: 2,
        },
    ]);
    program.interface.inputs.push(crate::Input {
        name: "stereo".to_owned(),
        ty: test_type(1),
        default: Some(crate::ConstantValue::Aggregate(vec![
            crate::ConstantValue::Scalar(ScalarValue::F32(0.0)),
        ])),
        range: None,
    });

    let errors = super::validate(&program).expect_err("short array default should fail");
    assert!(errors.iter().any(|error| {
        error.message.contains("input 'stereo' default") && error.message.contains("Array")
    }));
}

#[test]
fn rejects_runtime_handles_nested_in_interface_storage() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
        Type::Array {
            element: test_type(0),
            len: 2,
        },
        Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadOnly,
        },
        Type::Struct(crate::StructId::new(0)),
        Type::Tuple(vec![test_type(0)]),
    ]);
    program.structs.push(StructType {
        name: "ResourceBox".to_owned(),
        fields: vec![StructField {
            name: "resource".to_owned(),
            ty: test_type(2),
        }],
    });
    program.interface.inputs.push(crate::Input {
        name: "input_view".to_owned(),
        ty: test_type(1),
        default: None,
        range: None,
    });
    program.interface.outputs.push(Output {
        name: "output_buffer".to_owned(),
        ty: test_type(2),
    });
    program
        .interface
        .control_outputs
        .push(crate::ControlOutput {
            name: "control_resource".to_owned(),
            ty: test_type(3),
            mirror: crate::StateId::new(0),
        });
    program.state.push(StateSlot {
        integer_range: None,
        name: "control_resource_mirror".to_owned(),
        ty: test_type(3),
        persistence: StatePersistence::ControlMirror,
        authored: true,
        pinned: false,
    });
    program.interface.params.push(Param {
        name: "parameter_view".to_owned(),
        ty: test_type(4),
        default: ConstantValue::Aggregate(Vec::new()),
        range: None,
        control: crate::ParamControl::default(),
    });

    let errors = super::validate(&program).expect_err("runtime handles must not be stored");
    for context in [
        "input 'input_view'",
        "output 'output_buffer'",
        "control output 'control_resource'",
        "parameter 'parameter_view'",
    ] {
        assert!(errors.iter().any(|error| {
            error.message.contains(context)
                && error
                    .message
                    .contains("runtime-only slice or buffer handles")
        }));
    }
}

#[test]
fn rejects_runtime_handles_nested_in_persistent_state() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
        Type::Array {
            element: test_type(0),
            len: 2,
        },
        Type::Struct(crate::StructId::new(0)),
        Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Dynamic,
            access: AccessMode::ReadWrite,
        },
        Type::Tuple(vec![test_type(3)]),
    ]);
    program.structs.push(StructType {
        name: "NestedView".to_owned(),
        fields: vec![StructField {
            name: "views".to_owned(),
            ty: test_type(1),
        }],
    });
    program.state.extend([
        StateSlot {
            integer_range: None,
            name: "saved_view".to_owned(),
            ty: test_type(2),
            persistence: StatePersistence::Snapshot,
            authored: true,
            pinned: false,
        },
        StateSlot {
            integer_range: None,
            name: "scratch_buffer".to_owned(),
            ty: test_type(4),
            persistence: StatePersistence::InstanceScratch,
            authored: false,
            pinned: false,
        },
    ]);

    let errors = super::validate(&program).expect_err("runtime handles must not persist");
    assert!(errors.iter().any(|error| error
        .message
        .contains("snapshot state 'saved_view' type must not contain runtime-only")));
    assert!(errors.iter().any(|error| error
        .message
        .contains("instance-scratch state 'scratch_buffer' type must not contain runtime-only")));
}

#[test]
fn rejects_runtime_handles_nested_in_function_results() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
        Type::Array {
            element: test_type(0),
            len: 2,
        },
    ]);
    let mut callee = function("return_view", FunctionKind::User);
    callee.results.push(test_type(1));
    callee.body.statements.push(Statement {
        kind: StatementKind::Loop {
            body: crate::Block {
                statements: vec![Statement {
                    kind: StatementKind::Continue,
                    source: SourceSpan::UNKNOWN,
                }],
            },
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(callee);

    let errors = super::validate(&program).expect_err("runtime handle result should fail");
    assert!(errors.iter().any(|error| {
        error.function == Some(FunctionId::new(2))
            && error
                .message
                .contains("function result type must not contain runtime-only")
    }));
}

#[test]
fn accepts_runtime_handles_in_transient_function_values() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
        Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        },
    ]);
    let mut callee = function("consume_resources", FunctionKind::User);
    callee.params.extend([
        FunctionParam {
            integer_range: None,
            name: "view".to_owned(),
            ty: test_type(0),
            mode: PassingMode::Value,
        },
        FunctionParam {
            integer_range: None,
            name: "buffer".to_owned(),
            ty: test_type(1),
            mode: PassingMode::ReadWriteReference,
        },
    ]);
    callee.locals.push(Local {
        integer_range: None,
        name: Some("window".to_owned()),
        ty: test_type(0),
    });
    program.functions.push(callee);

    super::validate(&program).expect("function parameters and locals are transient");
}

#[test]
fn accepts_direct_read_only_slice_event_parameters() {
    let mut program = empty_program();
    program.types.push(Type::Slice {
        element: ScalarType::F32,
        access: AccessMode::ReadOnly,
    });
    program.interface.events.push(Event {
        name: "set_curve".to_owned(),
        params: vec![EventParam {
            name: "values".to_owned(),
            ty: test_type(0),
            default: None,
        }],
        handler: FunctionId::new(2),
    });
    program.functions.push(function(
        "onda_event::set_curve",
        FunctionKind::Event(EventId::new(0)),
    ));

    super::validate(&program).expect("read-only event slices are a supported payload type");
}

#[test]
fn rejects_mutable_buffer_and_nested_event_handles() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
        Type::Buffer {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadOnly,
        },
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
        Type::Array {
            element: test_type(2),
            len: 2,
        },
    ]);
    program.interface.events.push(Event {
        name: "invalid".to_owned(),
        params: vec![
            EventParam {
                name: "mutable_view".to_owned(),
                ty: test_type(0),
                default: None,
            },
            EventParam {
                name: "buffer".to_owned(),
                ty: test_type(1),
                default: None,
            },
            EventParam {
                name: "nested_views".to_owned(),
                ty: test_type(3),
                default: None,
            },
        ],
        handler: FunctionId::new(2),
    });
    program.functions.push(function(
        "onda_event::invalid",
        FunctionKind::Event(EventId::new(0)),
    ));

    let errors = super::validate(&program).expect_err("invalid event handles should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("parameter 'mutable_view' slice must be read-only")));
    for name in ["buffer", "nested_views"] {
        assert!(errors.iter().any(|error| {
            error.message.contains(&format!("parameter '{name}'"))
                && error.message.contains("direct read-only slice")
        }));
    }
}

#[test]
fn accepts_slice_construction_load_and_store() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Scalar(ScalarType::I32),
        Type::Array {
            element: test_type(0),
            len: 4,
        },
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
    ]);
    program.state.push(StateSlot {
        integer_range: None,
        name: "values".to_owned(),
        ty: test_type(2),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
    });
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(3),
        },
        Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        },
    ]);
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Place(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    }),
                    start: Value::Constant(ScalarValue::I32(1)),
                    len: Value::Constant(ScalarValue::I32(2)),
                    bounds: crate::BoundsMode::Unchecked,
                    access: AccessMode::ReadWrite,
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::SliceLoad {
                    slice: Value::Local(LocalId::new(0)),
                    index: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Clamp,
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::SliceStore {
                slice: Value::Local(LocalId::new(0)),
                index: Value::Constant(ScalarValue::I32(1)),
                value: Value::Local(LocalId::new(1)),
                bounds: crate::BoundsMode::Clamp,
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    validate_producer(&program).expect("well-typed slice MIR should validate");
}

#[test]
fn rejects_store_through_read_only_slice() {
    let mut program = empty_program();
    program.types.push(Type::Slice {
        element: ScalarType::F32,
        access: AccessMode::ReadOnly,
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("view".to_owned()),
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::SliceStore {
            slice: Value::Local(LocalId::new(0)),
            index: Value::Constant(ScalarValue::I32(0)),
            value: Value::Constant(ScalarValue::F32(1.0)),
            bounds: crate::BoundsMode::Clamp,
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("read-only slice store should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("destination is read-only")));
}

#[test]
fn rejects_field_projection_on_a_scalar_place() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: vec![Projection::Field(FieldId::new(0))],
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("scalar field projection should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("field projection requires a struct")));
}

#[test]
fn rejects_index_projection_on_a_scalar_place() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: vec![Projection::Index {
                    index: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Checked,
                }],
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("scalar index projection should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("index projection requires an array")));
}

#[test]
fn rejects_missing_struct_field_projection() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Struct(crate::StructId::new(0)),
    ]);
    program.structs.push(StructType {
        name: "Pair".to_owned(),
        fields: vec![StructField {
            name: "first".to_owned(),
            ty: test_type(0),
        }],
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(1),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: vec![Projection::Field(FieldId::new(1))],
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("missing field projection should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("missing field 1")));
}

#[test]
fn rejects_readwrite_reference_to_a_readonly_place() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.params.push(Param {
        name: "gain".to_owned(),
        ty: test_type(0),
        default: ConstantValue::Scalar(ScalarValue::F32(1.0)),
        range: None,
        control: crate::ParamControl::default(),
    });
    let mut callee = function("mutate", FunctionKind::User);
    callee.params.push(FunctionParam {
        integer_range: None,
        name: "value".to_owned(),
        ty: test_type(0),
        mode: PassingMode::ReadWriteReference,
    });
    program.functions.push(callee);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: vec![CallArgument::Place(Place {
                base: PlaceBase::Param(ParamId::new(0)),
                projections: Vec::new(),
            })],
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors =
        super::validate(&program).expect_err("read-write reference escalation should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("argument 0")));
}

#[test]
fn rejects_store_to_readonly_interface_buffer() {
    let mut program = empty_program();
    program.interface.buffers.push(Buffer {
        name: "samples".to_owned(),
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadOnly,
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::BufferStore {
            buffer: crate::BufferRef::Direct(BufferId::new(0)),
            channel: None,
            index: Value::Constant(ScalarValue::I32(0)),
            value: Value::Constant(ScalarValue::F32(1.0)),
            bounds: crate::BoundsMode::Checked,
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("read-only buffer store should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("read-only interface buffer")));
}

#[test]
fn ordinary_validation_rejects_unchecked_buffer_collection_selectors() {
    let mut program = empty_program();
    program.interface.buffers.push(Buffer {
        name: "samples".to_owned(),
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadWrite,
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("frames".to_owned()),
        ty: TypeId::new(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::BufferLen(BufferRef::ArrayElement {
                first: BufferId::new(0),
                len: 1,
                selector: Value::Constant(ScalarValue::I32(0)),
                bounds: crate::BoundsMode::Unchecked,
            }),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program)
        .expect_err("unchecked descriptor selection requires producer proof");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("trusted MIR producer proof")));
    validate_producer(&program).expect("trusted producer selector is structurally valid");
}

#[test]
fn buffer_collection_references_require_homogeneous_descriptors() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.buffers.extend([
        Buffer {
            name: "float_samples".to_owned(),
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        },
        Buffer {
            name: "bool_samples".to_owned(),
            element: ScalarType::Bool,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        },
    ]);
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("sample".to_owned()),
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::BufferLoad {
                buffer: BufferRef::ArrayElement {
                    first: BufferId::new(0),
                    len: 2,
                    selector: Value::Constant(ScalarValue::I32(1)),
                    bounds: crate::BoundsMode::Clamp,
                },
                channel: None,
                index: Value::Constant(ScalarValue::I32(0)),
                bounds: crate::BoundsMode::Clamp,
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors =
        super::validate(&program).expect_err("mixed descriptor collection must fail validation");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("spans incompatible descriptors")));
}

#[test]
fn direct_buffers_in_user_functions_require_compiler_runtime_provenance() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.buffers.push(Buffer {
        name: "samples".to_owned(),
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadWrite,
    });
    let mut handler = function("delegate_handler", FunctionKind::User);
    handler.locals.push(Local {
        integer_range: None,
        name: Some("sample".to_owned()),
        ty: test_type(0),
    });
    handler.body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::Local(LocalId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::BufferLoad {
                buffer: BufferRef::Direct(BufferId::new(0)),
                channel: None,
                index: Value::Constant(ScalarValue::I32(0)),
                bounds: crate::BoundsMode::Clamp,
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    handler.attributes.runtime_context = true;
    program.functions.push(handler.clone());
    let errors = super::validate(&program)
        .expect_err("source user functions must not gain direct buffer capability");
    assert!(errors.iter().any(|error| error
        .message
        .contains("user functions must receive buffers")));

    handler.attributes.origin = crate::FunctionOrigin::CompilerGenerated;
    program.functions[2] = handler;
    super::validate(&program)
        .expect("compiler-generated runtime handlers may access direct host buffers");
}

#[test]
fn static_buffer_channels_must_fit_signed_byte_extents() {
    let mut program = empty_program();
    program.types.push(Type::Buffer {
        element: ScalarType::F32,
        channels: BufferChannels::Static((i32::MAX as u32 / 4) + 1),
        access: AccessMode::ReadWrite,
    });
    program.interface.buffers.push(Buffer {
        name: "huge".to_owned(),
        element: ScalarType::F64,
        channels: BufferChannels::Static((i32::MAX as u32 / 8) + 1),
        access: AccessMode::ReadWrite,
    });

    let errors = super::validate(&program).expect_err("oversized channels must fail");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("buffer byte-extent limit"))
            .count(),
        2
    );
}

#[test]
fn rejects_readonly_buffer_for_readwrite_buffer_parameter() {
    let mut program = empty_program();
    program.types.push(Type::Buffer {
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadWrite,
    });
    program.interface.buffers.push(Buffer {
        name: "samples".to_owned(),
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadOnly,
    });
    let mut callee = function("mutate_buffer", FunctionKind::User);
    callee.params.push(FunctionParam {
        integer_range: None,
        name: "samples".to_owned(),
        ty: test_type(0),
        mode: PassingMode::ReadWriteReference,
    });
    program.functions.push(callee);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: vec![CallArgument::Buffer(crate::BufferRef::Direct(
                BufferId::new(0),
            ))],
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors =
        super::validate(&program).expect_err("read-only buffer capability escalation should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("argument 0")));
}

#[test]
fn rejects_cast_from_a_non_scalar_value() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("view".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        },
    ]);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(1)),
            value: Rvalue::Cast {
                value: Value::Local(LocalId::new(0)),
                to: ScalarType::F32,
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("aggregate cast source should fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("cast source must be a numeric scalar")));
}

#[test]
fn rejects_casts_between_bool_and_numeric_types() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::I32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Cast {
                value: Value::Constant(ScalarValue::Bool(true)),
                to: ScalarType::I32,
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("bool cast should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("cast requires numeric scalar types")));
}

#[test]
fn rejects_wrong_intrinsic_arity() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Intrinsic {
                intrinsic: Intrinsic::Sin,
                args: Vec::new(),
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("missing intrinsic argument should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("sin") && error.message.contains("expects 1")));
}

#[test]
fn rejects_integer_argument_to_float_intrinsic() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::I32));
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: None,
        ty: test_type(0),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Intrinsic {
                intrinsic: Intrinsic::Sin,
                args: vec![Value::Constant(ScalarValue::I32(1))],
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("integer sin argument should fail");
    assert!(errors.iter().any(|error| {
        error.message.contains("sin") && error.message.contains("unsupported type i32")
    }));
}

#[test]
fn rejects_comparison_of_non_scalar_values() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::Bool),
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("left".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: Some("right".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: None,
            ty: test_type(0),
        },
    ]);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(2)),
            value: Rvalue::Compare {
                op: CompareOp::Equal,
                lhs: Value::Local(LocalId::new(0)),
                rhs: Value::Local(LocalId::new(1)),
            },
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("slice comparison should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("comparison operands must be scalar")));
}

#[test]
fn rejects_slice_copy_with_different_element_types() {
    let mut program = empty_program();
    program.types.extend([
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
        Type::Slice {
            element: ScalarType::I32,
            access: AccessMode::ReadOnly,
        },
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("destination".to_owned()),
            ty: test_type(0),
        },
        Local {
            integer_range: None,
            name: Some("source".to_owned()),
            ty: test_type(1),
        },
    ]);
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::SliceCopy {
            destination: Value::Local(LocalId::new(0)),
            source: Value::Local(LocalId::new(1)),
        },
        source: SourceSpan::UNKNOWN,
    });

    let errors = super::validate(&program).expect_err("converting slice copy should fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("identical element types")));
}

#[test]
fn rejects_local_not_assigned_on_every_fallthrough_branch() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::Bool),
        Type::Scalar(ScalarType::F32),
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("condition".to_owned()),
            ty: test_type(0),
        },
        Local {
            integer_range: None,
            name: Some("value".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: Some("copy".to_owned()),
            ty: test_type(1),
        },
    ]);
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Compare {
                    op: CompareOp::Equal,
                    lhs: Value::Constant(ScalarValue::I32(0)),
                    rhs: Value::Constant(ScalarValue::I32(1)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::If {
                condition: Value::Local(LocalId::new(0)),
                then_block: crate::Block {
                    statements: vec![Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(LocalId::new(1)),
                            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
                        },
                        source: SourceSpan::UNKNOWN,
                    }],
                },
                else_block: crate::Block::default(),
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(2)),
                value: Rvalue::Use(Value::Local(LocalId::new(1))),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    let errors = super::validate(&program).expect_err("partial assignment must fail");
    assert!(errors.iter().any(|error| {
        error.message.contains("value")
            && error.message.contains("before it is definitely assigned")
    }));
}

#[test]
fn accepts_local_assigned_on_both_fallthrough_branches() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::Bool),
        Type::Scalar(ScalarType::F32),
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("condition".to_owned()),
            ty: test_type(0),
        },
        Local {
            integer_range: None,
            name: Some("value".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: Some("copy".to_owned()),
            ty: test_type(1),
        },
    ]);
    let assignment = |value| Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(1)),
            value: Rvalue::Use(Value::Constant(ScalarValue::F32(value))),
        },
        source: SourceSpan::UNKNOWN,
    };
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Compare {
                    op: CompareOp::Equal,
                    lhs: Value::Constant(ScalarValue::I32(0)),
                    rhs: Value::Constant(ScalarValue::I32(1)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::If {
                condition: Value::Local(LocalId::new(0)),
                then_block: crate::Block {
                    statements: vec![assignment(1.0)],
                },
                else_block: crate::Block {
                    statements: vec![assignment(2.0)],
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(2)),
                value: Rvalue::Use(Value::Local(LocalId::new(1))),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    super::validate(&program).expect("both branches establish the local");
}

#[test]
fn rejects_process_frame_that_is_overwritten_before_audio_use() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.outputs.push(Output {
        name: "out1".to_owned(),
        ty: test_type(0),
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("frame".to_owned()),
        ty: TypeId::new(0),
    });
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::ProcessFrame {
                    offset: Value::Constant(ScalarValue::I32(0)),
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::OutputStore {
                output: OutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                frame: Value::Local(LocalId::new(0)),
                value: Value::Constant(ScalarValue::F32(0.0)),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    let errors = super::validate(&program).expect_err("overwritten frame must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("dominated by process_frame")));
}

#[test]
fn tracks_elementwise_fixed_array_initialization() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: test_type(0),
            len: 2,
        },
    ]);
    program.functions[1].locals.extend([
        Local {
            integer_range: None,
            name: Some("values".to_owned()),
            ty: test_type(1),
        },
        Local {
            integer_range: None,
            name: Some("index".to_owned()),
            ty: TypeId::new(0),
        },
        Local {
            integer_range: None,
            name: Some("value".to_owned()),
            ty: test_type(0),
        },
    ]);
    for index in 0..2 {
        program.functions[1].body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: vec![Projection::Index {
                        index: Value::Constant(ScalarValue::I32(index)),
                        bounds: crate::BoundsMode::Unchecked,
                    }],
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::F32(index as f32))),
            },
            source: SourceSpan::UNKNOWN,
        });
    }
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(1)),
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(2)),
                value: Rvalue::Load(Place {
                    base: PlaceBase::Local(LocalId::new(0)),
                    projections: vec![Projection::Index {
                        index: Value::Local(LocalId::new(1)),
                        bounds: crate::BoundsMode::Checked,
                    }],
                }),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    validate_producer(&program).expect("all fixed-array elements are initialized");

    program.functions[1].body.statements.remove(1);
    let errors = super::validate(&program).expect_err("dynamic read of a partial array must fail");
    assert!(errors.iter().any(|error| error
        .message
        .contains("indexed before it is definitely assigned")));
}

#[test]
fn validates_checked_slice_ranges_and_rejects_invalid_unchecked_constants() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: test_type(0),
            len: 4,
        },
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadOnly,
        },
    ]);
    program.state.push(StateSlot {
        integer_range: None,
        name: "values".to_owned(),
        ty: test_type(1),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
    });
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("view".to_owned()),
        ty: test_type(2),
    });
    program.functions[1].body.statements.push(Statement {
        kind: StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::MakeSlice {
                source: SliceSource::Place(Place {
                    base: PlaceBase::State(crate::StateId::new(0)),
                    projections: Vec::new(),
                }),
                start: Value::Constant(ScalarValue::I32(4)),
                len: Value::Constant(ScalarValue::I32(0)),
                bounds: crate::BoundsMode::Checked,
                access: AccessMode::ReadOnly,
            },
        },
        source: SourceSpan::UNKNOWN,
    });
    super::validate(&program).expect("one-past-end empty slice is valid");

    let make_slice = &mut program.functions[1].body.statements[0].kind;
    let StatementKind::Assign {
        value: Rvalue::MakeSlice { bounds, .. },
        ..
    } = make_slice
    else {
        unreachable!()
    };
    *bounds = crate::BoundsMode::Unchecked;
    let errors = super::validate(&program)
        .expect_err("ordinary validation must reject producer-proved unchecked access");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("trusted MIR producer proof")));
    unsafe { super::validate_with_producer_proofs(&program) }
        .expect("trusted producer validation accepts the proven empty range");

    let StatementKind::Assign {
        value: Rvalue::MakeSlice { len, .. },
        ..
    } = &mut program.functions[1].body.statements[0].kind
    else {
        unreachable!()
    };
    *len = Value::Constant(ScalarValue::I32(1));
    let errors = unsafe { super::validate_with_producer_proofs(&program) }
        .expect_err("trusted producers still reject statically invalid unchecked slices");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("statically out-of-bounds range")));
}

#[test]
fn control_outputs_use_explicit_one_to_one_mirror_identity() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.state.push(StateSlot {
        integer_range: None,
        name: "different_debug_name".to_owned(),
        ty: test_type(0),
        persistence: StatePersistence::ControlMirror,
        authored: true,
        pinned: false,
    });
    program
        .interface
        .control_outputs
        .push(crate::ControlOutput {
            name: "meter".to_owned(),
            ty: test_type(0),
            mirror: crate::StateId::new(0),
        });
    super::validate(&program).expect("mirror identity is ID-based, not name-based");

    program
        .interface
        .control_outputs
        .push(crate::ControlOutput {
            name: "other_meter".to_owned(),
            ty: test_type(0),
            mirror: crate::StateId::new(0),
        });
    let errors = super::validate(&program).expect_err("duplicate mirror must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("referenced by 2 control outputs")));
}

#[test]
fn rejects_duplicate_host_interface_names_and_event_parameter_names() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.interface.inputs.push(crate::Input {
        name: "shared".to_owned(),
        ty: test_type(0),
        default: None,
        range: None,
    });
    program.interface.params.push(Param {
        name: "shared".to_owned(),
        ty: test_type(0),
        default: ConstantValue::Scalar(ScalarValue::F32(0.0)),
        range: None,
        control: crate::ParamControl::default(),
    });
    program.interface.events.push(Event {
        name: "update".to_owned(),
        params: vec![
            EventParam {
                name: "value".to_owned(),
                ty: test_type(0),
                default: None,
            },
            EventParam {
                name: "value".to_owned(),
                ty: test_type(0),
                default: None,
            },
        ],
        handler: FunctionId::new(2),
    });
    program.functions.push(function(
        "onda_event::update",
        FunctionKind::Event(EventId::new(0)),
    ));

    let errors = super::validate(&program).expect_err("duplicate ABI names must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("interface name 'shared'")));
    assert!(errors
        .iter()
        .any(|error| error.message.contains("duplicate parameter name 'value'")));
}

#[test]
fn rejects_recursive_and_oversized_fixed_aggregate_types() {
    let mut recursive = empty_program();
    recursive.types.push(Type::Array {
        element: test_type(0),
        len: 1,
    });
    let errors = super::validate(&recursive).expect_err("recursive aggregate must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("recursive fixed aggregate")));

    let mut oversized = empty_program();
    oversized.types.extend([
        Type::Scalar(ScalarType::F64),
        Type::Array {
            element: test_type(0),
            len: i32::MAX as u32,
        },
    ]);
    let errors = super::validate(&oversized).expect_err("oversized aggregate must fail");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("logical size exceeds")));
}

#[test]
fn constant_data_logical_bytes_fit_the_signed_i32_boundary() {
    let maximum_f64_elements = i32::MAX as usize / 8;
    assert!(super::scalar_sequence_fits_i32_bytes(
        maximum_f64_elements,
        ScalarType::F64
    ));
    assert!(!super::scalar_sequence_fits_i32_bytes(
        maximum_f64_elements + 1,
        ScalarType::F64
    ));
}

#[test]
fn slice_element_is_scalar_only_and_fixed_arrays_use_explicit_windows() {
    let mut program = empty_program();
    program.types.extend([
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: test_type(0),
            len: 2,
        },
        Type::Slice {
            element: ScalarType::F32,
            access: AccessMode::ReadWrite,
        },
    ]);
    program.state.push(StateSlot {
        integer_range: None,
        name: "values".to_owned(),
        ty: test_type(1),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
    });
    let mut callee = function("consume_pair", FunctionKind::User);
    callee.params.push(FunctionParam {
        integer_range: None,
        name: "pair".to_owned(),
        ty: test_type(1),
        mode: PassingMode::ReadOnlyReference,
    });
    program.functions.push(callee);
    program.functions[1].locals.push(Local {
        integer_range: None,
        name: Some("view".to_owned()),
        ty: test_type(2),
    });
    program.functions[1].body.statements.extend([
        Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeSlice {
                    source: SliceSource::Place(Place {
                        base: PlaceBase::State(crate::StateId::new(0)),
                        projections: Vec::new(),
                    }),
                    start: Value::Constant(ScalarValue::I32(0)),
                    len: Value::Constant(ScalarValue::I32(2)),
                    bounds: crate::BoundsMode::Unchecked,
                    access: AccessMode::ReadWrite,
                },
            },
            source: SourceSpan::UNKNOWN,
        },
        Statement {
            kind: StatementKind::Call {
                results: Vec::new(),
                function: FunctionId::new(2),
                args: vec![CallArgument::SliceWindow {
                    slice: Value::Local(LocalId::new(0)),
                    start: Value::Constant(ScalarValue::I32(0)),
                    bounds: crate::BoundsMode::Checked,
                }],
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);
    validate_producer(&program).expect("slice window safely carries a fixed-array ref");

    let StatementKind::Call { args, .. } = &mut program.functions[1].body.statements[1].kind else {
        unreachable!()
    };
    args[0] = CallArgument::SliceElement {
        slice: Value::Local(LocalId::new(0)),
        index: Value::Constant(ScalarValue::I32(0)),
        bounds: crate::BoundsMode::Checked,
    };
    let errors = super::validate(&program).expect_err("slice element cannot impersonate an array");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("argument 0")));
}

#[test]
fn rejects_control_output_store_from_event_handler() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.state.push(StateSlot {
        integer_range: None,
        name: "meter".to_owned(),
        ty: test_type(0),
        persistence: StatePersistence::ControlMirror,
        authored: true,
        pinned: false,
    });
    program
        .interface
        .control_outputs
        .push(crate::ControlOutput {
            name: "meter".to_owned(),
            ty: test_type(0),
            mirror: crate::StateId::new(0),
        });
    program.interface.events.push(Event {
        name: "update".to_owned(),
        params: Vec::new(),
        handler: FunctionId::new(2),
    });
    let mut handler = function("onda_event::update", FunctionKind::Event(EventId::new(0)));
    handler.body.statements.push(Statement {
        kind: StatementKind::ControlOutputStore {
            output: crate::ControlOutputId::new(0),
            element: None,
            bounds: crate::BoundsMode::Unchecked,
            value: Value::Constant(ScalarValue::F32(1.0)),
        },
        source: SourceSpan::UNKNOWN,
    });
    program.functions.push(handler);

    let errors = super::validate(&program).expect_err("events cannot write control outputs");
    assert!(errors
        .iter()
        .any(|error| error.message.contains("only valid in the process")));
}

#[test]
fn control_mirrors_are_readable_but_only_control_stores_can_mutate_them() {
    let mut program = empty_program();
    program.types.push(Type::Scalar(ScalarType::F32));
    program.state.push(StateSlot {
        integer_range: None,
        name: "meter_mirror".to_owned(),
        ty: test_type(0),
        persistence: StatePersistence::ControlMirror,
        authored: true,
        pinned: false,
    });
    program
        .interface
        .control_outputs
        .push(crate::ControlOutput {
            name: "meter".to_owned(),
            ty: test_type(0),
            mirror: crate::StateId::new(0),
        });

    let direct_write = Statement {
        kind: StatementKind::Assign {
            destination: Place {
                base: PlaceBase::State(crate::StateId::new(0)),
                projections: Vec::new(),
            },
            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
        },
        source: SourceSpan::UNKNOWN,
    };
    program.functions[0]
        .body
        .statements
        .push(direct_write.clone());
    program.functions[1].body.statements.extend([
        direct_write.clone(),
        Statement {
            kind: StatementKind::ControlOutputStore {
                output: crate::ControlOutputId::new(0),
                element: None,
                bounds: crate::BoundsMode::Unchecked,
                value: Value::Constant(ScalarValue::F32(1.0)),
            },
            source: SourceSpan::UNKNOWN,
        },
    ]);

    let mut user = function("bad_user", FunctionKind::User);
    user.body.statements.push(direct_write.clone());
    program.functions.push(user);

    let handler_id = FunctionId::new(3);
    program.interface.events.push(Event {
        name: "bad_event".to_owned(),
        params: Vec::new(),
        handler: handler_id,
    });
    let mut event = function(
        "onda_event::bad_event",
        FunctionKind::Event(EventId::new(0)),
    );
    event.body.statements.push(direct_write);
    program.functions.push(event);

    let errors =
        super::validate(&program).expect_err("direct control-mirror writes must be rejected");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error
                .message
                .contains("assignment destination is not writable"))
            .count(),
        4
    );
    assert!(!errors.iter().any(|error| error
        .message
        .contains("control output stores are only valid")));
}
