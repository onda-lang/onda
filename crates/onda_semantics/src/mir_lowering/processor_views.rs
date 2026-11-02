//! Complete processor environments at the semantic/MIR seam.
//!
//! Captures use ordinary MIR references and resources. They confer no alias
//! exclusivity and are subject to the same effect analysis and pruning as
//! authored parameters.

use super::processor_captures::ProcessorCapture;
use super::*;
use crate::processor_views::{
    ProcessorEnvironmentShape, ProcessorProjection, ProcessorStateShape, ProcessorStorageShape,
    ProcessorViewShape, ProcessorViewSource,
};

#[derive(Debug, Clone)]
pub(super) struct LoweredProcessorEnvironment {
    pub shape: ProcessorEnvironmentShape,
    pub owner: Option<ProcessorCapture<String>>,
    pub state_root: Option<ProcessorCapture<String>>,
    pub coordinates: Vec<Value>,
    pub resources: Vec<ProcessorCapture<Expr>>,
    pub outer: Option<Box<LoweredProcessorEnvironment>>,
}

#[derive(Debug, Clone)]
pub(super) struct LoweredProcessorView {
    pub shape: ProcessorViewShape,
    pub tag: Option<Value>,
    pub environments: Vec<LoweredProcessorEnvironment>,
}

pub(super) fn capture_prefix(param: &str, variant: usize) -> String {
    format!(
        "__onda_view_{}_{}",
        crate::internal_names::encode_internal_symbol_component(param),
        variant
    )
}

fn root_storage(environment: &ProcessorEnvironmentShape) -> Option<ProcessorStorageShape> {
    environment
        .owner_storage
        .clone()
        .or_else(|| {
            environment
                .state
                .as_ref()
                .map(|state| state.storage.clone())
        })
        .or_else(|| {
            environment
                .owner
                .as_ref()
                .map(|owner| ProcessorStorageShape {
                    proc_name: owner.owner.clone(),
                    len: None,
                })
        })
}

fn coordinate_count(environment: &ProcessorEnvironmentShape, array: bool) -> usize {
    let axes = environment
        .state
        .as_ref()
        .map_or_else(
            || {
                environment
                    .owner
                    .as_ref()
                    .map_or(0, |owner| owner.coordinate_arity())
            },
            |state| state.projection.coordinate_arity() + usize::from(state.storage.len.is_some()),
        )
        .max(usize::from(environment.resources.iter().any(|resource| {
            resource.mapped || resource.alternatives.is_some()
        })));
    axes.saturating_sub(usize::from(array))
}

fn capture_parameters(
    prefix: &str,
    environment: &ProcessorEnvironmentShape,
    array: bool,
    out: &mut Vec<(String, TypedFnParam)>,
) {
    if let Some(storage) = root_storage(environment) {
        let kind = if storage.len.is_some() {
            TypedFnParam::StructArray {
                struct_name: storage.proc_name,
                len: storage.len,
            }
        } else {
            TypedFnParam::Struct {
                struct_name: storage.proc_name,
            }
        };
        out.push((format!("{prefix}_owner"), kind));
    }
    for axis in 0..coordinate_count(environment, array) {
        out.push((
            format!("{prefix}_coordinate_{axis}"),
            TypedFnParam::Scalar {
                ty: Some(PrimitiveType::I32),
            },
        ));
    }
    for (index, resource) in environment.resources.iter().enumerate() {
        if resource.binding.is_some() {
            continue;
        }
        let kind = match resource.mapped_len.or(resource.len) {
            Some(len) => TypedFnParam::BufferArray {
                elem_ty: resource.elem_ty,
                channels: resource.channels.clone(),
                len,
            },
            None => TypedFnParam::Buffer {
                elem_ty: resource.elem_ty,
                channels: resource.channels.clone(),
            },
        };
        if let Some(count) = resource.alternatives {
            for choice in 0..count {
                out.push((
                    format!("{prefix}_resource_{index}_choice_{choice}"),
                    kind.clone(),
                ));
            }
        } else {
            out.push((format!("{prefix}_resource_{index}"), kind));
        }
    }
    if let Some(outer) = &environment.outer {
        capture_parameters(&format!("{prefix}_outer"), outer, false, out);
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn processor_capture_parameters(&self) -> Vec<(String, TypedFnParam)> {
        let mut result = Vec::new();
        for param in &self.function.params {
            let Some(shape) = self
                .processor_views
                .function_param(&self.function.name, param)
            else {
                continue;
            };
            if shape.environments.len() > 1 {
                result.push((
                    format!("{}_tag", capture_prefix(param, 0)),
                    TypedFnParam::Scalar {
                        ty: Some(PrimitiveType::I32),
                    },
                ));
            }
            for (variant, environment) in shape.environments.iter().enumerate() {
                capture_parameters(
                    &capture_prefix(param, variant),
                    environment,
                    shape.len.is_some(),
                    &mut result,
                );
            }
        }
        result
    }

    fn bind_environment_parameters(
        &self,
        prefix: &str,
        shape: &ProcessorEnvironmentShape,
        array: bool,
    ) -> Result<LoweredProcessorEnvironment, MirLoweringError> {
        let mut coordinates = Vec::new();
        for axis in 0..coordinate_count(shape, array) {
            let name = format!("{prefix}_coordinate_{axis}");
            let Some(Binding::Local(local, PrimitiveType::I32)) = self.bindings.get(&name) else {
                return Err(self.error(
                    format!("processor environment coordinate '{name}' was not legalized"),
                    function_location(self.function),
                ));
            };
            coordinates.push(Value::Local(*local));
        }
        Ok(LoweredProcessorEnvironment {
            shape: shape.clone(),
            state_root: shape
                .state
                .as_ref()
                .map(|_| ProcessorCapture::Direct(format!("{prefix}_owner"))),
            owner: shape
                .owner
                .as_ref()
                .map(|_| ProcessorCapture::Direct(format!("{prefix}_owner"))),
            coordinates,
            resources: shape
                .resources
                .iter()
                .enumerate()
                .map(|(index, resource)| {
                    if let Some(binding) = &resource.binding {
                        self.processor_views
                            .declaration(&binding.owner_struct, &binding.field_name)
                            .expect("registered processor resource declaration")
                            .resources[binding.resource_index]
                            .clone()
                    } else if let Some(count) = resource.alternatives {
                        Expr::ArrayLiteral {
                            loc: function_location(self.function).into(),
                            values: (0..count)
                                .map(|choice| {
                                    Expr::var(format!("{prefix}_resource_{index}_choice_{choice}"))
                                })
                                .collect(),
                        }
                    } else {
                        Expr::var(format!("{prefix}_resource_{index}"))
                    }
                })
                .map(ProcessorCapture::Direct)
                .collect(),
            outer: shape
                .outer
                .as_ref()
                .map(|outer| {
                    self.bind_environment_parameters(&format!("{prefix}_outer"), outer, false)
                        .map(Box::new)
                })
                .transpose()?,
        })
    }

    pub(super) fn bind_processor_parameters(&mut self) -> Result<(), MirLoweringError> {
        for param in &self.function.params {
            let Some(shape) = self
                .processor_views
                .function_param(&self.function.name, param)
                .cloned()
            else {
                continue;
            };
            let tag = if shape.environments.len() > 1 {
                let name = format!("{}_tag", capture_prefix(param, 0));
                let Some(Binding::Local(local, PrimitiveType::I32)) = self.bindings.get(&name)
                else {
                    return Err(self.error(
                        "processor environment tag was not legalized",
                        function_location(self.function),
                    ));
                };
                Some(Value::Local(*local))
            } else {
                None
            };
            let environments = shape
                .environments
                .iter()
                .enumerate()
                .map(|(variant, environment)| {
                    self.bind_environment_parameters(
                        &capture_prefix(param, variant),
                        environment,
                        shape.len.is_some(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let view = LoweredProcessorView {
                shape,
                tag,
                environments,
            };
            match self.bindings.get_mut(param) {
                Some(
                    Binding::StructParameter { processor, .. }
                    | Binding::StructView { processor, .. }
                    | Binding::ProcArrayParameter { processor, .. }
                    | Binding::StructArrayParameter { processor, .. },
                ) => *processor = Some(view),
                _ => {
                    return Err(self.error(
                        format!("processor parameter '{param}' lost its nominal binding"),
                        function_location(self.function),
                    ))
                }
            }
        }
        Ok(())
    }

    fn captured_processor_source(
        &self,
        source: &ProcessorViewSource,
    ) -> Result<LoweredProcessorView, MirLoweringError> {
        fn capture(
            source: &crate::processor_views::ProcessorEnvironmentCapture,
            shape: &ProcessorEnvironmentShape,
        ) -> Option<LoweredProcessorEnvironment> {
            let owner = match &source.owner {
                Some(Expr::Var { name, .. }) => Some(ProcessorCapture::Direct(name.clone())),
                None => None,
                _ => return None,
            };
            Some(LoweredProcessorEnvironment {
                state_root: match &source.state_root {
                    Some(Expr::Var { name, .. }) => Some(ProcessorCapture::Direct(name.clone())),
                    _ => None,
                },
                shape: shape.clone(),
                owner,
                coordinates: Vec::new(),
                resources: source
                    .resources
                    .iter()
                    .cloned()
                    .map(ProcessorCapture::Direct)
                    .collect(),
                outer: source
                    .outer
                    .as_ref()
                    .zip(shape.outer.as_ref())
                    .and_then(|(capture_value, shape)| capture(capture_value, shape))
                    .map(Box::new),
            })
        }
        let environments = source
            .captures
            .iter()
            .zip(&source.shape.environments)
            .map(|(value, shape)| {
                capture(value, shape).ok_or_else(|| {
                    self.error(
                        "processor owner projection lost its captured root",
                        function_location(self.function),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LoweredProcessorView {
            shape: source.shape.clone(),
            tag: None,
            environments,
        })
    }

    pub(super) fn processor_view(
        &self,
        name: &str,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        if let Some(
            Binding::StructParameter {
                processor: Some(view),
                ..
            }
            | Binding::StructView {
                processor: Some(view),
                ..
            }
            | Binding::ProcArrayParameter {
                processor: Some(view),
                ..
            }
            | Binding::StructArrayParameter {
                processor: Some(view),
                ..
            },
        ) = self.bindings.get(name)
        {
            return Ok(Some(view.clone()));
        }
        let identity = match self.bindings.get(name) {
            Some(
                Binding::StructParameter { struct_name, .. }
                | Binding::StructView { struct_name, .. },
            ) => Some((struct_name.clone(), None)),
            Some(Binding::ProcArrayParameter {
                proc_name,
                fixed_len,
                ..
            }) => Some((proc_name.clone(), Some(*fixed_len as usize))),
            Some(Binding::StructArrayParameter {
                struct_name,
                length: StructArrayLength::Fixed(len),
                ..
            }) => Some((struct_name.clone(), Some(*len as usize))),
            _ => None,
        };
        let (declared_owner, declared_field) = name.rsplit_once('.').unwrap_or(("self", name));
        let declared_member = self.owner_struct_name(declared_owner).is_some_and(|owner| {
            self.processor_views
                .declaration(owner, declared_field)
                .is_some()
        });
        if let Some((proc_name, len)) = identity.filter(|(proc_name, _)| {
            !declared_member && self.processor_views.owner_resources.contains_key(proc_name)
        }) {
            let state = ProcessorStateShape {
                storage: ProcessorStorageShape {
                    proc_name: proc_name.clone(),
                    len,
                },
                projection: ProcessorProjection {
                    owner: proc_name.clone(),
                    edges: Vec::new(),
                },
            };
            let (resource_shapes, resources) = if name == "self" {
                self.lexical_processor_resources(&proc_name)
                    .unwrap_or_default()
            } else {
                (Vec::new(), Vec::new())
            };
            let environment = ProcessorEnvironmentShape {
                state: Some(state),
                owner_storage: None,
                owner: None,
                resources: resource_shapes,
                outer: None,
            };
            return Ok(Some(LoweredProcessorView {
                shape: ProcessorViewShape {
                    proc_name,
                    len,
                    environments: vec![environment.clone()],
                },
                tag: None,
                environments: vec![LoweredProcessorEnvironment {
                    shape: environment,
                    owner: None,
                    state_root: Some(ProcessorCapture::Direct(name.to_owned())),
                    coordinates: Vec::new(),
                    resources: resources
                        .into_iter()
                        .map(ProcessorCapture::Direct)
                        .collect(),
                    outer: None,
                }],
            }));
        }
        if let Some(source) = self.processor_views.roots.get(name) {
            return self.captured_processor_source(source).map(Some);
        }
        let (owner, field) = name.rsplit_once('.').unwrap_or(("self", name));
        if owner == name {
            return Ok(None);
        }
        let Some(owner_type) = self.owner_struct_name(owner).map(str::to_owned) else {
            return Ok(None);
        };
        let outer = self.processor_view(owner)?;
        let Some(declaration) = self.processor_views.declaration(&owner_type, field) else {
            return Ok(None);
        };
        let environments = if let Some(parent) = &outer {
            parent
                .environments
                .iter()
                .map(|parent| {
                    let shape = self
                        .processor_views
                        .member_environment(declaration, Some(&parent.shape));
                    LoweredProcessorEnvironment {
                        owner: shape.owner.as_ref().and(parent.state_root.clone()),
                        state_root: parent.state_root.clone(),
                        coordinates: parent.coordinates.clone(),
                        resources: declaration
                            .resources
                            .iter()
                            .cloned()
                            .map(ProcessorCapture::Direct)
                            .collect(),
                        outer: shape.outer.as_ref().map(|_| Box::new(parent.clone())),
                        shape,
                    }
                })
                .collect::<Vec<_>>()
        } else {
            let source = self
                .processor_views
                .member_source(&owner_type, field, &Expr::var(owner), None)
                .expect("registered processor member");
            self.captured_processor_source(&source)?.environments
        };
        Ok(Some(LoweredProcessorView {
            shape: ProcessorViewShape {
                proc_name: declaration.proc_name.clone(),
                len: declaration.len,
                environments: environments
                    .iter()
                    .map(|environment| environment.shape.clone())
                    .collect(),
            },
            tag: outer.as_ref().and_then(|parent| parent.tag),
            environments,
        }))
    }

    pub(super) fn selected_processor_view(
        &self,
        base: &str,
        coordinate: Value,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        let Some(mut view) = self.processor_view(base)? else {
            return Ok(None);
        };
        view.shape = view.shape.selected();
        for environment in &mut view.environments {
            if environment.shape.state.is_some()
                || environment.shape.owner.is_some()
                || environment
                    .shape
                    .resources
                    .iter()
                    .any(|resource| resource.mapped || resource.alternatives.is_some())
            {
                environment.coordinates.push(coordinate);
            }
        }
        Ok(Some(view))
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn processor_argument_view(
        &self,
        expression: &Expr,
        prepared: &PreparedCallArgument,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        if let Some(source) = indexed_read_source(expression) {
            let PreparedCallArgument::IndexedStruct(element) = prepared else {
                return Ok(None);
            };
            return self.selected_processor_view(source.base, Value::Local(element.index));
        }
        match expression {
            Expr::Var { name, .. } => self.processor_view(name),
            _ => Ok(None),
        }
    }

    pub(super) fn append_processor_environment_arguments(
        &mut self,
        expected: &ProcessorEnvironmentShape,
        captured: &LoweredProcessorEnvironment,
        array: bool,
        block: &mut MirBlock,
        out: &mut Vec<CallArgument>,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if let Some(storage) = root_storage(expected) {
            let root = captured
                .state_root
                .as_ref()
                .or(captured.owner.as_ref())
                .ok_or_else(|| {
                    self.error(
                        "processor environment lost its root state",
                        location.clone(),
                    )
                })?;
            let root = self.materialize_processor_storage_capture(
                root,
                &storage,
                block,
                location.clone(),
            )?;
            if storage.len.is_some() {
                self.append_processor_owner_array(
                    &root,
                    &storage.proc_name,
                    block,
                    out,
                    location.clone(),
                )?;
            } else {
                let fields = self.struct_field_shapes(&storage.proc_name, location.clone())?;
                out.extend(self.struct_reference_arguments(
                    &root,
                    &fields,
                    None,
                    block,
                    location.clone(),
                )?);
            }
        }
        let count = coordinate_count(expected, array);
        if captured.coordinates.len() < count {
            return Err(self.error(
                "processor environment lost a captured selection coordinate",
                location,
            ));
        }
        out.extend(
            captured
                .coordinates
                .iter()
                .take(count)
                .copied()
                .map(CallArgument::Value),
        );
        if captured.resources.len() != expected.resources.len() {
            return Err(self.error(
                format!(
                    "processor environment has {} resource captures, expected {}",
                    captured.resources.len(),
                    expected.resources.len()
                ),
                location,
            ));
        }
        for (resource, shape) in captured.resources.iter().zip(&expected.resources) {
            if shape.binding.is_none() {
                let resource = self.materialize_processor_resource_capture(
                    resource,
                    shape,
                    block,
                    location.clone(),
                )?;
                self.append_processor_resource_arguments(&resource, shape, block, out)?;
            }
        }
        if let Some(outer) = &expected.outer {
            let captured = captured.outer.as_ref().ok_or_else(|| {
                self.error(
                    "processor environment lost an outer callback capture",
                    location.clone(),
                )
            })?;
            self.append_processor_environment_arguments(
                outer, captured, false, block, out, location,
            )?;
        }
        Ok(())
    }

    pub(super) fn append_processor_view_arguments(
        &mut self,
        expected: &ProcessorViewShape,
        captured: &LoweredProcessorView,
        block: &mut MirBlock,
        out: &mut Vec<CallArgument>,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        if expected.proc_name != captured.shape.proc_name || expected.len != captured.shape.len {
            return Err(self.error(
                "processor environment changed nominal type or extent",
                location,
            ));
        }
        if expected.environments.len() > 1 {
            let tag = captured.tag.ok_or_else(|| self.error(format!("processor environment choice lost its tag in {}: expected {expected:?}, captured {captured:?}", self.function.name), location.clone()))?;
            out.push(CallArgument::Value(tag));
        }
        for environment in &expected.environments {
            let index = captured.shape.environments.iter().position(|shape| shape == environment).ok_or_else(|| self.error(format!("processor argument lost its specialized environment shape in {}: expected {environment:?}, captured {:?}", self.function.name, captured.shape), location.clone()))?;
            self.append_processor_environment_arguments(
                environment,
                &captured.environments[index],
                expected.len.is_some(),
                block,
                out,
                location.clone(),
            )?;
        }
        Ok(())
    }

    pub(super) fn captured_value_expression(
        &mut self,
        value: Value,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Expr {
        let local = match value {
            Value::Local(local) => local,
            value => {
                let local = self.new_local(None, PrimitiveType::I32);
                self.assign_value(block, local, value, location.clone());
                local
            }
        };
        let name = format!("__onda_view_coordinate_value_{}", local.raw());
        self.bindings
            .insert(name.clone(), Binding::Local(local, PrimitiveType::I32));
        Expr::var(name).with_loc(location)
    }

    pub(super) fn try_lower_processor_operation(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        result_storage: Option<&str>,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<Option<Vec<LoweredValue>>>, MirLoweringError> {
        let Some(operation) = self.processor_views.operation(name).cloned() else {
            return Ok(None);
        };
        if !operation.projection.edges.is_empty() {
            return Ok(None);
        }
        let Some(receiver) = args.first() else {
            return Ok(None);
        };
        let mut normalized_args = args.to_vec();
        let view = if let Some(source) = indexed_read_source(&receiver.expr) {
            let Some(view) = self.processor_view(source.base)? else {
                return Ok(None);
            };
            if !view.environments.iter().any(|environment| {
                environment.shape.owner.is_some() || !environment.resources.is_empty()
            }) {
                return Ok(None);
            }
            let raw = self.lower_expr(source.index, block)?;
            let raw = self.coerce(raw, PrimitiveType::I32, block, source.index.loc())?;
            let extent = view.shape.len.ok_or_else(|| {
                self.error(
                    "processor operation lost its array extent",
                    location.clone(),
                )
            })?;
            let coordinate = self.materialize_index_to_length(
                raw.value,
                Value::Constant(ScalarValue::I32(extent as i32)),
                source.access,
                block,
                receiver.expr.loc(),
            );
            let index =
                self.captured_value_expression(Value::Local(coordinate), block, source.index.loc());
            normalized_args[0].expr = crate::index_access::indexed_read_expr(
                source.base.to_owned(),
                index,
                source.access,
                receiver.expr.loc().into(),
            );
            self.selected_processor_view(source.base, Value::Local(coordinate))?
                .expect("resolved processor array")
        } else if let Expr::Var { name, .. } = &receiver.expr {
            let Some(view) = self.processor_view(name)? else {
                return Ok(None);
            };
            view
        } else {
            return Ok(None);
        };
        let args = normalized_args.as_slice();
        if view.environments.len() != 1 {
            return self.lower_processor_operation_choice(
                name,
                args,
                &operation,
                &view,
                result_storage,
                location,
                block,
            );
        }
        let environment = &view.environments[0];
        if environment.shape.owner.is_none() && environment.resources.is_empty() {
            return Ok(None);
        }
        let (target, arguments) = self.processor_operation_arguments(
            name,
            args,
            &operation,
            environment,
            block,
            location.clone(),
        )?;
        self.lower_direct_user_call_into(&target, &[], &arguments, result_storage, location, block)
            .map(Some)
    }

    fn processor_operation_arguments(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        operation: &crate::processor_views::GeneratedProcessorOperation,
        environment: &LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(String, Vec<onda_frontend::CallArg>), MirLoweringError> {
        let target = environment
            .shape
            .owner
            .as_ref()
            .and_then(|owner| {
                self.processor_views.operation_target_for(
                    &self.function.name,
                    owner,
                    &operation.kind,
                )
            })
            .unwrap_or(name)
            .to_owned();
        let mut rewritten = Vec::new();
        if let Some(owner) = &environment.owner {
            let storage = root_storage(&environment.shape).expect("typed processor owner storage");
            let mut owner_root = self.materialize_processor_storage_capture(
                owner,
                &storage,
                block,
                location.clone(),
            )?;
            let mut coordinates = environment.coordinates.as_slice();
            if environment
                .shape
                .owner_storage
                .as_ref()
                .is_some_and(|storage| storage.len.is_some())
            {
                let coordinate = coordinates.first().copied().ok_or_else(|| {
                    self.error(
                        "processor owner array lost its captured coordinate",
                        location.clone(),
                    )
                })?;
                coordinates = &coordinates[1..];
                let selected = self.fresh_data_name();
                let index = self.captured_value_expression(coordinate, block, location.clone());
                let expression = Expr::Index {
                    loc: location.clone().into(),
                    base: owner_root,
                    index: Box::new(index),
                };
                if !self.lower_struct_array_element_alias(
                    &selected,
                    &expression,
                    block,
                    location.clone(),
                )? {
                    return Err(self.error(
                        "processor owner array could not materialize its selected owner",
                        location,
                    ));
                }
                owner_root = selected;
            }
            rewritten.push(onda_frontend::CallArg {
                name: None,
                expr: Expr::var(owner_root),
            });
            for coordinate in coordinates {
                rewritten.push(onda_frontend::CallArg {
                    name: None,
                    expr: self.captured_value_expression(*coordinate, block, location.clone()),
                });
            }
        } else {
            rewritten.push(args[0].clone());
        }
        let payload_end = args
            .len()
            .checked_sub(operation.resources.len())
            .filter(|end| *end >= 1)
            .ok_or_else(|| {
                self.error(
                    format!("processor operation '{name}' lost its trailing resource captures"),
                    location.clone(),
                )
            })?;
        rewritten.extend_from_slice(&args[1..payload_end]);
        let resource_plan = self
            .processor_views
            .operation(&target)
            .map(|operation| operation.resources.clone())
            .ok_or_else(|| {
                self.error(
                    "processor operation lost its typed resource plan",
                    location.clone(),
                )
            })?;
        for resource in resource_plan {
            let mut frame = environment;
            for _ in 0..resource.frame_depth {
                frame = frame.outer.as_deref().ok_or_else(|| {
                    self.error(
                        format!(
                            "processor operation lost resource frame at depth {}",
                            resource.frame_depth
                        ),
                        location.clone(),
                    )
                })?;
            }
            let expr = self.processor_environment_resource_expression(
                frame,
                resource.resource_index,
                block,
                location.clone(),
            )?;
            rewritten.push(onda_frontend::CallArg { name: None, expr });
        }
        Ok((target, rewritten))
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_processor_operation_choice(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        operation: &crate::processor_views::GeneratedProcessorOperation,
        view: &LoweredProcessorView,
        result_storage: Option<&str>,
        location: SourceLoc,
        block: &mut MirBlock,
    ) -> Result<Option<Option<Vec<LoweredValue>>>, MirLoweringError> {
        if view.environments.is_empty() {
            return Ok(None);
        }
        let tag = view.tag.ok_or_else(|| {
            self.error(
                "processor environment choice lost its captured tag",
                location.clone(),
            )
        })?;
        let original = &self.functions[*self
            .function_indices
            .get(name)
            .expect("processor operation declaration")];
        let result_types = match &original.return_ty {
            ReturnType::Scalar(ty) => vec![*ty],
            ReturnType::Tuple(types) => types.clone(),
            ReturnType::Data(_) => Vec::new(),
        };
        let results = result_types
            .into_iter()
            .map(|ty| (self.new_local(None, ty), ty))
            .collect::<Vec<_>>();
        let outer = self.bindings.clone();
        let mut alternatives = Vec::new();
        for environment in &view.environments {
            self.bindings = outer.clone();
            let mut branch = MirBlock::default();
            let (target, arguments) = self.processor_operation_arguments(
                name,
                args,
                operation,
                environment,
                &mut branch,
                location.clone(),
            )?;
            let values = self
                .lower_direct_user_call_into(
                    &target,
                    &[],
                    &arguments,
                    result_storage,
                    location.clone(),
                    &mut branch,
                )?
                .ok_or_else(|| {
                    self.error(
                        "processor environment alternative has no result contract",
                        location.clone(),
                    )
                })?;
            for ((local, ty), value) in results.iter().zip(values) {
                let value = self.coerce(value, *ty, &mut branch, location.clone())?;
                self.assign_value(&mut branch, *local, value.value, location.clone());
            }
            alternatives.push(branch);
        }
        self.bindings = outer;
        let selected = self.processor_choice_block(tag, alternatives, 0, location);
        block.statements.extend(selected.statements);
        Ok(Some(Some(
            results
                .into_iter()
                .map(|(local, ty)| LoweredValue {
                    value: Value::Local(local),
                    ty,
                })
                .collect(),
        )))
    }

    pub(super) fn processor_choice_block(
        &mut self,
        tag: Value,
        alternatives: Vec<MirBlock>,
        first: usize,
        location: SourceLoc,
    ) -> MirBlock {
        let count = alternatives.len();
        self.processor_choice_tree(tag, &mut alternatives.into_iter(), count, first, location)
    }

    fn processor_choice_tree(
        &mut self,
        tag: Value,
        alternatives: &mut std::vec::IntoIter<MirBlock>,
        count: usize,
        first: usize,
        location: SourceLoc,
    ) -> MirBlock {
        if count == 1 {
            return alternatives.next().expect("one alternative");
        }
        let mid = count / 2;
        let mut block = MirBlock::default();
        let condition = self.compare_value(
            &mut block,
            CompareOp::Less,
            tag,
            Value::Constant(ScalarValue::I32((first + mid) as i32)),
            location.clone(),
        );
        let then_block =
            self.processor_choice_tree(tag, alternatives, mid, first, location.clone());
        let else_block = self.processor_choice_tree(
            tag,
            alternatives,
            count - mid,
            first + mid,
            location.clone(),
        );
        self.push_statement(
            &mut block,
            StatementKind::If {
                condition,
                then_block,
                else_block,
            },
            location,
        );
        block
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn append_processor_owner_array(
        &mut self,
        root: &str,
        proc_name: &str,
        block: &mut MirBlock,
        out: &mut Vec<CallArgument>,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        self.prepare_data_array_view(root, block, location.clone())?;
        let fields = match self.bindings.get(root).cloned() {
            Some(Binding::StructArrayParameter { length, fields, .. }) => {
                out.push(CallArgument::Value(self.struct_array_length_value(
                    length,
                    block,
                    location.clone(),
                )));
                Some(fields)
            }
            Some(Binding::ProcArrayParameter { length, fields, .. }) => {
                let length = self.emit_temp(
                    block,
                    PrimitiveType::I32,
                    Rvalue::Load(Place {
                        base: PlaceBase::Parameter(length),
                        projections: Vec::new(),
                    }),
                    location.clone(),
                );
                out.push(CallArgument::Value(length.value));
                Some(fields)
            }
            _ => None,
        };
        if let Some(fields) = fields {
            out.extend(
                fields
                    .into_iter()
                    .map(|(_, local, _)| CallArgument::Value(Value::Local(local))),
            );
            return Ok(());
        }
        let (actual, count) = self
            .runtime_globals_for_unbound(root)
            .and_then(|globals| globals.array_struct_roots.get(root))
            .cloned()
            .ok_or_else(|| {
                self.error(
                    format!("processor owner array '{root}' lost its root storage"),
                    location.clone(),
                )
            })?;
        if actual != proc_name {
            return Err(self.error("processor owner root changed nominal type", location));
        }
        out.push(CallArgument::Value(Value::Constant(ScalarValue::I32(
            count as i32,
        ))));
        for field in self.struct_field_shapes(proc_name, location.clone())? {
            let path = match field {
                StructFieldShape::Scalar { name, .. } | StructFieldShape::Array { name, .. } => {
                    name
                }
            };
            let slice = self.lower_named_slice(
                &format!("{root}.{path}"),
                SliceSelection::default(),
                Some(onda_mir::AccessMode::ReadWrite),
                block,
                location.clone(),
            )?;
            out.push(CallArgument::Value(slice.value));
        }
        Ok(())
    }
}
