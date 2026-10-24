//! Separate an ordered sum from independent, already-materialized rendering.
//! No scratch storage or reassociation is introduced. Small loops and loops
//! without expensive pointwise math keep their original schedule.
use std::collections::HashSet;

use crate::*;

use super::{collect_statement_read_references, PassStats};

// Below this size the extra traversal can outweigh vector math, especially
// after unrolling. This is a profitability threshold, not a legality rule.
const MIN_ITERATIONS: i32 = 32;

pub(super) fn is_applicable(program: &Program) -> bool {
    for function in &program.functions {
        let mut pending = vec![&function.body];
        while let Some(block) = pending.pop() {
            for (index, statement) in block.statements.iter().enumerate() {
                match &statement.kind {
                    StatementKind::Loop { body } => {
                        let previous = index.checked_sub(1).map(|i| &block.statements[i]);
                        if plan(&program.types, &function.locals, previous, body).is_some() {
                            return true;
                        }
                        pending.push(body);
                    }
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        pending.extend([then_block, else_block]);
                    }
                    _ => {}
                }
            }
        }
    }
    false
}

pub(super) fn distribute(program: &mut Program, stats: &mut PassStats) {
    for function in &mut program.functions {
        visit(
            &program.types,
            &mut function.locals,
            &mut function.body,
            stats,
        );
    }
}

fn visit(types: &[Type], locals: &mut Vec<Local>, block: &mut Block, stats: &mut PassStats) {
    let mut output = Vec::with_capacity(block.statements.len());
    for mut statement in std::mem::take(&mut block.statements) {
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                visit(types, locals, then_block, stats);
                visit(types, locals, else_block, stats);
            }
            StatementKind::Loop { body } => {
                visit(types, locals, body, stats);
                if let Some(reduction) = split(types, locals, output.last(), body) {
                    output.push(statement);
                    output.extend(reduction);
                    stats.distributed_loops += 1;
                    continue;
                }
            }
            _ => {}
        }
        output.push(statement);
    }
    block.statements = output;
}

fn assignment(statement: &Statement) -> Option<(LocalId, &Rvalue)> {
    let StatementKind::Assign {
        destination:
            Place {
                base: PlaceBase::Local(local),
                projections,
            },
        value,
    } = &statement.kind
    else {
        return None;
    };
    projections.is_empty().then_some((*local, value))
}

struct FixedLoop {
    induction: LocalId,
    index: LocalId,
}

/// Distribution currently uses the nonempty form with a separate index copy.
fn fixed_loop(previous: &Statement, body: &Block) -> Option<FixedLoop> {
    let statements = &body.statements;
    let (induction, Rvalue::Use(Value::Constant(ScalarValue::I32(0)))) = assignment(previous)?
    else {
        return None;
    };
    let (index, Rvalue::Use(Value::Local(initial))) = assignment(statements.first()?)? else {
        return None;
    };
    let counted = crate::counted_loop(body)?;
    let Value::Constant(ScalarValue::I32(limit)) = counted.limit else {
        return None;
    };
    if *initial != induction
        || counted.index != induction
        || index == induction
        || limit < MIN_ITERATIONS
    {
        return None;
    }
    Some(FixedLoop { induction, index })
}

struct Distribution {
    reduction_at: usize,
    ty: TypeId,
}

fn plan(
    types: &[Type],
    locals: &[Local],
    previous: Option<&Statement>,
    body: &Block,
) -> Option<Distribution> {
    if locals.len() >= MAX_FUNCTION_LOCAL_COUNT {
        return None;
    }
    let previous = previous?;
    let loop_ = fixed_loop(previous, body)?;
    let statements = &body.statements;
    let reduction_at = statements.len().checked_sub(4)?;
    if reduction_at < 2 {
        return None;
    }
    let (
        temporary,
        Rvalue::Binary {
            op: BinaryOp::Add,
            lhs: Value::Local(accumulator),
            rhs: term,
        },
    ) = assignment(&statements[reduction_at])?
    else {
        return None;
    };
    let (destination, Rvalue::Use(Value::Local(result))) =
        assignment(&statements[reduction_at + 1])?
    else {
        return None;
    };
    if destination != *accumulator || *result != temporary || temporary == *accumulator {
        return None;
    }
    let ty = locals[destination.index()].ty;
    if !matches!(
        types[ty.index()],
        Type::Scalar(ScalarType::F32 | ScalarType::F64)
    ) {
        return None;
    }
    // Reuse the value the program already stores. A fresh temporary array
    // would change memory use, failure behavior, and the profitability model.
    let StatementKind::Assign {
        destination: materialized,
        value: Rvalue::Use(stored),
    } = &statements[reduction_at - 1].kind
    else {
        return None;
    };
    if stored != term || !indexed_object(materialized, loop_.index) {
        return None;
    }
    let map = &statements[1..reduction_at];
    let mut written = HashSet::new();
    collect_writes(map, &mut written);
    if written.contains(&loop_.induction) || written.contains(&loop_.index) {
        return None;
    }
    let forbidden = HashSet::from([temporary, *accumulator]);
    let mut defined = HashSet::new();
    let mut expensive = false;
    let context = MapAnalysisContext {
        types,
        locals,
        index: loop_.index,
        written: &written,
        forbidden: &forbidden,
    };
    if !independent_map(&context, map, &mut defined, &mut expensive) || !expensive {
        return None;
    }
    Some(Distribution { reduction_at, ty })
}

fn split(
    types: &[Type],
    locals: &mut Vec<Local>,
    previous: Option<&Statement>,
    body: &mut Block,
) -> Option<Vec<Statement>> {
    let Distribution { reduction_at, ty } = plan(types, locals, previous, body)?;
    let previous = previous?;
    let statements = &body.statements;
    let StatementKind::Assign {
        destination: materialized,
        ..
    } = &statements[reduction_at - 1].kind
    else {
        unreachable!("planned materialized reduction term")
    };
    let loaded = LocalId::new(locals.len() as u32);
    locals.push(Local {
        name: Some("$distributed.term".into()),
        ty,
        integer_range: None,
    });
    let mut reduction = vec![
        statements[0].clone(),
        Statement {
            source: statements[reduction_at].source,
            kind: StatementKind::Assign {
                destination: Place::local(loaded),
                value: Rvalue::Load(materialized.clone()),
            },
        },
    ];
    let mut addition = statements[reduction_at].clone();
    if let StatementKind::Assign {
        value: Rvalue::Binary { rhs, .. },
        ..
    } = &mut addition.kind
    {
        *rhs = Value::Local(loaded);
    }
    reduction.push(addition);
    reduction.extend_from_slice(&statements[reduction_at + 1..]);
    let source = previous.source;
    let result = vec![
        previous.clone(),
        Statement {
            source,
            kind: StatementKind::Loop {
                body: Block {
                    statements: reduction,
                },
            },
        },
    ];
    body.statements.drain(reduction_at..reduction_at + 2);
    Some(result)
}

fn indexed_object(place: &Place, index: LocalId) -> bool {
    matches!(place.base, PlaceBase::State(_) | PlaceBase::Local(_))
        && matches!(place.projections.as_slice(), [Projection::Index { index: Value::Local(at), bounds: BoundsMode::Unchecked }] if *at == index)
}

fn collect_writes(statements: &[Statement], written: &mut HashSet<LocalId>) {
    for statement in statements {
        if let Some((local, _)) = assignment(statement) {
            written.insert(local);
        }
        if let StatementKind::If {
            then_block,
            else_block,
            ..
        } = &statement.kind
        {
            collect_writes(&then_block.statements, written);
            collect_writes(&else_block.statements, written);
        }
    }
}

struct MapAnalysisContext<'a> {
    types: &'a [Type],
    locals: &'a [Local],
    index: LocalId,
    written: &'a HashSet<LocalId>,
    forbidden: &'a HashSet<LocalId>,
}

fn independent_map(
    context: &MapAnalysisContext<'_>,
    statements: &[Statement],
    defined: &mut HashSet<LocalId>,
    expensive: &mut bool,
) -> bool {
    for statement in statements {
        let mut reads = HashSet::new();
        collect_statement_read_references(statement, &mut reads);
        if reads.iter().any(|local| {
            context.forbidden.contains(local)
                || context.written.contains(local) && !defined.contains(local)
        }) {
            return false;
        }
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                if let Some((local, _)) = assignment(statement) {
                    if context.forbidden.contains(&local)
                        || !matches!(
                            context.types[context.locals[local.index()].ty.index()],
                            Type::Scalar(_)
                        )
                    {
                        return false;
                    }
                    defined.insert(local);
                } else if !indexed_object(destination, context.index) {
                    return false;
                }
                if crate::analysis::rvalue_may_fail(context.types, context.locals, value) {
                    return false;
                }
                match value {
                    Rvalue::Use(_)
                    | Rvalue::Unary { .. }
                    | Rvalue::Binary { .. }
                    | Rvalue::Compare { .. }
                    | Rvalue::Cast { .. }
                    | Rvalue::ConstDataLoad { .. } => {}
                    Rvalue::Load(place)
                        if indexed_object(place, context.index)
                            || place.projections.is_empty()
                                && matches!(
                                    place.base,
                                    PlaceBase::Local(_) | PlaceBase::State(_) | PlaceBase::Param(_)
                                ) => {}
                    Rvalue::Intrinsic { intrinsic, .. } => {
                        *expensive |= matches!(
                            intrinsic,
                            Intrinsic::Sin
                                | Intrinsic::Cos
                                | Intrinsic::Tan
                                | Intrinsic::Tanh
                                | Intrinsic::Atan
                                | Intrinsic::Atan2
                                | Intrinsic::Exp
                                | Intrinsic::Log
                                | Intrinsic::Pow
                        );
                    }
                    _ => return false,
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                let mut yes = defined.clone();
                let mut no = defined.clone();
                if !independent_map(context, &then_block.statements, &mut yes, expensive)
                    || !independent_map(context, &else_block.statements, &mut no, expensive)
                {
                    return false;
                }
                *defined = yes.intersection(&no).copied().collect();
            }
            // In particular: no calls, observable events, runtime failures,
            // shared RNG, early exits, or memory with unresolved aliases.
            _ => return false,
        }
    }
    true
}
