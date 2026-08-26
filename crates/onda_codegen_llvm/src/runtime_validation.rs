use onda_frontend::Diagnostic;

use crate::primitives::primitive_type_bytes;
use crate::{
    DeclaredEvent, JitProgram, RuntimeAllocator, RuntimeBuffer, RuntimeState,
    UninitializedRuntimeState,
};

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

    pub fn buffer_arrays(&self) -> &[crate::DeclaredBufferArray] {
        self.buffer_arrays.as_slice()
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

    pub fn param_domain(&self, index: usize) -> Option<crate::ParamDomain<'_>> {
        self.param_descriptor(index)?.param_domain()
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
            self.compiled.state_size_bytes()
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
        params: &[u8],
        state: &mut RuntimeState,
        snapshot: &[u8],
    ) -> Result<(), Diagnostic> {
        self.validate_state_snapshot(snapshot)?;
        self.initialize_state_in_place(params, state, true)?;
        self.overlay_state_snapshot(state, snapshot)
    }

    pub fn validate_state_snapshot(&self, snapshot: &[u8]) -> Result<(), Diagnostic> {
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
        Ok(())
    }

    /// Overlays a validated portable snapshot onto an already fully initialized state image.
    pub fn overlay_state_snapshot(
        &self,
        state: &mut RuntimeState,
        snapshot: &[u8],
    ) -> Result<(), Diagnostic> {
        self.validate_state_snapshot(snapshot)?;
        // SAFETY: the only externally supplied bytes are copied and normalized
        // before this function returns the state to its caller.
        let state_bytes = unsafe { state.bytes_mut() };
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
            if let Some(range) = segment.integer_range {
                normalize_integer_snapshot_value(
                    &mut state_bytes[segment.state_offset..state_end],
                    range,
                );
            }
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
            self.compiled
                .initialize_state_with_allocator(params, allocator)
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (params, allocator);
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn allocate_state_with_allocator(
        &self,
        allocator: Option<RuntimeAllocator>,
    ) -> Result<UninitializedRuntimeState, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            self.compiled.allocate_state_with_allocator(allocator)
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = allocator;
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn initialize_allocated_state(
        &self,
        params: &[u8],
        state: &mut UninitializedRuntimeState,
    ) -> Result<RuntimeState, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            self.compiled.initialize_allocated_state(params, state)
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (params, state);
            Err(Diagnostic::internal(
                "ORC backend is required but not enabled at build time",
            ))
        }
    }

    pub fn initialize_state_in_place(
        &self,
        params: &[u8],
        state: &mut RuntimeState,
        all: bool,
    ) -> Result<(), Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            self.compiled.initialize_state_in_place(params, state, all)
        }
        #[cfg(not(feature = "llvm-orc"))]
        {
            let _ = (params, state, all);
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
    /// of the call. External-buffer descriptor tables must remain immutable
    /// and must not overlap state, parameter, audio, or external-buffer sample
    /// storage.
    #[allow(clippy::too_many_arguments)]
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
            unsafe {
                self.compiled.process_checked(
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
            }
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

    /// Enters generated process code without validating any raw ABI tables.
    ///
    /// # Safety
    ///
    /// The state and parameter storage must match this program. Frame ranges,
    /// flags, pointer counts, buffer metadata, pointee extents/alignment, and
    /// all aliasing relationships must satisfy the same invariants enforced by
    /// [`Self::process_checked`] and remain valid for the duration of the call.
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<u32, Diagnostic> {
        #[cfg(feature = "llvm-orc")]
        {
            Ok(unsafe {
                self.compiled.process_unchecked(
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
            })
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
    #[allow(clippy::too_many_arguments)]
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
        let status = unsafe {
            self.trigger_event_by_index_with_status(
                state,
                params,
                event_index,
                payload,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            )?
        };
        crate::check_execution_status(status)
    }

    /// Validates payload and buffer shape, then returns the generated execution status.
    /// Validation errors are returned before generated event code is entered.
    ///
    /// # Safety
    ///
    /// Raw external-buffer pointers must satisfy their complete binding
    /// contract and remain valid for the duration of the call.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn trigger_event_by_index_with_status(
        &self,
        state: &mut RuntimeState,
        params: &[u8],
        event_index: usize,
        payload: &[u8],
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<u32, Diagnostic> {
        let Some(desc) = self.event_descriptor(event_index) else {
            return Ok(0);
        };
        validate_event_payload(desc, payload)?;
        #[cfg(feature = "llvm-orc")]
        {
            unsafe {
                self.compiled.trigger_event_by_index_with_status(
                    state,
                    params,
                    event_index,
                    payload,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                )
            }
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
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<u32, Diagnostic> {
        if self.event_descriptor(event_index).is_none() {
            return Ok(0);
        }
        #[cfg(feature = "llvm-orc")]
        {
            Ok(unsafe {
                self.compiled.trigger_event_by_index_unchecked(
                    state,
                    params,
                    event_index,
                    payload,
                    buffer_ptrs,
                    buffer_frames,
                    buffer_channels,
                    buffer_sample_rates,
                )
            })
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

fn normalize_integer_snapshot_value(bytes: &mut [u8], range: onda_mir::IntegerRangeInvariant) {
    use onda_mir::{IntegerRangeMode, ScalarValue};

    fn normalize(value: i128, min: i128, max: i128, mode: IntegerRangeMode) -> i128 {
        match mode {
            IntegerRangeMode::Clamp => value.clamp(min, max),
            IntegerRangeMode::Wrap => min + (value - min).rem_euclid(max - min + 1),
        }
    }

    match (range.min, range.max) {
        (ScalarValue::I32(min), ScalarValue::I32(max)) => {
            let raw = i32::from_ne_bytes(bytes.try_into().expect("validated i32 state size"));
            let value = normalize(
                i128::from(raw),
                i128::from(min),
                i128::from(max),
                range.mode,
            );
            bytes.copy_from_slice(&(value as i32).to_ne_bytes());
        }
        (ScalarValue::I64(min), ScalarValue::I64(max)) => {
            let raw = i64::from_ne_bytes(bytes.try_into().expect("validated i64 state size"));
            let value = normalize(
                i128::from(raw),
                i128::from(min),
                i128::from(max),
                range.mode,
            );
            bytes.copy_from_slice(&(value as i64).to_ne_bytes());
        }
        _ => unreachable!("validated integer storage range has matching integer endpoints"),
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

    /// Returns the physical state image without preserving compiler-declared
    /// integer storage invariants.
    ///
    /// # Safety
    ///
    /// Before the next processor entry, the caller must ensure every ranged
    /// state slot contains a value inside its declared inclusive interval.
    pub unsafe fn bytes_mut(&mut self) -> &mut [u8] {
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

    /// Replaces the physical state image without normalizing ranged slots.
    ///
    /// # Safety
    ///
    /// `bytes` must satisfy every compiler-declared state invariant before the
    /// next processor entry. Snapshot restoration should normally be used.
    pub unsafe fn copy_from_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
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
        unsafe { self.bytes_mut() }.copy_from_slice(bytes);
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
