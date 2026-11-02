use super::*;
use retained_storage::{load_scalar_uses, retain_storage, ViewExtent};

/// A compact entry prelude and its lexical bindings. Locals are private to
/// each invocation; only the data roots and captured selections persist.
#[derive(Debug)]
pub(super) struct RetainedBindings {
    pub locals: Vec<onda_mir::Local>,
    pub bindings: HashMap<String, Binding>,
    pub restore: MirBlock,
}

pub(super) struct InitRetentionContext<'a> {
    pub state: &'a mut Vec<onda_mir::StateSlot>,
    pub types: &'a [MirType],
    pub source_files: &'a [onda_mir::SourceFile],
    pub layouts: &'a AggregateLayoutTable,
}

pub(super) fn retain_init_bindings(
    init: &mut onda_mir::Function,
    mut bindings: HashMap<String, Binding>,
    views: &HashMap<String, SourceLoc>,
    local_names: &HashSet<String>,
    context: InitRetentionContext<'_>,
) -> Result<RetainedBindings, MirLoweringError> {
    let InitRetentionContext {
        state,
        types,
        source_files,
        layouts,
    } = context;
    let mut forbidden = HashSet::new();
    for (name, binding) in &mut bindings {
        if has_matching_root(name, |root| local_names.contains(root)) {
            visit_binding_locals(binding, &mut |local| {
                forbidden.insert(*local);
            });
        }
    }
    retained_storage::extend_storage_dependencies(
        &init.body,
        &init.locals,
        types,
        &mut forbidden,
        false,
        source_files,
    )?;
    forbidden.retain(|local| {
        !matches!(
            types[init.locals[local.index()].ty.index()],
            MirType::Slice { .. }
        )
    });
    let mut retention_roots = views.keys().cloned().collect::<HashSet<_>>();
    let mut pending = retention_roots.iter().cloned().collect::<Vec<_>>();
    while let Some(root) = pending.pop() {
        for (name, binding) in &bindings {
            if !crate::path_is_within_root(name, &root) {
                continue;
            }
            for capture in processor_capture_roots(binding) {
                if retention_roots.insert(capture.clone()) {
                    pending.push(capture);
                }
            }
        }
    }
    bindings.retain(|name, _| has_matching_root(name, |root| retention_roots.contains(root)));
    let mut retained = HashSet::new();
    for binding in bindings.values_mut() {
        visit_binding_locals(binding, &mut |local| {
            retained.insert(*local);
        });
    }
    retained_storage::extend_storage_dependencies(
        &init.body,
        &init.locals,
        types,
        &mut retained,
        true,
        source_files,
    )?;
    if !retained.is_disjoint(&forbidden) {
        let mut roots = views.iter().collect::<Vec<_>>();
        roots.sort_by_key(|(name, location)| (location.line, location.column, name.as_str()));
        for (root, location) in &roots {
            let mut candidate = HashSet::new();
            for (name, binding) in &mut bindings {
                if name == *root
                    || name
                        .strip_prefix(root.as_str())
                        .is_some_and(|suffix| suffix.starts_with('.'))
                {
                    visit_binding_locals(binding, &mut |local| {
                        candidate.insert(*local);
                    });
                }
            }
            retained_storage::extend_storage_dependencies(
                &init.body,
                &init.locals,
                types,
                &mut candidate,
                true,
                source_files,
            )?;
            if !candidate.is_disjoint(&forbidden) {
                return Err(MirLoweringError::new(
                    format!(
                        "persistent data view '{root}' borrows init-local storage; declare independent fixed data to retain its contents"
                    ),
                    (*location).clone(),
                ));
            }
        }
        return Err(MirLoweringError::new(
            "persistent data view borrows init-local storage; declare independent fixed data to retain its contents",
            roots
                .first()
                .map(|(_, location)| (*location).clone())
                .unwrap_or(SourceLoc::ZERO),
        ));
    }
    let extents = view_extents(&bindings, layouts)?;
    let (mut restore, owned) = retain_storage(
        &mut init.body,
        &mut init.locals,
        retained,
        &extents,
        state,
        types,
        source_files,
        "init",
        init.source,
    )?;
    load_scalar_uses(&mut init.body, &owned, &init.locals, types);
    load_scalar_uses(&mut restore, &owned, &init.locals, types);
    for binding in bindings.values_mut() {
        match binding {
            Binding::Local(local, ty) if owned.contains_key(local) => {
                *binding = Binding::PlaceAlias(
                    Place {
                        base: PlaceBase::State(owned[local]),
                        projections: Vec::new(),
                    },
                    *ty,
                );
            }
            Binding::PlaceAlias(place, _) => {
                if let PlaceBase::Local(local) = place.base {
                    if let Some(slot) = owned.get(&local) {
                        place.base = PlaceBase::State(*slot);
                    }
                }
            }
            _ => {}
        }
    }
    // Some captured values are consumed directly from binding metadata, such
    // as an owner's selected row or an environment tag. They need an entry
    // load even when no reconstructed descriptor reads them in the prelude.
    let mut captured_values = HashSet::new();
    for binding in bindings.values_mut() {
        visit_binding_locals(binding, &mut |local| {
            if owned.contains_key(local)
                && matches!(
                    types[init.locals[local.index()].ty.index()],
                    MirType::Scalar(_)
                )
            {
                captured_values.insert(*local);
            }
        });
    }
    let mut captured_values = captured_values.into_iter().collect::<Vec<_>>();
    captured_values.sort();
    restore
        .statements
        .extend(captured_values.into_iter().map(|local| Statement {
            kind: StatementKind::Assign {
                destination: Place::local(local),
                value: Rvalue::Load(Place {
                    base: PlaceBase::State(owned[&local]),
                    projections: Vec::new(),
                }),
            },
            source: init.source,
        }));
    // Compact before sharing the prelude: unrelated init temporaries must not
    // inflate every process, event, and runtime helper's local frame.
    let mut live = onda_mir::referenced_locals(&restore);
    for binding in bindings.values_mut() {
        visit_binding_locals(binding, &mut |local| {
            live.insert(*local);
        });
    }
    let mut mapping = vec![None; init.locals.len()];
    let mut locals = Vec::new();
    for (index, local) in init.locals.iter().enumerate() {
        if live.contains(&LocalId::new(index as u32)) {
            mapping[index] = Some(LocalId::new(locals.len() as u32));
            locals.push(local.clone());
        }
    }
    onda_mir::rewrite_block_locals(&mut restore, &mapping);
    for binding in bindings.values_mut() {
        visit_binding_locals(binding, &mut |local| {
            *local = mapping[local.index()].expect("binding local retained");
        });
    }
    Ok(RetainedBindings {
        locals,
        bindings,
        restore,
    })
}

fn has_matching_root(name: &str, contains: impl Fn(&str) -> bool) -> bool {
    contains(name)
        || name
            .match_indices('.')
            .any(|(index, _)| contains(&name[..index]))
}

pub(super) fn view_extents(
    bindings: &HashMap<String, Binding>,
    layouts: &AggregateLayoutTable,
) -> Result<Vec<ViewExtent>, MirLoweringError> {
    let mut extents = Vec::new();
    let mut seen = HashSet::new();
    for binding in bindings.values() {
        let Binding::StructArrayParameter {
            struct_name,
            length: StructArrayLength::Local(length),
            fields,
            ..
        } = binding
        else {
            continue;
        };
        if !seen.insert(*length) {
            continue;
        }
        let layout = layouts
            .layout_for_struct(struct_name)
            .expect("resolved struct layout");
        let tensors = layout
            .leaves
            .iter()
            .zip(fields)
            .map(|(leaf, (_, local, _))| {
                u32::try_from(leaf.tensor.element_count)
                    .map(|width| (*local, width))
                    .map_err(|_| {
                        MirLoweringError::new(
                            "retained view tensor width exceeds u32",
                            SourceLoc::ZERO,
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        extents.push(ViewExtent {
            length: *length,
            tensors,
        });
    }
    extents.sort_by_key(|extent| extent.length);
    Ok(extents)
}

fn visit_binding_locals(binding: &mut Binding, visit: &mut impl FnMut(&mut LocalId)) {
    fn value(value: &mut Value, visit: &mut impl FnMut(&mut LocalId)) {
        if let Value::Local(local) = value {
            visit(local);
        }
    }
    fn visit_environment(
        environment: &mut processor_views::LoweredProcessorEnvironment,
        visit: &mut impl FnMut(&mut LocalId),
    ) {
        if let Some(owner) = &mut environment.owner {
            owner.visit_values(&mut |tag| value(tag, visit));
        }
        if let Some(root) = &mut environment.state_root {
            root.visit_values(&mut |tag| value(tag, visit));
        }
        for resource in &mut environment.resources {
            resource.visit_values(&mut |tag| value(tag, visit));
        }
        for coordinate in &mut environment.coordinates {
            value(coordinate, visit);
        }
        if let Some(outer) = &mut environment.outer {
            visit_environment(outer, visit);
        }
    }
    if let Some(processor) = match binding {
        Binding::StructView { processor, .. }
        | Binding::StructParameter { processor, .. }
        | Binding::StructArrayParameter { processor, .. }
        | Binding::ProcArrayParameter { processor, .. } => processor.as_mut(),
        _ => None,
    } {
        if let Some(tag) = &mut processor.tag {
            value(tag, visit);
        }
        for capture in &mut processor.environments {
            visit_environment(capture, visit);
        }
    }
    match binding {
        Binding::Local(local, _) | Binding::Array(local, _, _) | Binding::Slice(local, _, _, _) => {
            visit(local)
        }
        Binding::BufferValue(local, _) => visit(local),
        Binding::BufferCollectionValue { local, start, .. } => {
            visit(local);
            value(start, visit);
        }
        Binding::BufferAlias(
            BufferBindingReference::Interface(onda_mir::BufferRef::ArrayElement {
                selector, ..
            }),
            _,
        )
        | Binding::BufferAlias(
            BufferBindingReference::Parameter(onda_mir::BufferParamRef::ArrayElement {
                selector,
                ..
            }),
            _,
        ) => {
            value(selector, visit);
        }
        Binding::SliceElementAlias { slice, index, .. } => {
            visit(slice);
            value(index, visit);
        }
        Binding::Tuple(fields) => {
            for (local, _) in fields {
                visit(local);
            }
        }
        Binding::TupleSliceElementAlias(fields) => {
            for (local, _, index) in fields {
                visit(local);
                value(index, visit);
            }
        }
        Binding::PlaceAlias(place, _) => {
            if let PlaceBase::Local(local) = &mut place.base {
                visit(local);
            }
            for projection in &mut place.projections {
                if let Projection::Index { index, .. } = projection {
                    value(index, visit);
                }
            }
        }
        Binding::StructArrayParameter { length, fields, .. } => {
            if let StructArrayLength::Local(length) = length {
                visit(length);
            }
            for (_, local, _) in fields {
                visit(local);
            }
        }
        Binding::ProcArrayParameter { active, fields, .. } => {
            if let Some(active) = active {
                visit(active);
            }
            for (_, local, _) in fields {
                visit(local);
            }
        }
        Binding::InitAll
        | Binding::ReferenceParameter(..)
        | Binding::EventParameter(..)
        | Binding::EventArrayParameter(..)
        | Binding::BufferParameter(..)
        | Binding::BufferParameterArray(..)
        | Binding::BufferAlias(..)
        | Binding::ArrayParameter(..)
        | Binding::TupleReferenceParameter(..)
        | Binding::StructParameter { .. }
        | Binding::StructView { .. }
        | Binding::StructArrayStorage { .. } => {}
    }
}

fn processor_capture_roots(binding: &Binding) -> Vec<String> {
    fn capture_roots(
        environment: &processor_views::LoweredProcessorEnvironment,
        result: &mut Vec<String>,
    ) {
        if let Some(owner) = &environment.owner {
            owner.visit_leaves(&mut |root| result.push(root.clone()));
        }
        if let Some(root) = &environment.state_root {
            root.visit_leaves(&mut |root| result.push(root.clone()));
        }
        for (resource, shape) in environment
            .resources
            .iter()
            .zip(&environment.shape.resources)
        {
            if shape.binding.is_some() {
                continue;
            }
            resource.visit_leaves(&mut |resource| {
                for expression in resource.walk() {
                    match expression {
                        Expr::Var { name, .. } | Expr::Index { base: name, .. } => {
                            result.push(name.clone())
                        }
                        _ => {}
                    }
                }
            });
        }
        if let Some(outer) = &environment.outer {
            capture_roots(outer, result);
        }
    }
    let processor = match binding {
        Binding::StructView { processor, .. }
        | Binding::StructParameter { processor, .. }
        | Binding::StructArrayParameter { processor, .. }
        | Binding::ProcArrayParameter { processor, .. } => processor.as_ref(),
        _ => None,
    };
    let mut result = Vec::new();
    if let Some(processor) = processor {
        for capture in &processor.environments {
            capture_roots(capture, &mut result);
        }
    }
    result
}
