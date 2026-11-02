//! Sparse, dependency-driven provenance for ephemeral buffer and slice values.
use super::*;
use std::collections::VecDeque;

#[derive(Debug, Default)]
pub(super) struct ResourceAliases {
    values: HashMap<usize, HashSet<ResourceOrigin>>,
    collections: HashMap<usize, Vec<HashSet<ResourceOrigin>>>,
    #[cfg(test)]
    assignment_visits: usize,
}

impl ResourceAliases {
    pub(super) fn get(&self, index: usize) -> Option<&HashSet<ResourceOrigin>> {
        self.values.get(&index)
    }
    pub(super) fn all(&self, local: LocalId) -> HashSet<ResourceOrigin> {
        self.collections.get(&local.index()).map_or_else(
            || self.get(local.index()).cloned().unwrap_or_default(),
            |slots| {
                slots
                    .iter()
                    .flat_map(|origins| origins.iter().copied())
                    .collect()
            },
        )
    }
    pub(super) fn slot(&self, local: LocalId, slot: usize) -> HashSet<ResourceOrigin> {
        self.collections
            .get(&local.index())
            .and_then(|slots| slots.get(slot))
            .cloned()
            .unwrap_or_else(|| self.all(local))
    }
}

fn resource_type<'a>(
    program: &'a Program,
    function: &Function,
    local: LocalId,
) -> Option<&'a Type> {
    function
        .locals
        .get(local.index())
        .and_then(|local| program.types.get(local.ty.index()))
        .filter(|ty| {
            matches!(
                ty,
                Type::Slice { .. }
                    | Type::Buffer { .. }
                    | Type::BufferSpan { .. }
                    | Type::BufferMap { .. }
            )
        })
}

pub(super) fn infer_local_resource_aliases(
    program: &Program,
    function: &Function,
) -> ResourceAliases {
    let mut aliases = ResourceAliases::default();
    if !function.locals.iter().any(|local| {
        matches!(
            program.types.get(local.ty.index()),
            Some(
                Type::Slice { .. }
                    | Type::Buffer { .. }
                    | Type::BufferSpan { .. }
                    | Type::BufferMap { .. }
            )
        )
    }) {
        return aliases;
    }
    // Only resource assignments need nodes. In particular, large numeric state
    // layouts allocate no per-local provenance sets or dependency vectors.
    let mut assignments = Vec::new();
    let mut blocks = vec![&function.body];
    while let Some(block) = blocks.pop() {
        for statement in &block.statements {
            match &statement.kind {
                StatementKind::Assign {
                    destination:
                        Place {
                            base: PlaceBase::Local(local),
                            projections,
                        },
                    value,
                } if projections.is_empty()
                    && resource_type(program, function, *local).is_some() =>
                {
                    assignments.push((*local, value));
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    blocks.push(else_block);
                    blocks.push(then_block);
                }
                StatementKind::Loop { body } => blocks.push(body),
                _ => {}
            }
        }
    }
    let mut dependents: HashMap<LocalId, Vec<usize>> = HashMap::new();
    for (index, (_, value)) in assignments.iter().enumerate() {
        if let Some(source) = source_local(value) {
            dependents.entry(source).or_default().push(index);
        }
    }
    let mut queued = vec![true; assignments.len()];
    let mut pending: VecDeque<_> = (0..assignments.len()).collect();
    while let Some(index) = pending.pop_front() {
        queued[index] = false;
        #[cfg(test)]
        {
            aliases.assignment_visits += 1;
        }
        let (local, value) = assignments[index];
        let origins = rvalue_resource_origins(program, function, value, &aliases);
        let slots = resource_type(program, function, local)
            .and_then(Type::buffer_collection)
            .map(|(_, _, _, len, _)| {
                (0..len as usize)
                    .map(|slot| match value {
                        Rvalue::MakeBuffer { source, .. } => call_argument_resource_origins(
                            program, function, source, &aliases, slot,
                        )
                        .unwrap_or_default(),
                        Rvalue::Use(Value::Local(source)) => aliases.slot(*source, slot),
                        Rvalue::Load(Place {
                            base: PlaceBase::Local(source),
                            ..
                        }) => aliases.slot(*source, slot),
                        Rvalue::Load(Place {
                            base: PlaceBase::Parameter(parameter),
                            ..
                        }) => HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                            parameter: parameter.index(),
                            slot,
                        })]),
                        _ => HashSet::new(),
                    })
                    .collect::<Vec<_>>()
            });
        let destination = aliases.values.entry(local.index()).or_default();
        let mut changed = extend(destination, origins);
        if let Some(slots) = slots {
            let destination = aliases.collections.entry(local.index()).or_default();
            destination.resize_with(slots.len(), HashSet::new);
            for (destination, origins) in destination.iter_mut().zip(slots) {
                changed |= extend(destination, origins);
            }
        }
        if changed {
            for &dependent in dependents.get(&local).into_iter().flatten() {
                if !queued[dependent] {
                    queued[dependent] = true;
                    pending.push_back(dependent);
                }
            }
        }
    }
    aliases
}

fn extend(destination: &mut HashSet<ResourceOrigin>, origins: HashSet<ResourceOrigin>) -> bool {
    let len = destination.len();
    destination.extend(origins);
    destination.len() != len
}

fn source_local(value: &Rvalue) -> Option<LocalId> {
    let place = |place: &Place| match place.base {
        PlaceBase::Local(local) => Some(local),
        _ => None,
    };
    match value {
        Rvalue::Use(Value::Local(local)) => Some(*local),
        Rvalue::Load(source) => place(source),
        Rvalue::MakeSlice {
            source: SliceSource::Place(source),
            ..
        } => place(source),
        Rvalue::MakeSlice {
            source: SliceSource::BufferParam { parameter, .. },
            ..
        } => parameter.local(),
        Rvalue::MakeBuffer { source, .. } => match source {
            CallArgument::Value(Value::Local(local)) => Some(*local),
            CallArgument::Place(source) => place(source),
            CallArgument::BufferParam(reference) => reference.local(),
            CallArgument::BufferSpan(crate::BufferSpanRef::Local { collection, .. }) => {
                Some(*collection)
            }
            _ => None,
        },
        _ => None,
    }
}

fn rvalue_resource_origins(
    program: &Program,
    function: &Function,
    value: &Rvalue,
    aliases: &ResourceAliases,
) -> HashSet<ResourceOrigin> {
    match value {
        Rvalue::MakeBuffer { source, .. } => {
            call_argument_resource_origins(program, function, source, aliases, usize::MAX)
                .unwrap_or_default()
        }
        Rvalue::Use(value) => value_resource_origins(*value, aliases),
        Rvalue::Load(place) => place_resource_origins(place, aliases),
        Rvalue::MakeSlice { source, .. } => match source {
            SliceSource::Buffer { buffer, .. } => buffer_ref_resource_origins(*buffer),
            SliceSource::BufferParam { parameter, .. } => {
                buffer_param_resource_origins(program, function, *parameter, aliases)
            }
            SliceSource::Place(place) => place_resource_origins(place, aliases),
            SliceSource::ConstData(_) => HashSet::new(),
        },
        _ => HashSet::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompileConfig, FunctionAttributes, Local, SourceSpan, Statement, TypeId};

    fn resource_chain(count: usize) -> Program {
        let mut program = Program::new(
            CompileConfig::new(48_000.0, 64).unwrap(),
            FunctionId::new(0),
            FunctionId::new(0),
        );
        program.types.push(Type::Buffer {
            element: ScalarType::F32,
            channels: crate::BufferChannels::Mono,
            access: crate::AccessMode::ReadWrite,
        });
        let mut function = Function {
            name: "copies".into(),
            kind: FunctionKind::User,
            attributes: FunctionAttributes::default(),
            params: Vec::new(),
            results: Vec::new(),
            locals: (0..count)
                .map(|_| Local {
                    name: None,
                    ty: TypeId::new(0),
                    integer_range: None,
                })
                .collect(),
            body: Block::default(),
            source: SourceSpan::UNKNOWN,
        };
        for index in (1..count).rev() {
            function.body.statements.push(Statement {
                kind: StatementKind::Assign {
                    destination: Place::local(LocalId::new(index as u32)),
                    value: Rvalue::Use(Value::Local(LocalId::new(index as u32 - 1))),
                },
                source: SourceSpan::UNKNOWN,
            });
        }
        function.body.statements.push(Statement {
            kind: StatementKind::Assign {
                destination: Place::local(LocalId::new(0)),
                value: Rvalue::MakeBuffer {
                    source: CallArgument::Buffer(crate::BufferRef::Direct(BufferId::new(7))),
                    ty: TypeId::new(0),
                },
            },
            source: SourceSpan::UNKNOWN,
        });
        program.functions.push(function);
        program
    }

    #[test]
    fn reversed_descriptor_copy_chain_uses_linear_work() {
        let program = resource_chain(10_000);
        let aliases = infer_local_resource_aliases(&program, &program.functions[0]);
        assert_eq!(
            aliases.get(9_999),
            Some(&HashSet::from([ResourceOrigin::Buffer(7)]))
        );
        assert!(aliases.assignment_visits <= 20_000);
    }

    #[test]
    fn slice_parameter_origins_follow_reversed_copy_chains() {
        let mut program = resource_chain(10_000);
        program.types[0] = Type::Slice {
            element: ScalarType::F32,
            access: crate::AccessMode::ReadWrite,
        };
        let function = &mut program.functions[0];
        function.params.push(crate::FunctionParam {
            name: "samples".into(),
            ty: TypeId::new(0),
            mode: crate::PassingMode::Value,
            integer_range: None,
        });
        let StatementKind::Assign { value, .. } =
            &mut function.body.statements.last_mut().unwrap().kind
        else {
            unreachable!()
        };
        *value = Rvalue::Load(Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: Vec::new(),
        });
        let aliases = infer_local_resource_aliases(&program, &program.functions[0]);
        assert_eq!(
            aliases.get(9_999),
            Some(&HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                parameter: 0,
                slot: 0
            })]))
        );
        assert!(aliases.assignment_visits <= 20_000);
    }

    #[test]
    fn copied_collection_values_keep_each_slot_and_the_complete_union() {
        let mut program = resource_chain(2);
        program.types[0] = Type::BufferSpan {
            element: ScalarType::F32,
            channels: crate::BufferChannels::Mono,
            access: crate::AccessMode::ReadWrite,
            len: 3,
        };
        let function = &mut program.functions[0];
        function.params.push(crate::FunctionParam {
            name: "buffers".into(),
            ty: TypeId::new(0),
            mode: crate::PassingMode::Value,
            integer_range: None,
        });
        let StatementKind::Assign { value, .. } =
            &mut function.body.statements.last_mut().unwrap().kind
        else {
            unreachable!()
        };
        *value = Rvalue::Load(Place {
            base: PlaceBase::Parameter(ParameterId::new(0)),
            projections: Vec::new(),
        });
        let aliases = infer_local_resource_aliases(&program, &program.functions[0]);
        let all = (0..3)
            .map(|slot| ResourceOrigin::Parameter(ParameterSlot { parameter: 0, slot }))
            .collect();
        assert_eq!(aliases.get(1), Some(&all));
        assert_eq!(aliases.slot(LocalId::new(1), usize::MAX), all);
        assert_eq!(
            aliases.slot(LocalId::new(1), 2),
            HashSet::from([ResourceOrigin::Parameter(ParameterSlot {
                parameter: 0,
                slot: 2
            })])
        );
    }

    #[test]
    fn numeric_locals_allocate_no_resource_provenance() {
        let mut program = resource_chain(10_000);
        program.types[0] = Type::Scalar(ScalarType::F32);
        let aliases = infer_local_resource_aliases(&program, &program.functions[0]);
        assert_eq!(aliases.assignment_visits, 0);
        assert_eq!(aliases.values.capacity(), 0);
        assert_eq!(aliases.collections.capacity(), 0);
    }
}
