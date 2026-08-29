//! Backend-neutral, versioned descriptions of compiled Onda processor artifacts.
//!
//! This crate deliberately has no compiler or backend dependencies. Code generators
//! serialize these types and host adapters deserialize them.

use serde::{Deserialize, Serialize};

pub const PROCESSOR_ARTIFACT_FORMAT: &str = "onda-processor";
// Synchronized from format-versions.json; do not edit these copies directly.
pub const PROCESSOR_ARTIFACT_FORMAT_VERSION: u32 = 5;
pub const PROCESSOR_ABI_VERSION: u32 = 5;
pub const PROCESSOR_EXECUTION_OK: u32 = 0;
pub const PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE: u32 = 1;
pub const PROCESSOR_INIT_PRESERVE_PINNED: u32 = 0;
pub const PROCESSOR_INIT_FULL: u32 = 1;
pub const PROCESSOR_SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Packed occurrence headers: stream-local index, payload byte count, and call-local sequence.
pub const DELEGATE_RECORD_HEADER_SIZE: usize = 12;
pub const PRINT_RECORD_HEADER_SIZE: usize = 12;

/// Caller-owned, call-scoped storage for top-level delegate occurrences.
///
/// Generated init, process, and event entries reset the three result counters at
/// entry. `storage` may be null; in that neutral configuration publication is
/// discarded without counting overflow.
#[repr(C)]
#[derive(Debug)]
pub struct DelegateBatch {
    pub storage: *mut u8,
    pub capacity_bytes: u32,
    pub used_bytes: u32,
    pub record_count: u32,
    pub overflow_count: u32,
}

impl DelegateBatch {
    pub fn from_storage(storage: &mut [u8]) -> Self {
        Self {
            storage: storage.as_mut_ptr(),
            capacity_bytes: u32::try_from(storage.len()).unwrap_or(u32::MAX),
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
        }
    }

    pub const fn absent() -> Self {
        Self {
            storage: std::ptr::null_mut(),
            capacity_bytes: 0,
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }
}

/// Caller-owned, call-scoped storage for authored print occurrences.
#[repr(C)]
#[derive(Debug)]
pub struct PrintBatch {
    pub storage: *mut u8,
    pub capacity_bytes: u32,
    pub used_bytes: u32,
    pub record_count: u32,
    pub overflow_count: u32,
}

impl PrintBatch {
    pub fn from_storage(storage: &mut [u8]) -> Self {
        Self {
            storage: storage.as_mut_ptr(),
            capacity_bytes: u32::try_from(storage.len()).unwrap_or(u32::MAX),
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
        }
    }

    pub const fn absent() -> Self {
        Self {
            storage: std::ptr::null_mut(),
            capacity_bytes: 0,
            used_bytes: 0,
            record_count: 0,
            overflow_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.used_bytes = 0;
        self.record_count = 0;
        self.overflow_count = 0;
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct ExecutionOutput {
    pub delegate_batch: *mut DelegateBatch,
    pub print_batch: *mut PrintBatch,
    /// Call-local sequence assigned to the next print or delegate publication.
    pub next_sequence: u32,
}

impl ExecutionOutput {
    pub const fn none() -> Self {
        Self {
            delegate_batch: std::ptr::null_mut(),
            print_batch: std::ptr::null_mut(),
            next_sequence: 0,
        }
    }
}

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
    pub delegate_record_header_size_bytes: usize,
    pub print_record_header_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramMetadata {
    pub source_files: Vec<SourceFileMetadata>,
    pub log_sites: Vec<LogSiteMetadata>,
    pub inputs: Vec<IoMetadata>,
    pub outputs: Vec<IoMetadata>,
    pub control_outputs: Vec<IoMetadata>,
    pub params: Vec<IoMetadata>,
    pub buffers: Vec<BufferMetadata>,
    #[serde(default)]
    pub buffer_arrays: Vec<BufferArrayMetadata>,
    pub events: Vec<EventMetadata>,
    pub delegates: Vec<DelegateMetadata>,
    pub states: Vec<StateMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFileMetadata {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpanMetadata {
    pub file: Option<usize>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSiteMetadata {
    pub index: usize,
    pub label: Option<String>,
    pub source: SourceSpanMetadata,
    pub lexical_owner: String,
    pub declaration: Option<String>,
    pub argument_types: Vec<String>,
    pub payload_size_bytes: usize,
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
    pub param_control: Option<ParamControlMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamControlMetadata {
    pub scale: String,
    pub curve: Option<f64>,
    pub unit: Option<String>,
    pub step_repr: Option<String>,
    pub step_count: Option<u32>,
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
pub struct BufferArrayMetadata {
    pub name: String,
    pub first_buffer: usize,
    pub len: usize,
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
pub struct DelegateMetadata {
    pub index: usize,
    pub name: String,
    pub payload_size_bytes: Option<usize>,
    pub payload_min_size_bytes: usize,
    pub has_dynamic_payload: bool,
    pub params: Vec<DelegateParamMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateParamMetadata {
    pub name: String,
    pub type_repr: String,
    pub scalar: String,
    pub array_len: usize,
    pub is_slice: bool,
    pub byte_offset: Option<usize>,
    pub byte_size: Option<usize>,
    pub element_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMetadata {
    pub name: String,
    #[serde(default = "default_true")]
    pub authored: bool,
    pub type_repr: String,
    pub scalar: String,
    pub array_len: usize,
    pub element_size_bytes: usize,
    pub packed_snapshot_byte_offset: usize,
    pub physical_state_byte_offset: usize,
    pub byte_size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integer_range: Option<IntegerRangeMetadata>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegerRangeMetadata {
    pub min: IntegerRangeEndpoint,
    pub max: IntegerRangeEndpoint,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegerRangeEndpoint {
    #[serde(rename = "type")]
    pub scalar: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_web_descriptor_fixture_round_trips_through_rust_schema() {
        let json = include_str!(
            "../../../packages/onda_processor_abi/test/fixtures/processor-descriptor-v5.json"
        );
        let descriptor: ProcessorDescriptor =
            serde_json::from_str(json).expect("shared descriptor should deserialize");
        assert!(matches!(
            descriptor.integration.profile,
            IntegrationProfile::CoreWebassemblyModule { .. }
        ));
        assert_eq!(descriptor.metadata.inputs[0].type_repr, "f32");
        assert!(descriptor.metadata.buffers[0].may_write);
        assert_eq!(
            descriptor.metadata.states[0]
                .integer_range
                .as_ref()
                .map(|range| range.mode.as_str()),
            Some("wrap")
        );
        let encoded = serde_json::to_string(&descriptor).expect("descriptor should serialize");
        serde_json::from_str::<ProcessorDescriptor>(&encoded)
            .expect("serialized descriptor should deserialize");
    }
}
