use super::*;

fn array(index: Value) -> Place {
    Place {
        base: PlaceBase::Parameter(ParameterId::new(3)),
        projections: vec![Projection::Index {
            index,
            bounds: BoundsMode::Clamp,
        }],
    }
}

fn check_fixed(statements: Vec<Statement>, expected: u64) {
    let mut program = fixture(statements);
    program.types.push(Type::Array {
        element: TypeId::new(0),
        len: 16,
    });
    program.functions[0].params.push(FunctionParam {
        name: "array".into(),
        ty: TypeId::new(3),
        mode: PassingMode::ReadWriteReference,
        integer_range: None,
    });
    check_program(program, expected);
}

fn accesses(index: Value) -> Vec<Statement> {
    vec![
        assign(8, Rvalue::Load(array(index))),
        statement(StatementKind::Assign {
            destination: array(index),
            value: Rvalue::Use(int(1)),
        }),
        assign(
            8,
            Rvalue::NormalizeIndex {
                index,
                length: int(16),
                bounds: BoundsMode::Checked,
            },
        ),
    ]
}

#[test]
fn fixed_array_and_normalized_indices_use_short_circuit_guards() {
    for conjunction in [false, true] {
        let mut body = prologue();
        body.extend([
            assign(
                6,
                compare(
                    if conjunction {
                        CompareOp::GreaterEqual
                    } else {
                        CompareOp::Less
                    },
                    value(2),
                    int(0),
                ),
            ),
            branch(
                6,
                if conjunction {
                    vec![assign(7, compare(CompareOp::Less, value(2), int(16)))]
                } else {
                    vec![assign(
                        7,
                        Rvalue::Use(Value::Constant(ScalarValue::Bool(true))),
                    )]
                },
                if conjunction {
                    vec![assign(
                        7,
                        Rvalue::Use(Value::Constant(ScalarValue::Bool(false))),
                    )]
                } else {
                    vec![assign(
                        7,
                        compare(CompareOp::GreaterEqual, value(2), int(16)),
                    )]
                },
            ),
            branch(
                7,
                if conjunction {
                    accesses(value(2))
                } else {
                    vec![leave()]
                },
                vec![],
            ),
        ]);
        if !conjunction {
            body.extend(accesses(value(2)));
        }
        check_fixed(body, 3);
    }
}

#[test]
fn fixed_array_guards_do_not_survive_mutation_or_one_sided_tests() {
    for mutate in [false, true] {
        let mut guarded = vec![];
        if mutate {
            guarded.push(assign(2, Rvalue::Use(int(-1))));
        }
        guarded.extend(accesses(value(2)));
        if mutate {
            guarded = vec![
                assign(7, compare(CompareOp::GreaterEqual, value(2), int(0))),
                branch(7, guarded, vec![]),
            ];
        }
        let mut body = prologue();
        body.extend([
            assign(6, compare(CompareOp::Less, value(2), int(16))),
            branch(6, guarded, vec![]),
        ]);
        check_fixed(body, 0);
    }
}

#[test]
fn fixed_array_affine_proofs_require_nonwrapping_intermediates() {
    for amount in [1, i32::MAX] {
        let mut body = prologue();
        body.extend([
            assign(6, compare(CompareOp::GreaterEqual, value(2), int(0))),
            branch(
                6,
                vec![
                    assign(7, compare(CompareOp::Less, value(2), int(15))),
                    branch(
                        7,
                        vec![
                            assign(9, binary(BinaryOp::Add, value(2), int(amount))),
                            assign(9, binary(BinaryOp::Subtract, value(9), int(amount - 1))),
                            assign(8, Rvalue::Load(array(value(9)))),
                        ],
                        vec![],
                    ),
                ],
                vec![],
            ),
        ]);
        check_fixed(body, u64::from(amount == 1));
    }
}

#[test]
fn counted_loop_lower_bound_requires_an_unmodified_induction_variable() {
    for modify in 0..3 {
        let mut loop_body = vec![
            assign(6, compare(CompareOp::Less, value(2), int(16))),
            branch(6, vec![], vec![statement(StatementKind::Break)]),
        ];
        match modify {
            1 => loop_body.push(assign(2, Rvalue::Use(int(-1)))),
            2 => loop_body.extend([
                assign(11, compare(CompareOp::Equal, value(2), int(7))),
                branch(11, vec![assign(2, Rvalue::Use(int(-1)))], vec![]),
            ]),
            _ => {}
        }
        loop_body.extend(accesses(value(2)));
        loop_body.push(assign(2, binary(BinaryOp::Add, value(2), int(1))));
        check_fixed(
            vec![
                assign(2, Rvalue::Use(int(0))),
                statement(StatementKind::Loop {
                    body: Block {
                        statements: loop_body,
                    },
                }),
            ],
            if modify != 0 { 0 } else { 3 },
        );
    }
}

#[test]
fn unguarded_increments_do_not_gain_a_nonwrapping_lower_bound() {
    check_fixed(
        vec![
            assign(2, Rvalue::Use(int(17))),
            statement(StatementKind::Loop {
                body: Block {
                    statements: vec![
                        assign(6, compare(CompareOp::Less, value(2), int(16))),
                        branch(6, accesses(value(2)), vec![]),
                        assign(7, compare(CompareOp::Equal, value(2), int(15))),
                        branch(
                            7,
                            vec![statement(StatementKind::Break)],
                            vec![assign(2, binary(BinaryOp::Add, value(2), int(1)))],
                        ),
                    ],
                },
            }),
        ],
        0,
    );
}
