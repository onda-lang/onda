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

/// Small fixed arrays are deliberately left as entry-block locals: native
/// backends can scalar-replace them, while moving them into instance state
/// makes every element access observable memory traffic. Larger arrays live in
/// prepared instance scratch so real-time stack use remains bounded.
///
/// Block/task-carried storage is already explicit state before this pass.
/// Acyclic calls therefore permit one scratch frame per function. Compatible
/// slots are reused after all dependent values and views are dead;
/// simultaneously live data remains disjoint.
const MAX_LOCAL_FIXED_ARRAY_BYTES: u64 = 64;

pub(super) fn plan_fixed_scratch(
    program: &mut onda_mir::Program,
) -> Result<(), Vec<MirLoweringError>> {
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
                (referenced.contains(&id)
                    && fixed_array_bytes(&program.types, local.ty)
                        .is_some_and(|bytes| bytes > MAX_LOCAL_FIXED_ARRAY_BYTES))
                .then_some((id, local, lifetimes[index]))
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
