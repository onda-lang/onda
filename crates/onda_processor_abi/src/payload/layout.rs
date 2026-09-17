use super::*;
use std::collections::HashSet;
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct PayloadTensor {
    pub path: String,
    /// Index in the executable message parameter list.
    pub parameter: usize,
    pub length_prefix: bool,
    pub encoding: ScalarEncoding,
    /// Fixed axes below the parameter's optional runtime axis, outermost first.
    pub shape: Vec<usize>,
    pub elements: usize,
    pub domain: Option<IntegerDomain>,
}

#[derive(Debug, Clone, Copy)]
pub struct IntegerDomain {
    pub min: i64,
    pub max: i64,
    pub wrap: bool,
}

#[derive(Debug, Clone)]
pub struct ParameterPlan {
    pub dynamic: bool,
    /// Struct slices use one shared logical length for all of their leaf tensors.
    pub length_parameter: Option<usize>,
    pub tensors: Range<usize>,
}

#[derive(Debug, Clone)]
pub struct PayloadPlan {
    pub(super) schema: PayloadSchema,
    pub(super) params: Vec<ParameterPlan>,
    pub(super) tensors: Vec<PayloadTensor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorRegion {
    pub tensor: usize,
    pub logical_len: usize,
    pub elements: usize,
    pub wire: Range<usize>,
    pub workspace: Range<usize>,
}

pub(super) enum Region {
    Length {
        wire: Range<usize>,
        workspace: Range<usize>,
    },
    Tensor(TensorRegion),
}

impl PayloadPlan {
    /// Prepare once, outside dispatch. Arrays remain tensor axes, so plan size
    /// depends on type shape, never on the number of array elements.
    pub fn new(schema: &PayloadSchema) -> Result<Self, PayloadError> {
        unique_fields(&schema.params)?;
        validate_defaults_and_extents(&schema.params)?;
        let mut plan = Self {
            schema: schema.clone(),
            params: Vec::new(),
            tensors: Vec::new(),
        };
        let mut abi_parameter = 0;
        for field in &schema.params {
            let start = plan.tensors.len();
            let (ty, dynamic) = match &field.ty {
                PayloadType::Slice { element } => (element.as_ref(), true),
                ty => (ty, false),
            };
            if dynamic && !array_element(ty) {
                return Err(PayloadError::InvalidSchema);
            }
            let length_parameter =
                (dynamic && matches!(ty, PayloadType::Struct { .. })).then(|| {
                    let index = abi_parameter;
                    abi_parameter += 1;
                    index
                });
            let leaves = ty.leaves()?;
            if leaves.is_empty() {
                return Err(PayloadError::InvalidSchema);
            }
            for leaf in leaves {
                let elements = leaf
                    .shape
                    .iter()
                    .try_fold(1usize, |size, extent| checked_mul(size, *extent))?;
                let path = if leaf.path.is_empty() {
                    field.name.clone()
                } else {
                    format!("{}.{}", field.name, leaf.path)
                };
                plan.tensors.push(PayloadTensor {
                    path,
                    parameter: abi_parameter,
                    length_prefix: dynamic && length_parameter.is_none(),
                    encoding: leaf.encoding,
                    shape: leaf.shape,
                    elements,
                    domain: leaf
                        .integer_range
                        .map(|range| prepare_domain(leaf.encoding, range))
                        .transpose()?,
                });
                abi_parameter += 1;
            }
            plan.params.push(ParameterPlan {
                dynamic,
                length_parameter,
                tensors: start..plan.tensors.len(),
            });
        }
        // Reject overflowing fixed shapes even when no input is supplied.
        plan.sizes(&vec![0; plan.dynamic_parameters()])?;
        Ok(plan)
    }

    pub fn schema(&self) -> &PayloadSchema {
        &self.schema
    }

    pub fn abi_parameter_count(&self) -> usize {
        self.tensors.len()
            + self
                .params
                .iter()
                .filter(|p| p.length_parameter.is_some())
                .count()
    }

    pub fn tensors(&self) -> &[PayloadTensor] {
        &self.tensors
    }
    pub fn parameters(&self) -> &[ParameterPlan] {
        &self.params
    }
    pub fn dynamic_parameters(&self) -> usize {
        self.params.iter().filter(|param| param.dynamic).count()
    }

    pub fn fixed_wire_size(&self) -> Option<usize> {
        (self.dynamic_parameters() == 0).then(|| self.minimum_sizes().0)
    }

    /// Wire and workspace sizes with every dynamic slice at length zero.
    pub fn minimum_sizes(&self) -> (usize, usize) {
        self.sizes(&vec![0; self.dynamic_parameters()])
            .expect("validated payload schema")
    }

    /// Exact fixed size, or at least `dynamic_capacity` for a dynamic schema.
    pub fn wire_capacity(&self, dynamic_capacity: usize) -> usize {
        self.fixed_wire_size()
            .unwrap_or_else(|| self.minimum_sizes().0.max(dynamic_capacity))
    }

    /// A workspace capacity sufficient for any valid payload up to `wire_capacity` bytes.
    pub fn workspace_capacity_for_wire_capacity(
        &self,
        wire_capacity: usize,
    ) -> Result<usize, PayloadError> {
        let mut capacity = checked_add(
            wire_capacity,
            self.dynamic_parameters()
                .checked_mul(3)
                .ok_or(PayloadError::Overflow)?,
        )?;
        for tensor in &self.tensors {
            capacity = checked_add(capacity, tensor.encoding.byte_size() - 1)?;
        }
        Ok(capacity)
    }

    /// Sizes depend only on shape and lengths, not values or normalization.
    pub fn sizes(&self, lengths: &[i32]) -> Result<(usize, usize), PayloadError> {
        let mut lengths = lengths.iter();
        let sizes = self.walk(
            |_, _| {
                let len = *lengths.next().ok_or(PayloadError::InvalidLengths)?;
                usize::try_from(len).map_err(|_| PayloadError::NegativeLength)
            },
            |_| {},
        )?;
        if lengths.next().is_some() {
            return Err(PayloadError::InvalidLengths);
        }
        Ok(sizes)
    }

    /// Validate complete wire boundaries before any handler or processor-state mutation.
    pub fn required_workspace(&self, input: &[u8]) -> Result<usize, PayloadError> {
        let (wire, workspace) = self.walk(|offset, _| read_length(input, offset), |_| {})?;
        if wire > input.len() {
            return Err(PayloadError::Truncated);
        }
        if wire < input.len() {
            return Err(PayloadError::TrailingBytes);
        }
        Ok(workspace)
    }

    pub(super) fn walk(
        &self,
        mut length: impl FnMut(usize, usize) -> Result<usize, PayloadError>,
        mut visit: impl FnMut(Region),
    ) -> Result<(usize, usize), PayloadError> {
        let mut wire = 0;
        let mut workspace = 0;
        for param in &self.params {
            let len = if param.dynamic {
                let prepared = aligned(workspace, 4)?;
                let len = length(wire, prepared)?;
                visit(Region::Length {
                    wire: wire..checked_add(wire, 4)?,
                    workspace: prepared..checked_add(prepared, 4)?,
                });
                wire = checked_add(wire, 4)?;
                workspace = checked_add(prepared, 4)?;
                len
            } else {
                1
            };
            for index in param.tensors.clone() {
                let leaf = &self.tensors[index];
                let elements = checked_mul(leaf.elements, len)?;
                let bytes = checked_mul(elements, leaf.encoding.byte_size())?;
                let prepared = aligned(workspace, leaf.encoding.byte_size())?;
                let region = TensorRegion {
                    tensor: index,
                    logical_len: len,
                    elements,
                    wire: wire..checked_add(wire, bytes)?,
                    workspace: prepared..checked_add(prepared, bytes)?,
                };
                wire = region.wire.end;
                workspace = region.workspace.end;
                visit(Region::Tensor(region));
            }
        }
        Ok((wire, workspace))
    }
}

pub(super) fn read_length(bytes: &[u8], offset: usize) -> Result<usize, PayloadError> {
    let prefix = bytes
        .get(offset..checked_add(offset, 4)?)
        .ok_or(PayloadError::Truncated)?;
    usize::try_from(i32::from_le_bytes(prefix.try_into().unwrap()))
        .map_err(|_| PayloadError::NegativeLength)
}

pub(super) fn unique_fields(fields: &[PayloadField]) -> Result<(), PayloadError> {
    let mut names = HashSet::new();
    for field in fields {
        if field.name.is_empty() || !names.insert(&field.name) {
            return Err(PayloadError::InvalidSchema);
        }
    }
    Ok(())
}
pub(super) fn array_element(ty: &PayloadType) -> bool {
    matches!(ty, PayloadType::Scalar { .. } | PayloadType::Struct { .. })
}
fn checked_add(a: usize, b: usize) -> Result<usize, PayloadError> {
    a.checked_add(b)
        .filter(|size| *size <= i32::MAX as usize)
        .ok_or(PayloadError::Overflow)
}
fn checked_mul(a: usize, b: usize) -> Result<usize, PayloadError> {
    a.checked_mul(b)
        .filter(|size| *size <= i32::MAX as usize)
        .ok_or(PayloadError::Overflow)
}
fn aligned(offset: usize, alignment: usize) -> Result<usize, PayloadError> {
    Ok(checked_add(offset, alignment - 1)? & !(alignment - 1))
}

fn prepare_domain(
    encoding: ScalarEncoding,
    range: &crate::IntegerRangeMetadata,
) -> Result<IntegerDomain, PayloadError> {
    let (scalar, min_limit, max_limit) = match encoding {
        ScalarEncoding::I32 => ("i32", i32::MIN as i64, i32::MAX as i64),
        ScalarEncoding::I64 => ("i64", i64::MIN, i64::MAX),
        _ => return Err(PayloadError::InvalidRange),
    };
    let min = range
        .min
        .value
        .parse::<i64>()
        .map_err(|_| PayloadError::InvalidRange)?;
    let max = range
        .max
        .value
        .parse::<i64>()
        .map_err(|_| PayloadError::InvalidRange)?;
    if range.min.scalar != scalar
        || range.max.scalar != scalar
        || min < min_limit
        || max > max_limit
        || min > max
    {
        return Err(PayloadError::InvalidRange);
    }
    let wrap = match range.mode.as_str() {
        "clamp" => false,
        "wrap" => true,
        _ => return Err(PayloadError::InvalidRange),
    };
    Ok(IntegerDomain { min, max, wrap })
}

fn validate_defaults_and_extents(fields: &[PayloadField]) -> Result<(), PayloadError> {
    fn names(fields: &[PayloadField]) -> Result<(), PayloadError> {
        if fields
            .iter()
            .any(|field| field.name.contains(['.', '[', ']']))
        {
            return Err(PayloadError::InvalidSchema);
        }
        Ok(())
    }
    names(fields)?;
    let mut pending = fields
        .iter()
        .map(|field| (&field.ty, field.default.as_ref(), 1usize))
        .collect::<Vec<_>>();
    while let Some((ty, default, outer)) = pending.pop() {
        if let Some(default) = default {
            if matches!(ty, PayloadType::Slice { .. }) {
                return Err(PayloadError::InvalidValue);
            }
            ty.visit_values(default, |_, encoding, value| {
                let mut bytes = [0; 8];
                value.write_scalar(encoding, &mut bytes[..encoding.byte_size()])
            })?;
        }
        match ty {
            PayloadType::Array { element, len } => {
                pending.push((element, None, checked_mul(outer, *len)?))
            }
            PayloadType::Slice { element } => pending.push((element, None, outer)),
            PayloadType::Struct { fields, .. } => {
                names(fields)?;
                pending.extend(
                    fields
                        .iter()
                        .map(|field| (&field.ty, field.default.as_ref(), outer)),
                );
            }
            PayloadType::Tuple { elements } => {
                pending.extend(elements.iter().map(|element| (element, None, outer)))
            }
            PayloadType::Scalar { .. } => {}
        }
    }
    Ok(())
}
