//! Native processor storage and conversion at the host's f32 audio boundary.
use onda_codegen_llvm::DeclaredIo;
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_project::BufferSamples;
use onda_runtime::{bind_input, bind_output, Instance};

#[derive(Debug)]
pub(super) struct AudioPort {
    samples: BufferSamples,
    frames: usize,
    slot_offset: usize,
}

impl AudioPort {
    pub fn new(descriptor: &DeclaredIo, frames: usize) -> Result<Self, Diagnostic> {
        let len = frames.checked_mul(descriptor.array_len()).ok_or_else(|| {
            Diagnostic::runtime(
                format!("audio port '{}' size overflows usize", descriptor.name()),
                0,
                0,
            )
        })?;
        let samples = match descriptor.elem_ty() {
            PrimitiveType::F32 => zeroed(len).map(BufferSamples::F32),
            PrimitiveType::F64 => zeroed(len).map(BufferSamples::F64),
            PrimitiveType::I32 | PrimitiveType::Enum(_) => zeroed(len).map(BufferSamples::I32),
            PrimitiveType::I64 => zeroed(len).map(BufferSamples::I64),
            PrimitiveType::Bool => zeroed(len).map(BufferSamples::Bool),
        }
        .map_err(|error| {
            Diagnostic::runtime(
                format!(
                    "cannot allocate audio port '{}': {error}",
                    descriptor.name()
                ),
                0,
                0,
            )
        })?;
        Ok(Self {
            samples,
            frames,
            slot_offset: descriptor.slot_offset(),
        })
    }

    fn byte_size(&self) -> usize {
        self.samples.len() * self.samples.element().byte_size()
    }

    pub fn bind_input(&self, instance: &mut Instance, index: usize) -> Result<(), Diagnostic> {
        // The session owns this naturally aligned allocation until the instance is dropped.
        unsafe { bind_input(instance, index, self.samples.as_ptr(), self.byte_size())? };
        Ok(())
    }

    pub fn bind_output(&mut self, instance: &mut Instance, index: usize) -> Result<(), Diagnostic> {
        let bytes = self.byte_size();
        // The allocation remains writable and stable across processing and instance rebuilds.
        unsafe { bind_output(instance, index, self.samples.as_mut_ptr(), bytes)? };
        Ok(())
    }

    pub fn clear(&mut self) {
        match &mut self.samples {
            BufferSamples::F32(values) => values.fill(0.0),
            BufferSamples::F64(values) => values.fill(0.0),
            BufferSamples::I32(values) => values.fill(0),
            BufferSamples::I64(values) => values.fill(0),
            BufferSamples::Bool(values) => values.fill(0),
        }
    }

    pub fn copy_input(&mut self, input: &[f32], channels: usize) {
        let frames = self.frames;
        let offset = self.slot_offset;
        // Rust's float-to-integer casts truncate and saturate; NaN becomes zero.
        match &mut self.samples {
            BufferSamples::F32(values) => {
                copy_input(values, frames, offset, input, channels, |x| x)
            }
            BufferSamples::F64(values) => {
                copy_input(values, frames, offset, input, channels, f64::from)
            }
            BufferSamples::I32(values) => {
                copy_input(values, frames, offset, input, channels, |x| x as i32)
            }
            BufferSamples::I64(values) => {
                copy_input(values, frames, offset, input, channels, |x| x as i64)
            }
            BufferSamples::Bool(values) => {
                copy_input(values, frames, offset, input, channels, |x| {
                    u8::from(x != 0.0)
                })
            }
        }
    }

    pub fn copy_output(&self, output: &mut [f32], channels: usize) {
        let frames = self.frames;
        let offset = self.slot_offset;
        match &self.samples {
            BufferSamples::F32(values) => {
                copy_output(values, frames, offset, output, channels, |x| x)
            }
            BufferSamples::F64(values) => {
                copy_output(values, frames, offset, output, channels, |x| x as f32)
            }
            BufferSamples::I32(values) => {
                copy_output(values, frames, offset, output, channels, |x| x as f32)
            }
            BufferSamples::I64(values) => {
                copy_output(values, frames, offset, output, channels, |x| x as f32)
            }
            BufferSamples::Bool(values) => {
                copy_output(values, frames, offset, output, channels, |x| {
                    f32::from(x != 0)
                })
            }
        }
    }
}

fn zeroed<T: Default + Clone>(len: usize) -> Result<Vec<T>, std::collections::TryReserveError> {
    let mut values = Vec::new();
    values.try_reserve_exact(len)?;
    values.resize(len, T::default());
    Ok(values)
}

fn copy_input<T: Copy + Default>(
    values: &mut [T],
    frames: usize,
    offset: usize,
    input: &[f32],
    channels: usize,
    convert: impl Fn(f32) -> T,
) {
    values.fill(T::default());
    if channels == 0 {
        return;
    }
    for (channel, values) in values.chunks_exact_mut(frames).enumerate() {
        let channel = offset + channel;
        if channel < channels {
            for (target, source) in values.iter_mut().zip(input.chunks_exact(channels)) {
                *target = convert(source[channel]);
            }
        }
    }
}

fn copy_output<T: Copy>(
    values: &[T],
    frames: usize,
    offset: usize,
    output: &mut [f32],
    channels: usize,
    convert: impl Fn(T) -> f32,
) {
    for (channel, values) in values.chunks_exact(frames).enumerate() {
        for (source, target) in values.iter().zip(output.chunks_exact_mut(channels)) {
            target[offset + channel] = convert(*source);
        }
    }
}
