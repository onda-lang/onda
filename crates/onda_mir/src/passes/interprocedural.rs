//! Proven call-argument constants and unreachable function removal. This runs
//! after semantic checking, so pruning never hides authored diagnostics.
use super::PassStats;
use crate::{
    Block, FunctionId, FunctionKind, PlaceBase, Program, ProgramRangeAnalysis, Rvalue, ScalarType,
    ScalarValue, StatementKind, Value,
};

pub(super) fn specialize_parameters(
    program: &mut Program,
    ranges: &ProgramRangeAnalysis,
    stats: &mut PassStats,
) {
    for (index, function) in program.functions.iter_mut().enumerate() {
        if function.kind != FunctionKind::User {
            continue;
        }
        let ranges = ranges.function(FunctionId::new(index as u32)).unwrap();
        let mut blocks = vec![&mut function.body];
        while let Some(block) = blocks.pop() {
            for statement in &mut block.statements {
                match &mut statement.kind {
                    StatementKind::Assign {
                        value: value @ Rvalue::Load(_),
                        ..
                    } => {
                        let Rvalue::Load(place) = value else {
                            unreachable!()
                        };
                        let PlaceBase::Parameter(parameter) = place.base else {
                            continue;
                        };
                        if !place.projections.is_empty() {
                            continue;
                        }
                        let Some(range) = ranges
                            .parameter(parameter)
                            .filter(|range| range.min() == range.max())
                        else {
                            continue;
                        };
                        let constant = match range.scalar() {
                            ScalarType::I32 => ScalarValue::I32(range.min() as i32),
                            ScalarType::I64 => ScalarValue::I64(range.min()),
                            _ => continue,
                        };
                        *value = Rvalue::Use(Value::Constant(constant));
                        stats.propagated_values += 1;
                    }
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => blocks.extend([then_block, else_block]),
                    StatementKind::Loop { body } => blocks.push(body),
                    _ => {}
                }
            }
        }
    }
}

pub(super) fn remove_unreachable_functions(program: &mut Program, stats: &mut PassStats) {
    let graph = crate::call_graph::CallGraph::new(program);
    let mut live = vec![false; program.functions.len()];
    let mut pending: Vec<_> = program
        .functions
        .iter()
        .enumerate()
        .filter_map(|(index, function)| (function.kind != FunctionKind::User).then_some(index))
        .collect();
    while let Some(index) = pending.pop() {
        if std::mem::replace(&mut live[index], true) {
            continue;
        }
        pending.extend(&graph.callees[index]);
    }
    let mut mapping = vec![None; live.len()];
    let mut next = 0;
    for (index, &live) in live.iter().enumerate() {
        if live {
            mapping[index] = Some(FunctionId::new(next));
            next += 1;
        }
    }
    if next as usize == live.len() {
        return;
    }
    stats.removed_functions += (live.len() - next as usize) as u64;
    let mut index = 0;
    program.functions.retain(|_| {
        let keep = live[index];
        index += 1;
        keep
    });
    let remap = |id: &mut FunctionId| *id = mapping[id.index()].expect("reachable function");
    remap(&mut program.entry_points.init);
    remap(&mut program.entry_points.process);
    for event in &mut program.interface.events {
        remap(&mut event.handler);
    }
    for function in &mut program.functions {
        let mut blocks: Vec<&mut Block> = vec![&mut function.body];
        while let Some(block) = blocks.pop() {
            for statement in &mut block.statements {
                match &mut statement.kind {
                    StatementKind::Call { function, .. } => remap(function),
                    StatementKind::If {
                        then_block,
                        else_block,
                        ..
                    } => blocks.extend([then_block, else_block]),
                    StatementKind::Loop { body } => blocks.push(body),
                    _ => {}
                }
            }
        }
    }
}
