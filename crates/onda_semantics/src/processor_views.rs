//! Logical processor capabilities shared by source specialization and MIR.
//!
//! Shapes contain declarations, never physical instance roots or selected
//! coordinates. Values retain those captures separately so equal shapes share
//! code while referring to independent storage and current resource bindings.

use std::collections::HashMap;

use onda_frontend::{Expr, PrimitiveType};

use crate::TypedBufferChannels;

pub(crate) fn bound_resource_member(receiver: &str, index: usize) -> String {
    format!("{receiver}.__onda_bound_buffer_{index}")
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorProjectionEdge {
    pub declaration: String,
    pub proc_name: String,
    pub extent: Option<usize>,
    pub coordinate_axis: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorProjection {
    pub owner: String,
    pub edges: Vec<ProcessorProjectionEdge>,
}

impl ProcessorProjection {
    pub fn coordinate_arity(&self) -> usize {
        self.edges
            .iter()
            .filter_map(|edge| edge.coordinate_axis)
            .max()
            .map_or(0, |axis| axis + 1)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ProcessorOperation {
    Init,
    Step,
    BlockPre,
    BlockPost,
    CallOutput(usize),
    Event(String),
    Local(String),
    ParameterHook(String),
}

#[derive(Debug, Clone)]
pub struct GeneratedProcessorOperation {
    pub name: String,
    pub projection: ProcessorProjection,
    pub kind: ProcessorOperation,
    pub resources: Vec<ProcessorOperationResource>,
}

#[derive(Debug, Clone)]
pub struct ProcessorOperationResource {
    /// Zero is the receiver; each following frame is its declared owner.
    pub frame_depth: usize,
    pub resource_index: usize,
}

/// One declared owner resource captured by an emitted function parameter.
/// Producers register this alongside the signature, before context cloning.
#[derive(Debug, Clone)]
pub struct ProcessorLexicalResource {
    pub owner_struct: String,
    pub resource_index: usize,
    pub parameter: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ProcessorResourceShape {
    pub elem_ty: PrimitiveType,
    pub channels: TypedBufferChannels,
    /// `None` is one buffer; `Some(n)` is a collection of n bindings.
    pub len: Option<usize>,
    /// Arbitrary authored bindings use an immutable logical binding map.
    pub mapped: bool,
    pub mapped_len: Option<usize>,
    /// A finite choice of scalar descriptors or contiguous descriptor spans.
    pub alternatives: Option<usize>,
}

impl Ord for ProcessorResourceShape {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn channels(channels: &TypedBufferChannels) -> (u8, usize) {
            match channels {
                TypedBufferChannels::Mono => (0, 1),
                TypedBufferChannels::Static(count) => (1, *count),
                TypedBufferChannels::Dynamic => (2, 0),
            }
        }
        (
            self.elem_ty.name(),
            channels(&self.channels),
            self.len,
            self.mapped,
            self.mapped_len,
            self.alternatives,
        )
            .cmp(&(
                other.elem_ty.name(),
                channels(&other.channels),
                other.len,
                other.mapped,
                other.mapped_len,
                other.alternatives,
            ))
    }
}

impl PartialOrd for ProcessorResourceShape {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorStorageShape {
    pub proc_name: String,
    pub len: Option<usize>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorStateShape {
    pub storage: ProcessorStorageShape,
    pub projection: ProcessorProjection,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorEnvironmentShape {
    pub state: Option<ProcessorStateShape>,
    pub owner_storage: Option<ProcessorStorageShape>,
    /// The owner projection selects the shared callback implementation.
    /// A resource-only environment has no owner projection.
    pub owner: Option<ProcessorProjection>,
    pub resources: Vec<ProcessorResourceShape>,
    /// An owner's own borrowed environment remains available to its callbacks.
    pub outer: Option<Box<ProcessorEnvironmentShape>>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct ProcessorViewShape {
    pub proc_name: String,
    /// Scalars have no extent. Processor arrays retain their fixed extent.
    pub len: Option<usize>,
    /// A canonical set of alternatives. A single alternative needs no tag.
    pub environments: Vec<ProcessorEnvironmentShape>,
}

impl ProcessorViewShape {
    pub fn requires_environment(&self) -> bool {
        self.environments.iter().any(|environment| {
            environment.owner.is_some()
                || !environment.resources.is_empty()
                || environment.outer.is_some()
                || environment.state.as_ref().is_some_and(|state| {
                    !state.projection.edges.is_empty()
                        || state.storage.proc_name != self.proc_name
                        || state.storage.len != self.len
                })
        })
    }

    pub fn plain(proc_name: String, len: Option<usize>) -> Self {
        Self {
            proc_name,
            len,
            environments: Vec::new(),
        }
    }

    pub fn selected(&self) -> Self {
        let mut selected = self.clone();
        selected.len = None;
        selected
    }

    pub fn join(&self, other: &Self) -> Option<Self> {
        if self.proc_name != other.proc_name || self.len != other.len {
            return None;
        }
        let plain = ProcessorEnvironmentShape {
            state: None,
            owner_storage: None,
            owner: None,
            resources: Vec::new(),
            outer: None,
        };
        let mut environments = if self.environments.is_empty() {
            vec![plain.clone()]
        } else {
            self.environments.clone()
        };
        let other_environments = if other.environments.is_empty() {
            std::slice::from_ref(&plain)
        } else {
            other.environments.as_slice()
        };
        for environment in other_environments {
            if !environments.contains(environment) {
                environments.push(environment.clone());
            }
        }
        // Declaration identities make this independent of branch ordering.
        environments.sort();
        Some(Self {
            proc_name: self.proc_name.clone(),
            len: self.len,
            environments,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProcessorEnvironmentCapture {
    pub state_root: Option<Expr>,
    pub owner: Option<Expr>,
    pub coordinates: Vec<Expr>,
    pub resources: Vec<Expr>,
    pub outer: Option<Box<ProcessorEnvironmentCapture>>,
}

#[derive(Debug, Clone)]
pub struct ProcessorViewSource {
    pub shape: ProcessorViewShape,
    pub captures: Vec<ProcessorEnvironmentCapture>,
}

#[derive(Debug, Clone)]
pub struct ProcessorViewDeclaration {
    pub owner_struct: String,
    pub field_name: String,
    pub proc_name: String,
    pub len: Option<usize>,
    pub active_field: Option<String>,
    pub environment: ProcessorEnvironmentShape,
    /// Constructor resource expressions in the declaration owner's scope.
    pub resources: Vec<Expr>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessorViewCatalog {
    pub lexical_resources: HashMap<String, Vec<ProcessorLexicalResource>>,
    pub declarations: Vec<ProcessorViewDeclaration>,
    pub operations: Vec<GeneratedProcessorOperation>,
    pub roots: HashMap<String, ProcessorViewSource>,
    pub owner_resources: HashMap<String, Vec<String>>,
    /// Source specialization metadata survives compile-context materialization.
    pub function_params: HashMap<String, HashMap<String, ProcessorViewShape>>,
    /// Nominal processor parameters can derive nested views inside a helper.
    pub function_processors: HashMap<String, Vec<String>>,
    /// Operations present in an executable body, before context materialization.
    pub function_operation_uses:
        HashMap<String, std::collections::HashSet<(String, ProcessorOperation)>>,
    /// Concrete compile-context target selected for a caller and source target.
    pub materialized_operations: HashMap<(String, String), String>,
    shapes: HashMap<ProcessorViewShape, usize>,
}

impl ProcessorViewCatalog {
    pub fn lexical_resources(&self, function: &str) -> Option<&[ProcessorLexicalResource]> {
        self.lexical_resources
            .get(crate::compile_context::lowered_function_origin(function))
            .map(Vec::as_slice)
    }
    pub(crate) fn register_operation_uses(&mut self, functions: &[crate::TypedFunction]) {
        for function in functions {
            let mut operations = std::collections::HashSet::new();
            for statement in &function.body {
                statement.visit_exprs(|expression| {
                    for expression in expression.walk() {
                        if let Expr::UserCall { name, .. } = expression {
                            if let Some(operation) = self.operation(name) {
                                let proc_name = operation
                                    .projection
                                    .edges
                                    .last()
                                    .map_or(&operation.projection.owner, |edge| &edge.proc_name);
                                operations.insert((proc_name.clone(), operation.kind.clone()));
                            }
                        }
                    }
                });
            }
            let origin = crate::compile_context::source_function_name(&function.name);
            self.function_operation_uses
                .insert(origin.to_owned(), operations);
        }
    }
    /// Instantiates declaration metadata without evaluating its expressions.
    /// Selection and MIR preparation capture coordinates in source order.
    pub fn member_source(
        &self,
        owner_struct: &str,
        field: &str,
        owner_expression: &Expr,
        outer: Option<&ProcessorViewSource>,
    ) -> Option<ProcessorViewSource> {
        let declaration = self.declaration(owner_struct, field)?;
        let alternatives = outer.map_or(1, |outer| outer.shape.environments.len().max(1));
        let mut environments = Vec::with_capacity(alternatives);
        let mut captures = Vec::with_capacity(alternatives);
        for variant in 0..alternatives {
            let parent_shape = outer.and_then(|outer| outer.shape.environments.get(variant));
            let parent_capture = outer.and_then(|outer| outer.captures.get(variant));
            let mut environment = declaration.environment.clone();
            if let Some(parent) = parent_shape {
                if let Some(parent_state) = &parent.state {
                    let mut projection = parent_state.projection.clone();
                    let axis = projection.coordinate_arity();
                    projection.edges.push(ProcessorProjectionEdge {
                        declaration: declaration.field_name.clone(),
                        proc_name: declaration.proc_name.clone(),
                        extent: declaration.len,
                        coordinate_axis: declaration.len.map(|_| axis),
                    });
                    environment.state = Some(ProcessorStateShape {
                        storage: parent_state.storage.clone(),
                        projection: projection.clone(),
                    });
                    if environment.owner.is_some() {
                        environment.owner = Some(projection);
                        environment.owner_storage = Some(parent_state.storage.clone());
                    }
                }
                if parent.owner.is_some()
                    || parent.outer.is_some()
                    || (environment.owner.is_some() && !parent.resources.is_empty())
                {
                    environment.outer = Some(Box::new(parent.clone()));
                }
            }
            let state_root = parent_capture
                .and_then(|capture| capture.state_root.clone())
                .or_else(|| Some(owner_expression.clone()));
            let mut resources = declaration.resources.clone();
            for resource in &mut resources {
                resource.visit_mut(|node| {
                    if let Expr::Index { base, .. } | Expr::Slice { base, .. } = node {
                        if let Some(index) =
                            self.owner_resources.get(owner_struct).and_then(|names| {
                                names.iter().position(|name| {
                                    name == base.strip_prefix("self.").unwrap_or(base)
                                })
                            })
                        {
                            if let Some(Expr::Var { name, .. }) =
                                parent_capture.and_then(|capture| capture.resources.get(index))
                            {
                                *base = name.clone();
                            }
                        }
                    }
                    if let Expr::Var { name, .. } = node {
                        if let Some((index, _)) =
                            self.owner_resources.get(owner_struct).and_then(|names| {
                                names.iter().enumerate().find(|(_, resource)| {
                                    resource.as_str() == name.strip_prefix("self.").unwrap_or(name)
                                })
                            })
                        {
                            if let Some(capture) =
                                parent_capture.and_then(|capture| capture.resources.get(index))
                            {
                                *node = capture.clone();
                                return false;
                            }
                        }
                        if name == "self" {
                            *node = owner_expression.clone();
                            return false;
                        }
                        if let (Some(field), Expr::Var { name: owner, .. }) =
                            (name.strip_prefix("self."), owner_expression)
                        {
                            *name = format!("{owner}.{field}");
                        }
                    }
                    true
                });
            }
            captures.push(ProcessorEnvironmentCapture {
                state_root: state_root.clone(),
                owner: environment.owner.as_ref().and(state_root),
                coordinates: parent_capture
                    .map_or_else(Vec::new, |capture| capture.coordinates.clone()),
                resources,
                outer: environment
                    .outer
                    .as_ref()
                    .and_then(|_| parent_capture.cloned())
                    .map(Box::new),
            });
            environments.push(environment);
        }
        Some(ProcessorViewSource {
            shape: ProcessorViewShape {
                proc_name: declaration.proc_name.clone(),
                len: declaration.len,
                environments,
            },
            captures,
        })
    }

    pub fn declaration(&self, owner: &str, field: &str) -> Option<&ProcessorViewDeclaration> {
        self.declarations.iter().find(|declaration| {
            declaration.owner_struct == owner && declaration.field_name == field
        })
    }

    pub fn operation(&self, name: &str) -> Option<&GeneratedProcessorOperation> {
        let origin = crate::compile_context::lowered_function_origin(name);
        self.operations
            .iter()
            .find(|operation| operation.name == origin)
    }

    pub fn operation_target(
        &self,
        projection: &ProcessorProjection,
        kind: &ProcessorOperation,
    ) -> Option<&str> {
        self.operations
            .iter()
            .find(|operation| operation.projection == *projection && operation.kind == *kind)
            .map(|operation| operation.name.as_str())
    }

    pub fn function_param(&self, function: &str, param: &str) -> Option<&ProcessorViewShape> {
        let origin = crate::compile_context::source_function_name(function);
        self.function_params.get(origin)?.get(param)
    }

    pub fn operation_target_for(
        &self,
        caller: &str,
        projection: &ProcessorProjection,
        kind: &ProcessorOperation,
    ) -> Option<&str> {
        let target = self.operation_target(projection, kind)?;
        Some(
            self.materialized_operations
                .get(&(caller.to_owned(), target.to_owned()))
                .map_or(target, String::as_str),
        )
    }

    pub(crate) fn function_dependencies(&self, function: &str) -> Vec<&str> {
        fn owners<'a>(
            environment: &'a ProcessorEnvironmentShape,
            result: &mut Vec<&'a ProcessorProjection>,
        ) {
            if let Some(owner) = &environment.owner {
                if !result.contains(&owner) {
                    result.push(owner);
                }
            }
            if let Some(outer) = &environment.outer {
                owners(outer, result);
            }
        }
        let origin = crate::compile_context::source_function_name(function);
        let Some(used_operations) = self.function_operation_uses.get(origin) else {
            return Vec::new();
        };
        let mut projections = Vec::new();
        if let Some(params) = self.function_params.get(origin) {
            for shape in params.values() {
                for environment in &shape.environments {
                    owners(environment, &mut projections);
                }
            }
        }
        let processors = self.function_processors.get(origin);
        self.operations
            .iter()
            .filter(|operation| {
                used_operations.contains(&(
                    operation.projection.edges.last().map_or_else(
                        || operation.projection.owner.clone(),
                        |edge| edge.proc_name.clone(),
                    ),
                    operation.kind.clone(),
                )) && (projections.contains(&&operation.projection)
                    || (!operation.projection.edges.is_empty()
                        && processors.is_some_and(|processors| {
                            processors.contains(&operation.projection.owner)
                        })))
            })
            .map(|operation| operation.name.as_str())
            .collect()
    }

    pub(crate) fn intern_shape(&mut self, shape: &ProcessorViewShape) -> usize {
        let next = self.shapes.len();
        *self.shapes.entry(shape.clone()).or_insert(next)
    }
}
