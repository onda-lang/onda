//! Declaration-based identities for processor views and generated operations.

use super::*;
use crate::processor_views::{
    GeneratedProcessorOperation, ProcessorEnvironmentCapture, ProcessorEnvironmentShape,
    ProcessorLexicalResource, ProcessorOperation, ProcessorOperationResource, ProcessorProjection,
    ProcessorProjectionEdge, ProcessorResourceShape, ProcessorStateShape, ProcessorStorageShape,
    ProcessorViewCatalog, ProcessorViewDeclaration, ProcessorViewShape, ProcessorViewSource,
};

#[derive(Clone)]
pub(super) struct DeclaredProjection {
    pub source_path: String,
    pub projection: ProcessorProjection,
}

impl DeclaredProjection {
    pub(super) fn source_path_for(edges: &[ProcessorProjectionEdge]) -> String {
        edges
            .iter()
            .map(|edge| {
                if edge.extent.is_some() {
                    format!("{}[0]", edge.declaration)
                } else {
                    edge.declaration.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("__")
    }
    pub(super) fn family_path(&self) -> String {
        self.projection
            .edges
            .iter()
            .map(|edge| {
                if edge.extent.is_some() {
                    format!("{}[]", edge.declaration)
                } else {
                    edge.declaration.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("__")
    }

    pub(super) fn proc_name(&self) -> &str {
        &self
            .projection
            .edges
            .last()
            .expect("child projection")
            .proc_name
    }
}

/// The operation ABI captures only resources used by its child or callback chain.
pub(super) fn projection_resources(
    declared: &DeclaredProjection,
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    options: AnalysisOptions,
) -> Vec<(ProcessorOperationResource, TypedBufferDecl)> {
    let child = declared.proc_name();
    let mut resources = shapes[child]
        .buffer_specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            (
                ProcessorOperationResource {
                    frame_depth: 0,
                    resource_index: index,
                },
                spec.clone(),
            )
        })
        .collect::<Vec<_>>();
    let mut context_index = 0;
    for edge_index in (0..declared.projection.edges.len()).rev() {
        let edge = &declared.projection.edges[edge_index];
        let owner = if edge_index == 0 {
            &declared.projection.owner
        } else {
            &declared.projection.edges[edge_index - 1].proc_name
        };
        if !nested_proc_captures_delegate_context(
            &procs[owner],
            &procs[&edge.proc_name],
            &edge.declaration,
            options,
            usize::from(edge.extent.is_some()),
        ) {
            continue;
        }
        for (index, spec) in shapes[owner].buffer_specs.iter().enumerate() {
            let mut spec = spec.clone();
            spec.name = delegate_owner_buffer_param_name(context_index);
            context_index += 1;
            resources.push((
                ProcessorOperationResource {
                    frame_depth: declared.projection.edges.len() - edge_index,
                    resource_index: index,
                },
                spec,
            ));
        }
    }
    resources
}

pub(super) fn operation_resources(
    declared: &DeclaredProjection,
    kind: &ProcessorOperation,
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    options: AnalysisOptions,
) -> Vec<(ProcessorOperationResource, TypedBufferDecl)> {
    let captures_child = match kind {
        ProcessorOperation::Local(name) => shapes[declared.proc_name()]
            .buffer_capturing_local_defs
            .contains(name),
        _ => true,
    };
    projection_resources(declared, shapes, procs, options)
        .into_iter()
        .filter(|(capture, _)| captures_child || capture.frame_depth != 0)
        .map(|(capture, mut spec)| {
            if capture.frame_depth == 0 {
                if let ProcessorOperation::Local(name) = kind {
                    if let Some(def) = procs[declared.proc_name()]
                        .local_defs
                        .iter()
                        .find(|def| def.name == *name)
                    {
                        spec.name = local_resources::parameter_name(
                            def,
                            &spec.name,
                            capture.resource_index,
                        );
                    }
                }
            }
            (capture, spec)
        })
        .collect()
}

pub(super) fn declared_projections(
    owner: &str,
    shapes: &HashMap<String, ProcLoweringShape>,
) -> Vec<DeclaredProjection> {
    fn visit(
        proc_name: &str,
        prefix: &str,
        projection: &ProcessorProjection,
        shapes: &HashMap<String, ProcLoweringShape>,
        result: &mut Vec<DeclaredProjection>,
    ) {
        let Some(shape) = shapes.get(proc_name) else {
            return;
        };
        let mut children = shape
            .state
            .nested_procs
            .iter()
            .map(|(name, child)| (name.clone(), child.proc_name.clone(), None))
            .collect::<Vec<_>>();
        children.extend(
            shape
                .state
                .nested_proc_arrays
                .iter()
                .filter_map(|(name, child)| {
                    Some((
                        name.clone(),
                        child.proc_name.clone(),
                        Some(shape.nested_proc_array_slots.get(name)?.len()),
                    ))
                }),
        );
        children.sort_by(|a, b| a.0.cmp(&b.0));
        for (declaration, child, extent) in children {
            let source = if extent.is_some() {
                format!("{declaration}[0]")
            } else {
                declaration.clone()
            };
            let source_path = if prefix.is_empty() {
                source
            } else {
                nested_field_name(prefix, &source)
            };
            let mut projected = projection.clone();
            projected.edges.push(ProcessorProjectionEdge {
                declaration,
                proc_name: child.clone(),
                extent,
                coordinate_axis: extent.map(|_| projection.coordinate_arity()),
            });
            result.push(DeclaredProjection {
                source_path: source_path.clone(),
                projection: projected.clone(),
            });
            visit(&child, &source_path, &projected, shapes, result);
        }
    }
    let mut result = Vec::new();
    visit(
        owner,
        "",
        &ProcessorProjection {
            owner: owner.to_owned(),
            edges: Vec::new(),
        },
        shapes,
        &mut result,
    );
    result
}

pub(super) fn projection_operations(
    declared: &DeclaredProjection,
    child: &ProcessorDef,
    shape: &ProcLoweringShape,
) -> Vec<(String, String, ProcessorOperation)> {
    let owner = &declared.projection.owner;
    let path = &declared.source_path;
    let family = declared.family_path();
    let mut operations = vec![
        (
            nested_init_fn_name(owner, path),
            nested_init_fn_name(owner, &family),
            ProcessorOperation::Init,
        ),
        (
            nested_step_fn_name(owner, path),
            nested_step_fn_name(owner, &family),
            ProcessorOperation::Step,
        ),
        (
            nested_block_pre_fn_name(owner, path),
            nested_block_pre_fn_name(owner, &family),
            ProcessorOperation::BlockPre,
        ),
        (
            nested_block_post_fn_name(owner, path),
            nested_block_post_fn_name(owner, &family),
            ProcessorOperation::BlockPost,
        ),
    ];
    operations.extend(child.events.iter().map(|event| {
        (
            nested_event_fn_name(owner, path, &event.name),
            nested_event_fn_name(owner, &family, &event.name),
            ProcessorOperation::Event(event.name.clone()),
        )
    }));
    operations.extend((0..shape.outs.len()).map(|index| {
        (
            nested_call_out_fn_name(owner, path, index),
            nested_call_out_fn_name(owner, &family, index),
            ProcessorOperation::CallOutput(index),
        )
    }));
    operations.extend(unique_proc_local_defs(child).into_iter().map(|def| {
        (
            nested_wrapper_local_def_name(owner, path, &def.name),
            nested_wrapper_local_def_name(owner, &family, &def.name),
            ProcessorOperation::Local(def.name.clone()),
        )
    }));
    operations
}

fn resource_shapes(specs: &[TypedBufferDecl], mapped: bool) -> Vec<ProcessorResourceShape> {
    specs
        .iter()
        .map(|spec| ProcessorResourceShape {
            elem_ty: spec.elem_ty,
            channels: spec.channels.clone(),
            len: spec.is_array.then_some(spec.array_len),
            mapped,
            mapped_len: None,
            alternatives: None,
            binding: None,
        })
        .collect()
}

pub(super) fn constructor_resources(
    owner: &ProcessorDef,
    field: &str,
    child: &ProcLoweringShape,
) -> Vec<Expr> {
    fn bound_arguments(arguments: &[CallArg], child: &ProcLoweringShape) -> Vec<Expr> {
        // Constructor lowering owns diagnostics; the catalog retains the same
        // bindings as unevaluated resource recipes.
        resolve_constructor_args(
            arguments,
            &child.param_specs,
            &child.buffer_specs,
            "processor constructor",
            &mut Vec::new(),
        )
        .into_iter()
        .skip(child.param_specs.len())
        .flatten()
        .cloned()
        .collect()
    }
    owner
        .init
        .iter()
        .find_map(|statement| {
            let Stmt::Assign {
                target: AssignTarget::Var(name),
                expr,
                ..
            } = statement
            else {
                return None;
            };
            if name != field {
                return None;
            }
            match expr {
                Expr::UserCall { args, .. } => Some(bound_arguments(args, child)),
                Expr::ArrayCtor {
                    init: Some(values), ..
                } => {
                    let constructors = values
                        .iter()
                        .filter_map(|value| match value {
                            Expr::UserCall { args, .. } => Some(bound_arguments(args, child)),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    let first = constructors.first()?;
                    Some(
                        (0..first.len())
                            .map(|index| {
                                let choices = constructors
                                    .iter()
                                    .filter_map(|arguments| arguments.get(index).cloned())
                                    .collect::<Vec<_>>();
                                if choices.iter().all(|choice| *choice == choices[0]) {
                                    choices[0].clone()
                                } else {
                                    Expr::ArrayLiteral {
                                        loc: Default::default(),
                                        values: choices,
                                    }
                                }
                            })
                            .collect(),
                    )
                }
                _ => None,
            }
        })
        .unwrap_or_default()
}

fn declaration_resource_shapes(
    owner: &ProcessorDef,
    child: &ProcLoweringShape,
    extent: Option<usize>,
    resources: &[Expr],
) -> Vec<ProcessorResourceShape> {
    child
        .buffer_specs
        .iter()
        .zip(resources)
        .map(|(spec, binding)| {
            let alternatives = match binding {
                Expr::ArrayLiteral { values, .. } => Some(values.len()),
                _ => None,
            };
            let mapped = extent.is_some()
                && match binding {
                    Expr::Var { name, .. } => {
                        !spec.is_array
                            && owner
                                .buffers
                                .iter()
                                .any(|buffer| buffer.name == *name && buffer.array_size.is_some())
                    }
                    _ => false,
                };
            ProcessorResourceShape {
                elem_ty: spec.elem_ty,
                channels: spec.channels.clone(),
                len: spec.is_array.then_some(spec.array_len),
                mapped,
                mapped_len: mapped
                    .then(|| extent.unwrap() * if spec.is_array { spec.array_len } else { 1 }),
                alternatives,
                binding: None,
            }
        })
        .collect()
}

pub(super) fn build_view_catalog(
    shapes: &HashMap<String, ProcLoweringShape>,
    procs: &HashMap<String, ProcessorDef>,
    top: &TopLevelProcRewriteMeta,
    proc_api: &HashMap<String, ProcApi>,
    options: AnalysisOptions,
    generated: &[Block],
) -> ProcessorViewCatalog {
    let signatures = generated
        .iter()
        .filter_map(|block| {
            let Block::Def(function) = block else {
                return None;
            };
            Some((
                function.name.as_str(),
                captured_resource_parameters(function),
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut catalog = ProcessorViewCatalog::default();
    let mut declarations = Vec::new();
    let mut operations = Vec::new();
    let mut owners = shapes.keys().collect::<Vec<_>>();
    owners.sort();
    for owner in owners {
        let Some(owner_proc) = procs.get(owner) else {
            continue;
        };
        catalog.owner_resources.insert(
            owner.clone(),
            shapes[owner]
                .buffer_specs
                .iter()
                .map(|spec| spec.name.clone())
                .collect(),
        );
        let root = ProcessorProjection {
            owner: owner.clone(),
            edges: Vec::new(),
        };
        let mut root_operations = vec![
            (
                format!("{owner}{PROC_INIT_FN_SUFFIX}"),
                ProcessorOperation::Init,
            ),
            (
                format!("{owner}{PROC_STEP_FN_SUFFIX}"),
                ProcessorOperation::Step,
            ),
            (
                format!("{owner}{PROC_BLOCK_PRE_FN_SUFFIX}"),
                ProcessorOperation::BlockPre,
            ),
            (
                format!("{owner}{PROC_BLOCK_POST_FN_SUFFIX}"),
                ProcessorOperation::BlockPost,
            ),
        ];
        root_operations.extend(owner_proc.events.iter().map(|event| {
            (
                format!("{owner}{PROC_EVENT_FN_PREFIX}{}", event.name),
                ProcessorOperation::Event(event.name.clone()),
            )
        }));
        root_operations.extend((0..shapes[owner].outs.len()).map(|index| {
            (
                format!("{owner}{PROC_CALL_OUT_FN_PREFIX}{index}"),
                ProcessorOperation::CallOutput(index),
            )
        }));
        root_operations.extend(unique_proc_local_defs(owner_proc).into_iter().map(|def| {
            (
                proc_local_hidden_def_name(owner, &def.name),
                ProcessorOperation::Local(def.name.clone()),
            )
        }));
        operations.extend(root_operations.into_iter().map(|(name, kind)| {
            GeneratedProcessorOperation {
                name,
                projection: root.clone(),
                resources: shapes[owner]
                    .buffer_specs
                    .iter()
                    .enumerate()
                    .filter(|_| match &kind {
                        ProcessorOperation::Local(name) => {
                            shapes[owner].buffer_capturing_local_defs.contains(name)
                        }
                        _ => true,
                    })
                    .map(|(resource_index, _)| ProcessorOperationResource {
                        frame_depth: 0,
                        resource_index,
                    })
                    .collect(),
                kind,
            }
        }));
        for declared in declared_projections(owner, shapes) {
            let Some(child) = procs.get(declared.proc_name()) else {
                continue;
            };
            let Some(shape) = shapes.get(declared.proc_name()) else {
                continue;
            };
            let edge = declared.projection.edges.last().expect("child edge");
            let direct = declared.projection.edges.len() == 1;
            let captures_owner = direct
                && nested_proc_captures_delegate_context(
                    owner_proc,
                    child,
                    &edge.declaration,
                    options,
                    declared.projection.coordinate_arity(),
                );
            if direct {
                let active_field = proc_api
                    .get(&edge.proc_name)
                    .filter(|api| proc_needs_block_hooks(api))
                    .and_then(|_| {
                        shapes[owner]
                            .nested_proc_active_fields
                            .get(&edge.declaration)
                    })
                    .cloned();
                let mut resources = constructor_resources(owner_proc, &edge.declaration, shape);
                let resource_aliases = persistent_buffer_binding_expressions(
                    &owner_proc.init,
                    &shapes[owner].buffer_specs,
                );
                for resource in &mut resources {
                    resource.visit_mut_postorder(|node| {
                        if let Expr::Var { name, .. } = node {
                            if let Some(expression) = resource_aliases.get(name) {
                                *node = expression.clone();
                            }
                        }
                    });
                }
                let mut resource_types =
                    declaration_resource_shapes(owner_proc, shape, edge.extent, &resources);
                for resource in &mut resources {
                    resource.visit_mut(|node| {
                        let name = match node {
                            Expr::Var { name, .. } | Expr::Index { base: name, .. } => Some(name),
                            _ => None,
                        };
                        if let Some(name) = name {
                            if owner_proc.buffers.iter().any(|buffer| buffer.name == *name)
                                || shapes[owner].field_names.contains(name)
                            {
                                *name = format!("self.{name}");
                            }
                        }
                        true
                    });
                }
                if captures_owner {
                    resources.extend(
                        shapes[owner]
                            .buffer_specs
                            .iter()
                            .map(|spec| Expr::var(format!("self.{}", spec.name))),
                    );
                    resource_types.extend(resource_shapes(&shapes[owner].buffer_specs, false));
                }
                declarations.push(ProcessorViewDeclaration {
                    owner_struct: owner.clone(),
                    field_name: edge.declaration.clone(),
                    proc_name: edge.proc_name.clone(),
                    len: edge.extent,
                    active_field: active_field.clone(),
                    environment: ProcessorEnvironmentShape {
                        resource_sources: Vec::new(),
                        state: Some(ProcessorStateShape {
                            local_lifecycle: false,
                            storage: ProcessorStorageShape {
                                proc_name: owner.clone(),
                                len: None,
                            },
                            projection: declared.projection.clone(),
                        }),
                        owner_storage: (captures_owner || active_field.is_some()).then(|| {
                            ProcessorStorageShape {
                                proc_name: owner.clone(),
                                len: None,
                            }
                        }),
                        owner: (captures_owner || active_field.is_some())
                            .then(|| declared.projection.clone()),
                        resources: resource_types,
                        outer: None,
                    },
                    resources,
                });
            }
            for (_, name, kind) in projection_operations(&declared, child, shape) {
                operations.push(GeneratedProcessorOperation {
                    name,
                    projection: declared.projection.clone(),
                    resources: operation_resources(&declared, &kind, shapes, procs, options)
                        .into_iter()
                        .map(|(capture, _)| capture)
                        .collect(),
                    kind,
                });
            }
        }
    }
    let slot_names = top
        .global_proc_array_slots
        .values()
        .flat_map(ProcArrayDeclaration::binding_names)
        .collect::<HashSet<_>>();
    for (name, instance) in &top.global_proc_instances {
        if slot_names.contains(name) {
            continue;
        }
        let resources = shapes
            .get(&instance.proc_name)
            .map(|shape| resource_shapes(&shape.buffer_specs, false))
            .unwrap_or_default();
        let environment = ProcessorEnvironmentShape {
            resource_sources: Vec::new(),
            state: Some(ProcessorStateShape {
                local_lifecycle: false,
                storage: ProcessorStorageShape {
                    proc_name: instance.proc_name.clone(),
                    len: None,
                },
                projection: ProcessorProjection {
                    owner: instance.proc_name.clone(),
                    edges: Vec::new(),
                },
            }),
            owner_storage: None,
            owner: None,
            resources,
            outer: None,
        };
        catalog.roots.insert(
            name.clone(),
            ProcessorViewSource {
                shape: ProcessorViewShape {
                    proc_name: instance.proc_name.clone(),
                    extent: crate::processor_views::ProcessorViewExtent::Scalar,
                    environments: vec![environment],
                },
                captures: vec![ProcessorEnvironmentCapture {
                    state_root: Some(Expr::var(name)),
                    owner: None,
                    coordinates: Vec::new(),
                    resources: instance.buffer_args.clone(),
                    outer: None,
                }],
            },
        );
    }
    for (name, slots) in &top.global_proc_array_slots {
        let Some(instance) = slots
            .first_binding()
            .and_then(|slot| top.global_proc_instances.get(slot))
        else {
            continue;
        };
        let bindings = (0..instance.buffer_args.len())
            .map(|index| {
                let choices = slots
                    .binding_names()
                    .filter_map(|slot| {
                        top.global_proc_instances
                            .get(slot)?
                            .buffer_args
                            .get(index)
                            .cloned()
                    })
                    .collect::<Vec<_>>();
                if choices.iter().all(|choice| *choice == choices[0]) {
                    choices[0].clone()
                } else {
                    Expr::ArrayLiteral {
                        loc: Default::default(),
                        values: choices,
                    }
                }
            })
            .collect::<Vec<_>>();
        let resources = shapes
            .get(&instance.proc_name)
            .map(|shape| {
                shape
                    .buffer_specs
                    .iter()
                    .zip(&bindings)
                    .map(|(spec, expression)| {
                        let choice_count = match expression {
                            Expr::ArrayLiteral { values, .. } => Some(values.len()),
                            _ => None,
                        };
                        let mapped = !spec.is_array && choice_count.is_some();
                        ProcessorResourceShape {
                            elem_ty: spec.elem_ty,
                            channels: spec.channels.clone(),
                            len: spec.is_array.then_some(spec.array_len),
                            mapped,
                            mapped_len: mapped.then_some(slots.len()),
                            alternatives: spec.is_array.then_some(choice_count).flatten(),
                            binding: None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let environment = ProcessorEnvironmentShape {
            resource_sources: Vec::new(),
            state: Some(ProcessorStateShape {
                local_lifecycle: false,
                storage: ProcessorStorageShape {
                    proc_name: instance.proc_name.clone(),
                    len: Some(slots.len()),
                },
                projection: ProcessorProjection {
                    owner: instance.proc_name.clone(),
                    edges: Vec::new(),
                },
            }),
            owner_storage: None,
            owner: None,
            resources,
            outer: None,
        };
        catalog.roots.insert(
            name.clone(),
            ProcessorViewSource {
                shape: ProcessorViewShape {
                    proc_name: instance.proc_name.clone(),
                    extent: crate::processor_views::ProcessorViewExtent::Array(slots.len()),
                    environments: vec![environment],
                },
                captures: vec![ProcessorEnvironmentCapture {
                    state_root: Some(Expr::var(name)),
                    owner: None,
                    coordinates: Vec::new(),
                    resources: bindings,
                    outer: None,
                }],
            },
        );
    }
    for operation in &mut operations {
        let Some(parameters) = signatures.get(operation.name.as_str()) else {
            operation.resources.clear();
            continue;
        };
        let specs = if operation.projection.edges.is_empty() {
            let shape = &shapes[&operation.projection.owner];
            let local = match &operation.kind {
                ProcessorOperation::Local(name) => procs[&operation.projection.owner]
                    .local_defs
                    .iter()
                    .find(|def| def.name == *name),
                _ => None,
            };
            local
                .map_or_else(
                    || shape.buffer_specs.clone(),
                    |def| local_resources::parameters(def, &shape.buffer_specs),
                )
                .into_iter()
                .map(|spec| spec.name)
                .collect::<Vec<_>>()
        } else {
            let declared = DeclaredProjection {
                source_path: DeclaredProjection::source_path_for(&operation.projection.edges),
                projection: operation.projection.clone(),
            };
            operation_resources(&declared, &operation.kind, shapes, procs, options)
                .into_iter()
                .map(|(_, spec)| spec.name)
                .collect()
        };
        let resources = std::mem::take(&mut operation.resources);
        let resources = resources
            .into_iter()
            .zip(specs)
            .filter(|(_, name)| parameters.contains(name))
            .collect::<Vec<_>>();
        catalog.lexical_resources.insert(
            operation.name.clone(),
            resources
                .iter()
                .map(|(resource, parameter)| {
                    let depth = operation.projection.edges.len() - resource.frame_depth;
                    let owner_struct = if depth == 0 {
                        operation.projection.owner.clone()
                    } else {
                        operation.projection.edges[depth - 1].proc_name.clone()
                    };
                    ProcessorLexicalResource {
                        owner_struct,
                        resource_index: resource.resource_index,
                        parameter: parameter.clone(),
                    }
                })
                .collect(),
        );
        operation.resources = resources
            .into_iter()
            .map(|(resource, _)| resource)
            .collect();
    }
    catalog.set_definitions(declarations, operations);
    catalog
}

pub(super) fn captured_resource_parameters(function: &FunctionDef) -> HashSet<String> {
    function
        .params
        .iter()
        .filter(|param| {
            matches!(
                param.ty,
                Some(FnParamType::Buffer(_) | FnParamType::BufferArray { .. })
            )
        })
        .map(|param| param.name.clone())
        .collect()
}
