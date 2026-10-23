use super::*;
use crate::{
    AccessMode, CompileConfig, FunctionAttributes, FunctionKind, FunctionParam, Local, ParameterId,
    PassingMode, Program, SourceSpan, Statement,
};

fn value(index: u32) -> Value {
    Value::Local(LocalId::new(index))
}
fn int(value: i32) -> Value {
    Value::Constant(ScalarValue::I32(value))
}
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
fn compare(op: CompareOp, lhs: Value, rhs: Value) -> Rvalue {
    Rvalue::Compare { op, lhs, rhs }
}
fn branch(condition: u32, yes: Vec<Statement>, no: Vec<Statement>) -> Statement {
    statement(StatementKind::If {
        condition: value(condition),
        then_block: Block { statements: yes },
        else_block: Block { statements: no },
    })
}
fn read(slice: u32, index: u32) -> Statement {
    assign(
        8,
        Rvalue::SliceLoad {
            slice: value(slice),
            index: value(index),
            bounds: BoundsMode::Clamp,
        },
    )
}
fn write(slice: u32, index: u32) -> Statement {
    statement(StatementKind::SliceStore {
        slice: value(slice),
        index: value(index),
        value: int(7),
        bounds: BoundsMode::Checked,
    })
}
fn leave() -> Statement {
    statement(StatementKind::Return { values: vec![] })
}

fn prologue() -> Vec<Statement> {
    let mut body = (0..3)
        .map(|parameter| {
            assign(
                parameter,
                Rvalue::Load(Place {
                    base: PlaceBase::Parameter(ParameterId::new(parameter)),
                    projections: vec![],
                }),
            )
        })
        .collect::<Vec<_>>();
    body.extend([
        assign(3, Rvalue::SliceLen(value(0))),
        assign(4, Rvalue::SliceLen(value(1))),
        assign(
            5,
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::Min,
                args: vec![value(3), value(4)],
            },
        ),
    ]);
    body
}

fn fixture(statements: Vec<Statement>) -> Program {
    let mut program = Program::new(
        CompileConfig {
            sample_rate: 48000.0,
            block_size: 128,
        },
        FunctionId::new(0),
        FunctionId::new(0),
    );
    program.types = vec![
        Type::Scalar(ScalarType::I32),
        Type::Scalar(ScalarType::Bool),
        Type::Slice {
            element: ScalarType::I32,
            access: AccessMode::ReadWrite,
        },
    ];
    program.functions.push(Function {
        name: "slice_bounds".into(),
        kind: FunctionKind::User,
        attributes: FunctionAttributes::default(),
        params: [2, 2, 0]
            .into_iter()
            .enumerate()
            .map(|(i, ty)| FunctionParam {
                name: format!("p{i}"),
                ty: TypeId::new(ty),
                mode: PassingMode::Value,
                integer_range: None,
            })
            .collect(),
        results: vec![],
        locals: [2, 2, 0, 0, 0, 0, 1, 1, 0, 0, 2, 1]
            .into_iter()
            .map(|ty| Local {
                name: None,
                ty: TypeId::new(ty),
                integer_range: None,
            })
            .collect(),
        body: Block { statements },
        source: SourceSpan::UNKNOWN,
    });
    program
}

fn check(statements: Vec<Statement>, expected: u64) {
    check_program(fixture(statements), expected);
}

fn check_program(mut program: Program, expected: u64) {
    let ranges = crate::analyze_program_integer_ranges(&program);
    let memory = crate::analyze_memory(&program);
    let mut body = std::mem::take(&mut program.functions[0].body);
    let context = Context {
        program: &program,
        function: &program.functions[0],
        ranges: ranges.function(FunctionId::new(0)).unwrap(),
        slice_lengths: vec![None; 12],
        memory: &memory,
        function_id: FunctionId::new(0),
    };
    assert_eq!(eliminate(&context, &mut body), expected);
    assert_eq!(
        eliminate(&context, &mut body),
        0,
        "proof pass must be idempotent"
    );
}

mod fixed;

#[test]
fn branch_guards_prove_both_sides_of_a_minimum_length() {
    let mut body = prologue();
    body.extend([
        assign(6, compare(CompareOp::GreaterEqual, value(2), int(0))),
        branch(
            6,
            vec![
                assign(7, compare(CompareOp::Less, value(2), value(5))),
                branch(7, vec![read(0, 2), write(1, 2)], vec![]),
            ],
            vec![],
        ),
    ]);
    check(body, 2);
}

fn binary(op: crate::BinaryOp, lhs: Value, rhs: Value) -> Rvalue {
    Rvalue::Binary { op, lhs, rhs }
}

#[test]
fn derived_indices_preserve_dynamic_length_relationships() {
    use crate::BinaryOp::{Add, Subtract};
    let mut reverse = prologue();
    reverse.extend([
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(5))),
        branch(
            7,
            vec![
                assign(9, binary(Subtract, value(3), int(1))),
                assign(9, binary(Subtract, value(9), value(2))),
                read(0, 9),
            ],
            vec![],
        ),
    ]);
    check(reverse, 1);

    let mut shifted = prologue();
    shifted.extend([
        assign(3, binary(Subtract, value(3), int(1))),
        assign(
            5,
            Rvalue::Intrinsic {
                intrinsic: Intrinsic::Min,
                args: vec![value(3), value(4)],
            },
        ),
        // Computing the index before testing the guard is safe to prove too:
        // the guard describes the same value identities.
        assign(9, binary(Add, value(2), int(1))),
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(5))),
        branch(7, vec![read(0, 9), write(1, 2)], vec![]),
    ]);
    check(shifted, 2);
}

#[test]
fn affine_proofs_do_not_assume_nonwrapping_arithmetic() {
    use crate::BinaryOp::{Add, Subtract};
    for (op, amount) in [(Add, 1), (Subtract, 1)] {
        let mut body = prologue();
        body.extend([
            assign(6, compare(CompareOp::Less, value(2), int(0))),
            branch(6, vec![leave()], vec![]),
            assign(7, compare(CompareOp::LessEqual, value(2), value(3))),
            branch(
                7,
                vec![assign(9, binary(op, value(2), int(amount))), read(0, 9)],
                vec![],
            ),
        ]);
        // +1 can overflow and exceed the length; -1 can be negative.
        check(body, 0);
    }
    let mut body = prologue();
    body.extend([
        assign(9, binary(Subtract, value(3), int(1))),
        assign(9, binary(Subtract, value(9), value(2))),
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(3))),
        assign(2, Rvalue::Use(int(0))),
        branch(7, vec![read(0, 9)], vec![]),
    ]);
    // The saved predicates still describe the old value; replacing the local
    // does not invalidate a valid proof of an already-computed index.
    check(body, 1);
}

#[test]
fn conditional_expression_ranges_do_not_escape_their_definitions() {
    let mut body = prologue();
    body.extend([
        assign(6, compare(CompareOp::GreaterEqual, value(2), int(0))),
        branch(
            6,
            vec![
                assign(
                    9,
                    Rvalue::Intrinsic {
                        intrinsic: Intrinsic::Min,
                        args: vec![value(2), int(0)],
                    },
                ),
                assign(9, binary(crate::BinaryOp::Add, value(9), int(1))),
            ],
            vec![],
        ),
        assign(7, compare(CompareOp::Less, value(2), value(3))),
        branch(7, vec![read(0, 2)], vec![]),
    ]);
    check(body, 0);
}

#[test]
fn exiting_guards_and_boolean_negation_preserve_facts() {
    for exit in [
        leave(),
        statement(StatementKind::Break),
        statement(StatementKind::Continue),
    ] {
        let mut body = prologue();
        body.extend([
            assign(6, compare(CompareOp::Less, value(2), int(0))),
            branch(6, vec![exit.clone()], vec![]),
            assign(7, compare(CompareOp::Less, value(2), value(5))),
            assign(
                11,
                Rvalue::Unary {
                    op: UnaryOp::LogicalNot,
                    operand: value(7),
                },
            ),
            branch(11, vec![exit], vec![]),
            assign(9, Rvalue::Use(value(2))),
            read(0, 9),
            write(1, 9),
        ]);
        check(body, 2);
    }
}

#[test]
fn inclusive_bounds_negative_indices_and_unrelated_lengths_keep_checks() {
    for (lower, upper, limit, expected) in [
        (true, CompareOp::LessEqual, 5, 0),
        (false, CompareOp::Less, 5, 0),
        (true, CompareOp::Less, 3, 1),
        (true, CompareOp::Less, 4, 1),
    ] {
        let mut body = prologue();
        if lower {
            body.extend([
                assign(6, compare(CompareOp::Less, value(2), int(0))),
                branch(6, vec![leave()], vec![]),
            ]);
        }
        body.extend([
            assign(7, compare(upper, value(2), value(limit))),
            branch(7, vec![read(0, 2), write(1, 2)], vec![]),
        ]);
        check(body, expected);
    }
}

#[test]
fn guards_do_not_survive_index_or_descriptor_rebinding() {
    for mutation in [
        assign(2, Rvalue::Use(int(1000))),
        assign(
            0,
            Rvalue::Load(Place {
                base: PlaceBase::Parameter(ParameterId::new(1)),
                projections: vec![],
            }),
        ),
    ] {
        let mut body = prologue();
        body.extend([
            assign(6, compare(CompareOp::Less, value(2), int(0))),
            branch(6, vec![leave()], vec![]),
            assign(7, compare(CompareOp::Less, value(2), value(3))),
            branch(7, vec![mutation, read(0, 2)], vec![]),
        ]);
        check(body, 0);
    }
    // The saved predicate describes the old index, even when it is tested later.
    let mut body = prologue();
    body.extend([
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(3))),
        assign(2, Rvalue::Use(int(1000))),
        branch(7, vec![read(0, 2)], vec![]),
    ]);
    check(body, 0);
}

#[test]
fn branch_facts_are_intersected_and_guarded_induction_preserves_its_lower_bound() {
    let mut body = prologue();
    body.extend([
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(3))),
        branch(7, vec![read(0, 2)], vec![]),
        read(0, 2),
    ]);
    check(body, 1);
    let mut body = prologue();
    body.extend([
        assign(2, Rvalue::Use(int(0))),
        statement(StatementKind::Loop {
            body: Block {
                statements: vec![
                    assign(7, compare(CompareOp::Less, value(2), value(3))),
                    branch(7, vec![], vec![statement(StatementKind::Break)]),
                    read(0, 2),
                    assign(
                        2,
                        Rvalue::Binary {
                            op: crate::BinaryOp::Add,
                            lhs: value(2),
                            rhs: int(1),
                        },
                    ),
                ],
            },
        }),
    ]);
    // The exclusive i32 loop guard prevents the unit increment from wrapping.
    check(body, 1);
}

#[test]
fn escaped_indices_are_not_treated_as_immutable_values() {
    let mut body = prologue();
    body.extend([
        assign(
            10,
            Rvalue::MakeSlice {
                source: crate::SliceSource::Place(Place::local(LocalId::new(2))),
                start: int(0),
                len: int(1),
                bounds: BoundsMode::Unchecked,
                access: AccessMode::ReadWrite,
            },
        ),
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(3))),
        branch(
            7,
            vec![
                statement(StatementKind::SliceStore {
                    slice: value(10),
                    index: int(0),
                    value: int(1000),
                    bounds: BoundsMode::Unchecked,
                }),
                read(0, 2),
            ],
            vec![],
        ),
    ]);
    check(body, 0);
}

#[test]
fn maximum_lengths_do_not_prove_both_inputs_are_accessible() {
    let mut body = prologue();
    body[5] = assign(
        5,
        Rvalue::Intrinsic {
            intrinsic: Intrinsic::Max,
            args: vec![value(3), value(4)],
        },
    );
    body.extend([
        assign(6, compare(CompareOp::Less, value(2), int(0))),
        branch(6, vec![leave()], vec![]),
        assign(7, compare(CompareOp::Less, value(2), value(5))),
        branch(7, vec![read(0, 2), write(1, 2)], vec![]),
    ]);
    check(body, 0);
}
