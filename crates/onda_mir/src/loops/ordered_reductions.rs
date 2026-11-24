use super::{assigned_local, independent, nonempty_loop, LoopAnalysis};
use crate::local_references::collect_direct_local_reads;
use crate::*;
use serde::Serialize;
use std::collections::HashSet;

/// A counted state loop whose final floating-point addition must stay ordered.
/// The backend still selects supported lane operations.
#[derive(Serialize)]
pub(crate) struct OrderedReduction {
    loop_ordinal: usize,
    index: LocalId,
    first: i32,
    last: i32,
    sum: LocalId,
    term: Value,
    condition: LocalId,
    sum_result: LocalId,
}

pub(crate) fn collect(program: &OptimizedProgram) -> Vec<OrderedReduction> {
    let process = program.entry_points.process;
    let function = &program.functions[process.index()];
    let analysis = LoopAnalysis::new(
        program,
        program.effects(),
        program.memory(),
        program.ranges(),
        process,
    );
    let mut reductions = Vec::new();
    let mut ordinal = 0;
    collect_block(&analysis, &function.body, &mut ordinal, &mut reductions);
    reductions
}

fn collect_block(
    analysis: &LoopAnalysis<'_>,
    block: &Block,
    ordinal: &mut usize,
    reductions: &mut Vec<OrderedReduction>,
) {
    for item in &block.statements {
        match &item.kind {
            StatementKind::Loop { body } => {
                if let Some(plan) = prove(analysis, body, *ordinal) {
                    reductions.push(plan);
                }
                *ordinal += 1;
                collect_block(analysis, body, ordinal, reductions);
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block(analysis, then_block, ordinal, reductions);
                collect_block(analysis, else_block, ordinal, reductions);
            }
            _ => {}
        }
    }
}

fn prove(
    analysis: &LoopAnalysis<'_>,
    body: &Block,
    loop_ordinal: usize,
) -> Option<OrderedReduction> {
    let counted = nonempty_loop(body)?;
    let Value::Constant(ScalarValue::I32(limit)) = counted.limit else {
        return None;
    };
    let (save, prefix) = counted.body.split_last()?;
    let (add, prefix) = prefix.split_last()?;
    let (sum, Rvalue::Use(Value::Local(saved))) = assigned_local(save)? else {
        return None;
    };
    let (
        sum_result,
        Rvalue::Binary {
            op: BinaryOp::Add,
            lhs: Value::Local(accumulator),
            rhs: term,
        },
    ) = assigned_local(add)?
    else {
        return None;
    };
    let function = &analysis.program.functions[analysis.function.index()];
    if sum_result != *saved
        || sum != *accumulator
        || !matches!(
            analysis.program.types[function.locals[sum.index()].ty.index()],
            Type::Scalar(ScalarType::F32 | ScalarType::F64)
        )
    {
        return None;
    }
    let tail = [counted.index, counted.condition, sum, sum_result];
    let mut writes: HashSet<_> = tail.into_iter().collect();
    if writes.len() != tail.len() {
        return None;
    }
    // Walk backwards from the reduction term. A local still needed at loop
    // entry must be invariant or the induction variable.
    let mut live_in = HashSet::new();
    if let Value::Local(local) = term {
        live_in.insert(*local);
    }
    for statement in prefix.iter().rev() {
        let locals = match &statement.kind {
            StatementKind::Call { results, .. } => results.as_slice(),
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(local),
                        projections,
                    },
                ..
            } => {
                if !projections.is_empty() {
                    return None;
                }
                std::slice::from_ref(local)
            }
            _ => &[],
        };
        if locals.iter().any(|local| tail.contains(local)) {
            return None;
        }
        for local in locals {
            live_in.remove(local);
            writes.insert(*local);
        }
        collect_direct_local_reads(statement, &mut live_in);
    }
    live_in.remove(&counted.index);
    if !writes.is_disjoint(&live_in) {
        return None;
    }
    if writes
        .iter()
        .any(|&local| analysis.facts.storage_escapes(local))
    {
        return None;
    }

    let reduction = OrderedReduction {
        loop_ordinal,
        index: counted.index,
        first: analysis.facts.preceding_reset(body)?,
        last: limit - 1,
        sum,
        term: *term,
        condition: counted.condition,
        sum_result,
    };
    independent::prove_counted(analysis, body, counted, Some(writes))?;
    Some(reduction)
}
