//! Completion of locally owned processors follows calls and loop iterations.
//! Borrowed views never acquire a second completion obligation.
use super::processor_captures::ProcessorCapture;
use super::*;
use crate::processor_views::ProcessorStorageShape;

#[derive(Clone)]
pub(super) struct ScopedProcessor {
    root: String,
    storage: ProcessorStorageShape,
    initialized: LocalId,
}

impl FunctionLowerer<'_> {
    pub(super) fn register_owned_processor_lifetime(
        &mut self,
        root: &str,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let Some(view) = self.processor_view(root)? else {
            return Ok(());
        };
        if !self.proc_array_hook_types.contains(&view.shape.proc_name) {
            return Ok(());
        }
        // Copies inherit data, never an already running child's activation.
        if self
            .processor_views
            .has_lifecycle_children(&view.shape.proc_name)
        {
            if let Some(len) = view.shape.array_len() {
                self.processor_lifecycle_elements(
                    root,
                    len,
                    block,
                    location.clone(),
                    |this, root, block| {
                        this.processor_owner_hooks(
                            root,
                            &view.shape.proc_name,
                            false,
                            block,
                            location.clone(),
                        )
                    },
                )?;
            } else {
                self.processor_owner_hooks(
                    root,
                    &view.shape.proc_name,
                    false,
                    block,
                    location.clone(),
                )?;
            }
        }
        if !self.local_processor_lifetimes {
            return Ok(());
        }

        // Keep declaration storage and callback captures available after branch
        // scopes merge or a later declaration shadows their source names.
        let mut bindings = self.bindings.clone();
        let preserved = self.preserve_processor_capture_roots(
            &mut bindings,
            &HashMap::new(),
            &[root.to_owned()],
        );
        let binding = bindings.get(root).expect("owned processor binding");
        let view = match binding {
            Binding::StructView {
                processor: Some(view),
                ..
            }
            | Binding::StructArrayParameter {
                processor: Some(view),
                ..
            }
            | Binding::ProcArrayParameter {
                processor: Some(view),
                ..
            } => view,
            _ => unreachable!("owned processor view"),
        };
        let Some(ProcessorCapture::Direct(root)) = &view.environments[0].state_root else {
            unreachable!("owned processor has one storage root")
        };
        let root = root.clone();
        let storage = ProcessorStorageShape {
            proc_name: view.shape.proc_name.clone(),
            len: view.shape.array_len(),
        };
        self.scoped_processor_bindings.extend(preserved);
        let initialized = self.new_local(None, PrimitiveType::Bool);
        self.assign_value(
            block,
            initialized,
            Value::Constant(ScalarValue::Bool(true)),
            location,
        );
        self.scoped_processors.push(ScopedProcessor {
            root,
            storage,
            initialized,
        });
        Ok(())
    }

    /// An untaken declaration must not read any of its branch-local captures.
    /// Return the prefix length so recorded statement positions can be rebased.
    pub(super) fn initialize_scoped_processors(&mut self, body: &mut MirBlock) -> usize {
        let mut entry = MirBlock::default();
        // Cleanup is guarded by declaration initialization, but MIR does not
        // infer that this also initializes every captured coordinate and tag.
        // Publish neutral scalar values without evaluating resource recipes.
        let mut captures = std::collections::BTreeSet::new();
        for binding in self.scoped_processor_bindings.values_mut() {
            if let Some(view) = binding.processor_view_mut() {
                view.visit_values(&mut |value| {
                    if let Value::Local(local) = value {
                        captures.insert(*local);
                    }
                });
            }
        }
        for local in captures {
            debug_assert_eq!(
                self.types[self.locals[local.index()].ty.index()],
                MirType::Scalar(onda_mir::ScalarType::I32)
            );
            self.assign_value(
                &mut entry,
                local,
                zero_value(PrimitiveType::I32),
                function_location(self.function),
            );
        }
        for processor in self.scoped_processors.clone() {
            self.assign_value(
                &mut entry,
                processor.initialized,
                Value::Constant(ScalarValue::Bool(false)),
                function_location(self.function),
            );
        }
        let prefix_len = entry.statements.len();
        entry.statements.append(&mut body.statements);
        *body = entry;
        prefix_len
    }

    pub(super) fn finish_scoped_processors(
        &mut self,
        first: usize,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if first == self.scoped_processors.len() {
            return Ok(());
        }
        let outer = self.bindings.clone();
        self.bindings.extend(self.scoped_processor_bindings.clone());
        for index in first..self.scoped_processors.len() {
            let processor = self.scoped_processors[index].clone();
            let mut completion = MirBlock::default();
            // Rebuild owned column references inside the initialized branch;
            // declaration-local descriptors do not survive a branch join.
            let root = self.materialize_processor_storage_capture(
                &ProcessorCapture::Direct(processor.root),
                &processor.storage,
                &mut completion,
                location.clone(),
            )?;
            let finish = |this: &mut Self, root: &str, block: &mut MirBlock| {
                this.processor_owner_hooks(
                    root,
                    &processor.storage.proc_name,
                    true,
                    block,
                    location.clone(),
                )?;
                this.finish_processor_view_slot(
                    root,
                    &processor.storage.proc_name,
                    block,
                    location.clone(),
                )
            };
            if let Some(len) = processor.storage.len {
                self.processor_lifecycle_elements(
                    &root,
                    len,
                    &mut completion,
                    location.clone(),
                    finish,
                )?;
            } else {
                finish(self, &root, &mut completion)?;
            }
            self.assign_value(
                &mut completion,
                processor.initialized,
                Value::Constant(ScalarValue::Bool(false)),
                location.clone(),
            );
            self.push_statement(
                block,
                StatementKind::If {
                    condition: Value::Local(processor.initialized),
                    then_block: completion,
                    else_block: MirBlock::default(),
                },
                location.clone(),
            );
        }
        self.bindings = outer;
        Ok(())
    }
}
