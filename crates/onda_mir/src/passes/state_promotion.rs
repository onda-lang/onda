use crate::{
    Block, CallArgument, FunctionId, FunctionKind, Local, LocalId, PassingMode, Place, PlaceBase,
    Program, Rvalue, SliceSource, StateId, StatePersistence, Statement, StatementKind, Type,
};

use super::PassStats;

// Crossing a process loop with too many promoted values creates a large PHI
// web in native SSA and can force spills in Wasm engines. Keep the portable
// transform to a small scalar working set; target backends remain free to
// promote additional fields using their register-pressure models.
const MAX_PORTABLE_PROMOTED_SCALARS: usize = 8;

#[derive(Debug, Clone, Copy, Default)]
struct StateAccess {
    reads: bool,
    writes: bool,
    eligible: bool,
}

pub(super) fn promote_process_scalar_state(program: &mut Program, stats: &mut PassStats) {
    let process_id = program.entry_points.process;
    let Some(process) = program.functions.get(process_id.index()) else {
        return;
    };
    if process.kind != FunctionKind::Process || contains_return(&process.body) {
        return;
    }

    let mut accesses = program
        .state
        .iter()
        .map(|state| StateAccess {
            eligible: matches!(program.types[state.ty.index()], Type::Scalar(_))
                && state.persistence != StatePersistence::ControlMirror,
            ..StateAccess::default()
        })
        .collect::<Vec<_>>();
    collect_block_accesses(program, &process.body, &mut accesses);

    // A state slot passed by reference must alias any direct access to that
    // same slot in the transitive callee graph. Since this portable transform
    // promotes only the process body (not functions shared with init/events),
    // leave such slots in memory and let a target-aware interprocedural pass
    // decide whether it can safely inline or clone the callees first.
    let mut directly_accessed_by_callees = vec![false; program.state.len()];
    collect_reachable_callee_state_accesses(program, process_id, &mut directly_accessed_by_callees);
    for (access, aliased) in accesses.iter_mut().zip(directly_accessed_by_callees) {
        access.eligible &= !aliased;
    }

    let promoted = accesses
        .iter()
        .enumerate()
        .filter_map(|(index, access)| {
            (access.eligible && (access.reads || access.writes))
                .then_some((StateId::new(index as u32), *access))
        })
        .collect::<Vec<_>>();
    if promoted.is_empty() || promoted.len() > MAX_PORTABLE_PROMOTED_SCALARS {
        return;
    }

    let process = &mut program.functions[process_id.index()];
    let mut mapping = vec![None; program.state.len()];
    let mut prologue = Vec::with_capacity(promoted.len());
    let mut epilogue = Vec::with_capacity(promoted.len());
    for (state, access) in promoted {
        let local = LocalId::new(process.locals.len() as u32);
        let slot = &program.state[state.index()];
        process.locals.push(Local {
            name: Some(format!("$promoted.state.{}", slot.name)),
            ty: slot.ty,
        });
        mapping[state.index()] = Some(local);
        // Initializing write-only references too keeps reference formation
        // independent of definite-assignment details in downstream backends.
        // The load is once per segment and is normally removed when proven
        // unnecessary after inlining.
        prologue.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(local),
                value: Rvalue::Load(Place {
                    base: PlaceBase::State(state),
                    projections: Vec::new(),
                }),
            },
            source: process.source,
        });
        if access.writes {
            epilogue.push(Statement {
                kind: StatementKind::Assign {
                    destination: Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    },
                    value: Rvalue::Use(crate::Value::Local(local)),
                },
                source: process.source,
            });
        }
        stats.promoted_state_slots = stats.promoted_state_slots.saturating_add(1);
    }
    rewrite_block_state(&mut process.body, &mapping);
    let mut body = std::mem::take(&mut process.body.statements);
    prologue.append(&mut body);
    prologue.append(&mut epilogue);
    process.body.statements = prologue;
}

fn contains_return(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(|statement| match &statement.kind {
            StatementKind::Return { .. } => true,
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => contains_return(then_block) || contains_return(else_block),
            StatementKind::Loop { body } => contains_return(body),
            _ => false,
        })
}

fn collect_block_accesses(program: &Program, block: &Block, accesses: &mut [StateAccess]) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                collect_place(destination, false, true, accesses);
                collect_rvalue(value, accesses);
            }
            StatementKind::Call { function, args, .. } => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = &program.functions[function.index()].params[index];
                    let (reads, writes) = match parameter.mode {
                        PassingMode::Value => (false, false),
                        PassingMode::ReadOnlyReference => (true, false),
                        // Passing mode is deliberately conservative here. A
                        // backend may refine this using EffectAnalysis.
                        PassingMode::ReadWriteReference => (true, true),
                    };
                    match argument {
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => {
                            collect_place(place, reads, writes, accesses);
                        }
                        _ => {}
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_block_accesses(program, then_block, accesses);
                collect_block_accesses(program, else_block, accesses);
            }
            StatementKind::Loop { body } => collect_block_accesses(program, body, accesses),
            _ => {}
        }
    }
}

fn collect_reachable_callee_state_accesses(
    program: &Program,
    root: FunctionId,
    direct_state_access: &mut [bool],
) {
    let mut visited = vec![false; program.functions.len()];
    visited[root.index()] = true;
    let mut pending = Vec::new();
    collect_calls(&program.functions[root.index()].body, &mut pending);
    while let Some(function) = pending.pop() {
        if std::mem::replace(&mut visited[function.index()], true) {
            continue;
        }
        let body = &program.functions[function.index()].body;
        collect_direct_state_accesses(body, direct_state_access);
        collect_calls(body, &mut pending);
    }
}

fn collect_calls(block: &Block, calls: &mut Vec<FunctionId>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { function, .. } => calls.push(*function),
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_calls(then_block, calls);
                collect_calls(else_block, calls);
            }
            StatementKind::Loop { body } => collect_calls(body, calls),
            _ => {}
        }
    }
}

fn collect_direct_state_accesses(block: &Block, direct_state_access: &mut [bool]) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                mark_direct_state_place(destination, direct_state_access);
                mark_direct_state_rvalue(value, direct_state_access);
            }
            StatementKind::Call { args, .. } => {
                for argument in args {
                    match argument {
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => {
                            mark_direct_state_place(place, direct_state_access);
                        }
                        _ => {}
                    }
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_direct_state_accesses(then_block, direct_state_access);
                collect_direct_state_accesses(else_block, direct_state_access);
            }
            StatementKind::Loop { body } => {
                collect_direct_state_accesses(body, direct_state_access)
            }
            _ => {}
        }
    }
}

fn mark_direct_state_rvalue(value: &Rvalue, direct_state_access: &mut [bool]) {
    match value {
        Rvalue::Load(place)
        | Rvalue::MakeSlice {
            source: SliceSource::Place(place),
            ..
        } => mark_direct_state_place(place, direct_state_access),
        _ => {}
    }
}

fn mark_direct_state_place(place: &Place, direct_state_access: &mut [bool]) {
    if let PlaceBase::State(state) = place.base {
        direct_state_access[state.index()] = true;
    }
}

fn collect_rvalue(value: &Rvalue, accesses: &mut [StateAccess]) {
    match value {
        Rvalue::Load(place) => collect_place(place, true, false, accesses),
        Rvalue::MakeSlice {
            source: SliceSource::Place(place),
            ..
        } => collect_place(place, true, false, accesses),
        _ => {}
    }
}

fn collect_place(place: &Place, reads: bool, writes: bool, accesses: &mut [StateAccess]) {
    let PlaceBase::State(state) = place.base else {
        return;
    };
    let access = &mut accesses[state.index()];
    access.reads |= reads;
    access.writes |= writes;
    if !place.projections.is_empty() {
        access.eligible = false;
    }
}

fn rewrite_block_state(block: &mut Block, mapping: &[Option<LocalId>]) {
    for statement in &mut block.statements {
        rewrite_statement(statement, mapping);
    }
}

fn rewrite_statement(statement: &mut Statement, mapping: &[Option<LocalId>]) {
    match &mut statement.kind {
        StatementKind::Assign { destination, value } => {
            rewrite_place(destination, mapping);
            rewrite_rvalue(value, mapping);
        }
        StatementKind::Call { args, .. } => {
            for argument in args {
                match argument {
                    CallArgument::Place(place) | CallArgument::ArrayWindow { array: place, .. } => {
                        rewrite_place(place, mapping)
                    }
                    _ => {}
                }
            }
        }
        StatementKind::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_block_state(then_block, mapping);
            rewrite_block_state(else_block, mapping);
        }
        StatementKind::Loop { body } => rewrite_block_state(body, mapping),
        _ => {}
    }
}

fn rewrite_rvalue(value: &mut Rvalue, mapping: &[Option<LocalId>]) {
    match value {
        Rvalue::Load(place) => rewrite_place(place, mapping),
        Rvalue::MakeSlice {
            source: SliceSource::Place(place),
            ..
        } => rewrite_place(place, mapping),
        _ => {}
    }
}

fn rewrite_place(place: &mut Place, mapping: &[Option<LocalId>]) {
    let PlaceBase::State(state) = place.base else {
        return;
    };
    if let Some(local) = mapping[state.index()] {
        place.base = PlaceBase::Local(local);
    }
}
