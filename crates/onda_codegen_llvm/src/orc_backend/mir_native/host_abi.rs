use crate::{RuntimeError, RuntimeSurface};
use onda_mir::{BufferChannels, Program, TypeId};

use crate::BufferDescriptorTables;

use super::{audio_port_shape, scalar_store_size};

pub(super) fn abi_const_ptr<T>(values: &[T]) -> *const T {
    if values.is_empty() {
        std::ptr::null()
    } else {
        values.as_ptr()
    }
}

pub(super) fn abi_mut_ptr<T>(values: &mut [T]) -> *mut T {
    if values.is_empty() {
        std::ptr::null_mut()
    } else {
        values.as_mut_ptr()
    }
}

pub(super) fn validate_audio_abi(
    program: &Program,
    input_ptrs: &[*const u8],
    output_ptrs: &[*mut u8],
) -> Result<(), RuntimeError> {
    validate_audio_table(
        program,
        RuntimeSurface::Input,
        program.interface.inputs.iter().map(|port| port.ty),
        input_ptrs.len(),
        |index| input_ptrs[index],
    )?;
    validate_audio_table(
        program,
        RuntimeSurface::Output,
        program.interface.outputs.iter().map(|port| port.ty),
        output_ptrs.len(),
        |index| output_ptrs[index].cast_const(),
    )
}

fn validate_audio_table<I>(
    program: &Program,
    surface: RuntimeSurface,
    ports: I,
    pointer_count: usize,
    mut pointer_at: impl FnMut(usize) -> *const u8,
) -> Result<(), RuntimeError>
where
    I: Iterator<Item = TypeId> + Clone,
{
    let expected = ports.clone().try_fold(0usize, |count, ty| {
        let (_, width) = audio_port_shape(program, ty).ok_or(RuntimeError::InvalidStorage)?;
        count.checked_add(width).ok_or(RuntimeError::InvalidStorage)
    })?;
    if pointer_count != expected {
        return Err(RuntimeError::InvalidShape { surface, index: 0 });
    }

    let mut flat_index = 0usize;
    for ty in ports {
        let (scalar, width) = audio_port_shape(program, ty)
            .expect("the first audio-interface traversal validated every port type");
        let alignment = usize::try_from(scalar_store_size(scalar))
            .expect("primitive scalar alignment fits usize");
        for _ in 0..width {
            let pointer = pointer_at(flat_index);
            if pointer.is_null() {
                return Err(RuntimeError::MissingBinding {
                    surface,
                    index: flat_index,
                });
            }
            if !pointer.addr().is_multiple_of(alignment) {
                return Err(RuntimeError::Misaligned {
                    surface,
                    index: flat_index,
                    alignment,
                });
            }
            flat_index += 1;
        }
    }
    Ok(())
}

pub(super) fn validate_buffer_abi(
    program: &Program,
    buffers: BufferDescriptorTables<'_>,
) -> Result<(), RuntimeError> {
    let expected = program.interface.buffers.len();
    if buffers.pointers.len() != expected
        || buffers.frames.len() != expected
        || buffers.channels.len() != expected
        || buffers.sample_rates.len() != expected
    {
        return Err(RuntimeError::InvalidStorage);
    }

    for index in 0..expected {
        let frames = buffers.frames[index];
        let channels = buffers.channels[index];
        let pointer_is_null = buffers.pointers[index].is_null();
        let sample_rate = buffers.sample_rates[index];
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(RuntimeError::InvalidSampleRate);
        }
        if frames <= 0 || channels <= 0 {
            return Err(RuntimeError::InvalidShape {
                surface: RuntimeSurface::Buffer,
                index,
            });
        }

        let alignment =
            usize::try_from(scalar_store_size(program.interface.buffers[index].element))
                .expect("primitive scalar alignment fits usize");
        if !pointer_is_null && !buffers.pointers[index].addr().is_multiple_of(alignment) {
            return Err(RuntimeError::Misaligned {
                surface: RuntimeSurface::Buffer,
                index,
                alignment,
            });
        }

        let expected_channels = match program.interface.buffers[index].channels {
            BufferChannels::Mono => Some(1),
            BufferChannels::Static(expected) => Some(expected),
            BufferChannels::Dynamic => None,
        };
        if let Some(expected_channels) = expected_channels {
            if u32::try_from(channels) != Ok(expected_channels) {
                return Err(RuntimeError::InvalidShape {
                    surface: RuntimeSurface::Buffer,
                    index,
                });
            }
        }
        if frames.checked_mul(channels).is_none() {
            return Err(RuntimeError::SizeOverflow);
        }
        let element_size =
            i32::try_from(scalar_store_size(program.interface.buffers[index].element))
                .expect("primitive scalar size fits i32");
        if frames
            .checked_mul(channels)
            .and_then(|elements| elements.checked_mul(element_size))
            .is_none()
        {
            return Err(RuntimeError::SizeOverflow);
        }
    }
    Ok(())
}
