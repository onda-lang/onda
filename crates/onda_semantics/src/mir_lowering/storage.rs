use super::*;

#[derive(Clone, Copy, Default)]
struct StorageLifetime {
    first: Option<usize>,
    last: usize,
}

impl StorageLifetime {
    fn record(&mut self, position: usize) {
        if self.first.is_none() {
            self.first = Some(position);
        }
        self.last = position;
    }
}

struct ReusableSlot {
    state: onda_mir::StateId,
    ty: TypeId,
    integer_range: Option<onda_mir::IntegerRangeInvariant>,
    available_after: usize,
}

/// Small fixed arrays are preferentially left as entry-block locals: native
/// backends can scalar-replace them, while moving them into instance state
/// makes every element access observable memory traffic. Larger arrays and
/// small arrays exceeding the conservative active-call-path budget live in
/// prepared instance scratch so real-time stack use remains bounded.
///
/// Block/task-carried storage is already explicit state before this pass.
/// Acyclic calls therefore permit one scratch frame per function. Compatible
/// slots are reused after all dependent values and views are dead;
/// simultaneously live data remains disjoint.
const MAX_LOCAL_FIXED_ARRAY_BYTES: u64 = 256;
const MAX_LOCAL_FIXED_ARRAY_PATH_BYTES: u64 = 64 * 1024;

pub(super) fn plan_fixed_scratch(
    program: &mut onda_mir::Program,
) -> Result<(), Vec<MirLoweringError>> {
    let promoted = fixed_array_promotions(program);
    for (function_id, function) in program.functions.iter_mut().enumerate() {
        let mut slots = HashMap::new();
        let referenced = onda_mir::referenced_locals(&function.body);
        let mut lifetimes = vec![StorageLifetime::default(); function.locals.len()];
        let mut dependencies = Vec::new();
        collect_storage_lifetimes(
            &function.body,
            &mut 0,
            &mut lifetimes,
            &mut dependencies,
            &mut HashSet::new(),
        );
        extend_backing_lifetimes(&mut lifetimes, &dependencies);
        let mut candidates = function
            .locals
            .iter()
            .enumerate()
            .filter_map(|(index, local)| {
                let id = LocalId::new(index as u32);
                (referenced.contains(&id) && promoted[function_id].contains(&id)).then_some((
                    id,
                    local,
                    lifetimes[index],
                ))
            })
            .collect::<Vec<_>>();
        candidates
            .sort_by_key(|(id, _, lifetime)| (lifetime.first.unwrap_or(usize::MAX), id.index()));

        let mut reusable = Vec::<ReusableSlot>::new();
        for (local_id, local, lifetime) in candidates {
            let state = reusable
                .iter_mut()
                .find(|slot| {
                    slot.ty == local.ty
                        && slot.integer_range == local.integer_range
                        && slot.available_after < lifetime.first.unwrap_or(0)
                })
                .map(|slot| {
                    slot.available_after = lifetime.last;
                    slot.state
                });
            let state = match state {
                Some(state) => state,
                None => {
                    let raw_id = u32::try_from(program.state.len()).map_err(|_| {
                        vec![MirLoweringError::new(
                            "planned data scratch exceeds the state slot limit",
                            SourceLoc::ZERO,
                        )]
                    })?;
                    let state = onda_mir::StateId::new(raw_id);
                    program.state.push(onda_mir::StateSlot {
                        name: format!(
                            "__onda_scratch.{function_id}.{}.{}",
                            local_id.index(),
                            local.name.as_deref().unwrap_or("temporary")
                        ),
                        ty: local.ty,
                        persistence: onda_mir::StatePersistence::InstanceScratch,
                        authored: false,
                        pinned: false,
                        integer_range: local.integer_range,
                    });
                    reusable.push(ReusableSlot {
                        state,
                        ty: local.ty,
                        integer_range: local.integer_range,
                        available_after: lifetime.last,
                    });
                    state
                }
            };
            slots.insert(local_id, state);
        }
        if !slots.is_empty() {
            rewrite_storage(&mut function.body, &slots);
        }
    }
    Ok(())
}

#[derive(Default)]
struct FunctionArrayPlan {
    small: Vec<(LocalId, u64)>,
    callees: Vec<usize>,
}

/// Selects fixed arrays that must use instance scratch. Every function frame
/// reserves all of its surviving local arrays at entry, so source lifetimes do
/// not reduce native stack size. Calls are acyclic after semantic analysis;
/// processing leaves before callers gives each caller the exact conservative
/// local-array cost of its deepest callee path.
fn fixed_array_promotions(program: &onda_mir::Program) -> Vec<HashSet<LocalId>> {
    let function_count = program.functions.len();
    let mut promoted = vec![HashSet::new(); function_count];
    let mut plans = Vec::with_capacity(function_count);

    for (function_index, function) in program.functions.iter().enumerate() {
        let referenced = onda_mir::referenced_locals(&function.body);
        let mut plan = FunctionArrayPlan::default();
        for (index, local) in function.locals.iter().enumerate() {
            let id = LocalId::new(index as u32);
            if !referenced.contains(&id) {
                continue;
            }
            let Some(bytes) = fixed_array_bytes(&program.types, local.ty) else {
                continue;
            };
            if bytes > MAX_LOCAL_FIXED_ARRAY_BYTES {
                promoted[function_index].insert(id);
            } else {
                plan.small.push((id, bytes));
            }
        }
        let mut callees = HashSet::new();
        collect_callees(&function.body, function_count, &mut callees);
        plan.callees = callees.into_iter().collect();
        plans.push(plan);
    }

    let mut callers = vec![Vec::new(); function_count];
    let mut pending_callees = Vec::with_capacity(function_count);
    for (caller, plan) in plans.iter().enumerate() {
        pending_callees.push(plan.callees.len());
        for &callee in &plan.callees {
            callers[callee].push(caller);
        }
    }

    let mut queue = pending_callees
        .iter()
        .enumerate()
        .filter_map(|(function, &pending)| (pending == 0).then_some(function))
        .collect::<VecDeque<_>>();
    let mut path_bytes = vec![0_u64; function_count];
    let mut processed = vec![false; function_count];
    while let Some(function) = queue.pop_front() {
        let callee_bytes = plans[function]
            .callees
            .iter()
            .map(|&callee| path_bytes[callee])
            .max()
            .unwrap_or(0);
        let allowance = MAX_LOCAL_FIXED_ARRAY_PATH_BYTES.saturating_sub(callee_bytes);
        let mut local_bytes = plans[function]
            .small
            .iter()
            .fold(0_u64, |total, (_, bytes)| total.saturating_add(*bytes));
        if local_bytes > allowance {
            let mut arrays = plans[function].small.clone();
            arrays.sort_by_key(|(id, bytes)| {
                (std::cmp::Reverse(*bytes), std::cmp::Reverse(id.index()))
            });
            for (id, bytes) in arrays {
                if local_bytes <= allowance {
                    break;
                }
                promoted[function].insert(id);
                local_bytes -= bytes;
            }
        }
        path_bytes[function] = callee_bytes.saturating_add(local_bytes);
        processed[function] = true;
        for &caller in &callers[function] {
            pending_callees[caller] -= 1;
            if pending_callees[caller] == 0 {
                queue.push_back(caller);
            }
        }
    }

    // Recursive MIR is rejected during validation. Conservatively move every
    // small array on a cycle (and in callers depending on it) out of the stack
    // so malformed diagnostic input cannot bypass the budget first.
    for (function, processed) in processed.into_iter().enumerate() {
        if !processed {
            promoted[function].extend(plans[function].small.iter().map(|(id, _)| *id));
        }
    }
    promoted
}

fn collect_callees(block: &MirBlock, function_count: usize, callees: &mut HashSet<usize>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Call { function, .. } if function.index() < function_count => {
                callees.insert(function.index());
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_callees(then_block, function_count, callees);
                collect_callees(else_block, function_count, callees);
            }
            StatementKind::Loop { body } => collect_callees(body, function_count, callees),
            _ => {}
        }
    }
}

fn fixed_array_bytes(types: &[MirType], ty: TypeId) -> Option<u64> {
    let MirType::Array { element, len } = types.get(ty.index())? else {
        return None;
    };
    let MirType::Scalar(element) = types.get(element.index())? else {
        return None;
    };
    u64::from(*len).checked_mul(element.logical_byte_width())
}

fn collect_storage_lifetimes(
    block: &MirBlock,
    position: &mut usize,
    lifetimes: &mut [StorageLifetime],
    dependencies: &mut Vec<(LocalId, LocalId)>,
    direct_references: &mut HashSet<LocalId>,
) {
    for statement in &block.statements {
        let current = *position;
        *position += 1;
        direct_references.clear();
        onda_mir::collect_direct_local_references(statement, direct_references);
        for &local in direct_references.iter() {
            if let Some(lifetime) = lifetimes.get_mut(local.index()) {
                lifetime.record(current);
            }
        }
        match &statement.kind {
            StatementKind::Assign {
                destination:
                    Place {
                        base: PlaceBase::Local(destination),
                        projections,
                    },
                value,
            } if projections.is_empty() => {
                let source = match value {
                    Rvalue::Use(Value::Local(source)) => Some(*source),
                    Rvalue::MakeSlice {
                        source:
                            onda_mir::SliceSource::Place(Place {
                                base: PlaceBase::Local(source),
                                ..
                            }),
                        ..
                    } => Some(*source),
                    _ => None,
                };
                if let Some(source) = source {
                    dependencies.push((source, *destination));
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_storage_lifetimes(
                    then_block,
                    position,
                    lifetimes,
                    dependencies,
                    direct_references,
                );
                collect_storage_lifetimes(
                    else_block,
                    position,
                    lifetimes,
                    dependencies,
                    direct_references,
                );
            }
            StatementKind::Loop { body } => {
                collect_storage_lifetimes(
                    body,
                    position,
                    lifetimes,
                    dependencies,
                    direct_references,
                );
            }
            _ => {}
        }
    }
}

fn extend_backing_lifetimes(
    lifetimes: &mut [StorageLifetime],
    dependencies: &[(LocalId, LocalId)],
) {
    loop {
        let mut changed = false;
        for &(source, dependent) in dependencies {
            let dependent_last = lifetimes
                .get(dependent.index())
                .map_or(0, |lifetime| lifetime.last);
            if let Some(source) = lifetimes.get_mut(source.index()) {
                if source.last < dependent_last {
                    source.last = dependent_last;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

pub(super) fn rewrite_storage(block: &mut MirBlock, slots: &HashMap<LocalId, onda_mir::StateId>) {
    for statement in &mut block.statements {
        rewrite_statement_storage(statement, slots);
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                rewrite_storage(then_block, slots);
                rewrite_storage(else_block, slots);
            }
            StatementKind::Loop { body } => rewrite_storage(body, slots),
            _ => {}
        }
    }
}

pub(super) fn rewrite_statement_storage(
    statement: &mut Statement,
    slots: &HashMap<LocalId, onda_mir::StateId>,
) {
    fn place(place: &mut Place, slots: &HashMap<LocalId, onda_mir::StateId>) {
        if let PlaceBase::Local(local) = place.base {
            if let Some(state) = slots.get(&local) {
                place.base = PlaceBase::State(*state);
            }
        }
    }
    match &mut statement.kind {
        StatementKind::Assign { destination, value } => {
            place(destination, slots);
            match value {
                Rvalue::Use(Value::Local(local)) if slots.contains_key(local) => {
                    *value = Rvalue::Load(Place {
                        base: PlaceBase::State(slots[local]),
                        projections: Vec::new(),
                    });
                }
                Rvalue::Load(source)
                | Rvalue::MakeSlice {
                    source: onda_mir::SliceSource::Place(source),
                    ..
                } => place(source, slots),
                _ => {}
            }
        }
        StatementKind::Call { args, .. } | StatementKind::PublishDelegate { args, .. } => {
            for argument in args {
                match argument {
                    CallArgument::Place(source)
                    | CallArgument::ArrayWindow { array: source, .. } => place(source, slots),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array_function(
        name: &str,
        array_ty: TypeId,
        count: usize,
        callee: Option<FunctionId>,
    ) -> onda_mir::Function {
        let locals = (0..count)
            .map(|_| onda_mir::Local {
                name: None,
                ty: array_ty,
                integer_range: None,
            })
            .collect::<Vec<_>>();
        let mut statements = (0..count)
            .map(|index| {
                let local = LocalId::new(index as u32);
                Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(local),
                        value: Rvalue::Load(Place::local(local)),
                    },
                    source: SourceSpan::UNKNOWN,
                }
            })
            .collect::<Vec<_>>();
        if let Some(function) = callee {
            statements.push(Statement {
                kind: StatementKind::Call {
                    results: Vec::new(),
                    function,
                    args: Vec::new(),
                },
                source: SourceSpan::UNKNOWN,
            });
        }
        onda_mir::Function {
            name: name.to_owned(),
            kind: onda_mir::FunctionKind::User,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals,
            body: MirBlock { statements },
            source: SourceSpan::UNKNOWN,
        }
    }

    #[test]
    fn local_array_budget_covers_the_complete_call_path() {
        let scalar = TypeId::new(0);
        let array = TypeId::new(1);
        let arrays_per_function =
            (MAX_LOCAL_FIXED_ARRAY_PATH_BYTES / MAX_LOCAL_FIXED_ARRAY_BYTES / 2 + 1) as usize;
        let mut program = onda_mir::Program::new(
            onda_mir::CompileConfig::new(48_000.0, 64).unwrap(),
            FunctionId::new(1),
            FunctionId::new(1),
        );
        program.types = vec![
            MirType::Scalar(ScalarType::F32),
            MirType::Array {
                element: scalar,
                len: (MAX_LOCAL_FIXED_ARRAY_BYTES / 4) as u32,
            },
        ];
        program.functions = vec![
            array_function("leaf", array, arrays_per_function, None),
            array_function(
                "caller",
                array,
                arrays_per_function,
                Some(FunctionId::new(0)),
            ),
        ];

        let promoted = fixed_array_promotions(&program);
        assert!(promoted[0].is_empty());
        assert_eq!(promoted[1].len(), 2);
        let retained =
            (arrays_per_function * 2 - promoted[1].len()) as u64 * MAX_LOCAL_FIXED_ARRAY_BYTES;
        assert_eq!(retained, MAX_LOCAL_FIXED_ARRAY_PATH_BYTES);
    }
}
