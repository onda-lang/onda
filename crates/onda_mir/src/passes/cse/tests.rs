use super::*;
use crate::{FunctionAttributes, FunctionKind, Local, Place, SourceSpan, Statement, TypeId};

fn value(local: u32) -> Value {
    Value::Local(LocalId::new(local))
}
fn float(value: f32) -> Value {
    Value::Constant(ScalarValue::F32(value))
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
fn product(local: u32) -> Statement {
    assign(
        local,
        Rvalue::Binary {
            op: BinaryOp::Multiply,
            lhs: value(0),
            rhs: float(2.0),
        },
    )
}
fn run(statements: Vec<Statement>) -> (Block, PassStats) {
    let mut function = Function {
        name: "cse".into(),
        kind: FunctionKind::User,
        attributes: FunctionAttributes::default(),
        params: vec![],
        results: vec![],
        source: SourceSpan::UNKNOWN,
        locals: (0..8)
            .map(|_| Local {
                name: None,
                integer_range: None,
                ty: TypeId::new(0),
            })
            .collect(),
        body: Block { statements },
    };
    let mut stats = PassStats::default();
    eliminate_common_subexpressions(&mut function, &[vec![]], &mut stats);
    (function.body, stats)
}
fn rvalue(body: &Block, index: usize) -> &Rvalue {
    let StatementKind::Assign { value, .. } = &body.statements[index].kind else {
        panic!("not an assignment")
    };
    value
}

#[test]
fn overwritten_results_are_not_reused() {
    let (body, stats) = run(vec![
        assign(0, Rvalue::Use(float(3.0))),
        product(1),
        assign(1, Rvalue::Use(float(0.0))),
        product(2),
        product(3),
    ]);
    assert_eq!(stats.eliminated_common_subexpressions, 1);
    assert!(matches!(rvalue(&body, 3), Rvalue::Binary { .. }));
    assert_eq!(rvalue(&body, 4), &Rvalue::Use(value(2)));
}

#[test]
fn joins_and_loop_carried_values_are_not_reused() {
    let (body, stats) = run(vec![
        assign(0, Rvalue::Use(float(3.0))),
        product(1),
        statement(StatementKind::If {
            condition: Value::Constant(ScalarValue::Bool(true)),
            then_block: Block {
                statements: vec![assign(0, Rvalue::Use(float(4.0)))],
            },
            else_block: Block::default(),
        }),
        product(2),
        statement(StatementKind::Loop {
            body: Block {
                statements: vec![
                    product(3),
                    assign(0, Rvalue::Use(float(5.0))),
                    statement(StatementKind::Break),
                ],
            },
        }),
        product(4),
    ]);
    assert_eq!(stats.eliminated_common_subexpressions, 0);
    assert!(matches!(rvalue(&body, 3), Rvalue::Binary { .. }));
    assert!(matches!(rvalue(&body, 5), Rvalue::Binary { .. }));
}

#[test]
fn expressions_distinguish_signed_zero_and_nan_payloads() {
    let divide = |local, rhs| {
        assign(
            local,
            Rvalue::Binary {
                op: BinaryOp::Divide,
                lhs: value(0),
                rhs: float(rhs),
            },
        )
    };
    let (body, stats) = run(vec![
        assign(0, Rvalue::Use(float(1.0))),
        divide(1, 0.0),
        divide(2, -0.0),
        divide(3, f32::from_bits(0x7fc00001)),
        divide(4, f32::from_bits(0x7fc00002)),
        divide(5, -0.0),
    ]);
    assert_eq!(stats.eliminated_common_subexpressions, 1);
    assert_eq!(rvalue(&body, 5), &Rvalue::Use(value(2)));
}
