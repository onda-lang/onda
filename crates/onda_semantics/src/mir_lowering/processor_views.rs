//! Complete processor environments at the semantic/MIR seam.
//!
//! Captures use ordinary MIR references and resources. They confer no alias
//! exclusivity and are subject to the same effect analysis and pruning as
//! authored parameters.

use super::processor_captures::ProcessorCapture;
use super::*;
use crate::processor_views::{
    ProcessorEnvironmentShape, ProcessorProjection, ProcessorResourceShape, ProcessorStateShape,
    ProcessorStorageShape, ProcessorViewShape, ProcessorViewSource,
};

pub(super) struct ProcessorCaptureParameter {
    pub name: String,
    pub kind: TypedFnParam,
    /// Preserve the collection representation while flattening the capture ABI.
    pub resource: Option<ProcessorResourceShape>,
}

impl ProcessorCaptureParameter {
    fn new(name: String, kind: TypedFnParam) -> Self {
        Self {
            name,
            kind,
            resource: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct LoweredProcessorEnvironment {
    pub shape: ProcessorEnvironmentShape,
    pub owner: Option<ProcessorCapture<String>>,
    pub state_root: Option<ProcessorCapture<String>>,
    pub coordinates: Vec<Value>,
    /// Resource rows can retain their source offset after storage is copied.
    pub resource_offset: Value,
    /// Declaration recipes have no eager expression capture; their enclosing
    /// frame supplies the capabilities needed when the operation selects them.
    pub resources: Vec<Option<ProcessorCapture<Expr>>>,
    pub resource_sources: Vec<LoweredProcessorView>,
    pub outer: Option<Box<LoweredProcessorEnvironment>>,
}

#[derive(Debug, Clone)]
pub(super) struct LoweredProcessorView {
    pub shape: ProcessorViewShape,
    /// Logical row offset into the captured roots, independent of leaf slices.
    pub start: Value,
    pub tag: Option<Value>,
    pub environments: Vec<LoweredProcessorEnvironment>,
}

impl LoweredProcessorEnvironment {
    fn visit_environments(&self, visit: &mut impl FnMut(&Self)) {
        visit(self);
        if let Some(outer) = &self.outer {
            outer.visit_environments(visit);
        }
        for source in &self.resource_sources {
            source.visit_environments(visit);
        }
    }

    fn visit_environments_mut(&mut self, visit: &mut impl FnMut(&mut Self)) {
        visit(self);
        if let Some(outer) = &mut self.outer {
            outer.visit_environments_mut(visit);
        }
        for source in &mut self.resource_sources {
            source.visit_environments_mut(visit);
        }
    }
}

impl LoweredProcessorView {
    pub(super) fn visit_environments(&self, visit: &mut impl FnMut(&LoweredProcessorEnvironment)) {
        for environment in &self.environments {
            environment.visit_environments(visit);
        }
    }

    pub(super) fn visit_environments_mut(
        &mut self,
        visit: &mut impl FnMut(&mut LoweredProcessorEnvironment),
    ) {
        for environment in &mut self.environments {
            environment.visit_environments_mut(visit);
        }
    }

    /// Coordinates and choice tags must be retained and remapped together,
    /// including views used by inherited resource recipes.
    pub(super) fn visit_values(&mut self, visit: &mut impl FnMut(&mut Value)) {
        visit(&mut self.start);
        if let Some(tag) = &mut self.tag {
            visit(tag);
        }
        self.visit_environments_mut(&mut |environment| {
            for capture in [&mut environment.owner, &mut environment.state_root]
                .into_iter()
                .flatten()
            {
                capture.visit_values(visit);
            }
            for resource in environment.resources.iter_mut().flatten() {
                resource.visit_values(visit);
            }
            environment.coordinates.iter_mut().for_each(&mut *visit);
            visit(&mut environment.resource_offset);
            for source in &mut environment.resource_sources {
                visit(&mut source.start);
                if let Some(tag) = &mut source.tag {
                    visit(tag);
                }
            }
        });
    }
}

impl Binding {
    pub(super) fn processor_view(&self) -> Option<&LoweredProcessorView> {
        match self {
            Self::StructView { processor, .. }
            | Self::StructParameter { processor, .. }
            | Self::StructArrayParameter { processor, .. }
            | Self::ProcArrayParameter { processor, .. } => processor.as_ref(),
            _ => None,
        }
    }

    pub(super) fn processor_view_mut(&mut self) -> Option<&mut LoweredProcessorView> {
        match self {
            Self::StructView { processor, .. }
            | Self::StructParameter { processor, .. }
            | Self::StructArrayParameter { processor, .. }
            | Self::ProcArrayParameter { processor, .. } => processor.as_mut(),
            _ => None,
        }
    }
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

fn has_scalar_lifecycle_capture(
    environment: &ProcessorEnvironmentShape,
    hook_types: &HashSet<String>,
) -> bool {
    environment.state.as_ref().is_some_and(|state| {
        state.local_lifecycle
            && state.storage.len.is_none()
            && state.projection.edges.is_empty()
            && hook_types.contains(&state.storage.proc_name)
    })
}

fn capture_parameters(
    prefix: &str,
    environment: &ProcessorEnvironmentShape,
    array: bool,
    hook_types: &HashSet<String>,
    out: &mut Vec<ProcessorCaptureParameter>,
) {
    if let Some(storage) = root_storage(environment) {
        let kind = if let Some(len) = storage.len {
            TypedFnParam::ProcArray {
                proc_name: storage.proc_name,
                len,
            }
        } else {
            TypedFnParam::Struct {
                struct_name: storage.proc_name,
            }
        };
        out.push(ProcessorCaptureParameter::new(
            format!("{prefix}_owner"),
            kind,
        ));
    }
    if has_scalar_lifecycle_capture(environment, hook_types) {
        out.push(ProcessorCaptureParameter::new(
            super::processor_lifecycle::owned_active_name(&format!("{prefix}_owner")),
            TypedFnParam::Array {
                elem_ty: PrimitiveType::Bool,
                len: None,
            },
        ));
    }
    for axis in 0..coordinate_count(environment, array) {
        out.push(ProcessorCaptureParameter::new(
            format!("{prefix}_coordinate_{axis}"),
            TypedFnParam::Scalar {
                ty: Some(PrimitiveType::I32),
            },
        ));
    }
    if environment.has_indexed_resources() {
        out.push(ProcessorCaptureParameter::new(
            format!("{prefix}_resource_offset"),
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
            let mut resource = resource.clone();
            resource.alternatives = None;
            for choice in 0..count {
                out.push(ProcessorCaptureParameter {
                    name: format!("{prefix}_resource_{index}_choice_{choice}"),
                    kind: kind.clone(),
                    resource: Some(resource.clone()),
                });
            }
        } else {
            out.push(ProcessorCaptureParameter {
                name: format!("{prefix}_resource_{index}"),
                kind,
                resource: Some(resource.clone()),
            });
        }
    }
    if let Some(outer) = &environment.outer {
        capture_parameters(&format!("{prefix}_outer"), outer, false, hook_types, out);
    }
    for (index, source) in environment.resource_sources.iter().enumerate() {
        let prefix = format!("{prefix}_source_{index}");
        if source.environments.len() > 1 {
            out.push(ProcessorCaptureParameter::new(
                format!("{prefix}_tag"),
                TypedFnParam::Scalar {
                    ty: Some(PrimitiveType::I32),
                },
            ));
        }
        for (variant, environment) in source.environments.iter().enumerate() {
            capture_parameters(
                &format!("{prefix}_{variant}"),
                environment,
                false,
                hook_types,
                out,
            );
        }
    }
}

impl FunctionLowerer<'_> {
    pub(super) fn processor_view_offset(
        &mut self,
        start: Value,
        offset: Value,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Value {
        if start == Value::Constant(ScalarValue::I32(0)) {
            return offset;
        }
        self.emit_temp(
            block,
            PrimitiveType::I32,
            Rvalue::Binary {
                op: MirBinaryOp::Add,
                lhs: start,
                rhs: offset,
            },
            location,
        )
        .value
    }

    pub(super) fn slice_processor_view(
        &mut self,
        base: &str,
        start: Value,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        let Some(mut view) = self.processor_view(base)? else {
            return Ok(None);
        };
        view.shape = view.shape.sliced();
        view.start = self.processor_view_offset(view.start, start, block, location);
        Ok(Some(view))
    }

    pub(super) fn bind_owned_processor_view(
        &mut self,
        name: &str,
        source: &str,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(), MirLoweringError> {
        let Some(mut view) = self.processor_view(source)? else {
            return Ok(());
        };
        let array_source = view.shape.is_array();
        view.shape = match self.data_type_of(&Expr::var(name)) {
            Some(DataType::Array { len, .. }) => view.shape.owned_array(len),
            _ => view.shape.owned(),
        }
        .with_local_lifecycle(self.local_processor_lifetimes);
        let sources = std::mem::take(&mut view.environments);
        view.environments = view
            .shape
            .environments
            .iter()
            .enumerate()
            .map(|(index, shape)| LoweredProcessorEnvironment {
                shape: shape.clone(),
                state_root: Some(ProcessorCapture::Direct(name.to_owned())),
                owner: None,
                coordinates: Vec::new(),
                resource_offset: if array_source && shape.has_indexed_resources() {
                    self.processor_view_offset(
                        sources[index].resource_offset,
                        view.start,
                        block,
                        location.clone(),
                    )
                } else {
                    Value::Constant(ScalarValue::I32(0))
                },
                resources: if array_source {
                    sources[index].resources.clone()
                } else {
                    vec![None; shape.resources.len()]
                },
                resource_sources: if array_source {
                    sources[index].resource_sources.clone()
                } else {
                    Vec::new()
                },
                outer: if array_source || sources[index].shape.forwards_resources() {
                    sources[index].outer.clone()
                } else {
                    shape
                        .outer
                        .as_ref()
                        .map(|_| Box::new(sources[index].clone()))
                },
            })
            .collect();
        view.start = Value::Constant(ScalarValue::I32(0));
        if !self.bindings.contains_key(name) {
            if let Some(data) = self.data_type_of(&Expr::var(name)) {
                self.bind_data_root(name, &data, location.clone())?;
            }
        }
        self.prepare_data_array_view(name, block, location.clone())?;
        self.initialize_owned_processor_lifecycle(name, &view.shape, block, location.clone())?;
        if let Some(
            Binding::StructView { processor, .. }
            | Binding::StructArrayParameter { processor, .. }
            | Binding::ProcArrayParameter { processor, .. },
        ) = self.bindings.get_mut(name)
        {
            *processor = Some(view);
        }
        self.register_owned_processor_lifetime(name, block, location)?;
        Ok(())
    }

    pub(super) fn processor_capture_parameters(&self) -> Vec<ProcessorCaptureParameter> {
        let mut result = Vec::new();
        for param in &self.function.params {
            let Some(shape) = self
                .processor_views
                .function_param(&self.function.name, param)
            else {
                continue;
            };
            if shape.environments.len() > 1 {
                result.push(ProcessorCaptureParameter::new(
                    format!("{}_tag", capture_prefix(param, 0)),
                    TypedFnParam::Scalar {
                        ty: Some(PrimitiveType::I32),
                    },
                ));
            }
            if shape.is_array() {
                result.push(ProcessorCaptureParameter::new(
                    format!("{}_start", capture_prefix(param, 0)),
                    TypedFnParam::Scalar {
                        ty: Some(PrimitiveType::I32),
                    },
                ));
            }
            for (variant, environment) in shape.environments.iter().enumerate() {
                capture_parameters(
                    &capture_prefix(param, variant),
                    environment,
                    shape.is_array(),
                    self.proc_array_hook_types,
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
            resource_offset: if shape.has_indexed_resources() {
                let name = format!("{prefix}_resource_offset");
                let Some(Binding::Local(local, PrimitiveType::I32)) = self.bindings.get(&name)
                else {
                    return Err(self.error(
                        "processor resource offset was not legalized",
                        function_location(self.function),
                    ));
                };
                Value::Local(*local)
            } else {
                Value::Constant(ScalarValue::I32(0))
            },
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
                    if resource.binding.is_some() {
                        return None;
                    }
                    let expression = if let Some(count) = resource.alternatives {
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
                    };
                    Some(ProcessorCapture::Direct(expression))
                })
                .collect(),
            resource_sources: shape
                .resource_sources
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    let prefix = format!("{prefix}_source_{index}");
                    let tag = if source.environments.len() > 1 {
                        let name = format!("{prefix}_tag");
                        let Some(Binding::Local(local, PrimitiveType::I32)) =
                            self.bindings.get(&name)
                        else {
                            return Err(self.error(
                                "processor resource source lost its tag",
                                function_location(self.function),
                            ));
                        };
                        Some(Value::Local(*local))
                    } else {
                        None
                    };
                    Ok(LoweredProcessorView {
                        start: Value::Constant(ScalarValue::I32(0)),
                        shape: source.clone(),
                        tag,
                        environments: source
                            .environments
                            .iter()
                            .enumerate()
                            .map(|(variant, shape)| {
                                self.bind_environment_parameters(
                                    &format!("{prefix}_{variant}"),
                                    shape,
                                    false,
                                )
                            })
                            .collect::<Result<_, _>>()?,
                    })
                })
                .collect::<Result<_, MirLoweringError>>()?,
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
                        shape.is_array(),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let view = LoweredProcessorView {
                start: if shape.is_array() {
                    let name = format!("{}_start", capture_prefix(param, 0));
                    let Some(Binding::Local(local, PrimitiveType::I32)) = self.bindings.get(&name)
                    else {
                        return Err(self.error(
                            "processor array start was not legalized",
                            function_location(self.function),
                        ));
                    };
                    Value::Local(*local)
                } else {
                    Value::Constant(ScalarValue::I32(0))
                },
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
                resource_offset: Value::Constant(ScalarValue::I32(0)),
                resource_sources: Vec::new(),
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
                    .zip(&shape.resources)
                    .map(|(resource, shape)| {
                        shape
                            .binding
                            .is_none()
                            .then(|| ProcessorCapture::Direct(resource.clone()))
                    })
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
            start: Value::Constant(ScalarValue::I32(0)),
            shape: source.shape.clone(),
            tag: None,
            environments,
        })
    }

    pub(super) fn processor_view(
        &self,
        name: &str,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        if let Some(view) = self.bindings.get(name).and_then(Binding::processor_view) {
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
                local_lifecycle: false,
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
                resource_sources: Vec::new(),
                state: Some(state),
                owner_storage: None,
                owner: None,
                resources: resource_shapes,
                outer: None,
            };
            return Ok(Some(LoweredProcessorView {
                start: Value::Constant(ScalarValue::I32(0)),
                shape: ProcessorViewShape {
                    proc_name,
                    extent: len.map_or(
                        crate::processor_views::ProcessorViewExtent::Scalar,
                        crate::processor_views::ProcessorViewExtent::Array,
                    ),
                    environments: vec![environment.clone()],
                },
                tag: None,
                environments: vec![LoweredProcessorEnvironment {
                    resource_offset: Value::Constant(ScalarValue::I32(0)),
                    resource_sources: Vec::new(),
                    shape: environment,
                    owner: None,
                    state_root: Some(ProcessorCapture::Direct(name.to_owned())),
                    coordinates: Vec::new(),
                    resources: resources
                        .into_iter()
                        .map(|resource| Some(ProcessorCapture::Direct(resource)))
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
                        resource_offset: Value::Constant(ScalarValue::I32(0)),
                        resource_sources: Vec::new(),
                        owner: shape.owner.as_ref().and(parent.state_root.clone()),
                        state_root: parent.state_root.clone(),
                        coordinates: parent.coordinates.clone(),
                        resources: declaration
                            .resources
                            .iter()
                            .zip(&shape.resources)
                            .map(|(resource, shape)| {
                                shape
                                    .binding
                                    .is_none()
                                    .then(|| ProcessorCapture::Direct(resource.clone()))
                            })
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
            start: Value::Constant(ScalarValue::I32(0)),
            shape: ProcessorViewShape {
                proc_name: declaration.proc_name.clone(),
                extent: declaration.len.map_or(
                    crate::processor_views::ProcessorViewExtent::Scalar,
                    crate::processor_views::ProcessorViewExtent::Array,
                ),
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
        &mut self,
        base: &str,
        coordinate: Value,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        let Some(mut view) = self.processor_view(base)? else {
            return Ok(None);
        };
        let coordinate = self.processor_view_offset(view.start, coordinate, block, location);
        view.start = Value::Constant(ScalarValue::I32(0));
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
        &mut self,
        expression: &Expr,
        prepared: &PreparedCallArgument,
        block: &mut MirBlock,
    ) -> Result<Option<LoweredProcessorView>, MirLoweringError> {
        if let Some(source) = indexed_read_source(expression) {
            let PreparedCallArgument::IndexedStruct(element) = prepared else {
                return Ok(None);
            };
            return self.selected_processor_view(
                source.base,
                Value::Local(element.index),
                block,
                expression.loc(),
            );
        }
        match expression {
            Expr::Var { name, .. } => self.processor_view(name),
            _ => match prepared {
                PreparedCallArgument::DirectReference(Expr::Var { name, .. }) => {
                    self.processor_view(name)
                }
                _ => Ok(None),
            },
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
            if let Some(len) = storage.len {
                self.append_proc_array_call_arguments(
                    "processor capture",
                    "owner",
                    &storage.proc_name,
                    len,
                    &Expr::var(&root),
                    block,
                    out,
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
        if has_scalar_lifecycle_capture(expected, self.proc_array_hook_types) {
            let active =
                self.processor_environment_active_slice(captured, block, location.clone())?;
            out.push(CallArgument::Value(active.value));
        }
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
        if expected.has_indexed_resources() {
            out.push(CallArgument::Value(captured.resource_offset));
        }
        for (resource, shape) in captured.resources.iter().zip(&expected.resources) {
            if shape.binding.is_none() {
                let resource = resource.as_ref().ok_or_else(|| {
                    self.error(
                        "processor environment lost its direct resource capture",
                        location.clone(),
                    )
                })?;
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
                outer,
                captured,
                false,
                block,
                out,
                location.clone(),
            )?;
        }
        if expected.resource_sources.len() != captured.resource_sources.len() {
            return Err(self.error(
                "processor element sources changed their capture schema",
                location,
            ));
        }
        for (shape, source) in expected
            .resource_sources
            .iter()
            .zip(&captured.resource_sources)
        {
            self.append_processor_view_arguments(shape, source, block, out, location.clone())?;
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
        if expected.proc_name != captured.shape.proc_name
            || expected.extent != captured.shape.extent
        {
            return Err(self.error(
                "processor environment changed nominal type or extent",
                location,
            ));
        }
        if expected.environments.len() > 1 {
            let tag = captured.tag.ok_or_else(|| self.error(format!("processor environment choice lost its tag in {}: expected {expected:?}, captured {captured:?}", self.function.name), location.clone()))?;
            out.push(CallArgument::Value(tag));
        }
        if expected.is_array() {
            out.push(CallArgument::Value(captured.start));
        }
        for environment in &expected.environments {
            let index = captured.shape.environments.iter().position(|shape| shape == environment).ok_or_else(|| self.error(format!("processor argument lost its specialized environment shape in {}: expected {environment:?}, captured {:?}", self.function.name, captured.shape), location.clone()))?;
            self.append_processor_environment_arguments(
                environment,
                &captured.environments[index],
                expected.is_array(),
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
                environment.shape.owner.is_some()
                    || !environment.resources.is_empty()
                    || (self.proc_array_hook_types.contains(&view.shape.proc_name)
                        && environment
                            .shape
                            .state
                            .as_ref()
                            .is_some_and(|state| state.storage.len.is_some()))
            }) {
                return Ok(None);
            }
            let raw = self.lower_expr(source.index, block)?;
            let raw = self.coerce(raw, PrimitiveType::I32, block, source.index.loc())?;
            let extent = match self.bindings.get(source.base).cloned() {
                Some(Binding::StructArrayParameter { length, .. }) => {
                    self.struct_array_length_value(length, block, location.clone())
                }
                Some(Binding::ProcArrayParameter { fixed_len, .. }) => {
                    Value::Constant(ScalarValue::I32(fixed_len as i32))
                }
                _ => Value::Constant(ScalarValue::I32(view.shape.array_len().ok_or_else(|| {
                    self.error(
                        "processor operation lost its array extent",
                        location.clone(),
                    )
                })? as i32)),
            };
            let coordinate = self.materialize_index_to_length(
                raw.value,
                extent,
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
            self.selected_processor_view(
                source.base,
                Value::Local(coordinate),
                block,
                receiver.expr.loc(),
            )?
            .expect("resolved processor array")
        } else if let Expr::Var { name, .. } = &receiver.expr {
            let Some(view) = self.processor_view(name)? else {
                return Ok(None);
            };
            view
        } else {
            return Ok(None);
        };
        if view.environments.len() == 1 {
            let frame = &view.environments[0];
            if frame.shape.owner.is_none()
                && frame.resources.is_empty()
                && !has_scalar_lifecycle_capture(&frame.shape, self.proc_array_hook_types)
                && frame
                    .shape
                    .state
                    .as_ref()
                    .is_none_or(|state| state.storage.len.is_none())
            {
                return Ok(None);
            }
        }
        // Authored arguments execute before the operation selects resources or
        // activates its lifecycle. Preserve their captured values in every arm.
        let normalized_args = self.capture_processor_payload(
            name,
            &normalized_args,
            operation.resources.len(),
            block,
            location.clone(),
        )?;
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
        self.begin_processor_view_slot(&operation, &args[0], environment, block, location.clone())?;
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

    pub(super) fn processor_operation_target(
        &self,
        name: &str,
        operation: &crate::processor_views::GeneratedProcessorOperation,
        environment: &LoweredProcessorEnvironment,
    ) -> String {
        environment
            .shape
            .owner
            .as_ref()
            .and_then(|owner| {
                self.processor_views.operation_target_for(
                    crate::compile_context::CompileContext::new(AnalysisOptions {
                        sample_rate: self.config.sample_rate,
                        block_size: self.config.block_size as usize,
                    }),
                    owner,
                    &operation.kind,
                )
            })
            .unwrap_or(name)
            .to_owned()
    }

    pub(super) fn processor_operation_arguments(
        &mut self,
        name: &str,
        args: &[onda_frontend::CallArg],
        operation: &crate::processor_views::GeneratedProcessorOperation,
        environment: &LoweredProcessorEnvironment,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<(String, Vec<onda_frontend::CallArg>), MirLoweringError> {
        let target = self.processor_operation_target(name, operation, environment);
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
        rewritten.extend_from_slice(&args[1..]);
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
        let resources =
            self.processor_operation_resources(environment, &resource_plan, block, location)?;
        rewritten.extend(
            resources
                .into_iter()
                .map(|expr| onda_frontend::CallArg { name: None, expr }),
        );
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
        self.dispatch_processor_environments(
            view,
            block,
            location.clone(),
            |this, environment, branch| {
                this.begin_processor_view_slot(
                    operation,
                    &args[0],
                    environment,
                    branch,
                    location.clone(),
                )?;
                let (target, arguments) = this.processor_operation_arguments(
                    name,
                    args,
                    operation,
                    environment,
                    branch,
                    location.clone(),
                )?;
                let values = this
                    .lower_direct_user_call_into(
                        &target,
                        &[],
                        &arguments,
                        result_storage,
                        location.clone(),
                        branch,
                    )?
                    .ok_or_else(|| {
                        this.error(
                            "processor environment alternative has no result contract",
                            location.clone(),
                        )
                    })?;
                for ((local, ty), value) in results.iter().zip(values) {
                    let value = this.coerce(value, *ty, branch, location.clone())?;
                    this.assign_value(branch, *local, value.value, location.clone());
                }
                Ok(())
            },
        )?;
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

    /// Lower each environment inside its selected branch. Temporary bindings
    /// created while preparing an operation cannot escape into another choice.
    pub(super) fn dispatch_processor_environments(
        &mut self,
        view: &LoweredProcessorView,
        block: &mut MirBlock,
        location: SourceLoc,
        mut emit: impl FnMut(
            &mut Self,
            &LoweredProcessorEnvironment,
            &mut MirBlock,
        ) -> Result<(), MirLoweringError>,
    ) -> Result<(), MirLoweringError> {
        if view.environments.len() == 1 {
            return emit(self, &view.environments[0], block);
        }
        let tag = view.tag.ok_or_else(|| {
            self.error(
                "processor environment choice lost its captured tag",
                location.clone(),
            )
        })?;
        let outer = self.bindings.clone();
        let mut alternatives = Vec::with_capacity(view.environments.len());
        for environment in &view.environments {
            let mut branch = MirBlock::default();
            let result = emit(self, environment, &mut branch);
            self.bindings.clone_from(&outer);
            result?;
            alternatives.push(branch);
        }
        let selected = self.processor_choice_block(tag, alternatives, 0, location);
        block.statements.extend(selected.statements);
        Ok(())
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
    fn capture_processor_payload(
        &mut self,
        function: &str,
        args: &[onda_frontend::CallArg],
        resource_count: usize,
        block: &mut MirBlock,
        location: SourceLoc,
    ) -> Result<Vec<onda_frontend::CallArg>, MirLoweringError> {
        let index = *self.function_indices.get(function).ok_or_else(|| {
            self.error("processor operation lost its declaration", location.clone())
        })?;
        let callee = &self.functions[index];
        let resolved = super::calls::resolve_call_arguments(callee, args, location.clone())?;
        let payload_end = callee
            .params
            .len()
            .checked_sub(resource_count)
            .filter(|end| *end >= 1)
            .ok_or_else(|| {
                self.error("processor operation lost its resource arguments", location)
            })?;
        let mut captured = resolved.expressions[..payload_end]
            .iter()
            .map(|expr| onda_frontend::CallArg {
                name: None,
                expr: (*expr).clone(),
            })
            .collect::<Vec<_>>();
        for index in resolved.evaluation_order {
            if index == 0 || index >= payload_end {
                continue;
            }
            let arg = &mut captured[index];
            let kind = &callee.param_kinds[index];
            let expression = &arg.expr;
            if let TypedFnParam::Struct { struct_name } = kind {
                let data = DataType::Struct(struct_name.clone());
                arg.expr = Expr::var(self.lower_data_expr(expression, &data, block)?)
                    .with_loc(expression.loc());
                continue;
            }
            let prepared = self.prepare_call_argument(
                function,
                &callee.params[index],
                kind,
                &callee.readonly_data_params,
                expression,
                block,
            )?;
            let mut scalar = |value: LoweredValue| {
                let name = self.fresh_data_name();
                let local = self.new_local(None, value.ty);
                self.assign_value(block, local, value.value, expression.loc());
                self.bindings
                    .insert(name.clone(), Binding::Local(local, value.ty));
                Expr::var(name).with_loc(expression.loc())
            };
            let captured = match prepared {
                PreparedCallArgument::Scalar(value) => scalar(value),
                PreparedCallArgument::Tuple(values) => Expr::Tuple {
                    loc: expression.loc().into(),
                    values: values.into_iter().map(&mut scalar).collect(),
                },
                PreparedCallArgument::Array(slice) => {
                    let name = self.fresh_data_name();
                    let Value::Local(local) = slice.value else {
                        unreachable!("prepared slices are locals")
                    };
                    self.bindings.insert(
                        name.clone(),
                        Binding::Slice(local, slice.element, slice.access, None),
                    );
                    Expr::var(name).with_loc(expression.loc())
                }
                PreparedCallArgument::DirectReference(expression) => expression,
                PreparedCallArgument::IndexedStruct(_) => {
                    unreachable!("structured payload captured as data")
                }
            };
            arg.expr = captured;
        }
        Ok(captured)
    }
}
