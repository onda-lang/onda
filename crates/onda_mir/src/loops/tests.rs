use super::*;

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}

fn assign(local: LocalId, value: Rvalue) -> Statement {
    statement(StatementKind::Assign {
        destination: Place::local(local),
        value,
    })
}

fn repeated_loops(count: usize) -> Program {
    let mut program = Program::new(
        CompileConfig::new(48000.0, 128).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::Scalar(ScalarType::Bool),
        Type::Scalar(ScalarType::F32),
        Type::Array {
            element: TypeId::new(2),
            len: 128,
        },
    ];
    program.state.push(StateSlot {
        name: "values".into(),
        ty: TypeId::new(3),
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
        integer_range: None,
    });
    for (name, kind) in [
        ("init", FunctionKind::Init),
        ("process", FunctionKind::Process),
    ] {
        program.functions.push(Function {
            name: name.into(),
            kind,
            attributes: FunctionAttributes {
                runtime_context: true,
                ..Default::default()
            },
            params: if kind == FunctionKind::Process {
                process_function_params(TypeId::new(0))
            } else {
                vec![]
            },
            results: vec![],
            locals: vec![],
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        });
    }
    let function = &mut program.functions[1];
    for index in 0..count {
        let induction = LocalId::new((2 * index) as u32);
        let condition = LocalId::new((2 * index + 1) as u32);
        function
            .locals
            .extend([TypeId::new(0), TypeId::new(1)].map(|ty| Local {
                name: None,
                ty,
                integer_range: None,
            }));
        function.body.statements.push(assign(
            induction,
            Rvalue::Use(Value::Constant(ScalarValue::I32(0))),
        ));
        function
            .body
            .statements
            .push(statement(StatementKind::Loop {
                body: Block {
                    statements: vec![
                        assign(
                            condition,
                            Rvalue::Compare {
                                op: CompareOp::Less,
                                lhs: Value::Local(induction),
                                rhs: Value::Constant(ScalarValue::I32(128)),
                            },
                        ),
                        statement(StatementKind::If {
                            condition: Value::Local(condition),
                            then_block: Block::default(),
                            else_block: Block {
                                statements: vec![statement(StatementKind::Break)],
                            },
                        }),
                        statement(StatementKind::Assign {
                            destination: Place {
                                base: PlaceBase::State(StateId::new(0)),
                                projections: vec![Projection::Index {
                                    index: Value::Local(induction),
                                    bounds: BoundsMode::Clamp,
                                }],
                            },
                            value: Rvalue::Use(Value::Constant(ScalarValue::F32(1.0))),
                        }),
                        assign(
                            induction,
                            Rvalue::Binary {
                                op: BinaryOp::Add,
                                lhs: Value::Local(induction),
                                rhs: Value::Constant(ScalarValue::I32(1)),
                            },
                        ),
                    ],
                },
            }));
    }
    program
}

fn check(program: &Program, expected: &[bool]) {
    validate(program).unwrap();
    let effects = analyze_effects(program);
    let memory = analyze_memory(program);
    let ranges = analyze_program_integer_ranges_with_effects(program, &effects);
    let analysis = LoopAnalysis::new(program, &effects, &memory, &ranges, FunctionId::new(1));
    let mut pending = vec![&program.functions[1].body];
    let mut loops = Vec::new();
    while let Some(block) = pending.pop() {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Loop { body } => loops.push(body),
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => pending.extend([else_block, then_block]),
                _ => {}
            }
        }
    }
    assert_eq!(loops.len(), expected.len());
    // Each proof starts with the same immutable descriptor facts, irrespective
    // of prior queries or the order in which a backend emits the loops.
    for order in [false, true] {
        let mut queries: Vec<_> = loops.iter().zip(expected).collect();
        if order {
            queries.reverse();
        }
        for (body, &expected) in queries {
            assert_eq!(analysis.independent_memory_loop(body), expected);
        }
    }
}

#[test]
fn many_loop_queries_share_one_function_traversal() {
    for count in [32, 128, 512, 2048] {
        let program = repeated_loops(count);
        check(&program, &vec![true; count]);
        let facts = facts::FunctionFacts::new(&program.functions[1]);
        assert_eq!(facts.visits, 7 * count);
        for statement in &program.functions[1].body.statements {
            if let StatementKind::Loop { body } = &statement.kind {
                let counted = counted_loop(body).unwrap();
                assert_eq!(facts.preceding_reset(body), Some(0));
                assert!(facts.locals[counted.index.index()].unit_induction());
            }
        }
    }
}

#[test]
fn resets_belong_to_the_immediately_following_loop_in_the_same_block() {
    let mut program = repeated_loops(2);
    program.functions[1].body.statements.swap(0, 2);
    let statements = &mut program.functions[1].body.statements;
    statements.insert(0, statements[2].clone());
    check(&program, &[false, false]);

    let mut program = repeated_loops(2);
    let statements = &mut program.functions[1].body.statements;
    let second = statements.split_off(2);
    let first_loop = statements.pop().unwrap();
    statements.push(statement(StatementKind::If {
        condition: Value::Constant(ScalarValue::Bool(true)),
        then_block: Block {
            statements: vec![first_loop],
        },
        else_block: Block { statements: second },
    }));
    check(&program, &[false, true]);
}

#[test]
fn reusing_an_induction_local_preserves_each_loops_memory_proof() {
    let mut program = repeated_loops(1);
    let repeated = program.functions[1].body.statements.clone();
    program.functions[1].body.statements.extend(repeated);
    check(&program, &[true, true]);
    let facts = facts::FunctionFacts::new(&program.functions[1]);
    assert!(!facts.locals[0].unit_induction());
}
