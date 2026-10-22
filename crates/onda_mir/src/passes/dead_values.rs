use std::collections::HashSet;

use crate::{Block, Function, Local, LocalId, PlaceBase, Statement, StatementKind, Type};

use super::{collect_statement_read_references, PassStats};

/// Trace local dependencies from effects and potentially failing operations.
/// Locals are not SSA: every assignment to a live local contributes dependencies.
/// Each local is visited once, including chains and cycles across structured blocks.
pub(super) fn remove(types: &[Type], function: &mut Function, stats: &mut PassStats) {
    let mut dependencies = vec![Vec::new(); function.locals.len()];
    let mut live = vec![false; function.locals.len()];
    let mut pending = Vec::new();
    let mut reads = HashSet::new();
    let mut blocks = vec![&function.body];
    while let Some(block) = blocks.pop() {
        for statement in &block.statements {
            reads.clear();
            collect_statement_read_references(statement, &mut reads);
            if let Some(local) = discardable_destination(types, &function.locals, statement) {
                dependencies[local.index()].extend(reads.iter().copied());
            } else {
                for local in &reads {
                    mark_live(*local, &mut live, &mut pending);
                }
            }
            match &statement.kind {
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    blocks.extend([then_block, else_block]);
                }
                StatementKind::Loop { body } => blocks.push(body),
                _ => {}
            }
        }
    }
    while let Some(local) = pending.pop() {
        for dependency in &dependencies[local.index()] {
            mark_live(*dependency, &mut live, &mut pending);
        }
    }
    retain_live_assignments(types, &function.locals, &mut function.body, &live, stats);
}

fn mark_live(local: LocalId, live: &mut [bool], pending: &mut Vec<LocalId>) {
    if !live[local.index()] {
        live[local.index()] = true;
        pending.push(local);
    }
}

fn discardable_destination(
    types: &[Type],
    locals: &[Local],
    statement: &Statement,
) -> Option<LocalId> {
    let StatementKind::Assign { destination, value } = &statement.kind else {
        return None;
    };
    let PlaceBase::Local(local) = destination.base else {
        return None;
    };
    (destination.projections.is_empty() && !crate::analysis::rvalue_may_fail(types, locals, value))
        .then_some(local)
}

fn retain_live_assignments(
    types: &[Type],
    locals: &[Local],
    block: &mut Block,
    live: &[bool],
    stats: &mut PassStats,
) {
    block.statements.retain_mut(|statement| {
        if discardable_destination(types, locals, statement)
            .is_some_and(|local| !live[local.index()])
        {
            stats.removed_dead_assignments += 1;
            return false;
        }
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                retain_live_assignments(types, locals, then_block, live, stats);
                retain_live_assignments(types, locals, else_block, live, stats);
            }
            StatementKind::Loop { body } => {
                retain_live_assignments(types, locals, body, live, stats)
            }
            _ => {}
        }
        true
    });
}
