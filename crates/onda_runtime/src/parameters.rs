//! Parameter writes and held control-output reads.
use super::*;

pub fn set_param_by_index(
    instance: &mut Instance,
    index: usize,
    value_bytes: &[u8],
) -> Result<(), RuntimeError> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(RuntimeError::UnknownIndex {
            surface: RuntimeSurface::Parameter,
            index,
        });
    };
    let expected_bytes = desc.byte_size();
    if value_bytes.len() != expected_bytes {
        return Err(RuntimeError::SizeMismatch {
            surface: RuntimeSurface::Parameter,
            index,
            expected: expected_bytes,
            actual: value_bytes.len(),
        });
    }
    let start = desc.byte_offset();
    let end = start.saturating_add(expected_bytes);
    if end > instance.params.len() {
        return Err(RuntimeError::InvalidStorage);
    }
    instance.params[start..end].copy_from_slice(value_bytes);
    Ok(())
}

/// Writes one primitive parameter element without touching its siblings.
pub fn set_param_element_by_index(
    instance: &mut Instance,
    index: usize,
    element: usize,
    value_bytes: &[u8],
) -> Result<(), RuntimeError> {
    let desc = instance
        .program
        .param_descriptor(index)
        .ok_or(RuntimeError::UnknownIndex {
            surface: RuntimeSurface::Parameter,
            index,
        })?;
    let width = desc.byte_size() / desc.array_len();
    if element >= desc.array_len() || value_bytes.len() != width {
        return Err(RuntimeError::InvalidShape {
            surface: RuntimeSurface::Parameter,
            index,
        });
    }
    let start = desc.byte_offset() + element * width;
    let storage = instance
        .params
        .get_mut(start..start + width)
        .ok_or(RuntimeError::InvalidStorage)?;
    storage.copy_from_slice(value_bytes);
    Ok(())
}

pub fn set_param_plain_f64(
    instance: &mut Instance,
    index: usize,
    plain: f64,
) -> Result<(), RuntimeError> {
    set_param_control_f64(instance, index, None, ParamControlInput::Plain(plain))
}

/// Constrains and writes one primitive parameter element in its plain domain.
pub fn set_param_element_plain_f64(
    instance: &mut Instance,
    index: usize,
    element: usize,
    plain: f64,
) -> Result<(), RuntimeError> {
    set_param_control_f64(
        instance,
        index,
        Some(element),
        ParamControlInput::Plain(plain),
    )
}

pub fn set_param_normalized(
    instance: &mut Instance,
    index: usize,
    normalized: f64,
) -> Result<(), RuntimeError> {
    set_param_control_f64(
        instance,
        index,
        None,
        ParamControlInput::Normalized(normalized),
    )
}

/// Maps and writes one primitive parameter element from a normalized host value.
pub fn set_param_element_normalized(
    instance: &mut Instance,
    index: usize,
    element: usize,
    normalized: f64,
) -> Result<(), RuntimeError> {
    set_param_control_f64(
        instance,
        index,
        Some(element),
        ParamControlInput::Normalized(normalized),
    )
}

#[derive(Debug, Clone, Copy)]
enum ParamControlInput {
    Plain(f64),
    Normalized(f64),
}

fn set_param_control_f64(
    instance: &mut Instance,
    index: usize,
    element: Option<usize>,
    input: ParamControlInput,
) -> Result<(), RuntimeError> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(RuntimeError::UnknownIndex {
            surface: RuntimeSurface::Parameter,
            index,
        });
    };
    if element.is_none() && desc.is_array() {
        return Err(RuntimeError::InvalidShape {
            surface: RuntimeSurface::Parameter,
            index,
        });
    }
    if element.is_some_and(|element| element >= desc.array_len()) {
        return Err(RuntimeError::InvalidShape {
            surface: RuntimeSurface::Parameter,
            index,
        });
    }
    let ty = desc.elem_ty();
    let value = match ty {
        PrimitiveType::Bool => match input {
            ParamControlInput::Plain(value) | ParamControlInput::Normalized(value) => {
                if value >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
        },
        _ => {
            let domain = desc
                .param_domain()
                .ok_or(RuntimeError::NoControlDomain { index })?;
            match input {
                ParamControlInput::Plain(value) => domain.constrain_plain(value),
                ParamControlInput::Normalized(value) => domain.normalized_to_plain(value),
            }
        }
    };
    let mut bytes = [0_u8; 8];
    let len = match ty {
        PrimitiveType::F32 => {
            bytes[..4].copy_from_slice(&(value as f32).to_ne_bytes());
            4
        }
        PrimitiveType::F64 => {
            bytes.copy_from_slice(&value.to_ne_bytes());
            8
        }
        PrimitiveType::I32 | PrimitiveType::Enum(_) => {
            bytes[..4].copy_from_slice(&(value.round() as i32).to_ne_bytes());
            4
        }
        PrimitiveType::I64 => {
            bytes.copy_from_slice(&(value.round() as i64).to_ne_bytes());
            8
        }
        PrimitiveType::Bool => {
            bytes[0] = u8::from(value != 0.0);
            1
        }
    };
    let start = desc.byte_offset() + element.unwrap_or(0) * len;
    instance.params[start..start + len].copy_from_slice(&bytes[..len]);
    Ok(())
}

pub fn read_control_output_bytes(
    instance: &Instance,
    index: usize,
    out: &mut [u8],
) -> Result<(), RuntimeError> {
    let Some(desc) = instance.program.control_output_descriptor(index) else {
        return Err(RuntimeError::UnknownIndex {
            surface: RuntimeSurface::ControlOutput,
            index,
        });
    };
    let expected_bytes = desc.byte_size();
    if out.len() != expected_bytes {
        return Err(RuntimeError::SizeMismatch {
            surface: RuntimeSurface::ControlOutput,
            index,
            expected: expected_bytes,
            actual: out.len(),
        });
    }
    let Some(start) = instance.program.control_output_storage_byte_offset(index) else {
        return Err(RuntimeError::InvalidStorage);
    };
    let end = start.saturating_add(expected_bytes);
    let state = instance.initialized_state()?.bytes();
    if end > state.len() {
        return Err(RuntimeError::InvalidStorage);
    }
    out.copy_from_slice(&state[start..end]);
    Ok(())
}
