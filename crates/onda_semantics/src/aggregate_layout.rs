use std::collections::{HashMap, HashSet};
use std::fmt;

use onda_frontend::PrimitiveType;
use onda_processor_abi::payload::{PayloadField, PayloadType, ScalarEncoding};

use crate::{TypedFieldType, TypedStruct, TypedStructField};

pub(crate) const MAX_AGGREGATE_NESTING: usize = 256;
/// Bounds the recursive shapes materialized across the program. Fixed array
/// extents remain tensor axes and therefore do not increase this count.
pub(crate) const MAX_AGGREGATE_LAYOUT_NODES: usize = 1 << 16;

/// Deterministic program-local identity for a resolved aggregate layout.
///
/// IDs are assigned by lexicographically sorted struct name, so source/module
/// declaration order cannot change the backend contract.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AggregateLayoutId(u32);

impl AggregateLayoutId {
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Deterministic identity for a primitive leaf within an aggregate layout.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AggregateLeafId(u32);

impl AggregateLeafId {
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One resolved component of the source-level path to a primitive leaf.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AggregatePathComponent {
    /// A named field. `aggregate` resolves a nested struct reference to its
    /// stable layout ID. `extent` is present when the field is an array.
    Field {
        name: String,
        aggregate: Option<AggregateLayoutId>,
        extent: Option<usize>,
    },
    /// A primitive tuple element within the preceding tuple field.
    TupleElement { index: usize },
}

/// A checked row-major tensor description in scalar elements.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AggregateTensorLayout {
    pub shape: Vec<usize>,
    pub strides: Vec<usize>,
    pub element_count: usize,
}

impl AggregateTensorLayout {
    pub fn scalar() -> Self {
        Self {
            shape: Vec::new(),
            strides: Vec::new(),
            element_count: 1,
        }
    }

    pub fn from_shape(shape: Vec<usize>) -> Result<Self, AggregateLayoutArithmeticError> {
        let mut strides = vec![0; shape.len()];
        let mut element_count = 1usize;
        for (index, extent) in shape.iter().copied().enumerate().rev() {
            strides[index] = element_count;
            element_count = element_count.checked_mul(extent).ok_or_else(|| {
                AggregateLayoutArithmeticError {
                    shape: shape.clone(),
                }
            })?;
        }
        Ok(Self {
            shape,
            strides,
            element_count,
        })
    }

    /// Returns the storage tensor for an array of this aggregate type.
    pub fn with_outer_extent(
        &self,
        outer_extent: usize,
    ) -> Result<Self, AggregateLayoutArithmeticError> {
        let mut shape = Vec::with_capacity(self.shape.len() + 1);
        shape.push(outer_extent);
        shape.extend_from_slice(&self.shape);
        Self::from_shape(shape)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AggregateLayoutArithmeticError {
    pub shape: Vec<usize>,
}

impl fmt::Display for AggregateLayoutArithmeticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "aggregate shape {:?} exceeds addressable size",
            self.shape
        )
    }
}

impl std::error::Error for AggregateLayoutArithmeticError {}

/// One primitive storage leaf after recursively resolving nested structs.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AggregateLeafLayout {
    pub id: AggregateLeafId,
    pub path: Vec<AggregatePathComponent>,
    /// Canonical structure-of-arrays storage suffix (`bins.real`, `pair.__0`).
    pub storage_path: String,
    pub scalar: PrimitiveType,
    pub tensor: AggregateTensorLayout,
    /// Scalar offset in a densely flattened value of one aggregate instance.
    pub scalar_offset: usize,
}

impl AggregateLeafLayout {
    pub fn storage_for_outer_extent(
        &self,
        outer_extent: usize,
    ) -> Result<AggregateTensorLayout, AggregateLayoutArithmeticError> {
        self.tensor.with_outer_extent(outer_extent)
    }
}

/// Fully resolved layout for one semantic struct.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AggregateLayout {
    pub id: AggregateLayoutId,
    pub struct_name: String,
    pub leaves: Vec<AggregateLeafLayout>,
    /// Number of primitive scalar slots in one densely flattened instance.
    pub scalar_width: usize,
    shape: PayloadType,
}

impl AggregateLayout {
    /// Resolved nominal shape shared with message schema and wire planning.
    pub fn payload_type(&self) -> &PayloadType {
        &self.shape
    }

    pub fn leaf(&self, id: AggregateLeafId) -> Option<&AggregateLeafLayout> {
        self.leaves.get(id.index()).filter(|leaf| leaf.id == id)
    }

    pub fn leaf_by_storage_path(&self, path: &str) -> Option<&AggregateLeafLayout> {
        self.leaves.iter().find(|leaf| leaf.storage_path == path)
    }
}

/// Backend-facing table of all aggregate layouts in a typed program.
#[derive(Debug, Clone, Default)]
pub struct AggregateLayoutTable {
    layouts: Vec<AggregateLayout>,
    ids_by_struct_name: HashMap<String, AggregateLayoutId>,
}

impl AggregateLayoutTable {
    pub fn build(structs: &[TypedStruct]) -> Result<Self, Vec<AggregateLayoutError>> {
        validate_aggregate_structure(structs).map_err(|error| vec![error])?;
        LayoutBuilder::new(structs)?.build()
    }

    pub(crate) fn populate_message_defaults<'a>(
        &mut self,
        params: impl Iterator<Item = &'a crate::TypedEventParam>,
        structs: &HashMap<String, Vec<crate::TypedStructField>>,
        options: crate::AnalysisOptions,
        errors: &mut Vec<onda_frontend::Diagnostic>,
    ) {
        let names = params
            .filter_map(|param| match &param.ty {
                crate::TypedEventParamType::Data(crate::DataType::Struct(name))
                | crate::TypedEventParamType::Data(crate::DataType::Array {
                    element: onda_frontend::ArrayElemType::Struct(name),
                    ..
                })
                | crate::TypedEventParamType::StructSlice { name } => Some(name),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut evaluator =
            crate::data_construction::DefaultEvaluator::new(structs, options, errors);
        for layout in &mut self.layouts {
            if names.contains(&layout.struct_name) {
                crate::data_construction::populate_schema_defaults(
                    &mut layout.shape,
                    &mut evaluator,
                );
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.layouts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.layouts.len()
    }

    pub fn layouts(&self) -> &[AggregateLayout] {
        &self.layouts
    }

    pub fn id_for_struct(&self, struct_name: &str) -> Option<AggregateLayoutId> {
        self.ids_by_struct_name.get(struct_name).copied()
    }

    pub fn get(&self, id: AggregateLayoutId) -> Option<&AggregateLayout> {
        self.layouts
            .get(id.index())
            .filter(|layout| layout.id == id)
    }

    pub fn layout_for_struct(&self, struct_name: &str) -> Option<&AggregateLayout> {
        self.id_for_struct(struct_name).and_then(|id| self.get(id))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AggregateLayoutError {
    DuplicateStruct {
        struct_name: String,
    },
    TooManyLayouts {
        count: usize,
    },
    LayoutsTooLarge {
        struct_name: String,
        count: usize,
        maximum: usize,
    },
    DuplicateField {
        struct_name: String,
        field_name: String,
    },
    UnknownStruct {
        struct_name: String,
        field_path: String,
        referenced_struct: String,
    },
    RecursiveAggregate {
        cycle: Vec<String>,
    },
    NestingTooDeep {
        struct_name: String,
        depth: usize,
        maximum: usize,
    },
    MalformedField {
        struct_name: String,
        field_name: String,
        reason: String,
    },
    SizeOverflow {
        struct_name: String,
        field_path: String,
        shape: Vec<usize>,
    },
}

impl fmt::Display for AggregateLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateStruct { struct_name } => {
                write!(f, "duplicate aggregate definition '{struct_name}'")
            }
            Self::TooManyLayouts { count } => {
                write!(f, "aggregate layout count {count} exceeds the u32 ID space")
            }
            Self::LayoutsTooLarge {
                struct_name,
                count,
                maximum,
            } => write!(
                f,
                "aggregate layouts exceed the limit of {maximum} expanded shape nodes while planning '{struct_name}' ({count} required)"
            ),
            Self::DuplicateField {
                struct_name,
                field_name,
            } => write!(
                f,
                "aggregate '{struct_name}' contains duplicate field '{field_name}'"
            ),
            Self::UnknownStruct {
                struct_name,
                field_path,
                referenced_struct,
            } => write!(
                f,
                "aggregate '{struct_name}' field '{field_path}' references unknown struct '{referenced_struct}'"
            ),
            Self::RecursiveAggregate { cycle } => {
                write!(f, "recursive aggregate layout cycle: {}", cycle.join(" -> "))
            }
            Self::NestingTooDeep {
                struct_name,
                depth,
                maximum,
            } => write!(
                f,
                "aggregate nesting ending at '{struct_name}' reaches depth {depth}, exceeding the limit of {maximum}"
            ),
            Self::MalformedField {
                struct_name,
                field_name,
                reason,
            } => write!(
                f,
                "aggregate '{struct_name}' field '{field_name}' has invalid resolved metadata: {reason}"
            ),
            Self::SizeOverflow {
                struct_name,
                field_path,
                shape,
            } => write!(
                f,
                "aggregate '{struct_name}' field '{field_path}' shape {shape:?} exceeds addressable size"
            ),
        }
    }
}

impl std::error::Error for AggregateLayoutError {}

fn aggregate_fields(definition: &TypedStruct) -> impl Iterator<Item = &TypedStructField> {
    definition
        .fields
        .iter()
        .filter(|field| !field.name.contains('.'))
}

/// Validates recursive structure and expanded shape size before passes that
/// materialize aggregate paths. The iterative traversal keeps malformed,
/// impractically deep, or exponentially branching source bounded.
pub(crate) fn validate_aggregate_structure(
    structs: &[TypedStruct],
) -> Result<(), AggregateLayoutError> {
    let indices = structs
        .iter()
        .enumerate()
        .map(|(index, definition)| (definition.name.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut edges = vec![Vec::new(); structs.len()];
    let mut incoming = vec![0usize; structs.len()];
    for (index, definition) in structs.iter().enumerate() {
        for field in aggregate_fields(definition) {
            let nested = match field.ty {
                TypedFieldType::Struct => field.struct_name.as_deref(),
                TypedFieldType::Array(_) => field.array_elem_struct.as_deref(),
                TypedFieldType::Scalar(_) | TypedFieldType::Tuple(_) => None,
            };
            let Some(&nested) = nested.and_then(|name| indices.get(name)) else {
                continue;
            };
            edges[index].push(nested);
            incoming[nested] += 1;
        }
    }

    let mut states = vec![0u8; structs.len()];
    for root in 0..structs.len() {
        if states[root] != 0 {
            continue;
        }
        states[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some((current, next_edge)) = stack.last_mut() {
            if let Some(&nested) = edges[*current].get(*next_edge) {
                *next_edge += 1;
                match states[nested] {
                    0 => {
                        states[nested] = 1;
                        stack.push((nested, 0));
                    }
                    1 => {
                        let start = stack
                            .iter()
                            .position(|(candidate, _)| *candidate == nested)
                            .unwrap_or(0);
                        let mut cycle = stack[start..]
                            .iter()
                            .map(|(index, _)| structs[*index].name.clone())
                            .collect::<Vec<_>>();
                        cycle.push(structs[nested].name.clone());
                        return Err(AggregateLayoutError::RecursiveAggregate { cycle });
                    }
                    _ => {}
                }
            } else {
                states[*current] = 2;
                stack.pop();
            }
        }
    }

    let mut queue = std::collections::VecDeque::new();
    let mut depths = vec![1usize; structs.len()];
    let mut topological = Vec::with_capacity(structs.len());
    for (index, count) in incoming.iter().enumerate() {
        if *count == 0 {
            queue.push_back(index);
        }
    }
    while let Some(current) = queue.pop_front() {
        topological.push(current);
        for &nested in &edges[current] {
            let depth = depths[current] + 1;
            if depth > MAX_AGGREGATE_NESTING {
                return Err(AggregateLayoutError::NestingTooDeep {
                    struct_name: structs[nested].name.clone(),
                    depth,
                    maximum: MAX_AGGREGATE_NESTING,
                });
            }
            depths[nested] = depths[nested].max(depth);
            incoming[nested] -= 1;
            if incoming[nested] == 0 {
                queue.push_back(nested);
            }
        }
    }

    let mut shape_nodes = vec![0usize; structs.len()];
    let mut total_nodes = 0usize;
    for &current in topological.iter().rev() {
        let mut count = 1usize;
        for field in aggregate_fields(&structs[current]) {
            let field_count = match &field.ty {
                TypedFieldType::Scalar(_) => 1,
                TypedFieldType::Tuple(elements) => 1usize.saturating_add(elements.len()),
                TypedFieldType::Struct => field
                    .struct_name
                    .as_deref()
                    .and_then(|name| indices.get(name))
                    .map_or(1, |index| shape_nodes[*index]),
                TypedFieldType::Array(_) => 1usize.saturating_add(
                    field
                        .array_elem_struct
                        .as_deref()
                        .and_then(|name| indices.get(name))
                        .map_or(1, |index| shape_nodes[*index]),
                ),
            };
            count = count.saturating_add(field_count);
        }
        shape_nodes[current] = count;
        total_nodes = total_nodes.saturating_add(count);
        if total_nodes > MAX_AGGREGATE_LAYOUT_NODES {
            return Err(AggregateLayoutError::LayoutsTooLarge {
                struct_name: structs[current].name.clone(),
                count: total_nodes,
                maximum: MAX_AGGREGATE_LAYOUT_NODES,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum VisitState {
    Visiting,
    Built,
}

struct LayoutBuilder {
    definitions: HashMap<String, TypedStruct>,
    ids: HashMap<String, AggregateLayoutId>,
    sorted_names: Vec<String>,
    states: HashMap<String, VisitState>,
    stack: Vec<String>,
    built: HashMap<String, AggregateLayout>,
}

impl LayoutBuilder {
    fn new(structs: &[TypedStruct]) -> Result<Self, Vec<AggregateLayoutError>> {
        if u32::try_from(structs.len()).is_err() {
            return Err(vec![AggregateLayoutError::TooManyLayouts {
                count: structs.len(),
            }]);
        }

        let mut definitions = HashMap::with_capacity(structs.len());
        let mut errors = Vec::new();
        for definition in structs {
            if definitions
                .insert(definition.name.clone(), definition.clone())
                .is_some()
            {
                errors.push(AggregateLayoutError::DuplicateStruct {
                    struct_name: definition.name.clone(),
                });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let mut sorted_names = definitions.keys().cloned().collect::<Vec<_>>();
        sorted_names.sort();
        let ids = sorted_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.clone(),
                    AggregateLayoutId(u32::try_from(index).expect("layout count checked above")),
                )
            })
            .collect();

        Ok(Self {
            definitions,
            ids,
            sorted_names,
            states: HashMap::new(),
            stack: Vec::new(),
            built: HashMap::new(),
        })
    }

    fn build(mut self) -> Result<AggregateLayoutTable, Vec<AggregateLayoutError>> {
        let mut errors = Vec::new();
        for name in self.sorted_names.clone() {
            if let Err(error) = self.build_one(&name) {
                errors.push(error);
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let layouts = self
            .sorted_names
            .iter()
            .map(|name| {
                self.built
                    .remove(name)
                    .expect("every sorted aggregate was built")
            })
            .collect();
        Ok(AggregateLayoutTable {
            layouts,
            ids_by_struct_name: self.ids,
        })
    }

    fn build_one(&mut self, struct_name: &str) -> Result<(), AggregateLayoutError> {
        match self.states.get(struct_name) {
            Some(VisitState::Built) => return Ok(()),
            Some(VisitState::Visiting) => {
                let cycle_start = self
                    .stack
                    .iter()
                    .position(|name| name == struct_name)
                    .unwrap_or(0);
                let mut cycle = self.stack[cycle_start..].to_vec();
                cycle.push(struct_name.to_owned());
                return Err(AggregateLayoutError::RecursiveAggregate { cycle });
            }
            None => {}
        }

        let Some(definition) = self.definitions.get(struct_name).cloned() else {
            // Nested references are reported with their owning field context.
            unreachable!("top-level layout builder only visits known definitions");
        };
        self.states
            .insert(struct_name.to_owned(), VisitState::Visiting);
        self.stack.push(struct_name.to_owned());

        let result = self.build_fields(&definition);
        self.stack.pop();

        match result {
            Ok((mut leaves, scalar_width, shape)) => {
                for (index, leaf) in leaves.iter_mut().enumerate() {
                    leaf.id = AggregateLeafId(
                        u32::try_from(index).expect("aggregate shape size checked above"),
                    );
                }
                let layout = AggregateLayout {
                    id: self.ids[struct_name],
                    struct_name: struct_name.to_owned(),
                    leaves,
                    scalar_width,
                    shape,
                };
                self.states
                    .insert(struct_name.to_owned(), VisitState::Built);
                self.built.insert(struct_name.to_owned(), layout);
                Ok(())
            }
            Err(error) => {
                self.states.remove(struct_name);
                Err(error)
            }
        }
    }

    fn build_fields(
        &mut self,
        definition: &TypedStruct,
    ) -> Result<(Vec<AggregateLeafLayout>, usize, PayloadType), AggregateLayoutError> {
        let mut fields = Vec::new();
        let mut seen = HashSet::new();
        // Processor lowering also uses TypedStruct for structural parameter
        // maps whose dotted names are already-flattened access paths, not
        // nominal aggregate fields.
        for field in aggregate_fields(definition) {
            if !seen.insert(field.name.clone()) {
                return Err(AggregateLayoutError::DuplicateField {
                    struct_name: definition.name.clone(),
                    field_name: field.name.clone(),
                });
            }
            fields.push(PayloadField {
                name: field.name.clone(),
                ty: self.build_field(definition, field)?,
                default: None,
            });
        }
        let shape = PayloadType::Struct {
            name: definition.name.clone(),
            fields,
        };
        let planned = shape
            .leaves()
            .map_err(|error| AggregateLayoutError::MalformedField {
                struct_name: definition.name.clone(),
                field_name: String::new(),
                reason: error.to_string(),
            })?;
        let mut leaves = Vec::with_capacity(planned.len());
        let mut scalar_width = 0usize;
        for tensor in planned {
            let path = self.resolve_path(definition, &tensor.path);
            let mut leaf = self.scalar_leaf(
                path,
                tensor.path,
                primitive_encoding(tensor.encoding),
                tensor.shape,
                definition,
            )?;
            leaf.scalar_offset = scalar_width;
            scalar_width = scalar_width
                .checked_add(leaf.tensor.element_count)
                .ok_or_else(|| AggregateLayoutError::SizeOverflow {
                    struct_name: definition.name.clone(),
                    field_path: leaf.storage_path.clone(),
                    shape: leaf.tensor.shape.clone(),
                })?;
            leaves.push(leaf);
        }
        Ok((leaves, scalar_width, shape))
    }

    fn build_field(
        &mut self,
        owner: &TypedStruct,
        field: &TypedStructField,
    ) -> Result<PayloadType, AggregateLayoutError> {
        let scalar = |ty| PayloadType::Scalar {
            encoding: scalar_encoding(ty),
            integer_range: field.integer_range.map(|range| {
                let scalar = match range.ty {
                    PrimitiveType::I32 => "i32",
                    PrimitiveType::I64 => "i64",
                    _ => unreachable!("integer domain"),
                };
                onda_processor_abi::IntegerRangeMetadata {
                    min: onda_processor_abi::IntegerRangeEndpoint {
                        scalar: scalar.to_owned(),
                        value: range.min.to_string(),
                    },
                    max: onda_processor_abi::IntegerRangeEndpoint {
                        scalar: scalar.to_owned(),
                        value: range.max.to_string(),
                    },
                    mode: if range.wrap { "wrap" } else { "clamp" }.to_owned(),
                }
            }),
        };
        match &field.ty {
            TypedFieldType::Scalar(ty) => {
                self.require_no_aggregate_metadata(owner, field)?;
                Ok(scalar(*ty))
            }
            TypedFieldType::Tuple(types) => {
                self.require_no_aggregate_metadata(owner, field)?;
                Ok(PayloadType::Tuple {
                    elements: types.iter().copied().map(scalar).collect(),
                })
            }
            TypedFieldType::Struct => {
                if field.array_elem_ty.is_some() || field.array_elem_struct.is_some() {
                    return Err(self.malformed(owner, field, "struct field has array metadata"));
                }
                let Some(name) = field.struct_name.as_deref() else {
                    return Err(self.malformed(owner, field, "struct target is missing"));
                };
                self.nested_shape(owner, field, name)
            }
            TypedFieldType::Array(len) => {
                if field.struct_name.is_some() {
                    return Err(self.malformed(owner, field, "array field has struct metadata"));
                }
                let element = match (field.array_elem_ty, field.array_elem_struct.as_deref()) {
                    (Some(ty), None) => scalar(ty),
                    (None, Some(name)) => self.nested_shape(owner, field, name)?,
                    (None, None) => {
                        return Err(self.malformed(owner, field, "array element type is missing"))
                    }
                    (Some(_), Some(_)) => {
                        return Err(self.malformed(
                            owner,
                            field,
                            "array has both primitive and struct element types",
                        ))
                    }
                };
                Ok(PayloadType::Array {
                    element: Box::new(element),
                    len: *len,
                })
            }
        }
    }

    fn nested_shape(
        &mut self,
        owner: &TypedStruct,
        field: &TypedStructField,
        name: &str,
    ) -> Result<PayloadType, AggregateLayoutError> {
        if !self.ids.contains_key(name) {
            return Err(AggregateLayoutError::UnknownStruct {
                struct_name: owner.name.clone(),
                field_path: field.name.clone(),
                referenced_struct: name.to_owned(),
            });
        }
        self.build_one(name)?;
        Ok(self.built[name].shape.clone())
    }

    // Restore semantic identities along a path emitted by the common planner.
    // This only annotates a leaf; traversal order and tensor axes have one owner.
    fn resolve_path(&self, definition: &TypedStruct, path: &str) -> Vec<AggregatePathComponent> {
        let mut owner = definition;
        let mut tuple = false;
        path.split('.')
            .map(|name| {
                if tuple {
                    return AggregatePathComponent::TupleElement {
                        index: name.strip_prefix("__").unwrap().parse().unwrap(),
                    };
                }
                let field = owner
                    .fields
                    .iter()
                    .find(|field| field.name == name)
                    .expect("resolved field path");
                let nested = field
                    .struct_name
                    .as_ref()
                    .or(field.array_elem_struct.as_ref());
                let aggregate = nested.map(|name| self.ids[name]);
                let extent = match field.ty {
                    TypedFieldType::Array(len) => Some(len),
                    _ => None,
                };
                tuple = matches!(field.ty, TypedFieldType::Tuple(_));
                if let Some(name) = nested {
                    owner = &self.definitions[name];
                }
                AggregatePathComponent::Field {
                    name: name.to_owned(),
                    aggregate,
                    extent,
                }
            })
            .collect()
    }

    fn scalar_leaf(
        &self,
        path: Vec<AggregatePathComponent>,
        storage_path: String,
        scalar: PrimitiveType,
        shape: Vec<usize>,
        owner: &TypedStruct,
    ) -> Result<AggregateLeafLayout, AggregateLayoutError> {
        let tensor = AggregateTensorLayout::from_shape(shape.clone()).map_err(|_| {
            AggregateLayoutError::SizeOverflow {
                struct_name: owner.name.clone(),
                field_path: storage_path.clone(),
                shape,
            }
        })?;
        Ok(AggregateLeafLayout {
            id: AggregateLeafId(0),
            path,
            storage_path,
            scalar,
            tensor,
            scalar_offset: 0,
        })
    }

    fn require_no_aggregate_metadata(
        &self,
        owner: &TypedStruct,
        field: &TypedStructField,
    ) -> Result<(), AggregateLayoutError> {
        if field.struct_name.is_some()
            || field.array_elem_ty.is_some()
            || field.array_elem_struct.is_some()
        {
            Err(self.malformed(owner, field, "scalar/tuple field has aggregate metadata"))
        } else {
            Ok(())
        }
    }

    fn malformed(
        &self,
        owner: &TypedStruct,
        field: &TypedStructField,
        reason: impl Into<String>,
    ) -> AggregateLayoutError {
        AggregateLayoutError::MalformedField {
            struct_name: owner.name.clone(),
            field_name: field.name.clone(),
            reason: reason.into(),
        }
    }
}

pub(crate) fn scalar_encoding(ty: PrimitiveType) -> ScalarEncoding {
    match ty {
        PrimitiveType::F32 => ScalarEncoding::F32,
        PrimitiveType::F64 => ScalarEncoding::F64,
        PrimitiveType::I32 => ScalarEncoding::I32,
        PrimitiveType::I64 => ScalarEncoding::I64,
        PrimitiveType::Bool => ScalarEncoding::Bool,
    }
}

pub(crate) fn primitive_encoding(ty: ScalarEncoding) -> PrimitiveType {
    match ty {
        ScalarEncoding::F32 => PrimitiveType::F32,
        ScalarEncoding::F64 => PrimitiveType::F64,
        ScalarEncoding::I32 => PrimitiveType::I32,
        ScalarEncoding::I64 => PrimitiveType::I64,
        ScalarEncoding::Bool => PrimitiveType::Bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onda_frontend::parse_program;

    fn scalar_field(name: &str, scalar: PrimitiveType) -> TypedStructField {
        TypedStructField {
            name: name.to_owned(),
            ty: TypedFieldType::Scalar(scalar),
            default: None,
            integer_range: None,
            struct_name: None,
            array_elem_ty: None,
            array_elem_struct: None,
        }
    }

    fn struct_field(name: &str, target: &str) -> TypedStructField {
        TypedStructField {
            name: name.to_owned(),
            ty: TypedFieldType::Struct,
            default: None,
            integer_range: None,
            struct_name: Some(target.to_owned()),
            array_elem_ty: None,
            array_elem_struct: None,
        }
    }

    fn struct_array_field(name: &str, target: &str, len: usize) -> TypedStructField {
        TypedStructField {
            name: name.to_owned(),
            ty: TypedFieldType::Array(len),
            default: None,
            integer_range: None,
            struct_name: None,
            array_elem_ty: None,
            array_elem_struct: Some(target.to_owned()),
        }
    }

    #[test]
    fn ids_do_not_depend_on_declaration_order() {
        let zed = TypedStruct {
            name: "Zed".to_owned(),
            fields: vec![scalar_field("value", PrimitiveType::F32)],
        };
        let alpha = TypedStruct {
            name: "Alpha".to_owned(),
            fields: vec![scalar_field("value", PrimitiveType::F64)],
        };
        let forward = AggregateLayoutTable::build(&[zed.clone(), alpha.clone()]).unwrap();
        let reverse = AggregateLayoutTable::build(&[alpha, zed]).unwrap();

        assert_eq!(
            forward.id_for_struct("Alpha"),
            reverse.id_for_struct("Alpha")
        );
        assert_eq!(forward.id_for_struct("Zed"), reverse.id_for_struct("Zed"));
        assert_eq!(forward.id_for_struct("Alpha").unwrap().as_u32(), 0);
        assert_eq!(forward.id_for_struct("Zed").unwrap().as_u32(), 1);
    }

    #[test]
    fn reports_direct_and_array_recursion() {
        let direct = TypedStruct {
            name: "Direct".to_owned(),
            fields: vec![struct_field("next", "Direct")],
        };
        let direct_error = AggregateLayoutTable::build(&[direct]).unwrap_err();
        assert!(matches!(
            &direct_error[0],
            AggregateLayoutError::RecursiveAggregate { cycle }
                if cycle == &["Direct", "Direct"]
        ));

        let a = TypedStruct {
            name: "A".to_owned(),
            fields: vec![struct_array_field("bs", "B", 2)],
        };
        let b = TypedStruct {
            name: "B".to_owned(),
            fields: vec![struct_array_field("as", "A", 3)],
        };
        let errors = AggregateLayoutTable::build(&[a, b]).unwrap_err();
        assert!(errors.iter().any(|error| matches!(
            error,
            AggregateLayoutError::RecursiveAggregate { cycle }
                if cycle == &["A", "B", "A"] || cycle == &["B", "A", "B"]
        )));
    }

    #[test]
    fn rejects_excessive_nesting_without_recursive_traversal() {
        let mut structs = (0..=MAX_AGGREGATE_NESTING)
            .map(|index| TypedStruct {
                name: format!("S{index}"),
                fields: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (index, definition) in structs.iter_mut().take(MAX_AGGREGATE_NESTING).enumerate() {
            definition.fields = vec![struct_field("next", &format!("S{}", index + 1))];
        }
        structs[MAX_AGGREGATE_NESTING].fields = vec![scalar_field("value", PrimitiveType::F32)];

        assert!(matches!(
            validate_aggregate_structure(&structs),
            Err(AggregateLayoutError::NestingTooDeep {
                depth,
                maximum: MAX_AGGREGATE_NESTING,
                ..
            }) if depth == MAX_AGGREGATE_NESTING + 1
        ));
        assert!(validate_aggregate_structure(&structs[1..]).is_ok());
    }

    #[test]
    fn rejects_empty_struct_branching_before_materializing_shapes() {
        let mut structs = vec![TypedStruct {
            name: "S0".to_owned(),
            fields: Vec::new(),
        }];
        let mut shape_nodes = 1usize;
        let mut total_nodes = shape_nodes;
        for level in 1.. {
            let nested = format!("S{}", level - 1);
            structs.push(TypedStruct {
                name: format!("S{level}"),
                fields: vec![
                    struct_field("left", &nested),
                    struct_field("right", &nested),
                ],
            });
            shape_nodes = 1 + 2 * shape_nodes;
            total_nodes += shape_nodes;
            if total_nodes > MAX_AGGREGATE_LAYOUT_NODES {
                break;
            }
        }

        assert!(matches!(
            validate_aggregate_structure(&structs),
            Err(AggregateLayoutError::LayoutsTooLarge {
                count,
                maximum: MAX_AGGREGATE_LAYOUT_NODES,
                ..
            }) if count == total_nodes
        ));
    }

    #[test]
    fn rejects_nested_shape_overflow() {
        let leaf = TypedStruct {
            name: "Leaf".to_owned(),
            fields: vec![scalar_field("value", PrimitiveType::F32)],
        };
        let inner = TypedStruct {
            name: "Inner".to_owned(),
            fields: vec![struct_array_field("leaves", "Leaf", 2)],
        };
        let outer = TypedStruct {
            name: "Outer".to_owned(),
            fields: vec![struct_array_field("inners", "Inner", usize::MAX)],
        };
        let errors = AggregateLayoutTable::build(&[leaf, inner, outer]).unwrap_err();
        assert!(errors.iter().any(|error| matches!(
            error,
            AggregateLayoutError::SizeOverflow {
                struct_name,
                field_path,
                shape,
            } if struct_name == "Outer"
                && field_path == "inners.leaves.value"
                && shape == &[usize::MAX, 2]
        )));
    }

    #[test]
    fn typed_program_exposes_convolution_style_nested_layouts() {
        let source = r#"
struct Complex:
  real: f32
  imag: f32

struct FFT:
  bins: Complex[8]
  twiddles: Complex[4]
  bitrev: i32[8]

struct Convolver:
  plans: FFT[3]
  history: Complex[6]
  counters: (i32, i64)

sample:
  out1 = 0.0
"#;
        let parsed = parse_program(source).expect("nested aggregate source should parse");
        let typed = crate::analyze(parsed).expect("nested aggregate source should analyze");

        let complex_id = typed
            .aggregate_layouts
            .id_for_struct("Complex")
            .expect("Complex layout ID");
        let fft_id = typed
            .aggregate_layouts
            .id_for_struct("FFT")
            .expect("FFT layout ID");
        let convolver = typed
            .aggregate_layouts
            .layout_for_struct("Convolver")
            .expect("Convolver layout");

        let real = convolver
            .leaf_by_storage_path("plans.bins.real")
            .expect("nested FFT real leaf");
        assert_eq!(real.scalar, PrimitiveType::F32);
        assert_eq!(real.tensor.shape, [3, 8]);
        assert_eq!(real.tensor.strides, [8, 1]);
        assert_eq!(real.tensor.element_count, 24);
        assert!(matches!(
            &real.path[..2],
            [
                AggregatePathComponent::Field {
                    aggregate: Some(id),
                    extent: Some(3),
                    ..
                },
                AggregatePathComponent::Field {
                    aggregate: Some(nested_id),
                    extent: Some(8),
                    ..
                }
            ] if *id == fft_id && *nested_id == complex_id
        ));

        let root_storage = real
            .storage_for_outer_extent(2)
            .expect("root storage extent should fit");
        assert_eq!(root_storage.shape, [2, 3, 8]);
        assert_eq!(root_storage.strides, [24, 8, 1]);
        assert_eq!(root_storage.element_count, 48);

        let counter = convolver
            .leaf_by_storage_path("counters.__1")
            .expect("tuple leaf");
        assert_eq!(counter.scalar, PrimitiveType::I64);
        assert!(matches!(
            counter.path.last(),
            Some(AggregatePathComponent::TupleElement { index: 1 })
        ));
        assert_eq!(convolver.scalar_width, 110);
    }
}
