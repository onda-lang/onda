use super::*;

pub(super) type OrcProcessFn = unsafe extern "C" fn(
    *const *const u8,
    *const *mut u8,
    u32,
    *const u8,
    *mut u8,
    *const *mut u8,
    *const i32,
    *const i32,
    *const f32,
);
pub(super) type OrcInitFn = unsafe extern "C" fn(*const u8, *mut u8);
pub(super) type OrcEventFn =
    unsafe extern "C" fn(
        *const u8,
        *const u8,
        *mut u8,
        *const *mut u8,
        *const i32,
        *const i32,
        *const f32,
    );

pub(super) fn is_proc_glue_function_name(name: &str) -> bool {
    name.contains(".__proc_")
}

#[derive(Debug)]
pub(crate) struct OrcProcess {
    lljit: LLVMOrcLLJITRef,
    process_fn: OrcProcessFn,
    init_fn: OrcInitFn,
    event_fns: Vec<OrcEventFn>,
    param_size_bytes: usize,
    in_channels: usize,
    out_channels: usize,
    buffer_count: usize,
    state_size_bytes: usize,
    proc_slot_buffer_ref_layouts: Vec<ProcSlotBufferRefLayout>,
}

impl OrcProcess {
    pub(crate) fn new(
        lljit: LLVMOrcLLJITRef,
        process_fn: OrcProcessFn,
        init_fn: OrcInitFn,
        event_fns: Vec<OrcEventFn>,
        param_size_bytes: usize,
        in_channels: usize,
        out_channels: usize,
        buffer_count: usize,
        state_size_bytes: usize,
        proc_slot_buffer_ref_layouts: Vec<ProcSlotBufferRefLayout>,
    ) -> Self {
        Self {
            lljit,
            process_fn,
            init_fn,
            event_fns,
            param_size_bytes,
            in_channels,
            out_channels,
            buffer_count,
            state_size_bytes,
            proc_slot_buffer_ref_layouts,
        }
    }

    pub(crate) fn state_size_bytes(&self) -> usize {
        self.state_size_bytes
    }

    pub(crate) fn param_size_bytes(&self) -> usize {
        self.param_size_bytes
    }

    pub(crate) fn in_channels(&self) -> usize {
        self.in_channels
    }

    pub(crate) fn out_channels(&self) -> usize {
        self.out_channels
    }

    pub(crate) fn buffer_count(&self) -> usize {
        self.buffer_count
    }

    pub(crate) fn run(
        &self,
        in_ptrs: *const *const u8,
        out_ptrs: *const *mut u8,
        frames: u32,
        params: *const u8,
        state: *mut u8,
        buffer_ptrs: *const *mut u8,
        buffer_frames: *const i32,
        buffer_channels: *const i32,
        buffer_sample_rates: *const f32,
    ) {
        unsafe {
            (self.process_fn)(
                in_ptrs,
                out_ptrs,
                frames,
                params,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
        }
    }

    pub(crate) fn run_init(&self, params: *const u8, state: *mut u8) {
        unsafe {
            (self.init_fn)(params, state);
        }
    }

    pub(crate) fn run_event(
        &self,
        event_index: u32,
        payload: *const u8,
        params: *const u8,
        state: *mut u8,
        buffer_ptrs: *const *mut u8,
        buffer_frames: *const i32,
        buffer_channels: *const i32,
        buffer_sample_rates: *const f32,
    ) {
        let Some(fn_ref) = self.event_fns.get(event_index as usize).copied() else {
            return;
        };
        unsafe {
            (fn_ref)(
                payload,
                params,
                state,
                buffer_ptrs,
                buffer_frames,
                buffer_channels,
                buffer_sample_rates,
            );
        }
    }

    pub(crate) fn sync_proc_buffer_refs(
        &self,
        state: *mut u8,
        buffer_ptrs: &[*mut u8],
        buffer_frames: &[i32],
        buffer_channels: &[i32],
        buffer_sample_rates: &[f32],
    ) -> Result<(), Diagnostic> {
        if self.proc_slot_buffer_ref_layouts.is_empty() {
            return Ok(());
        }
        if state.is_null() {
            return Err(Diagnostic::runtime(
                "runtime state pointer is null during proc buffer-ref sync",
                0,
                0,
            ));
        }

        for layout in &self.proc_slot_buffer_ref_layouts {
            for (slot_idx, buffer_idx) in layout.slot_buffer_indices.iter().enumerate() {
                let Some(ptr_value) = buffer_ptrs.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer pointer index",
                    ));
                };
                let Some(frames_value) = buffer_frames.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer frames index",
                    ));
                };
                let Some(channels_value) = buffer_channels.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer channels index",
                    ));
                };
                let Some(sample_rate_value) = buffer_sample_rates.get(*buffer_idx).copied() else {
                    return Err(Diagnostic::internal(
                        "proc buffer-ref sync mapping references out-of-range buffer sample-rate index",
                    ));
                };

                let ptr_byte_off = layout
                    .ptr_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<usize>()));
                let frames_byte_off = layout
                    .frames_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<i32>()));
                let channels_byte_off = layout
                    .channels_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<i32>()));
                let samplerate_byte_off = layout
                    .samplerate_offset
                    .saturating_add(slot_idx.saturating_mul(size_of::<f32>()));

                unsafe {
                    std::ptr::write_unaligned(
                        state.add(ptr_byte_off).cast::<usize>(),
                        ptr_value as usize,
                    );
                    std::ptr::write_unaligned(
                        state.add(frames_byte_off).cast::<i32>(),
                        frames_value,
                    );
                    std::ptr::write_unaligned(
                        state.add(channels_byte_off).cast::<i32>(),
                        channels_value,
                    );
                    std::ptr::write_unaligned(
                        state.add(samplerate_byte_off).cast::<f32>(),
                        sample_rate_value,
                    );
                }
            }
        }

        Ok(())
    }
}

impl Drop for OrcProcess {
    fn drop(&mut self) {
        unsafe {
            let err = LLVMOrcDisposeLLJIT(self.lljit);
            if !err.is_null() {
                let msg_ptr = LLVMGetErrorMessage(err);
                if !msg_ptr.is_null() {
                    LLVMDisposeErrorMessage(msg_ptr);
                }
            }
        }
    }
}
