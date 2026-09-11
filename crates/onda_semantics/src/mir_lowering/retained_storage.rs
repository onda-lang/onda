use super::*;

pub(super) struct BlockRegion {
    pub statement: usize,
    pub locals: std::ops::Range<usize>,
    pub extents: Vec<ViewExtent>,
    pub abortable: bool,
}

pub(super) struct ViewExtent {
    pub length: LocalId,
    pub tensors: Vec<(LocalId, u32)>,
}

/// Keep owned block storage in snapshots and reconstruct carried views from
/// their storage roots and captured coordinates. Only descriptor formation is
/// repeated: authored computations, calls, initializers, and copies execute
/// once in block-pre. No native address enters persistent state.
pub(super) fn retain_block_storage(
    function: &mut onda_mir::Function,
    region: BlockRegion,
    state: &mut Vec<onda_mir::StateSlot>,
    types: &[MirType],
) -> Result<(), MirLoweringError> {
    let continuation = MirBlock {
        statements: function.body.statements[region.statement + 1..].to_vec(),
    };
    let mut retained = onda_mir::referenced_locals(&continuation);
    retained.retain(|local| region.locals.contains(&local.index()));
    if retained.is_empty() {
        return Ok(());
    }
    let extents = region
        .extents
        .into_iter()
        .filter(|extent| retained.contains(&extent.length))
        .collect::<Vec<_>>();
    for extent in &extents {
        retained.extend(extent.tensors.iter().map(|(slice, _)| *slice));
    }
    let StatementKind::If { then_block, .. } = &function.body.statements[region.statement].kind
    else {
        unreachable!("block-pre region is guarded by BEGIN_BLOCK")
    };
    let mut declarations = if region.abortable {
        let [Statement {
            kind: StatementKind::Loop { body },
            ..
        }] = then_block.statements.as_slice()
        else {
            unreachable!("abortable block-pre region is wrapped in one activation loop")
        };
        body.clone()
    } else {
        then_block.clone()
    };
    let (restoration, owned) = retain_storage(
        &mut declarations,
        &mut function.locals,
        retained,
        &extents,
        state,
        types,
        "block",
        function.source,
        &HashSet::new(),
    )?;
    let StatementKind::If {
        then_block,
        else_block,
        ..
    } = &mut function.body.statements[region.statement].kind
    else {
        unreachable!()
    };
    if region.abortable {
        let [Statement {
            kind: StatementKind::Loop { body },
            ..
        }] = then_block.statements.as_mut_slice()
        else {
            unreachable!()
        };
        *body = declarations;
    } else {
        *then_block = declarations;
    }
    *else_block = restoration;
    // Scalar leaves can also occur as ordinary MIR values. Load such values
    // at their use, while all address-taking and stores target the state slot.
    load_scalar_uses(&mut function.body, &owned, &function.locals, types);
    Ok(())
}

/// Plan storage once, then reuse the same reconstruction at each entry that
/// can observe it. Persistent slots contain data and selections, never addresses.
#[allow(clippy::too_many_arguments)]
pub(super) fn retain_storage(
    declarations: &mut MirBlock,
    locals: &mut Vec<onda_mir::Local>,
    mut retained: HashSet<LocalId>,
    extents: &[ViewExtent],
    state: &mut Vec<onda_mir::StateSlot>,
    types: &[MirType],
    scope: &str,
    source: SourceSpan,
    forbidden: &HashSet<LocalId>,
) -> Result<(MirBlock, HashMap<LocalId, onda_mir::StateId>), MirLoweringError> {
    let derived_lengths = extents
        .iter()
        .filter(|extent| !extent.tensors.is_empty())
        .map(|extent| extent.length)
        .collect::<HashSet<_>>();
    extend_storage_dependencies(declarations, locals, types, &mut retained, true)?;
    if !retained.is_disjoint(forbidden) {
        return Err(MirLoweringError::new(
            "persistent data view borrows init-local storage; declare independent fixed data to retain its contents",
            SourceLoc::ZERO,
        ));
    }
    let mut planner = Planner {
        locals,
        state,
        types,
        retained: &retained,
        scope,
        owned: HashMap::new(),
    };
    let mut owned = retained.iter().copied().collect::<Vec<_>>();
    owned.sort();
    for local in owned {
        if !planner.is_slice(local) && !derived_lengths.contains(&local) {
            let slot = planner.slot(local, "storage")?;
            planner.owned.insert(local, slot);
        }
    }
    let mut restoration = planner.reconstruct(declarations)?;
    // Logical lengths are properties of the restored tensors, not independent
    // snapshot values. This also bounds accesses if a snapshot was modified.
    planner.restore_extents(extents, &mut restoration, source)?;
    Ok((restoration, planner.owned))
}

/// Follow only address provenance, excluding loaded contents and coordinates.
pub(super) fn extend_storage_dependencies(
    block: &MirBlock,
    locals: &[onda_mir::Local],
    types: &[MirType],
    retained: &mut HashSet<LocalId>,
    validate_origins: bool,
) -> Result<(), MirLoweringError> {
    loop {
        let before = retained.len();
        collect_dependencies(block, locals, types, retained, validate_origins)?;
        if before == retained.len() {
            return Ok(());
        }
    }
}

fn collect_dependencies(
    block: &MirBlock,
    locals: &[onda_mir::Local],
    types: &[MirType],
    retained: &mut HashSet<LocalId>,
    validate_origins: bool,
) -> Result<(), MirLoweringError> {
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign { destination, value }
                if matches!(destination.base, PlaceBase::Local(local) if retained.contains(&local)
                    && matches!(types[locals[local.index()].ty.index()], MirType::Slice { .. })) =>
            {
                let source = match value {
                    Rvalue::MakeSlice {
                        source: onda_mir::SliceSource::Place(place),
                        ..
                    }
                    | Rvalue::Load(place) => Some(place.base),
                    Rvalue::Use(Value::Local(local)) => Some(PlaceBase::Local(*local)),
                    Rvalue::MakeSlice {
                        source: onda_mir::SliceSource::ConstData(_),
                        ..
                    } => None,
                    _ if validate_origins => return Err(escape_error()),
                    _ => None,
                };
                match source {
                    Some(PlaceBase::Local(local)) => {
                        retained.insert(local);
                    }
                    Some(PlaceBase::Parameter(_) | PlaceBase::EventParam(_)) => {
                        if validate_origins {
                            return Err(escape_error());
                        }
                    }
                    Some(PlaceBase::State(_) | PlaceBase::Param(_)) | None => {}
                }
            }
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                collect_dependencies(then_block, locals, types, retained, validate_origins)?;
                collect_dependencies(else_block, locals, types, retained, validate_origins)?;
            }
            StatementKind::Loop { body } => {
                collect_dependencies(body, locals, types, retained, validate_origins)?
            }
            _ => {}
        }
    }
    Ok(())
}

fn escape_error() -> MirLoweringError {
    MirLoweringError::new(
        "block-carried data view borrows storage that cannot survive a process boundary",
        SourceLoc::ZERO,
    )
}

struct Planner<'a> {
    scope: &'a str,
    locals: &'a mut Vec<onda_mir::Local>,
    state: &'a mut Vec<onda_mir::StateSlot>,
    types: &'a [MirType],
    retained: &'a HashSet<LocalId>,
    owned: HashMap<LocalId, onda_mir::StateId>,
}

impl Planner<'_> {
    fn restore_extents(
        &mut self,
        extents: &[ViewExtent],
        block: &mut MirBlock,
        source: SourceSpan,
    ) -> Result<(), MirLoweringError> {
        let bool_type = self
            .types
            .iter()
            .position(|ty| *ty == MirType::Scalar(ScalarType::Bool));
        for extent in extents {
            if extent.tensors.is_empty() {
                let slot = self.owned[&extent.length];
                block.statements.push(Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(extent.length),
                        value: Rvalue::Load(Place {
                            base: PlaceBase::State(slot),
                            projections: Vec::new(),
                        }),
                    },
                    source,
                });
                continue;
            }
            for (index, (slice, width)) in extent.tensors.iter().enumerate() {
                let length = self.local(self.locals[extent.length.index()].ty)?;
                block.statements.push(Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(length),
                        value: Rvalue::SliceLen(Value::Local(*slice)),
                    },
                    source,
                });
                if *width != 1 {
                    block.statements.push(Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(length),
                            value: Rvalue::Binary {
                                op: MirBinaryOp::Divide,
                                lhs: Value::Local(length),
                                rhs: Value::Constant(ScalarValue::I32(*width as i32)),
                            },
                        },
                        source,
                    });
                }
                let assign = Statement {
                    kind: StatementKind::Assign {
                        destination: Place::local(extent.length),
                        value: Rvalue::Use(Value::Local(length)),
                    },
                    source,
                };
                if index == 0 {
                    block.statements.push(assign);
                } else {
                    let condition = self.local(TypeId::new(
                        bool_type.expect("process has a boolean BEGIN_BLOCK condition") as u32,
                    ))?;
                    block.statements.push(Statement {
                        kind: StatementKind::Assign {
                            destination: Place::local(condition),
                            value: Rvalue::Compare {
                                op: CompareOp::Less,
                                lhs: Value::Local(length),
                                rhs: Value::Local(extent.length),
                            },
                        },
                        source,
                    });
                    block.statements.push(Statement {
                        kind: StatementKind::If {
                            condition: Value::Local(condition),
                            then_block: MirBlock {
                                statements: vec![assign],
                            },
                            else_block: MirBlock::default(),
                        },
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    fn local(&mut self, ty: TypeId) -> Result<LocalId, MirLoweringError> {
        let id = u32::try_from(self.locals.len()).map_err(|_| {
            MirLoweringError::new(
                "retained data exceeds the local slot limit",
                SourceLoc::ZERO,
            )
        })?;
        self.locals.push(onda_mir::Local {
            name: None,
            ty,
            integer_range: None,
        });
        Ok(LocalId::new(id))
    }

    fn is_slice(&self, local: LocalId) -> bool {
        matches!(
            self.types[self.locals[local.index()].ty.index()],
            MirType::Slice { .. }
        )
    }

    fn slot(
        &mut self,
        local: LocalId,
        purpose: &str,
    ) -> Result<onda_mir::StateId, MirLoweringError> {
        let id = u32::try_from(self.state.len()).map_err(|_| {
            MirLoweringError::new(
                "retained data exceeds the state slot limit",
                SourceLoc::ZERO,
            )
        })?;
        let local = &self.locals[local.index()];
        self.state.push(onda_mir::StateSlot {
            name: format!(
                "__onda_{}.{purpose}.{id}.{}",
                self.scope,
                local.name.as_deref().unwrap_or("temporary")
            ),
            ty: local.ty,
            persistence: onda_mir::StatePersistence::Snapshot,
            authored: false,
            pinned: false,
            integer_range: local.integer_range,
        });
        Ok(onda_mir::StateId::new(id))
    }

    fn capture(
        &mut self,
        value: &mut Value,
        capture: &mut MirBlock,
        restore: &mut MirBlock,
        source: SourceSpan,
    ) -> Result<(), MirLoweringError> {
        let Value::Local(local) = *value else {
            return Ok(());
        };
        if self.is_slice(local) {
            return Ok(());
        }
        let slot = self.slot(local, "selection")?;
        let restored = self.local(self.locals[local.index()].ty)?;
        let place = Place {
            base: PlaceBase::State(slot),
            projections: Vec::new(),
        };
        capture.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: place.clone(),
                value: Rvalue::Use(*value),
            },
            source,
        });
        restore.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(restored),
                value: Rvalue::Load(place),
            },
            source,
        });
        *value = Value::Local(restored);
        Ok(())
    }

    fn reconstruct(&mut self, block: &mut MirBlock) -> Result<MirBlock, MirLoweringError> {
        let mut captured = MirBlock::default();
        let mut restore = MirBlock::default();
        for mut statement in std::mem::take(&mut block.statements) {
            let source = statement.source;
            match &mut statement.kind {
                StatementKind::Assign { destination, value } if matches!(destination.base, PlaceBase::Local(local) if self.retained.contains(&local) && self.is_slice(local)) =>
                {
                    let destination = destination.clone();
                    let mut value = value.clone();
                    let mut coordinates = MirBlock::default();
                    match &mut value {
                        Rvalue::MakeSlice {
                            source: slice_source,
                            start,
                            len,
                            bounds,
                            ..
                        } => {
                            // Snapshot coordinates are scalar data. Recheck
                            // them before recreating a native address.
                            if *bounds == BoundsMode::Unchecked {
                                *bounds = BoundsMode::Checked;
                            }
                            self.capture(start, &mut coordinates, &mut restore, source)?;
                            self.capture(len, &mut coordinates, &mut restore, source)?;
                            if let onda_mir::SliceSource::Place(place) = slice_source {
                                for projection in &mut place.projections {
                                    if let Projection::Index { index, bounds } = projection {
                                        if *bounds == BoundsMode::Unchecked {
                                            *bounds = BoundsMode::Checked;
                                        }
                                        self.capture(
                                            index,
                                            &mut coordinates,
                                            &mut restore,
                                            source,
                                        )?;
                                    }
                                }
                            }
                        }
                        Rvalue::Use(_) | Rvalue::Load(_) => {}
                        _ => return Err(escape_error()),
                    }
                    captured.statements.push(statement);
                    captured.statements.extend(coordinates.statements);
                    restore.statements.push(Statement {
                        kind: StatementKind::Assign { destination, value },
                        source,
                    });
                    continue;
                }
                StatementKind::If {
                    condition,
                    then_block,
                    else_block,
                } => {
                    let restored_then = self.reconstruct(then_block)?;
                    let restored_else = self.reconstruct(else_block)?;
                    if !restored_then.statements.is_empty() || !restored_else.statements.is_empty()
                    {
                        let mut selected = *condition;
                        self.capture(&mut selected, &mut captured, &mut restore, source)?;
                        restore.statements.push(Statement {
                            kind: StatementKind::If {
                                condition: selected,
                                then_block: restored_then,
                                else_block: restored_else,
                            },
                            source,
                        });
                    }
                }
                StatementKind::Loop { body } => {
                    if !self.reconstruct(body)?.statements.is_empty() {
                        return Err(MirLoweringError::new(
                            "data view cannot retain a loop-local selection outside its iteration",
                            SourceLoc::ZERO,
                        ));
                    }
                }
                _ => {}
            }
            captured.statements.push(statement);
        }
        *block = captured;
        Ok(restore)
    }
}

pub(super) fn load_scalar_uses(
    block: &mut MirBlock,
    slots: &HashMap<LocalId, onda_mir::StateId>,
    locals: &[onda_mir::Local],
    types: &[MirType],
) {
    let mut statements = Vec::new();
    for mut statement in std::mem::take(&mut block.statements) {
        let references = match &statement.kind {
            StatementKind::If {
                condition: Value::Local(local),
                ..
            } => HashSet::from([*local]),
            StatementKind::If { .. } | StatementKind::Loop { .. } => HashSet::new(),
            _ => onda_mir::referenced_locals(&MirBlock {
                statements: vec![statement.clone()],
            }),
        };
        let mut reads = references
            .into_iter()
            .filter(|local| {
                slots.contains_key(local)
                    && matches!(types[locals[local.index()].ty.index()], MirType::Scalar(_))
            })
            .collect::<Vec<_>>();
        reads.sort();
        for local in reads {
            statements.push(Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(local),
                    value: Rvalue::Load(Place {
                        base: PlaceBase::State(slots[&local]),
                        projections: Vec::new(),
                    }),
                },
                source: statement.source,
            });
        }
        match &mut statement.kind {
            StatementKind::If {
                then_block,
                else_block,
                ..
            } => {
                load_scalar_uses(then_block, slots, locals, types);
                load_scalar_uses(else_block, slots, locals, types);
            }
            StatementKind::Loop { body } => load_scalar_uses(body, slots, locals, types),
            _ => {}
        }
        // Reload destinations stay local; authored writes target state.
        storage::rewrite_statement_storage(&mut statement, slots);
        statements.push(statement);
    }
    block.statements = statements;
}
