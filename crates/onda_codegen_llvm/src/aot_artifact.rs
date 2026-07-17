#[cfg(feature = "llvm-orc")]
use onda_semantics::TypedConstValue;
use serde::Serialize;

pub use onda_processor_abi::{
    ProcessorDescriptor as AotMetadata, StateMetadata as AotStateMetadata, PROCESSOR_ABI_VERSION,
    PROCESSOR_ARTIFACT_FORMAT, PROCESSOR_ARTIFACT_FORMAT_VERSION as AOT_METADATA_FORMAT_VERSION,
    PROCESSOR_SNAPSHOT_FORMAT_VERSION as AOT_SNAPSHOT_FORMAT_VERSION,
};

#[cfg(feature = "llvm-orc")]
pub use onda_processor_abi::IntegrationProfile as AotIntegrationProfile;

#[cfg(feature = "llvm-orc")]
use onda_processor_abi::{
    BufferMetadata as AotBufferMetadata, CompileInfo as AotCompileInfo,
    EventMetadata as AotEventMetadata, EventParamMetadata as AotEventParamMetadata,
    Exports as AotExports, IntegrationInfo as AotIntegrationInfo, IoMetadata as AotIoMetadata,
    ProgramMetadata as AotProgramMetadata, RuntimeInfo as AotRuntimeInfo,
    TargetInfo as AotTargetInfo,
};

#[cfg(feature = "llvm-orc")]
use crate::mir_metadata::{build_mir_program_metadata, MirMetadataError, MirMetadataLayoutView};
#[cfg(feature = "llvm-orc")]
use crate::runtime_metadata::ProgramMetadata;
#[cfg(feature = "llvm-orc")]
use crate::{DeclaredBufferChannels, TargetConfig};

#[derive(Debug, Clone, Serialize)]
pub struct AotObjectArtifact {
    pub object_bytes: Vec<u8>,
    pub metadata: AotMetadata,
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
    data_layout: String,
    pointer_width_bits: u32,
    byte_order: &'static str,
    state_size_bytes: usize,
    state_align_bytes: usize,
    param_size_bytes: usize,
    param_align_bytes: usize,
) -> Result<AotMetadata, MirMetadataError> {
    let metadata = build_mir_program_metadata(program, layout)?;
    Ok(build_aot_metadata_from_descriptors(
        metadata,
        program.schema_version,
        program.interface.events.len(),
        program.config.sample_rate,
        program.config.block_size as usize,
        fast_math,
        target,
        resolved_triple,
        resolved_cpu,
        resolved_features,
        data_layout,
        pointer_width_bits,
        byte_order,
        state_size_bytes,
        state_align_bytes,
        param_size_bytes,
        param_align_bytes,
    ))
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "llvm-orc")]
fn build_aot_metadata_from_descriptors(
    metadata: ProgramMetadata,
    mir_schema_version: u32,
    event_count: usize,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: &TargetConfig,
    resolved_triple: String,
    resolved_cpu: String,
    resolved_features: String,
    data_layout: String,
    pointer_width_bits: u32,
    byte_order: &'static str,
    state_size_bytes: usize,
    state_align_bytes: usize,
    param_size_bytes: usize,
    param_align_bytes: usize,
) -> AotMetadata {
    let snapshot_size_bytes = metadata
        .state_entries
        .last()
        .map_or(0, |state| state.byte_offset() + state.byte_size());
    let event_exports = (0..event_count)
        .map(|idx| format!("onda_event_{idx}"))
        .collect::<Vec<_>>();
    let mut required_symbols = vec!["onda_init".to_owned(), "onda_process".to_owned()];
    required_symbols.extend(event_exports.iter().cloned());
    let is_wasm = resolved_triple.starts_with("wasm32-") || resolved_triple.starts_with("wasm64-");
    let integration_profile = if is_wasm {
        AotIntegrationProfile::WebassemblyRelocatableObject {
            symbol_visibility: "linker_managed".to_owned(),
            no_entry: true,
            export_memory: true,
        }
    } else {
        AotIntegrationProfile::NativeRelocatableObject {
            symbol_visibility: "linker_managed".to_owned(),
        }
    };
    AotMetadata {
        format: PROCESSOR_ARTIFACT_FORMAT.to_owned(),
        format_version: AOT_METADATA_FORMAT_VERSION,
        artifact_kind: "relocatable_object".to_owned(),
        abi_version: PROCESSOR_ABI_VERSION,
        backend: "llvm".to_owned(),
        mir_schema_version,
        target: AotTargetInfo {
            triple: resolved_triple,
            cpu: resolved_cpu,
            features: resolved_features,
            reloc_model: target.reloc_model.as_str().to_owned(),
            code_model: target.code_model.as_str().to_owned(),
            opt_level: target.opt_level.as_str().to_owned(),
            abi_name: target.abi_name.clone(),
            data_layout,
            pointer_width_bits,
            byte_order: byte_order.to_owned(),
            pointer_model: if is_wasm {
                "linear_memory_offset".to_owned()
            } else {
                "native_address".to_owned()
            },
            calling_convention: "c".to_owned(),
        },
        integration: AotIntegrationInfo {
            required_symbols,
            one_processor_per_artifact: true,
            profile: integration_profile,
        },
        compile: AotCompileInfo {
            sample_rate,
            block_size,
            fast_math,
        },
        exports: AotExports {
            init: "onda_init".to_owned(),
            process: "onda_process".to_owned(),
            events: event_exports,
        },
        runtime: AotRuntimeInfo {
            state_size_bytes,
            state_align_bytes,
            param_size_bytes,
            param_align_bytes,
            state_initialization: "zeroed".to_owned(),
            snapshot_size_bytes,
            snapshot_format_version: AOT_SNAPSHOT_FORMAT_VERSION,
            snapshot_byte_order: "little_endian".to_owned(),
            snapshot_restore_base: "post_init_physical_state_image".to_owned(),
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
        default_reprs: param
            .default_values()
            .map(|values| values.iter().copied().map(format_const_value).collect()),
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
