#[cfg(feature = "llvm-orc")]
use onda_semantics::TypedConstValue;
use serde::Serialize;

#[cfg(feature = "llvm-orc")]
use crate::metadata::ProgramMetadata;
#[cfg(feature = "llvm-orc")]
use crate::mir_metadata::{build_mir_program_metadata, MirMetadataError, MirMetadataLayoutView};
#[cfg(feature = "llvm-orc")]
use crate::{DeclaredBufferChannels, TargetConfig};

/// Current JSON sidecar schema emitted with native AOT objects.
pub const AOT_METADATA_FORMAT_VERSION: u32 = 2;
/// Packed persistent-state snapshot encoding described by the sidecar.
pub const AOT_SNAPSHOT_FORMAT_VERSION: u32 = 1;

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
    /// Physical target-layout state storage required by the native entrypoints.
    pub state_size_bytes: usize,
    /// Minimum alignment required for the physical state allocation.
    pub state_align_bytes: usize,
    /// Required host initialization policy before calling `onda_init`.
    pub state_initialization: &'static str,
    /// Packed, target-independent persistent-state snapshot size.
    pub snapshot_size_bytes: usize,
    /// Version of the packed snapshot byte encoding.
    pub snapshot_format_version: u32,
    /// Byte order used for every scalar element in the packed snapshot.
    pub snapshot_byte_order: &'static str,
    /// Physical image a host must copy before overlaying restored persistent segments.
    pub snapshot_restore_base: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct AotProgramMetadata {
    pub inputs: Vec<AotIoMetadata>,
    pub outputs: Vec<AotIoMetadata>,
    pub control_outputs: Vec<AotIoMetadata>,
    pub params: Vec<AotIoMetadata>,
    pub buffers: Vec<AotBufferMetadata>,
    pub events: Vec<AotEventMetadata>,
    /// Persistent state only. Scratch and control-output mirrors are omitted.
    pub states: Vec<AotStateMetadata>,
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

#[derive(Debug, Clone, Serialize)]
pub struct AotStateMetadata {
    pub name: String,
    pub type_repr: String,
    pub array_len: usize,
    /// Bytes per scalar element; hosts need not parse `type_repr`.
    pub element_size_bytes: usize,
    /// Offset in the packed little-endian snapshot.
    pub packed_snapshot_byte_offset: usize,
    /// Offset in the target-layout physical state allocation.
    pub physical_state_byte_offset: usize,
    pub byte_size: usize,
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "llvm-orc")]
pub(crate) fn build_mir_aot_metadata(
    program: &onda_mir::Program,
    layout: MirMetadataLayoutView<'_>,
    fast_math: bool,
    target: &TargetConfig,
    resolved_triple: String,
    resolved_cpu: String,
    resolved_features: String,
    state_size_bytes: usize,
    state_align_bytes: usize,
) -> Result<AotMetadata, MirMetadataError> {
    let metadata = build_mir_program_metadata(program, layout)?;
    Ok(build_aot_metadata_from_descriptors(
        metadata,
        program.interface.events.len(),
        program.config.sample_rate,
        program.config.block_size as usize,
        fast_math,
        target,
        resolved_triple,
        resolved_cpu,
        resolved_features,
        state_size_bytes,
        state_align_bytes,
    ))
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "llvm-orc")]
fn build_aot_metadata_from_descriptors(
    metadata: ProgramMetadata,
    event_count: usize,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: &TargetConfig,
    resolved_triple: String,
    resolved_cpu: String,
    resolved_features: String,
    state_size_bytes: usize,
    state_align_bytes: usize,
) -> AotMetadata {
    let snapshot_size_bytes = metadata
        .state_entries
        .last()
        .map_or(0, |state| state.byte_offset() + state.byte_size());
    AotMetadata {
        format_version: AOT_METADATA_FORMAT_VERSION,
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
            events: (0..event_count)
                .map(|idx| format!("onda_event_{idx}"))
                .collect(),
        },
        runtime: AotRuntimeInfo {
            state_size_bytes,
            state_align_bytes,
            state_initialization: "zeroed",
            snapshot_size_bytes,
            snapshot_format_version: AOT_SNAPSHOT_FORMAT_VERSION,
            snapshot_byte_order: "little_endian",
            snapshot_restore_base: "post_init_physical_state_image",
        },
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
            states: metadata
                .state_entries
                .iter()
                .map(map_state_metadata)
                .collect(),
        },
    }
}

#[cfg(feature = "llvm-orc")]
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

#[cfg(feature = "llvm-orc")]
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

#[cfg(feature = "llvm-orc")]
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

#[cfg(feature = "llvm-orc")]
fn map_state_metadata(state: &crate::DeclaredState) -> AotStateMetadata {
    AotStateMetadata {
        name: state.name().to_owned(),
        type_repr: state.type_repr(),
        array_len: state.array_len(),
        element_size_bytes: state.byte_size() / state.array_len(),
        packed_snapshot_byte_offset: state.byte_offset(),
        physical_state_byte_offset: state.storage_byte_offset(),
        byte_size: state.byte_size(),
    }
}

#[cfg(feature = "llvm-orc")]
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

#[cfg(feature = "llvm-orc")]
fn format_const_value(value: TypedConstValue) -> String {
    match value {
        TypedConstValue::F32(v) => v.to_string(),
        TypedConstValue::F64(v) => v.to_string(),
        TypedConstValue::I32(v) => v.to_string(),
        TypedConstValue::I64(v) => v.to_string(),
        TypedConstValue::Bool(v) => v.to_string(),
    }
}
