use onda_mir::*;

fn function(name: &str, kind: FunctionKind) -> Function {
    Function {
        name: name.into(),
        kind,
        attributes: FunctionAttributes::default(),
        params: Vec::new(),
        results: Vec::new(),
        locals: Vec::new(),
        body: Block::default(),
        source: SourceSpan::UNKNOWN,
    }
}
fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}
fn parameter(ty: u32) -> FunctionParam {
    FunctionParam {
        name: "resources".into(),
        ty: TypeId::new(ty),
        mode: PassingMode::Value,
        integer_range: None,
    }
}
fn mapped_program() -> Program {
    let mut program = Program::new(
        CompileConfig::new(48_000.0, 64).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::BufferMap {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
            len: 3,
        },
        Type::BufferMap {
            element: ScalarType::F32,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
            len: 2,
        },
    ];
    program.interface.buffers = [ScalarType::F32, ScalarType::I32, ScalarType::F32]
        .into_iter()
        .enumerate()
        .map(|(index, element)| Buffer {
            name: format!("buffer{index}"),
            element,
            channels: BufferChannels::Mono,
            access: AccessMode::ReadWrite,
        })
        .collect();
    program.buffer_maps.push(BufferMap {
        buffers: vec![BufferId::new(2), BufferId::new(0), BufferId::new(2)],
    });
    let init = function("init", FunctionKind::Init);
    let mut process = function("process", FunctionKind::Process);
    process.params = process_function_params(TypeId::new(0));
    process.body.statements.push(statement(StatementKind::Call {
        results: Vec::new(),
        function: FunctionId::new(2),
        args: vec![CallArgument::BufferSpan(BufferSpanRef::MappedInterface {
            map: BufferMapId::new(0),
            start: 0,
            len: 3,
        })],
    }));
    let mut parent = function("forward", FunctionKind::User);
    parent.params.push(parameter(1));
    parent.body.statements.push(statement(StatementKind::Call {
        results: Vec::new(),
        function: FunctionId::new(3),
        args: vec![CallArgument::BufferSpan(BufferSpanRef::Parameter {
            span: ParameterId::new(0),
            start: 1,
            len: 2,
        })],
    }));
    let mut child = function("write", FunctionKind::User);
    child.params.push(parameter(2));
    child
        .body
        .statements
        .push(statement(StatementKind::BufferParamStore {
            parameter: BufferParamRef::ArrayElement {
                span: ParameterId::new(0),
                selector: Value::Constant(ScalarValue::I32(1)),
                bounds: BoundsMode::Clamp,
            },
            channel: None,
            index: Value::Constant(ScalarValue::I32(0)),
            value: Value::Constant(ScalarValue::F32(17.0)),
            bounds: BoundsMode::Clamp,
        }));
    program.functions = vec![init, process, parent, child];
    program
}

#[test]
fn mapped_resources_keep_exact_write_origins_through_subviews_and_transports() {
    let program = mapped_program();
    validate(&program).unwrap();
    assert_eq!(
        analyze_buffer_writes(&program).unwrap().buffers(),
        &[false, false, true]
    );
    let json = to_json(&program).unwrap();
    let decoded = from_json_program(&json).unwrap();
    assert_eq!(decoded, program);
    let packed = to_messagepack(&program).unwrap();
    let decoded = from_messagepack(&packed).unwrap();
    assert_eq!(decoded.as_program(), &program);
    let (optimized, _) = optimize(validate_owned(program).unwrap()).unwrap();
    assert_eq!(
        analyze_buffer_writes(&optimized).unwrap().buffers(),
        &[false, false, true]
    );
}

#[test]
fn dynamic_map_selection_unions_only_the_mapped_resources() {
    let mut program = mapped_program();
    let child = &mut program.functions[3];
    child.locals.push(Local {
        name: None,
        ty: TypeId::new(0),
        integer_range: None,
    });
    child.body.statements.insert(
        0,
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::BufferParamSamples(BufferParamRef::ArrayElement {
                span: ParameterId::new(0),
                selector: Value::Constant(ScalarValue::I32(0)),
                bounds: BoundsMode::Clamp,
            }),
        }),
    );
    let StatementKind::BufferParamStore { parameter, .. } = &mut child.body.statements[1].kind
    else {
        unreachable!()
    };
    let BufferParamRef::ArrayElement { selector, .. } = parameter else {
        unreachable!()
    };
    *selector = Value::Local(LocalId::new(0));
    validate(&program).unwrap();
    assert_eq!(
        analyze_buffer_writes(&program).unwrap().buffers(),
        &[true, false, true]
    );
}

#[test]
fn rejects_invalid_map_resources_windows_permissions_and_layouts() {
    for mutate in [
        |p: &mut Program| p.buffer_maps[0].buffers[2] = BufferId::new(99),
        |p: &mut Program| p.buffer_maps[0].buffers[2] = BufferId::new(1),
        |p: &mut Program| p.interface.buffers[2].access = AccessMode::ReadOnly,
        |p: &mut Program| p.interface.buffers[2].channels = BufferChannels::Static(2),
        |p: &mut Program| {
            p.types[2] = Type::BufferSpan {
                element: ScalarType::F32,
                channels: BufferChannels::Mono,
                access: AccessMode::ReadWrite,
                len: 2,
            }
        },
        |p: &mut Program| {
            let StatementKind::Call { args, .. } = &mut p.functions[1].body.statements[0].kind
            else {
                unreachable!()
            };
            args[0] = CallArgument::BufferSpan(BufferSpanRef::MappedInterface {
                map: BufferMapId::new(99),
                start: 0,
                len: 3,
            });
        },
        |p: &mut Program| {
            let StatementKind::Call { args, .. } = &mut p.functions[2].body.statements[0].kind
            else {
                unreachable!()
            };
            args[0] = CallArgument::BufferSpan(BufferSpanRef::Parameter {
                span: ParameterId::new(0),
                start: u32::MAX,
                len: 2,
            });
        },
    ] {
        let mut program = mapped_program();
        mutate(&mut program);
        assert!(validate(&program).is_err());
    }
}

#[test]
fn write_analysis_reports_a_missing_map_without_panicking() {
    let mut program = mapped_program();
    program.buffer_maps.clear();
    assert!(analyze_buffer_writes(&program).is_err());
}

#[test]
fn rejects_overflowing_interface_buffer_spans() {
    let mut program = mapped_program();
    for ty in &mut program.types {
        if let Type::BufferMap {
            element,
            channels,
            access,
            len,
        } = *ty
        {
            *ty = Type::BufferSpan {
                element,
                channels,
                access,
                len,
            };
        }
    }
    program.interface.buffers[1].element = ScalarType::F32;
    let set_span = |program: &mut Program, first| {
        let StatementKind::Call { args, .. } = &mut program.functions[1].body.statements[0].kind
        else {
            unreachable!()
        };
        args[0] = CallArgument::BufferSpan(BufferSpanRef::Interface {
            first: BufferId::new(first),
            len: 3,
        });
    };
    set_span(&mut program, 0);
    validate(&program).expect("a contiguous homogeneous interface span is valid");
    set_span(&mut program, u32::MAX);
    assert!(
        validate(&program).is_err(),
        "overflow cannot produce an empty valid span"
    );
}

#[test]
fn rejects_noninteger_local_buffer_call_selectors() {
    let mut program = mapped_program();
    let buffer_ty = TypeId::new(program.types.len() as u32);
    program.types.push(Type::Buffer {
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadWrite,
    });
    let mut consume = function("consume", FunctionKind::User);
    consume.params.push(FunctionParam {
        name: "buffer".into(),
        ty: buffer_ty,
        mode: PassingMode::ReadWriteReference,
        integer_range: None,
    });
    let consume_id = FunctionId::new(program.functions.len() as u32);
    program.functions.push(consume);
    let process = &mut program.functions[1];
    process.locals.push(Local {
        name: None,
        ty: TypeId::new(1),
        integer_range: None,
    });
    process.body.statements.extend([
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::MakeBuffer {
                source: CallArgument::BufferSpan(BufferSpanRef::MappedInterface {
                    map: BufferMapId::new(0),
                    start: 0,
                    len: 3,
                }),
                ty: TypeId::new(1),
            },
        }),
        statement(StatementKind::Call {
            results: Vec::new(),
            function: consume_id,
            args: vec![CallArgument::BufferParam(BufferParamRef::LocalElement {
                collection: LocalId::new(0),
                selector: Value::Constant(ScalarValue::I32(0)),
                bounds: BoundsMode::Clamp,
            })],
        }),
    ]);
    validate(&program).expect("an integer selector into an initialized local map is valid");
    for invalid in [ScalarValue::F32(0.0), ScalarValue::Bool(false)] {
        let StatementKind::Call { args, .. } = &mut program.functions[1]
            .body
            .statements
            .last_mut()
            .unwrap()
            .kind
        else {
            unreachable!()
        };
        let CallArgument::BufferParam(BufferParamRef::LocalElement { selector, .. }) = &mut args[0]
        else {
            unreachable!()
        };
        *selector = Value::Constant(invalid);
        assert!(
            validate(&program).is_err(),
            "buffer selectors must have integer type"
        );
    }
}

#[test]
fn local_map_windows_preserve_exact_origins_through_optimization() {
    let mut program = mapped_program();
    let parent = &mut program.functions[2];
    parent.locals.push(Local {
        name: None,
        ty: TypeId::new(1),
        integer_range: None,
    });
    parent.body.statements.insert(
        0,
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::MakeBuffer {
                source: CallArgument::BufferSpan(BufferSpanRef::Parameter {
                    span: ParameterId::new(0),
                    start: 0,
                    len: 3,
                }),
                ty: TypeId::new(1),
            },
        }),
    );
    let StatementKind::Call { args, .. } = &mut parent.body.statements[1].kind else {
        unreachable!()
    };
    args[0] = CallArgument::BufferSpan(BufferSpanRef::Local {
        collection: LocalId::new(0),
        start: Value::Constant(ScalarValue::I32(1)),
        len: 2,
        bounds: BoundsMode::Checked,
    });
    validate(&program).unwrap();
    assert_eq!(
        analyze_buffer_writes(&program).unwrap().buffers(),
        &[false, false, true]
    );
    let (optimized, _) = optimize(validate_owned(program).unwrap()).unwrap();
    assert_eq!(
        analyze_buffer_writes(&optimized).unwrap().buffers(),
        &[false, false, true]
    );
}

#[test]
fn branch_joined_scalar_descriptors_have_precise_effects_and_cannot_be_snapshotted() {
    let mut program = mapped_program();
    let buffer_ty = TypeId::new(program.types.len() as u32);
    program.types.push(Type::Buffer {
        element: ScalarType::F32,
        channels: BufferChannels::Mono,
        access: AccessMode::ReadWrite,
    });
    let bool_ty = TypeId::new(program.types.len() as u32);
    program.types.push(Type::Scalar(ScalarType::Bool));
    let process = &mut program.functions[1];
    process.locals.extend([
        Local {
            name: None,
            ty: buffer_ty,
            integer_range: None,
        },
        Local {
            name: None,
            ty: bool_ty,
            integer_range: None,
        },
    ]);
    let capture = |buffer| Block {
        statements: vec![statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(0)),
            value: Rvalue::MakeBuffer {
                source: CallArgument::Buffer(BufferRef::Direct(BufferId::new(buffer))),
                ty: buffer_ty,
            },
        })],
    };
    process.body.statements = vec![
        statement(StatementKind::Assign {
            destination: Place::local(LocalId::new(1)),
            value: Rvalue::Compare {
                op: CompareOp::Greater,
                lhs: Value::Constant(ScalarValue::I32(1)),
                rhs: Value::Constant(ScalarValue::I32(0)),
            },
        }),
        statement(StatementKind::If {
            condition: Value::Local(LocalId::new(1)),
            then_block: capture(0),
            else_block: capture(2),
        }),
        statement(StatementKind::BufferParamStore {
            parameter: BufferParamRef::Local(LocalId::new(0)),
            channel: None,
            index: Value::Constant(ScalarValue::I32(0)),
            value: Value::Constant(ScalarValue::F32(7.0)),
            bounds: BoundsMode::Clamp,
        }),
    ];
    validate(&program).unwrap();
    assert_eq!(
        analyze_buffer_writes(&program).unwrap().buffers(),
        &[true, false, true]
    );
    assert_eq!(
        from_json_program(&to_json(&program).unwrap()).unwrap(),
        program
    );
    program.state.push(StateSlot {
        name: "invalid_descriptor".into(),
        ty: buffer_ty,
        persistence: StatePersistence::Snapshot,
        authored: false,
        pinned: false,
        integer_range: None,
    });
    assert!(validate(&program).is_err());
}
