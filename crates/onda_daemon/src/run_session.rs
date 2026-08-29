use std::path::{Path, PathBuf};
use std::{collections::HashMap, mem};

use onda_codegen_llvm::{
    check_execution_status, jit_program_from_optimized_mir_with_options, DeclaredBufferChannels,
    DeclaredDelegate, DeclaredEvent, DeclaredEventParam, JitProgram, MirCompileOptions,
    TargetOptLevel,
};
use onda_frontend::{Diagnostic, PrimitiveType};
use onda_project::{BufferAsset, BufferElement, BufferSamples, ProjectLimits};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, decode_print_batch_for_program,
    format_print_batch_for_program, init_with_output, prepare_unchecked_process,
    process_unchecked_segment, set_param_by_index, trigger_event_by_index, DelegateBatch,
    ExecutionOutput, InitMode, Instance, InstanceConfig, PrintBatch, PrintValue,
    DELEGATE_RECORD_HEADER_SIZE,
};
use onda_semantics::{AnalysisOptions, CompileInputs, TypedProgram};

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
    pub waveform: Option<RunBufferWaveform>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunBufferWaveform {
    pub min_value: f64,
    pub max_value: f64,
    pub minimums: Vec<f64>,
    pub maximums: Vec<f64>,
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

#[derive(Debug, Clone)]
pub struct RunDelegateInfo {
    pub index: usize,
    pub name: String,
    pub params: Vec<RunDelegateParamInfo>,
}

#[derive(Debug, Clone)]
pub struct RunDelegateParamInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunDelegateOccurrence {
    pub sequence: u32,
    pub index: usize,
    pub name: String,
    pub values: Vec<RunDelegateValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunDelegateValue {
    pub name: String,
    pub value: RunEventValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunDelegateBatch {
    pub occurrences: Vec<RunDelegateOccurrence>,
    pub overflow_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunPrintBatch {
    pub text: String,
    pub entries: Vec<RunPrintEntry>,
    pub overflow_count: u32,
    pub transport_drop_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunPrintEntry {
    pub sequence: u32,
    pub site_index: u32,
    pub label: Option<String>,
    pub source_file: Option<String>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub lexical_owner: String,
    pub declaration: Option<String>,
    pub values: Vec<RunPrintValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunPrintValue {
    pub type_repr: String,
    pub value: RunEventValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEventValue {
    Bool(bool),
    Number(f64),
    I64(i64),
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

/// An external buffer supplied before a run instance is initialized.
#[derive(Debug, Clone)]
pub struct InitialBufferBinding {
    pub name: String,
    pub asset: BufferAsset,
    pub loaded_path: Option<PathBuf>,
}

impl InitialBufferBinding {
    pub fn from_asset(
        name: impl Into<String>,
        asset: BufferAsset,
        loaded_path: Option<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            asset,
            loaded_path,
        }
    }

    pub fn load_file(
        name: impl Into<String>,
        path: impl AsRef<Path>,
        limits: ProjectLimits,
    ) -> Result<Self, onda_project::ProjectError> {
        let path = path.as_ref();
        Ok(Self::from_asset(
            name,
            onda_project::load_buffer_file(path, limits)?,
            Some(path.to_path_buf()),
        ))
    }
}

#[derive(Debug)]
pub struct RunSession {
    path: PathBuf,
    version: Option<DocumentVersion>,
    options: RunOptions,
    compile_inputs: CompileInputs,
    typed: TypedProgram,
    jit: JitProgram,
    instance: Instance,
    param_values: HashMap<String, f64>,
    param_runtime_values: HashMap<String, f64>,
    buffer_bindings: Vec<Option<RunBufferBinding>>,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
    delegate_storage: Vec<u8>,
    delegate_used: usize,
    delegate_record_count: u32,
    delegate_overflow_count: u32,
    print_storage: Vec<u8>,
    print_used: usize,
    print_record_count: u32,
    print_overflow_count: u32,
}

const RUN_DELEGATE_CAPACITY_BYTES: usize = 64 * 1024;
const RUN_PRINT_CAPACITY_BYTES: usize = 64 * 1024;
const RUN_BUFFER_WAVEFORM_COLUMNS: usize = 128;

#[derive(Debug)]
struct RunBufferBinding {
    samples: BufferSamples,
    frames: usize,
    channels: usize,
    sample_rate_hz: f32,
    loaded_path: Option<PathBuf>,
    waveform: RunBufferWaveform,
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
        Self::build_with_initial_buffers(analysis, path, options, std::iter::empty())
    }

    pub fn build_with_initial_buffers(
        analysis: &AnalysisSession,
        path: impl AsRef<Path>,
        options: RunOptions,
        initial_buffers: impl IntoIterator<Item = InitialBufferBinding>,
    ) -> Result<Self, RunBuildError> {
        Self::build_with_inputs_and_initial_buffers(
            analysis,
            path,
            options,
            &CompileInputs::default(),
            initial_buffers,
        )
    }

    pub fn build_with_inputs_and_initial_buffers(
        analysis: &AnalysisSession,
        path: impl AsRef<Path>,
        options: RunOptions,
        inputs: &CompileInputs,
        initial_buffers: impl IntoIterator<Item = InitialBufferBinding>,
    ) -> Result<Self, RunBuildError> {
        let path = normalize_session_path(path.as_ref());
        let snapshot =
            analysis.analyze_document_with_inputs(&path, options.analysis_options(), inputs);
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

        let mut input_buffers = jit
            .inputs()
            .iter()
            .map(|desc| vec![0.0_f32; options.block_size.saturating_mul(desc.array_len())])
            .collect::<Vec<_>>();
        let mut output_buffers = jit
            .outputs()
            .iter()
            .map(|desc| vec![0.0_f32; options.block_size.saturating_mul(desc.array_len())])
            .collect::<Vec<_>>();
        let mut buffer_bindings = std::iter::repeat_with(|| None)
            .take(jit.buffer_count())
            .collect::<Vec<_>>();
        for binding in initial_buffers {
            let binding_name = binding.name;
            let (index, binding) =
                validated_buffer_binding(&jit, &binding_name, binding.asset, binding.loaded_path)
                    .map_err(RunBuildError::Runtime)?;
            if buffer_bindings[index].is_some() {
                return Err(RunBuildError::Runtime(Diagnostic::runtime(
                    format!("buffer '{binding_name}' has more than one initial binding"),
                    0,
                    0,
                )));
            }
            buffer_bindings[index] = Some(binding);
        }
        let param_values = HashMap::new();
        let param_runtime_values = HashMap::new();
        let mut print_storage = if jit.mir().log_sites.is_empty() {
            Vec::new()
        } else {
            vec![0; RUN_PRINT_CAPACITY_BYTES]
        };
        let mut prints = Self::next_print_batch(&mut print_storage, 0);
        let instance_result = create_bound_instance(
            &jit,
            options,
            &mut input_buffers,
            &mut output_buffers,
            &mut buffer_bindings,
            None,
            &param_values,
            &param_runtime_values,
            ExecutionOutput {
                delegate_batch: None,
                print_batch: Some(&mut prints),
            },
        );
        let print_result = (
            prints.used_bytes,
            prints.record_count,
            prints.overflow_count,
        );
        let instance = instance_result.map_err(RunBuildError::Runtime)?;

        Ok(Self {
            path,
            version,
            options,
            compile_inputs: inputs.clone(),
            typed,
            jit,
            instance,
            param_values,
            param_runtime_values,
            buffer_bindings,
            input_buffers,
            output_buffers,
            delegate_storage: Vec::new(),
            delegate_used: 0,
            delegate_record_count: 0,
            delegate_overflow_count: 0,
            print_storage,
            print_used: print_result.0 as usize,
            print_record_count: print_result.1,
            print_overflow_count: print_result.2,
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

    pub fn compile_inputs(&self) -> &CompileInputs {
        &self.compile_inputs
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
                    waveform: binding.map(|binding| binding.waveform.clone()),
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

    pub fn delegate_info(&self) -> Vec<RunDelegateInfo> {
        (0..self.jit.delegate_count())
            .filter_map(|index| {
                let desc = self.jit.delegate_descriptor(index)?;
                Some(RunDelegateInfo {
                    index,
                    name: desc.name().to_owned(),
                    params: desc
                        .params()
                        .iter()
                        .enumerate()
                        .map(|(param_index, param)| RunDelegateParamInfo {
                            index: param_index,
                            name: param.name().to_owned(),
                            type_repr: param.type_repr(),
                        })
                        .collect(),
                })
            })
            .collect()
    }

    pub fn set_delegate_collection_enabled(&mut self, enabled: bool) {
        self.begin_delegate_batch();
        if enabled && self.delegate_storage.is_empty() && self.jit.delegate_count() != 0 {
            self.delegate_storage = vec![0; RUN_DELEGATE_CAPACITY_BYTES];
        } else if !enabled {
            self.delegate_storage.clear();
            self.delegate_storage.shrink_to_fit();
        }
    }

    pub fn delegate_collection_enabled(&self) -> bool {
        !self.delegate_storage.is_empty()
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
        self.begin_delegate_batch();
        self.begin_print_batch();
        let mut batch = Self::next_delegate_batch(&mut self.delegate_storage, self.delegate_used);
        let mut prints = Self::next_print_batch(&mut self.print_storage, self.print_used);
        let result = trigger_event_by_index(
            &mut self.instance,
            index,
            &payload,
            ExecutionOutput {
                delegate_batch: Some(&mut batch),
                print_batch: Some(&mut prints),
            },
        );
        let batch_result = (batch.used_bytes, batch.record_count, batch.overflow_count);
        let print_result = (
            prints.used_bytes,
            prints.record_count,
            prints.overflow_count,
        );
        self.finish_delegate_batch(batch_result);
        self.finish_print_batch(print_result);
        if result.is_err() {
            self.begin_delegate_batch();
        }
        result
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
        self.begin_delegate_batch();
        self.begin_print_batch();
        for buffer in &mut self.output_buffers {
            buffer.fill(0.0);
        }
        let mut sequence_base = 0_u32;
        // SAFETY: all input, output, and declared-buffer bindings are installed
        // and prepared during build/rebuild. Their backing allocations remain
        // stable for the lifetime of this instance.
        for &(start_frame, frames, flags) in segments {
            let delegate_start = self.delegate_used;
            let print_start = self.print_used;
            let mut batch =
                Self::next_delegate_batch(&mut self.delegate_storage, self.delegate_used);
            let mut prints = Self::next_print_batch(&mut self.print_storage, self.print_used);
            let result = unsafe {
                process_unchecked_segment(
                    &mut self.instance,
                    start_frame,
                    frames,
                    flags,
                    ExecutionOutput {
                        delegate_batch: Some(&mut batch),
                        print_batch: Some(&mut prints),
                    },
                )
            };
            let batch_result = (batch.used_bytes, batch.record_count, batch.overflow_count);
            let print_result = (
                prints.used_bytes,
                prints.record_count,
                prints.overflow_count,
            );
            let delegate_end = delegate_start + batch_result.0 as usize;
            let print_end = print_start + print_result.0 as usize;
            rebase_packed_output_sequences(
                &mut self.delegate_storage[delegate_start..delegate_end],
                sequence_base,
            );
            rebase_packed_output_sequences(
                &mut self.print_storage[print_start..print_end],
                sequence_base,
            );
            self.finish_delegate_batch(batch_result);
            self.finish_print_batch(print_result);
            sequence_base = sequence_base
                .saturating_add(batch_result.1)
                .saturating_add(batch_result.2)
                .saturating_add(print_result.1)
                .saturating_add(print_result.2);
            match result.and_then(check_execution_status) {
                Ok(()) => {}
                Err(error) => {
                    self.begin_delegate_batch();
                    return Err(error);
                }
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

    pub fn take_delegate_batch(&mut self) -> Result<RunDelegateBatch, Diagnostic> {
        let result = decode_run_delegate_batch(
            &self.jit,
            &self.delegate_storage[..self.delegate_used],
            self.delegate_record_count,
            self.delegate_overflow_count,
        );
        self.begin_delegate_batch();
        result
    }

    pub fn take_print_batch(&mut self) -> Result<RunPrintBatch, Diagnostic> {
        let mut batch = unsafe {
            PrintBatch::from_raw_parts(
                self.print_storage.as_mut_ptr(),
                self.print_storage.len() as u32,
            )
        };
        batch.used_bytes = self.print_used as u32;
        batch.record_count = self.print_record_count;
        batch.overflow_count = self.print_overflow_count;
        let result = decode_run_print_batch(&self.jit, &batch);
        self.begin_print_batch();
        result
    }

    fn begin_delegate_batch(&mut self) {
        self.delegate_used = 0;
        self.delegate_record_count = 0;
        self.delegate_overflow_count = 0;
    }

    fn next_delegate_batch(storage: &mut [u8], used: usize) -> DelegateBatch<'_> {
        if storage.is_empty() {
            DelegateBatch::absent()
        } else {
            DelegateBatch::from_storage(&mut storage[used..])
        }
    }

    fn finish_delegate_batch(&mut self, batch: (u32, u32, u32)) {
        let (used_bytes, record_count, overflow_count) = batch;
        self.delegate_used = self
            .delegate_used
            .saturating_add(used_bytes as usize)
            .min(self.delegate_storage.len());
        self.delegate_record_count = self.delegate_record_count.saturating_add(record_count);
        self.delegate_overflow_count = self.delegate_overflow_count.saturating_add(overflow_count);
    }

    fn begin_print_batch(&mut self) {
        self.print_used = 0;
        self.print_record_count = 0;
        self.print_overflow_count = 0;
    }

    fn next_print_batch(storage: &mut [u8], used: usize) -> PrintBatch<'_> {
        if storage.is_empty() {
            PrintBatch::absent()
        } else {
            PrintBatch::from_storage(&mut storage[used..])
        }
    }

    fn finish_print_batch(&mut self, batch: (u32, u32, u32)) {
        let (used_bytes, record_count, overflow_count) = batch;
        self.print_used = self
            .print_used
            .saturating_add(used_bytes as usize)
            .min(self.print_storage.len());
        self.print_record_count = self.print_record_count.saturating_add(record_count);
        self.print_overflow_count = self.print_overflow_count.saturating_add(overflow_count);
    }

    pub fn snapshot_state_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        self.instance.snapshot_state_bytes()
    }

    pub fn restore_state_bytes(&mut self, bytes: &[u8]) -> Result<(), Diagnostic> {
        let result = self
            .instance
            .restore_state_bytes(bytes)
            .and_then(|()| prepare_unchecked_process(&mut self.instance));
        self.begin_delegate_batch();
        self.begin_print_batch();
        result
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
        self.rebuild_instance()?;
        self.begin_delegate_batch();
        Ok(())
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
        let (index, mut binding) = validated_buffer_binding(&self.jit, name, asset, loaded_path)?;
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
        replacement: Option<BufferBindingReplacement<'_>>,
    ) -> Result<Instance, Diagnostic> {
        self.begin_print_batch();
        let mut prints = Self::next_print_batch(&mut self.print_storage, 0);
        let result = create_bound_instance(
            &self.jit,
            self.options,
            &mut self.input_buffers,
            &mut self.output_buffers,
            &mut self.buffer_bindings,
            replacement,
            &self.param_values,
            &self.param_runtime_values,
            ExecutionOutput {
                delegate_batch: None,
                print_batch: Some(&mut prints),
            },
        );
        let print_result = (
            prints.used_bytes,
            prints.record_count,
            prints.overflow_count,
        );
        self.finish_print_batch(print_result);
        result
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

fn validated_buffer_binding(
    jit: &JitProgram,
    name: &str,
    asset: BufferAsset,
    loaded_path: Option<PathBuf>,
) -> Result<(usize, RunBufferBinding), Diagnostic> {
    let Some(index) = jit.buffer_index(name) else {
        return Err(Diagnostic::runtime(
            format!("unknown buffer '{name}'"),
            0,
            0,
        ));
    };
    let desc = jit
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
    let waveform = buffer_waveform(
        &asset.samples,
        asset.frames as usize,
        asset.channels as usize,
    );
    Ok((
        index,
        RunBufferBinding {
            samples: asset.samples,
            frames: asset.frames as usize,
            channels: asset.channels as usize,
            sample_rate_hz: asset.sample_rate,
            loaded_path,
            waveform,
        },
    ))
}

fn buffer_waveform(samples: &BufferSamples, frames: usize, channels: usize) -> RunBufferWaveform {
    match samples {
        BufferSamples::Bool(values) => {
            buffer_waveform_values(values, frames, channels, |value| f64::from(*value != 0))
        }
        BufferSamples::I32(values) => {
            buffer_waveform_values(values, frames, channels, |value| f64::from(*value))
        }
        BufferSamples::I64(values) => {
            buffer_waveform_values(values, frames, channels, |value| *value as f64)
        }
        BufferSamples::F32(values) => {
            buffer_waveform_values(values, frames, channels, |value| f64::from(*value))
        }
        BufferSamples::F64(values) => {
            buffer_waveform_values(values, frames, channels, |value| *value)
        }
    }
}

fn buffer_waveform_values<T>(
    values: &[T],
    frames: usize,
    channels: usize,
    to_f64: impl Fn(&T) -> f64,
) -> RunBufferWaveform {
    let column_count = frames.min(RUN_BUFFER_WAVEFORM_COLUMNS);
    let mut minimums = Vec::with_capacity(column_count);
    let mut maximums = Vec::with_capacity(column_count);
    let mut min_value = f64::INFINITY;
    let mut max_value = f64::NEG_INFINITY;

    for column in 0..column_count {
        let start_frame = proportional_index(column, frames, column_count);
        let end_frame = proportional_index(column + 1, frames, column_count);
        let start = start_frame.saturating_mul(channels).min(values.len());
        let end = end_frame.saturating_mul(channels).min(values.len());
        let mut column_min = f64::INFINITY;
        let mut column_max = f64::NEG_INFINITY;
        for value in &values[start..end] {
            let value = to_f64(value);
            if !value.is_finite() {
                continue;
            }
            column_min = column_min.min(value);
            column_max = column_max.max(value);
        }
        if !column_min.is_finite() {
            column_min = 0.0;
            column_max = 0.0;
        }
        min_value = min_value.min(column_min);
        max_value = max_value.max(column_max);
        minimums.push(column_min);
        maximums.push(column_max);
    }

    if !min_value.is_finite() {
        min_value = 0.0;
        max_value = 0.0;
    }
    RunBufferWaveform {
        min_value,
        max_value,
        minimums,
        maximums,
    }
}

fn proportional_index(index: usize, length: usize, divisions: usize) -> usize {
    if divisions == 0 {
        return 0;
    }
    let quotient = length / divisions;
    let remainder = length % divisions;
    quotient * index + remainder * index / divisions
}

#[allow(clippy::too_many_arguments)]
fn create_bound_instance(
    jit: &JitProgram,
    options: RunOptions,
    input_buffers: &mut [Vec<f32>],
    output_buffers: &mut [Vec<f32>],
    buffer_bindings: &mut [Option<RunBufferBinding>],
    mut replacement: Option<BufferBindingReplacement<'_>>,
    param_values: &HashMap<String, f64>,
    param_runtime_values: &HashMap<String, f64>,
    output: ExecutionOutput<'_, '_>,
) -> Result<Instance, Diagnostic> {
    let config = InstanceConfig {
        sample_rate: options.sample_rate,
        frames_per_block: options.block_size,
        in_channels: jit.required_in_channels(),
        out_channels: jit.required_out_channels(),
    };
    let mut instance = create_instance(jit.clone(), config)?;

    for (index, buffer) in input_buffers.iter_mut().enumerate() {
        unsafe {
            bind_input(
                &mut instance,
                index,
                buffer.as_ptr().cast::<u8>(),
                mem::size_of_val(buffer.as_slice()),
            )?;
        }
    }
    for (index, buffer) in output_buffers.iter_mut().enumerate() {
        unsafe {
            bind_output(
                &mut instance,
                index,
                buffer.as_mut_ptr().cast::<u8>(),
                mem::size_of_val(buffer.as_slice()),
            )?;
        }
    }
    for (index, current_binding) in buffer_bindings.iter_mut().enumerate() {
        let binding = match replacement.as_mut() {
            Some(replacement) if replacement.index == index => replacement.binding.as_deref_mut(),
            _ => current_binding.as_mut(),
        };
        let Some(binding) = binding else { continue };
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
    for (name, value) in param_values {
        let Some(index) = jit.param_index(name) else {
            continue;
        };
        let Some(desc) = jit.param_descriptor(index) else {
            continue;
        };
        let runtime_value = param_runtime_values.get(name).copied().unwrap_or(*value);
        let bytes = scalar_param_bytes(desc.elem_ty(), runtime_value)?;
        set_param_by_index(&mut instance, index, bytes.as_slice())?;
    }

    init_with_output(&mut instance, InitMode::Full, output)?;
    prepare_unchecked_process(&mut instance)?;
    Ok(instance)
}

fn should_smooth_run_param(ty: PrimitiveType) -> bool {
    matches!(ty, PrimitiveType::F32 | PrimitiveType::F64)
}

fn run_print_value(value: PrintValue) -> RunPrintValue {
    match value {
        PrintValue::F32(value) => RunPrintValue {
            type_repr: "f32".to_owned(),
            value: RunEventValue::Number(f64::from(value)),
        },
        PrintValue::F64(value) => RunPrintValue {
            type_repr: "f64".to_owned(),
            value: RunEventValue::Number(value),
        },
        PrintValue::I32(value) => RunPrintValue {
            type_repr: "i32".to_owned(),
            value: RunEventValue::Number(f64::from(value)),
        },
        PrintValue::I64(value) => RunPrintValue {
            type_repr: "i64".to_owned(),
            value: RunEventValue::I64(value),
        },
        PrintValue::Bool(value) => RunPrintValue {
            type_repr: "bool".to_owned(),
            value: RunEventValue::Bool(value),
        },
    }
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
        PrimitiveType::I64 => RunEventValue::I64(0),
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
        PrimitiveType::I64 if bytes.len() == 8 => RunEventValue::I64(i64::from_ne_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])),
        PrimitiveType::Bool if !bytes.is_empty() => RunEventValue::Bool(bytes[0] != 0),
        PrimitiveType::Bool => RunEventValue::Bool(false),
        _ => RunEventValue::Number(0.0),
    }
}

/// RunSession aggregates several process calls into one host-visible batch.
/// The processor ABI numbers records within each call, so translate the new
/// records into that aggregate batch's sequence domain as they are appended.
/// Both print and delegate records use the same packed header shape.
fn rebase_packed_output_sequences(storage: &mut [u8], base: u32) {
    if base == 0 {
        return;
    }
    let mut cursor = 0usize;
    while let Some(header_end) = cursor
        .checked_add(DELEGATE_RECORD_HEADER_SIZE)
        .filter(|&end| end <= storage.len())
    {
        let payload_bytes = native_u32(&storage[cursor + 4..cursor + 8]) as usize;
        let sequence = native_u32(&storage[cursor + 8..header_end]);
        storage[cursor + 8..header_end]
            .copy_from_slice(&sequence.saturating_add(base).to_ne_bytes());
        let Some(record_end) = header_end
            .checked_add(payload_bytes)
            .filter(|&end| end <= storage.len())
        else {
            debug_assert!(false, "generated output record has a partial payload");
            return;
        };
        cursor = record_end;
    }
    debug_assert_eq!(cursor, storage.len());
}

fn decode_run_print_batch(
    jit: &JitProgram,
    batch: &PrintBatch<'_>,
) -> Result<RunPrintBatch, Diagnostic> {
    let text = format_print_batch_for_program(jit, batch)?;
    let entries = decode_print_batch_for_program(jit, batch)?
        .into_iter()
        .map(|occurrence| {
            let site = occurrence.site;
            let source_file = site.source.file.and_then(|file| {
                jit.mir()
                    .source_files
                    .get(file.index())
                    .map(|source| source.path.clone())
            });
            RunPrintEntry {
                sequence: occurrence.sequence,
                site_index: occurrence.site_index,
                label: site.label.clone(),
                source_file,
                line: site.source.line,
                column: site.source.column,
                end_line: site.source.end_line,
                end_column: site.source.end_column,
                lexical_owner: site.lexical_owner.clone(),
                declaration: site.declaration.clone(),
                values: occurrence.values.into_iter().map(run_print_value).collect(),
            }
        })
        .collect();
    Ok(RunPrintBatch {
        text,
        entries,
        overflow_count: batch.overflow_count,
        transport_drop_count: 0,
    })
}

fn decode_run_delegate_batch(
    jit: &JitProgram,
    storage: &[u8],
    record_count: u32,
    overflow_count: u32,
) -> Result<RunDelegateBatch, Diagnostic> {
    let mut cursor = 0usize;
    let mut occurrences = Vec::with_capacity(record_count as usize);
    while cursor < storage.len() {
        let header = take_delegate_bytes(
            storage,
            &mut cursor,
            DELEGATE_RECORD_HEADER_SIZE,
            "record header",
        )?;
        let delegate_index = native_u32(&header[..4]) as usize;
        let payload_bytes = native_u32(&header[4..8]) as usize;
        let sequence = native_u32(&header[8..12]);
        let payload = take_delegate_bytes(storage, &mut cursor, payload_bytes, "payload")?;
        let Some(delegate) = jit.delegate_descriptor(delegate_index) else {
            return Err(invalid_delegate_record(format!(
                "record references unknown delegate index {delegate_index}"
            )));
        };
        occurrences.push(decode_run_delegate_occurrence(
            delegate_index,
            sequence,
            delegate,
            payload,
        )?);
    }
    if occurrences.len() != record_count as usize {
        return Err(invalid_delegate_record(format!(
            "record count is {record_count}, but packed storage contains {} records",
            occurrences.len()
        )));
    }
    Ok(RunDelegateBatch {
        occurrences,
        overflow_count,
    })
}

fn decode_run_delegate_occurrence(
    index: usize,
    sequence: u32,
    delegate: &DeclaredDelegate,
    payload: &[u8],
) -> Result<RunDelegateOccurrence, Diagnostic> {
    let mut cursor = 0usize;
    let mut values = Vec::with_capacity(delegate.params().len());
    for param in delegate.params() {
        let count = if param.is_slice() {
            let bytes = take_delegate_bytes(payload, &mut cursor, 4, "slice length")?;
            native_u32(bytes) as usize
        } else {
            param.array_len()
        };
        let scalar_bytes = event_scalar_bytes(param.elem_ty());
        let byte_count = count
            .checked_mul(scalar_bytes)
            .ok_or_else(|| invalid_delegate_record("payload element count overflows usize"))?;
        let bytes = take_delegate_bytes(payload, &mut cursor, byte_count, "parameter")?;
        let value = if param.is_array() || param.is_slice() {
            RunEventValue::Array(
                bytes
                    .chunks_exact(scalar_bytes)
                    .map(|bytes| scalar_run_event_value(param.elem_ty(), bytes))
                    .collect(),
            )
        } else {
            scalar_run_event_value(param.elem_ty(), bytes)
        };
        values.push(RunDelegateValue {
            name: param.name().to_owned(),
            value,
        });
    }
    if cursor != payload.len() {
        return Err(invalid_delegate_record(format!(
            "delegate '{}' payload has {} trailing bytes",
            delegate.name(),
            payload.len() - cursor
        )));
    }
    Ok(RunDelegateOccurrence {
        sequence,
        index,
        name: delegate.name().to_owned(),
        values,
    })
}

fn take_delegate_bytes<'a>(
    storage: &'a [u8],
    cursor: &mut usize,
    len: usize,
    field: &str,
) -> Result<&'a [u8], Diagnostic> {
    let end = cursor
        .checked_add(len)
        .filter(|&end| end <= storage.len())
        .ok_or_else(|| invalid_delegate_record(format!("partial {field}")))?;
    let bytes = &storage[*cursor..end];
    *cursor = end;
    Ok(bytes)
}

fn native_u32(bytes: &[u8]) -> u32 {
    u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn invalid_delegate_record(message: impl Into<String>) -> Diagnostic {
    Diagnostic::runtime(format!("invalid delegate batch: {}", message.into()), 0, 0)
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
        PrimitiveType::I64 => {
            let value = match value {
                RunEventValue::I64(value) => *value,
                _ => event_number_value(event_name, param, value)? as i64,
            };
            out.extend_from_slice(&value.to_ne_bytes());
        }
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
                RunEventValue::I64(value) => {
                    if *value == 0 {
                        0_i8
                    } else if *value == 1 {
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
        RunEventValue::I64(value) => Ok(*value as f64),
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
