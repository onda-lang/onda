//! Borrowed message plans and allocation-free queries of their packed layout.
use super::*;
use onda_processor_abi::payload::{PayloadError, PayloadPlan, PayloadRegion};

/// Program-owned C views of one canonical message plan. The JIT owns `plan`;
/// the enclosing CompiledProgram keeps it alive through every descriptor borrow.
#[allow(non_camel_case_types)]
pub struct onda_payload_plan_t {
    plan: *const PayloadPlan,
    tensors: Vec<PayloadTensorInfo>,
}

#[repr(C)]
pub struct onda_payload_tensor_info_t {
    pub path: *const c_char,
    pub shape: *const i32,
    pub shape_rank: i32,
    pub parameter_index: i32,
    pub element_type: i32,
    pub is_slice: i32,
    pub fixed_element_count: i32,
}

struct PayloadTensorInfo {
    info: onda_payload_tensor_info_t,
    _path: CString,
    _shape: Box<[i32]>,
}

impl onda_payload_plan_t {
    pub(super) fn new(plan: &PayloadPlan) -> Result<Self, Diagnostic> {
        let mut tensors = Vec::with_capacity(plan.tensors().len());
        for (parameter_index, parameter) in plan.parameters().iter().enumerate() {
            for index in parameter.tensors.clone() {
                let tensor = &plan.tensors()[index];
                let path = CString::new(tensor.path.as_str())
                    .map_err(|_| Diagnostic::internal("payload tensor path contains NUL"))?;
                let shape = tensor
                    .shape
                    .iter()
                    .map(|extent| i32::try_from(*extent))
                    .collect::<Result<Box<[_]>, _>>()
                    .map_err(|_| Diagnostic::internal("payload tensor shape exceeds i32"))?;
                let info = onda_payload_tensor_info_t {
                    path: path.as_ptr(),
                    shape: if shape.is_empty() {
                        ptr::null()
                    } else {
                        shape.as_ptr()
                    },
                    shape_rank: i32::try_from(shape.len())
                        .map_err(|_| Diagnostic::internal("payload tensor rank exceeds i32"))?,
                    parameter_index: i32::try_from(parameter_index)
                        .map_err(|_| Diagnostic::internal("payload parameter index exceeds i32"))?,
                    element_type: match tensor.encoding {
                        onda_processor_abi::payload::ScalarEncoding::F32 => ONDA_PRIMITIVE_F32,
                        onda_processor_abi::payload::ScalarEncoding::F64 => ONDA_PRIMITIVE_F64,
                        onda_processor_abi::payload::ScalarEncoding::I32 => ONDA_PRIMITIVE_I32,
                        onda_processor_abi::payload::ScalarEncoding::I64 => ONDA_PRIMITIVE_I64,
                        onda_processor_abi::payload::ScalarEncoding::Bool => ONDA_PRIMITIVE_BOOL,
                    },
                    is_slice: i32::from(parameter.dynamic),
                    fixed_element_count: i32::try_from(tensor.elements)
                        .map_err(|_| Diagnostic::internal("payload tensor extent exceeds i32"))?,
                };
                tensors.push(PayloadTensorInfo {
                    info,
                    _path: path,
                    _shape: shape,
                });
            }
        }
        Ok(Self { plan, tensors })
    }
}

/// Borrows a leaf descriptor for either an event or delegate until program destruction.
#[no_mangle]
pub unsafe extern "C" fn onda_payload_tensor_info(
    plan: *const onda_payload_plan_t,
    tensor_index: i32,
) -> *const onda_payload_tensor_info_t {
    plan.as_ref()
        .and_then(|plan| plan.tensors.get(usize::try_from(tensor_index).ok()?))
        .map_or(ptr::null(), |tensor| &tensor.info)
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
    let Some(handle) = plan.as_ref() else {
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
    let plan = &*handle.plan;
    let lengths = if slice_length_count == 0 {
        &[]
    } else {
        slice::from_raw_parts(slice_lengths, slice_length_count as usize)
    };
    write_layout(plan, out_regions, region_capacity, out_layout, |visit| {
        if out_regions.is_null() {
            plan.sizes(lengths)
        } else {
            plan.visit_layout(lengths, visit)
        }
    })
}

/// Queries the layout of complete packed wire bytes, without decoding values.
#[no_mangle]
pub unsafe extern "C" fn onda_payload_layout_from_bytes(
    plan: *const onda_payload_plan_t,
    payload: *const u8,
    payload_bytes: i32,
    out_regions: *mut onda_payload_region_t,
    region_capacity: i32,
    out_layout: *mut onda_payload_layout_t,
) -> i32 {
    let Some(handle) = plan.as_ref() else {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    };
    if payload_bytes < 0 || (payload_bytes != 0 && payload.is_null()) {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let input = if payload_bytes == 0 {
        &[]
    } else {
        slice::from_raw_parts(payload, payload_bytes as usize)
    };
    let plan = &*handle.plan;
    write_layout(plan, out_regions, region_capacity, out_layout, |visit| {
        if out_regions.is_null() {
            plan.required_workspace(input)
                .map(|workspace| (input.len(), workspace))
        } else {
            plan.visit_wire_layout(input, visit)
        }
    })
}

unsafe fn write_layout(
    plan: &PayloadPlan,
    out_regions: *mut onda_payload_region_t,
    region_capacity: i32,
    out_layout: *mut onda_payload_layout_t,
    resolve: impl FnOnce(&mut dyn FnMut(PayloadRegion)) -> Result<(usize, usize), PayloadError>,
) -> i32 {
    if out_layout.is_null()
        || region_capacity < 0
        || (region_capacity != 0 && out_regions.is_null())
    {
        return ONDA_API_ERROR_INVALID_ARGUMENT;
    }
    let Ok(region_count) = i32::try_from(plan.region_count()) else {
        return ONDA_API_ERROR_SIZE_OVERFLOW;
    };
    if !out_regions.is_null() && region_capacity < region_count {
        return ONDA_API_ERROR_SIZE_MISMATCH;
    }
    let mut next = 0;
    let sizes = resolve(&mut |region| {
        if out_regions.is_null() {
            return;
        }
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
    });
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
