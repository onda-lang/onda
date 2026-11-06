//! Sparse provenance with symbolic collection windows and demand-driven slots.
use super::*;
use std::collections::VecDeque;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
enum CollectionSource {
    Interface {
        first: usize,
    },
    Parameter {
        parameter: usize,
        start: usize,
    },
    Map {
        map: usize,
        start: usize,
    },
    Local {
        local: LocalId,
        starts: Range<usize>,
    },
}

#[derive(Debug)]
struct CollectionAliases {
    len: usize,
    sources: Vec<CollectionSource>,
}

#[derive(Debug, Default)]
pub(super) struct ResourceAliases<'a> {
    values: HashMap<usize, HashSet<ResourceOrigin>>,
    collections: HashMap<usize, CollectionAliases>,
    maps: &'a [crate::BufferMap],
    #[cfg(test)]
    assignment_visits: usize,
}

impl ResourceAliases<'_> {
    #[cfg(test)]
    fn get(&self, index: usize) -> Option<&HashSet<ResourceOrigin>> {
        self.values.get(&index)
    }

    pub(super) fn all(&self, local: LocalId) -> HashSet<ResourceOrigin> {
        self.collections.get(&local.index()).map_or_else(
            || self.values.get(&local.index()).cloned().unwrap_or_default(),
            |collection| self.range(local, 0..collection.len),
        )
    }

    pub(super) fn slot(&self, local: LocalId, slot: usize) -> HashSet<ResourceOrigin> {
        match self.collections.get(&local.index()) {
            Some(collection) if slot < collection.len => self.range(local, slot..slot + 1),
            _ => self.all(local),
        }
    }

    /// Translate one demanded interval through the collection graph. A dynamic
    /// start maps an interval to another interval, never to a table of slot sets.
    /// The worklist also handles reversed definitions and descriptor cycles.
    pub(super) fn range(&self, local: LocalId, range: Range<usize>) -> HashSet<ResourceOrigin> {
        let mut result = HashSet::new();
        let mut pending = vec![(local, range.start, range.end)];
        let mut visited = HashSet::new();
        while let Some((local, start, end)) = pending.pop() {
            if start >= end || !visited.insert((local, start, end)) {
                continue;
            }
            let Some(collection) = self.collections.get(&local.index()) else {
                result.extend(
                    self.values
                        .get(&local.index())
                        .into_iter()
                        .flatten()
                        .copied(),
                );
                continue;
            };
            let end = end.min(collection.len);
            for source in &collection.sources {
                match source {
                    CollectionSource::Interface { first } => {
                        result.extend((first + start..first + end).map(ResourceOrigin::Buffer));
                    }
                    CollectionSource::Parameter {
                        parameter,
                        start: offset,
                    } => {
                        result.extend((offset + start..offset + end).map(|slot| {
                            ResourceOrigin::Parameter(ParameterSlot {
                                parameter: *parameter,
                                slot,
                            })
                        }));
                    }
                    CollectionSource::Map { map, start: offset } => {
                        if let Some(entries) = self
                            .maps
                            .get(*map)
                            .and_then(|map| map.buffers.get(offset + start..offset + end))
                        {
                            result.extend(
                                entries
                                    .iter()
                                    .map(|buffer| ResourceOrigin::Buffer(buffer.index())),
                            );
                        }
                    }
                    CollectionSource::Local { local, starts } if !starts.is_empty() => {
                        pending.push((*local, starts.start + start, starts.end - 1 + end));
                    }
                    CollectionSource::Local { .. } => {}
                }
            }
        }
        result
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

fn collection_source(
    program: &Program,
    function: &Function,
    value: &Rvalue,
) -> Option<CollectionSource> {
    let place = |place: &Place| match place.base {
        PlaceBase::Local(local) => Some(CollectionSource::Local {
            local,
            starts: 0..1,
        }),
        PlaceBase::Parameter(parameter) => Some(CollectionSource::Parameter {
            parameter: parameter.index(),
            start: 0,
        }),
        _ => None,
    };
    match value {
        Rvalue::Use(Value::Local(local)) => Some(CollectionSource::Local {
            local: *local,
            starts: 0..1,
        }),
        Rvalue::Load(source) => place(source),
        Rvalue::MakeBuffer { source, .. } => match source {
            CallArgument::Value(Value::Local(local)) => Some(CollectionSource::Local {
                local: *local,
                starts: 0..1,
            }),
            CallArgument::Place(source) => place(source),
            CallArgument::BufferSpan(crate::BufferSpanRef::Interface { first, .. }) => {
                Some(CollectionSource::Interface {
                    first: first.index(),
                })
            }
            CallArgument::BufferSpan(crate::BufferSpanRef::MappedInterface {
                map, start, ..
            }) => Some(CollectionSource::Map {
                map: map.index(),
                start: *start as usize,
            }),
            CallArgument::BufferSpan(crate::BufferSpanRef::Parameter { span, start, .. }) => {
                Some(CollectionSource::Parameter {
                    parameter: span.index(),
                    start: *start as usize,
                })
            }
            CallArgument::BufferSpan(crate::BufferSpanRef::Local {
                collection,
                start,
                len,
                bounds,
            }) => {
                let (_, _, _, source_len, _) =
                    resource_type(program, function, *collection)?.buffer_collection()?;
                Some(CollectionSource::Local {
                    local: *collection,
                    starts: selected_slot_range(
                        *start,
                        source_len.saturating_sub(*len).saturating_add(1) as usize,
                        *bounds,
                    ),
                })
            }
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn infer_local_resource_aliases<'a>(
    program: &'a Program,
    function: &Function,
) -> ResourceAliases<'a> {
    let mut aliases = ResourceAliases {
        maps: &program.buffer_maps,
        ..Default::default()
    };
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
                    if let Some((_, _, _, len, _)) =
                        resource_type(program, function, *local).and_then(Type::buffer_collection)
                    {
                        let collection =
                            aliases.collections.entry(local.index()).or_insert_with(|| {
                                CollectionAliases {
                                    len: len as usize,
                                    sources: Vec::new(),
                                }
                            });
                        if let Some(source) = collection_source(program, function, value) {
                            if !collection.sources.contains(&source) {
                                collection.sources.push(source);
                            }
                        }
                    } else {
                        assignments.push((*local, value));
                    }
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => blocks.extend([else_block, then_block]),
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
        let destination = aliases.values.entry(local.index()).or_default();
        let before = destination.len();
        destination.extend(origins);
        if destination.len() != before {
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

fn source_local(value: &Rvalue) -> Option<LocalId> {
    let place = |place: &Place| match place.base {
        PlaceBase::Local(local) => Some(local),
        _ => None,
    };
    match value {
        Rvalue::Use(Value::Local(local)) => Some(*local),
        Rvalue::Load(source)
        | Rvalue::MakeSlice {
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
        assert_eq!(aliases.all(LocalId::new(1)), all);
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
