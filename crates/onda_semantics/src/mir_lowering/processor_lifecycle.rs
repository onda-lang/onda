//! Lifecycle identity follows the same roots and coordinates as processor state.
use super::processor_captures::ProcessorCapture;
use super::processor_views::LoweredProcessorEnvironment;
use super::*;
use crate::processor_views::ProcessorOperation;

pub(super) fn owned_active_name(root: &str) -> String {
    // Keep lifecycle storage under its owner so capture preservation and
    // retention traverse it with the ordinary storage leaves.
    format!("{root}.__onda_proc_block_active")
}

impl FunctionLowerer<'_> {
    pub(super) fn initialize_owned_processor_lifecycle(
        &mut self,
        root: &str,
        shape: &crate::processor_views::ProcessorViewShape,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if !self.proc_array_hook_types.contains(&shape.proc_name)
            || (shape.array_len().is_none() && !self.local_processor_lifetimes)
        {
            return Ok(());
        }
        let len = shape.array_len().unwrap_or(1);
        let name = owned_active_name(root);
        if self.bindings.contains_key(&name)
            || self.runtime_globals.is_some_and(|globals| {
                globals
                    .state_arrays
                    .contains_key(&runtime_proc_array_active_symbol(root))
            })
        {
            return Ok(());
        }
        let len = self.data_extent(len, location.clone())?;
        let local = self.new_array_local(Some(name.clone()), PrimitiveType::Bool, len);
        self.bindings.insert(
            name.clone(),
            Binding::Array(local, PrimitiveType::Bool, len),
        );
        let active = self.lower_named_slice(
            &name,
            SliceSelection::default(),
            Some(onda_mir::AccessMode::ReadWrite),
            block,
            location.clone(),
        )?;
        self.push_statement(
            block,
            StatementKind::SliceFill {
                destination: active.value,
                value: Value::Constant(ScalarValue::Bool(false)),
            },
            location,
        );
        Ok(())
    }

    pub(super) fn processor_boundary_hooks(
        &mut self,
        finish: bool,
        sample: &[Stmt],
        scheduled_post: &[Stmt],
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let outer = self.bindings.clone();
        let owners = self
            .processor_views
            .lifecycle_owner_types(sample, crate::processor_views::ProcessorBoundary::Sample);
        let covered = scheduled_post
            .iter()
            .filter_map(|statement| {
                let Stmt::Expr {
                    expr: Expr::UserCall { name, args, .. },
                    ..
                } = statement
                else {
                    return None;
                };
                let operation = self.processor_views.operation(name)?;
                let Expr::Var { name: root, .. } = &args.first()?.expr else {
                    return None;
                };
                (operation.kind == ProcessorOperation::BlockPost
                    && operation.projection.edges.is_empty())
                .then_some(root.as_str())
            })
            .collect::<HashSet<_>>();
        // Scheduled parents can own completion, but descendant activation
        // resets once at BEGIN_BLOCK, before either a child or its parent steps.
        let mut roots = self
            .processor_views
            .roots
            .iter()
            .filter(|(root, source)| {
                (!finish || !covered.contains(root.as_str()))
                    && owners.contains(&source.shape.proc_name)
                    && self
                        .processor_views
                        .has_lifecycle_children(&source.shape.proc_name)
            })
            .map(|(root, source)| (root.clone(), source.shape.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        // Retained owned copies have independent lifecycle slots. Aliases keep
        // their source root and must not schedule the same storage twice.
        for (name, binding) in &self.bindings {
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
                _ => continue,
            };
            if owners.contains(&view.shape.proc_name)
                && (self.processor_views.has_lifecycle_children(&view.shape.proc_name)
                    || self.bindings.contains_key(&owned_active_name(name)))
                && view.environments.iter().all(|frame| {
                    matches!(&frame.state_root, Some(ProcessorCapture::Direct(root)) if root == name)
                        && frame.shape.state.as_ref().is_some_and(|state| !state.local_lifecycle && state.projection.edges.is_empty())
                })
            {
                roots.insert(name.clone(), view.shape.clone());
            }
        }
        for (root, shape) in roots {
            let owned_active = self.bindings.contains_key(&owned_active_name(&root));
            if owned_active && !finish {
                let active = self.processor_root_active_slice(
                    &ProcessorCapture::Direct(root.clone()),
                    block,
                    location.clone(),
                )?;
                self.push_statement(
                    block,
                    StatementKind::SliceFill {
                        destination: active.value,
                        value: Value::Constant(ScalarValue::Bool(false)),
                    },
                    location.clone(),
                );
            }
            if let Some(len) = shape.array_len() {
                self.processor_lifecycle_elements(
                    &root,
                    len,
                    block,
                    location.clone(),
                    |this, selected, body| {
                        this.processor_owner_hooks(
                            selected,
                            &shape.proc_name,
                            finish,
                            body,
                            location.clone(),
                        )?;
                        if owned_active && finish {
                            this.finish_processor_view_slot(
                                selected,
                                &shape.proc_name,
                                body,
                                location.clone(),
                            )?;
                        }
                        Ok(())
                    },
                )?;
            } else {
                self.processor_owner_hooks(
                    &root,
                    &shape.proc_name,
                    finish,
                    block,
                    location.clone(),
                )?;
            }
        }
        // Descriptors created in BEGIN_BLOCK must not escape to later segments.
        self.bindings = outer;
        Ok(())
    }

    /// Complete descendants before entering the owner's post body or its task
    /// guard. An independently borrowed descendant need not activate its owner.
    pub(super) fn finish_processor_function_children(
        &mut self,
        block: &mut MirBlock,
    ) -> Result<(), MirLoweringError> {
        let Some(owner) = self
            .processor_views
            .post_hook_descendant_owner(&self.function.name)
        else {
            return Ok(());
        };
        let owner = owner.to_owned();
        self.processor_owner_hooks(
            "self",
            &owner,
            true,
            block,
            function_location(self.function),
        )?;
        Ok(())
    }

    pub(super) fn processor_owner_hooks(
        &mut self,
        root: &str,
        owner: &str,
        finish: bool,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let declarations = self
            .processor_views
            .declarations()
            .iter()
            .filter(|declaration| declaration.owner_struct == owner)
            .cloned()
            .collect::<Vec<_>>();
        for declaration in declarations {
            let descendants = self
                .processor_views
                .has_lifecycle_children(&declaration.proc_name);
            if declaration.active_field.is_none() && !descendants {
                continue;
            }
            let child = format!("{root}.{}", declaration.field_name);
            self.prepare_scalar_processor_view(root, block, location.clone())?;
            if let Some(field) = &declaration.active_field {
                if !finish {
                    let active = self.lower_named_slice(
                        &format!("{root}.{field}"),
                        SliceSelection::default(),
                        Some(onda_mir::AccessMode::ReadWrite),
                        block,
                        location.clone(),
                    )?;
                    self.push_statement(
                        block,
                        StatementKind::SliceFill {
                            destination: active.value,
                            value: Value::Constant(ScalarValue::Bool(false)),
                        },
                        location.clone(),
                    );
                }
            }
            if let Some(len) = declaration.len {
                if finish || descendants {
                    self.processor_lifecycle_elements(
                        &child,
                        len,
                        block,
                        location.clone(),
                        |this, selected, body| {
                            if descendants {
                                this.processor_owner_hooks(
                                    selected,
                                    &declaration.proc_name,
                                    finish,
                                    body,
                                    location.clone(),
                                )?;
                            }
                            if finish && declaration.active_field.is_some() {
                                this.finish_processor_view_slot(
                                    selected,
                                    &declaration.proc_name,
                                    body,
                                    location.clone(),
                                )?;
                            }
                            Ok(())
                        },
                    )?;
                }
            } else {
                if descendants {
                    self.processor_owner_hooks(
                        &child,
                        &declaration.proc_name,
                        finish,
                        block,
                        location.clone(),
                    )?;
                }
                if finish && declaration.active_field.is_some() {
                    self.prepare_scalar_processor_view(&child, block, location.clone())?;
                    self.finish_processor_view_slot(
                        &child,
                        &declaration.proc_name,
                        block,
                        location.clone(),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn processor_lifecycle_elements(
        &mut self,
        root: &str,
        len: usize,
        block: &mut MirBlock,
        location: SourceLoc,
        mut element: impl FnMut(&mut Self, &str, &mut MirBlock) -> Result<(), MirLoweringError>,
    ) -> Result<(), MirLoweringError> {
        let index = self.new_local(None, PrimitiveType::I32);
        let mut body = MirBlock::default();
        let selected = self.fresh_data_name();
        let expression = Expr::Index {
            loc: location.clone().into(),
            base: root.to_owned(),
            index: Box::new(self.captured_value_expression(
                Value::Local(index),
                &mut body,
                location.clone(),
            )),
        };
        if !self.lower_struct_array_element_alias(
            &selected,
            &expression,
            &mut body,
            location.clone(),
        )? {
            return Err(self.error("processor lifecycle lost its declared array", location));
        }
        element(self, &selected, &mut body)?;
        self.emit_counted_loop(block, index, len as i32, body, location);
        Ok(())
    }

    pub(super) fn finish_processor_view_slot(
        &mut self,
        receiver: &str,
        proc_name: &str,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let view = self
            .processor_view(receiver)?
            .ok_or_else(|| self.error("processor post-hook lost its view", location.clone()))?;
        self.dispatch_processor_environments(
            &view,
            block,
            location.clone(),
            |this, frame, block| {
                this.finish_processor_environment_slot(
                    receiver,
                    proc_name,
                    frame,
                    block,
                    location.clone(),
                )
            },
        )
    }

    fn finish_processor_environment_slot(
        &mut self,
        receiver: &str,
        proc_name: &str,
        frame: &LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let projection = crate::processor_views::ProcessorProjection {
            owner: proc_name.to_owned(),
            edges: Vec::new(),
        };
        let name = self
            .processor_views
            .operation_target(&projection, &ProcessorOperation::BlockPost)
            .ok_or_else(|| self.error("processor post-hook lost its operation", location.clone()))?
            .to_owned();
        let operation = self
            .processor_views
            .operation(&name)
            .expect("registered operation")
            .clone();
        let target = self.processor_operation_target(&name, &operation, frame);
        // Unactivated scalar hooks are absent after runtime dependency
        // selection, so their constants remain unevaluated.
        if !self.function_indices.contains_key(&target) {
            return Ok(());
        }
        let active = self.processor_environment_active_slice(frame, block, location.clone())?;
        let coordinate =
            processor_lifecycle_coordinate(frame).expect("selected processor lifecycle coordinate");
        let condition = self.emit_temp(
            block,
            PrimitiveType::Bool,
            Rvalue::SliceLoad {
                slice: active.value,
                index: coordinate,
                bounds: BoundsMode::Unchecked,
            },
            location.clone(),
        );
        let mut then_block = MirBlock::default();
        let receiver = onda_frontend::CallArg {
            name: None,
            expr: Expr::var(receiver),
        };
        let (target, arguments) = self.processor_operation_arguments(
            &name,
            &[receiver],
            &operation,
            frame,
            &mut then_block,
            location.clone(),
        )?;
        self.lower_direct_user_call_into(
            &target,
            &[],
            &arguments,
            None,
            location.clone(),
            &mut then_block,
        )?;
        self.push_statement(
            &mut then_block,
            StatementKind::SliceStore {
                slice: active.value,
                index: coordinate,
                value: Value::Constant(ScalarValue::Bool(false)),
                bounds: BoundsMode::Unchecked,
            },
            location.clone(),
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition: condition.value,
                then_block,
                else_block: MirBlock::default(),
            },
            location,
        );
        Ok(())
    }

    pub(super) fn processor_array_active_slice(
        &mut self,
        root: &str,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredSlice, MirLoweringError> {
        let view = self.processor_view(root)?.ok_or_else(|| {
            self.error(
                "processor array lost its lifecycle identity",
                location.clone(),
            )
        })?;
        if view.environments.len() == 1 {
            return self.processor_environment_active_slice(&view.environments[0], block, location);
        }
        let local =
            self.new_slice_local(None, PrimitiveType::Bool, onda_mir::AccessMode::ReadWrite);
        let mut alternatives = Vec::new();
        for frame in &view.environments {
            let mut branch = MirBlock::default();
            let active =
                self.processor_environment_active_slice(frame, &mut branch, location.clone())?;
            self.assign_value(&mut branch, local, active.value, location.clone());
            alternatives.push(branch);
        }
        let tag = view.tag.ok_or_else(|| {
            self.error("processor lifecycle choice lost its tag", location.clone())
        })?;
        block.statements.extend(
            self.processor_choice_block(tag, alternatives, 0, location)
                .statements,
        );
        Ok(LoweredSlice {
            value: Value::Local(local),
            element: PrimitiveType::Bool,
            access: onda_mir::AccessMode::ReadWrite,
        })
    }

    pub(super) fn processor_environment_active_slice(
        &mut self,
        frame: &LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredSlice, MirLoweringError> {
        let state = frame.shape.state.as_ref().ok_or_else(|| {
            self.error(
                "processor lifecycle lost its state projection",
                location.clone(),
            )
        })?;
        if let Some((owner, edge)) = state.projection.parent_and_declaration() {
            let field = self
                .processor_views
                .declaration(owner, &edge.declaration)
                .and_then(|declaration| declaration.active_field.clone())
                .ok_or_else(|| {
                    self.error(
                        "processor array lost its active-slot field",
                        location.clone(),
                    )
                })?;
            let mut parent = frame.clone();
            parent.shape.state.as_mut().unwrap().projection.edges.pop();
            let root = self.processor_environment_state(&parent, block, location.clone())?;
            return self.lower_named_slice(
                &format!("{root}.{field}"),
                SliceSelection::default(),
                Some(onda_mir::AccessMode::ReadWrite),
                block,
                location,
            );
        }
        let root = frame.state_root.as_ref().ok_or_else(|| {
            self.error(
                "processor lifecycle lost its storage root",
                location.clone(),
            )
        })?;
        self.processor_root_active_slice(root, block, location)
    }

    fn processor_root_active_slice(
        &mut self,
        root: &ProcessorCapture<String>,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<LoweredSlice, MirLoweringError> {
        match root {
            ProcessorCapture::Direct(root) => {
                let owned = owned_active_name(root);
                if self.bindings.contains_key(&owned) {
                    return self.lower_named_slice(
                        &owned,
                        SliceSelection::default(),
                        Some(onda_mir::AccessMode::ReadWrite),
                        block,
                        location,
                    );
                }
                if let Some(Binding::ProcArrayParameter {
                    active: Some(local),
                    ..
                }) = self.bindings.get(root)
                {
                    return Ok(LoweredSlice {
                        value: Value::Local(*local),
                        element: PrimitiveType::Bool,
                        access: onda_mir::AccessMode::ReadWrite,
                    });
                }
                self.lower_named_slice(
                    &runtime_proc_array_active_symbol(root),
                    SliceSelection::default(),
                    Some(onda_mir::AccessMode::ReadWrite),
                    block,
                    location,
                )
            }
            ProcessorCapture::Choice { tag, alternatives } => {
                let local = self.new_slice_local(
                    None,
                    PrimitiveType::Bool,
                    onda_mir::AccessMode::ReadWrite,
                );
                let mut branches = Vec::with_capacity(alternatives.len());
                for root in alternatives {
                    let mut branch = MirBlock::default();
                    let active = self.processor_root_active_slice(
                        &ProcessorCapture::Direct(root.clone()),
                        &mut branch,
                        location.clone(),
                    )?;
                    self.assign_value(&mut branch, local, active.value, location.clone());
                    branches.push(branch);
                }
                block.statements.extend(
                    self.processor_choice_block(*tag, branches, 0, location)
                        .statements,
                );
                Ok(LoweredSlice {
                    value: Value::Local(local),
                    element: PrimitiveType::Bool,
                    access: onda_mir::AccessMode::ReadWrite,
                })
            }
        }
    }

    pub(super) fn begin_processor_view_slot(
        &mut self,
        operation: &crate::processor_views::GeneratedProcessorOperation,
        receiver: &onda_frontend::CallArg,
        frame: &LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if !matches!(
            operation.kind,
            ProcessorOperation::Step | ProcessorOperation::CallOutput(_)
        ) {
            return Ok(());
        }
        let proc_name = &operation.projection.owner;
        if !self.proc_array_hook_types.contains(proc_name) {
            return Ok(());
        }
        let Some(state) = &frame.shape.state else {
            return Ok(());
        };
        if state.is_scalar_root() {
            return Ok(());
        }
        let active = self.processor_environment_active_slice(frame, block, location.clone())?;
        let coordinate = processor_lifecycle_coordinate(frame).ok_or_else(|| {
            self.error(
                "processor lifecycle lost its selected coordinate",
                location.clone(),
            )
        })?;
        let value = self.emit_temp(
            block,
            PrimitiveType::Bool,
            Rvalue::SliceLoad {
                slice: active.value,
                index: coordinate,
                bounds: BoundsMode::Unchecked,
            },
            location.clone(),
        );
        let condition = self.emit_temp(
            block,
            PrimitiveType::Bool,
            Rvalue::Unary {
                op: UnaryOp::LogicalNot,
                operand: value.value,
            },
            location.clone(),
        );
        let mut then_block = MirBlock::default();
        let mut pre = operation.clone();
        pre.kind = ProcessorOperation::BlockPre;
        let name = self
            .processor_views
            .operation_target(&pre.projection, &pre.kind)
            .ok_or_else(|| {
                self.error(
                    "processor lifecycle lost its block-pre operation",
                    location.clone(),
                )
            })?
            .to_owned();
        let arguments = vec![receiver.clone()];
        let (target, arguments) = self.processor_operation_arguments(
            &name,
            &arguments,
            &pre,
            frame,
            &mut then_block,
            location.clone(),
        )?;
        self.lower_direct_user_call_into(
            &target,
            &[],
            &arguments,
            None,
            location.clone(),
            &mut then_block,
        )?;
        self.push_statement(
            &mut then_block,
            StatementKind::SliceStore {
                slice: active.value,
                index: coordinate,
                value: Value::Constant(ScalarValue::Bool(true)),
                bounds: BoundsMode::Unchecked,
            },
            location.clone(),
        );
        self.push_statement(
            block,
            StatementKind::If {
                condition: condition.value,
                then_block,
                else_block: MirBlock::default(),
            },
            location,
        );
        Ok(())
    }
}

fn processor_lifecycle_coordinate(frame: &LoweredProcessorEnvironment) -> Option<Value> {
    let state = frame.shape.state.as_ref()?;
    if state
        .projection
        .edges
        .last()
        .map_or(state.storage.len.is_none(), |edge| edge.extent.is_none())
    {
        Some(Value::Constant(ScalarValue::I32(0)))
    } else {
        frame.coordinates.last().copied()
    }
}
