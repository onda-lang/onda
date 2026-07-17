use onda_frontend::Diagnostic;

use crate::primitives::primitive_type_bytes;
#[cfg(feature = "llvm-orc")]
use crate::CompiledProgram;
use crate::{
    CodegenOptions, CompileOptions, DeclaredEvent, JitProgram, RuntimeAllocator, RuntimeBuffer,
    RuntimeState, TargetCpu,
};

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

pub(crate) fn validate_codegen_options(options: &CodegenOptions) -> Result<(), Diagnostic> {
    if !options.sample_rate.is_finite() || options.sample_rate <= 0.0 {
        return Err(Diagnostic::internal(
            "codegen option 'sample_rate' must be finite and greater than zero",
        ));
    }
    if options.block_size == 0 {
        return Err(Diagnostic::internal(
            "codegen option 'block_size' must be greater than zero",
        ));
    }

    if let Some(triple) = &options.target.triple {
        if triple.trim().is_empty() {
            return Err(Diagnostic::internal(
                "target config 'triple' must not be empty when provided",
            ));
        }
    }

    match &options.target.cpu {
        TargetCpu::Host => {}
        TargetCpu::Explicit(cpu) => {
            if cpu.trim().is_empty() {
                return Err(Diagnostic::internal(
                    "target config 'cpu' must not be empty when explicitly provided",
                ));
            }
        }
    }

    if let Some(features) = &options.target.features {
        if features.contains(char::is_whitespace) {
            return Err(Diagnostic::internal(
                "target config 'features' must be a comma-separated LLVM feature string without whitespace",
            ));
        }
    }

    if let Some(abi_name) = &options.target.abi_name {
        if abi_name.trim().is_empty() {
            return Err(Diagnostic::internal(
                "target config 'abi_name' must not be empty when provided",
            ));
        }
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
            let data_bytes = primitive_type_bytes(param.elem_ty())
                .checked_mul(len)
                .filter(|bytes| *bytes <= i32::MAX as usize)
                .ok_or_else(|| {
                    Diagnostic::runtime(
                        format!(
                            "event '{}' slice parameter '{}' byte extent exceeds i32 runtime limit",
                            desc.name(),
                            param.name()
                        ),
                        0,
                        0,
                    )
                })?;
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
        self.inputs
            .iter()
            .map(|input| input.slot_offset().saturating_add(input.array_len()))
            .max()
            .unwrap_or(0)
    }

    pub fn required_out_channels(&self) -> usize {
        self.outputs
            .iter()
            .map(|output| output.slot_offset().saturating_add(output.array_len()))
            .max()
            .unwrap_or(0)
    }

    pub fn default_param_bytes(&self) -> Vec<u8> {
        let mut out = vec![0_u8; self.param_byte_size()];
        for param in self.params.iter() {
            let default = param
                .default_bytes()
                .expect("validated parameter metadata has default bytes");
            let start = param.byte_offset();
            let end = start.saturating_add(default.len());
            out[start..end].copy_from_slice(default);
        }
        out
    }

    pub fn write_default_param_bytes(&self, out: &mut [u8]) -> Result<(), Diagnostic> {
        let expected = self.param_byte_size();
        if out.len() != expected {
            return Err(Diagnostic::runtime(
                "runtime parameter default storage size does not match compiled program",
                0,
                0,
            ));
        }
        for param in self.params.iter() {
            let default = param.default_bytes().ok_or_else(|| {
                Diagnostic::runtime(
                    format!("parameter '{}' has no compiled default bytes", param.name()),
                    0,
                    0,
                )
            })?;
            let start = param.byte_offset();
            let end = start.checked_add(default.len()).ok_or_else(|| {
                Diagnostic::runtime("runtime parameter default byte range overflow", 0, 0)
            })?;
            let Some(destination) = out.get_mut(start..end) else {
                return Err(Diagnostic::runtime(
                    "runtime parameter default byte range exceeds compiled storage",
                    0,
                    0,
                ));
            };
            destination.copy_from_slice(default);
        }
        Ok(())
    }

    pub fn inputs(&self) -> &[crate::DeclaredIo] {
        self.inputs.as_slice()
    }

    pub fn outputs(&self) -> &[crate::DeclaredIo] {
        self.outputs.as_slice()
    }

    pub fn control_outputs(&self) -> &[crate::DeclaredIo] {
        self.control_outputs.as_slice()
    }

    pub fn params(&self) -> &[crate::DeclaredIo] {
        self.params.as_slice()
    }

    pub fn buffers(&self) -> &[crate::DeclaredBuffer] {
        self.buffers.as_slice()
    }

    pub fn state_entries(&self) -> &[crate::DeclaredState] {
        self.state_entries.as_slice()
    }

    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }

    pub fn control_output_count(&self) -> usize {
        self.control_outputs.len()
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

    pub fn state_count(&self) -> usize {
        self.state_entries.len()
    }

    pub fn input_name(&self, index: usize) -> Option<&str> {
        self.inputs.get(index).map(|io| io.name())
    }

    pub fn output_name(&self, index: usize) -> Option<&str> {
        self.outputs.get(index).map(|io| io.name())
    }

    pub fn control_output_name(&self, index: usize) -> Option<&str> {
        self.control_outputs.get(index).map(|io| io.name())
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

    pub fn state_name(&self, index: usize) -> Option<&str> {
        self.state_entries.get(index).map(|entry| entry.name())
    }

    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.input_index.get(name).copied()
    }

    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.output_index.get(name).copied()
    }

    pub fn control_output_index(&self, name: &str) -> Option<usize> {
        self.control_output_index.get(name).copied()
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

    pub fn control_output_type(&self, index: usize) -> Option<String> {
        self.control_outputs.get(index).map(|io| io.type_repr())
    }

    pub fn param_type(&self, index: usize) -> Option<String> {
        self.params.get(index).map(|io| io.type_repr())
    }

    pub fn buffer_type(&self, index: usize) -> Option<String> {
        self.buffers.get(index).map(|buffer| buffer.type_repr())
    }

    pub fn state_type(&self, index: usize) -> Option<String> {
        self.state_entries.get(index).map(|entry| entry.type_repr())
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

    pub fn control_output_type_bytes(&self, index: usize) -> Option<usize> {
        self.control_outputs.get(index).map(|io| io.byte_size())
    }

    pub fn control_output_elem_type(&self, index: usize) -> Option<onda_frontend::PrimitiveType> {
        self.control_outputs.get(index).map(|io| io.elem_ty())
    }

    pub fn control_output_array_len(&self, index: usize) -> Option<usize> {
        self.control_outputs.get(index).map(|io| io.array_len())
    }

    pub fn control_output_slot_offset(&self, index: usize) -> Option<usize> {
        self.control_outputs.get(index).map(|io| io.slot_offset())
    }

    pub fn control_output_byte_offset(&self, index: usize) -> Option<usize> {
        self.control_outputs.get(index).map(|io| io.byte_offset())
    }

    pub fn control_output_storage_byte_offset(&self, index: usize) -> Option<usize> {
        self.control_outputs
            .get(index)
            .and_then(|io| io.state_byte_offset())
    }

    pub fn control_output_descriptor(&self, index: usize) -> Option<&crate::DeclaredIo> {
        self.control_outputs.get(index)
    }

    pub fn param_type_bytes(&self, index: usize) -> Option<usize> {
        self.params.get(index).map(|io| io.byte_size())
    }

    pub fn state_type_bytes(&self, index: usize) -> Option<usize> {
        self.state_entries.get(index).map(|entry| entry.byte_size())
    }

    pub fn param_descriptor(&self, index: usize) -> Option<&crate::DeclaredIo> {
        self.params.get(index)
    }

    pub fn param_slot_count(&self) -> usize {
        self.params.iter().map(|param| param.array_len()).sum()
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

    /// Packed, target-independent persistent snapshot size.
    pub fn state_size_bytes(&self) -> usize {
        self.snapshot_size_bytes
    }

    /// Physical target-layout state storage required by generated code.
    pub fn physical_state_size_bytes(&self) -> usize {
        #[cfg(feature = "llvm-orc")]
        {
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => compiled.state_size_bytes(),
                CompiledProgram::MirOrc(compiled) => compiled.state_size_bytes(),
            };
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            0
        }
    }

    pub fn write_state_snapshot(
        &self,
        state: &RuntimeState,
        destination: &mut [u8],
    ) -> Result<(), Diagnostic> {
        if destination.len() != self.snapshot_size_bytes {
            return Err(Diagnostic::runtime(
                format!(
                    "state snapshot byte size mismatch: expected {}, got {}",
                    self.snapshot_size_bytes,
                    destination.len()
                ),
                0,
                0,
            ));
        }
        let state_bytes = state.bytes();
        for segment in self.snapshot_segments.iter() {
            let state_end = segment
                .state_offset
                .checked_add(segment.byte_size)
                .filter(|end| *end <= state_bytes.len())
                .ok_or_else(|| {
                    Diagnostic::internal("snapshot segment exceeds physical state storage")
                })?;
            let snapshot_end = segment.snapshot_offset + segment.byte_size;
            copy_snapshot_segment(
                &state_bytes[segment.state_offset..state_end],
                &mut destination[segment.snapshot_offset..snapshot_end],
                segment.element_size,
            );
        }
        Ok(())
    }

    pub fn restore_state_snapshot(
        &self,
        state: &mut RuntimeState,
        initial_state: &RuntimeState,
        snapshot: &[u8],
    ) -> Result<(), Diagnostic> {
        if snapshot.len() != self.snapshot_size_bytes {
            return Err(Diagnostic::runtime(
                format!(
                    "state snapshot byte size mismatch: expected {}, got {}",
                    self.snapshot_size_bytes,
                    snapshot.len()
                ),
                0,
                0,
            ));
        }
        if state.byte_size() != initial_state.byte_size() {
            return Err(Diagnostic::internal(
                "initial and live physical state layouts differ",
            ));
        }
        state.bytes_mut().copy_from_slice(initial_state.bytes());
        let state_bytes = state.bytes_mut();
        for segment in self.snapshot_segments.iter() {
            let state_end = segment
                .state_offset
                .checked_add(segment.byte_size)
                .filter(|end| *end <= state_bytes.len())
                .ok_or_else(|| {
                    Diagnostic::internal("snapshot segment exceeds physical state storage")
                })?;
            let snapshot_end = segment.snapshot_offset + segment.byte_size;
            copy_snapshot_segment(
                &snapshot[segment.snapshot_offset..snapshot_end],
                &mut state_bytes[segment.state_offset..state_end],
                segment.element_size,
            );
        }
        Ok(())
    }

    pub fn event_descriptor(&self, index: usize) -> Option<&crate::DeclaredEvent> {
        self.events.get(index)
    }

    pub fn initialize_state(&self, params: &[u8]) -> Result<RuntimeState, Diagnostic> {
        self.initialize_state_with_allocator(params, None)
    }

    pub fn initialize_state_with_allocator(
        &self,
        params: &[u8],
        allocator: Option<RuntimeAllocator>,
    ) -> Result<RuntimeState, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => {
                    initialize_state_orc(compiled, params, allocator)
                }
                CompiledProgram::MirOrc(compiled) => {
                    compiled.initialize_state_with_allocator(params, allocator)
                }
            };
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (params, allocator);
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    /// Validates ABI shape before entering generated code.
    ///
    /// # Safety
    ///
    /// The raw input, output, and external-buffer pointers must remain valid,
    /// correctly sized/aligned, and mutually non-overlapping for the duration
    /// of the call.
    pub unsafe fn process_checked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: usize,
        frames: usize,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => process_checked_orc(
                    compiled,
                    state,
                    params,
                    start_frame,
                    frames,
                    flags,
                    in_ptrs,
                    out_ptrs,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                ),
                CompiledProgram::MirOrc(compiled) => unsafe {
                    compiled.process_checked(
                        state,
                        params,
                        start_frame,
                        frames,
                        flags,
                        in_ptrs,
                        out_ptrs,
                        buffer_ptrs,
                        buffer_frames,
                        buffer_channels,
                        buffer_sample_rates,
                    )
                },
            };
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                start_frame,
                frames,
                flags,
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

    /// Refreshes legacy derived buffer references from raw host tables.
    ///
    /// # Safety
    ///
    /// The raw buffer pointers and metadata must describe live host regions.
    pub unsafe fn sync_proc_buffer_refs_for_process_checked(
        &self,
        state: &mut RuntimeState,
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(all(feature = "llvm-orc", not(test)))]
        let _ = (
            state,
            buffer_ptrs,
            buffer_frames,
            buffer_channels,
            buffer_sample_rates,
        );
        #[cfg(feature = "llvm-orc")]
        {
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => {
                    sync_proc_buffer_refs_for_process_checked_orc(
                        compiled,
                        state,
                        buffer_ptrs,
                        buffer_frames,
                        buffer_channels,
                        buffer_sample_rates,
                    )
                }
                // MIR represents external buffer references as transient symbolic call values.
                // Both direct and processor-dispatched accesses consume the current validated
                // host table, so preparation has no pointer-bearing derived state to refresh.
                CompiledProgram::MirOrc(_) => Ok(()),
            };
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

    /// Enters generated process code without validating any raw ABI tables.
    ///
    /// # Safety
    ///
    /// The state and parameter storage must match this program. Frame ranges,
    /// flags, pointer counts, buffer metadata, pointee extents/alignment, and
    /// all aliasing relationships must satisfy the same invariants enforced by
    /// [`Self::process_checked`] and remain valid for the duration of the call.
    pub unsafe fn process_unchecked(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        start_frame: u32,
        frames: u32,
        flags: u32,
        in_ptrs: &[*const u8],
        out_ptrs: &[*mut u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => {
                    unsafe {
                        process_unchecked_orc(
                            compiled,
                            state,
                            params,
                            start_frame,
                            frames,
                            flags,
                            in_ptrs,
                            out_ptrs,
                            buffer_ptrs,
                            buffer_frames,
                            buffer_channels,
                            buffer_sample_rates,
                        );
                    }
                    Ok(())
                }
                CompiledProgram::MirOrc(compiled) => {
                    unsafe {
                        compiled.process_unchecked(
                            state,
                            params,
                            start_frame,
                            frames,
                            flags,
                            in_ptrs,
                            out_ptrs,
                            buffer_ptrs,
                            buffer_frames,
                            buffer_channels,
                            buffer_sample_rates,
                        );
                    }
                    Ok(())
                }
            };
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (
                state,
                params,
                start_frame,
                frames,
                flags,
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

    /// Validates payload shape before entering generated event code.
    ///
    /// # Safety
    ///
    /// Raw external-buffer pointers must satisfy their complete binding
    /// contract and remain valid for the duration of the call.
    pub unsafe fn trigger_event_by_index(
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
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => trigger_event_orc(
                    compiled,
                    state,
                    params,
                    event_index,
                    payload,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                ),
                CompiledProgram::MirOrc(compiled) => unsafe {
                    compiled.trigger_event_by_index(
                        state,
                        params,
                        event_index,
                        payload,
                        buffer_ptrs,
                        buffer_frames,
                        buffer_channels,
                        buffer_sample_rates,
                    )
                },
            };
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

    /// Enters generated event code without validating payload or buffer shape.
    ///
    /// # Safety
    ///
    /// The state, parameters, event payload, and raw external-buffer tables
    /// must satisfy the same invariants enforced by
    /// [`Self::trigger_event_by_index`] and remain valid for the duration of
    /// the call.
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
            return match &self.compiled {
                #[cfg(test)]
                CompiledProgram::LegacyOrc(compiled) => {
                    unsafe {
                        trigger_event_orc_unchecked(
                            compiled,
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
                    Ok(())
                }
                CompiledProgram::MirOrc(compiled) => {
                    unsafe {
                        compiled.trigger_event_by_index_unchecked(
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
                    Ok(())
                }
            };
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

fn copy_snapshot_segment(source: &[u8], destination: &mut [u8], element_size: usize) {
    debug_assert_eq!(source.len(), destination.len());
    debug_assert!(element_size > 0 && source.len().is_multiple_of(element_size));
    if cfg!(target_endian = "little") || element_size == 1 {
        destination.copy_from_slice(source);
        return;
    }
    for (source, destination) in source
        .chunks_exact(element_size)
        .zip(destination.chunks_exact_mut(element_size))
    {
        for (output, input) in destination.iter_mut().zip(source.iter().rev()) {
            *output = *input;
        }
    }
}

impl RuntimeState {
    pub fn byte_size(&self) -> usize {
        self.state_size_bytes
    }

    pub fn bytes(&self) -> &[u8] {
        if self.state_size_bytes == 0 {
            return &[];
        }
        // SAFETY: state_words is allocated as the backing storage for exactly
        // state_size_bytes bytes rounded up to u64 words by initialize_state.
        unsafe {
            std::slice::from_raw_parts(
                self.state_words.as_ptr().cast::<u8>(),
                self.state_size_bytes,
            )
        }
    }

    pub fn bytes_mut(&mut self) -> &mut [u8] {
        if self.state_size_bytes == 0 {
            return &mut [];
        }
        // SAFETY: state_words is uniquely borrowed here and stores at least
        // state_size_bytes bytes rounded up to u64 words.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.state_words.as_mut_ptr().cast::<u8>(),
                self.state_size_bytes,
            )
        }
    }

    pub fn copy_from_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        if bytes.len() != self.state_size_bytes {
            return Err(Diagnostic::runtime(
                format!(
                    "state snapshot byte size mismatch: expected {}, got {}",
                    self.state_size_bytes,
                    bytes.len()
                ),
                0,
                0,
            ));
        }
        self.bytes_mut().copy_from_slice(bytes);
        Ok(())
    }

    pub fn try_clone_with_allocator(
        &self,
        allocator: Option<RuntimeAllocator>,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            state_words: RuntimeBuffer::try_from_slice_in(self.state_words.as_slice(), allocator)?,
            state_size_bytes: self.state_size_bytes,
        })
    }
}

#[cfg(all(test, feature = "llvm-orc"))]
fn required_state_words(state_size_bytes: usize) -> usize {
    (state_size_bytes + 7) / 8
}

#[cfg(all(test, feature = "llvm-orc"))]
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

#[cfg(all(test, feature = "llvm-orc"))]
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

#[cfg(all(test, feature = "llvm-orc"))]
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

#[cfg(all(test, feature = "llvm-orc"))]
fn process_checked_orc(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    start_frame: usize,
    frames: usize,
    flags: u32,
    in_ptrs: &[*const u8],
    out_ptrs: &[*mut u8],
    buffer_ptrs: &[*mut u8],
    buffer_frames: &[i32],
    buffer_channels: &[i32],
    buffer_sample_rates: &[f32],
) -> Result<(), Diagnostic> {
    let start_frame = u32::try_from(start_frame).map_err(|_| {
        Diagnostic::runtime(
            "start frame does not fit u32 for ORC process entrypoint",
            0,
            0,
        )
    })?;
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
        start_frame,
        frames,
        flags,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
        buffer_sample_rates.as_ptr(),
    );
    Ok(())
}

#[cfg(all(test, feature = "llvm-orc"))]
fn sync_proc_buffer_refs_for_process_checked_orc(
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

#[cfg(all(test, feature = "llvm-orc"))]
unsafe fn process_unchecked_orc(
    compiled: &crate::orc_backend::OrcProcess,
    state: &mut RuntimeState,
    params: &[u8],
    start_frame: u32,
    frames: u32,
    flags: u32,
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
        start_frame,
        frames,
        flags,
        params.as_ptr(),
        state.state_words.as_mut_ptr().cast::<u8>(),
        buffer_ptrs.as_ptr(),
        buffer_frames.as_ptr(),
        buffer_channels.as_ptr(),
        buffer_sample_rates.as_ptr(),
    );
}

#[cfg(all(test, feature = "llvm-orc"))]
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

#[cfg(all(test, feature = "llvm-orc"))]
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

#[cfg(all(test, feature = "llvm-orc"))]
fn initialize_state_orc(
    compiled: &crate::orc_backend::OrcProcess,
    params: &[u8],
    allocator: Option<RuntimeAllocator>,
) -> Result<RuntimeState, Diagnostic> {
    validate_param_bytes(compiled, params)?;
    let state_size_bytes = compiled.state_size_bytes();
    let mut state_words =
        RuntimeBuffer::try_from_elem_in(required_state_words(state_size_bytes), 0_u64, allocator)?;
    compiled.run_init(params.as_ptr(), state_words.as_mut_ptr().cast::<u8>());
    Ok(RuntimeState {
        state_words,
        state_size_bytes,
    })
}
