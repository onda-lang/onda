use super::*;

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

fn copy(local: u32) -> Statement {
    assign(local, Rvalue::Use(Value::Local(LocalId::new(0))))
}

fn compare(local: u32, op: CompareOp, constant: i32) -> Statement {
    assign(
        3,
        Rvalue::Compare {
            op,
            lhs: Value::Local(LocalId::new(local)),
            rhs: Value::Constant(ScalarValue::I32(constant)),
        },
    )
}

fn branch(then_block: Vec<Statement>, else_block: Vec<Statement>) -> Statement {
    statement(StatementKind::If {
        condition: Value::Local(LocalId::new(4)),
        then_block: Block {
            statements: then_block,
        },
        else_block: Block {
            statements: else_block,
        },
    })
}

fn function() -> Function {
    Function {
        name: "predicate_proof".into(),
        kind: FunctionKind::User,
        attributes: Default::default(),
        params: vec![],
        results: vec![],
        locals: (0..5)
            .map(|i| Local {
                name: None,
                ty: TypeId::new(if i < 3 { 0 } else { 1 }),
                integer_range: None,
            })
            .collect(),
        body: Block::default(),
        source: SourceSpan::UNKNOWN,
    }
}

fn folds(statements: Vec<Statement>) -> Vec<bool> {
    let mut function = function();
    function.body.statements = statements;
    let summaries = [Predicates::new()];
    let facts = FunctionFacts::new(&function);
    let mut scan = Scan::new(&function, &facts, &summaries, Some(8));
    let mut locals = vec![None; function.locals.len()];
    locals[0] = Some(Symbol::Index(Origin::Induction));
    scan.statements(&function.body.statements, &mut locals);
    scan.folds.iter().map(|fold| fold.value).collect()
}

fn view(local: u32) -> Statement {
    assign(
        5,
        Rvalue::MakeSlice {
            source: SliceSource::Place(Place::local(LocalId::new(local))),
            start: Value::Constant(ScalarValue::I32(0)),
            len: Value::Constant(ScalarValue::I32(1)),
            bounds: BoundsMode::Checked,
            access: AccessMode::ReadWrite,
        },
    )
}

fn indirect_write(value: i32) -> Statement {
    statement(StatementKind::SliceStore {
        slice: Value::Local(LocalId::new(5)),
        index: Value::Constant(ScalarValue::I32(0)),
        value: Value::Constant(ScalarValue::I32(value)),
        bounds: BoundsMode::Checked,
    })
}

fn aliased_loop(local: u32, escape_before_loop: bool) -> Program {
    let mut program = Program::new(
        CompileConfig::new(48_000.0, 32).unwrap(),
        FunctionId::new(0),
        FunctionId::new(1),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::Scalar(ScalarType::Bool),
        Type::Slice {
            element: ScalarType::I32,
            access: AccessMode::ReadWrite,
        },
    ];
    let mut init = function();
    init.kind = FunctionKind::Init;
    init.locals.clear();
    let mut process = function();
    process.kind = FunctionKind::Process;
    process.params = process_function_params(TypeId::new(0));
    process.locals.push(Local {
        name: None,
        ty: TypeId::new(2),
        integer_range: None,
    });
    if escape_before_loop {
        process.body.statements.push(view(local));
    }
    process
        .body
        .statements
        .push(assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(0)))));
    let mut body = vec![copy(1)];
    if !escape_before_loop {
        body.push(view(local));
    }
    body.extend([
        // Mutating the induction variable ends the loop immediately; mutating
        // its copy makes the first-index predicate true on every iteration.
        indirect_write(if local == 0 { 31 } else { 0 }),
        compare(local, CompareOp::Equal, 0),
        statement(StatementKind::If {
            condition: Value::Local(LocalId::new(3)),
            then_block: Block::default(),
            else_block: Block::default(),
        }),
        compare(0, CompareOp::Equal, 31),
        statement(StatementKind::If {
            condition: Value::Local(LocalId::new(3)),
            then_block: Block {
                statements: vec![statement(StatementKind::Break)],
            },
            else_block: Block {
                statements: vec![assign(
                    0,
                    Rvalue::Binary {
                        op: BinaryOp::Add,
                        lhs: Value::Local(LocalId::new(0)),
                        rhs: Value::Constant(ScalarValue::I32(1)),
                    },
                )],
            },
        }),
    ]);
    process.body.statements.push(statement(StatementKind::Loop {
        body: Block { statements: body },
    }));
    program.functions = vec![init, process];
    program
}

#[test]
fn indirect_writes_to_copies_and_induction_do_not_prove_first_index_predicates() {
    for local in [0, 1] {
        for before_loop in [false, true] {
            let program = validate_owned(aliased_loop(local, before_loop)).unwrap();
            assert!(
                Plan::new(&program).is_empty(),
                "local={local}, escape_before_loop={before_loop}"
            );
        }
    }
}

#[test]
fn folds_require_current_copies_on_every_incoming_path() {
    assert_eq!(
        folds(vec![
            // A previous iteration's copy is unknown at the next iteration's entry.
            compare(1, CompareOp::Equal, 8),
            copy(1),
            compare(1, CompareOp::Equal, 8),
            compare(1, CompareOp::NotEqual, 8),
            compare(1, CompareOp::Equal, 9),
            compare(1, CompareOp::Less, 8),
            // A copy overwritten on either branch no longer proves identity.
            branch(
                vec![assign(1, Rvalue::Use(Value::Constant(ScalarValue::I32(8))))],
                vec![]
            ),
            compare(1, CompareOp::Equal, 8),
            // Both paths reestablish identity; nested preorder positions also matter.
            branch(
                vec![copy(1), compare(1, CompareOp::Equal, 8)],
                vec![copy(1)]
            ),
            compare(1, CompareOp::Equal, 8),
        ]),
        vec![false, true, false, false]
    );
}

#[test]
fn calls_invalidate_referenced_copies_and_result_locals() {
    for call in [
        statement(StatementKind::Call {
            function: FunctionId::new(0),
            args: vec![CallArgument::Place(Place::local(LocalId::new(1)))],
            results: vec![],
        }),
        statement(StatementKind::Call {
            function: FunctionId::new(0),
            args: vec![],
            results: vec![LocalId::new(1)],
        }),
    ] {
        assert!(folds(vec![copy(1), call, compare(1, CompareOp::Equal, 8)]).is_empty());
    }
}

#[test]
fn reference_escapes_and_mutations_reject_the_induction_proof() {
    let index = LocalId::new(0);
    for change in [
        assign(0, Rvalue::Use(Value::Constant(ScalarValue::I32(8)))),
        statement(StatementKind::Call {
            function: FunctionId::new(0),
            args: vec![CallArgument::Place(Place::local(index))],
            results: vec![],
        }),
        statement(StatementKind::Call {
            function: FunctionId::new(0),
            args: vec![],
            results: vec![index],
        }),
        branch(vec![statement(StatementKind::Continue)], vec![]),
    ] {
        assert!(!unchanged_induction(&[change], index));
    }
}

#[test]
fn folding_positions_preserve_other_assignments_and_branch_structure() {
    let function = function();
    let summaries = [Predicates::new()];
    let facts = FunctionFacts::new(&function);
    let mut scan = Scan::new(&function, &facts, &summaries, Some(8));
    let mut locals = vec![None; function.locals.len()];
    locals[0] = Some(Symbol::Index(Origin::Induction));
    let mut block = Block {
        statements: vec![
            copy(1),
            branch(
                vec![compare(1, CompareOp::Equal, 8)],
                vec![compare(1, CompareOp::NotEqual, 8)],
            ),
            compare(1, CompareOp::Equal, 9),
        ],
    };
    let mut expected = block.clone();
    let StatementKind::If {
        then_block,
        else_block,
        ..
    } = &mut expected.statements[1].kind
    else {
        unreachable!()
    };
    then_block.statements[0] = assign(3, Rvalue::Use(Value::Constant(ScalarValue::Bool(false))));
    else_block.statements[0] = assign(3, Rvalue::Use(Value::Constant(ScalarValue::Bool(true))));
    scan.statements(&block.statements, &mut locals);
    let mut stats = super::super::PassStats::default();
    fold_remainder(
        &mut block,
        &mut scan.folds.iter().peekable(),
        &mut 0,
        &mut stats,
    );
    assert_eq!(block, expected);
    assert_eq!(stats.folded_rvalues, 2);
}

#[test]
fn only_identity_values_authorize_reversed_comparisons() {
    for (value, expected) in [
        (Rvalue::Load(Place::local(LocalId::new(0))), vec![false]),
        (
            Rvalue::NormalizeIndex {
                index: Value::Local(LocalId::new(0)),
                length: Value::Constant(ScalarValue::I32(64)),
                bounds: BoundsMode::Unchecked,
            },
            vec![false],
        ),
        (
            Rvalue::NormalizeIndex {
                index: Value::Local(LocalId::new(0)),
                length: Value::Constant(ScalarValue::I32(64)),
                bounds: BoundsMode::Clamp,
            },
            vec![],
        ),
    ] {
        let reversed = assign(
            3,
            Rvalue::Compare {
                op: CompareOp::Equal,
                lhs: Value::Constant(ScalarValue::I32(8)),
                rhs: Value::Local(LocalId::new(1)),
            },
        );
        assert_eq!(folds(vec![assign(1, value), reversed]), expected);
    }
}
