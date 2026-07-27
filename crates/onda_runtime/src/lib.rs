use onda_codegen_llvm::{
    DeclaredBufferChannels, JitProgram, RuntimeAllocator, RuntimeBuffer, RuntimeState,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_realtime::configure_current_thread_audio_fp_mode;

pub use onda_codegen_llvm::{ParamDomain, ParamScalarType, ParamScale};

pub const PROCESS_BEGIN_BLOCK: u32 = 1 << 0;
pub const PROCESS_END_BLOCK: u32 = 1 << 1;
pub const PROCESS_FULL_BLOCK: u32 = PROCESS_BEGIN_BLOCK | PROCESS_END_BLOCK;

#[derive(Debug, Clone, Copy)]
pub struct InstanceConfig {
    pub sample_rate: f32,
    pub frames_per_block: usize,
    pub in_channels: usize,
    pub out_channels: usize,
}

#[derive(Debug)]
pub struct Instance {
    pub(crate) program: JitProgram,
    pub(crate) config: InstanceConfig,
    pub(crate) params: RuntimeBuffer<u8>,
    pub(crate) state: RuntimeState,
    pub(crate) initial_state: RuntimeState,
    pub(crate) input_bindings: RuntimeBuffer<Option<BoundInput>>,
    pub(crate) output_bindings: RuntimeBuffer<Option<BoundOutput>>,
    pub(crate) buffer_bindings: RuntimeBuffer<Option<BoundBuffer>>,
    pub(crate) input_ptrs: RuntimeBuffer<*const u8>,
    pub(crate) output_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_ptrs: RuntimeBuffer<*mut u8>,
    pub(crate) buffer_frames: RuntimeBuffer<i32>,
    pub(crate) buffer_channels: RuntimeBuffer<i32>,
    pub(crate) buffer_sample_rates: RuntimeBuffer<f32>,
    pub(crate) inputs_validated: bool,
    pub(crate) outputs_validated: bool,
    pub(crate) buffers_validated: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundInput {
    ptr: *const u8,
    bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundOutput {
    ptr: *mut u8,
    bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct BoundBuffer {
    ptr: *mut u8,
    frames_i32: i32,
    channels_i32: i32,
    sample_rate_hz: f32,
}

impl Instance {
    pub fn in_channels(&self) -> usize {
        self.config.in_channels
    }

    pub fn out_channels(&self) -> usize {
        self.config.out_channels
    }

    pub fn input_count(&self) -> usize {
        self.program.input_count()
    }

    pub fn output_count(&self) -> usize {
        self.program.output_count()
    }

    pub fn control_output_count(&self) -> usize {
        self.program.control_output_count()
    }

    pub fn param_count(&self) -> usize {
        self.program.param_count()
    }

    pub fn buffer_count(&self) -> usize {
        self.program.buffer_count()
    }

    pub fn event_count(&self) -> usize {
        self.program.event_count()
    }

    pub fn state_count(&self) -> usize {
        self.program.state_count()
    }

    pub fn input_name(&self, index: usize) -> Option<&str> {
        self.program.input_name(index)
    }

    pub fn output_name(&self, index: usize) -> Option<&str> {
        self.program.output_name(index)
    }

    pub fn control_output_name(&self, index: usize) -> Option<&str> {
        self.program.control_output_name(index)
    }

    pub fn param_name(&self, index: usize) -> Option<&str> {
        self.program.param_name(index)
    }

    pub fn param_domain(&self, index: usize) -> Option<ParamDomain<'_>> {
        self.program.param_domain(index)
    }

    pub fn buffer_name(&self, index: usize) -> Option<&str> {
        self.program.buffer_name(index)
    }

    pub fn event_name(&self, index: usize) -> Option<&str> {
        self.program.event_name(index)
    }

    pub fn state_name(&self, index: usize) -> Option<&str> {
        self.program.state_name(index)
    }

    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.program.input_index(name)
    }

    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.program.output_index(name)
    }

    pub fn control_output_index(&self, name: &str) -> Option<usize> {
        self.program.control_output_index(name)
    }

    pub fn param_index(&self, name: &str) -> Option<usize> {
        self.program.param_index(name)
    }

    pub fn buffer_index(&self, name: &str) -> Option<usize> {
        self.program.buffer_index(name)
    }

    pub fn event_index(&self, name: &str) -> Option<usize> {
        self.program.event_index(name)
    }

    pub fn state_type(&self, index: usize) -> Option<String> {
        self.program.state_type(index)
    }

    pub fn input_type(&self, index: usize) -> Option<String> {
        self.program.input_type(index)
    }

    pub fn output_type(&self, index: usize) -> Option<String> {
        self.program.output_type(index)
    }

    pub fn control_output_type(&self, index: usize) -> Option<String> {
        self.program.control_output_type(index)
    }

    pub fn param_type(&self, index: usize) -> Option<String> {
        self.program.param_type(index)
    }

    pub fn buffer_type(&self, index: usize) -> Option<String> {
        self.program.buffer_type(index)
    }

    pub fn input_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.input_type_bytes(index)
    }

    pub fn output_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.output_type_bytes(index)
    }

    pub fn control_output_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.control_output_type_bytes(index)
    }

    pub fn control_output_elem_type(&self, index: usize) -> Option<PrimitiveType> {
        self.program.control_output_elem_type(index)
    }

    pub fn control_output_array_len(&self, index: usize) -> Option<usize> {
        self.program.control_output_array_len(index)
    }

    pub fn control_output_slot_offset(&self, index: usize) -> Option<usize> {
        self.program.control_output_slot_offset(index)
    }

    pub fn control_output_byte_offset(&self, index: usize) -> Option<usize> {
        self.program.control_output_byte_offset(index)
    }

    pub fn param_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.param_type_bytes(index)
    }

    pub fn event_payload_bytes(&self, index: usize) -> Option<usize> {
        self.program.event_payload_bytes(index)
    }

    pub fn state_type_bytes(&self, index: usize) -> Option<usize> {
        self.program.state_type_bytes(index)
    }

    pub fn state_size_bytes(&self) -> usize {
        self.program.state_size_bytes()
    }

    pub fn snapshot_state_bytes(&self) -> Vec<u8> {
        let mut snapshot = vec![0_u8; self.program.state_size_bytes()];
        self.program
            .write_state_snapshot(&self.state, &mut snapshot)
            .expect("instance snapshot buffer matches compiled snapshot layout");
        snapshot
    }

    pub fn write_snapshot_state_bytes(&self, destination: &mut [u8]) -> Result<(), Diagnostic> {
        self.program.write_state_snapshot(&self.state, destination)
    }

    pub fn restore_state_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        self.program
            .restore_state_snapshot(&mut self.state, &self.initial_state, bytes)?;
        self.buffers_validated = false;
        Ok(())
    }
}

pub fn create_instance(
    program: JitProgram,
    config: InstanceConfig,
) -> Result<Instance, Diagnostic> {
    create_instance_inner(program, config, None)
}

pub fn create_instance_with_allocator(
    program: JitProgram,
    config: InstanceConfig,
    allocator: RuntimeAllocator,
) -> Result<Instance, Diagnostic> {
    create_instance_inner(program, config, Some(allocator))
}

fn create_instance_inner(
    program: JitProgram,
    config: InstanceConfig,
    allocator: Option<RuntimeAllocator>,
) -> Result<Instance, Diagnostic> {
    if !config.sample_rate.is_finite() || config.sample_rate <= 0.0 {
        return Err(Diagnostic::runtime(
            "instance sample_rate must be finite and greater than zero",
            0,
            0,
        ));
    }
    if config.sample_rate.to_bits() != program.sample_rate().to_bits() {
        return Err(Diagnostic::runtime(
            format!(
                "instance sample_rate ({}) must match program compile-time sample rate ({})",
                config.sample_rate,
                program.sample_rate(),
            ),
            0,
            0,
        ));
    }
    if config.frames_per_block == 0 {
        return Err(Diagnostic::runtime(
            "frames_per_block must be greater than zero",
            0,
            0,
        ));
    }
    if config.out_channels == 0 && program.required_out_channels() > 0 {
        return Err(Diagnostic::runtime(
            "out_channels must be greater than zero when the program has audio outputs",
            0,
            0,
        ));
    }
    if config.in_channels < program.required_in_channels() {
        return Err(Diagnostic::runtime(
            "configured input channels are fewer than program inputs",
            0,
            0,
        ));
    }
    if config.out_channels < program.required_out_channels() {
        return Err(Diagnostic::runtime(
            "configured output channels are fewer than program outputs",
            0,
            0,
        ));
    }

    if config.frames_per_block != program.block_size() {
        return Err(Diagnostic::runtime(
            format!(
                "instance frames_per_block ({}) must match program compile-time block size ({})",
                config.frames_per_block,
                program.block_size()
            ),
            0,
            0,
        ));
    }
    u32::try_from(config.frames_per_block).map_err(|_| {
        Diagnostic::runtime(
            "frames_per_block does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;

    let required_in_channels = program.required_in_channels();
    let required_out_channels = program.required_out_channels();

    let mut params = RuntimeBuffer::try_from_elem_in(program.param_byte_size(), 0_u8, allocator)?;
    program.write_default_param_bytes(&mut params)?;
    configure_current_thread_audio_fp_mode();
    let state = program.initialize_state_with_allocator(&params, allocator)?;
    let initial_state = state.try_clone_with_allocator(allocator)?;

    let input_count = program.input_count();
    let output_count = program.output_count();
    let buffer_count = program.buffer_count();
    Ok(Instance {
        program,
        config,
        params,
        state,
        initial_state,
        input_bindings: RuntimeBuffer::try_from_elem_in(input_count, None, allocator)?,
        output_bindings: RuntimeBuffer::try_from_elem_in(output_count, None, allocator)?,
        buffer_bindings: RuntimeBuffer::try_from_elem_in(buffer_count, None, allocator)?,
        input_ptrs: RuntimeBuffer::try_from_elem_in(
            required_in_channels,
            std::ptr::null(),
            allocator,
        )?,
        output_ptrs: RuntimeBuffer::try_from_elem_in(
            required_out_channels,
            std::ptr::null_mut(),
            allocator,
        )?,
        buffer_ptrs: RuntimeBuffer::try_from_elem_in(
            buffer_count,
            std::ptr::null_mut(),
            allocator,
        )?,
        buffer_frames: RuntimeBuffer::try_from_elem_in(buffer_count, 0_i32, allocator)?,
        buffer_channels: RuntimeBuffer::try_from_elem_in(buffer_count, 0_i32, allocator)?,
        buffer_sample_rates: RuntimeBuffer::try_from_elem_in(buffer_count, 0.0_f32, allocator)?,
        inputs_validated: required_in_channels == 0,
        outputs_validated: required_out_channels == 0,
        buffers_validated: buffer_count == 0,
    })
}

pub fn reset_instance_state(instance: &mut Instance) {
    instance
        .state
        .bytes_mut()
        .copy_from_slice(instance.initial_state.bytes());
    instance.buffers_validated = false;
}

pub fn set_param_by_index(
    instance: &mut Instance,
    index: usize,
    value_bytes: &[u8],
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    let expected_bytes = desc.byte_size();
    if value_bytes.len() != expected_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "parameter '{}' expects {} bytes, got {}",
                desc.name(),
                expected_bytes,
                value_bytes.len()
            ),
            0,
            0,
        ));
    }
    let start = desc.byte_offset();
    let end = start.saturating_add(expected_bytes);
    if end > instance.params.len() {
        return Err(Diagnostic::runtime(
            format!(
                "parameter '{}' byte range [{start}, {end}) is out of bounds for runtime storage ({})",
                desc.name(),
                instance.params.len()
            ),
            0,
            0,
        ));
    }
    instance.params[start..end].copy_from_slice(value_bytes);
    Ok(())
}

pub fn set_param_plain_f64(
    instance: &mut Instance,
    index: usize,
    plain: f64,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    if desc.is_array() {
        return Err(Diagnostic::runtime(
            format!("parameter '{}' is not a scalar", desc.name()),
            0,
            0,
        ));
    }
    let value = match desc.elem_ty() {
        PrimitiveType::Bool => {
            return set_param_by_index(instance, index, &[u8::from(plain >= 0.5)]);
        }
        _ => desc
            .param_domain()
            .map(|domain| domain.constrain_plain(plain))
            .ok_or_else(|| {
                Diagnostic::runtime(
                    format!("parameter '{}' has no numeric control domain", desc.name()),
                    0,
                    0,
                )
            })?,
    };
    set_scalar_param_f64(instance, index, desc.elem_ty(), value)
}

fn set_scalar_param_f64(
    instance: &mut Instance,
    index: usize,
    ty: PrimitiveType,
    value: f64,
) -> Result<(), Diagnostic> {
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
        PrimitiveType::I32 => {
            bytes[..4].copy_from_slice(&(value.round() as i32).to_ne_bytes());
            4
        }
        PrimitiveType::I64 => {
            bytes.copy_from_slice(&(value.round() as i64).to_ne_bytes());
            8
        }
        PrimitiveType::Bool => unreachable!(),
    };
    set_param_by_index(instance, index, &bytes[..len])
}

pub fn set_param_normalized(
    instance: &mut Instance,
    index: usize,
    normalized: f64,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.param_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown parameter index {index}"),
            0,
            0,
        ));
    };
    if desc.elem_ty() == PrimitiveType::Bool && !desc.is_array() {
        return set_param_by_index(instance, index, &[u8::from(normalized >= 0.5)]);
    }
    let plain = desc
        .param_domain()
        .map(|domain| domain.normalized_to_plain(normalized))
        .ok_or_else(|| {
            Diagnostic::runtime(
                format!("parameter '{}' has no numeric control domain", desc.name()),
                0,
                0,
            )
        })?;
    set_scalar_param_f64(instance, index, desc.elem_ty(), plain)
}

pub fn read_control_output_bytes(
    instance: &Instance,
    index: usize,
    out: &mut [u8],
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.control_output_descriptor(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown control output index {index}"),
            0,
            0,
        ));
    };
    let expected_bytes = desc.byte_size();
    if out.len() != expected_bytes {
        return Err(Diagnostic::runtime(
            format!(
                "control output '{}' expects {} destination bytes, got {}",
                desc.name(),
                expected_bytes,
                out.len()
            ),
            0,
            0,
        ));
    }
    let Some(start) = instance.program.control_output_storage_byte_offset(index) else {
        return Err(Diagnostic::runtime(
            format!("control output '{}' has no runtime storage", desc.name()),
            0,
            0,
        ));
    };
    let end = start.saturating_add(expected_bytes);
    let state = instance.state.bytes();
    if end > state.len() {
        return Err(Diagnostic::runtime(
            format!(
                "control output '{}' byte range [{start}, {end}) is out of bounds for runtime storage ({})",
                desc.name(),
                state.len()
            ),
            0,
            0,
        ));
    }
    out.copy_from_slice(&state[start..end]);
    Ok(())
}

/// Binds borrowed host input memory without copying it.
///
/// # Safety
///
/// `ptr` must remain readable for `bytes` bytes at a stable address until the
/// slot is rebound/unbound or the instance is destroyed. The pointer must have
/// the natural alignment of the declared primitive element type; this function
/// validates the address before retaining it. Bound input, output, and
/// external-buffer regions must not overlap while processing.
pub unsafe fn bind_input(
    instance: &mut Instance,
    index: usize,
    ptr: *const u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.inputs().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown input index {index}"),
            0,
            0,
        ));
    };
    if ptr.is_null() {
        if bytes == 0 {
            instance.input_bindings[index] = None;
            instance.inputs_validated = false;
            return Ok(());
        }
        return Err(Diagnostic::runtime(
            format!("input '{}' binding pointer is null", desc.name()),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr, desc.elem_ty(), "input", desc.name())?;
    let expected = desc
        .byte_size()
        .saturating_mul(instance.config.frames_per_block);
    if bytes != expected {
        return Err(Diagnostic::runtime(
            format!(
                "input '{}' expects {} bytes for one block, got {}",
                desc.name(),
                expected,
                bytes
            ),
            0,
            0,
        ));
    }
    instance.input_bindings[index] = Some(BoundInput { ptr, bytes });
    instance.inputs_validated = false;
    Ok(())
}

/// Binds borrowed host output memory without copying it.
///
/// # Safety
///
/// `ptr` must remain writable for `bytes` bytes at a stable address until the
/// slot is rebound/unbound or the instance is destroyed. The pointer must have
/// the natural alignment of the declared primitive element type; this function
/// validates the address before retaining it. Bound input, output, and
/// external-buffer regions must not overlap while processing.
pub unsafe fn bind_output(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    bytes: usize,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.outputs().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown output index {index}"),
            0,
            0,
        ));
    };
    if ptr.is_null() {
        if bytes == 0 {
            instance.output_bindings[index] = None;
            instance.outputs_validated = false;
            return Ok(());
        }
        return Err(Diagnostic::runtime(
            format!("output '{}' binding pointer is null", desc.name()),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr.cast_const(), desc.elem_ty(), "output", desc.name())?;
    let expected = desc
        .byte_size()
        .saturating_mul(instance.config.frames_per_block);
    if bytes != expected {
        return Err(Diagnostic::runtime(
            format!(
                "output '{}' expects {} bytes for one block, got {}",
                desc.name(),
                expected,
                bytes
            ),
            0,
            0,
        ));
    }
    instance.output_bindings[index] = Some(BoundOutput { ptr, bytes });
    instance.outputs_validated = false;
    Ok(())
}

/// Binds borrowed external-buffer memory without copying it.
///
/// A zero `sample_rate_hz` unbinds the slot regardless of pointer and shape. A null pointer with
/// zero frames and channels also unbinds the slot. Otherwise the binding must be nonempty and
/// `sample_rate_hz` must be finite and positive.
///
/// # Safety
///
/// When this call binds the slot, `ptr` must remain valid for `frames * channels` elements of
/// `elem_ty`, with the element's required alignment, until rebound/unbound or instance destruction.
/// The region must be writable when the declaration permits writes, and all bound host regions must
/// be mutually non-overlapping while processing. Unbind calls do not access `ptr`.
pub unsafe fn bind_buffer(
    instance: &mut Instance,
    index: usize,
    ptr: *mut u8,
    frames: usize,
    channels: usize,
    sample_rate_hz: f32,
    elem_ty: PrimitiveType,
) -> Result<(), Diagnostic> {
    let Some(desc) = instance.program.buffers().get(index) else {
        return Err(Diagnostic::runtime(
            format!("unknown buffer index {index}"),
            0,
            0,
        ));
    };
    if sample_rate_hz == 0.0 || (ptr.is_null() && frames == 0 && channels == 0) {
        instance.buffer_bindings[index] = None;
        instance.buffers_validated = false;
        return Ok(());
    }
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' requires finite sample_rate > 0, got {}",
                desc.name(),
                sample_rate_hz
            ),
            0,
            0,
        ));
    }
    if elem_ty != desc.elem_ty() {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' element type mismatch: expected {:?}, got {:?}",
                desc.name(),
                desc.elem_ty(),
                elem_ty
            ),
            0,
            0,
        ));
    }
    if frames == 0 || channels == 0 || ptr.is_null() {
        return Err(Diagnostic::runtime(
            format!(
                "buffer '{}' must be unbound with null + zero frames/channels or bound with a non-null pointer and positive frames/channels",
                desc.name()
            ),
            0,
            0,
        ));
    }
    validate_pointer_alignment(ptr.cast_const(), elem_ty, "buffer", desc.name())?;
    match desc.channels() {
        DeclaredBufferChannels::Mono => {
            if channels != 1 {
                return Err(Diagnostic::runtime(
                    format!(
                        "buffer '{}' expects mono (1 channel), got {}",
                        desc.name(),
                        channels
                    ),
                    0,
                    0,
                ));
            }
        }
        DeclaredBufferChannels::Static(expected) => {
            if channels != expected {
                return Err(Diagnostic::runtime(
                    format!(
                        "buffer '{}' expects {} channels, got {}",
                        desc.name(),
                        expected,
                        channels
                    ),
                    0,
                    0,
                ));
            }
        }
        DeclaredBufferChannels::Dynamic => {}
    }
    let frames_i32 = i32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            format!(
                "buffer '{}' frames {} exceed i32 runtime limit",
                desc.name(),
                frames
            ),
            0,
            0,
        )
    })?;
    let channels_i32 = i32::try_from(channels).map_err(|_| {
        Diagnostic::runtime(
            format!(
                "buffer '{}' channels {} exceed i32 runtime limit",
                desc.name(),
                channels
            ),
            0,
            0,
        )
    })?;
    validate_buffer_byte_extent(frames_i32, channels_i32, elem_ty, desc.name())?;
    instance.buffer_bindings[index] = Some(BoundBuffer {
        ptr,
        frames_i32,
        channels_i32,
        sample_rate_hz,
    });
    instance.buffers_validated = false;
    Ok(())
}

pub fn validate_inputs(instance: &mut Instance) -> Result<(), Diagnostic> {
    let frames = instance.config.frames_per_block;
    prepare_input_ptrs_from_bindings(instance, frames)?;
    instance.inputs_validated = true;
    Ok(())
}

pub fn validate_outputs(instance: &mut Instance) -> Result<(), Diagnostic> {
    let frames = instance.config.frames_per_block;
    for (out_idx, desc) in instance.program.outputs().iter().enumerate() {
        let Some(binding) = instance.output_bindings.get(out_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required output '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        let expected = desc.byte_size().saturating_mul(frames);
        if binding.bytes != expected {
            return Err(Diagnostic::runtime(
                format!(
                    "output '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    expected
                ),
                0,
                0,
            ));
        }
    }
    prepare_output_ptrs_for_process(instance, frames)?;
    instance.outputs_validated = true;
    Ok(())
}

pub fn validate_buffers(instance: &mut Instance) -> Result<(), Diagnostic> {
    prepare_buffer_ptrs_from_bindings(instance)?;
    instance.buffers_validated = true;
    Ok(())
}

pub fn validate_bindings(instance: &mut Instance) -> Result<(), Diagnostic> {
    validate_buffers(instance)?;
    validate_inputs(instance)?;
    validate_outputs(instance)?;
    Ok(())
}

pub fn process_checked(instance: &mut Instance, frames: usize) -> Result<(), Diagnostic> {
    process_checked_segment(instance, 0, frames, PROCESS_FULL_BLOCK)
}

pub fn process_checked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    validate_process_request(instance, start_frame, frames, flags)?;
    validate_bindings_for_process(instance)?;
    sync_proc_buffer_refs_for_process(instance)?;
    unsafe {
        instance.program.process_checked(
            &mut instance.state,
            &instance.params,
            start_frame,
            frames,
            flags,
            &instance.input_ptrs,
            &instance.output_ptrs,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
        )?;
    }
    Ok(())
}

/// Validates the current host bindings and completes backend-specific preparation for unchecked
/// processing.
///
/// This is not a stale-binding snapshot operation. Call it again after rebinding before entering
/// an unchecked processing loop; MIR backends consume the current validated buffer table directly.
pub fn prepare_unchecked_process(instance: &mut Instance) -> Result<(), Diagnostic> {
    validate_bindings_for_process(instance)?;
    sync_proc_buffer_refs_for_process(instance)
}

/// Processes one complete block without revalidating host bindings.
///
/// # Safety
///
/// The instance's input, output, and buffer bindings must have been successfully prepared with
/// [`prepare_unchecked_process`] (or the equivalent validation functions) after their most recent
/// mutation. Every bound region must remain valid, correctly sized, and appropriately aligned for
/// the duration of this call, without aliases that violate Rust's memory rules.
pub unsafe fn process_unchecked(instance: &mut Instance) -> Result<(), Diagnostic> {
    unsafe {
        process_unchecked_segment(
            instance,
            0,
            instance.config.frames_per_block,
            PROCESS_FULL_BLOCK,
        )
    }
}

/// Processes a segment of the configured block without revalidating host bindings.
///
/// # Safety
///
/// The instance's input, output, and buffer bindings must have been successfully prepared with
/// [`prepare_unchecked_process`] (or the equivalent validation functions) after their most recent
/// mutation. Every bound region must remain valid, correctly sized, and appropriately aligned for
/// the duration of this call, without aliases that violate Rust's memory rules.
pub unsafe fn process_unchecked_segment(
    instance: &mut Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    validate_process_request(instance, start_frame, frames, flags)?;
    debug_assert!(
        instance.inputs_validated && instance.outputs_validated && instance.buffers_validated,
        "process_unchecked called without validating required input/output/buffer bindings; this is UB in release builds"
    );
    let start_frame = u32::try_from(start_frame).map_err(|_| {
        Diagnostic::runtime(
            "start frame does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;
    let frames = u32::try_from(frames).map_err(|_| {
        Diagnostic::runtime(
            "frame count does not fit u32 runtime/JIT process entrypoint",
            0,
            0,
        )
    })?;
    unsafe {
        instance.program.process_unchecked(
            &mut instance.state,
            &instance.params,
            start_frame,
            frames,
            flags,
            &instance.input_ptrs,
            &instance.output_ptrs,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
        )?;
    }
    Ok(())
}

fn validate_process_request(
    instance: &Instance,
    start_frame: usize,
    frames: usize,
    flags: u32,
) -> Result<(), Diagnostic> {
    let Some(end_frame) = start_frame.checked_add(frames) else {
        return Err(Diagnostic::runtime(
            "segment frame range overflows usize",
            0,
            0,
        ));
    };
    if end_frame > instance.config.frames_per_block {
        return Err(Diagnostic::runtime(
            "segment start frame + frame count must be less than or equal to fixed instance block size",
            0,
            0,
        ));
    }
    let unknown_flags = flags & !PROCESS_FULL_BLOCK;
    if unknown_flags != 0 {
        return Err(Diagnostic::runtime(
            format!("unknown process flags 0x{unknown_flags:x}"),
            0,
            0,
        ));
    }
    Ok(())
}

fn validate_bindings_for_process(instance: &mut Instance) -> Result<(), Diagnostic> {
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    if !instance.inputs_validated {
        validate_inputs(instance)?;
    }
    if !instance.outputs_validated {
        validate_outputs(instance)?;
    }
    Ok(())
}

fn sync_proc_buffer_refs_for_process(instance: &mut Instance) -> Result<(), Diagnostic> {
    unsafe {
        instance.program.sync_proc_buffer_refs_for_process_checked(
            &mut instance.state,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
        )
    }
}

pub fn trigger_event_by_index(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    if !instance.buffers_validated {
        validate_buffers(instance)?;
    }
    unsafe {
        instance.program.trigger_event_by_index(
            &mut instance.state,
            &instance.params,
            event_index,
            payload,
            &instance.buffer_ptrs,
            &instance.buffer_frames,
            &instance.buffer_channels,
            &instance.buffer_sample_rates,
        )
    }
}

/// Dispatches an event without validating its payload or current buffer bindings.
///
/// # Safety
///
/// Buffer bindings must have been validated after their most recent mutation and must remain valid
/// for the call. `payload` must exactly match the declared fixed or dynamic layout for
/// `event_index`, including all slice length prefixes and element data.
pub unsafe fn trigger_event_by_index_unchecked(
    instance: &mut Instance,
    event_index: usize,
    payload: &[u8],
) -> Result<(), Diagnostic> {
    configure_current_thread_audio_fp_mode();
    debug_assert!(
        instance.buffers_validated,
        "trigger_event_by_index_unchecked called without validating required buffer bindings"
    );
    instance.program.trigger_event_by_index_unchecked(
        &mut instance.state,
        &instance.params,
        event_index,
        payload,
        &instance.buffer_ptrs,
        &instance.buffer_frames,
        &instance.buffer_channels,
        &instance.buffer_sample_rates,
    )
}

fn prepare_buffer_ptrs_from_bindings(instance: &mut Instance) -> Result<(), Diagnostic> {
    for (idx, desc) in instance.program.buffers().iter().enumerate() {
        let Some(bound) = instance.buffer_bindings.get(idx).and_then(|v| *v) else {
            return Err(Diagnostic::runtime(
                format!("required buffer '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        instance.buffer_ptrs[idx] = bound.ptr;
        instance.buffer_frames[idx] = bound.frames_i32;
        instance.buffer_channels[idx] = bound.channels_i32;
        instance.buffer_sample_rates[idx] = bound.sample_rate_hz;
    }
    Ok(())
}

fn primitive_type_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn primitive_type_alignment(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 => std::mem::align_of::<f32>(),
        PrimitiveType::F64 => std::mem::align_of::<f64>(),
        PrimitiveType::I32 => std::mem::align_of::<i32>(),
        PrimitiveType::I64 => std::mem::align_of::<i64>(),
        PrimitiveType::Bool => std::mem::align_of::<u8>(),
    }
}

fn validate_pointer_alignment(
    ptr: *const u8,
    ty: PrimitiveType,
    surface: &str,
    name: &str,
) -> Result<(), Diagnostic> {
    let required = primitive_type_alignment(ty);
    if (ptr as usize).is_multiple_of(required) {
        return Ok(());
    }
    Err(Diagnostic::runtime(
        format!("{surface} '{name}' binding pointer requires {required}-byte alignment for {ty:?}"),
        0,
        0,
    ))
}

fn validate_buffer_byte_extent(
    frames: i32,
    channels: i32,
    element: PrimitiveType,
    name: &str,
) -> Result<i32, Diagnostic> {
    let element_size =
        i32::try_from(primitive_type_bytes(element)).expect("primitive element sizes fit i32");
    frames
        .checked_mul(channels)
        .and_then(|elements| elements.checked_mul(element_size))
        .ok_or_else(|| {
        Diagnostic::runtime(
            format!(
                "buffer '{name}' byte extent {frames} * {channels} * {element_size} exceeds i32 runtime limit"
            ),
            0,
            0,
        )
    })
}

fn prepare_input_ptrs_from_bindings(
    instance: &mut Instance,
    frames: usize,
) -> Result<(), Diagnostic> {
    let required_in_channels = instance.program.required_in_channels();
    if instance.input_ptrs.len() != required_in_channels {
        return Err(Diagnostic::runtime(
            "runtime input channel pointer storage does not match compiled program",
            0,
            0,
        ));
    }

    for (in_idx, desc) in instance.program.inputs().iter().enumerate() {
        let Some(binding) = instance.input_bindings.get(in_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required input '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        if binding.bytes != desc.byte_size().saturating_mul(frames) {
            return Err(Diagnostic::runtime(
                format!(
                    "input '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    desc.byte_size().saturating_mul(frames)
                ),
                0,
                0,
            ));
        }
        let elem_bytes = primitive_type_bytes(desc.elem_ty());
        for ch in 0..desc.array_len() {
            let in_channel = desc.slot_offset().saturating_add(ch);
            let src_off = ch.saturating_mul(frames).saturating_mul(elem_bytes);
            instance.input_ptrs[in_channel] = unsafe { binding.ptr.add(src_off) };
        }
    }
    Ok(())
}

fn prepare_output_ptrs_for_process(
    instance: &mut Instance,
    frames: usize,
) -> Result<(), Diagnostic> {
    let required_out_channels = instance.program.required_out_channels();
    if instance.output_ptrs.len() != required_out_channels {
        return Err(Diagnostic::runtime(
            "runtime output channel pointer storage does not match compiled program",
            0,
            0,
        ));
    }
    instance.output_ptrs.fill(std::ptr::null_mut());

    for (out_idx, desc) in instance.program.outputs().iter().enumerate() {
        let Some(binding) = instance.output_bindings.get(out_idx).and_then(|b| *b) else {
            return Err(Diagnostic::runtime(
                format!("required output '{}' is not bound", desc.name()),
                0,
                0,
            ));
        };
        if binding.bytes != desc.byte_size().saturating_mul(frames) {
            return Err(Diagnostic::runtime(
                format!(
                    "output '{}' bound buffer size {} does not match expected {}",
                    desc.name(),
                    binding.bytes,
                    desc.byte_size().saturating_mul(frames)
                ),
                0,
                0,
            ));
        }
        let elem_bytes = primitive_type_bytes(desc.elem_ty());
        for ch in 0..desc.array_len() {
            let out_channel = desc.slot_offset().saturating_add(ch);
            let dst_off = ch.saturating_mul(frames).saturating_mul(elem_bytes);
            instance.output_ptrs[out_channel] = unsafe { binding.ptr.add(dst_off) };
        }
    }
    for desc in instance.program.outputs() {
        for ch in 0..desc.array_len() {
            let out_channel = desc.slot_offset().saturating_add(ch);
            if instance.output_ptrs[out_channel].is_null() {
                return Err(Diagnostic::runtime(
                    format!("output '{}' channel pointer is null", desc.name()),
                    0,
                    0,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_bindings_reject_misaligned_primitive_addresses() {
        let storage = [0_u64; 2];
        let aligned = storage.as_ptr().cast::<u8>();
        let misaligned = unsafe { aligned.add(1) };

        for ty in [
            PrimitiveType::F32,
            PrimitiveType::F64,
            PrimitiveType::I32,
            PrimitiveType::I64,
        ] {
            validate_pointer_alignment(aligned, ty, "test", "value")
                .expect("u64 storage should satisfy primitive alignment");
            let error = validate_pointer_alignment(misaligned, ty, "test", "value")
                .expect_err("offset byte pointer must be rejected");
            assert!(error.message.contains("alignment"));
        }
        validate_pointer_alignment(misaligned, PrimitiveType::Bool, "test", "value")
            .expect("byte elements have alignment one");
    }

    #[test]
    fn checked_buffer_bindings_reject_wrapping_element_counts() {
        let error = validate_buffer_byte_extent(i32::MAX, 2, PrimitiveType::F32, "huge")
            .expect_err("buffer byte extent must fit the generated i32 ABI");
        assert!(error.message.contains("exceeds i32 runtime limit"));
        let f64_error =
            validate_buffer_byte_extent(i32::MAX / 8 + 1, 1, PrimitiveType::F64, "wide")
                .expect_err("f64 byte extent must fit i32 even when element count does");
        assert!(f64_error.message.contains("byte extent"));
        assert_eq!(
            validate_buffer_byte_extent(1024, 2, PrimitiveType::F32, "ok").unwrap(),
            8192
        );
    }
}
