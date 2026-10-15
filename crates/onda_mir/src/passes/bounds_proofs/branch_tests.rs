use super::*;
use crate::{
    CompareOp, CompileConfig, ConstData, ConstDataId, FunctionAttributes, FunctionKind,
    FunctionParam, Local, LocalId, ParameterId, PassingMode, ScalarType, SourceSpan, Statement,
};

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        source: SourceSpan::UNKNOWN,
    }
}

fn assign(local: u32, value: Rvalue) -> Statement {
    statement(StatementKind::Assign {
        destination: Place::local(LocalId::new(local)),
        value,
    })
}

fn compare(local: u32, op: CompareOp, rhs: i32) -> Statement {
    assign(
        local,
        Rvalue::Compare {
            op,
            lhs: Value::Local(LocalId::new(0)),
            rhs: Value::Constant(ScalarValue::I32(rhs)),
        },
    )
}

fn load() -> Statement {
    assign(
        2,
        Rvalue::ConstDataLoad {
            data: ConstDataId::new(0),
            index: Value::Local(LocalId::new(0)),
            bounds: BoundsMode::Clamp,
        },
    )
}

fn branch(condition: u32, yes: Vec<Statement>, no: Vec<Statement>) -> Statement {
    statement(StatementKind::If {
        condition: Value::Local(LocalId::new(condition)),
        then_block: Block { statements: yes },
        else_block: Block { statements: no },
    })
}

fn guarded(statements: Vec<Statement>) -> Vec<Statement> {
    vec![
        compare(1, CompareOp::GreaterEqual, 0),
        branch(
            1,
            vec![
                compare(3, CompareOp::Less, 4),
                branch(3, statements, vec![]),
            ],
            vec![],
        ),
    ]
}

fn program(statements: Vec<Statement>) -> crate::Program {
    let mut program = crate::Program::new(
        CompileConfig {
            sample_rate: 48_000.0,
            block_size: 64,
        },
        FunctionId::new(0),
        FunctionId::new(0),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::Scalar(ScalarType::Bool),
    ];
    program.const_data.push(ConstData {
        name: "table".into(),
        element: ScalarType::I32,
        values: (0..4).map(ScalarValue::I32).collect(),
    });
    let mut body = vec![assign(
        0,
        Rvalue::Load(Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: vec![],
        }),
    )];
    body.extend(statements);
    program.functions.push(Function {
        name: "guarded".into(),
        kind: FunctionKind::User,
        attributes: FunctionAttributes::default(),
        params: vec![FunctionParam {
            name: "index".into(),
            ty: TypeId::new(0),
            mode: PassingMode::Value,
            integer_range: None,
        }],
        results: vec![],
        locals: [0, 1, 0, 1]
            .into_iter()
            .map(|ty| Local {
                name: None,
                ty: TypeId::new(ty),
                integer_range: None,
            })
            .collect(),
        body: Block { statements: body },
        source: SourceSpan::UNKNOWN,
    });
    program
}

fn bounds(block: &Block) -> Vec<BoundsMode> {
    let mut result = vec![];
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                value: Rvalue::ConstDataLoad { bounds, .. } | Rvalue::SliceLoad { bounds, .. },
                ..
            }
            | StatementKind::SliceStore { bounds, .. } => result.push(*bounds),
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                result.extend(bounds(then_block));
                result.extend(bounds(else_block));
            }
            StatementKind::Loop { body } => result.extend(bounds(body)),
            _ => {}
        }
    }
    result
}

fn optimized_bounds(statements: Vec<Statement>) -> Vec<BoundsMode> {
    let mut program = program(statements);
    eliminate_proven_bounds_checks(&mut program, &mut PassStats::default());
    bounds(&program.functions[0].body)
}

#[test]
fn nested_comparisons_prove_only_the_dominated_access() {
    let mut statements = guarded(vec![load()]);
    statements.push(load());
    assert_eq!(
        optimized_bounds(statements),
        [BoundsMode::Unchecked, BoundsMode::Clamp]
    );
}

#[test]
fn false_edges_and_reversed_comparisons_prove_bounds() {
    let reversed = assign(
        1,
        Rvalue::Compare {
            op: CompareOp::Greater,
            lhs: Value::Constant(ScalarValue::I32(0)),
            rhs: Value::Local(LocalId::new(0)),
        },
    );
    assert_eq!(
        optimized_bounds(vec![
            reversed,
            branch(
                1,
                vec![],
                vec![
                    compare(3, CompareOp::GreaterEqual, 4),
                    branch(3, vec![], vec![load()])
                ]
            )
        ]),
        [BoundsMode::Unchecked]
    );
}

#[test]
fn writes_invalidate_guard_facts_and_old_comparison_results() {
    let overwrite = assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(-1))));
    assert_eq!(
        optimized_bounds(guarded(vec![load(), overwrite.clone(), load()])),
        [BoundsMode::Unchecked, BoundsMode::Clamp]
    );
    assert_eq!(
        optimized_bounds(vec![
            compare(1, CompareOp::Equal, 1),
            overwrite,
            branch(1, vec![load()], vec![])
        ]),
        [BoundsMode::Clamp]
    );
}

#[test]
fn branch_joins_discard_facts_invalidated_on_one_path() {
    let overwrite = assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(-1))));
    let statements = guarded(vec![branch(1, vec![overwrite], vec![]), load()]);
    assert_eq!(optimized_bounds(statements), [BoundsMode::Clamp]);
}

#[test]
fn single_iteration_loops_keep_entry_facts_but_not_exit_facts() {
    let body = Block {
        statements: vec![
            load(),
            assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(-1)))),
            statement(StatementKind::Break),
        ],
    };
    assert_eq!(
        optimized_bounds(guarded(vec![
            statement(StatementKind::Loop { body }),
            load()
        ])),
        [BoundsMode::Unchecked, BoundsMode::Clamp]
    );
    let body = Block {
        statements: guarded(vec![load(), statement(StatementKind::Break)]),
    };
    assert_eq!(
        optimized_bounds(vec![statement(StatementKind::Loop { body })]),
        [BoundsMode::Unchecked]
    );
}

#[test]
fn a_terminating_guard_preserves_facts_on_the_surviving_path() {
    let done = statement(StatementKind::Return { values: vec![] });
    assert_eq!(
        optimized_bounds(vec![
            compare(1, CompareOp::Less, 0),
            branch(1, vec![done.clone()], vec![]),
            compare(3, CompareOp::GreaterEqual, 4),
            branch(3, vec![done], vec![]),
            load()
        ]),
        [BoundsMode::Unchecked]
    );
}

#[test]
fn unknown_calls_and_indirect_writes_forget_branch_facts() {
    let p = program(vec![]);
    let analysis = crate::analyze_integer_ranges(&p, FunctionId::new(0));
    let mut facts = BranchRanges::new(&p, &p.functions[0], &analysis);
    for mutation in [
        StatementKind::Call {
            function: FunctionId::new(0),
            args: vec![],
            results: vec![],
        },
        StatementKind::SliceStore {
            slice: Value::Local(LocalId::new(2)),
            index: Value::Constant(ScalarValue::I32(0)),
            value: Value::Constant(ScalarValue::I32(-1)),
            bounds: BoundsMode::Clamp,
        },
    ] {
        facts.observe(&compare(1, CompareOp::Equal, 1).kind);
        facts.assume(Value::Local(LocalId::new(1)), true);
        assert_eq!(facts.value(Value::Local(LocalId::new(0))).unwrap().min(), 1);
        facts.observe(&mutation);
        assert_eq!(
            facts.value(Value::Local(LocalId::new(0))).unwrap().min(),
            i64::from(i32::MIN)
        );
        facts.assume(Value::Local(LocalId::new(1)), true);
        assert_eq!(
            facts.value(Value::Local(LocalId::new(0))).unwrap().min(),
            i64::from(i32::MIN)
        );
    }
}

#[test]
fn guarded_integer_casts_propagate_only_when_value_preserving() {
    for width in [ScalarType::I32, ScalarType::I64] {
        let mut p = program(vec![]);
        p.types.push(Type::Scalar(width));
        p.functions[0].locals[2].ty = TypeId::new(2);
        let analysis = crate::analyze_integer_ranges(&p, FunctionId::new(0));
        let mut facts = BranchRanges::new(&p, &p.functions[0], &analysis);
        facts.observe(&compare(1, CompareOp::Equal, 2).kind);
        facts.assume(Value::Local(LocalId::new(1)), true);
        facts.observe(
            &assign(
                2,
                Rvalue::Cast {
                    value: Value::Local(LocalId::new(0)),
                    to: width,
                },
            )
            .kind,
        );
        let range = facts.value(Value::Local(LocalId::new(2))).unwrap();
        assert_eq!((range.scalar(), range.min(), range.max()), (width, 2, 2));
        facts.reset();
        facts.observe(
            &assign(
                0,
                Rvalue::Cast {
                    value: Value::Constant(ScalarValue::I64(i64::MAX)),
                    to: ScalarType::I32,
                },
            )
            .kind,
        );
        assert_eq!(
            facts.value(Value::Local(LocalId::new(0))).unwrap().min(),
            i64::from(i32::MIN)
        );
    }
}

fn increment(amount: i32) -> Statement {
    assign(
        0,
        Rvalue::Binary {
            op: crate::BinaryOp::Add,
            lhs: Value::Local(LocalId::new(0)),
            rhs: Value::Constant(ScalarValue::I32(amount)),
        },
    )
}

fn counted_loop(start: i32, op: CompareOp, limit: i32, body: Vec<Statement>) -> Vec<Statement> {
    let mut statements = vec![
        compare(1, op, limit),
        branch(1, vec![], vec![statement(StatementKind::Break)]),
    ];
    statements.extend(body);
    vec![
        assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(start)))),
        statement(StatementKind::Loop {
            body: Block { statements },
        }),
    ]
}

#[test]
fn inductive_loop_ranges_cover_forward_reverse_and_strided_updates() {
    for (start, op, limit, step) in [
        (0, CompareOp::Less, 4, 1),
        (0, CompareOp::Less, 4, 3),
        (3, CompareOp::GreaterEqual, 0, -1),
        (3, CompareOp::GreaterEqual, 0, -2),
    ] {
        assert_eq!(
            optimized_bounds(counted_loop(
                start,
                op,
                limit,
                vec![load(), increment(step)]
            )),
            [BoundsMode::Unchecked],
        );
    }
}

#[test]
fn loop_proofs_include_continue_edges_and_conditional_writes() {
    for terminal in [StatementKind::Continue, StatementKind::Break] {
        let body = vec![
            load(),
            compare(3, CompareOp::Equal, 2),
            branch(3, vec![increment(-10), statement(terminal.clone())], vec![]),
            increment(1),
        ];
        let expected = if matches!(terminal, StatementKind::Continue) {
            BoundsMode::Clamp
        } else {
            BoundsMode::Unchecked
        };
        assert_eq!(
            optimized_bounds(counted_loop(0, CompareOp::Less, 4, body)),
            [expected],
        );
    }
}

#[test]
fn loop_proofs_reject_overflow_and_out_of_range_initial_values() {
    for (start, step) in [(-1, 1), (0, i32::MAX), (0, -1)] {
        assert_eq!(
            optimized_bounds(counted_loop(
                start,
                CompareOp::Less,
                4,
                vec![load(), increment(step)]
            )),
            [BoundsMode::Clamp],
        );
    }
}

#[test]
fn derived_indices_use_the_interval_at_the_assignment() {
    for (op, rhs, expected) in [
        (crate::BinaryOp::Add, 1, BoundsMode::Unchecked),
        (crate::BinaryOp::Subtract, -1, BoundsMode::Unchecked),
        (crate::BinaryOp::Multiply, 2, BoundsMode::Unchecked),
        (crate::BinaryOp::Add, i32::MAX, BoundsMode::Clamp),
        (crate::BinaryOp::Subtract, 1, BoundsMode::Clamp),
    ] {
        let derive = assign(
            2,
            Rvalue::Binary {
                op,
                lhs: Value::Local(LocalId::new(0)),
                rhs: Value::Constant(ScalarValue::I32(rhs)),
            },
        );
        let access = assign(
            2,
            Rvalue::ConstDataLoad {
                data: ConstDataId::new(0),
                index: Value::Local(LocalId::new(2)),
                bounds: BoundsMode::Clamp,
            },
        );
        assert_eq!(
            optimized_bounds(counted_loop(
                0,
                CompareOp::Less,
                2,
                vec![derive, access, increment(1)]
            )),
            [expected],
        );
    }
}

#[test]
fn loop_calls_and_nested_mutations_do_not_preserve_entry_only_facts() {
    let nested = statement(StatementKind::Loop {
        body: Block {
            statements: vec![increment(-10), statement(StatementKind::Break)],
        },
    });
    let call = statement(StatementKind::Call {
        function: FunctionId::new(0),
        args: vec![],
        results: vec![],
    });
    for mutation in [nested, call] {
        assert_eq!(
            optimized_bounds(counted_loop(
                0,
                CompareOp::Less,
                4,
                vec![load(), mutation, increment(1)]
            )),
            [BoundsMode::Clamp],
        );
    }
}

fn slice_program(statements: Vec<Statement>) -> crate::Program {
    let mut p = program(vec![]);
    p.types.push(Type::Slice {
        element: ScalarType::I32,
        access: crate::AccessMode::ReadWrite,
    });
    p.functions[0].params.push(FunctionParam {
        name: "values".into(),
        ty: TypeId::new(2),
        mode: PassingMode::Value,
        integer_range: None,
    });
    for ty in [2, 0, 2, 0, 0] {
        p.functions[0].locals.push(Local {
            name: None,
            ty: TypeId::new(ty),
            integer_range: None,
        });
    }
    p.functions[0].body.statements.extend([
        assign(
            4,
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(1)),
                projections: vec![],
            }),
        ),
        assign(5, Rvalue::SliceLen(Value::Local(LocalId::new(4)))),
    ]);
    p.functions[0].body.statements.extend(statements);
    p
}

fn slice_guard(body: Vec<Statement>) -> Vec<Statement> {
    vec![
        compare(1, CompareOp::GreaterEqual, 0),
        branch(
            1,
            vec![
                assign(
                    3,
                    Rvalue::Compare {
                        op: CompareOp::Less,
                        lhs: Value::Local(LocalId::new(0)),
                        rhs: Value::Local(LocalId::new(5)),
                    },
                ),
                branch(3, body, vec![]),
            ],
            vec![],
        ),
    ]
}

fn slice_load(slice: u32, index: u32) -> Statement {
    assign(
        2,
        Rvalue::SliceLoad {
            slice: Value::Local(LocalId::new(slice)),
            index: Value::Local(LocalId::new(index)),
            bounds: BoundsMode::Clamp,
        },
    )
}

fn slice_store(slice: u32) -> Statement {
    statement(StatementKind::SliceStore {
        slice: Value::Local(LocalId::new(slice)),
        index: Value::Local(LocalId::new(0)),
        value: Value::Constant(ScalarValue::I32(99)),
        bounds: BoundsMode::Clamp,
    })
}

fn prove_slice_bounds(mut p: crate::Program) -> Vec<BoundsMode> {
    eliminate_proven_bounds_checks(&mut p, &mut PassStats::default());
    bounds(&p.functions[0].body)
}

#[test]
fn dynamic_slice_guards_cover_reads_writes_and_value_copies() {
    assert_eq!(
        prove_slice_bounds(slice_program(slice_guard(vec![
            slice_load(4, 0),
            slice_store(4),
            slice_load(4, 0),
            assign(6, Rvalue::Use(Value::Local(LocalId::new(4)))),
            assign(7, Rvalue::Use(Value::Local(LocalId::new(5)))),
            assign(8, Rvalue::Use(Value::Local(LocalId::new(0)))),
            slice_load(6, 8),
        ]))),
        vec![BoundsMode::Unchecked; 4]
    );
    // An upper bound alone does not exclude negative indices.
    assert_eq!(
        prove_slice_bounds(slice_program(vec![
            assign(
                3,
                Rvalue::Compare {
                    op: CompareOp::Less,
                    lhs: Value::Local(LocalId::new(0)),
                    rhs: Value::Local(LocalId::new(5))
                }
            ),
            branch(3, vec![slice_load(4, 0)], vec![]),
        ])),
        [BoundsMode::Clamp]
    );
}

#[test]
fn dynamic_slice_relations_are_invalidated_by_each_operand_write_and_branch_join() {
    for mutation in [
        assign(0, Rvalue::Use(Value::Local(LocalId::new(5)))),
        assign(5, Rvalue::Use(Value::Local(LocalId::new(0)))),
        assign(
            4,
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(1)),
                projections: vec![],
            }),
        ),
    ] {
        assert_eq!(
            prove_slice_bounds(slice_program(slice_guard(vec![
                slice_load(4, 0),
                mutation.clone(),
                slice_load(4, 0),
            ]))),
            [BoundsMode::Unchecked, BoundsMode::Clamp]
        );
        assert_eq!(
            prove_slice_bounds(slice_program(slice_guard(vec![
                branch(1, vec![mutation], vec![]),
                slice_load(4, 0),
            ]))),
            [BoundsMode::Clamp]
        );
    }
}

#[test]
fn slice_store_through_a_copied_descriptor_invalidates_an_aliased_scalar_index() {
    let mut p = slice_program(vec![]);
    p.functions[0].body.statements.push(assign(
        4,
        Rvalue::MakeSlice {
            source: crate::SliceSource::Place(Place::local(LocalId::new(0))),
            start: Value::Constant(ScalarValue::I32(0)),
            len: Value::Constant(ScalarValue::I32(1)),
            access: crate::AccessMode::ReadWrite,
            bounds: BoundsMode::Checked,
        },
    ));
    p.functions[0].body.statements.extend([
        assign(6, Rvalue::Use(Value::Local(LocalId::new(4)))),
        assign(5, Rvalue::SliceLen(Value::Local(LocalId::new(4)))),
    ]);
    p.functions[0].body.statements.extend(slice_guard(vec![
        slice_load(4, 0),
        slice_store(6),
        slice_load(4, 0),
    ]));
    assert_eq!(
        prove_slice_bounds(p),
        [BoundsMode::Unchecked, BoundsMode::Clamp, BoundsMode::Clamp]
    );
}

#[test]
fn dynamic_slice_loop_headers_drop_relations_changed_on_a_backedge() {
    for mutation in [
        assign(0, Rvalue::Use(Value::Local(LocalId::new(5)))),
        assign(5, Rvalue::Use(Value::Local(LocalId::new(0)))),
        assign(
            4,
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(1)),
                projections: vec![],
            }),
        ),
    ] {
        let p = slice_program(slice_guard(vec![statement(StatementKind::Loop {
            body: Block {
                statements: vec![
                    slice_load(4, 0),
                    mutation,
                    statement(StatementKind::Continue),
                ],
            },
        })]));
        assert_eq!(prove_slice_bounds(p), [BoundsMode::Clamp]);
    }
}

#[test]
fn normalized_length_copies_do_not_describe_the_original_slice() {
    let mut p = slice_program(vec![
        assign(7, Rvalue::Use(Value::Local(LocalId::new(5)))),
        compare(1, CompareOp::GreaterEqual, 0),
        branch(
            1,
            vec![
                assign(
                    3,
                    Rvalue::Compare {
                        op: CompareOp::Less,
                        lhs: Value::Local(LocalId::new(0)),
                        rhs: Value::Local(LocalId::new(7)),
                    },
                ),
                branch(3, vec![slice_load(4, 0)], vec![]),
            ],
            vec![],
        ),
    ]);
    p.functions[0].locals[7].integer_range = Some(crate::IntegerRangeInvariant {
        min: ScalarValue::I32(100),
        max: ScalarValue::I32(200),
        mode: crate::IntegerRangeMode::Clamp,
    });
    assert_eq!(prove_slice_bounds(p), [BoundsMode::Clamp]);
}
