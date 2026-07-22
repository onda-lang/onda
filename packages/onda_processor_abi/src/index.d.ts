export const PROCESSOR_ARTIFACT_FORMAT: "onda-processor";
// Synchronized from format-versions.json; do not edit these copies directly.
export const PROCESSOR_ARTIFACT_FORMAT_VERSION: 1;
export const PROCESSOR_ABI_VERSION: 1;
export const PROCESSOR_SNAPSHOT_FORMAT_VERSION: 1;

export type OndaScalarType = "f32" | "f64" | "i32" | "i64" | "bool";
export type OndaArtifactKind = "webassembly_module" | "relocatable_object";

export interface OndaTargetInfo {
  triple: string;
  cpu: string;
  features: string;
  reloc_model: string;
  code_model: string;
  opt_level: string;
  abi_name: string | null;
  data_layout: string;
  pointer_width_bits: number;
  byte_order: "little_endian" | "big_endian";
  pointer_model: "native_address" | "linear_memory_offset";
  calling_convention: string;
}

export type OndaIntegrationProfile =
  | { kind: "native_relocatable_object"; symbol_visibility: string }
  | {
      kind: "webassembly_relocatable_object";
      symbol_visibility: string;
      no_entry: boolean;
      export_memory: boolean;
    }
  | {
      kind: "core_webassembly_module";
      imports: string[];
      memory_export: string;
      heap_base_export: string;
    };

export interface OndaIoMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  array_len: number;
  element_size_bytes: number;
  slot_offset: number;
  byte_offset: number | null;
  state_byte_offset: number | null;
  byte_size: number;
  default_reprs: string[] | null;
  range_min_repr: string | null;
  range_max_repr: string | null;
}

export interface OndaBufferMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  element_size_bytes: number;
  channels: "mono" | "static" | "dynamic";
  static_channels: number | null;
  access: "read_only" | "read_write";
  may_write: boolean;
}

export interface OndaEventParamMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  array_len: number;
  is_slice: boolean;
  byte_offset: number | null;
  byte_size: number | null;
  element_size_bytes: number;
  has_default: boolean;
  default_reprs: string[] | null;
}

export interface OndaEventMetadata {
  name: string;
  export: string;
  payload_size_bytes: number | null;
  payload_min_size_bytes: number;
  has_dynamic_payload: boolean;
  params: OndaEventParamMetadata[];
}

export interface OndaStateMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  array_len: number;
  element_size_bytes: number;
  packed_snapshot_byte_offset: number;
  physical_state_byte_offset: number;
  byte_size: number;
}

export interface OndaProcessorMetadata {
  format: "onda-processor";
  format_version: 1;
  artifact_kind: OndaArtifactKind;
  abi_version: 1;
  backend: string;
  mir_schema_version: number;
  target: OndaTargetInfo;
  integration: {
    required_symbols: string[];
    one_processor_per_artifact: true;
    profile: OndaIntegrationProfile;
  };
  compile: { sample_rate: number; block_size: number; fast_math: boolean };
  exports: {
    memory?: string;
    heap_base?: string;
    init: string;
    process: string;
    events: string[];
  };
  runtime: {
    state_size_bytes: number;
    state_align_bytes: number;
    param_size_bytes: number;
    param_align_bytes: number;
    state_initialization: "zeroed";
    snapshot_size_bytes: number;
    snapshot_format_version: 1;
    snapshot_byte_order: "little_endian";
    snapshot_restore_base: "post_init_physical_state_image";
    requires_full_blocks: boolean;
  };
  metadata: {
    inputs: OndaIoMetadata[];
    outputs: OndaIoMetadata[];
    control_outputs: OndaIoMetadata[];
    params: OndaIoMetadata[];
    buffers: OndaBufferMetadata[];
    events: OndaEventMetadata[];
    states: OndaStateMetadata[];
  };
  required_features?: string[];
  optimization?: {
    enabled: boolean;
    level: number;
    shrink_level: number;
    fast_math: boolean;
    simd: boolean;
    inline_functions_with_loops: boolean;
  };
  integrity?: { algorithm: string; wasm: string };
}

export interface OndaProcessorArtifact {
  wasm: Uint8Array | ArrayBuffer | ArrayBufferView;
  metadata: OndaProcessorMetadata;
  wat?: string;
}

export class OndaArtifactError extends Error {}
export function validateProcessorMetadata(
  metadata: object,
  expectedKind?: OndaArtifactKind | null,
): OndaProcessorMetadata;
export function validateProcessorArtifact(
  artifact: OndaProcessorArtifact,
  options?: { inspectModule?: boolean },
): { wasm: Uint8Array; metadata: OndaProcessorMetadata };
export function validateProcessorModule(
  module: WebAssembly.Module,
  metadata: object,
): { module: WebAssembly.Module; metadata: OndaProcessorMetadata };
export function serializeProcessorMetadata(metadata: object, space?: number): string;
export function parseProcessorMetadata(
  input: string | object,
  expectedKind?: OndaArtifactKind | null,
): OndaProcessorMetadata;
export function createProcessorArtifactFiles(
  artifact: OndaProcessorArtifact,
  options?: { baseName?: string },
): Promise<{
  wasm: { name: string; mediaType: "application/wasm"; bytes: Uint8Array };
  metadata: {
    name: string;
    mediaType: "application/json";
    text: string;
    value: OndaProcessorMetadata;
  };
}>;
export function loadProcessorArtifactFiles(
  wasm: Uint8Array | ArrayBuffer | ArrayBufferView,
  metadata: string | object,
): Promise<{ wasm: Uint8Array; metadata: OndaProcessorMetadata }>;
