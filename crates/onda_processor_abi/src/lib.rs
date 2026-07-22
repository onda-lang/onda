//! Backend-neutral, versioned descriptions of compiled Onda processor artifacts.
//!
//! This crate deliberately has no compiler or backend dependencies. Code generators
//! serialize these types and host adapters deserialize them.

use serde::{Deserialize, Serialize};

pub const PROCESSOR_ARTIFACT_FORMAT: &str = "onda-processor";
// Synchronized from format-versions.json; do not edit these copies directly.
pub const PROCESSOR_ARTIFACT_FORMAT_VERSION: u32 = 1;
pub const PROCESSOR_ABI_VERSION: u32 = 1;
pub const PROCESSOR_SNAPSHOT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorDescriptor {
    pub format: String,
    pub format_version: u32,
    pub artifact_kind: String,
    pub abi_version: u32,
    pub backend: String,
    pub mir_schema_version: u32,
    pub target: TargetInfo,
    pub integration: IntegrationInfo,
    pub compile: CompileInfo,
    pub exports: Exports,
    pub runtime: RuntimeInfo,
    pub metadata: ProgramMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimization: Option<OptimizationInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<IntegrityInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInfo {
    pub required_symbols: Vec<String>,
    pub one_processor_per_artifact: bool,
    pub profile: IntegrationProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntegrationProfile {
    NativeRelocatableObject {
        symbol_visibility: String,
    },
    WebassemblyRelocatableObject {
        symbol_visibility: String,
        no_entry: bool,
        export_memory: bool,
    },
    CoreWebassemblyModule {
        imports: Vec<String>,
        memory_export: String,
        heap_base_export: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetInfo {
    pub triple: String,
    pub cpu: String,
    pub features: String,
    pub reloc_model: String,
    pub code_model: String,
    pub opt_level: String,
    pub abi_name: Option<String>,
    pub data_layout: String,
    pub pointer_width_bits: u32,
    pub byte_order: String,
    pub pointer_model: String,
    pub calling_convention: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileInfo {
    pub sample_rate: f32,
    pub block_size: usize,
    pub fast_math: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationInfo {
    pub enabled: bool,
    pub level: u32,
    pub shrink_level: u32,
    pub fast_math: bool,
    pub simd: bool,
    pub inline_functions_with_loops: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityInfo {
    pub algorithm: String,
    pub wasm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exports {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_base: Option<String>,
    pub init: String,
    pub process: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub state_size_bytes: usize,
    pub state_align_bytes: usize,
    pub param_size_bytes: usize,
    pub param_align_bytes: usize,
    pub state_initialization: String,
    pub snapshot_size_bytes: usize,
    pub snapshot_format_version: u32,
    pub snapshot_byte_order: String,
    pub snapshot_restore_base: String,
    pub requires_full_blocks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramMetadata {
    pub inputs: Vec<IoMetadata>,
    pub outputs: Vec<IoMetadata>,
    pub control_outputs: Vec<IoMetadata>,
    pub params: Vec<IoMetadata>,
    pub buffers: Vec<BufferMetadata>,
    pub events: Vec<EventMetadata>,
    pub states: Vec<StateMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoMetadata {
    pub name: String,
    pub type_repr: String,
    pub scalar: String,
    pub array_len: usize,
    pub element_size_bytes: usize,
    pub slot_offset: usize,
    pub byte_offset: Option<usize>,
    pub state_byte_offset: Option<usize>,
    pub byte_size: usize,
    pub default_reprs: Option<Vec<String>>,
    pub range_min_repr: Option<String>,
    pub range_max_repr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferMetadata {
    pub name: String,
    pub type_repr: String,
    pub scalar: String,
    pub element_size_bytes: usize,
    pub channels: String,
    pub static_channels: Option<usize>,
    pub access: String,
    pub may_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    pub name: String,
    pub export: String,
    pub payload_size_bytes: Option<usize>,
    pub payload_min_size_bytes: usize,
    pub has_dynamic_payload: bool,
    pub params: Vec<EventParamMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventParamMetadata {
    pub name: String,
    pub type_repr: String,
    pub scalar: String,
    pub array_len: usize,
    pub is_slice: bool,
    pub byte_offset: Option<usize>,
    pub byte_size: Option<usize>,
    pub element_size_bytes: usize,
    pub has_default: bool,
    /// Scalar spellings in wire order. Arrays contain one entry per element.
    pub default_reprs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMetadata {
    pub name: String,
    pub type_repr: String,
    pub scalar: String,
    pub array_len: usize,
    pub element_size_bytes: usize,
    pub packed_snapshot_byte_offset: usize,
    pub physical_state_byte_offset: usize,
    pub byte_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_web_descriptor_fixture_round_trips_through_rust_schema() {
        let json = include_str!(
            "../../../packages/onda_processor_abi/test/fixtures/processor-descriptor-v1.json"
        );
        let descriptor: ProcessorDescriptor =
            serde_json::from_str(json).expect("shared descriptor should deserialize");
        assert!(matches!(
            descriptor.integration.profile,
            IntegrationProfile::CoreWebassemblyModule { .. }
        ));
        assert_eq!(descriptor.metadata.inputs[0].type_repr, "f32");
        assert!(descriptor.metadata.buffers[0].may_write);
        let encoded = serde_json::to_string(&descriptor).expect("descriptor should serialize");
        serde_json::from_str::<ProcessorDescriptor>(&encoded)
            .expect("serialized descriptor should deserialize");
    }
}
