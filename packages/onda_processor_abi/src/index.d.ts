export const PROCESSOR_ARTIFACT_FORMAT: "onda-processor";
// Synchronized from format-versions.json; do not edit these copies directly.
export const PROCESSOR_ARTIFACT_FORMAT_VERSION: 5;
export const PROCESSOR_ABI_VERSION: 5;
export const PROCESSOR_EXECUTION_OK: 0;
export const PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE: 1;
export const PROCESSOR_INIT_PRESERVE_PINNED: 0;
export const PROCESSOR_INIT_FULL: 1;
export type OndaProcessorInitMode = 0 | 1;
export const PROCESSOR_SNAPSHOT_FORMAT_VERSION: 1;
/** Bytes preceding the payload of every packed delegate occurrence. */
export const DELEGATE_RECORD_HEADER_SIZE_BYTES: 12;
/** wasm32 byte size of the call-scoped delegate batch descriptor. */
export const DELEGATE_BATCH_SIZE_BYTES: 20;
/** Bytes preceding the payload of every packed print occurrence. */
export const PRINT_RECORD_HEADER_SIZE_BYTES: 12;
/** wasm32 byte size of the call-scoped print batch descriptor. */
export const PRINT_BATCH_SIZE_BYTES: 20;
/** wasm32 byte size of the call-scoped execution-output descriptor. */
export const EXECUTION_OUTPUT_SIZE_BYTES: 12;

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
  param_control: OndaParamControlMetadata | null;
}

export interface OndaParamControlMetadata {
  scale: "linear" | "log";
  /** SuperCollider-style lincurve value; null selects linear/log scale directly. */
  curve: number | null;
  unit: string | null;
  step_repr: string | null;
  step_count: number | null;
}

export interface OndaPreparedParamControl {
  readonly name: string | null;
  readonly scalar: OndaScalarType;
  readonly minimum: number | null;
  readonly maximum: number | null;
  readonly scale: "linear" | "log" | null;
  readonly curve: number | null;
  readonly unit: string | null;
  readonly step: number | null;
  readonly stepCount: number | null;
  constrainPlain(plain: number | boolean): number | boolean;
  normalizedToPlain(normalized: number): number | boolean;
  plainToNormalized(plain: number | boolean): number;
}

export interface OndaParamDomain {
  name?: string | null;
  scalar: OndaScalarType;
  minimum: number | null;
  maximum: number | null;
  scale: "linear" | "log" | null;
  curve?: number | null;
  unit?: string | null;
  step?: number | null;
  stepCount?: number | null;
}

/** Validate an already-decoded domain once for repeated host-control use. */
export function createParamDomain(domain: OndaParamDomain): OndaPreparedParamControl;

/** Validate and decode descriptor metadata once for repeated host-control use. */
export function createParamControl(param: OndaIoMetadata): OndaPreparedParamControl;

/** Clamp and snap a plain scalar value according to descriptor metadata. */
export function constrainParamPlain(
  param: OndaIoMetadata,
  plain: number | boolean,
): number | boolean;

/** Convert a normalized host value to its canonical plain scalar value. */
export function paramNormalizedToPlain(
  param: OndaIoMetadata,
  normalized: number,
): number | boolean;

/** Convert a plain scalar value to its canonical normalized host value. */
export function paramPlainToNormalized(
  param: OndaIoMetadata,
  plain: number | boolean,
): number;

export interface OndaBufferMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  element_size_bytes: number;
  channels: "mono" | "static" | "dynamic";
  static_channels: number | null;
  /** Declared buffer access capability. */
  access: "read_only" | "read_write";
  /** Whether reachable code may write this physical slot; unresolved selectors are conservative. */
  may_write: boolean;
}

export interface OndaBufferArrayMetadata {
  name: string;
  first_buffer: number;
  len: number;
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

export interface OndaDelegateParamMetadata {
  name: string;
  type_repr: string;
  scalar: OndaScalarType;
  array_len: number;
  is_slice: boolean;
  byte_offset: number | null;
  byte_size: number | null;
  element_size_bytes: number;
}

export interface OndaDelegateMetadata {
  index: number;
  name: string;
  payload_size_bytes: number | null;
  payload_min_size_bytes: number;
  has_dynamic_payload: boolean;
  params: OndaDelegateParamMetadata[];
}

export interface OndaStateMetadata {
  name: string;
  /** False for compiler-owned snapshot storage omitted from authored-state reflection. */
  authored?: boolean;
  type_repr: string;
  scalar: OndaScalarType;
  array_len: number;
  element_size_bytes: number;
  packed_snapshot_byte_offset: number;
  physical_state_byte_offset: number;
  byte_size: number;
  integer_range?: {
    min: { type: "i32" | "i64"; value: string };
    max: { type: "i32" | "i64"; value: string };
    mode: "clamp" | "wrap";
  } | null;
}

export interface OndaProcessorMetadata {
  format: "onda-processor";
  format_version: 5;
  artifact_kind: OndaArtifactKind;
  abi_version: 5;
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
    delegate_record_header_size_bytes: 12;
    print_record_header_size_bytes: 12;
  };
  metadata: {
    source_files: Array<{ path: string }>;
    log_sites: OndaLogSiteMetadata[];
    inputs: OndaIoMetadata[];
    outputs: OndaIoMetadata[];
    control_outputs: OndaIoMetadata[];
    params: OndaIoMetadata[];
    buffers: OndaBufferMetadata[];
    buffer_arrays: OndaBufferArrayMetadata[];
    events: OndaEventMetadata[];
    delegates: OndaDelegateMetadata[];
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

export interface OndaLogSiteMetadata {
  index: number;
  label: string | null;
  source: { file: number | null; line: number; column: number; end_line: number; end_column: number };
  lexical_owner: string;
  declaration: string | null;
  argument_types: Array<"f32" | "f64" | "i32" | "i64" | "bool">;
  payload_size_bytes: number;
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

export interface OndaDelegateBatch {
  storageAddress: number;
  capacityBytes: number;
  usedBytes: number;
  recordCount: number;
  overflowCount: number;
}

export type OndaPrintBatch = OndaDelegateBatch;
export interface OndaPrintEntry {
  siteIndex: number;
  sequence: number;
  label: string | null;
  source: OndaLogSiteMetadata["source"];
  lexicalOwner: string;
  declaration: string | null;
  values: Array<{ type: "f32" | "f64" | "i32" | "i64" | "bool"; value: number | bigint | boolean }>;
}

export interface OndaDelegateOccurrence {
  delegateIndex: number;
  sequence: number;
  name: string;
  payloadByteLength: number;
  payload: Uint8Array;
  values: Record<string, number | bigint | boolean | Array<number | bigint | boolean>>;
}

/** Initialize one reusable wasm32 batch descriptor and clear its result counters. */
export function writeDelegateBatch(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  batchAddress: number,
  storageAddress: number,
  capacityBytes: number,
): void;
/** Read and validate the result descriptor after successful generated execution. */
export function readDelegateBatch(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  batchAddress: number,
): OndaDelegateBatch;
export function writePrintBatch(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  batchAddress: number,
  storageAddress: number,
  capacityBytes: number,
): void;
export function readPrintBatch(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  batchAddress: number,
): OndaPrintBatch;
export function writeExecutionOutput(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  outputAddress: number,
  delegateBatchAddress?: number,
  printBatchAddress?: number,
): void;
/** Prepare every present batch and the shared sequence for one processor entry call. */
export function resetExecutionOutput(
  memory: WebAssembly.Memory | ArrayBuffer | ArrayBufferView | DataView,
  outputAddress: number,
): void;
export function decodePrintRecords(
  storage: Uint8Array | ArrayBuffer | ArrayBufferView,
  usedBytes: number,
  logSites: OndaLogSiteMetadata[],
  byteOrder?: "little_endian" | "big_endian",
): OndaPrintEntry[];
export function formatPrintBatch(
  memory: WebAssembly.Memory,
  printBatchAddress: number,
  metadata: OndaProcessorMetadata | { log_sites: OndaLogSiteMetadata[]; target?: OndaTargetInfo },
): { text: string; entries: OndaPrintEntry[]; overflowCount: number };
export function formatPrintRecords(
  storage: Uint8Array | ArrayBuffer | ArrayBufferView,
  usedBytes: number,
  metadata: OndaProcessorMetadata | { log_sites: OndaLogSiteMetadata[]; target?: OndaTargetInfo },
  overflowCount?: number,
): { text: string; entries: OndaPrintEntry[]; overflowCount: number };
/** Decode every complete record in used storage according to delegate metadata. */
export function decodeDelegateRecords(
  storage: Uint8Array | ArrayBuffer | ArrayBufferView,
  usedBytes: number,
  delegates: OndaDelegateMetadata[],
  byteOrder?: "little_endian" | "big_endian",
): OndaDelegateOccurrence[];
