use std::path::{Path, PathBuf};

use omni_codegen_llvm::{lower_and_jit_with_options, CompileOptions, ExecutionBackend, JitProgram};
use omni_frontend::Diagnostic;
use omni_runtime::{
    bind_input, bind_output, create_instance, process_bound, reset_instance_state,
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
    _input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
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

        if jit.buffer_count() != 0 {
            return Err(PreviewBuildError::Runtime(Diagnostic::runtime(
                "preview does not yet support external buffers",
                0,
                0,
            )));
        }

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

        Ok(Self {
            path,
            version,
            options,
            typed,
            jit,
            instance,
            _input_buffers: input_buffers,
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
        set_param_by_index(&mut self.instance, index, &bytes)
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
