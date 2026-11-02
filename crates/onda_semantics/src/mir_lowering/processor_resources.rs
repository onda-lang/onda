//! Typed resource captures, local descriptor joins, and immutable binding maps.
use super::*;
use crate::processor_views::{ProcessorEnvironmentShape, ProcessorResourceShape};
use std::cell::RefCell;

#[derive(Debug, Default)]
struct ResourceMaps {
    maps: Vec<onda_mir::BufferMap>,
    ids: HashMap<Vec<onda_mir::BufferId>, onda_mir::BufferMapId>,
}

#[derive(Debug, Default)]
pub(super) struct ProcessorResourceCatalog(RefCell<ResourceMaps>);

impl ProcessorResourceCatalog {
    pub(super) fn intern(&self, buffers: Vec<onda_mir::BufferId>) -> onda_mir::BufferMapId {
        let mut registry = self.0.borrow_mut();
        if let Some(id) = registry.ids.get(&buffers) {
            return *id;
        }
        let id = onda_mir::BufferMapId::new(registry.maps.len() as u32);
        registry.maps.push(onda_mir::BufferMap {
            buffers: buffers.clone(),
        });
        registry.ids.insert(buffers, id);
        id
    }

    pub(super) fn take_maps(&self) -> Vec<onda_mir::BufferMap> {
        std::mem::take(&mut self.0.borrow_mut().maps)
    }
}

fn resource_shape(
    name: &str,
    prefix: &str,
    environment: &ProcessorEnvironmentShape,
) -> Option<ProcessorResourceShape> {
    for (index, shape) in environment.resources.iter().enumerate() {
        if shape.binding.is_some() {
            continue;
        }
        let resource = format!("{prefix}_resource_{index}");
        if let Some(count) = shape.alternatives {
            if name
                .strip_prefix(&format!("{resource}_choice_"))
                .and_then(|choice| choice.parse::<usize>().ok())
                .is_some_and(|choice| choice < count)
            {
                let mut shape = shape.clone();
                shape.alternatives = None;
                return Some(shape);
            }
        } else if name == resource {
            return Some(shape.clone());
        }
    }
    environment
        .outer
        .as_ref()
        .and_then(|outer| resource_shape(name, &format!("{prefix}_outer"), outer))
}

impl FunctionLowerer<'_> {
    /// Resolve inherited constructor recipes only after selecting the active
    /// environment. Forwarding a view therefore needs only roots and coordinates,
    /// never descriptors initialized in an inactive branch.
    pub(super) fn processor_environment_resource_expression(
        &mut self,
        frame: &processor_views::LoweredProcessorEnvironment,
        index: usize,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Expr, MirLoweringError> {
        let (expression, shape) = frame
            .resources
            .get(index)
            .zip(frame.shape.resources.get(index))
            .ok_or_else(|| {
                self.error(
                    "processor operation lost an ordered resource capture",
                    location.clone(),
                )
            })?;
        let expression = if let Some(binding) = &shape.binding {
            let expression = self
                .processor_views
                .declaration(&binding.owner_struct, &binding.field_name)
                .and_then(|declaration| declaration.resources.get(binding.resource_index))
                .cloned()
                .ok_or_else(|| {
                    self.error(
                        "inherited processor resource lost its declaration recipe",
                        location.clone(),
                    )
                })?;
            let parent = frame.outer.as_deref().ok_or_else(|| {
                self.error(
                    "inherited processor resource lost its owner frame",
                    location.clone(),
                )
            })?;
            let names = self
                .processor_views
                .owner_resources
                .get(&binding.owner_struct)
                .cloned()
                .unwrap_or_default();
            let needs_owner = expression.walk().any(|node| {
                let name = match node {
                    Expr::Var { name, .. }
                    | Expr::Index { base: name, .. }
                    | Expr::Slice { base: name, .. } => name,
                    _ => return false,
                };
                name == "self"
                    || name
                        .strip_prefix("self.")
                        .is_some_and(|field| !names.iter().any(|name| name == field))
            });
            let owner = needs_owner
                .then(|| {
                    self.processor_environment_state(parent, block, location.clone())
                        .map(Expr::var)
                })
                .transpose()?;
            let mut resources = vec![None; parent.resources.len()];
            for (index, name) in names.iter().enumerate() {
                let used = expression.walk().any(|node| {
                    let name_in_recipe = match node {
                        Expr::Var { name, .. }
                        | Expr::Index { base: name, .. }
                        | Expr::Slice { base: name, .. } => name,
                        _ => return false,
                    };
                    name_in_recipe
                        .strip_prefix("self.")
                        .unwrap_or(name_in_recipe)
                        == name
                });
                if used {
                    resources[index] = Some(self.processor_environment_resource_expression(
                        parent,
                        index,
                        block,
                        location.clone(),
                    )?);
                }
            }
            self.processor_views.instantiate_member_resource(
                &binding.owner_struct,
                &expression,
                owner.as_ref(),
                &resources,
            )
        } else {
            self.materialize_processor_resource_capture(expression, shape, block, location.clone())?
        };
        self.processor_resource_expression(
            &expression,
            shape,
            frame.coordinates.last().copied(),
            block,
        )
    }

    pub(super) fn lexical_processor_resources(
        &self,
        owner: &str,
    ) -> Option<(Vec<ProcessorResourceShape>, Vec<Expr>)> {
        let fields = self.processor_views.owner_resources.get(owner)?;
        let resources = self
            .processor_views
            .lexical_resources(&self.function.name)?;
        let captures_by_index = resources
            .iter()
            .filter(|resource| resource.owner_struct == owner)
            .map(|resource| {
                self.function
                    .params
                    .iter()
                    .position(|parameter| *parameter == resource.parameter)
                    .map(|index| {
                        (
                            resource.resource_index,
                            (
                                &self.function.params[index],
                                &self.function.param_kinds[index],
                            ),
                        )
                    })
            })
            .collect::<Option<Vec<_>>>()?;
        let mut captures = vec![None; fields.len()];
        for (index, (name, kind)) in captures_by_index {
            let (elem_ty, channels, len) = match kind {
                TypedFnParam::Buffer { elem_ty, channels } => (*elem_ty, channels.clone(), None),
                TypedFnParam::BufferArray {
                    elem_ty,
                    channels,
                    len,
                } => (*elem_ty, channels.clone(), Some(*len)),
                _ => return None,
            };
            *captures.get_mut(index)? = Some((
                ProcessorResourceShape {
                    elem_ty,
                    channels,
                    len,
                    mapped: false,
                    mapped_len: None,
                    alternatives: None,
                    binding: None,
                },
                Expr::var(name),
            ));
        }
        captures
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(|captures| captures.into_iter().unzip())
    }

    fn resolve_processor_resource_expression(&self, expression: &Expr) -> Expr {
        let mut expression = expression.clone();
        expression.visit_mut(|node| {
            if let Expr::Var { name, .. }
            | Expr::Index { base: name, .. }
            | Expr::Slice { base: name, .. } = node
            {
                if let Some(field) = name.strip_prefix("self.") {
                    if self.direct_buffer_reference(field).is_some()
                        || matches!(
                            self.bindings.get(field),
                            Some(
                                Binding::BufferParameterArray { .. }
                                    | Binding::BufferCollectionValue { .. }
                            )
                        )
                    {
                        *name = field.to_owned();
                    }
                }
            }
            true
        });
        expression
    }

    fn processor_resource_type(
        &mut self,
        shape: &ProcessorResourceShape,
        location: SourceLoc,
    ) -> Result<TypeId, MirLoweringError> {
        let channels = mir_buffer_channels(&shape.channels).ok_or_else(|| {
            self.error(
                "processor resource channel count exceeds MIR limits",
                location.clone(),
            )
        })?;
        let ty = if let Some(len) = shape.mapped_len.or(shape.len) {
            let len = u32::try_from(len)
                .ok()
                .filter(|len| *len > 0 && *len <= i32::MAX as u32)
                .ok_or_else(|| {
                    self.error("processor resource extent exceeds MIR limits", location)
                })?;
            if shape.mapped {
                MirType::BufferMap {
                    element: scalar_type(shape.elem_ty),
                    channels,
                    access: onda_mir::AccessMode::ReadWrite,
                    len,
                }
            } else {
                MirType::BufferSpan {
                    element: scalar_type(shape.elem_ty),
                    channels,
                    access: onda_mir::AccessMode::ReadWrite,
                    len,
                }
            }
        } else {
            MirType::Buffer {
                element: scalar_type(shape.elem_ty),
                channels,
                access: onda_mir::AccessMode::ReadWrite,
            }
        };
        if let Some(index) = self.types.iter().position(|candidate| *candidate == ty) {
            return Ok(TypeId::new(index as u32));
        }
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(ty);
        Ok(id)
    }

    pub(super) fn processor_resource_parameter_type(
        &mut self,
        name: &str,
    ) -> Result<Option<TypeId>, MirLoweringError> {
        let shape = self.function.params.iter().find_map(|param| {
            let view = self
                .processor_views
                .function_param(&self.function.name, param)?;
            view.environments
                .iter()
                .enumerate()
                .find_map(|(variant, environment)| {
                    resource_shape(
                        name,
                        &processor_views::capture_prefix(param, variant),
                        environment,
                    )
                })
        });
        shape
            .map(|shape| self.processor_resource_type(&shape, function_location(self.function)))
            .transpose()
    }

    pub(super) fn local_buffer_collection_reference(
        &mut self,
        base: &str,
        index: &Expr,
        bounds: BoundsMode,
        block: &mut MirBlock,
    ) -> Result<Option<(MaterializedBufferReference, PrimitiveType)>, MirLoweringError> {
        let Some(Binding::BufferCollectionValue {
            local,
            element,
            start,
            len,
        }) = self.bindings.get(base).cloned()
        else {
            return Ok(None);
        };
        let value = self.lower_expr(index, block)?;
        let value = self.coerce(value, PrimitiveType::I32, block, index.loc())?;
        let selector = self.materialize_index_to_length(
            value.value,
            Value::Constant(ScalarValue::I32(len as i32)),
            if bounds == BoundsMode::Unchecked {
                IndexAccess::Unchecked
            } else {
                IndexAccess::Clamp
            },
            block,
            index.loc(),
        );
        let selector = Value::Local(selector);
        let selector = if start == Value::Constant(ScalarValue::I32(0)) {
            selector
        } else {
            self.emit_temp(
                block,
                PrimitiveType::I32,
                Rvalue::Binary {
                    op: onda_mir::BinaryOp::Add,
                    lhs: start,
                    rhs: selector,
                },
                index.loc(),
            )
            .value
        };
        Ok(Some((
            MaterializedBufferReference::Parameter(onda_mir::BufferParamRef::LocalElement {
                collection: local,
                selector,
                bounds,
            }),
            element,
        )))
    }

    fn append_interface_resource_ids(
        &self,
        expression: &Expr,
        out: &mut Vec<onda_mir::BufferId>,
    ) -> Result<(), MirLoweringError> {
        match expression {
            Expr::ArrayLiteral { values, .. } => {
                for value in values {
                    self.append_interface_resource_ids(value, out)?;
                }
                return Ok(());
            }
            Expr::Var { name, .. } => {
                if let Some((
                    BufferBindingReference::Interface(onda_mir::BufferRef::Direct(id)),
                    _,
                )) = self.direct_buffer_reference(name)
                {
                    out.push(id);
                    return Ok(());
                }
                if let Some((first, _, len)) = self
                    .runtime_globals
                    .and_then(|globals| globals.buffer_arrays.get(name))
                    .copied()
                {
                    out.extend((0..len).map(|index| onda_mir::BufferId::new(first.raw() + index)));
                    return Ok(());
                }
            }
            Expr::Index { base, index, .. } => {
                if let Some((first, _, len)) = self
                    .runtime_globals
                    .and_then(|globals| globals.buffer_arrays.get(base))
                    .copied()
                {
                    if let Some(index) = crate::try_constant_index_i64(index) {
                        out.push(onda_mir::BufferId::new(
                            first.raw() + index.clamp(0, i64::from(len) - 1) as u32,
                        ));
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
        Err(self.error(
            "heterogeneous buffer parameters require direct resource selection",
            expression.loc(),
        ))
    }

    pub(super) fn append_processor_resource_arguments(
        &mut self,
        expression: &Expr,
        shape: &ProcessorResourceShape,
        block: &mut MirBlock,
        out: &mut Vec<CallArgument>,
    ) -> Result<(), MirLoweringError> {
        if let Some(count) = shape.alternatives {
            let Expr::ArrayLiteral { values, .. } = expression else {
                return Err(self.error(
                    "processor resource choices lost their capture recipe",
                    expression.loc(),
                ));
            };
            if values.len() != count || count == 0 {
                return Err(self.error(
                    "processor resource choices changed their extent",
                    expression.loc(),
                ));
            }
            let mut element = shape.clone();
            element.alternatives = None;
            for value in values {
                out.push(self.lower_processor_resource_argument(value, &element, block)?);
            }
        } else {
            out.push(self.lower_processor_resource_argument(expression, shape, block)?);
        }
        Ok(())
    }

    pub(super) fn lower_processor_resource_argument(
        &mut self,
        expression: &Expr,
        shape: &ProcessorResourceShape,
        block: &mut MirBlock,
    ) -> Result<CallArgument, MirLoweringError> {
        let resolved = self.resolve_processor_resource_expression(expression);
        let expression = &resolved;
        let physical_len = shape.mapped_len.or(shape.len);
        if let Expr::Var { name, .. } = expression {
            if let Some(Binding::BufferCollectionValue {
                local, start, len, ..
            }) = self.bindings.get(name).cloned()
            {
                return Ok(CallArgument::BufferSpan(onda_mir::BufferSpanRef::Local {
                    collection: local,
                    start,
                    len,
                    bounds: BoundsMode::Checked,
                }));
            }
        }
        if let Some(len) = physical_len {
            if shape.mapped && matches!(expression, Expr::ArrayLiteral { .. }) {
                let mut buffers = Vec::new();
                self.append_interface_resource_ids(expression, &mut buffers)?;
                if buffers.len() != len {
                    return Err(self.error(
                        "processor binding map has the wrong extent",
                        expression.loc(),
                    ));
                }
                let catalog = self
                    .runtime_globals
                    .and_then(|globals| globals.processor_resources)
                    .ok_or_else(|| {
                        self.error(
                            "processor binding map has no lowering catalog",
                            expression.loc(),
                        )
                    })?;
                let map = catalog.intern(buffers);
                return Ok(CallArgument::BufferSpan(
                    onda_mir::BufferSpanRef::MappedInterface {
                        map,
                        start: 0,
                        len: len as u32,
                    },
                ));
            }
            let mut arguments = Vec::new();
            self.lower_buffer_array_call_arguments(
                "processor environment",
                "resource",
                shape.elem_ty,
                len,
                expression,
                &mut arguments,
            )?;
            return arguments.pop().ok_or_else(|| {
                self.error(
                    "processor collection capture produced no argument",
                    expression.loc(),
                )
            });
        }
        let reference = if let Expr::Var { name, .. } = expression {
            self.direct_buffer_reference(name).map(|(reference, _)| {
                self.materialize_buffer_reference(reference, block, expression.loc())
            })
        } else {
            self.selected_buffer_reference(expression, BoundsMode::Clamp, block)?
                .map(|(reference, _)| reference)
        }
        .ok_or_else(|| {
            self.error(
                "processor resource capture is not a buffer",
                expression.loc(),
            )
        })?;
        Ok(match reference {
            MaterializedBufferReference::Interface(reference) => CallArgument::Buffer(reference),
            MaterializedBufferReference::Parameter(reference) => {
                CallArgument::BufferParam(reference)
            }
        })
    }

    fn bind_processor_resource_value(
        &mut self,
        local: LocalId,
        shape: &ProcessorResourceShape,
        start: Value,
        len: Option<u32>,
    ) -> Expr {
        let name = format!("__onda_resource_value_{}", local.raw());
        let binding = if let Some(len) = len {
            Binding::BufferCollectionValue {
                local,
                element: shape.elem_ty,
                start,
                len,
            }
        } else {
            Binding::BufferValue(local, shape.elem_ty)
        };
        self.bindings.insert(name.clone(), binding);
        Expr::var(name)
    }

    pub(super) fn processor_resource_expression(
        &mut self,
        expression: &Expr,
        shape: &ProcessorResourceShape,
        coordinate: Option<Value>,
        block: &mut MirBlock,
    ) -> Result<Expr, MirLoweringError> {
        let resolved = self.resolve_processor_resource_expression(expression);
        let expression = &resolved;
        if let Some(count) = shape.alternatives {
            let Expr::ArrayLiteral { values, .. } = expression else {
                return Err(self.error(
                    "selected processor resource lost its finite choices",
                    expression.loc(),
                ));
            };
            if values.len() != count || count == 0 {
                return Err(self.error(
                    "selected processor resource changed its choice count",
                    expression.loc(),
                ));
            }
            let coordinate = coordinate.ok_or_else(|| {
                self.error(
                    "selected processor resource lost its coordinate",
                    expression.loc(),
                )
            })?;
            let mut element = shape.clone();
            element.alternatives = None;
            let ty = self.processor_resource_type(&element, expression.loc())?;
            let local = LocalId::new(self.locals.len() as u32);
            self.locals.push(onda_mir::Local {
                name: None,
                ty,
                integer_range: None,
            });
            let mut choices = Vec::with_capacity(count);
            for value in values {
                let mut branch = MirBlock::default();
                let source =
                    self.lower_processor_resource_argument(value, &element, &mut branch)?;
                self.push_statement(
                    &mut branch,
                    StatementKind::Assign {
                        destination: Place::local(local),
                        value: Rvalue::MakeBuffer { source, ty },
                    },
                    value.loc(),
                );
                choices.push(branch);
            }
            let selected = self.processor_choice_block(coordinate, choices, 0, expression.loc());
            block.statements.extend(selected.statements);
            return Ok(self.bind_processor_resource_value(
                local,
                &element,
                Value::Constant(ScalarValue::I32(0)),
                element.len.map(|len| len as u32),
            ));
        }
        if !shape.mapped {
            if matches!(expression, Expr::Var { .. }) {
                return Ok(expression.clone());
            }
            let source = self.lower_processor_resource_argument(expression, shape, block)?;
            let ty = self.processor_resource_type(shape, expression.loc())?;
            let local = LocalId::new(self.locals.len() as u32);
            self.locals.push(onda_mir::Local {
                name: None,
                ty,
                integer_range: None,
            });
            self.push_statement(
                block,
                StatementKind::Assign {
                    destination: Place::local(local),
                    value: Rvalue::MakeBuffer { source, ty },
                },
                expression.loc(),
            );
            return Ok(self.bind_processor_resource_value(
                local,
                shape,
                Value::Constant(ScalarValue::I32(0)),
                shape.len.map(|len| len as u32),
            ));
        }
        let argument = self.lower_processor_resource_argument(expression, shape, block)?;
        let ty = self.processor_resource_type(shape, expression.loc())?;
        let local = LocalId::new(self.locals.len() as u32);
        self.locals.push(onda_mir::Local {
            name: None,
            ty,
            integer_range: None,
        });
        self.push_statement(
            block,
            StatementKind::Assign {
                destination: Place::local(local),
                value: Rvalue::MakeBuffer {
                    source: argument,
                    ty,
                },
            },
            expression.loc(),
        );
        let coordinate = coordinate.ok_or_else(|| {
            self.error(
                "selected processor resource lost its coordinate",
                expression.loc(),
            )
        })?;
        if let Some(len) = shape.len {
            let start = self
                .emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Binary {
                        op: onda_mir::BinaryOp::Multiply,
                        lhs: coordinate,
                        rhs: Value::Constant(ScalarValue::I32(len as i32)),
                    },
                    expression.loc(),
                )
                .value;
            return Ok(self.bind_processor_resource_value(local, shape, start, Some(len as u32)));
        }
        let collection = self.bind_processor_resource_value(
            local,
            shape,
            Value::Constant(ScalarValue::I32(0)),
            shape.mapped_len.map(|len| len as u32),
        );
        let Expr::Var { name: base, .. } = collection else {
            unreachable!()
        };
        let index_name = format!("__onda_resource_selection_{}", local.raw());
        let index_local = self.new_local(None, PrimitiveType::I32);
        self.assign_value(block, index_local, coordinate, expression.loc());
        self.bindings.insert(
            index_name.clone(),
            Binding::Local(index_local, PrimitiveType::I32),
        );
        Ok(Expr::Index {
            loc: expression.loc().into(),
            base,
            index: Box::new(Expr::var(index_name)),
        })
    }

    pub(super) fn join_processor_resource_expressions(
        &mut self,
        then_expressions: &[Expr],
        else_expressions: &[Expr],
        shapes: &[ProcessorResourceShape],
        then_block: &mut MirBlock,
        else_block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Vec<Expr>, MirLoweringError> {
        if then_expressions.len() != shapes.len() || else_expressions.len() != shapes.len() {
            return Err(self.error(
                "processor resource join changed its capture schema",
                location,
            ));
        }
        let mut result = Vec::with_capacity(shapes.len());
        for ((a, b), shape) in then_expressions.iter().zip(else_expressions).zip(shapes) {
            if a == b {
                result.push(a.clone());
                continue;
            }
            if let Some(count) = shape.alternatives {
                let (Expr::ArrayLiteral { values: a, .. }, Expr::ArrayLiteral { values: b, .. }) =
                    (a, b)
                else {
                    return Err(
                        self.error("processor resource join lost its finite choices", location)
                    );
                };
                if a.len() != count || b.len() != count {
                    return Err(
                        self.error("processor resource join changed its choice count", location)
                    );
                }
                let mut element = shape.clone();
                element.alternatives = None;
                let mut values = Vec::with_capacity(count);
                for (a, b) in a.iter().zip(b) {
                    values.extend(self.join_processor_resource_expressions(
                        std::slice::from_ref(a),
                        std::slice::from_ref(b),
                        std::slice::from_ref(&element),
                        then_block,
                        else_block,
                        location.clone(),
                    )?);
                }
                result.push(Expr::ArrayLiteral {
                    loc: location.clone().into(),
                    values,
                });
                continue;
            }
            let a = self.lower_processor_resource_argument(a, shape, then_block)?;
            let b = self.lower_processor_resource_argument(b, shape, else_block)?;
            let ty = self.processor_resource_type(shape, location.clone())?;
            let local = LocalId::new(self.locals.len() as u32);
            self.locals.push(onda_mir::Local {
                name: None,
                ty,
                integer_range: None,
            });
            for (block, source) in [(&mut *then_block, a), (&mut *else_block, b)] {
                self.push_statement(
                    block,
                    StatementKind::Assign {
                        destination: Place::local(local),
                        value: Rvalue::MakeBuffer { source, ty },
                    },
                    location.clone(),
                );
            }
            result.push(self.bind_processor_resource_value(
                local,
                shape,
                Value::Constant(ScalarValue::I32(0)),
                shape.mapped_len.or(shape.len).map(|len| len as u32),
            ));
        }
        Ok(result)
    }
}
