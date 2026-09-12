use super::layout::{read_length, IntegerDomain, Region};
use super::*;

/// A validated normalized payload. Its borrow keeps caller-owned workspace
/// alive through synchronous forwarding, without retaining the original input.
pub struct PreparedPayload<'a> {
    plan: &'a PayloadPlan,
    storage: &'a [u8],
    wire_size: usize,
}

impl PayloadPlan {
    /// Validate shape and capacity before changing workspace. There are no
    /// fallible operations once normalization starts and no dispatch allocations.
    pub fn prepare<'a>(
        &'a self,
        input: &[u8],
        workspace: &'a mut [u8],
    ) -> Result<PreparedPayload<'a>, PayloadError> {
        let required = self.required_workspace(input)?;
        if workspace.len() < required {
            return Err(PayloadError::InsufficientCapacity);
        }
        if required != 0 && !workspace.as_ptr().addr().is_multiple_of(8) {
            return Err(PayloadError::MisalignedWorkspace);
        }
        let storage = &mut workspace[..required];
        self.walk(
            |offset, _| read_length(input, offset),
            |region| match region {
                Region::Length { wire, workspace } => storage[workspace].copy_from_slice(
                    &i32::from_le_bytes(input[wire].try_into().unwrap()).to_ne_bytes(),
                ),
                Region::Tensor(region) => {
                    let leaf = &self.tensors[region.tensor];
                    let size = leaf.encoding.byte_size();
                    for (source, target) in input[region.wire]
                        .chunks_exact(size)
                        .zip(storage[region.workspace].chunks_exact_mut(size))
                    {
                        normalize_scalar(leaf.encoding, leaf.domain, source, target);
                    }
                }
            },
        )
        .expect("preflight validated every extent");
        Ok(PreparedPayload {
            plan: self,
            storage,
            wire_size: input.len(),
        })
    }
}

impl PreparedPayload<'_> {
    pub fn storage(&self) -> &[u8] {
        self.storage
    }
    pub fn wire_size(&self) -> usize {
        self.wire_size
    }

    pub fn visit_tensors(&self, mut visit: impl FnMut(&PayloadTensor, TensorRegion, &[u8])) {
        self.plan
            .walk(
                |_, offset| read_prepared_length(self.storage, offset),
                |region| {
                    if let Region::Tensor(region) = region {
                        let bytes = &self.storage[region.workspace.clone()];
                        visit(&self.plan.tensors[region.tensor], region, bytes);
                    }
                },
            )
            .expect("prepared payload retains validated lengths");
    }

    /// Write one complete packed payload, or leave output untouched on failure.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, PayloadError> {
        if output.len() < self.wire_size {
            return Err(PayloadError::InsufficientCapacity);
        }
        self.plan
            .walk(
                |_, offset| read_prepared_length(self.storage, offset),
                |region| match region {
                    Region::Length { wire, workspace } => output[wire].copy_from_slice(
                        &i32::from_ne_bytes(self.storage[workspace].try_into().unwrap())
                            .to_le_bytes(),
                    ),
                    Region::Tensor(region) => {
                        let size = self.plan.tensors[region.tensor].encoding.byte_size();
                        for (source, target) in self.storage[region.workspace]
                            .chunks_exact(size)
                            .zip(output[region.wire].chunks_exact_mut(size))
                        {
                            target.copy_from_slice(source);
                            if cfg!(target_endian = "big") {
                                target.reverse();
                            }
                        }
                    }
                },
            )
            .expect("prepared payload retains validated lengths");
        Ok(self.wire_size)
    }
}

fn normalize_integer(value: i64, domain: Option<IntegerDomain>) -> i64 {
    let Some(domain) = domain else { return value };
    if domain.wrap {
        let min = i128::from(domain.min);
        let width = i128::from(domain.max) - min + 1;
        (min + (i128::from(value) - min).rem_euclid(width)) as i64
    } else {
        value.clamp(domain.min, domain.max)
    }
}

fn normalize_scalar(
    encoding: ScalarEncoding,
    domain: Option<IntegerDomain>,
    source: &[u8],
    target: &mut [u8],
) {
    match encoding {
        ScalarEncoding::I32 => {
            let value = i32::from_le_bytes(source.try_into().unwrap());
            target.copy_from_slice(&(normalize_integer(value as i64, domain) as i32).to_ne_bytes());
        }
        ScalarEncoding::I64 => {
            let value = i64::from_le_bytes(source.try_into().unwrap());
            target.copy_from_slice(&normalize_integer(value, domain).to_ne_bytes());
        }
        ScalarEncoding::Bool => target[0] = u8::from(source[0] != 0),
        ScalarEncoding::F32 | ScalarEncoding::F64 => {
            // Preserve IEEE payload bits, including signed zero and NaNs.
            target.copy_from_slice(source);
            if cfg!(target_endian = "big") {
                target.reverse();
            }
        }
    }
}

fn read_prepared_length(bytes: &[u8], offset: usize) -> Result<usize, PayloadError> {
    let end = offset.checked_add(4).ok_or(PayloadError::Overflow)?;
    let prefix = bytes.get(offset..end).ok_or(PayloadError::Truncated)?;
    usize::try_from(i32::from_ne_bytes(prefix.try_into().unwrap()))
        .map_err(|_| PayloadError::NegativeLength)
}
