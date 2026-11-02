//! Logical scalar children borrow their owner's flattened physical fields.

use super::*;

impl FunctionLowerer<'_> {
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
        let processor = self.processor_view(root, block)?;
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
