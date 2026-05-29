use onda_semantics::{TypedConstValue, TypedProgram};
use serde::Serialize;

use crate::metadata::build_program_metadata;
use crate::{DeclaredBufferChannels, TargetConfig};

#[derive(Debug, Clone, Serialize)]
pub struct AotObjectArtifact {
    pub object_bytes: Vec<u8>,
    pub metadata: AotMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotMetadata {
    pub format_version: u32,
    pub target: AotTargetInfo,
    pub compile: AotCompileInfo,
    pub exports: AotExports,
    pub runtime: AotRuntimeInfo,
    pub metadata: AotProgramMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotTargetInfo {
    pub triple: String,
    pub cpu: String,
    pub features: String,
    pub reloc_model: String,
    pub code_model: String,
    pub opt_level: String,
    pub abi_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotCompileInfo {
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotExports {
    pub init: String,
    pub process: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotRuntimeInfo {
    pub state_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotProgramMetadata {
    pub inputs: Vec<AotIoMetadata>,
    pub outputs: Vec<AotIoMetadata>,
    pub control_outputs: Vec<AotIoMetadata>,
    pub params: Vec<AotIoMetadata>,
    pub buffers: Vec<AotBufferMetadata>,
    pub events: Vec<AotEventMetadata>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotIoMetadata {
    pub name: String,
    pub type_repr: String,
    pub array_len: usize,
    pub slot_offset: usize,
    pub byte_offset: usize,
    pub state_byte_offset: Option<usize>,
    pub byte_size: usize,
    pub default_repr: Option<String>,
    pub range_min_repr: Option<String>,
    pub range_max_repr: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotBufferMetadata {
    pub name: String,
    pub type_repr: String,
    pub channels: String,
    pub static_channels: Option<usize>,
    pub may_write: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotEventMetadata {
    pub name: String,
    pub payload_bytes: Option<usize>,
    pub params: Vec<AotEventParamMetadata>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotEventParamMetadata {
    pub name: String,
    pub type_repr: String,
    pub array_len: usize,
    pub is_slice: bool,
    pub byte_offset: usize,
    pub byte_size: Option<usize>,
    pub has_default: bool,
}

pub(crate) fn build_aot_metadata(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: &TargetConfig,
    resolved_triple: String,
    resolved_cpu: String,
    resolved_features: String,
    state_size_bytes: usize,
) -> AotMetadata {
    let metadata = build_program_metadata(typed);

    AotMetadata {
        format_version: 1,
        target: AotTargetInfo {
            triple: resolved_triple,
            cpu: resolved_cpu,
            features: resolved_features,
            reloc_model: target.reloc_model.as_str().to_owned(),
            code_model: target.code_model.as_str().to_owned(),
            opt_level: target.opt_level.as_str().to_owned(),
            abi_name: target.abi_name.clone(),
        },
        compile: AotCompileInfo {
            sample_rate,
            block_size,
            fast_math,
        },
        exports: AotExports {
            init: "onda_init".to_owned(),
            process: "onda_process".to_owned(),
            events: (0..typed.events.len())
                .map(|idx| format!("onda_event_{idx}"))
                .collect(),
        },
        runtime: AotRuntimeInfo { state_size_bytes },
        metadata: AotProgramMetadata {
            inputs: metadata.inputs.iter().map(map_io_metadata).collect(),
            outputs: metadata.outputs.iter().map(map_io_metadata).collect(),
            control_outputs: metadata
                .control_outputs
                .iter()
                .map(map_io_metadata)
                .collect(),
            params: metadata.params.iter().map(map_io_metadata).collect(),
            buffers: metadata.buffers.iter().map(map_buffer_metadata).collect(),
            events: metadata.events.iter().map(map_event_metadata).collect(),
        },
    }
}

fn map_io_metadata(io: &crate::DeclaredIo) -> AotIoMetadata {
    AotIoMetadata {
        name: io.name().to_owned(),
        type_repr: io.type_repr(),
        array_len: io.array_len(),
        slot_offset: io.slot_offset(),
        byte_offset: io.byte_offset(),
        state_byte_offset: io.state_byte_offset(),
        byte_size: io.byte_size(),
        default_repr: io.default().map(format_const_value),
        range_min_repr: io.range().map(|range| format_const_value(range.min)),
        range_max_repr: io.range().map(|range| format_const_value(range.max)),
    }
}

fn map_buffer_metadata(buffer: &crate::DeclaredBuffer) -> AotBufferMetadata {
    let (channels, static_channels) = match buffer.channels() {
        DeclaredBufferChannels::Mono => ("mono".to_owned(), Some(1)),
        DeclaredBufferChannels::Static(ch) => ("static".to_owned(), Some(ch)),
        DeclaredBufferChannels::Dynamic => ("dynamic".to_owned(), None),
    };
    AotBufferMetadata {
        name: buffer.name().to_owned(),
        type_repr: buffer.type_repr(),
        channels,
        static_channels,
        may_write: buffer.may_write(),
    }
}

fn map_event_metadata(event: &crate::DeclaredEvent) -> AotEventMetadata {
    AotEventMetadata {
        name: event.name().to_owned(),
        payload_bytes: event.payload_bytes(),
        params: event
            .params()
            .iter()
            .map(map_event_param_metadata)
            .collect(),
    }
}

fn map_event_param_metadata(param: &crate::DeclaredEventParam) -> AotEventParamMetadata {
    AotEventParamMetadata {
        name: param.name().to_owned(),
        type_repr: param.type_repr(),
        array_len: param.array_len(),
        is_slice: param.is_slice(),
        byte_offset: param.byte_offset(),
        byte_size: param.byte_size(),
        has_default: param.has_default(),
    }
}

fn format_const_value(value: TypedConstValue) -> String {
    match value {
        TypedConstValue::F32(v) => v.to_string(),
        TypedConstValue::F64(v) => v.to_string(),
        TypedConstValue::I32(v) => v.to_string(),
        TypedConstValue::I64(v) => v.to_string(),
        TypedConstValue::Bool(v) => v.to_string(),
    }
}
