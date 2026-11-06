//! Borrowed message plans and allocation-free queries of their packed layout.
use super::*;
use onda_processor_abi::payload::{PayloadError, PayloadPlan, PayloadRegion};

#[repr(C)]
pub struct onda_payload_plan_t {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct onda_payload_layout_t {
    pub payload_bytes: i32,
    pub workspace_bytes: i32,
    pub region_count: i32,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct onda_payload_region_t {
    pub parameter_index: i32,
    /// -1 identifies a slice's i32 length prefix; other indices address leaf tensors.
    pub tensor_index: i32,
    pub element_count: i32,
    pub payload_offset: i32,
    pub payload_bytes: i32,
    pub workspace_offset: i32,
}

/// Resolves an immutable program-borrowed plan. No allocations or partial output
/// writes occur. Null regions with zero capacity query only sizes and region count.
#[no_mangle]
pub unsafe extern "C" fn onda_payload_layout(
    plan: *const onda_payload_plan_t,
    slice_lengths: *const i32,
    slice_length_count: i32,
    out_regions: *mut onda_payload_region_t,
    region_capacity: i32,
    out_layout: *mut onda_payload_layout_t,
) -> i32 {
    let Some(plan) = plan.cast::<PayloadPlan>().as_ref() else {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    if out_layout.is_null()
        || slice_length_count < 0
        || region_capacity < 0
        || (slice_length_count != 0 && slice_lengths.is_null())
        || (region_capacity != 0 && out_regions.is_null())
    {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let lengths = if slice_length_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(slice_lengths, slice_length_count as usize)
    };
    let Ok(region_count) = i32::try_from(plan.region_count()) else {
        return ONDA_API_ERROR_SIZE_OVERFLOW;
    };
    if !out_regions.is_null() && region_capacity < region_count {
        return ONDA_API_ERROR_SIZE_MISMATCH;
    }
    let mut next = 0;
    let sizes = if out_regions.is_null() {
        plan.sizes(lengths)
    } else {
        plan.visit_layout(lengths, |region| {
            let (parameter, tensor, elements, wire, workspace) = match region {
                PayloadRegion::Length {
                    parameter,
                    wire,
                    workspace,
                } => (parameter, -1, 1, wire, workspace),
                PayloadRegion::Tensor(region) => (
                    region.parameter,
                    region.tensor as i32,
                    region.elements,
                    region.wire,
                    region.workspace,
                ),
            };
            // The plan bounds byte extents to i32; every parameter has a tensor,
            // and the checked region count bounds both kinds of index.
            out_regions.add(next).write(onda_payload_region_t {
                parameter_index: parameter as i32,
                tensor_index: tensor,
                element_count: elements as i32,
                payload_offset: wire.start as i32,
                payload_bytes: wire.len() as i32,
                workspace_offset: workspace.start as i32,
            });
            next += 1;
        })
    };
    match sizes {
        Ok((payload_bytes, workspace_bytes)) => {
            out_layout.write(onda_payload_layout_t {
                payload_bytes: payload_bytes as i32,
                workspace_bytes: workspace_bytes as i32,
                region_count,
            });
            ONDA_EXECUTION_OK
        }
        Err(PayloadError::Overflow) => ONDA_API_ERROR_SIZE_OVERFLOW,
        Err(_) => ONDA_API_ERROR_INVALID_SHAPE,
    }
}
