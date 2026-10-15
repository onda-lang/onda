use crate::{
    Block, CallArgument, EffectAnalysis, FunctionId, FunctionKind, Local, LocalId, PassingMode,
    Place, PlaceBase, Program, Rvalue, SliceSource, StateId, StatePersistence, Statement,
    StatementKind, Type,
};

use super::PassStats;

// Bound the process-loop PHI web without restricting ordinary DSP processors
// to a handful of cached fields. Benchmark runtime, not generated code size,
// when tuning this budget; target backends still control register allocation.
const MAX_PORTABLE_PROMOTED_SCALARS: usize = 64;

#[derive(Debug, Clone, Copy, Default)]
struct StateAccess {
    reads: bool,
    writes: bool,
    eligible: bool,
    preferred_read_depth: Option<usize>,
    direct_read_depth: Option<usize>,
    loop_depth: usize,
    write_depth: Option<usize>,
}

impl StateAccess {
    fn profitable_at_depth(&self, depth: usize, cache_direct_invariants: bool) -> bool {
        depth == 0
            || (depth == 1 && self.loop_depth == 0)
            || (self.loop_depth == depth
                && (self.preferred_read_depth == Some(depth)
                    || self.write_depth == Some(depth)
                    || (cache_direct_invariants && self.direct_read_depth == Some(depth))))
    }
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
                && state.persistence != StatePersistence::ControlMirror
                // Task lifecycle stores deliberately commit before executing
                // a resume region so a propagated runtime failure leaves the
                // task Failed. Hoisting them into a process-local with only a
                // normal-path epilogue would violate that contract.
                && !state.name.contains("__onda_task_"),
            ..StateAccess::default()
        })
        .collect::<Vec<_>>();
    let effects = crate::analyze_effects(program);
    // With only fixed scalar state, a backend can distinguish invariant loads
    // from the loop's stores. Aggregate or descriptor-based writes can obscure
    // that separation after lowering; retain direct-read caching in that case.
    let cache_direct_invariants = program
        .state
        .iter()
        .any(|state| !matches!(program.types[state.ty.index()], Type::Scalar(_)))
        || effects
            .function(process_id)
            .writes
            .intersects(crate::MemoryRegionSet::INDIRECT);
    let reference_reads = preferred_reference_reads(program);
    collect_block_accesses(
        program,
        &effects,
        &reference_reads,
        &process.body,
        0,
        false,
        &mut accesses,
    );

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

    // Prefer the deepest scalar working set instead of keeping unrelated outer
    // state live across nested work. Array-only inner loops must not disable
    // useful scalar caching in their enclosing loop.
    let loop_depth = accesses
        .iter()
        .filter(|access| access.eligible)
        .map(|access| access.loop_depth)
        .max()
        .unwrap_or(0);

    let promoted = accesses
        .iter()
        .enumerate()
        .filter_map(|(index, access)| {
            (access.eligible
                && (access.reads || access.writes)
                && access.profitable_at_depth(loop_depth, cache_direct_invariants))
            .then_some((StateId::new(index as u32), *access))
        })
        // Keep a bounded working set even in large processors. Selecting in
        // state order is deterministic and retains the same alias checks for
        // every slot; exceeding the budget must not disable the entire pass.
        .take(MAX_PORTABLE_PROMOTED_SCALARS)
        .collect::<Vec<_>>();
    if promoted.is_empty() {
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
            integer_range: slot.integer_range,
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

fn collect_block_accesses(
    program: &Program,
    effects: &EffectAnalysis,
    reference_reads: &[Vec<bool>],
    block: &Block,
    depth: usize,
    conditional: bool,
    accesses: &mut [StateAccess],
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value } => {
                collect_place(destination, false, true, true, false, depth, accesses);
                collect_rvalue(value, depth, accesses);
            }
            StatementKind::Call { function, args, .. } => {
                for (index, argument) in args.iter().enumerate() {
                    let parameter = &program.functions[function.index()].params[index];
                    let (reads, writes) = match parameter.mode {
                        PassingMode::Value => (false, false),
                        PassingMode::ReadOnlyReference => (true, false),
                        PassingMode::ReadWriteReference => (true, true),
                        PassingMode::ResultReference => (false, true),
                    };
                    // Refine profitability without independently changing the
                    // established writeback policy for borrowed state.
                    let scope_writes = effects.function(*function).parameters[index].writes;
                    match argument {
                        CallArgument::Place(place)
                        | CallArgument::ArrayWindow { array: place, .. } => {
                            collect_place(
                                place,
                                reads,
                                writes,
                                scope_writes,
                                reference_reads[function.index()][index],
                                depth,
                                accesses,
                            );
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
                // Loop guards have a terminating branch. Other branches may
                // contain occasional work that should not displace the state
                // used on every iteration of the enclosing loop.
                let conditional = conditional
                    || (super::block_falls_through(then_block)
                        && super::block_falls_through(else_block));
                collect_block_accesses(
                    program,
                    effects,
                    reference_reads,
                    then_block,
                    depth,
                    conditional,
                    accesses,
                );
                collect_block_accesses(
                    program,
                    effects,
                    reference_reads,
                    else_block,
                    depth,
                    conditional,
                    accesses,
                );
            }
            StatementKind::Loop { body } => {
                collect_block_accesses(
                    program,
                    effects,
                    reference_reads,
                    body,
                    depth + usize::from(!conditional),
                    conditional,
                    accesses,
                );
            }
            _ => {}
        }
    }
}

// Profitability only: multiple instances of a conditional reader can keep many
// coefficients live across the loop and external math calls. Prefer selected
// memory operands there; a single reader still benefits from ordinary caching.
// Propagate the hint through forwarding wrappers, independently of alias proofs.
fn process_call_multiplicity(program: &Program) -> Vec<u8> {
    // Count static invocation paths, capped at two. Each function propagates
    // at most two increments, avoiding expansion of shared helper subgraphs.
    let root = program.entry_points.process;
    let mut counts = vec![0_u8; program.functions.len()];
    let mut propagated = counts.clone();
    counts[root.index()] = 1;
    let mut pending = vec![root];
    while let Some(function) = pending.pop() {
        let index = function.index();
        let delta = counts[index] - propagated[index];
        if delta == 0 {
            continue;
        }
        propagated[index] = counts[index];
        let mut callees = Vec::new();
        collect_calls(&program.functions[index].body, &mut callees);
        for callee in callees {
            let count = &mut counts[callee.index()];
            let next = (*count + delta).min(2);
            if next != *count {
                *count = next;
                pending.push(callee);
            }
        }
    }
    counts
}

fn preferred_reference_reads(program: &Program) -> Vec<Vec<bool>> {
    let multiplicity = process_call_multiplicity(program);
    let mut reads = program
        .functions
        .iter()
        .map(|function| vec![false; function.params.len()])
        .collect::<Vec<_>>();
    loop {
        let previous = reads.clone();
        for (index, (function, reads)) in program.functions.iter().zip(&mut reads).enumerate() {
            collect_preferred_reference_reads(
                &function.body,
                &previous,
                multiplicity[index] < 2,
                reads,
            );
        }
        if reads == previous {
            return reads;
        }
    }
}

fn collect_preferred_reference_reads(
    block: &Block,
    callees: &[Vec<bool>],
    include_branches: bool,
    reads: &mut [bool],
) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                value: Rvalue::Load(place),
                ..
            } => {
                if let PlaceBase::Parameter(parameter) = place.base {
                    reads[parameter.index()] = true;
                }
            }
            StatementKind::Call { function, args, .. } => {
                for (argument, read) in args.iter().zip(&callees[function.index()]) {
                    if *read {
                        if let CallArgument::Place(Place {
                            base: PlaceBase::Parameter(parameter),
                            ..
                        }) = argument
                        {
                            reads[parameter.index()] = true;
                        }
                    }
                }
            }
            StatementKind::Loop { body } => {
                collect_preferred_reference_reads(body, callees, include_branches, reads)
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } if include_branches => {
                collect_preferred_reference_reads(then_block, callees, true, reads);
                collect_preferred_reference_reads(else_block, callees, true, reads);
            }
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

fn collect_rvalue(value: &Rvalue, depth: usize, accesses: &mut [StateAccess]) {
    match value {
        Rvalue::Load(place) => {
            collect_place(place, true, false, false, false, depth, accesses);
            if let PlaceBase::State(state) = place.base {
                record_depth(&mut accesses[state.index()].direct_read_depth, depth);
            }
        }
        Rvalue::MakeSlice {
            source: SliceSource::Place(place),
            ..
        } => {
            collect_place(place, true, false, false, false, depth, accesses);
            // A descriptor can outlive reference formation and write the slot.
            // Promoting its backing storage needs descriptor-alias analysis.
            if let PlaceBase::State(state) = place.base {
                accesses[state.index()].eligible = false;
            }
        }
        _ => {}
    }
}

fn collect_place(
    place: &Place,
    reads: bool,
    writes: bool,
    scope_writes: bool,
    preferred_read: bool,
    depth: usize,
    accesses: &mut [StateAccess],
) {
    let PlaceBase::State(state) = place.base else {
        return;
    };
    let access = &mut accesses[state.index()];
    access.reads |= reads;
    access.writes |= writes;
    if reads && preferred_read {
        record_depth(&mut access.preferred_read_depth, depth);
    }
    if reads || writes {
        access.loop_depth = access.loop_depth.max(depth);
    }
    if scope_writes {
        record_depth(&mut access.write_depth, depth);
    }
    if !place.projections.is_empty() {
        access.eligible = false;
    }
}

fn record_depth(previous: &mut Option<usize>, depth: usize) {
    *previous = Some(previous.map_or(depth, |previous| previous.max(depth)));
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
