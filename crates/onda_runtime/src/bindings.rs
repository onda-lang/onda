//! Validates host memory and commits only the affected ABI descriptor slots.
use super::*;
use RuntimeSurface::{Buffer, Input, Output};

/// Checks initialized state and completeness of required audio bindings.
pub fn prepare_process(instance: &Instance) -> Result<(), RuntimeError> {
    instance.initialized_state()?;
    for (surface, missing, bound) in [
        (Input, instance.missing_inputs, &instance.input_bound),
        (Output, instance.missing_outputs, &instance.output_bound),
    ] {
        if missing != 0 {
            return Err(RuntimeError::MissingBinding {
                surface,
                index: bound
                    .iter()
                    .position(|bound| !bound)
                    .expect("missing binding"),
            });
        }
    }
    Ok(())
}

fn validate_audio_binding(
    descriptor: &onda_codegen_llvm::DeclaredIo,
    surface: RuntimeSurface,
    index: usize,
    pointer: *const u8,
    bytes: usize,
    frames: usize,
) -> Result<(), RuntimeError> {
    let expected = descriptor
        .byte_size()
        .checked_mul(frames)
        .ok_or(RuntimeError::SizeOverflow)?;
    if bytes != expected || pointer.is_null() {
        return Err(RuntimeError::SizeMismatch {
            surface,
            index,
            expected,
            actual: bytes,
        });
    }
    validate_alignment(pointer, descriptor.elem_ty(), surface, index)
}

fn validate_alignment(
    pointer: *const u8,
    ty: PrimitiveType,
    surface: RuntimeSurface,
    index: usize,
) -> Result<(), RuntimeError> {
    let alignment = primitive_type_alignment(ty);
    if !(pointer as usize).is_multiple_of(alignment) {
        return Err(RuntimeError::Misaligned {
            surface,
            index,
            alignment,
        });
    }
    Ok(())
}

/// Binds full-block planar input storage, or unbinds with null and zero bytes.
///
/// # Safety
/// Bound storage must stay readable, naturally aligned and at a stable address
/// until replaced or instance destruction. All accessed host regions must be
/// mutually non-overlapping. No operation on this instance may overlap the call.
pub unsafe fn bind_input(
    instance: &mut Instance,
    index: usize,
    pointer: *const u8,
    bytes: usize,
) -> Result<(), RuntimeError> {
    let desc = instance
        .program
        .inputs()
        .get(index)
        .ok_or(RuntimeError::UnknownIndex {
            surface: Input,
            index,
        })?;
    let unbind = pointer.is_null() && bytes == 0;
    if !unbind {
        validate_audio_binding(
            desc,
            Input,
            index,
            pointer,
            bytes,
            instance.program.block_size(),
        )?;
    }
    for channel in 0..desc.array_len() {
        instance.input_ptrs[desc.slot_offset() + channel] = if unbind {
            std::ptr::null()
        } else {
            unsafe {
                pointer.add(
                    channel * instance.program.block_size() * primitive_type_bytes(desc.elem_ty()),
                )
            }
        };
    }
    instance.missing_inputs =
        instance.missing_inputs + usize::from(unbind) - usize::from(!instance.input_bound[index]);
    instance.input_bound[index] = !unbind;
    Ok(())
}

/// Binds full-block planar output storage, or unbinds with null and zero bytes.
///
/// # Safety
/// Storage must satisfy [`bind_input`]'s lifetime, extent, alignment and aliasing
/// contract and must additionally remain writable.
pub unsafe fn bind_output(
    instance: &mut Instance,
    index: usize,
    pointer: *mut u8,
    bytes: usize,
) -> Result<(), RuntimeError> {
    let desc = instance
        .program
        .outputs()
        .get(index)
        .ok_or(RuntimeError::UnknownIndex {
            surface: Output,
            index,
        })?;
    let unbind = pointer.is_null() && bytes == 0;
    if !unbind {
        validate_audio_binding(
            desc,
            Output,
            index,
            pointer,
            bytes,
            instance.program.block_size(),
        )?;
    }
    for channel in 0..desc.array_len() {
        instance.output_ptrs[desc.slot_offset() + channel] = if unbind {
            std::ptr::null_mut()
        } else {
            unsafe {
                pointer.add(
                    channel * instance.program.block_size() * primitive_type_bytes(desc.elem_ty()),
                )
            }
        };
    }
    instance.missing_outputs =
        instance.missing_outputs + usize::from(unbind) - usize::from(!instance.output_bound[index]);
    instance.output_bound[index] = !unbind;
    Ok(())
}

/// Binds external samples. A zero sample rate or null/zero shape unbinds the slot.
/// Updates descriptors immediately; this never reruns authored initialization.
///
/// # Safety
/// The pointer must remain valid for `frames * channels` naturally aligned
/// elements until replaced or instance destruction, writable if the program may
/// write this buffer, and disjoint from every other accessed host region.
#[allow(clippy::too_many_arguments)]
pub unsafe fn bind_buffer(
    instance: &mut Instance,
    index: usize,
    pointer: *mut u8,
    frames: usize,
    channels: usize,
    sample_rate: f32,
    element: PrimitiveType,
) -> Result<(), RuntimeError> {
    let desc = instance
        .program
        .buffers()
        .get(index)
        .ok_or(RuntimeError::UnknownIndex {
            surface: Buffer,
            index,
        })?;
    if sample_rate == 0.0 || (pointer.is_null() && frames == 0 && channels == 0) {
        instance.buffer_ptrs[index] = std::ptr::null_mut();
        instance.buffer_frames[index] = 1;
        instance.buffer_channels[index] = match desc.channels() {
            DeclaredBufferChannels::Mono | DeclaredBufferChannels::Dynamic => 1,
            DeclaredBufferChannels::Static(count) => count as i32,
        };
        instance.buffer_sample_rates[index] = instance.program.sample_rate();
        return Ok(());
    }
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(RuntimeError::InvalidSampleRate);
    }
    if element != desc.elem_ty() {
        return Err(RuntimeError::InvalidElementType { index });
    }
    if pointer.is_null()
        || frames == 0
        || channels == 0
        || match desc.channels() {
            DeclaredBufferChannels::Mono => channels != 1,
            DeclaredBufferChannels::Static(expected) => channels != expected,
            DeclaredBufferChannels::Dynamic => false,
        }
    {
        return Err(RuntimeError::InvalidShape {
            surface: Buffer,
            index,
        });
    }
    validate_alignment(pointer, element, Buffer, index)?;
    frames
        .checked_mul(channels)
        .and_then(|count| count.checked_mul(primitive_type_bytes(element)))
        .filter(|bytes| *bytes <= i32::MAX as usize)
        .ok_or(RuntimeError::SizeOverflow)?;
    instance.buffer_ptrs[index] = pointer;
    instance.buffer_frames[index] = frames as i32;
    instance.buffer_channels[index] = channels as i32;
    instance.buffer_sample_rates[index] = sample_rate;
    Ok(())
}
