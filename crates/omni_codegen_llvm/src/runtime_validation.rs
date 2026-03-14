use omni_frontend::Diagnostic;

use crate::primitives::{append_typed_const_bytes, primitive_type_bytes};
use crate::{CompileOptions, DeclaredEvent, JitProgram, RuntimeState};

pub(crate) fn validate_compile_options(options: &CompileOptions) -> Result<(), Diagnostic> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(Diagnostic::internal(
            "compile option 'sample_rate' must be finite and greater than zero",
        ));
    }
    if options.block_size == 0 {
        return Err(Diagnostic::internal(
            "compile option 'block_size' must be greater than zero",
        ));
    }
    Ok(())
}

pub(crate) fn validate_event_payload(
    desc: &DeclaredEvent,
    payload: &[u8],
) -> Result<(), Diagnostic> {
    if let Some(expected) = desc.payload_bytes() {
        if payload.len() != expected {
            return Err(Diagnostic::runtime(
                format!(
                    "event '{}' expects {} payload bytes, got {}",
                    desc.name(),
                    expected,
                    payload.len()
                ),
                0,
                0,
            ));
        }
        return Ok(());
    }

    let mut offset = 0usize;
    for param in desc.params() {
        if param.is_slice() {
            if payload.len().saturating_sub(offset) < std::mem::size_of::<i32>() {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated before slice parameter '{}'",
                        desc.name(),
                        param.name()
                    ),
                    0,
                    0,
                ));
            }
            let len_bytes: [u8; 4] = payload[offset..offset + 4]
                .try_into()
                .expect("slice len bytes");
            let len = i32::from_ne_bytes(len_bytes);
            if len < 0 {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' slice parameter '{}' has negative length {}",
                        desc.name(),
                        param.name(),
                        len
                    ),
                    0,
                    0,
                ));
            }
            let len = len as usize;
            let data_bytes = primitive_type_bytes(param.elem_ty()).saturating_mul(len);
            offset = offset.saturating_add(4);
            if payload.len().saturating_sub(offset) < data_bytes {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated in slice parameter '{}'; expected {} element bytes after length prefix",
                        desc.name(),
                        param.name(),
                        data_bytes
                    ),
                    0,
                    0,
                ));
            }
            offset = offset.saturating_add(data_bytes);
        } else {
            let bytes = param.byte_size().unwrap_or(0);
            if payload.len().saturating_sub(offset) < bytes {
                return Err(Diagnostic::runtime(
                    format!(
                        "event '{}' payload is truncated in parameter '{}'; expected {} bytes",
                        desc.name(),
                        param.name(),
                        bytes
                    ),
                    0,
                    0,
                ));
            }
            offset = offset.saturating_add(bytes);
        }
    }

    if offset != payload.len() {
        return Err(Diagnostic::runtime(
            format!(
                "event '{}' expects {} payload bytes for its dynamic layout, got {}",
                desc.name(),
                offset,
                payload.len()
            ),
            0,
            0,
        ));
    }

    Ok(())
}

impl JitProgram {
    pub fn required_in_channels(&self) -> usize {
        self.typed.ins.len()
    }

    pub fn required_out_channels(&self) -> usize {
        self.typed.outs.len()
    }

    pub fn default_param_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for param in &self.typed.params {
            append_typed_const_bytes(&mut out, param.default, param.ty);
        }
        out
    }

    pub fn inputs(&self) -> &[crate::DeclaredIo] {
        self.inputs.as_slice()
    }

    pub fn outputs(&self) -> &[crate::DeclaredIo] {
        self.outputs.as_slice()
    }

    pub fn params(&self) -> &[crate::DeclaredIo] {
        self.params.as_slice()
    }

    pub fn buffers(&self) -> &[crate::DeclaredBuffer] {
        self.buffers.as_slice()
    }

    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    pub fn input_name(&self, index: usize) -> Option<&str> {
        self.inputs.get(index).map(|io| io.name())
    }

    pub fn output_name(&self, index: usize) -> Option<&str> {
        self.outputs.get(index).map(|io| io.name())
    }

    pub fn param_name(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(|io| io.name())
    }

    pub fn buffer_name(&self, index: usize) -> Option<&str> {
        self.buffers.get(index).map(|buffer| buffer.name())
    }

    pub fn event_name(&self, index: usize) -> Option<&str> {
        self.events.get(index).map(|event| event.name())
    }

    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.input_index.get(name).copied()
    }

    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.output_index.get(name).copied()
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.param_index.get(name).copied()
    }

    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.buffer_index.get(name).copied()
    }

    pub fn event_index(&self, name: &str) -> Option<usize> {
        self.event_index.get(name).copied()
    }

    pub fn input_type(&self, index: usize) -> Option<String> {
        self.inputs.get(index).map(|io| io.type_repr())
    }

    pub fn output_type(&self, index: usize) -> Option<String> {
        self.outputs.get(index).map(|io| io.type_repr())
    }

    pub fn param_type(&self, index: usize) -> Option<String> {
        self.params.get(index).map(|io| io.type_repr())
    }

    pub fn buffer_type(&self, index: usize) -> Option<String> {
        self.buffers.get(index).map(|buffer| buffer.type_repr())
    }

    pub fn event_payload_bytes(&self, index: usize) -> Option<usize> {
        self.events
            .get(index)
            .and_then(|event| event.payload_bytes())
    }

    pub fn input_type_bytes(&self, index: usize) -> Option<usize> {
        self.inputs.get(index).map(|io| io.byte_size())
    }

    pub fn output_type_bytes(&self, index: usize) -> Option<usize> {
        self.outputs.get(index).map(|io| io.byte_size())
    }

    pub fn param_type_bytes(&self, index: usize) -> Option<usize> {
        self.params.get(index).map(|io| io.byte_size())
    }

    pub fn param_descriptor(&self, index: usize) -> Option<&crate::DeclaredIo> {
        self.params.get(index)
    }

    pub fn param_slot_count(&self) -> usize {
        self.typed.params.len()
    }

    pub fn param_byte_size(&self) -> usize {
        self.params.iter().map(|param| param.byte_size()).sum()
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn event_descriptor(&self, index: usize) -> Option<&crate::DeclaredEvent> {
        self.events.get(index)
    }

    pub fn initialize_state(&self, params: &[u8]) -> Result<RuntimeState, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return initialize_state_orc(&self.compiled, params);
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = params;
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn process_bound(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        frames: usize,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return process_bound_orc(
                &self.compiled,
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn sync_proc_buffer_refs_for_process_bound(
        &self,
        state: &mut RuntimeState,
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return sync_proc_buffer_refs_for_process_bound_orc(
                &self.compiled,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub unsafe fn process_bound_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        frames: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            process_bound_orc_unchecked(
                &self.compiled,
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            return Ok(());
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                frames,
                in_ptrs,
                out_ptrs,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn trigger_event_by_index(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        let Some(desc) = self.event_descriptor(event_index) else {
            return Ok(());
        };
        validate_event_payload(desc, payload)?;
        #[cfg(feature = "llvm-orc")]
        {
            return trigger_event_orc(
                &self.compiled,
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub unsafe fn trigger_event_by_index_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        if self.event_descriptor(event_index).is_none() {
            return Ok(());
        }
        #[cfg(feature = "llvm-orc")]
        {
            trigger_event_orc_unchecked(
                &self.compiled,
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            return Ok(());
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }
}

#[cfg(feature = "llvm-orc")]
fn required_state_words(state_size_bytes: usize) -> usize {
    (state_size_bytes + 7) / 8
}

#[cfg(feature = "llvm-orc")]
fn validate_buffer_metadata_counts(
    compiled: &crate::orc_backend::OrcProcess,
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    if buffer_ptrs.len() != compiled.buffer_count()
        || buffer_frames.len() != compiled.buffer_count()
        || buffer_channels.len() != compiled.buffer_count()
        || buffer_sample_rates.len() != compiled.buffer_count()
    {
        return Err(Diagnostic::runtime(
            format!(
                "runtime buffer metadata count mismatch: ptrs={}, frames={}, chans={}, samplerates={}, expected={}",
                buffer_ptrs.len(),
                buffer_frames.len(),
                buffer_channels.len(),
                buffer_sample_rates.len(),
                compiled.buffer_count()
            ),
            0,
            0,
        ));
    }
    Ok(())
}

#[cfg(feature = "llvm-orc")]
fn validate_runtime_state(
    compiled: &crate::orc_backend::OrcProcess,
    state: &RuntimeState,
) -> Result<(), Diagnostic> {
    if state.state_size_bytes != compiled.state_size_bytes() {
        return Err(Diagnostic::runtime(
            "runtime state buffer size does not match compiled program state layout",
            0,
            0,
        ));
    }
    if state.state_words.len() < required_state_words(state.state_size_bytes) {
        return Err(Diagnostic::runtime(
            "runtime state backing storage is smaller than required by compiled program",
            0,
            0,
        ));
    }
    Ok(())
}

#[cfg(feature = "llvm-orc")]
fn validate_param_bytes(
    compiled: &crate::orc_backend::OrcProcess,
    params: &[u8],
) -> Result<(), Diagnostic> {
    let expected_param_bytes = compiled.param_size_bytes();
    if params.len() != expected_param_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "runtime parameter byte count {} does not match compiled program ({expected_param_bytes})",
                params.len()
            ),
            0,
            0,
        ));
    }
    Ok(())
}

#[cfg(feature = "llvm-orc")]
fn process_bound_orc(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    frames: usize,
    in_ptrs: &[*const u8],
    out_ptrs: &[*mut u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    let frames = u32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            "frame count does not fit u32 for ORC process entrypoint",
            0,
            0,
        )
    })?;
    if in_ptrs.len() != compiled.in_channels() {
        return Err(Diagnostic::runtime(
            format!(
                "runtime input channel pointer count {} does not match compiled program ({})",
                in_ptrs.len(),
                compiled.in_channels()
            ),
            0,
            0,
        ));
    }
    if out_ptrs.len() != compiled.out_channels() {
        return Err(Diagnostic::runtime(
            format!(
                "runtime output channel pointer count {} does not match compiled program ({})",
                out_ptrs.len(),
                compiled.out_channels()
            ),
            0,
            0,
        ));
    }
    validate_buffer_metadata_counts(
        compiled,
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates,
    )?;
    validate_runtime_state(compiled, state)?;
    validate_param_bytes(compiled, params)?;

    compiled.run(
        in_ptrs.as_ptr(),
        out_ptrs.as_ptr(),
        frames,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
        buffer_sample_rates.as_ptr(),
    );
    Ok(())
}

#[cfg(feature = "llvm-orc")]
fn sync_proc_buffer_refs_for_process_bound_orc(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    validate_buffer_metadata_counts(
        compiled,
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates,
    )?;
    validate_runtime_state(compiled, state)?;
    compiled.sync_proc_buffer_refs(
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates,
    )?;
    Ok(())
}

#[cfg(feature = "llvm-orc")]
unsafe fn process_bound_orc_unchecked(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    frames: u32,
    in_ptrs: &[*const u8],
    out_ptrs: &[*mut u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) {
    compiled.run(
        in_ptrs.as_ptr(),
        out_ptrs.as_ptr(),
        frames,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
        buffer_sample_rates.as_ptr(),
    );
}

#[cfg(feature = "llvm-orc")]
fn trigger_event_orc(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    event_index: usize,
    payload: &[u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    validate_buffer_metadata_counts(
        compiled,
        buffer_ptrs,
        buffer_frames,
        buffer_channels,
        buffer_sample_rates,
    )?;
    validate_runtime_state(compiled, state)?;
    validate_param_bytes(compiled, params)?;

    let event_index_u32 = u32::try_from(event_index).map_err(|_| {
        Diagnostic::runtime(
            "event index does not fit u32 for ORC event entrypoint",
            0,
            0,
        )
    })?;
    compiled.run_event(
        event_index_u32,
        payload.as_ptr(),
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
        buffer_sample_rates.as_ptr(),
    );
    Ok(())
}

#[cfg(feature = "llvm-orc")]
unsafe fn trigger_event_orc_unchecked(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    event_index: usize,
    payload: &[u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) {
    if let Ok(event_index_u32) = u32::try_from(event_index) {
        compiled.run_event(
            event_index_u32,
            payload.as_ptr(),
            params.as_ptr(),
            state.state_words.as_mut_ptr().cast::<u8>(),
            buffer_ptrs.as_ptr(),
            buffer_frames.as_ptr(),
            buffer_channels.as_ptr(),
            buffer_sample_rates.as_ptr(),
        );
    }
}

#[cfg(feature = "llvm-orc")]
fn initialize_state_orc(
    compiled: &crate::orc_backend::OrcProcess,
    params: &[u8],
) -> Result<RuntimeState, Diagnostic> {
    validate_param_bytes(compiled, params)?;
    let state_size_bytes = compiled.state_size_bytes();
    let mut state_words = vec![0_u64; required_state_words(state_size_bytes)];
    compiled.run_init(params.as_ptr(), state_words.as_mut_ptr().cast::<u8>());
    Ok(RuntimeState {
        state_words,
        state_size_bytes,
    })
}
