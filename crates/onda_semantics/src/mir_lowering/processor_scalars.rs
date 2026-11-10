//! Logical scalar children borrow their owner's flattened physical fields.

use super::*;

impl FunctionLowerer<'_> {
    /// Reconstruct a selected logical owner from persistent roots and captured
    /// coordinates, including array axes between scalar child projections.
    pub(super) fn processor_environment_state(
        &mut self,
        frame: &processor_views::LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<String, MirLoweringError> {
        let state = frame.shape.state.as_ref().ok_or_else(|| {
            self.error(
                "processor resource owner lost its state shape",
                location.clone(),
            )
        })?;
        let capture = frame.state_root.as_ref().ok_or_else(|| {
            self.error(
                "processor resource owner lost its state root",
                location.clone(),
            )
        })?;
        let mut root = self.materialize_processor_storage_capture(
            capture,
            &state.storage,
            block,
            location.clone(),
        )?;
        let axes = std::iter::once((None, state.storage.len.is_some().then_some(0))).chain(
            state.projection.edges.iter().map(|edge| {
                (
                    Some(edge.declaration.as_str()),
                    edge.coordinate_axis
                        .map(|axis| axis + usize::from(state.storage.len.is_some())),
                )
            }),
        );
        for (field, axis) in axes {
            if let Some(field) = field {
                root = format!("{root}.{field}");
            }
            if let Some(axis) = axis {
                let coordinate = frame.coordinates.get(axis).copied().ok_or_else(|| {
                    self.error(
                        "processor resource owner lost its selection coordinate",
                        location.clone(),
                    )
                })?;
                let selected = self.fresh_data_name();
                let index = self.captured_value_expression(coordinate, block, location.clone());
                let expression = Expr::Index {
                    loc: location.clone().into(),
                    base: root,
                    index: Box::new(index),
                };
                if !self.lower_struct_array_element_alias(
                    &selected,
                    &expression,
                    block,
                    location.clone(),
                )? {
                    return Err(self.error(
                        "processor resource owner could not materialize its state",
                        location,
                    ));
                }
                root = selected;
            } else {
                self.prepare_scalar_processor_view(&root, block, location.clone())?;
            }
        }
        Ok(root)
    }

    pub(super) fn prepare_scalar_processor_view(
        &mut self,
        root: &str,
        block: &mut MirBlock,
        loc: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if self.bindings.contains_key(root) {
            return Ok(());
        }
        let Some((owner, field)) = root.rsplit_once('.') else {
            return Ok(());
        };
        self.prepare_scalar_processor_view(owner, block, loc.clone())?;
        let Some(owner_type) = self.owner_struct_name(owner) else {
            return Ok(());
        };
        let Some(declaration) = self.processor_views.declaration(owner_type, field) else {
            return Ok(());
        };
        if declaration.len.is_some() {
            return Ok(());
        }
        let proc_name = declaration.proc_name.clone();
        let processor = self.processor_view(root)?;
        for shape in self.struct_field_shapes(&proc_name, loc.clone())? {
            let path = match &shape {
                StructFieldShape::Scalar { name, .. } | StructFieldShape::Array { name, .. } => {
                    name
                }
            };
            let physical = format!("{owner}.{field}__{path}");
            let logical = format!("{root}.{path}");
            let binding = if let Some(binding) = self.bindings.get(&physical).cloned() {
                binding
            } else if let StructFieldShape::Array { element, len, .. } = shape {
                let slice = self.lower_named_slice(
                    &physical,
                    SliceSelection::default(),
                    None,
                    block,
                    loc.clone(),
                )?;
                let Value::Local(local) = slice.value else {
                    unreachable!("a named slice is materialized in a local")
                };
                Binding::Slice(local, element, slice.access, Some(len))
            } else {
                let (state, ty) = self
                    .runtime_globals_for_unbound(&physical)
                    .and_then(|globals| globals.states.get(&physical))
                    .copied()
                    .ok_or_else(|| {
                        self.error(
                            format!("processor projection '{root}' has no field '{path}'"),
                            loc.clone(),
                        )
                    })?;
                Binding::PlaceAlias(
                    Place {
                        base: PlaceBase::State(state),
                        projections: Vec::new(),
                    },
                    ty,
                )
            };
            self.bindings.insert(logical, binding);
        }
        self.bindings.insert(
            root.to_owned(),
            Binding::StructView {
                struct_name: proc_name,
                processor,
            },
        );
        Ok(())
    }
}
