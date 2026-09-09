use super::*;

/// Fixed invocation arrays live in prepared instance scratch. Acyclic calls
/// permit one frame per function: simultaneously active functions have disjoint
/// frames, and multiple live call results occupy distinct caller slots. Carried
/// block/task storage is already explicit state before this pass.
pub(super) fn plan_fixed_scratch(
    program: &mut onda_mir::Program,
) -> Result<(), Vec<MirLoweringError>> {
    for (function_id, function) in program.functions.iter_mut().enumerate() {
        let mut slots = HashMap::new();
        let referenced = onda_mir::referenced_locals(&function.body);
        let mut addressed = HashSet::new();
        collect_slice_backing_locals(&function.body, &mut addressed);
        for (local_id, local) in function.locals.iter().enumerate() {
            if !referenced.contains(&LocalId::new(local_id as u32)) {
                continue;
            }
            if !matches!(
                program.types.get(local.ty.index()),
                Some(MirType::Array { .. })
            ) && !(matches!(
                program.types.get(local.ty.index()),
                Some(MirType::Scalar(_))
            ) && addressed.contains(&LocalId::new(local_id as u32)))
            {
                continue;
            }
            let raw_id = u32::try_from(program.state.len()).map_err(|_| {
                vec![MirLoweringError::new(
                    "planned data scratch exceeds the state slot limit",
                    SourceLoc::ZERO,
                )]
            })?;
            let state = onda_mir::StateId::new(raw_id);
            slots.insert(LocalId::new(local_id as u32), state);
            program.state.push(onda_mir::StateSlot {
                name: format!(
                    "__onda_scratch.{function_id}.{local_id}.{}",
                    local.name.as_deref().unwrap_or("temporary")
                ),
                ty: local.ty,
                persistence: onda_mir::StatePersistence::InstanceScratch,
                authored: false,
                pinned: false,
                integer_range: local.integer_range,
            });
        }
        if !slots.is_empty() {
            rewrite_storage(&mut function.body, &slots);
        }
    }
    Ok(())
}

fn collect_slice_backing_locals(block: &MirBlock, locals: &mut HashSet<LocalId>) {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign {
                value:
                    Rvalue::MakeSlice {
                        source:
                            onda_mir::SliceSource::Place(Place {
                                base: PlaceBase::Local(local),
                                ..
                            }),
                        ..
                    },
                ..
            } => {
                locals.insert(*local);
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_slice_backing_locals(then_block, locals);
                collect_slice_backing_locals(else_block, locals);
            }
            StatementKind::Loop { body } => collect_slice_backing_locals(body, locals),
            _ => {}
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
