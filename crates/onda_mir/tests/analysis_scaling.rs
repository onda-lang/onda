use onda_mir::*;

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}

fn function(name: String, kind: FunctionKind) -> Function {
    Function {
        name,
        kind,
        attributes: FunctionAttributes {
            runtime_context: true,
            ..Default::default()
        },
        params: vec![],
        results: vec![],
        locals: vec![],
        body: Block::default(),
        source: SourceSpan::UNKNOWN,
    }
}

fn chain(n: usize) -> Program {
    let ty = TypeId::new(0);
    let mut p = Program::new(
        CompileConfig::new(48000.0, 128).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    p.types.push(Type::Scalar(ScalarType::I32));
    p.state.push(StateSlot {
        name: "state".into(),
        ty,
        persistence: StatePersistence::Snapshot,
        authored: true,
        pinned: false,
        integer_range: None,
    });
    p.functions
        .push(function("init".into(), FunctionKind::Init));
    let mut process = function("process".into(), FunctionKind::Process);
    process.params = process_function_params(ty);
    process.locals.push(Local {
        name: None,
        ty,
        integer_range: None,
    });
    process.body.statements.push(statement(StatementKind::Call {
        results: vec![LocalId::new(0)],
        function: FunctionId::new(2),
        args: vec![],
    }));
    p.functions.push(process);
    for i in 0..n {
        let mut f = function(format!("f{i}"), FunctionKind::User);
        f.results.push(ty);
        if i + 1 < n {
            f.locals.push(Local {
                name: None,
                ty,
                integer_range: None,
            });
            f.body.statements.push(statement(StatementKind::Call {
                results: vec![LocalId::new(0)],
                function: FunctionId::new(i as u32 + 3),
                args: vec![],
            }));
            f.body.statements.push(statement(StatementKind::Return {
                values: vec![Value::Local(LocalId::new(0))],
            }));
        } else {
            f.body.statements.push(statement(StatementKind::Assign {
                destination: Place {
                    base: PlaceBase::State(StateId::new(0)),
                    projections: vec![],
                },
                value: Rvalue::Use(Value::Constant(ScalarValue::I32(1))),
            }));
            f.body.statements.push(statement(StatementKind::Return {
                values: vec![Value::Constant(ScalarValue::I32(1))],
            }));
        }
        p.functions.push(f);
    }
    p
}

#[test]
fn call_chain_analysis_visits_scale_linearly() {
    for size in [32, 128, 512] {
        let program = chain(size);
        validate(&program).unwrap();
        let effects = analyze_effects(&program);
        let ranges = analyze_program_integer_ranges_with_effects(&program, &effects);
        assert!(!effects.function(FunctionId::new(2)).writes.is_empty());
        assert!(
            effects.function_visits <= 3 * program.functions.len(),
            "effects: {} visits for {size} functions",
            effects.function_visits
        );
        assert!(
            ranges.function_visits <= 4 * program.functions.len(),
            "ranges: {} visits for {size} functions",
            ranges.function_visits
        );
        let (optimized, _) = optimize(validate_owned(program).unwrap()).unwrap();
        assert_eq!(optimized.effects(), &analyze_effects(&optimized));
        assert_eq!(
            optimized.ranges(),
            &analyze_program_integer_ranges_with_effects(&optimized, optimized.effects())
        );
        assert_eq!(optimized.memory(), &analyze_memory(&optimized));
    }
}

#[test]
fn reference_origins_flow_through_long_forwarding_chains() {
    for size in [32, 128, 512] {
        let mut program = chain(size);
        for (index, function) in program.functions.iter_mut().enumerate().skip(1) {
            if index > 1 {
                function.params.push(FunctionParam {
                    name: "storage".into(),
                    ty: TypeId::new(0),
                    mode: PassingMode::ReadOnlyReference,
                    integer_range: None,
                });
            }
            if let StatementKind::Call { args, .. } = &mut function.body.statements[0].kind {
                args.push(CallArgument::Place(Place {
                    base: if index == 1 {
                        PlaceBase::State(StateId::new(0))
                    } else {
                        PlaceBase::Parameter(ParameterId::new(0))
                    },
                    projections: vec![],
                }));
            }
        }
        validate(&program).unwrap();
        let memory = analyze_memory(&program);
        let origins = memory.parameter(FunctionId::new((size + 1) as u32), ParameterId::new(0));
        assert_eq!(
            origins.only_states().unwrap().collect::<Vec<_>>(),
            vec![StateId::new(0)]
        );
        assert!(
            memory.function_visits <= 3 * program.functions.len(),
            "{} visits for {size} functions",
            memory.function_visits
        );
    }
}

#[test]
fn window_offsets_and_extents_flow_through_long_forwarding_chains() {
    for size in [32, 128, 512] {
        let mut program = chain(size);
        for remaining in (1024 - size..=1024).rev() {
            program.types.push(Type::Array {
                element: TypeId::new(0),
                len: remaining as u32,
            });
        }
        program.state[0].ty = TypeId::new(1);
        for (index, function) in program.functions.iter_mut().enumerate().skip(1) {
            if index > 1 {
                function.params.push(FunctionParam {
                    name: "storage".into(),
                    ty: TypeId::new(index as u32),
                    mode: PassingMode::ReadOnlyReference,
                    integer_range: None,
                });
            }
            if let StatementKind::Call { args, .. } = &mut function.body.statements[0].kind {
                args.push(CallArgument::ArrayWindow {
                    array: Place {
                        base: if index == 1 {
                            PlaceBase::State(StateId::new(0))
                        } else {
                            PlaceBase::Parameter(ParameterId::new(0))
                        },
                        projections: vec![],
                    },
                    start: Value::Constant(ScalarValue::I32(1)),
                    bounds: BoundsMode::Checked,
                });
            } else {
                function.body.statements.remove(0);
            }
        }
        validate(&program).unwrap();
        let memory = analyze_memory(&program);
        let regions = memory
            .parameter(FunctionId::new((size + 1) as u32), ParameterId::new(0))
            .state_regions()
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(
            regions,
            [StateRegion {
                state: StateId::new(0),
                start: size as u32,
                len: (1024 - size) as u32
            }]
        );
        assert!(memory.function_visits <= 3 * program.functions.len());
    }
}

#[test]
fn alternative_views_remain_bounded_across_long_forwarding_chains() {
    for alternatives in [2, 256] {
        let size = 128;
        let mut program = chain(size);
        program.types.extend([
            Type::Array {
                element: TypeId::new(0),
                len: 4096,
            },
            Type::Array {
                element: TypeId::new(0),
                len: 4,
            },
        ]);
        program.state[0].ty = TypeId::new(1);
        for (index, function) in program.functions.iter_mut().enumerate().skip(2) {
            function.params.push(FunctionParam {
                name: "view".into(),
                ty: TypeId::new(2),
                mode: PassingMode::ReadOnlyReference,
                integer_range: None,
            });
            if index + 1 < size + 2 {
                let StatementKind::Call { args, .. } = &mut function.body.statements[0].kind else {
                    unreachable!()
                };
                args.push(CallArgument::Place(Place {
                    base: PlaceBase::Parameter(ParameterId::new(0)),
                    projections: vec![],
                }));
            } else {
                function.body.statements.remove(0);
            }
        }
        program.functions[1].body.statements = (0..alternatives)
            .map(|offset| {
                statement(StatementKind::Call {
                    function: FunctionId::new(2),
                    results: vec![LocalId::new(0)],
                    args: vec![CallArgument::ArrayWindow {
                        array: Place {
                            base: PlaceBase::State(StateId::new(0)),
                            projections: vec![],
                        },
                        start: Value::Constant(ScalarValue::I32(offset * 8)),
                        bounds: BoundsMode::Checked,
                    }],
                })
            })
            .collect();
        validate(&program).unwrap();
        let memory = analyze_memory(&program);
        let origins = memory.parameter(FunctionId::new((size + 1) as u32), ParameterId::new(0));
        assert_eq!(
            origins.only_states().unwrap().collect::<Vec<_>>(),
            [StateId::new(0)]
        );
        if alternatives == 2 {
            assert_eq!(
                origins
                    .state_regions()
                    .unwrap()
                    .map(|r| (r.start, r.len))
                    .collect::<Vec<_>>(),
                [(0, 4), (8, 4)]
            );
        } else {
            assert!(origins.state_regions().is_none());
        }
        assert!(memory.function_visits <= 3 * program.functions.len());
    }
}

#[test]
fn shared_helper_merges_all_callers_without_revisiting_each_edge_per_update() {
    let size = 256;
    let mut program = chain(size);
    let leaf = FunctionId::new((size + 1) as u32);
    let ty = TypeId::new(0);
    for (index, caller) in program.functions[2..size + 1].iter_mut().enumerate() {
        let StatementKind::Call { function, args, .. } = &mut caller.body.statements[0].kind else {
            unreachable!()
        };
        *function = leaf;
        args.push(CallArgument::Value(Value::Constant(ScalarValue::I32(
            index as i32,
        ))));
    }
    let helper = &mut program.functions[leaf.index()];
    helper.params.push(FunctionParam {
        name: "value".into(),
        ty,
        mode: PassingMode::Value,
        integer_range: None,
    });
    helper.locals.push(Local {
        name: None,
        ty,
        integer_range: None,
    });
    helper.body.statements[1] = statement(StatementKind::Assign {
        destination: Place {
            base: PlaceBase::Local(LocalId::new(0)),
            projections: vec![],
        },
        value: Rvalue::Load(Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: vec![],
        }),
    });
    helper
        .body
        .statements
        .push(statement(StatementKind::Return {
            values: vec![Value::Local(LocalId::new(0))],
        }));
    for index in 3..size + 1 {
        program.functions[1]
            .body
            .statements
            .push(statement(StatementKind::Call {
                results: vec![LocalId::new(0)],
                function: FunctionId::new(index as u32),
                args: vec![],
            }));
    }
    validate(&program).unwrap();
    let ranges = analyze_program_integer_ranges(&program);
    let range = ranges
        .function(leaf)
        .unwrap()
        .parameter(ParameterId::new(0))
        .unwrap();
    assert_eq!((range.min(), range.max()), (0, size as i64 - 2));
    assert!(ranges.function_visits <= 4 * program.functions.len());
}
