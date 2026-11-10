use super::*;
use onda_mir::{
    BinaryOp, BoundsMode, CompileConfig, Function, FunctionAttributes, FunctionId, FunctionParam,
    InlineHint, Local, LocalId, ParameterId, PassingMode, PlaceBase, ScalarValue, SourceSpan,
    StateId, StatePersistence, StateSlot, Statement, TypeId,
};

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}

fn function(name: &str, kind: FunctionKind) -> Function {
    Function {
        name: name.to_owned(),
        kind,
        attributes: FunctionAttributes {
            inline: InlineHint::Never,
            ..Default::default()
        },
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: Block::default(),
        source: SourceSpan::UNKNOWN,
    }
}

fn window_program(length: u32, bounds: BoundsMode, additional_offset: Option<i32>) -> Program {
    let mut program = Program::new(
        CompileConfig::new(48000.0, 128).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: TypeId::new(1),
            len: length,
        },
        Type::Array {
            element: TypeId::new(1),
            len: 4,
        },
    ];
    program.state.push(StateSlot {
        name: "values".to_owned(),
        ty: TypeId::new(2),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
        integer_range: None,
    });
    let mut process = function("process", FunctionKind::Process);
    process.params = onda_mir::process_function_params(TypeId::new(0));
    process.locals = vec![
        Local {
            name: None,
            ty: TypeId::new(0),
            integer_range: None
        };
        2
    ];
    process.body.statements = vec![
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(0)),
                projections: Vec::new(),
            }),
        }),
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(1)),
            value: Rvalue::Binary {
                op: BinaryOp::Multiply,
                lhs: Value::Local(LocalId::new(0)),
                rhs: Value::Constant(ScalarValue::I32(4)),
            },
        }),
        statement(StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: vec![CallArgument::ArrayWindow {
                array: Place {
                    base: PlaceBase::State(StateId::new(0)),
                    projections: Vec::new(),
                },
                start: Value::Local(LocalId::new(1)),
                bounds,
            }],
        }),
    ];
    if let Some(offset) = additional_offset {
        process.body.statements.push(statement(StatementKind::Call {
            results: Vec::new(),
            function: FunctionId::new(2),
            args: vec![CallArgument::ArrayWindow {
                array: Place {
                    base: PlaceBase::State(StateId::new(0)),
                    projections: Vec::new(),
                },
                start: Value::Constant(ScalarValue::I32(offset)),
                bounds: BoundsMode::Checked,
            }],
        }));
    }
    let mut write = function("write_window", FunctionKind::User);
    write.params.push(FunctionParam {
        name: "values".to_owned(),
        ty: TypeId::new(3),
        mode: PassingMode::ReadWriteReference,
        integer_range: None,
    });
    write.body.statements.push(statement(StatementKind::Assign {
        destination: Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: vec![Projection::Index {
                index: Value::Constant(ScalarValue::I32(0)),
                bounds: BoundsMode::Checked,
            }],
        },
        value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
    }));
    program.functions = vec![function("init", FunctionKind::Init), process, write];
    program
}

#[test]
fn dynamic_tensor_windows_expose_only_their_guaranteed_reference_alignment() {
    for (length, bounds, extra, alignment) in [
        (128, BoundsMode::Clamp, None, 16),
        (130, BoundsMode::Clamp, None, 8),
        (129, BoundsMode::Clamp, None, 4),
        (129, BoundsMode::Checked, None, 16),
        (128, BoundsMode::Checked, Some(2), 8),
    ] {
        let program = window_program(length, bounds, extra);
        onda_mir::validate(&program).unwrap();
        let ir = lower_mir_to_llvm_ir_with_options(
            &program,
            MirCompileOptions {
                math: Default::default(),
                fast_math: false,
                opt_level: TargetOptLevel::O0,
            },
        )
        .unwrap();
        let definition = ir
            .lines()
            .find(|line| line.starts_with("define internal") && line.contains("@__onda_mir_fn_2("))
            .expect("window helper definition");
        assert!(
            definition.contains(&format!("align {alignment} ")),
            "{definition}"
        );
        assert!(!definition.contains("noalias"), "{definition}");
    }
}
