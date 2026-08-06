use onda_frontend::Diagnostic;
use onda_mir::{BufferChannels, Program, TypeId};

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
) -> Result<(), Diagnostic> {
    validate_audio_table(
        program,
        "input",
        program
            .interface
            .inputs
            .iter()
            .map(|port| (port.name.as_str(), port.ty)),
        input_ptrs.len(),
        |index| input_ptrs[index],
    )?;
    validate_audio_table(
        program,
        "output",
        program
            .interface
            .outputs
            .iter()
            .map(|port| (port.name.as_str(), port.ty)),
        output_ptrs.len(),
        |index| output_ptrs[index].cast_const(),
    )
}

fn validate_audio_table<'a, I>(
    program: &Program,
    direction: &str,
    ports: I,
    pointer_count: usize,
    mut pointer_at: impl FnMut(usize) -> *const u8,
) -> Result<(), Diagnostic>
where
    I: Iterator<Item = (&'a str, TypeId)> + Clone,
{
    let expected = ports.clone().try_fold(0usize, |count, (_, ty)| {
        let (_, width) = audio_port_shape(program, ty).ok_or_else(|| {
            Diagnostic::runtime(
                format!("native MIR {direction} interface contains a non-scalar audio port"),
                0,
                0,
            )
        })?;
        count.checked_add(width).ok_or_else(|| {
            Diagnostic::runtime(
                format!("native MIR {direction} channel count exceeds usize"),
                0,
                0,
            )
        })
    })?;
    if pointer_count != expected {
        return Err(Diagnostic::runtime(
            format!("runtime {direction} pointer count is {pointer_count}; expected {expected}"),
            0,
            0,
        ));
    }

    let mut flat_index = 0usize;
    for (name, ty) in ports {
        let (scalar, width) = audio_port_shape(program, ty)
            .expect("the first audio-interface traversal validated every port type");
        let alignment = usize::try_from(scalar_store_size(scalar))
            .expect("primitive scalar alignment fits usize");
        for channel in 0..width {
            let pointer = pointer_at(flat_index);
            if pointer.is_null() {
                return Err(audio_channel_error(
                    direction,
                    flat_index,
                    name,
                    channel,
                    width,
                    "pointer is null",
                ));
            }
            if !pointer.addr().is_multiple_of(alignment) {
                return Err(audio_channel_error(
                    direction,
                    flat_index,
                    name,
                    channel,
                    width,
                    &format!("pointer requires {alignment}-byte alignment"),
                ));
            }
            flat_index += 1;
        }
    }
    Ok(())
}

fn audio_channel_error(
    direction: &str,
    flat_index: usize,
    name: &str,
    channel: usize,
    width: usize,
    problem: &str,
) -> Diagnostic {
    let name = if width == 1 {
        name.to_owned()
    } else {
        format!("{name}[{channel}]")
    };
    Diagnostic::runtime(
        format!("runtime {direction} channel {flat_index} (`{name}`) {problem}"),
        0,
        0,
    )
}

pub(super) fn validate_buffer_abi(
    program: &Program,
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    let expected = program.interface.buffers.len();
    if buffer_ptrs.len() != expected
        || buffer_frames.len() != expected
        || buffer_channels.len() != expected
        || buffer_sample_rates.len() != expected
    {
        return Err(Diagnostic::runtime(
            format!(
                "runtime buffer metadata count mismatch: ptrs={}, frames={}, chans={}, samplerates={}, expected={expected}",
                buffer_ptrs.len(),
                buffer_frames.len(),
                buffer_channels.len(),
                buffer_sample_rates.len(),
            ),
            0,
            0,
        ));
    }

    for index in 0..expected {
        let frames = buffer_frames[index];
        let channels = buffer_channels[index];
        let pointer_is_null = buffer_ptrs[index].is_null();
        let sample_rate = buffer_sample_rates[index];
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(Diagnostic::runtime(
                format!(
                    "runtime buffer {index} requires finite positive sample rate, got {sample_rate}"
                ),
                0,
                0,
            ));
        }
        if frames <= 0 || channels <= 0 {
            return Err(Diagnostic::runtime(
                format!(
                    "runtime buffer {index} requires positive dimensions; got {frames} * {channels}"
                ),
                0,
                0,
            ));
        }

        let alignment =
            usize::try_from(scalar_store_size(program.interface.buffers[index].element))
                .expect("primitive scalar alignment fits usize");
        if !pointer_is_null && !buffer_ptrs[index].addr().is_multiple_of(alignment) {
            return Err(Diagnostic::runtime(
                format!("runtime buffer {index} pointer requires {alignment}-byte alignment"),
                0,
                0,
            ));
        }

        let expected_channels = match program.interface.buffers[index].channels {
            BufferChannels::Mono => Some(1),
            BufferChannels::Static(expected) => Some(expected),
            BufferChannels::Dynamic => None,
        };
        if let Some(expected_channels) = expected_channels {
            if u32::try_from(channels) != Ok(expected_channels) {
                return Err(Diagnostic::runtime(
                    format!(
                        "runtime buffer {index} requires {expected_channels} channels, got {channels}"
                    ),
                    0,
                    0,
                ));
            }
        }
        if frames.checked_mul(channels).is_none() {
            return Err(Diagnostic::runtime(
                format!("runtime buffer {index} element count {frames} * {channels} exceeds i32"),
                0,
                0,
            ));
        }
        let element_size =
            i32::try_from(scalar_store_size(program.interface.buffers[index].element))
                .expect("primitive scalar size fits i32");
        if frames
            .checked_mul(channels)
            .and_then(|elements| elements.checked_mul(element_size))
            .is_none()
        {
            return Err(Diagnostic::runtime(
                format!(
                    "runtime buffer {index} byte extent {frames} * {channels} * {element_size} exceeds i32"
                ),
                0,
                0,
            ));
        }
    }
    Ok(())
}
