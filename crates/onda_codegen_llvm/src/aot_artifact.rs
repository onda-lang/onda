#[cfg(feature = "llvm-orc")]
use onda_mir::ScalarValue;
use serde::Serialize;

pub use onda_processor_abi::{
    ProcessorDescriptor as AotMetadata, StateMetadata as AotStateMetadata, PROCESSOR_ABI_VERSION,
    PROCESSOR_ARTIFACT_FORMAT, PROCESSOR_ARTIFACT_FORMAT_VERSION as AOT_METADATA_FORMAT_VERSION,
    PROCESSOR_EXECUTION_OK, PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
    PROCESSOR_SNAPSHOT_FORMAT_VERSION as AOT_SNAPSHOT_FORMAT_VERSION,
};

#[cfg(feature = "llvm-orc")]
pub use onda_processor_abi::IntegrationProfile as AotIntegrationProfile;

#[cfg(feature = "llvm-orc")]
use onda_processor_abi::{
    BufferMetadata as AotBufferMetadata, CompileInfo as AotCompileInfo,
    DelegateMetadata as AotDelegateMetadata, DelegateParamMetadata as AotDelegateParamMetadata,
    EventMetadata as AotEventMetadata, EventParamMetadata as AotEventParamMetadata,
    Exports as AotExports, IntegerRangeEndpoint, IntegerRangeMetadata,
    IntegrationInfo as AotIntegrationInfo, IoMetadata as AotIoMetadata,
    ParamControlMetadata as AotParamControlMetadata, ProgramMetadata as AotProgramMetadata,
    RuntimeInfo as AotRuntimeInfo, TargetInfo as AotTargetInfo,
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
    let mut descriptor = build_aot_metadata_from_descriptors(
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
    );
    descriptor.metadata.source_files = program
        .source_files
        .iter()
        .map(|file| onda_processor_abi::SourceFileMetadata {
            path: file.path.clone(),
        })
        .collect();
    descriptor.metadata.log_sites = program
        .log_sites
        .iter()
        .enumerate()
        .map(|(index, site)| onda_processor_abi::LogSiteMetadata {
            index,
            label: site.label.clone(),
            source: onda_processor_abi::SourceSpanMetadata {
                file: site.source.file.map(|file| file.index()),
                line: site.source.line,
                column: site.source.column,
                end_line: site.source.end_line,
                end_column: site.source.end_column,
            },
            lexical_owner: site.lexical_owner.clone(),
            declaration: site.declaration.clone(),
            argument_types: site
                .argument_types
                .iter()
                .map(|scalar| format!("{scalar:?}").to_ascii_lowercase())
                .collect(),
            payload_size_bytes: site.payload_size as usize,
        })
        .collect();
    Ok(descriptor)
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
    let mut required_symbols = vec!["onda_processor_init".to_owned(), "onda_process".to_owned()];
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
            memory: None,
            heap_base: None,
            init: "onda_processor_init".to_owned(),
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
            requires_full_blocks: false,
            delegate_record_header_size_bytes: onda_processor_abi::DELEGATE_RECORD_HEADER_SIZE,
            print_record_header_size_bytes: onda_processor_abi::PRINT_RECORD_HEADER_SIZE,
        },
        metadata: AotProgramMetadata {
            source_files: Vec::new(),
            log_sites: Vec::new(),
            inputs: metadata.inputs.iter().map(map_io_metadata).collect(),
            outputs: metadata.outputs.iter().map(map_io_metadata).collect(),
            control_outputs: metadata
                .control_outputs
                .iter()
                .map(map_io_metadata)
                .collect(),
            params: metadata.params.iter().map(map_io_metadata).collect(),
            buffers: metadata.buffers.iter().map(map_buffer_metadata).collect(),
            buffer_arrays: metadata
                .buffer_arrays
                .iter()
                .map(|array| onda_processor_abi::BufferArrayMetadata {
                    name: array.name().to_owned(),
                    first_buffer: array.first(),
                    len: array.len(),
                })
                .collect(),
            events: metadata
                .events
                .iter()
                .enumerate()
                .map(|(index, event)| map_event_metadata(index, event))
                .collect(),
            delegates: metadata
                .delegates
                .iter()
                .enumerate()
                .map(|(index, delegate)| map_delegate_metadata(index, delegate))
                .collect(),
            states: metadata
                .state_entries
                .iter()
                .map(map_state_metadata)
                .collect(),
        },
        required_features: Vec::new(),
        optimization: None,
        integrity: None,
    }
}

#[cfg(feature = "llvm-orc")]
fn map_io_metadata(io: &crate::DeclaredIo) -> AotIoMetadata {
    AotIoMetadata {
        name: io.name().to_owned(),
        type_repr: io.type_repr(),
        scalar: primitive_type_name(io.elem_ty()).to_owned(),
        array_len: io.array_len(),
        element_size_bytes: crate::primitives::primitive_type_bytes(io.elem_ty()),
        slot_offset: io.slot_offset(),
        byte_offset: Some(io.byte_offset()),
        state_byte_offset: io.state_byte_offset(),
        byte_size: io.byte_size(),
        default_reprs: io
            .default_values()
            .map(|values| values.iter().copied().map(format_const_value).collect()),
        range_min_repr: io.range().map(|range| format_const_value(range.min)),
        range_max_repr: io.range().map(|range| format_const_value(range.max)),
        param_control: io.param_control().map(|control| AotParamControlMetadata {
            scale: match control.scale {
                onda_mir::ParamScale::Linear => "linear",
                onda_mir::ParamScale::Log => "log",
            }
            .to_owned(),
            curve: control.curve,
            unit: control.unit.clone(),
            step_repr: control.step.map(format_const_value),
            step_count: control.step_count,
        }),
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
        scalar: primitive_type_name(buffer.elem_ty()).to_owned(),
        element_size_bytes: crate::primitives::primitive_type_bytes(buffer.elem_ty()),
        channels,
        static_channels,
        access: match buffer.access() {
            onda_mir::AccessMode::ReadOnly => "read_only",
            onda_mir::AccessMode::ReadWrite => "read_write",
        }
        .to_owned(),
        may_write: buffer.may_write(),
    }
}

#[cfg(feature = "llvm-orc")]
fn map_event_metadata(index: usize, event: &crate::DeclaredEvent) -> AotEventMetadata {
    let params = event
        .params()
        .iter()
        .map(|param| map_event_param_metadata(param, param.byte_offset()))
        .collect();
    AotEventMetadata {
        name: event.name().to_owned(),
        export: format!("onda_event_{index}"),
        payload_size_bytes: event.payload_bytes(),
        payload_min_size_bytes: event.payload_min_bytes(),
        has_dynamic_payload: event.payload_bytes().is_none(),
        params,
    }
}

#[cfg(feature = "llvm-orc")]
fn map_delegate_metadata(index: usize, delegate: &crate::DeclaredDelegate) -> AotDelegateMetadata {
    let params = delegate
        .params()
        .iter()
        .map(|param| AotDelegateParamMetadata {
            name: param.name().to_owned(),
            type_repr: param.type_repr(),
            scalar: primitive_type_name(param.elem_ty()).to_owned(),
            array_len: param.array_len(),
            is_array: param.is_array(),
            is_slice: param.is_slice(),
            byte_offset: param.byte_offset(),
            byte_size: param.byte_size(),
            element_size_bytes: crate::primitives::primitive_type_bytes(param.elem_ty()),
        })
        .collect();
    AotDelegateMetadata {
        index,
        name: delegate.name().to_owned(),
        payload_size_bytes: delegate.payload_bytes(),
        payload_min_size_bytes: delegate.payload_min_bytes(),
        has_dynamic_payload: delegate.payload_bytes().is_none(),
        params,
    }
}

#[cfg(feature = "llvm-orc")]
fn map_state_metadata(state: &crate::DeclaredState) -> AotStateMetadata {
    AotStateMetadata {
        name: state.name().to_owned(),
        authored: state.is_authored(),
        type_repr: state.type_repr(),
        scalar: primitive_type_name(state.elem_ty()).to_owned(),
        array_len: state.array_len(),
        element_size_bytes: state.byte_size() / state.array_len(),
        packed_snapshot_byte_offset: state.byte_offset(),
        physical_state_byte_offset: state.storage_byte_offset(),
        byte_size: state.byte_size(),
        integer_range: state.integer_range().map(|range| {
            let endpoint = |value: ScalarValue| match value {
                ScalarValue::I32(value) => IntegerRangeEndpoint {
                    scalar: "i32".to_owned(),
                    value: value.to_string(),
                },
                ScalarValue::I64(value) => IntegerRangeEndpoint {
                    scalar: "i64".to_owned(),
                    value: value.to_string(),
                },
                _ => unreachable!("validated state integer range is integer-valued"),
            };
            IntegerRangeMetadata {
                min: endpoint(range.min),
                max: endpoint(range.max),
                mode: match range.mode {
                    onda_mir::IntegerRangeMode::Clamp => "clamp",
                    onda_mir::IntegerRangeMode::Wrap => "wrap",
                }
                .to_owned(),
            }
        }),
    }
}

#[cfg(feature = "llvm-orc")]
fn map_event_param_metadata(
    param: &crate::DeclaredEventParam,
    byte_offset: Option<usize>,
) -> AotEventParamMetadata {
    AotEventParamMetadata {
        name: param.name().to_owned(),
        type_repr: param.type_repr(),
        scalar: primitive_type_name(param.elem_ty()).to_owned(),
        array_len: param.array_len(),
        is_array: param.is_array(),
        is_slice: param.is_slice(),
        byte_offset,
        byte_size: param.byte_size(),
        element_size_bytes: crate::primitives::primitive_type_bytes(param.elem_ty()),
        has_default: param.has_default(),
        default_reprs: param
            .default_values()
            .map(|values| values.iter().copied().map(format_const_value).collect()),
    }
}

#[cfg(feature = "llvm-orc")]
fn primitive_type_name(ty: onda_frontend::PrimitiveType) -> &'static str {
    match ty {
        onda_frontend::PrimitiveType::F32 => "f32",
        onda_frontend::PrimitiveType::F64 => "f64",
        onda_frontend::PrimitiveType::I32 => "i32",
        onda_frontend::PrimitiveType::I64 => "i64",
        onda_frontend::PrimitiveType::Bool => "bool",
    }
}

#[cfg(feature = "llvm-orc")]
fn format_const_value(value: ScalarValue) -> String {
    match value {
        ScalarValue::F32(v) if v.is_finite() => v.to_string(),
        ScalarValue::F32(v) => format!("0x{:08x}", v.to_bits()),
        ScalarValue::F64(v) if v.is_finite() => v.to_string(),
        ScalarValue::F64(v) => format!("0x{:016x}", v.to_bits()),
        ScalarValue::I32(v) => v.to_string(),
        ScalarValue::I64(v) => v.to_string(),
        ScalarValue::Bool(v) => v.to_string(),
    }
}

#[cfg(all(test, feature = "llvm-orc"))]
mod tests {
    use super::*;
    use onda_frontend::PrimitiveType;

    #[test]
    fn dynamic_event_offsets_are_only_static_before_the_first_slice() {
        let event = crate::DeclaredEvent {
            name: "curve".to_owned(),
            params: vec![
                crate::DeclaredEventParam {
                    name: "enabled".to_owned(),
                    elem_ty: PrimitiveType::F32,
                    array_len: 1,
                    is_array: false,
                    is_slice: false,
                    byte_offset: Some(0),
                    default_bytes: None,
                    default_values: None,
                },
                crate::DeclaredEventParam {
                    name: "values".to_owned(),
                    elem_ty: PrimitiveType::F64,
                    array_len: 0,
                    is_array: false,
                    is_slice: true,
                    byte_offset: Some(4),
                    default_bytes: None,
                    default_values: None,
                },
                crate::DeclaredEventParam {
                    name: "stamp".to_owned(),
                    elem_ty: PrimitiveType::I64,
                    array_len: 1,
                    is_array: false,
                    is_slice: false,
                    byte_offset: None,
                    default_bytes: None,
                    default_values: None,
                },
            ],
            payload_bytes: None,
            payload_min_bytes: 16,
        };

        let mapped = map_event_metadata(0, &event);
        assert_eq!(mapped.payload_min_size_bytes, 16);
        assert_eq!(mapped.params[0].byte_offset, Some(0));
        assert_eq!(mapped.params[1].byte_offset, Some(4));
        assert_eq!(mapped.params[2].byte_offset, None);
    }
}
