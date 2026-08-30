use std::collections::{HashMap, HashSet};
use std::fmt;

use onda_frontend::PrimitiveType;

use crate::{TypedFieldType, TypedStruct, TypedStructField};

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
}

impl AggregateLayout {
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
        LayoutBuilder::new(structs)?.build()
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
    TooManyLeaves {
        struct_name: String,
        count: usize,
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
            Self::TooManyLeaves { struct_name, count } => write!(
                f,
                "aggregate '{struct_name}' has {count} leaves, exceeding the u32 ID space"
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

    fn build_one(&mut self, struct_name: &str) -> Result<AggregateLayout, AggregateLayoutError> {
        match self.states.get(struct_name) {
            Some(VisitState::Built) => {
                return Ok(self
                    .built
                    .get(struct_name)
                    .expect("built state has a layout")
                    .clone());
            }
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
            Ok((mut leaves, scalar_width)) => {
                if u32::try_from(leaves.len()).is_err() {
                    self.states.remove(struct_name);
                    return Err(AggregateLayoutError::TooManyLeaves {
                        struct_name: struct_name.to_owned(),
                        count: leaves.len(),
                    });
                }
                for (index, leaf) in leaves.iter_mut().enumerate() {
                    leaf.id = AggregateLeafId(
                        u32::try_from(index).expect("aggregate leaf count checked above"),
                    );
                }
                let layout = AggregateLayout {
                    id: self.ids[struct_name],
                    struct_name: struct_name.to_owned(),
                    leaves,
                    scalar_width,
                };
                self.states
                    .insert(struct_name.to_owned(), VisitState::Built);
                self.built.insert(struct_name.to_owned(), layout.clone());
                Ok(layout)
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
    ) -> Result<(Vec<AggregateLeafLayout>, usize), AggregateLayoutError> {
        // Nested struct fields are also present as dotted compatibility entries
        // in `TypedStruct::fields`. Only undotted entries are source-level
        // fields; recursion below resolves their descendants canonically.
        let fields = definition
            .fields
            .iter()
            .filter(|field| !field.name.contains('.'))
            .cloned()
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut leaves = Vec::new();
        let mut scalar_width = 0usize;

        for field in fields {
            if !seen.insert(field.name.clone()) {
                return Err(AggregateLayoutError::DuplicateField {
                    struct_name: definition.name.clone(),
                    field_name: field.name,
                });
            }
            let mut field_leaves = self.build_field(definition, &field)?;
            for leaf in &mut field_leaves {
                leaf.scalar_offset = scalar_width;
                scalar_width = scalar_width
                    .checked_add(leaf.tensor.element_count)
                    .ok_or_else(|| AggregateLayoutError::SizeOverflow {
                        struct_name: definition.name.clone(),
                        field_path: leaf.storage_path.clone(),
                        shape: leaf.tensor.shape.clone(),
                    })?;
            }
            leaves.extend(field_leaves);
        }
        Ok((leaves, scalar_width))
    }

    fn build_field(
        &mut self,
        owner: &TypedStruct,
        field: &TypedStructField,
    ) -> Result<Vec<AggregateLeafLayout>, AggregateLayoutError> {
        match &field.ty {
            TypedFieldType::Scalar(scalar) => {
                self.require_no_aggregate_metadata(owner, field)?;
                Ok(vec![self.scalar_leaf(
                    vec![AggregatePathComponent::Field {
                        name: field.name.clone(),
                        aggregate: None,
                        extent: None,
                    }],
                    field.name.clone(),
                    *scalar,
                    Vec::new(),
                    owner,
                )?])
            }
            TypedFieldType::Tuple(scalars) => {
                self.require_no_aggregate_metadata(owner, field)?;
                let mut leaves = Vec::with_capacity(scalars.len());
                for (index, scalar) in scalars.iter().copied().enumerate() {
                    leaves.push(self.scalar_leaf(
                        vec![
                            AggregatePathComponent::Field {
                                name: field.name.clone(),
                                aggregate: None,
                                extent: None,
                            },
                            AggregatePathComponent::TupleElement { index },
                        ],
                        format!("{}.__{index}", field.name),
                        scalar,
                        Vec::new(),
                        owner,
                    )?);
                }
                Ok(leaves)
            }
            TypedFieldType::Struct => {
                if field.array_elem_ty.is_some() || field.array_elem_struct.is_some() {
                    return Err(self.malformed(owner, field, "struct field has array metadata"));
                }
                let Some(nested_name) = field.struct_name.as_deref() else {
                    return Err(self.malformed(owner, field, "struct target is missing"));
                };
                self.nested_leaves(owner, field, nested_name, None)
            }
            TypedFieldType::Array(extent) => {
                if field.struct_name.is_some() {
                    return Err(self.malformed(owner, field, "array field has struct metadata"));
                }
                match (field.array_elem_ty, field.array_elem_struct.as_deref()) {
                    (Some(scalar), None) => Ok(vec![self.scalar_leaf(
                        vec![AggregatePathComponent::Field {
                            name: field.name.clone(),
                            aggregate: None,
                            extent: Some(*extent),
                        }],
                        field.name.clone(),
                        scalar,
                        vec![*extent],
                        owner,
                    )?]),
                    (None, Some(nested_name)) => {
                        self.nested_leaves(owner, field, nested_name, Some(*extent))
                    }
                    (None, None) => {
                        Err(self.malformed(owner, field, "array element type is missing"))
                    }
                    (Some(_), Some(_)) => Err(self.malformed(
                        owner,
                        field,
                        "array has both primitive and struct element types",
                    )),
                }
            }
        }
    }

    fn nested_leaves(
        &mut self,
        owner: &TypedStruct,
        field: &TypedStructField,
        nested_name: &str,
        extent: Option<usize>,
    ) -> Result<Vec<AggregateLeafLayout>, AggregateLayoutError> {
        let Some(nested_id) = self.ids.get(nested_name).copied() else {
            return Err(AggregateLayoutError::UnknownStruct {
                struct_name: owner.name.clone(),
                field_path: field.name.clone(),
                referenced_struct: nested_name.to_owned(),
            });
        };
        let nested = self.build_one(nested_name)?;
        let mut leaves = Vec::with_capacity(nested.leaves.len());
        for nested_leaf in nested.leaves {
            let mut path = Vec::with_capacity(nested_leaf.path.len() + 1);
            path.push(AggregatePathComponent::Field {
                name: field.name.clone(),
                aggregate: Some(nested_id),
                extent,
            });
            path.extend(nested_leaf.path);

            let mut shape =
                Vec::with_capacity(nested_leaf.tensor.shape.len() + usize::from(extent.is_some()));
            if let Some(extent) = extent {
                shape.push(extent);
            }
            shape.extend_from_slice(&nested_leaf.tensor.shape);
            leaves.push(self.scalar_leaf(
                path,
                format!("{}.{}", field.name, nested_leaf.storage_path),
                nested_leaf.scalar,
                shape,
                owner,
            )?);
        }
        Ok(leaves)
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
