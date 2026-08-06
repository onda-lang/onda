use std::path::{Path, PathBuf};
use std::{collections::HashMap, mem};

use onda_codegen_llvm::{
    check_execution_status, jit_program_from_optimized_mir_with_options, DeclaredBufferChannels,
    DeclaredEvent, DeclaredEventParam, JitProgram, MirCompileOptions, TargetOptLevel,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_project::{BufferAsset, BufferElement, BufferSamples, ProjectLimits};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, prepare_unchecked_process,
    process_unchecked_segment, set_param_by_index, trigger_event_by_index, Instance,
    InstanceConfig,
};
use onda_semantics::{AnalysisOptions, TypedProgram};

use onda_semantics::{normalize_session_path, AnalysisSession, DocumentVersion};

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub sample_rate: f32,
    pub block_size: usize,
    pub float_param_smoothing_ms: f64,
    pub fast_math: bool,
    pub opt_level: TargetOptLevel,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            block_size: 512,
            float_param_smoothing_ms: 20.0,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        }
    }
}

impl RunOptions {
    pub fn analysis_options(&self) -> AnalysisOptions {
        AnalysisOptions {
            sample_rate: self.sample_rate,
            block_size: self.block_size,
        }
    }

    pub fn mir_compile_options(&self) -> MirCompileOptions {
        MirCompileOptions {
            fast_math: self.fast_math,
            opt_level: self.opt_level,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunParamInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
    pub value: Option<f64>,
    pub default: Option<f64>,
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    pub scale: Option<String>,
    pub curve: Option<f64>,
    pub unit: Option<String>,
    pub step: Option<f64>,
    pub step_count: Option<u32>,
    pub scalar: bool,
}

#[derive(Debug, Clone)]
pub struct RunBufferInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
    pub channels: RunBufferChannels,
    pub loaded_path: Option<String>,
    pub loaded_frames: Option<usize>,
    pub loaded_channels: Option<usize>,
    pub loaded_sample_rate_hz: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct RunEventInfo {
    pub index: usize,
    pub name: String,
    pub params: Vec<RunEventParamInfo>,
}

#[derive(Debug, Clone)]
pub struct RunEventParamInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
    pub value: RunEventValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEventValue {
    Bool(bool),
    Number(f64),
    Array(Vec<RunEventValue>),
}

#[derive(Debug, Clone, Copy)]
pub enum RunBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug)]
pub enum RunBuildError {
    Diagnostics(Vec<Diagnostic>),
    Runtime(Diagnostic),
}

#[derive(Debug)]
pub struct RunSession {
    path: PathBuf,
    version: Option<DocumentVersion>,
    options: RunOptions,
    typed: TypedProgram,
    jit: JitProgram,
    instance: Instance,
    param_values: HashMap<String, f64>,
    param_runtime_values: HashMap<String, f64>,
    buffer_bindings: Vec<Option<RunBufferBinding>>,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
}

#[derive(Debug)]
struct RunBufferBinding {
    samples: BufferSamples,
    frames: usize,
    channels: usize,
    sample_rate_hz: f32,
    loaded_path: Option<PathBuf>,
}

struct BufferBindingReplacement<'a> {
    index: usize,
    binding: Option<&'a mut RunBufferBinding>,
}

impl RunSession {
    pub fn build(
        analysis: &AnalysisSession,
        path: impl AsRef<Path>,
        options: RunOptions,
    ) -> Result<Self, RunBuildError> {
        let path = normalize_session_path(path.as_ref());
        let snapshot = analysis.analyze_document(&path, options.analysis_options());
        let version = snapshot.version;
        let Some(typed) = snapshot.typed else {
            return Err(RunBuildError::Diagnostics(snapshot.diagnostics));
        };
        let Some(mir) = snapshot.mir else {
            return Err(RunBuildError::Diagnostics(vec![Diagnostic::internal(
                "analysis succeeded without producing executable MIR",
            )]));
        };
        let jit = jit_program_from_optimized_mir_with_options(mir, options.mir_compile_options())
            .map_err(RunBuildError::Diagnostics)?;

        let config = InstanceConfig {
            sample_rate: options.sample_rate,
            frames_per_block: options.block_size,
            in_channels: jit.required_in_channels(),
            out_channels: jit.required_out_channels(),
        };
        let mut instance = create_instance(jit.clone(), config).map_err(RunBuildError::Runtime)?;

        let mut input_buffers = jit
            .inputs()
            .iter()
            .map(|desc| vec![0.0_f32; options.block_size.saturating_mul(desc.array_len())])
            .collect::<Vec<_>>();
        for (index, buffer) in input_buffers.iter_mut().enumerate() {
            unsafe {
                bind_input(
                    &mut instance,
                    index,
                    buffer.as_ptr().cast::<u8>(),
                    std::mem::size_of_val(buffer.as_slice()),
                )
            }
            .map_err(RunBuildError::Runtime)?;
        }

        let mut output_buffers = jit
            .outputs()
            .iter()
            .map(|desc| vec![0.0_f32; options.block_size.saturating_mul(desc.array_len())])
            .collect::<Vec<_>>();
        for (index, buffer) in output_buffers.iter_mut().enumerate() {
            unsafe {
                bind_output(
                    &mut instance,
                    index,
                    buffer.as_mut_ptr().cast::<u8>(),
                    std::mem::size_of_val(buffer.as_slice()),
                )
            }
            .map_err(RunBuildError::Runtime)?;
        }

        let buffer_bindings = std::iter::repeat_with(|| None)
            .take(jit.buffer_count())
            .collect::<Vec<_>>();
        prepare_unchecked_process(&mut instance).map_err(RunBuildError::Runtime)?;

        Ok(Self {
            path,
            version,
            options,
            typed,
            jit,
            instance,
            param_values: HashMap::new(),
            param_runtime_values: HashMap::new(),
            buffer_bindings,
            input_buffers,
            output_buffers,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn version(&self) -> Option<DocumentVersion> {
        self.version
    }

    pub fn options(&self) -> RunOptions {
        self.options
    }

    pub fn typed_program(&self) -> &TypedProgram {
        &self.typed
    }

    pub fn output_channel_count(&self) -> usize {
        self.jit.required_out_channels()
    }

    pub fn input_channel_count(&self) -> usize {
        self.jit.required_in_channels()
    }

    pub fn param_info(&self) -> Vec<RunParamInfo> {
        (0..self.jit.param_count())
            .filter_map(|index| {
                let desc = self.jit.param_descriptor(index)?;
                let value = self
                    .param_values
                    .get(desc.name())
                    .copied()
                    .or_else(|| desc.default_as_f64());
                let domain = desc.param_domain();
                Some(RunParamInfo {
                    index,
                    name: desc.name().to_owned(),
                    type_repr: desc.type_repr(),
                    value,
                    default: desc.default_as_f64(),
                    range_min: desc.range_min_as_f64(),
                    range_max: desc.range_max_as_f64(),
                    scale: domain.map(|domain| domain.scale_name().to_owned()),
                    curve: domain.and_then(|domain| domain.curve()),
                    unit: domain
                        .and_then(|domain| domain.unit())
                        .map(ToOwned::to_owned),
                    step: domain.and_then(|domain| domain.step()),
                    step_count: domain.and_then(|domain| domain.step_count()),
                    scalar: desc.array_len() == 1,
                })
            })
            .collect()
    }

    pub fn buffer_info(&self) -> Vec<RunBufferInfo> {
        self.jit
            .buffers()
            .iter()
            .enumerate()
            .map(|(index, desc)| {
                let binding = self.buffer_bindings.get(index).and_then(Option::as_ref);
                RunBufferInfo {
                    index,
                    name: desc.name().to_owned(),
                    type_repr: desc.type_repr(),
                    channels: match desc.channels() {
                        DeclaredBufferChannels::Mono => RunBufferChannels::Mono,
                        DeclaredBufferChannels::Static(channels) => {
                            RunBufferChannels::Static(channels)
                        }
                        DeclaredBufferChannels::Dynamic => RunBufferChannels::Dynamic,
                    },
                    loaded_path: binding
                        .and_then(|binding| binding.loaded_path.as_ref())
                        .map(|path| display_path(path)),
                    loaded_frames: binding.map(|binding| binding.frames),
                    loaded_channels: binding.map(|binding| binding.channels),
                    loaded_sample_rate_hz: binding.map(|binding| binding.sample_rate_hz),
                }
            })
            .collect()
    }

    pub fn event_info(&self) -> Vec<RunEventInfo> {
        (0..self.jit.event_count())
            .filter_map(|index| {
                let desc = self.jit.event_descriptor(index)?;
                Some(RunEventInfo {
                    index,
                    name: desc.name().to_owned(),
                    params: desc
                        .params()
                        .iter()
                        .enumerate()
                        .map(|(param_index, param)| RunEventParamInfo {
                            index: param_index,
                            name: param.name().to_owned(),
                            type_repr: param.type_repr(),
                            value: default_run_event_value(param),
                        })
                        .collect(),
                })
            })
            .collect()
    }

    pub fn set_param_f64(&mut self, name: &str, value: f64) -> Result<(), Diagnostic> {
        let Some(index) = self.jit.param_index(name) else {
            return Err(Diagnostic::runtime(
                format!("unknown parameter '{name}'"),
                0,
                0,
            ));
        };
        let Some(desc) = self.jit.param_descriptor(index) else {
            return Err(Diagnostic::runtime(
                format!("unknown parameter '{name}'"),
                0,
                0,
            ));
        };
        if desc.array_len() != 1 {
            return Err(Diagnostic::runtime(
                format!("parameter '{name}' is not scalar"),
                0,
                0,
            ));
        }
        let value = if desc.elem_ty() == PrimitiveType::Bool {
            if value >= 0.5 {
                1.0
            } else {
                0.0
            }
        } else {
            desc.param_domain()
                .map(|domain| domain.constrain_plain(value))
                .unwrap_or(value)
        };
        self.param_values.insert(name.to_owned(), value);
        if should_smooth_run_param(desc.elem_ty())
            && desc
                .param_domain()
                .is_none_or(|domain| domain.step_count().is_none())
        {
            self.param_runtime_values
                .entry(name.to_owned())
                .or_insert_with(|| default_run_param_value(desc));
        } else {
            let bytes = scalar_param_bytes(desc.elem_ty(), value)?;
            set_param_by_index(&mut self.instance, index, bytes.as_slice())?;
            self.param_runtime_values.insert(name.to_owned(), value);
        }
        Ok(())
    }

    pub fn trigger_event(
        &mut self,
        name: &str,
        values: &[RunEventValue],
    ) -> Result<(), Diagnostic> {
        let Some(index) = self.jit.event_index(name) else {
            return Err(Diagnostic::runtime(format!("unknown event '{name}'"), 0, 0));
        };
        let Some(desc) = self.jit.event_descriptor(index) else {
            return Err(Diagnostic::runtime(format!("unknown event '{name}'"), 0, 0));
        };
        let payload = event_payload_bytes(desc, values)?;
        trigger_event_by_index(&mut self.instance, index, &payload)
    }

    pub fn render_block(&mut self) -> Result<Vec<Vec<f32>>, Diagnostic> {
        self.render_block_segments(&[(
            0,
            self.options.block_size,
            onda_runtime::PROCESS_FULL_BLOCK,
        )])
    }

    pub fn render_block_segments(
        &mut self,
        segments: &[(usize, usize, u32)],
    ) -> Result<Vec<Vec<f32>>, Diagnostic> {
        let mut rendered = vec![
            0.0;
            self.options
                .block_size
                .saturating_mul(self.jit.required_out_channels())
        ];
        self.render_block_segments_interleaved(&mut rendered, segments)?;

        let mut channels = Vec::with_capacity(self.jit.required_out_channels());
        for channel in 0..self.jit.required_out_channels() {
            let mut samples = Vec::with_capacity(self.options.block_size);
            samples.extend(
                rendered
                    .chunks_exact(self.jit.required_out_channels())
                    .map(|frame| frame[channel]),
            );
            channels.push(samples);
        }
        Ok(channels)
    }

    /// Renders directly into a caller-owned interleaved buffer.
    ///
    /// The buffer must contain exactly one block. Once the session has been
    /// built, this path performs no host allocations.
    pub fn render_block_interleaved(&mut self, rendered: &mut [f32]) -> Result<(), Diagnostic> {
        self.render_block_segments_interleaved(
            rendered,
            &[(0, self.options.block_size, onda_runtime::PROCESS_FULL_BLOCK)],
        )
    }

    /// Renders one block through an explicit segmented process schedule.
    /// Segments may include zero-frame begin/end notifications and must each
    /// satisfy the runtime process ABI.
    pub fn render_block_segments_interleaved(
        &mut self,
        rendered: &mut [f32],
        segments: &[(usize, usize, u32)],
    ) -> Result<(), Diagnostic> {
        let output_channels = self.jit.required_out_channels();
        let expected_samples = self.options.block_size.saturating_mul(output_channels);
        if rendered.len() != expected_samples {
            return Err(Diagnostic::runtime(
                format!(
                    "run render buffer expects {expected_samples} samples, got {}",
                    rendered.len()
                ),
                0,
                0,
            ));
        }

        self.apply_smoothed_params()?;
        for buffer in &mut self.output_buffers {
            buffer.fill(0.0);
        }
        // SAFETY: all input, output, and declared-buffer bindings are installed
        // and prepared during build/rebuild. Their backing allocations remain
        // stable for the lifetime of this instance.
        for &(start_frame, frames, flags) in segments {
            unsafe {
                let status =
                    process_unchecked_segment(&mut self.instance, start_frame, frames, flags)?;
                check_execution_status(status)?;
            }
        }

        let mut output_channel = 0;
        for (buffer, desc) in self.output_buffers.iter().zip(self.jit.outputs()) {
            for ch in 0..desc.array_len() {
                let channel =
                    &buffer[ch * self.options.block_size..(ch + 1) * self.options.block_size];
                for (frame, &sample) in channel.iter().enumerate() {
                    rendered[frame * output_channels + output_channel] = sample;
                }
                output_channel += 1;
            }
        }
        Ok(())
    }

    pub fn snapshot_state_bytes(&self) -> Vec<u8> {
        self.instance.snapshot_state_bytes()
    }

    pub fn restore_state_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        self.instance.restore_state_bytes(bytes)?;
        prepare_unchecked_process(&mut self.instance)?;
        Ok(())
    }

    pub fn set_input_block(&mut self, interleaved: &[f32], source_channels: usize) {
        for buffer in &mut self.input_buffers {
            buffer.fill(0.0);
        }
        if source_channels == 0 || interleaved.is_empty() {
            return;
        }

        let frames = self
            .options
            .block_size
            .min(interleaved.len() / source_channels);
        for (buffer, desc) in self.input_buffers.iter_mut().zip(self.jit.inputs()) {
            for ch in 0..desc.array_len() {
                let src_channel = desc.slot_offset().saturating_add(ch);
                if src_channel >= source_channels {
                    continue;
                }
                let dst_base = ch.saturating_mul(self.options.block_size);
                for frame in 0..frames {
                    buffer[dst_base + frame] = interleaved[frame * source_channels + src_channel];
                }
            }
        }
    }

    /// Restores parameter defaults without changing processor state.
    pub fn reset_params(&mut self) -> Result<(), Diagnostic> {
        for index in 0..self.jit.param_count() {
            let desc = self
                .jit
                .param_descriptor(index)
                .expect("parameter index is within the compiled descriptor count");
            let default = desc
                .default_bytes()
                .expect("validated parameter metadata has default bytes");
            set_param_by_index(&mut self.instance, index, default)?;
        }
        self.param_values.clear();
        self.param_runtime_values.clear();
        Ok(())
    }

    /// Creates a new runtime instance from the already-compiled JIT program.
    /// Host-owned parameter targets and buffer bindings are retained, while all
    /// processor state and parameter-smoothing history start fresh.
    pub fn restart(&mut self) -> Result<(), Diagnostic> {
        self.param_runtime_values = self.param_values.clone();
        self.rebuild_instance()
    }

    pub fn bind_buffer_wav_path(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), Diagnostic> {
        self.bind_buffer_file_path(name, path)
    }

    pub fn bind_buffer_file_path(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), Diagnostic> {
        let path = path.as_ref();
        let asset =
            onda_project::load_buffer_file(path, ProjectLimits::default()).map_err(|error| {
                Diagnostic::runtime(
                    format!("failed to load buffer asset '{}': {error}", path.display()),
                    0,
                    0,
                )
            })?;
        self.bind_buffer_asset_with_path(name, asset, Some(path.to_path_buf()))
    }

    pub fn bind_buffer_samples(
        &mut self,
        name: &str,
        samples: Vec<f32>,
        channels: usize,
        sample_rate_hz: f32,
    ) -> Result<(), Diagnostic> {
        let frames = samples.len().checked_div(channels).unwrap_or(0);
        let frames = u32::try_from(frames).map_err(|_| {
            Diagnostic::runtime(format!("buffer '{name}' frame count exceeds u32"), 0, 0)
        })?;
        let channels = u32::try_from(channels).map_err(|_| {
            Diagnostic::runtime(format!("buffer '{name}' channel count exceeds u32"), 0, 0)
        })?;
        let asset = BufferAsset {
            frames,
            channels,
            sample_rate: sample_rate_hz,
            samples: BufferSamples::F32(samples),
        };
        self.bind_buffer_asset_with_path(name, asset, None)
    }

    pub fn bind_buffer_asset(&mut self, name: &str, asset: BufferAsset) -> Result<(), Diagnostic> {
        self.bind_buffer_asset_with_path(name, asset, None)
    }

    pub fn bind_buffer_asset_at_path(
        &mut self,
        name: &str,
        asset: BufferAsset,
        loaded_path: impl Into<PathBuf>,
    ) -> Result<(), Diagnostic> {
        self.bind_buffer_asset_with_path(name, asset, Some(loaded_path.into()))
    }

    pub fn clear_buffer(&mut self, name: &str) -> Result<(), Diagnostic> {
        let Some(index) = self.jit.buffer_index(name) else {
            return Err(Diagnostic::runtime(
                format!("unknown buffer '{name}'"),
                0,
                0,
            ));
        };
        let instance = self.build_instance(Some(BufferBindingReplacement {
            index,
            binding: None,
        }))?;
        self.instance = instance;
        self.buffer_bindings[index] = None;
        Ok(())
    }

    fn bind_buffer_asset_with_path(
        &mut self,
        name: &str,
        asset: BufferAsset,
        loaded_path: Option<PathBuf>,
    ) -> Result<(), Diagnostic> {
        let Some(index) = self.jit.buffer_index(name) else {
            return Err(Diagnostic::runtime(
                format!("unknown buffer '{name}'"),
                0,
                0,
            ));
        };
        let desc = self
            .jit
            .buffers()
            .get(index)
            .ok_or_else(|| Diagnostic::runtime(format!("unknown buffer '{name}'"), 0, 0))?;
        asset.validate(&ProjectLimits::default()).map_err(|error| {
            Diagnostic::runtime(format!("invalid asset for buffer '{name}': {error}"), 0, 0)
        })?;
        let elem_ty = primitive_type_for_buffer_element(asset.element());
        if desc.elem_ty() != elem_ty {
            return Err(Diagnostic::runtime(
                format!(
                    "buffer '{}' expects {}, but its asset contains {}",
                    name,
                    desc.type_repr(),
                    elem_ty.name()
                ),
                0,
                0,
            ));
        }
        let mut binding = RunBufferBinding {
            samples: asset.samples,
            frames: asset.frames as usize,
            channels: asset.channels as usize,
            sample_rate_hz: asset.sample_rate,
            loaded_path,
        };
        let instance = self.build_instance(Some(BufferBindingReplacement {
            index,
            binding: Some(&mut binding),
        }))?;
        self.instance = instance;
        self.buffer_bindings[index] = Some(binding);
        Ok(())
    }

    fn rebuild_instance(&mut self) -> Result<(), Diagnostic> {
        let instance = self.build_instance(None)?;
        self.instance = instance;
        Ok(())
    }

    fn build_instance(
        &mut self,
        mut replacement: Option<BufferBindingReplacement<'_>>,
    ) -> Result<Instance, Diagnostic> {
        let config = InstanceConfig {
            sample_rate: self.options.sample_rate,
            frames_per_block: self.options.block_size,
            in_channels: self.jit.required_in_channels(),
            out_channels: self.jit.required_out_channels(),
        };
        let mut instance = create_instance(self.jit.clone(), config)?;

        for (index, buffer) in self.input_buffers.iter_mut().enumerate() {
            unsafe {
                bind_input(
                    &mut instance,
                    index,
                    buffer.as_ptr().cast::<u8>(),
                    mem::size_of_val(buffer.as_slice()),
                )?;
            }
        }
        for (index, buffer) in self.output_buffers.iter_mut().enumerate() {
            unsafe {
                bind_output(
                    &mut instance,
                    index,
                    buffer.as_mut_ptr().cast::<u8>(),
                    mem::size_of_val(buffer.as_slice()),
                )?;
            }
        }
        for index in 0..self.buffer_bindings.len() {
            let binding = match replacement.as_mut() {
                Some(replacement) if replacement.index == index => {
                    replacement.binding.as_deref_mut()
                }
                _ => self.buffer_bindings[index].as_mut(),
            };
            let Some(binding) = binding else {
                continue;
            };
            unsafe {
                bind_buffer(
                    &mut instance,
                    index,
                    binding.samples.as_mut_ptr(),
                    binding.frames,
                    binding.channels,
                    binding.sample_rate_hz,
                    primitive_type_for_buffer_element(binding.samples.element()),
                )?;
            }
        }
        for (name, value) in self.param_values.clone() {
            let Some(index) = self.jit.param_index(&name) else {
                continue;
            };
            let Some(desc) = self.jit.param_descriptor(index) else {
                continue;
            };
            let runtime_value = self
                .param_runtime_values
                .get(&name)
                .copied()
                .unwrap_or(value);
            let bytes = scalar_param_bytes(desc.elem_ty(), runtime_value)?;
            set_param_by_index(&mut instance, index, bytes.as_slice())?;
        }
        prepare_unchecked_process(&mut instance)?;
        Ok(instance)
    }

    fn apply_smoothed_params(&mut self) -> Result<(), Diagnostic> {
        if self.options.float_param_smoothing_ms <= 0.0 {
            for (name, &target_value) in &self.param_values {
                let Some(index) = self.jit.param_index(name) else {
                    continue;
                };
                let Some(desc) = self.jit.param_descriptor(index) else {
                    continue;
                };
                if !should_smooth_run_param(desc.elem_ty())
                    || desc
                        .param_domain()
                        .is_some_and(|domain| domain.step_count().is_some())
                {
                    continue;
                }
                let bytes = scalar_param_bytes(desc.elem_ty(), target_value)?;
                set_param_by_index(&mut self.instance, index, bytes.as_slice())?;
                *self
                    .param_runtime_values
                    .get_mut(name)
                    .expect("smoothed run params have initialized runtime values") = target_value;
            }
            return Ok(());
        }
        let block_ms = (self.options.block_size as f64 * 1000.0)
            / f64::from(self.options.sample_rate.max(1.0));
        let alpha = (block_ms / self.options.float_param_smoothing_ms).clamp(0.0, 1.0);
        for (name, &target_value) in &self.param_values {
            let Some(index) = self.jit.param_index(name) else {
                continue;
            };
            let Some(desc) = self.jit.param_descriptor(index) else {
                continue;
            };
            if !should_smooth_run_param(desc.elem_ty())
                || desc
                    .param_domain()
                    .is_some_and(|domain| domain.step_count().is_some())
            {
                continue;
            }
            let current_value = self
                .param_runtime_values
                .get(name)
                .copied()
                .unwrap_or_else(|| default_run_param_value(desc));
            let mut next_value = current_value + (target_value - current_value) * alpha;
            if (target_value - next_value).abs() <= f64::max(0.0001, target_value.abs() * 0.001) {
                next_value = target_value;
            }
            let bytes = scalar_param_bytes(desc.elem_ty(), next_value)?;
            set_param_by_index(&mut self.instance, index, bytes.as_slice())?;
            *self
                .param_runtime_values
                .get_mut(name)
                .expect("smoothed run params have initialized runtime values") = next_value;
        }
        Ok(())
    }
}

fn should_smooth_run_param(ty: PrimitiveType) -> bool {
    matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
}

fn default_run_event_value(param: &DeclaredEventParam) -> RunEventValue {
    if param.is_slice() {
        return RunEventValue::Array(Vec::new());
    }
    if param.is_array() {
        let scalar_size = event_scalar_bytes(param.elem_ty());
        let defaults = param.default_bytes().unwrap_or_default();
        return RunEventValue::Array(
            (0..param.array_len())
                .map(|index| {
                    let start = index.saturating_mul(scalar_size);
                    let end = start.saturating_add(scalar_size);
                    defaults
                        .get(start..end)
                        .map(|bytes| scalar_run_event_value(param.elem_ty(), bytes))
                        .unwrap_or_else(|| zero_run_event_value(param.elem_ty()))
                })
                .collect(),
        );
    }
    let Some(bytes) = param.default_bytes() else {
        return zero_run_event_value(param.elem_ty());
    };
    scalar_run_event_value(param.elem_ty(), bytes)
}

fn zero_run_event_value(ty: PrimitiveType) -> RunEventValue {
    match ty {
        PrimitiveType::Bool => RunEventValue::Bool(false),
        _ => RunEventValue::Number(0.0),
    }
}

fn event_scalar_bytes(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn scalar_run_event_value(ty: PrimitiveType, bytes: &[u8]) -> RunEventValue {
    match ty {
        PrimitiveType::F32 if bytes.len() == 4 => {
            RunEventValue::Number(
                f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
            )
        }
        PrimitiveType::F64 if bytes.len() == 8 => RunEventValue::Number(f64::from_ne_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])),
        PrimitiveType::I32 if bytes.len() == 4 => {
            RunEventValue::Number(
                i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64
            )
        }
        PrimitiveType::I64 if bytes.len() == 8 => RunEventValue::Number(i64::from_ne_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]) as f64),
        PrimitiveType::Bool if !bytes.is_empty() => RunEventValue::Bool(bytes[0] != 0),
        PrimitiveType::Bool => RunEventValue::Bool(false),
        _ => RunEventValue::Number(0.0),
    }
}

fn event_payload_bytes(
    desc: &DeclaredEvent,
    values: &[RunEventValue],
) -> Result<Vec<u8>, Diagnostic> {
    if values.len() != desc.params().len() {
        return Err(Diagnostic::runtime(
            format!(
                "event '{}' expects {} values, got {}",
                desc.name(),
                desc.params().len(),
                values.len()
            ),
            0,
            0,
        ));
    }

    let mut out = Vec::with_capacity(desc.payload_bytes().unwrap_or(0));
    for (param, value) in desc.params().iter().zip(values.iter()) {
        if param.is_slice() || param.is_array() {
            let RunEventValue::Array(values) = value else {
                return Err(event_value_error(
                    desc.name(),
                    param,
                    format!("requires an array value, got {value:?}"),
                ));
            };
            if param.is_array() && values.len() != param.array_len() {
                return Err(event_value_error(
                    desc.name(),
                    param,
                    format!(
                        "requires exactly {} values, got {}",
                        param.array_len(),
                        values.len()
                    ),
                ));
            }
            if param.is_slice() {
                let length = i32::try_from(values.len()).map_err(|_| {
                    event_value_error(desc.name(), param, "contains too many values".to_owned())
                })?;
                out.extend_from_slice(&length.to_ne_bytes());
            }
            for value in values {
                append_scalar_event_value(&mut out, desc.name(), param, value)?;
            }
        } else {
            append_scalar_event_value(&mut out, desc.name(), param, value)?;
        }
    }
    Ok(out)
}

fn event_value_error(event_name: &str, param: &DeclaredEventParam, detail: String) -> Diagnostic {
    Diagnostic::runtime(
        format!(
            "event '{}' parameter '{}' {}",
            event_name,
            param.name(),
            detail
        ),
        0,
        0,
    )
}

fn append_scalar_event_value(
    out: &mut Vec<u8>,
    event_name: &str,
    param: &DeclaredEventParam,
    value: &RunEventValue,
) -> Result<(), Diagnostic> {
    match param.elem_ty() {
        PrimitiveType::F32 => out.extend_from_slice(
            &(event_number_value(event_name, param, value)? as f32).to_ne_bytes(),
        ),
        PrimitiveType::F64 => {
            out.extend_from_slice(&event_number_value(event_name, param, value)?.to_ne_bytes())
        }
        PrimitiveType::I32 => out.extend_from_slice(
            &(event_number_value(event_name, param, value)? as i32).to_ne_bytes(),
        ),
        PrimitiveType::I64 => out.extend_from_slice(
            &(event_number_value(event_name, param, value)? as i64).to_ne_bytes(),
        ),
        PrimitiveType::Bool => {
            let encoded = match value {
                RunEventValue::Bool(value) => {
                    if *value {
                        1_i8
                    } else {
                        0_i8
                    }
                }
                RunEventValue::Number(value) => {
                    if *value == 0.0 {
                        0_i8
                    } else if *value == 1.0 {
                        1_i8
                    } else {
                        return Err(Diagnostic::runtime(
                            format!(
                                "event '{}' parameter '{}' requires a boolean value, got {value}",
                                event_name,
                                param.name()
                            ),
                            0,
                            0,
                        ));
                    }
                }
                RunEventValue::Array(_) => {
                    return Err(event_value_error(
                        event_name,
                        param,
                        "requires a boolean scalar value".to_owned(),
                    ));
                }
            };
            out.extend_from_slice(&encoded.to_ne_bytes());
        }
    }
    Ok(())
}

fn event_number_value(
    event_name: &str,
    param: &DeclaredEventParam,
    value: &RunEventValue,
) -> Result<f64, Diagnostic> {
    match value {
        RunEventValue::Number(value) => Ok(*value),
        RunEventValue::Bool(value) => Err(Diagnostic::runtime(
            format!(
                "event '{}' parameter '{}' requires a numeric {} value, got {}",
                event_name,
                param.name(),
                param.type_repr(),
                value
            ),
            0,
            0,
        )),
        RunEventValue::Array(_) => Err(event_value_error(
            event_name,
            param,
            format!("requires numeric {} values", param.type_repr()),
        )),
    }
}

fn default_run_param_value(desc: &onda_codegen_llvm::DeclaredIo) -> f64 {
    desc.default_as_f64()
        .or_else(|| desc.range_min_as_f64())
        .unwrap_or(0.0)
}

fn primitive_type_for_buffer_element(element: BufferElement) -> PrimitiveType {
    match element {
        BufferElement::Bool => PrimitiveType::Bool,
        BufferElement::I32 => PrimitiveType::I32,
        BufferElement::I64 => PrimitiveType::I64,
        BufferElement::F32 => PrimitiveType::F32,
        BufferElement::F64 => PrimitiveType::F64,
    }
}

struct ScalarParamBytes {
    bytes: [u8; 8],
    len: usize,
}

impl ScalarParamBytes {
    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

fn scalar_param_bytes(
    ty: onda_frontend::PrimitiveType,
    value: f64,
) -> Result<ScalarParamBytes, Diagnostic> {
    let mut out = ScalarParamBytes {
        bytes: [0; 8],
        len: 0,
    };
    let len = match ty {
        onda_frontend::PrimitiveType::F32 => {
            out.bytes[..4].copy_from_slice(&(value as f32).to_ne_bytes());
            4
        }
        onda_frontend::PrimitiveType::F64 => {
            out.bytes.copy_from_slice(&value.to_ne_bytes());
            8
        }
        onda_frontend::PrimitiveType::I32 => {
            out.bytes[..4].copy_from_slice(&(value as i32).to_ne_bytes());
            4
        }
        onda_frontend::PrimitiveType::I64 => {
            out.bytes.copy_from_slice(&(value as i64).to_ne_bytes());
            8
        }
        onda_frontend::PrimitiveType::Bool => {
            out.bytes[0] = if value == 0.0 {
                0
            } else if value == 1.0 {
                1
            } else {
                return Err(Diagnostic::runtime(
                    format!("boolean parameter requires 0 or 1, got {value}"),
                    0,
                    0,
                ));
            };
            1
        }
    };
    out.len = len;
    Ok(out)
}

fn display_path(path: &Path) -> String {
    let raw = path.display().to_string();
    raw.strip_prefix(r"\\?\")
        .or_else(|| raw.strip_prefix("//?/"))
        .unwrap_or(&raw)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::display_path;
    use std::path::Path;

    #[test]
    fn display_path_strips_windows_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\Users\franc\audio\file.wav")),
            r"C:\Users\franc\audio\file.wav"
        );
    }
}
