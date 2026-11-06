//! Logical processor capabilities shared by source specialization and MIR.
//!
//! Shapes contain declarations, never physical instance roots or selected
//! coordinates. Values retain those captures separately so equal shapes share
//! code while referring to independent storage and current resource bindings.

use std::collections::HashMap;

use onda_frontend::{Expr, PrimitiveType};

use crate::TypedBufferChannels;
mod boundaries;
pub(crate) use boundaries::{
    record_boundary_operations, record_function_operations, ProcessorBoundary,
};

const BOUND_RESOURCE_PREFIX: &str = "__onda_bound_buffer_";

pub(crate) fn bound_resource_member(receiver: &str, index: usize) -> String {
    format!("{receiver}.{BOUND_RESOURCE_PREFIX}{index}")
}

/// Generated resource operands are capabilities of the checked receiver, not
/// lexical buffer variables. MIR resolves and validates their actual bindings.
pub(crate) fn is_bound_resource_argument(
    expression: &Expr,
    parameter: Option<&onda_frontend::FnParamType>,
) -> bool {
    matches!(
        parameter,
        Some(
            onda_frontend::FnParamType::Buffer(_) | onda_frontend::FnParamType::BufferArray { .. }
        )
    ) && matches!(expression, Expr::Var { name, .. } if name.rsplit_once('.')
            .and_then(|(_, field)| field.strip_prefix(BOUND_RESOURCE_PREFIX))
            .and_then(|index| index.parse::<usize>().ok()).is_some())
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
    fn proc_name(&self) -> &str {
        self.edges
            .last()
            .map_or(&self.owner, |edge| &edge.proc_name)
    }

    pub fn parent_and_declaration(&self) -> Option<(&str, &ProcessorProjectionEdge)> {
        let (edge, parents) = self.edges.split_last()?;
        Some((
            parents
                .last()
                .map_or(self.owner.as_str(), |parent| parent.proc_name.as_str()),
            edge,
        ))
    }

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

impl ProcessorOperation {
    fn lifecycle_dependencies(&self) -> &'static [Self] {
        match self {
            Self::Step | Self::CallOutput(_) => &[Self::BlockPre, Self::BlockPost],
            _ => &[],
        }
    }
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

/// A constructor recipe evaluated against the enclosing captured environment.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ProcessorResourceBinding {
    Declaration {
        owner_struct: String,
        field_name: String,
        resource_index: usize,
    },
    RootRecipe {
        recipe: usize,
        resource_index: usize,
    },
    Forwarded {
        resource_index: usize,
    },
}

/// Root recipes borrow their base resource collections. Their selectors stay
/// in the declaration's global scope and execute only at the selected operation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRootResourceRecipe {
    pub resources: Vec<Expr>,
    pub parameters: Vec<String>,
    pub captures: Vec<ProcessorResourceShape>,
    pub globals: Vec<String>,
}

pub(crate) struct ProcessorResourceRecipe<'a> {
    pub expression: &'a Expr,
    pub parameters: &'a [String],
    pub globals: &'a [String],
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
    pub binding: Option<ProcessorResourceBinding>,
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
            &self.binding,
        )
            .cmp(&(
                other.elem_ty.name(),
                channels(&other.channels),
                other.len,
                other.mapped,
                other.mapped_len,
                other.alternatives,
                &other.binding,
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
    /// Local copies own activation until their enclosing call or sample ends.
    /// Retained scalar roots use the scheduled block hooks instead.
    pub local_lifecycle: bool,
}

impl ProcessorStateShape {
    pub(crate) fn is_scalar_root(&self) -> bool {
        !self.local_lifecycle && self.storage.len.is_none() && self.projection.edges.is_empty()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorEnvironmentShape {
    pub state: Option<ProcessorStateShape>,
    pub owner_storage: Option<ProcessorStorageShape>,
    /// The owner projection selects the shared callback implementation.
    /// A resource-only environment has no owner projection.
    pub owner: Option<ProcessorProjection>,
    pub resources: Vec<ProcessorResourceShape>,
    /// Explicit scalar initializers of an owned array. Their resources are
    /// selected by the copied row; storage and lifecycle belong to the copy.
    pub resource_sources: Vec<ProcessorViewShape>,
    /// An owner's own borrowed environment remains available to its callbacks.
    pub outer: Option<Box<ProcessorEnvironmentShape>>,
}

impl ProcessorEnvironmentShape {
    /// A pure identity forwarding frame adds no resource scope or selection.
    /// Owned copies can reuse its parent instead of retaining copy history.
    pub(crate) fn forwards_resources(&self) -> bool {
        self.owner.is_none()
            && self.outer.is_some()
            && self.resource_sources.is_empty()
            && !self.resources.is_empty()
            && self.resources.iter().enumerate().all(|(index, resource)| {
                resource.binding
                    == Some(ProcessorResourceBinding::Forwarded {
                        resource_index: index,
                    })
            })
    }

    pub(crate) fn has_indexed_resources(&self) -> bool {
        !self.resource_sources.is_empty()
            || self
                .resources
                .iter()
                .any(|resource| resource.mapped || resource.alternatives.is_some())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ProcessorViewExtent {
    Scalar,
    Array(usize),
    Slice,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct ProcessorViewShape {
    pub proc_name: String,
    pub extent: ProcessorViewExtent,
    /// A canonical set of alternatives. A single alternative needs no tag.
    pub environments: Vec<ProcessorEnvironmentShape>,
}

impl ProcessorViewShape {
    pub fn is_array(&self) -> bool {
        self.extent != ProcessorViewExtent::Scalar
    }

    pub fn array_len(&self) -> Option<usize> {
        match self.extent {
            ProcessorViewExtent::Array(len) => Some(len),
            _ => None,
        }
    }

    pub fn sliced(&self) -> Self {
        let mut shape = self.clone();
        shape.extent = ProcessorViewExtent::Slice;
        shape
    }

    pub(crate) fn owned_elements(sources: &[Self]) -> Option<Self> {
        let first = sources.first()?;
        if sources
            .iter()
            .any(|source| source.proc_name != first.proc_name || source.is_array())
        {
            return None;
        }
        let mut shape = first.owned_array(sources.len());
        let mut environment = shape.environments.remove(0);
        environment.outer = None;
        environment.resource_sources = if environment.resources.is_empty() {
            Vec::new()
        } else {
            sources.iter().map(Self::resource_source).collect()
        };
        shape.environments = vec![environment];
        Some(shape)
    }

    /// Copying resources retains their declaration context, not the history of
    /// owned rows through which that context was selected.
    pub(crate) fn resource_source(&self) -> Self {
        let mut environments = Vec::new();
        fn collect(frame: &ProcessorEnvironmentShape, out: &mut Vec<ProcessorEnvironmentShape>) {
            if frame.forwards_resources() {
                collect(
                    frame.outer.as_deref().expect("forwarded resource parent"),
                    out,
                );
            } else if !frame.resource_sources.is_empty() {
                for source in &frame.resource_sources {
                    for frame in &source.environments {
                        collect(frame, out);
                    }
                }
            } else if !out.contains(frame) {
                out.push(frame.clone());
            }
        }
        for frame in &self.environments {
            collect(frame, &mut environments);
        }
        environments.sort();
        Self {
            environments,
            ..self.selected()
        }
    }

    pub(crate) fn owned(&self) -> Self {
        self.owned_as(self.array_len())
    }

    pub(crate) fn owned_array(&self, len: usize) -> Self {
        self.owned_as(Some(len))
    }

    fn owned_as(&self, len: Option<usize>) -> Self {
        let mut shape = self.clone();
        shape.extent = len.map_or(ProcessorViewExtent::Scalar, ProcessorViewExtent::Array);
        if shape.environments.is_empty() {
            shape.environments.push(ProcessorEnvironmentShape {
                state: None,
                owner_storage: None,
                owner: None,
                resources: Vec::new(),
                resource_sources: Vec::new(),
                outer: None,
            });
        }
        for environment in &mut shape.environments {
            let source = (!self.is_array()).then(|| {
                if environment.forwards_resources() {
                    environment
                        .outer
                        .as_deref()
                        .expect("forwarded resource parent")
                        .clone()
                } else {
                    environment.clone()
                }
            });
            environment.state = Some(ProcessorStateShape {
                local_lifecycle: false,
                storage: ProcessorStorageShape {
                    proc_name: shape.proc_name.clone(),
                    len,
                },
                projection: ProcessorProjection {
                    owner: shape.proc_name.clone(),
                    edges: Vec::new(),
                },
            });
            environment.owner = None;
            environment.owner_storage = None;
            // Array copies preserve row order. Their resource recipes select
            // the copied row, while retaining the original enclosing scope.
            // Scalar copies instead retain their already selected source row.
            let Some(source) = source else { continue };
            environment.outer = (!source.resources.is_empty()).then(|| Box::new(source));
            environment.resource_sources.clear();
            for (resource_index, resource) in environment.resources.iter_mut().enumerate() {
                resource.mapped = false;
                resource.mapped_len = None;
                resource.alternatives = None;
                resource.binding = Some(ProcessorResourceBinding::Forwarded { resource_index });
            }
        }
        shape
    }

    pub(crate) fn is_scalar_root(&self) -> bool {
        !self.is_array()
            && self.environments.iter().all(|environment| {
                environment
                    .state
                    .as_ref()
                    .is_some_and(ProcessorStateShape::is_scalar_root)
            })
    }

    pub fn requires_environment(&self) -> bool {
        self.extent == ProcessorViewExtent::Slice
            || self.environments.iter().any(|environment| {
                environment.owner.is_some()
                    || !environment.resources.is_empty()
                    || environment.outer.is_some()
                    || !environment.resource_sources.is_empty()
                    || environment.state.as_ref().is_some_and(|state| {
                        state.local_lifecycle
                            || !state.projection.edges.is_empty()
                            || state.storage.proc_name != self.proc_name
                            || state.storage.len != self.array_len()
                    })
            })
    }

    pub fn plain(proc_name: String, len: Option<usize>) -> Self {
        Self {
            proc_name,
            extent: len.map_or(ProcessorViewExtent::Scalar, ProcessorViewExtent::Array),
            environments: Vec::new(),
        }
    }

    pub fn selected(&self) -> Self {
        let mut selected = self.clone();
        selected.extent = ProcessorViewExtent::Scalar;
        selected
    }

    pub(crate) fn with_local_lifecycle(mut self, local: bool) -> Self {
        for frame in &mut self.environments {
            if let Some(state) = &mut frame.state {
                state.local_lifecycle = local;
            }
        }
        self
    }

    pub fn join(&self, other: &Self) -> Option<Self> {
        if self.proc_name != other.proc_name || self.extent != other.extent {
            return None;
        }
        let plain = ProcessorEnvironmentShape {
            state: None,
            owner_storage: None,
            owner: None,
            resources: Vec::new(),
            resource_sources: Vec::new(),
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
            extent: self.extent.clone(),
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
    pub(crate) boundary_operations: HashMap<ProcessorBoundary, Vec<(String, ProcessorViewShape)>>,
    pub(crate) function_operations: HashMap<String, Vec<(String, ProcessorViewShape)>>,
    pub root_resource_recipes: Vec<ProcessorRootResourceRecipe>,
    pub lexical_resources: HashMap<String, Vec<ProcessorLexicalResource>>,
    declarations: Vec<ProcessorViewDeclaration>,
    operations: Vec<GeneratedProcessorOperation>,
    declaration_indices: HashMap<String, HashMap<String, usize>>,
    operation_indices: HashMap<String, usize>,
    operation_targets: HashMap<ProcessorProjection, HashMap<ProcessorOperation, usize>>,
    /// Nominal parameters can select any nested declaration of their owner type.
    nested_operation_targets: HashMap<String, HashMap<(String, ProcessorOperation), Vec<usize>>>,
    pub roots: HashMap<String, ProcessorViewSource>,
    /// Checked init aliases used when retaining their runtime operation adapters.
    pub(crate) boundary_views: HashMap<ProcessorBoundary, HashMap<String, ProcessorViewShape>>,
    pub owner_resources: HashMap<String, Vec<String>>,
    /// Source specialization metadata survives compile-context materialization.
    pub function_params: HashMap<String, HashMap<String, ProcessorViewShape>>,
    /// Nominal processor parameters can derive nested views inside a helper.
    pub function_processors: HashMap<String, Vec<String>>,
    /// Operations present in an executable body, before context materialization.
    pub function_operation_uses:
        HashMap<String, std::collections::HashSet<(String, ProcessorOperation)>>,
    /// Direct executable calls, keyed by source specialization.
    function_callees: HashMap<String, Vec<String>>,
    /// Concrete operation and resource-helper targets in the caller's compile context.
    pub(crate) materialized_targets:
        HashMap<(crate::compile_context::CompileContext, String), String>,
    shapes: HashMap<ProcessorViewShape, usize>,
}

impl ProcessorViewCatalog {
    /// Register definitions together so lookup keys cannot diverge from their
    /// ordered storage. Subsequent recipe rewriting only changes resources.
    pub(crate) fn set_definitions(
        &mut self,
        declarations: Vec<ProcessorViewDeclaration>,
        operations: Vec<GeneratedProcessorOperation>,
    ) {
        self.declaration_indices.clear();
        self.operation_indices.clear();
        self.operation_targets.clear();
        self.nested_operation_targets.clear();
        for (index, declaration) in declarations.iter().enumerate() {
            self.declaration_indices
                .entry(declaration.owner_struct.clone())
                .or_default()
                .entry(declaration.field_name.clone())
                .or_insert(index);
        }
        for (index, operation) in operations.iter().enumerate() {
            self.operation_indices
                .entry(operation.name.clone())
                .or_insert(index);
            self.operation_targets
                .entry(operation.projection.clone())
                .or_default()
                .entry(operation.kind.clone())
                .or_insert(index);
            if !operation.projection.edges.is_empty() {
                self.nested_operation_targets
                    .entry(operation.projection.owner.clone())
                    .or_default()
                    .entry((
                        operation.projection.proc_name().to_owned(),
                        operation.kind.clone(),
                    ))
                    .or_default()
                    .push(index);
            }
        }
        self.declarations = declarations;
        self.operations = operations;
    }

    pub fn declarations(&self) -> &[ProcessorViewDeclaration] {
        &self.declarations
    }

    pub fn operations(&self) -> &[GeneratedProcessorOperation] {
        &self.operations
    }

    pub(crate) fn declaration_resources_mut(&mut self, index: usize) -> &mut Vec<Expr> {
        &mut self.declarations[index].resources
    }

    pub(crate) fn has_lifecycle_children(&self, owner: &str) -> bool {
        self.declarations
            .iter()
            .filter(|declaration| declaration.owner_struct == owner)
            .any(|declaration| {
                declaration.active_field.is_some()
                    || self.has_lifecycle_children(&declaration.proc_name)
            })
    }

    /// Root post helpers complete descendants before their authored body.
    /// Projected wrappers are entered after that traversal has visited them.
    pub(crate) fn post_hook_descendant_owner(&self, function: &str) -> Option<&str> {
        let operation = self.operation(function)?;
        (operation.kind == ProcessorOperation::BlockPost
            && operation.projection.edges.is_empty()
            && self.has_lifecycle_children(&operation.projection.owner))
        .then_some(operation.projection.owner.as_str())
    }

    pub(crate) fn function_uses_root_resource_recipes(&self, function: &str) -> bool {
        fn uses(environment: &ProcessorEnvironmentShape) -> bool {
            environment.resources.iter().any(|resource| {
                matches!(
                    resource.binding,
                    Some(ProcessorResourceBinding::RootRecipe { .. })
                )
            }) || environment.outer.as_deref().is_some_and(uses)
                || environment
                    .resource_sources
                    .iter()
                    .any(|source| source.environments.iter().any(uses))
        }
        self.function_params
            .get(crate::compile_context::source_function_name(function))
            .is_some_and(|parameters| {
                parameters
                    .values()
                    .any(|shape| shape.environments.iter().any(uses))
            })
    }

    pub(crate) fn resource_recipe(
        &self,
        binding: &ProcessorResourceBinding,
    ) -> Option<ProcessorResourceRecipe<'_>> {
        let (expression, parameters, globals) = match binding {
            ProcessorResourceBinding::Declaration {
                owner_struct,
                field_name,
                resource_index,
            } => (
                self.declaration(owner_struct, field_name)?
                    .resources
                    .get(*resource_index)?,
                self.owner_resources.get(owner_struct)?.as_slice(),
                &[][..],
            ),
            ProcessorResourceBinding::RootRecipe {
                recipe,
                resource_index,
            } => {
                let recipe = self.root_resource_recipes.get(*recipe)?;
                (
                    recipe.resources.get(*resource_index)?,
                    recipe.parameters.as_slice(),
                    recipe.globals.as_slice(),
                )
            }
            ProcessorResourceBinding::Forwarded { .. } => return None,
        };
        Some(ProcessorResourceRecipe {
            expression,
            parameters,
            globals,
        })
    }

    /// Source checking sees nominal children; storage discovery keeps the
    /// owner's physical fields, which may already contain flattened children.
    pub(crate) fn source_struct_defs(
        &self,
        physical: &HashMap<String, Vec<crate::TypedStructField>>,
    ) -> HashMap<String, Vec<crate::TypedStructField>> {
        let mut source = physical.clone();
        for declaration in &self.declarations {
            let fields = source.entry(declaration.owner_struct.clone()).or_default();
            if declaration.len.is_none()
                && !fields
                    .iter()
                    .any(|field| field.name == declaration.field_name)
            {
                fields.push(crate::TypedStructField {
                    name: declaration.field_name.clone(),
                    ty: crate::TypedFieldType::Struct,
                    default: None,
                    integer_range: None,
                    struct_name: Some(declaration.proc_name.clone()),
                    array_elem_ty: None,
                    array_elem_struct: None,
                });
            }
        }
        source
    }

    pub fn lexical_resources(&self, function: &str) -> Option<&[ProcessorLexicalResource]> {
        self.lexical_resources
            .get(crate::compile_context::lowered_function_origin(function))
            .map(Vec::as_slice)
    }
    pub(crate) fn register_operation_uses(&mut self, functions: &[crate::TypedFunction]) {
        self.function_operation_uses.clear();
        self.function_callees.clear();
        for function in functions {
            let mut operations = std::collections::HashSet::new();
            let mut callees = std::collections::BTreeSet::new();
            for statement in &function.body {
                statement.visit_exprs(|expression| {
                    for expression in expression.walk() {
                        if let Expr::UserCall { name, .. } = expression {
                            if let Some(operation) = self.operation(name) {
                                // Guarded scalar completion is a dependency of
                                // an activation, never evidence of activation.
                                if operation.kind == ProcessorOperation::BlockPost
                                    && operation
                                        .projection
                                        .edges
                                        .last()
                                        .is_some_and(|edge| edge.extent.is_none())
                                {
                                    continue;
                                }
                                let proc_name = operation.projection.proc_name();
                                operations.insert((proc_name.to_owned(), operation.kind.clone()));
                                for dependency in operation.kind.lifecycle_dependencies() {
                                    operations.insert((proc_name.to_owned(), dependency.clone()));
                                }
                            }
                            callees.insert(
                                crate::compile_context::source_function_name(name).to_owned(),
                            );
                        }
                    }
                });
            }
            let origin = crate::compile_context::source_function_name(&function.name);
            self.function_operation_uses
                .entry(origin.to_owned())
                .or_default()
                .extend(operations);
            self.function_callees
                .entry(origin.to_owned())
                .or_default()
                .extend(callees);
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
            let environment = self.member_environment(declaration, parent_shape);
            let state_root = parent_capture
                .and_then(|capture| capture.state_root.clone())
                .or_else(|| Some(owner_expression.clone()));
            let resources = if parent_shape.is_some() {
                // Inherited resources remain declaration recipes. Their parent roots and
                // coordinates are captured below; descriptors are selected at the call.
                declaration.resources.clone()
            } else {
                declaration
                    .resources
                    .iter()
                    .map(|resource| {
                        instantiate_resource_recipe(
                            resource,
                            Some(owner_expression),
                            self.owner_resources
                                .get(owner_struct)
                                .map_or(&[], Vec::as_slice),
                            &[],
                        )
                    })
                    .collect()
            };
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
                extent: declaration
                    .len
                    .map_or(ProcessorViewExtent::Scalar, ProcessorViewExtent::Array),
                environments,
            },
            captures,
        })
    }

    pub fn member_environment(
        &self,
        declaration: &ProcessorViewDeclaration,
        parent: Option<&ProcessorEnvironmentShape>,
    ) -> ProcessorEnvironmentShape {
        let mut environment = declaration.environment.clone();
        if let Some(parent) = parent {
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
                    local_lifecycle: parent_state.local_lifecycle,
                    storage: parent_state.storage.clone(),
                    projection: projection.clone(),
                });
                if environment.owner.is_some() {
                    environment.owner = Some(projection);
                    environment.owner_storage = Some(parent_state.storage.clone());
                }
            }
            if parent.owner.is_some() || parent.outer.is_some() || !environment.resources.is_empty()
            {
                environment.outer = Some(Box::new(parent.clone()));
            }
            for (index, resource) in environment.resources.iter_mut().enumerate() {
                resource.binding = Some(ProcessorResourceBinding::Declaration {
                    owner_struct: declaration.owner_struct.clone(),
                    field_name: declaration.field_name.clone(),
                    resource_index: index,
                });
            }
        }
        environment
    }

    pub fn declaration(&self, owner: &str, field: &str) -> Option<&ProcessorViewDeclaration> {
        let index = self.declaration_indices.get(owner)?.get(field)?;
        self.declarations.get(*index)
    }

    pub fn operation(&self, name: &str) -> Option<&GeneratedProcessorOperation> {
        let origin = crate::compile_context::lowered_function_origin(name);
        self.operations.get(*self.operation_indices.get(origin)?)
    }

    pub fn operation_target(
        &self,
        projection: &ProcessorProjection,
        kind: &ProcessorOperation,
    ) -> Option<&str> {
        let index = self.operation_targets.get(projection)?.get(kind)?;
        Some(self.operations[*index].name.as_str())
    }

    pub fn function_param(&self, function: &str, param: &str) -> Option<&ProcessorViewShape> {
        let origin = crate::compile_context::source_function_name(function);
        self.function_params.get(origin)?.get(param)
    }

    pub(crate) fn operation_target_for(
        &self,
        context: crate::compile_context::CompileContext,
        projection: &ProcessorProjection,
        kind: &ProcessorOperation,
    ) -> Option<&str> {
        let target = self.operation_target(projection, kind)?;
        Some(self.materialized_target_for(context, target))
    }

    pub(crate) fn materialized_target_for<'a>(
        &'a self,
        context: crate::compile_context::CompileContext,
        target: &'a str,
    ) -> &'a str {
        self.materialized_targets
            .get(&(context, target.to_owned()))
            .map_or(target, String::as_str)
    }

    fn resource_dependencies(&self, shape: &ProcessorViewShape) -> Vec<&str> {
        let mut dependencies = std::collections::BTreeSet::new();
        let mut environments = shape.environments.iter().collect::<Vec<_>>();
        while let Some(environment) = environments.pop() {
            environments.extend(
                environment
                    .resource_sources
                    .iter()
                    .flat_map(|source| &source.environments),
            );
            if let Some(outer) = environment.outer.as_deref() {
                environments.push(outer);
            }
            for resource in &environment.resources {
                let Some(recipe) = resource
                    .binding
                    .as_ref()
                    .and_then(|binding| self.resource_recipe(binding))
                else {
                    continue;
                };
                for node in recipe.expression.walk() {
                    if let Expr::UserCall { name, .. } = node {
                        dependencies.insert(name.as_str());
                    }
                }
            }
        }
        dependencies.into_iter().collect()
    }

    /// Follow executable calls and implicit adapters from this boundary only.
    pub(crate) fn lifecycle_owner_types(
        &self,
        body: &[onda_frontend::Stmt],
        boundary: ProcessorBoundary,
    ) -> std::collections::HashSet<String> {
        let mut owners = std::collections::HashSet::new();
        let mut pending = Vec::new();
        for statement in body {
            statement.visit_exprs(|expression| {
                for expression in expression.walk() {
                    let Expr::UserCall { name, .. } = expression else {
                        continue;
                    };
                    if let Some((function, _)) = self
                        .function_callees
                        .get_key_value(crate::compile_context::source_function_name(name))
                    {
                        pending.push(function.as_str());
                    }
                }
            });
        }
        for (name, shape) in self
            .boundary_operations
            .get(&boundary)
            .into_iter()
            .flatten()
        {
            if self.operation(name).is_some_and(|operation| {
                matches!(
                    operation.kind,
                    ProcessorOperation::Step | ProcessorOperation::CallOutput(_)
                )
            }) {
                owners.extend(shape.environments.iter().filter_map(|environment| {
                    environment
                        .state
                        .as_ref()
                        .map(|state| state.storage.proc_name.clone())
                }));
            }
        }
        let mut visited = std::collections::HashSet::new();
        while let Some(function) = pending.pop() {
            let function = crate::compile_context::source_function_name(function);
            if !visited.insert(function) {
                continue;
            }
            if let Some(callees) = self.function_callees.get(function) {
                pending.extend(callees.iter().map(String::as_str));
            }
            pending.extend(self.function_dependencies(function));
            if !self
                .function_operation_uses
                .get(function)
                .is_some_and(|operations| {
                    operations.iter().any(|(_, operation)| {
                        matches!(
                            operation,
                            ProcessorOperation::Step | ProcessorOperation::CallOutput(_)
                        )
                    })
                })
            {
                continue;
            }
            for shape in self
                .function_params
                .get(function)
                .into_iter()
                .flat_map(HashMap::values)
            {
                for environment in &shape.environments {
                    if let Some(state) = &environment.state {
                        owners.insert(state.storage.proc_name.clone());
                    }
                }
            }
        }
        owners
    }

    pub(crate) fn root_operation_dependencies(
        &self,
        body: &[onda_frontend::Stmt],
        boundary: ProcessorBoundary,
    ) -> Vec<&str> {
        let mut targets = std::collections::BTreeSet::new();
        let owners = self.lifecycle_owner_types(body, boundary);
        // A borrowed child can be active even when its parent was never stepped.
        // The process boundary still owns its post-hook and current resources.
        for source in self.roots.values() {
            if !owners.contains(&source.shape.proc_name) {
                continue;
            }
            for operation in &self.operations {
                let Some((owner, edge)) = operation.projection.parent_and_declaration() else {
                    continue;
                };
                if operation.kind == ProcessorOperation::BlockPost
                    && operation.projection.owner == source.shape.proc_name
                    && self
                        .declaration(owner, &edge.declaration)
                        .is_some_and(|declaration| {
                            declaration.len.is_some() && declaration.active_field.is_some()
                        })
                {
                    targets.insert(operation.name.as_str());
                }
            }
        }
        self.operation_view_dependencies(
            self.boundary_operations
                .get(&boundary)
                .into_iter()
                .flatten(),
            &mut targets,
        );
        targets.into_iter().collect()
    }

    fn operation_view_dependencies<'a>(
        &'a self,
        operations: impl IntoIterator<Item = &'a (String, ProcessorViewShape)>,
        targets: &mut std::collections::BTreeSet<&'a str>,
    ) {
        for (name, shape) in operations {
            let Some(operation) = self.operation(name) else {
                continue;
            };
            targets.extend(self.resource_dependencies(shape));
            if !operation.projection.edges.is_empty() {
                continue;
            }
            for environment in &shape.environments {
                if let Some(target) = environment
                    .owner
                    .as_ref()
                    .and_then(|owner| self.operation_target(owner, &operation.kind))
                {
                    targets.insert(target);
                }
                if environment
                    .state
                    .as_ref()
                    .is_some_and(|state| !state.is_scalar_root())
                {
                    let projection = environment.owner.as_ref().unwrap_or(&operation.projection);
                    for dependency in operation.kind.lifecycle_dependencies() {
                        if let Some(target) = self.operation_target(projection, dependency) {
                            targets.insert(target);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn function_dependencies(&self, function: &str) -> Vec<&str> {
        fn owners<'a>(
            environment: &'a ProcessorEnvironmentShape,
            result: &mut Vec<&'a ProcessorProjection>,
        ) {
            if let Some(owner) = environment
                .owner
                .as_ref()
                .or_else(|| environment.state.as_ref().map(|state| &state.projection))
            {
                if !result.contains(&owner) {
                    result.push(owner);
                }
            }
            if let Some(outer) = &environment.outer {
                owners(outer, result);
            }
        }
        let origin = crate::compile_context::source_function_name(function);
        let mut dependencies = std::collections::BTreeSet::new();
        self.operation_view_dependencies(
            self.function_operations.get(origin).into_iter().flatten(),
            &mut dependencies,
        );
        let Some(used_operations) = self
            .function_operation_uses
            .get(origin)
            .filter(|operations| !operations.is_empty())
        else {
            return dependencies.into_iter().collect();
        };
        let mut projections = Vec::new();
        if let Some(params) = self.function_params.get(origin) {
            for shape in params.values() {
                for environment in &shape.environments {
                    owners(environment, &mut projections);
                }
            }
        }
        for projection in projections {
            for (proc_name, kind) in used_operations {
                if projection.proc_name() == proc_name {
                    if let Some(target) = self.operation_target(projection, kind) {
                        dependencies.insert(target);
                    }
                }
            }
        }
        for owner in self.function_processors.get(origin).into_iter().flatten() {
            if let Some(targets) = self.nested_operation_targets.get(owner) {
                for operation in used_operations {
                    for &index in targets.get(operation).into_iter().flatten() {
                        dependencies.insert(self.operations[index].name.as_str());
                    }
                }
            }
        }
        if let Some(parameters) = self.function_params.get(origin) {
            for shape in parameters.values() {
                dependencies.extend(self.resource_dependencies(shape));
            }
        }
        dependencies.into_iter().collect()
    }

    pub(crate) fn intern_shape(&mut self, shape: &ProcessorViewShape) -> usize {
        let next = self.shapes.len();
        *self.shapes.entry(shape.clone()).or_insert(next)
    }
}

/// Instantiate a constructor resource recipe against a captured owner and its bindings.
pub(crate) fn instantiate_resource_recipe(
    expression: &Expr,
    owner_expression: Option<&Expr>,
    resource_names: &[String],
    parent_resources: &[Option<Expr>],
) -> Expr {
    let mut resource = expression.clone();
    resource.visit_mut(|node| {
        if let Expr::Index { base, .. } | Expr::Slice { base, .. } = node {
            if let Some(index) = resource_names
                .iter()
                .position(|name| name == base.strip_prefix("self.").unwrap_or(base))
            {
                if let Some(Expr::Var { name, .. }) =
                    parent_resources.get(index).and_then(Option::as_ref)
                {
                    *base = name.clone();
                }
            } else if let (Some(field), Some(Expr::Var { name: owner, .. })) =
                (base.strip_prefix("self."), owner_expression)
            {
                *base = format!("{owner}.{field}");
            }
        }
        if let Expr::Var { name, .. } = node {
            if let Some((index, _)) = resource_names.iter().enumerate().find(|(_, resource)| {
                resource.as_str() == name.strip_prefix("self.").unwrap_or(name)
            }) {
                if let Some(capture) = parent_resources.get(index).and_then(Option::as_ref) {
                    *node = capture.clone();
                    return false;
                }
            }
            if name == "self" {
                if let Some(owner) = owner_expression {
                    *node = owner.clone();
                    return false;
                }
            }
            if let (Some(field), Some(Expr::Var { name: owner, .. })) =
                (name.strip_prefix("self."), owner_expression)
            {
                *name = format!("{owner}.{field}");
            }
        }
        true
    });
    resource
}
