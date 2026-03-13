use std::path::{Path, PathBuf};
use std::{collections::HashMap, mem};

use omni_codegen_llvm::{
    lower_and_jit_with_options, CompileOptions, DeclaredBufferChannels, ExecutionBackend,
    JitProgram,
};
use omni_frontend::{Diagnostic, PrimitiveType};
use omni_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_bound, reset_instance_state,
    set_param_by_index, Instance, InstanceConfig,
};
use omni_semantics::{AnalysisOptions, TypedProgram};

use crate::analysis_session::{normalize_session_path, AnalysisSession, DocumentVersion};

#[derive(Debug, Clone, Copy)]
pub struct PreviewOptions {
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
    pub backend: ExecutionBackend,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            block_size: 512,
            fast_math: false,
            backend: ExecutionBackend::Auto,
        }
    }
}

impl PreviewOptions {
    pub fn analysis_options(&self) -> AnalysisOptions {
        AnalysisOptions {
            sample_rate: self.sample_rate,
            block_size: self.block_size,
        }
    }

    pub fn compile_options(&self) -> CompileOptions {
        CompileOptions {
            backend: self.backend,
            sample_rate: self.sample_rate,
            block_size: self.block_size,
            fast_math: self.fast_math,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewParamInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
    pub default: Option<f64>,
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    pub scalar: bool,
}

#[derive(Debug, Clone)]
pub struct PreviewBufferInfo {
    pub index: usize,
    pub name: String,
    pub type_repr: String,
    pub channels: PreviewBufferChannels,
    pub loaded_path: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum PreviewBufferChannels {
    Mono,
    Static(usize),
    Dynamic,
}

#[derive(Debug)]
pub enum PreviewBuildError {
    Diagnostics(Vec<Diagnostic>),
    Runtime(Diagnostic),
}

#[derive(Debug)]
pub struct PreviewSession {
    path: PathBuf,
    version: Option<DocumentVersion>,
    options: PreviewOptions,
    typed: TypedProgram,
    jit: JitProgram,
    instance: Instance,
    param_values: HashMap<String, f64>,
    buffer_bindings: Vec<PreviewBufferBinding>,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
}

#[derive(Debug)]
struct PreviewBufferBinding {
    _samples: Vec<f32>,
    frames: usize,
    channels: usize,
    loaded_path: Option<PathBuf>,
}

impl PreviewSession {
    pub fn build(
        analysis: &AnalysisSession,
        path: impl AsRef<Path>,
        options: PreviewOptions,
    ) -> Result<Self, PreviewBuildError> {
        let path = normalize_session_path(path.as_ref());
        let snapshot = analysis.analyze_document(&path, options.analysis_options());
        let version = snapshot.version;
        let Some(typed) = snapshot.typed else {
            return Err(PreviewBuildError::Diagnostics(snapshot.diagnostics));
        };

        let jit = lower_and_jit_with_options(typed.clone(), options.compile_options())
            .map_err(PreviewBuildError::Diagnostics)?;

        let config = InstanceConfig {
            sample_rate: options.sample_rate,
            frames_per_block: options.block_size,
            in_channels: jit.required_in_channels(),
            out_channels: jit.required_out_channels(),
        };
        let mut instance =
            create_instance(jit.clone(), config).map_err(PreviewBuildError::Runtime)?;

        let mut input_buffers = vec![vec![0.0_f32; options.block_size]; jit.input_count()];
        for (index, buffer) in input_buffers.iter_mut().enumerate() {
            bind_input(
                &mut instance,
                index,
                buffer.as_ptr().cast::<u8>(),
                std::mem::size_of_val(buffer.as_slice()),
            )
            .map_err(PreviewBuildError::Runtime)?;
        }

        let mut output_buffers = vec![vec![0.0_f32; options.block_size]; jit.output_count()];
        for (index, buffer) in output_buffers.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                index,
                buffer.as_mut_ptr().cast::<u8>(),
                std::mem::size_of_val(buffer.as_slice()),
            )
            .map_err(PreviewBuildError::Runtime)?;
        }

        let buffer_bindings =
            bind_placeholder_buffers(&jit, &mut instance, options.block_size)
                .map_err(PreviewBuildError::Runtime)?;

        Ok(Self {
            path,
            version,
            options,
            typed,
            jit,
            instance,
            param_values: HashMap::new(),
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

    pub fn options(&self) -> PreviewOptions {
        self.options
    }

    pub fn typed_program(&self) -> &TypedProgram {
        &self.typed
    }

    pub fn output_channel_count(&self) -> usize {
        self.output_buffers.len()
    }

    pub fn param_info(&self) -> Vec<PreviewParamInfo> {
        (0..self.jit.param_count())
            .filter_map(|index| {
                let desc = self.jit.param_descriptor(index)?;
                Some(PreviewParamInfo {
                    index,
                    name: desc.name().to_owned(),
                    type_repr: desc.type_repr(),
                    default: desc.default_as_f64(),
                    range_min: desc.range_min_as_f64(),
                    range_max: desc.range_max_as_f64(),
                    scalar: desc.array_len() == 1,
                })
            })
            .collect()
    }

    pub fn buffer_info(&self) -> Vec<PreviewBufferInfo> {
        self.jit
            .buffers()
            .iter()
            .enumerate()
            .map(|(index, desc)| PreviewBufferInfo {
                index,
                name: desc.name().to_owned(),
                type_repr: desc.type_repr(),
                channels: match desc.channels() {
                    DeclaredBufferChannels::Mono => PreviewBufferChannels::Mono,
                    DeclaredBufferChannels::Static(channels) => {
                        PreviewBufferChannels::Static(channels)
                    }
                    DeclaredBufferChannels::Dynamic => PreviewBufferChannels::Dynamic,
                },
                loaded_path: self
                    .buffer_bindings
                    .get(index)
                    .and_then(|binding| binding.loaded_path.as_ref())
                    .map(|path| path.display().to_string()),
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
        let bytes = scalar_param_bytes(desc.elem_ty(), value)?;
        set_param_by_index(&mut self.instance, index, &bytes)?;
        self.param_values.insert(name.to_owned(), value);
        Ok(())
    }

    pub fn render_block(&mut self) -> Result<Vec<Vec<f32>>, Diagnostic> {
        for buffer in &mut self.output_buffers {
            buffer.fill(0.0);
        }
        process_bound(&mut self.instance, self.options.block_size)?;
        Ok(self.output_buffers.clone())
    }

    pub fn reset(&mut self) {
        reset_instance_state(&mut self.instance);
    }

    pub fn bind_buffer_wav_path(
        &mut self,
        name: &str,
        path: impl AsRef<Path>,
    ) -> Result<(), Diagnostic> {
        let path = path.as_ref();
        let (samples, channels, _sample_rate_hz) = read_wav_interleaved_f32(path)?;
        self.bind_buffer_samples(name, samples, channels, Some(path.to_path_buf()))
    }

    pub fn clear_buffer(&mut self, name: &str) -> Result<(), Diagnostic> {
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
        let channels = default_preview_buffer_channels(desc.channels());
        let frames = self.options.block_size.max(1);
        let samples = vec![0.0_f32; frames.saturating_mul(channels)];
        self.bind_buffer_samples(name, samples, channels, None)
    }

    fn bind_buffer_samples(
        &mut self,
        name: &str,
        samples: Vec<f32>,
        channels: usize,
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
        if desc.elem_ty() != PrimitiveType::F32 {
            return Err(Diagnostic::runtime(
                format!(
                    "preview only supports f32-typed buffer bindings, but '{}' is {}",
                    name,
                    desc.type_repr()
                ),
                0,
                0,
            ));
        }
        if channels == 0 || samples.is_empty() || samples.len() % channels != 0 {
            return Err(Diagnostic::runtime(
                format!("buffer '{}' data is not a valid interleaved f32 audio buffer", name),
                0,
                0,
            ));
        }
        let frames = samples.len() / channels;
        if self.buffer_bindings.len() <= index {
            self.buffer_bindings.resize_with(index + 1, || PreviewBufferBinding {
                _samples: Vec::new(),
                frames: 0,
                channels: 0,
                loaded_path: None,
            });
        }
        self.buffer_bindings[index] = PreviewBufferBinding {
            _samples: samples,
            frames,
            channels,
            loaded_path,
        };
        self.rebuild_instance()?;
        Ok(())
    }

    fn rebuild_instance(&mut self) -> Result<(), Diagnostic> {
        let config = InstanceConfig {
            sample_rate: self.options.sample_rate,
            frames_per_block: self.options.block_size,
            in_channels: self.jit.required_in_channels(),
            out_channels: self.jit.required_out_channels(),
        };
        let mut instance = create_instance(self.jit.clone(), config)?;

        for (index, buffer) in self.input_buffers.iter_mut().enumerate() {
            bind_input(
                &mut instance,
                index,
                buffer.as_ptr().cast::<u8>(),
                mem::size_of_val(buffer.as_slice()),
            )?;
        }
        for (index, buffer) in self.output_buffers.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                index,
                buffer.as_mut_ptr().cast::<u8>(),
                mem::size_of_val(buffer.as_slice()),
            )?;
        }
        for (index, binding) in self.buffer_bindings.iter_mut().enumerate() {
            bind_buffer(
                &mut instance,
                index,
                binding._samples.as_mut_ptr().cast::<u8>(),
                binding.frames,
                binding.channels,
                PrimitiveType::F32,
            )?;
        }
        for (name, value) in self.param_values.clone() {
            let Some(index) = self.jit.param_index(&name) else {
                continue;
            };
            let Some(desc) = self.jit.param_descriptor(index) else {
                continue;
            };
            let bytes = scalar_param_bytes(desc.elem_ty(), value)?;
            set_param_by_index(&mut instance, index, &bytes)?;
        }
        self.instance = instance;
        Ok(())
    }
}

fn bind_placeholder_buffers(
    jit: &JitProgram,
    instance: &mut Instance,
    block_size: usize,
) -> Result<Vec<PreviewBufferBinding>, Diagnostic> {
    let mut bindings = Vec::with_capacity(jit.buffer_count());
    for (index, desc) in jit.buffers().iter().enumerate() {
        if desc.elem_ty() != PrimitiveType::F32 {
            return Err(Diagnostic::runtime(
                format!(
                    "preview only supports f32-typed buffer declarations, but '{}' is {}",
                    desc.name(),
                    desc.type_repr()
                ),
                0,
                0,
            ));
        }
        let channels = default_preview_buffer_channels(desc.channels());
        let frames = block_size.max(1);
        let mut samples = vec![0.0_f32; frames.saturating_mul(channels)];
        bind_buffer(
            instance,
            index,
            samples.as_mut_ptr().cast::<u8>(),
            frames,
            channels,
            PrimitiveType::F32,
        )?;
        bindings.push(PreviewBufferBinding {
            _samples: samples,
            frames,
            channels,
            loaded_path: None,
        });
    }
    Ok(bindings)
}

fn default_preview_buffer_channels(channels: DeclaredBufferChannels) -> usize {
    match channels {
        DeclaredBufferChannels::Mono => 1,
        DeclaredBufferChannels::Static(channels) => channels.max(1),
        DeclaredBufferChannels::Dynamic => 1,
    }
}

fn read_wav_interleaved_f32(path: &Path) -> Result<(Vec<f32>, usize, u32), Diagnostic> {
    let mut reader = hound::WavReader::open(path).map_err(|err| {
        Diagnostic::runtime(
            format!("failed to open wav '{}': {err}", path.display()),
            0,
            0,
        )
    })?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    if channels == 0 {
        return Err(Diagnostic::runtime(
            format!("wav '{}' has zero channels", path.display()),
            0,
            0,
        ));
    }

    let samples = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Diagnostic::runtime(
                    format!("failed to read wav samples '{}': {err}", path.display()),
                    0,
                    0,
                )
            })?,
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|sample| sample.map(|value| value as f32 / i8::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Diagnostic::runtime(
                    format!("failed to read wav samples '{}': {err}", path.display()),
                    0,
                    0,
                )
            })?,
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| value as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Diagnostic::runtime(
                    format!("failed to read wav samples '{}': {err}", path.display()),
                    0,
                    0,
                )
            })?,
        (hound::SampleFormat::Int, 24) | (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|sample| sample.map(|value| value as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| {
                Diagnostic::runtime(
                    format!("failed to read wav samples '{}': {err}", path.display()),
                    0,
                    0,
                )
            })?,
        _ => {
            return Err(Diagnostic::runtime(
                format!(
                    "unsupported wav format for '{}': {:?} {} bits",
                    path.display(),
                    spec.sample_format,
                    spec.bits_per_sample
                ),
                0,
                0,
            ))
        }
    };

    if samples.is_empty() {
        return Err(Diagnostic::runtime(
            format!("wav '{}' contains no samples", path.display()),
            0,
            0,
        ));
    }

    Ok((samples, channels, spec.sample_rate))
}

fn scalar_param_bytes(ty: omni_frontend::PrimitiveType, value: f64) -> Result<Vec<u8>, Diagnostic> {
    let mut out = Vec::new();
    match ty {
        omni_frontend::PrimitiveType::F32 => out.extend_from_slice(&(value as f32).to_ne_bytes()),
        omni_frontend::PrimitiveType::F64 => out.extend_from_slice(&value.to_ne_bytes()),
        omni_frontend::PrimitiveType::I32 => out.extend_from_slice(&(value as i32).to_ne_bytes()),
        omni_frontend::PrimitiveType::I64 => out.extend_from_slice(&(value as i64).to_ne_bytes()),
        omni_frontend::PrimitiveType::Bool => {
            let encoded = if value == 0.0 {
                0_i8
            } else if value == 1.0 {
                1_i8
            } else {
                return Err(Diagnostic::runtime(
                    format!("boolean parameter requires 0 or 1, got {value}"),
                    0,
                    0,
                ));
            };
            out.extend_from_slice(&encoded.to_ne_bytes());
        }
    }
    Ok(out)
}
