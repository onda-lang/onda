//! Host value traversal uses the same leaf order as storage planning. Values
//! may be named objects or ordered aggregates without coupling the ABI to a UI
//! toolkit or a particular JSON implementation.
use super::*;

pub trait PayloadSource: Sized {
    fn sequence_len(&self) -> Option<usize>;
    fn struct_len(&self) -> Option<usize>;
    fn member(&self, index: usize, name: Option<&str>) -> Option<&Self>;
    /// Write one packed little-endian scalar, without domain normalization.
    fn write_scalar(&self, encoding: ScalarEncoding, output: &mut [u8])
        -> Result<(), PayloadError>;
}

impl PayloadType {
    /// Visit source scalar values in canonical leaf order. Arrays revisit the
    /// same leaf indices; they never expand the shape or executable signature.
    pub fn visit_values<V: PayloadSource>(
        &self,
        value: &V,
        mut visit: impl FnMut(usize, ScalarEncoding, &V) -> Result<(), PayloadError>,
    ) -> Result<(), PayloadError> {
        fn leaf_count(ty: &PayloadType) -> usize {
            match ty {
                PayloadType::Scalar { .. } => 1,
                PayloadType::Tuple { elements } => elements.len(),
                PayloadType::Struct { fields, .. } => {
                    fields.iter().map(|field| leaf_count(&field.ty)).sum()
                }
                PayloadType::Array { element, .. } | PayloadType::Slice { element } => {
                    leaf_count(element)
                }
            }
        }
        fn walk<V: PayloadSource>(
            ty: &PayloadType,
            value: &V,
            start: usize,
            visit: &mut impl FnMut(usize, ScalarEncoding, &V) -> Result<(), PayloadError>,
        ) -> Result<usize, PayloadError> {
            match ty {
                PayloadType::Scalar { encoding, .. } => {
                    visit(start, *encoding, value)?;
                    Ok(start + 1)
                }
                PayloadType::Struct { fields, .. } => {
                    if value.struct_len() != Some(fields.len()) {
                        return Err(PayloadError::InvalidValue);
                    }
                    let mut next = start;
                    for (index, field) in fields.iter().enumerate() {
                        next = walk(
                            &field.ty,
                            value
                                .member(index, Some(&field.name))
                                .ok_or(PayloadError::InvalidValue)?,
                            next,
                            visit,
                        )?;
                    }
                    Ok(next)
                }
                PayloadType::Tuple { elements } => {
                    if value.sequence_len() != Some(elements.len()) {
                        return Err(PayloadError::InvalidValue);
                    }
                    let mut next = start;
                    for (index, ty) in elements.iter().enumerate() {
                        next = walk(
                            ty,
                            value
                                .member(index, None)
                                .ok_or(PayloadError::InvalidValue)?,
                            next,
                            visit,
                        )?;
                    }
                    Ok(next)
                }
                PayloadType::Array { element, .. } | PayloadType::Slice { element } => {
                    let len = value.sequence_len().ok_or(PayloadError::InvalidValue)?;
                    if let PayloadType::Array { len: expected, .. } = ty {
                        if len != *expected {
                            return Err(PayloadError::InvalidValue);
                        }
                    }
                    let mut next = start + leaf_count(element);
                    for index in 0..len {
                        next = walk(
                            element,
                            value
                                .member(index, None)
                                .ok_or(PayloadError::InvalidValue)?,
                            start,
                            visit,
                        )?;
                    }
                    Ok(next)
                }
            }
        }
        walk(self, value, 0, &mut visit).map(|_| ())
    }
}

/// Reusable host encoder. Construction provisions all storage; successful encoding
/// performs no allocations and rejects payloads exceeding the supplied wire capacity.
#[derive(Debug)]
pub struct PayloadEncoder {
    plan: PayloadPlan,
    workspace: PayloadEncoderWorkspace,
}

/// Reusable storage for encoding payloads from multiple prepared plans.
/// Construction provisions all storage; successful encoding performs no allocations.
#[derive(Debug)]
pub struct PayloadEncoderWorkspace {
    lengths: Vec<i32>,
    offsets: Vec<usize>,
    output: Vec<u8>,
}

impl PayloadPlan {
    fn value_lengths<V: PayloadSource>(
        &self,
        values: &[V],
        lengths: &mut [i32],
    ) -> Result<(), PayloadError> {
        if values.len() != self.params.len() {
            return Err(PayloadError::InvalidValue);
        }
        let mut dynamic = 0;
        for (param, value) in self.params.iter().zip(values) {
            if param.dynamic {
                lengths[dynamic] =
                    i32::try_from(value.sequence_len().ok_or(PayloadError::InvalidValue)?)
                        .map_err(|_| PayloadError::Overflow)?;
                dynamic += 1;
            }
        }
        Ok(())
    }

    /// Encode application-owned values outside realtime execution.
    pub fn encode_values<V: PayloadSource>(&self, values: &[V]) -> Result<Vec<u8>, PayloadError> {
        let mut lengths = vec![0; self.dynamic_parameters()];
        self.value_lengths(values, &mut lengths)?;
        let (size, _) = self.sizes(&lengths)?;
        let mut encoder = PayloadEncoder::new(self.clone(), size)?;
        encoder.encode(values)?;
        Ok(encoder.workspace.output)
    }
}

impl PayloadEncoder {
    pub fn new(plan: PayloadPlan, wire_capacity: usize) -> Result<Self, PayloadError> {
        let workspace = PayloadEncoderWorkspace::new(std::slice::from_ref(&plan), wire_capacity)?;
        Ok(Self { plan, workspace })
    }

    pub fn encode<V: PayloadSource>(&mut self, values: &[V]) -> Result<&[u8], PayloadError> {
        self.workspace.encode(&self.plan, values)
    }
}

impl PayloadEncoderWorkspace {
    pub fn new(plans: &[PayloadPlan], wire_capacity: usize) -> Result<Self, PayloadError> {
        if wire_capacity > i32::MAX as usize {
            return Err(PayloadError::Overflow);
        }
        Ok(Self {
            lengths: vec![
                0;
                plans
                    .iter()
                    .map(PayloadPlan::dynamic_parameters)
                    .max()
                    .unwrap_or(0)
            ],
            offsets: vec![
                0;
                plans
                    .iter()
                    .map(|plan| plan.tensors.len())
                    .max()
                    .unwrap_or(0)
            ],
            output: vec![0; wire_capacity],
        })
    }

    pub fn wire_capacity(&self) -> usize {
        self.output.len()
    }

    pub fn encode<V: PayloadSource>(
        &mut self,
        plan: &PayloadPlan,
        values: &[V],
    ) -> Result<&[u8], PayloadError> {
        let Self {
            lengths,
            offsets,
            output,
        } = self;
        let lengths = lengths
            .get_mut(..plan.dynamic_parameters())
            .ok_or(PayloadError::InsufficientCapacity)?;
        let offsets = offsets
            .get_mut(..plan.tensors.len())
            .ok_or(PayloadError::InsufficientCapacity)?;
        plan.value_lengths(values, lengths)?;
        let (size, _) = plan.sizes(lengths)?;
        if size > output.len() {
            return Err(PayloadError::InsufficientCapacity);
        }
        let mut dynamic = 0;
        plan.walk(
            |offset, _| {
                let len = lengths[dynamic];
                dynamic += 1;
                output[offset..offset + 4].copy_from_slice(&len.to_le_bytes());
                Ok(len as usize)
            },
            |region| {
                if let super::layout::Region::Tensor(region) = region {
                    offsets[region.tensor] = region.wire.start;
                }
            },
        )?;
        for ((field, param), value) in plan.schema.params.iter().zip(&plan.params).zip(values) {
            field.ty.visit_values(value, |leaf, encoding, value| {
                let index = param.tensors.start + leaf;
                let tensor = plan.tensors.get(index).ok_or(PayloadError::InvalidSchema)?;
                if tensor.encoding != encoding {
                    return Err(PayloadError::InvalidSchema);
                }
                let end = offsets[index]
                    .checked_add(encoding.byte_size())
                    .ok_or(PayloadError::Overflow)?;
                let bytes = output
                    .get_mut(offsets[index]..end)
                    .ok_or(PayloadError::InvalidValue)?;
                value.write_scalar(encoding, bytes)?;
                offsets[index] = end;
                Ok(())
            })?;
        }
        Ok(&output[..size])
    }
}

impl PayloadSource for PayloadDefault {
    fn struct_len(&self) -> Option<usize> {
        self.sequence_len()
    }
    fn sequence_len(&self) -> Option<usize> {
        match self {
            Self::Aggregate(values) => Some(values.len()),
            Self::Scalar(_) => None,
        }
    }
    fn member(&self, index: usize, _name: Option<&str>) -> Option<&Self> {
        match self {
            Self::Aggregate(values) => values.get(index),
            Self::Scalar(_) => None,
        }
    }
    fn write_scalar(
        &self,
        encoding: ScalarEncoding,
        output: &mut [u8],
    ) -> Result<(), PayloadError> {
        let Self::Scalar(text) = self else {
            return Err(PayloadError::InvalidValue);
        };
        match encoding {
            ScalarEncoding::Bool => output.copy_from_slice(&[u8::from(
                text.parse::<bool>()
                    .map_err(|_| PayloadError::InvalidValue)?,
            )]),
            ScalarEncoding::I32 => output.copy_from_slice(
                &text
                    .parse::<i32>()
                    .map_err(|_| PayloadError::InvalidValue)?
                    .to_le_bytes(),
            ),
            ScalarEncoding::I64 => output.copy_from_slice(
                &text
                    .parse::<i64>()
                    .map_err(|_| PayloadError::InvalidValue)?
                    .to_le_bytes(),
            ),
            ScalarEncoding::F32 => output.copy_from_slice(
                &text
                    .parse::<f32>()
                    .map_err(|_| PayloadError::InvalidValue)?
                    .to_le_bytes(),
            ),
            ScalarEncoding::F64 => output.copy_from_slice(
                &text
                    .parse::<f64>()
                    .map_err(|_| PayloadError::InvalidValue)?
                    .to_le_bytes(),
            ),
        }
        Ok(())
    }
}

/// Construct application-owned decoded values outside realtime execution.
pub trait PayloadSink: Sized {
    fn scalar(encoding: ScalarEncoding, wire: &[u8]) -> Self;
    fn aggregate(ty: &PayloadType, values: Vec<Self>) -> Self;
}

impl PayloadPlan {
    pub fn decode_values<V: PayloadSink>(&self, input: &[u8]) -> Result<Vec<V>, PayloadError> {
        self.required_workspace(input)?;
        let mut offsets = vec![0; self.tensors.len()];
        let mut lengths = Vec::with_capacity(self.dynamic_parameters());
        self.walk(
            |offset, _| {
                let length = super::layout::read_length(input, offset)?;
                lengths.push(length);
                Ok(length)
            },
            |region| {
                if let super::layout::Region::Tensor(region) = region {
                    offsets[region.tensor] = region.wire.start;
                }
            },
        )?;
        fn decode<V: PayloadSink>(
            ty: &PayloadType,
            input: &[u8],
            offsets: &mut [usize],
            leaf: &mut usize,
            dynamic: usize,
        ) -> V {
            match ty {
                PayloadType::Scalar { encoding, .. } => {
                    let start = offsets[*leaf];
                    let end = start + encoding.byte_size();
                    offsets[*leaf] = end;
                    *leaf += 1;
                    V::scalar(*encoding, &input[start..end])
                }
                PayloadType::Struct { fields, .. } => V::aggregate(
                    ty,
                    fields
                        .iter()
                        .map(|field| decode(&field.ty, input, offsets, leaf, 0))
                        .collect(),
                ),
                PayloadType::Tuple { elements } => V::aggregate(
                    ty,
                    elements
                        .iter()
                        .map(|element| decode(element, input, offsets, leaf, 0))
                        .collect(),
                ),
                PayloadType::Array { element, .. } | PayloadType::Slice { element } => {
                    let len = match ty {
                        PayloadType::Array { len, .. } => *len,
                        _ => dynamic,
                    };
                    let start = *leaf;
                    let values = (0..len)
                        .map(|_| {
                            *leaf = start;
                            decode(element, input, offsets, leaf, 0)
                        })
                        .collect();
                    if len == 0 {
                        *leaf += element.leaves().expect("validated schema").len();
                    }
                    V::aggregate(ty, values)
                }
            }
        }
        let mut dynamic = lengths.into_iter();
        let mut output = Vec::with_capacity(self.params.len());
        for (param, field) in self.params.iter().zip(&self.schema.params) {
            let len = if param.dynamic {
                dynamic.next().expect("validated lengths")
            } else {
                0
            };
            let mut leaf = param.tensors.start;
            output.push(decode(&field.ty, input, &mut offsets, &mut leaf, len));
        }
        Ok(output)
    }
}
