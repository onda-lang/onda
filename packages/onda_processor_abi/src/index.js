import "./param-control.js";

const PARAM_CONTROL = globalThis.__ONDA_PARAM_CONTROL_V2__;

export const PROCESSOR_ARTIFACT_FORMAT = "onda-processor";
// Synchronized from format-versions.json; do not edit these copies directly.
export const PROCESSOR_ARTIFACT_FORMAT_VERSION = 5;
export const PROCESSOR_ABI_VERSION = 5;
export const PROCESSOR_EXECUTION_OK = 0;
export const PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE = 1;
export const PROCESSOR_INIT_PRESERVE_PINNED = 0;
export const PROCESSOR_INIT_FULL = 1;
export const PROCESSOR_SNAPSHOT_FORMAT_VERSION = 1;
export const DELEGATE_RECORD_HEADER_SIZE_BYTES = 12;
export const DELEGATE_BATCH_SIZE_BYTES = 20;
export const PRINT_RECORD_HEADER_SIZE_BYTES = 12;
export const PRINT_BATCH_SIZE_BYTES = 20;
export const EXECUTION_OUTPUT_SIZE_BYTES = 12;

export const {
  createParamDomain,
  createParamControl,
  constrainParamPlain,
  paramNormalizedToPlain,
  paramPlainToNormalized,
} = PARAM_CONTROL;

export class OndaArtifactError extends Error {
  constructor(message) {
    super(message);
    this.name = "OndaArtifactError";
  }
}

export function validateProcessorMetadata(metadata, expectedKind = null) {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    throw new OndaArtifactError("processor metadata must be an object");
  }
  if (metadata.format !== PROCESSOR_ARTIFACT_FORMAT) {
    throw new OndaArtifactError(
      `unsupported processor metadata format '${String(metadata.format)}'`,
    );
  }
  if (metadata.format_version !== PROCESSOR_ARTIFACT_FORMAT_VERSION) {
    throw new OndaArtifactError(
      `unsupported processor metadata version ${String(metadata.format_version)}; expected ${PROCESSOR_ARTIFACT_FORMAT_VERSION}`,
    );
  }
  if (metadata.abi_version !== PROCESSOR_ABI_VERSION) {
    throw new OndaArtifactError(
      `unsupported processor ABI version ${String(metadata.abi_version)}; expected ${PROCESSOR_ABI_VERSION}`,
    );
  }
  if (
    metadata.artifact_kind !== "webassembly_module"
    && metadata.artifact_kind !== "relocatable_object"
  ) {
    throw new OndaArtifactError(
      `unsupported processor artifact kind '${String(metadata.artifact_kind)}'`,
    );
  }
  if (expectedKind !== null && metadata.artifact_kind !== expectedKind) {
    throw new OndaArtifactError(
      `expected processor artifact kind '${expectedKind}', got '${metadata.artifact_kind}'`,
    );
  }
  requireInteger(metadata.mir_schema_version, "mir_schema_version", 1);
  requireString(metadata.backend, "backend");
  requireString(metadata.target?.triple, "target.triple");
  requireString(metadata.target?.cpu, "target.cpu");
  requireString(metadata.target?.features, "target.features", true);
  requireString(metadata.target?.reloc_model, "target.reloc_model");
  requireString(metadata.target?.code_model, "target.code_model");
  requireString(metadata.target?.opt_level, "target.opt_level");
  if (metadata.target?.abi_name !== null) {
    requireString(metadata.target?.abi_name, "target.abi_name");
  }
  requireString(metadata.target?.data_layout, "target.data_layout");
  requireInteger(metadata.target?.pointer_width_bits, "target.pointer_width_bits", 1);
  if (
    metadata.target?.byte_order !== "little_endian"
    && metadata.target?.byte_order !== "big_endian"
  ) {
    throw new OndaArtifactError("target.byte_order must name a supported byte order");
  }
  if (
    metadata.target?.pointer_model !== "native_address"
    && metadata.target?.pointer_model !== "linear_memory_offset"
  ) {
    throw new OndaArtifactError("target.pointer_model must name a supported pointer model");
  }
  requireString(metadata.target?.calling_convention, "target.calling_convention");
  requirePositiveFinite(metadata.compile?.sample_rate, "compile.sample_rate");
  requireInteger(metadata.compile?.block_size, "compile.block_size", 1);
  requireBoolean(metadata.compile?.fast_math, "compile.fast_math");
  requireInteger(metadata.runtime?.state_size_bytes, "runtime.state_size_bytes", 0);
  requireInteger(metadata.runtime?.state_align_bytes, "runtime.state_align_bytes", 1);
  requireInteger(metadata.runtime?.param_size_bytes, "runtime.param_size_bytes", 0);
  requireInteger(metadata.runtime?.param_align_bytes, "runtime.param_align_bytes", 1);
  requireInteger(
    metadata.runtime?.snapshot_size_bytes,
    "runtime.snapshot_size_bytes",
    0,
  );
  requireInteger(
    metadata.runtime?.snapshot_format_version,
    "runtime.snapshot_format_version",
    1,
  );
  if (
    metadata.runtime.snapshot_format_version !== PROCESSOR_SNAPSHOT_FORMAT_VERSION
  ) {
    throw new OndaArtifactError(
      `unsupported processor snapshot version ${String(metadata.runtime.snapshot_format_version)}; expected ${PROCESSOR_SNAPSHOT_FORMAT_VERSION}`,
    );
  }
  requireLiteral(metadata.runtime?.state_initialization, "runtime.state_initialization", "zeroed");
  requireLiteral(metadata.runtime?.snapshot_byte_order, "runtime.snapshot_byte_order", "little_endian");
  requireLiteral(
    metadata.runtime?.snapshot_restore_base,
    "runtime.snapshot_restore_base",
    "post_init_physical_state_image",
  );
  requireLiteral(
    metadata.runtime?.print_record_header_size_bytes,
    "runtime.print_record_header_size_bytes",
    PRINT_RECORD_HEADER_SIZE_BYTES,
  );
  requireBoolean(metadata.runtime?.requires_full_blocks, "runtime.requires_full_blocks");
  requireLiteral(
    metadata.runtime?.delegate_record_header_size_bytes,
    "runtime.delegate_record_header_size_bytes",
    DELEGATE_RECORD_HEADER_SIZE_BYTES,
  );
  requireString(metadata.exports?.init, "exports.init");
  requireString(metadata.exports?.process, "exports.process");
  if (!Array.isArray(metadata.exports?.events)) {
    throw new OndaArtifactError("exports.events must be an array");
  }
  for (const name of metadata.exports.events) {
    requireString(name, "exports.events[]");
  }
  if (!Array.isArray(metadata.integration?.required_symbols)) {
    throw new OndaArtifactError("integration.required_symbols must be an array");
  }
  for (const name of metadata.integration.required_symbols) {
    requireString(name, "integration.required_symbols[]");
  }
  const requiredSymbols = new Set(metadata.integration.required_symbols);
  for (const name of [
    metadata.exports.init,
    metadata.exports.process,
    ...metadata.exports.events,
  ]) {
    if (!requiredSymbols.has(name)) {
      throw new OndaArtifactError(
        `integration.required_symbols is missing executable export '${name}'`,
      );
    }
  }
  if (metadata.integration?.one_processor_per_artifact !== true) {
    throw new OndaArtifactError(
      `integration.one_processor_per_artifact must be true for ABI version ${PROCESSOR_ABI_VERSION}`,
    );
  }
  const profileKind = metadata.integration?.profile?.kind;
  const expectedProfileKinds = metadata.artifact_kind === "webassembly_module"
    ? ["core_webassembly_module"]
    : ["native_relocatable_object", "webassembly_relocatable_object"];
  if (!expectedProfileKinds.includes(profileKind)) {
    throw new OndaArtifactError(
      `integration profile '${String(profileKind)}' is incompatible with artifact kind '${metadata.artifact_kind}'`,
    );
  }
  if (profileKind === "core_webassembly_module") {
    requireLiteral(metadata.target.byte_order, "target.byte_order", "little_endian");
    requireString(metadata.exports?.memory, "exports.memory");
    requireString(metadata.exports?.heap_base, "exports.heap_base");
    requireString(metadata.integration.profile?.memory_export, "integration.profile.memory_export");
    requireString(metadata.integration.profile?.heap_base_export, "integration.profile.heap_base_export");
    if (
      metadata.integration.profile.memory_export !== metadata.exports.memory
      || metadata.integration.profile.heap_base_export !== metadata.exports.heap_base
    ) {
      throw new OndaArtifactError(
        "core WebAssembly profile exports do not match the processor export table",
      );
    }
    for (const name of [metadata.exports.memory, metadata.exports.heap_base]) {
      if (!requiredSymbols.has(name)) {
        throw new OndaArtifactError(
          `integration.required_symbols is missing runtime export '${name}'`,
        );
      }
    }
    if (!Array.isArray(metadata.integration.profile.imports)) {
      throw new OndaArtifactError("integration.profile.imports must be an array");
    }
    for (const name of metadata.integration.profile.imports) {
      requireString(name, "integration.profile.imports[]");
    }
  } else {
    requireString(
      metadata.integration.profile?.symbol_visibility,
      "integration.profile.symbol_visibility",
    );
    if (profileKind === "webassembly_relocatable_object") {
      requireBoolean(metadata.integration.profile?.no_entry, "integration.profile.no_entry");
      requireBoolean(
        metadata.integration.profile?.export_memory,
        "integration.profile.export_memory",
      );
    }
  }
  if (!metadata.metadata || typeof metadata.metadata !== "object") {
    throw new OndaArtifactError("metadata payload must be an object");
  }
  for (const field of [
    "states",
    "inputs",
    "outputs",
    "control_outputs",
    "params",
    "buffers",
    "events",
    "delegates",
    "source_files",
    "log_sites",
  ]) {
    if (!Array.isArray(metadata.metadata[field])) {
      throw new OndaArtifactError(`metadata.${field} must be an array`);
    }
  }
  for (const field of ["inputs", "outputs", "control_outputs", "params"]) {
    metadata.metadata[field].forEach((entry, index) =>
      validateIoMetadata(
        entry,
        `metadata.${field}[${index}]`,
        field === "params",
      )
    );
  }
  metadata.metadata.buffers.forEach((entry, index) =>
    validateBufferMetadata(entry, `metadata.buffers[${index}]`)
  );
  const bufferArrays = metadata.metadata.buffer_arrays ?? [];
  if (!Array.isArray(bufferArrays)) {
    throw new OndaArtifactError("metadata.buffer_arrays must be an array");
  }
  const bufferArrayNames = new Set();
  const groupedBuffers = new Set();
  bufferArrays.forEach((entry, index) => {
    const path = `metadata.buffer_arrays[${index}]`;
    requireString(entry?.name, `${path}.name`);
    requireInteger(entry?.first_buffer, `${path}.first_buffer`, 0);
    requireInteger(entry?.len, `${path}.len`, 1);
    if (bufferArrayNames.has(entry.name)) {
      throw new OndaArtifactError(`${path}.name duplicates buffer array '${entry.name}'`);
    }
    bufferArrayNames.add(entry.name);
    if (entry.first_buffer + entry.len > metadata.metadata.buffers.length) {
      throw new OndaArtifactError(`${path} exceeds metadata.buffers`);
    }
    const first = metadata.metadata.buffers[entry.first_buffer];
    for (let slot = entry.first_buffer; slot < entry.first_buffer + entry.len; slot += 1) {
      if (groupedBuffers.has(slot)) {
        throw new OndaArtifactError(`${path} overlaps another buffer array at slot ${slot}`);
      }
      groupedBuffers.add(slot);
      const buffer = metadata.metadata.buffers[slot];
      if (
        buffer.scalar !== first.scalar ||
        buffer.channels !== first.channels ||
        buffer.static_channels !== first.static_channels ||
        buffer.access !== first.access
      ) {
        throw new OndaArtifactError(`${path} contains incompatible buffer descriptors`);
      }
    }
  });
  metadata.metadata.events.forEach((entry, index) =>
    validateEventMetadata(entry, `metadata.events[${index}]`)
  );
  metadata.metadata.delegates.forEach((entry, index) =>
    validateDelegateMetadata(entry, index, `metadata.delegates[${index}]`)
  );
  metadata.metadata.source_files.forEach((entry, index) =>
    requireString(entry?.path, `metadata.source_files[${index}].path`, true)
  );
  metadata.metadata.log_sites.forEach((entry, index) => {
    const path = `metadata.log_sites[${index}]`;
    requireInteger(entry?.index, `${path}.index`, 0);
    if (entry.index !== index) {
      throw new OndaArtifactError(`${path}.index must equal its descriptor index`);
    }
    if (entry.label !== null) requireString(entry.label, `${path}.label`, true);
    if (entry.declaration !== null) requireString(entry.declaration, `${path}.declaration`, true);
    requireString(entry?.lexical_owner, `${path}.lexical_owner`);
    requireInteger(entry?.payload_size_bytes, `${path}.payload_size_bytes`, 0);
    if (!Array.isArray(entry?.argument_types)) {
      throw new OndaArtifactError(`${path}.argument_types must be an array`);
    }
    entry.argument_types.forEach((scalar, scalarIndex) => {
      if (!["f32", "f64", "i32", "i64", "bool"].includes(scalar)) {
        throw new OndaArtifactError(`${path}.argument_types[${scalarIndex}] is not printable`);
      }
    });
    const expectedPayloadSize = entry.argument_types.reduce(
      (size, scalar) => size + (scalar === "f64" || scalar === "i64" ? 8 : scalar === "bool" ? 1 : 4),
      0,
    );
    if (entry.payload_size_bytes !== expectedPayloadSize) {
      throw new OndaArtifactError(
        `${path}.payload_size_bytes must be ${expectedPayloadSize} for its argument_types`,
      );
    }
    const source = entry?.source;
    if (source?.file !== null) {
      requireInteger(source?.file, `${path}.source.file`, 0);
      if (source.file >= metadata.metadata.source_files.length) {
        throw new OndaArtifactError(`${path}.source.file references a missing source file`);
      }
    }
    for (const field of ["line", "column", "end_line", "end_column"]) {
      requireInteger(source?.[field], `${path}.source.${field}`, 0);
    }
  });
  const describedEventExports = metadata.metadata.events.map((event) => event.export);
  if (
    describedEventExports.length !== metadata.exports.events.length
    || describedEventExports.some((name, index) => name !== metadata.exports.events[index])
  ) {
    throw new OndaArtifactError(
      "metadata.events exports must match exports.events in declaration order",
    );
  }
  metadata.metadata.states.forEach((entry, index) =>
    validateStateMetadata(entry, `metadata.states[${index}]`)
  );
  validateRuntimeLayouts(metadata);
  if (metadata.required_features !== undefined) {
    requireStringArray(metadata.required_features, "required_features");
  }
  if (metadata.optimization !== undefined) {
    requireBoolean(metadata.optimization?.enabled, "optimization.enabled");
    requireInteger(metadata.optimization?.level, "optimization.level", 0);
    requireInteger(metadata.optimization?.shrink_level, "optimization.shrink_level", 0);
    requireBoolean(metadata.optimization?.fast_math, "optimization.fast_math");
    requireBoolean(metadata.optimization?.simd, "optimization.simd");
    requireBoolean(
      metadata.optimization?.inline_functions_with_loops,
      "optimization.inline_functions_with_loops",
    );
  }
  if (metadata.integrity !== undefined) {
    requireString(metadata.integrity?.algorithm, "integrity.algorithm");
    requireString(metadata.integrity?.wasm, "integrity.wasm");
  }
  return metadata;
}

function validateDelegateMetadata(value, index, path) {
  requireInteger(value?.index, `${path}.index`, 0);
  if (value.index !== index) {
    throw new OndaArtifactError(`${path}.index must match declaration order`);
  }
  requireString(value?.name, `${path}.name`);
  validatePayloadMetadata(value, path, false);
}

function validateIoMetadata(value, path, isParameter) {
  requireString(value?.name, `${path}.name`);
  requireString(value?.type_repr, `${path}.type_repr`);
  requireScalar(value?.scalar, `${path}.scalar`);
  requireInteger(value?.array_len, `${path}.array_len`, 1);
  requireInteger(value?.element_size_bytes, `${path}.element_size_bytes`, 1);
  requireInteger(value?.slot_offset, `${path}.slot_offset`, 0);
  requireNullableInteger(value?.byte_offset, `${path}.byte_offset`, 0);
  requireNullableInteger(value?.state_byte_offset, `${path}.state_byte_offset`, 0);
  requireInteger(value?.byte_size, `${path}.byte_size`, 1);
  requireNullableStringArray(value?.default_reprs, `${path}.default_reprs`);
  requireNullableString(value?.range_min_repr, `${path}.range_min_repr`);
  requireNullableString(value?.range_max_repr, `${path}.range_max_repr`);
  validateParamControlMetadata(value?.param_control, `${path}.param_control`);
  requireScalarLayout(value, path);
  if (value.default_reprs !== null && value.default_reprs.length !== value.array_len) {
    throw new OndaArtifactError(`${path}.default_reprs must contain one value per element`);
  }
  if ((value.range_min_repr === null) !== (value.range_max_repr === null)) {
    throw new OndaArtifactError(`${path} range bounds must either both be present or both be null`);
  }
  if (value.param_control !== null && value.range_min_repr === null) {
    throw new OndaArtifactError(`${path}.param_control requires range bounds`);
  }
  if (
    value.param_control !== null
    && (value.array_len !== 1 || value.type_repr !== value.scalar)
  ) {
    throw new OndaArtifactError(`${path}.param_control requires a scalar parameter`);
  }
  if (value.param_control !== null && !isParameter) {
    throw new OndaArtifactError(`${path}.param_control is only valid for parameters`);
  }
  if (
    isParameter
    && value.range_min_repr !== null
    && value.type_repr === value.scalar
    && value.scalar !== "bool"
    && value.param_control === null
  ) {
    throw new OndaArtifactError(`${path}.param_control is required for a scalar numeric range`);
  }
  if (value.param_control !== null) {
    try {
      PARAM_CONTROL.validateParamControlDomain(value, true);
    } catch (error) {
      throw new OndaArtifactError(`${path}.param_control is invalid: ${error.message}`);
    }
  }
}

function validateParamControlMetadata(value, path) {
  if (value === undefined) {
    throw new OndaArtifactError(`${path} must be present`);
  }
  if (value === null) return;
  if (typeof value !== "object" || Array.isArray(value)) {
    throw new OndaArtifactError(`${path} must be an object or null`);
  }
  if (!PARAM_CONTROL.scales.includes(value.scale)) {
    throw new OndaArtifactError(`${path}.scale must be 'linear' or 'log'`);
  }
  requireNullableFinite(value.curve, `${path}.curve`);
  requireNullableString(value.unit, `${path}.unit`);
  requireNullableString(value.step_repr, `${path}.step_repr`);
  requireNullableInteger(value.step_count, `${path}.step_count`, 1);
  if (value.step_count !== null && value.step_count > 0xffff_ffff) {
    throw new OndaArtifactError(`${path}.step_count must fit u32`);
  }
  if ((value.step_repr === null) !== (value.step_count === null)) {
    throw new OndaArtifactError(
      `${path}.step_repr and step_count must either both be present or both be null`,
    );
  }
  if (value.scale === "log" && value.step_repr !== null) {
    throw new OndaArtifactError(`${path} cannot combine logarithmic scale with step`);
  }
  if (value.scale === "log" && value.curve !== null) {
    throw new OndaArtifactError(`${path} cannot combine logarithmic scale with curve`);
  }
}

function validateBufferMetadata(value, path) {
  requireString(value?.name, `${path}.name`);
  requireString(value?.type_repr, `${path}.type_repr`);
  requireScalar(value?.scalar, `${path}.scalar`);
  requireInteger(value?.element_size_bytes, `${path}.element_size_bytes`, 1);
  if (!["mono", "static", "dynamic"].includes(value?.channels)) {
    throw new OndaArtifactError(`${path}.channels has an unsupported value`);
  }
  requireNullableInteger(value?.static_channels, `${path}.static_channels`, 1);
  requireScalarElementSize(value, path);
  if (
    (value.channels === "mono" && value.static_channels !== 1)
    || (value.channels === "static" && value.static_channels === null)
    || (value.channels === "dynamic" && value.static_channels !== null)
  ) {
    throw new OndaArtifactError(`${path}.static_channels is inconsistent with channels`);
  }
  if (!["read_only", "read_write"].includes(value?.access)) {
    throw new OndaArtifactError(`${path}.access has an unsupported value`);
  }
  requireBoolean(value?.may_write, `${path}.may_write`);
  if (value.may_write && value.access !== "read_write") {
    throw new OndaArtifactError(`${path}.may_write requires read_write access`);
  }
}

function validateEventMetadata(value, path) {
  requireString(value?.name, `${path}.name`);
  requireString(value?.export, `${path}.export`);
  validatePayloadMetadata(value, path, true);
}

function validatePayloadMetadata(value, path, supportsDefaults) {
  requireNullableInteger(value?.payload_size_bytes, `${path}.payload_size_bytes`, 0);
  requireInteger(value?.payload_min_size_bytes, `${path}.payload_min_size_bytes`, 0);
  requireBoolean(value?.has_dynamic_payload, `${path}.has_dynamic_payload`);
  if (!Array.isArray(value?.params)) {
    throw new OndaArtifactError(`${path}.params must be an array`);
  }
  let minimumSize = 0;
  let hasDynamicParam = false;
  value.params.forEach((param, index) => {
    const paramPath = `${path}.params[${index}]`;
    requireString(param?.name, `${paramPath}.name`);
    requireString(param?.type_repr, `${paramPath}.type_repr`);
    requireScalar(param?.scalar, `${paramPath}.scalar`);
    requireInteger(param?.array_len, `${paramPath}.array_len`, 0);
    requireBoolean(param?.is_array, `${paramPath}.is_array`);
    requireBoolean(param?.is_slice, `${paramPath}.is_slice`);
    requireNullableInteger(param?.byte_offset, `${paramPath}.byte_offset`, 0);
    requireNullableInteger(param?.byte_size, `${paramPath}.byte_size`, 0);
    requireInteger(param?.element_size_bytes, `${paramPath}.element_size_bytes`, 1);
    if (supportsDefaults) {
      requireBoolean(param?.has_default, `${paramPath}.has_default`);
      requireNullableStringArray(param?.default_reprs, `${paramPath}.default_reprs`);
    }
    requireScalarElementSize(param, paramPath);
    if (hasDynamicParam ? param.byte_offset !== null : param.byte_offset !== minimumSize) {
      throw new OndaArtifactError(`${paramPath}.byte_offset is inconsistent with event layout`);
    }
    if (param.is_slice) {
      if (
        param.is_array
        || param.type_repr !== `${param.scalar}[]`
        || param.array_len !== 0
        || param.byte_size !== null
        || (supportsDefaults && param.has_default)
      ) {
        throw new OndaArtifactError(`${paramPath} has an invalid slice descriptor`);
      }
      minimumSize += 4;
      hasDynamicParam = true;
    } else {
      const expectedType = param.is_array
        ? `${param.scalar}[${param.array_len}]`
        : param.scalar;
      if (
        param.array_len < 1
        || (!param.is_array && param.array_len !== 1)
        || param.type_repr !== expectedType
        || param.byte_size !== param.element_size_bytes * param.array_len
      ) {
        throw new OndaArtifactError(`${paramPath} has an invalid fixed-size descriptor`);
      }
      minimumSize += param.byte_size;
    }
    if (supportsDefaults) {
      if (param.has_default !== (param.default_reprs !== null)) {
        throw new OndaArtifactError(`${paramPath}.has_default must reflect default_reprs`);
      }
      if (param.default_reprs !== null && param.default_reprs.length !== param.array_len) {
        throw new OndaArtifactError(`${paramPath}.default_reprs must contain one value per element`);
      }
    }
  });
  if (value.payload_min_size_bytes !== minimumSize) {
    throw new OndaArtifactError(`${path}.payload_min_size_bytes is inconsistent with params`);
  }
  if (value.has_dynamic_payload !== hasDynamicParam) {
    throw new OndaArtifactError(`${path}.has_dynamic_payload is inconsistent with params`);
  }
  if (
    hasDynamicParam
      ? value.payload_size_bytes !== null
      : value.payload_size_bytes !== minimumSize
  ) {
    throw new OndaArtifactError(`${path}.payload_size_bytes is inconsistent with params`);
  }
}

function validateStateMetadata(value, path) {
  requireString(value?.name, `${path}.name`);
  if (value?.authored !== undefined) {
    requireBoolean(value.authored, `${path}.authored`);
  }
  requireString(value?.type_repr, `${path}.type_repr`);
  requireScalar(value?.scalar, `${path}.scalar`);
  requireInteger(value?.array_len, `${path}.array_len`, 1);
  requireInteger(value?.element_size_bytes, `${path}.element_size_bytes`, 1);
  requireInteger(
    value?.packed_snapshot_byte_offset,
    `${path}.packed_snapshot_byte_offset`,
    0,
  );
  requireInteger(
    value?.physical_state_byte_offset,
    `${path}.physical_state_byte_offset`,
    0,
  );
  requireInteger(value?.byte_size, `${path}.byte_size`, 1);
  requireScalarLayout(value, path);
  const range = value?.integer_range;
  if (range === undefined || range === null) return;
  if (value.scalar !== "i32" && value.scalar !== "i64") {
    throw new OndaArtifactError(`${path}.integer_range requires i32 or i64 state`);
  }
  if (value.array_len !== 1) {
    throw new OndaArtifactError(`${path}.integer_range requires scalar state`);
  }
  if (range.mode !== "clamp" && range.mode !== "wrap") {
    throw new OndaArtifactError(`${path}.integer_range.mode must be 'clamp' or 'wrap'`);
  }
  const parseEndpoint = (endpoint, endpointPath) => {
    if (endpoint?.type !== value.scalar) {
      throw new OndaArtifactError(`${endpointPath}.type must match state scalar '${value.scalar}'`);
    }
    requireString(endpoint?.value, `${endpointPath}.value`);
    if (!/^-?(0|[1-9][0-9]*)$/.test(endpoint.value)) {
      throw new OndaArtifactError(`${endpointPath}.value must be a canonical decimal integer`);
    }
    const parsed = BigInt(endpoint.value);
    const minimum = value.scalar === "i32" ? -(1n << 31n) : -(1n << 63n);
    const maximum = value.scalar === "i32" ? (1n << 31n) - 1n : (1n << 63n) - 1n;
    if (parsed < minimum || parsed > maximum) {
      throw new OndaArtifactError(`${endpointPath}.value exceeds ${value.scalar}`);
    }
    return parsed;
  };
  const minimum = parseEndpoint(range.min, `${path}.integer_range.min`);
  const maximum = parseEndpoint(range.max, `${path}.integer_range.max`);
  if (minimum > maximum) {
    throw new OndaArtifactError(`${path}.integer_range minimum exceeds maximum`);
  }
}

function validateRuntimeLayouts(descriptor) {
  const { metadata, runtime } = descriptor;
  for (const field of ["inputs", "outputs", "control_outputs", "params"]) {
    validateSequentialSlots(metadata[field], `metadata.${field}`);
  }

  for (const field of ["inputs", "outputs"]) {
    validateLogicalByteOffsets(metadata[field], `metadata.${field}`);
    metadata[field].forEach((entry, index) => {
      if (entry.state_byte_offset !== null) {
        throw new OndaArtifactError(
          `metadata.${field}[${index}].state_byte_offset must be null`,
        );
      }
    });
  }

  const paramRegions = metadata.params.map((entry, index) => {
    if (entry.byte_offset === null) {
      throw new OndaArtifactError(`metadata.params[${index}].byte_offset must not be null`);
    }
    if (entry.state_byte_offset !== null) {
      throw new OndaArtifactError(
        `metadata.params[${index}].state_byte_offset must be null`,
      );
    }
    return storageRegion(
      entry.byte_offset,
      entry.byte_size,
      `metadata.params[${index}]`,
    );
  });
  validateNonOverlappingRegions(
    paramRegions,
    runtime.param_size_bytes,
    "runtime parameter storage",
  );

  validateLogicalByteOffsets(
    metadata.control_outputs,
    "metadata.control_outputs",
  );
  const physicalStateRegions = metadata.states.map((entry, index) =>
    storageRegion(
      entry.physical_state_byte_offset,
      entry.byte_size,
      `metadata.states[${index}]`,
    )
  );
  for (const [index, entry] of metadata.control_outputs.entries()) {
    if (entry.state_byte_offset === null) {
      throw new OndaArtifactError(
        `metadata.control_outputs[${index}].state_byte_offset must not be null`,
      );
    }
    physicalStateRegions.push(storageRegion(
      entry.state_byte_offset,
      entry.byte_size,
      `metadata.control_outputs[${index}]`,
    ));
  }
  validateNonOverlappingRegions(
    physicalStateRegions,
    runtime.state_size_bytes,
    "runtime physical-state storage",
  );

  let packedOffset = 0;
  for (const [index, entry] of metadata.states.entries()) {
    if (entry.packed_snapshot_byte_offset !== packedOffset) {
      throw new OndaArtifactError(
        `metadata.states[${index}].packed_snapshot_byte_offset must be ${packedOffset}`,
      );
    }
    packedOffset = checkedLayoutEnd(
      packedOffset,
      entry.byte_size,
      `metadata.states[${index}] packed snapshot`,
    );
  }
  if (packedOffset !== runtime.snapshot_size_bytes) {
    throw new OndaArtifactError(
      `metadata.states describe ${packedOffset} snapshot bytes; runtime.snapshot_size_bytes is ${runtime.snapshot_size_bytes}`,
    );
  }
}

function validateSequentialSlots(entries, path) {
  let expected = 0;
  for (const [index, entry] of entries.entries()) {
    if (entry.slot_offset !== expected) {
      throw new OndaArtifactError(`${path}[${index}].slot_offset must be ${expected}`);
    }
    expected = checkedLayoutEnd(expected, entry.array_len, `${path}[${index}] slots`);
  }
}

function validateLogicalByteOffsets(entries, path) {
  let expected = 0;
  for (const [index, entry] of entries.entries()) {
    if (entry.byte_offset !== null && entry.byte_offset !== expected) {
      throw new OndaArtifactError(`${path}[${index}].byte_offset must be ${expected} or null`);
    }
    expected = checkedLayoutEnd(expected, entry.byte_size, `${path}[${index}] bytes`);
  }
}

function storageRegion(offset, size, path) {
  return {
    offset,
    end: checkedLayoutEnd(offset, size, path),
    path,
  };
}

function validateNonOverlappingRegions(regions, regionSize, description) {
  const sorted = [...regions].sort((lhs, rhs) => lhs.offset - rhs.offset);
  let previous = null;
  for (const region of sorted) {
    if (region.end > regionSize) {
      throw new OndaArtifactError(
        `${region.path} exceeds ${description} size ${regionSize}`,
      );
    }
    if (previous !== null && region.offset < previous.end) {
      throw new OndaArtifactError(
        `${region.path} overlaps ${previous.path} in ${description}`,
      );
    }
    previous = region;
  }
}

function checkedLayoutEnd(offset, size, path) {
  const end = offset + size;
  if (!Number.isSafeInteger(end)) {
    throw new OndaArtifactError(`${path} byte extent exceeds the safe integer range`);
  }
  return end;
}

export function validateProcessorArtifact(
  artifact,
  { inspectModule = true } = {},
) {
  if (!artifact || typeof artifact !== "object") {
    throw new OndaArtifactError("processor artifact must be an object");
  }
  const wasm = asUint8Array(artifact.wasm);
  const metadata = validateProcessorMetadata(
    artifact.metadata,
    "webassembly_module",
  );
  if (!WebAssembly.validate(wasm)) {
    throw new OndaArtifactError("processor artifact is not valid WebAssembly");
  }
  if (!inspectModule) return { wasm, metadata };
  const module = new WebAssembly.Module(wasm);
  validateProcessorModule(module, metadata);
  return { wasm, metadata };
}

export function validateProcessorModule(module, metadataInput) {
  let imports;
  let moduleExports;
  try {
    imports = WebAssembly.Module.imports(module);
    moduleExports = WebAssembly.Module.exports(module);
  } catch {
    throw new OndaArtifactError("processor module must be a WebAssembly.Module");
  }
  const metadata = validateProcessorMetadata(
    metadataInput,
    "webassembly_module",
  );
  if (imports.length !== 0) {
    throw new OndaArtifactError(
      `processor artifact has unexpected imports: ${imports.map((entry) => `${entry.module}.${entry.name}`).join(", ")}`,
    );
  }
  const exports = new Map(
    moduleExports.map((entry) => [entry.name, entry.kind]),
  );
  for (const name of metadata.integration.required_symbols) {
    if (!exports.has(name)) {
      throw new OndaArtifactError(
        `processor artifact is missing required export '${name}'`,
      );
    }
  }
  const expectedKinds = new Map([
    [metadata.exports.memory, "memory"],
    [metadata.exports.heap_base, "global"],
    [metadata.exports.init, "function"],
    [metadata.exports.process, "function"],
    ...metadata.exports.events.map((name) => [name, "function"]),
  ]);
  for (const [name, expectedKind] of expectedKinds) {
    const actualKind = exports.get(name);
    if (actualKind !== expectedKind) {
      throw new OndaArtifactError(
        `processor export '${name}' must be a ${expectedKind}, got ${String(actualKind)}`,
      );
    }
  }
  return { module, metadata };
}

export function serializeProcessorMetadata(metadata, space = 2) {
  validateProcessorMetadata(metadata);
  return `${JSON.stringify(metadata, null, space)}\n`;
}

export function parseProcessorMetadata(input, expectedKind = null) {
  let metadata;
  try {
    metadata = typeof input === "string" ? JSON.parse(input) : input;
  } catch (error) {
    throw new OndaArtifactError(`invalid processor metadata JSON: ${error.message}`);
  }
  return validateProcessorMetadata(metadata, expectedKind);
}

export async function createProcessorArtifactFiles(
  artifact,
  { baseName = "processor" } = {},
) {
  const { wasm, metadata } = validateProcessorArtifact(artifact);
  const digest = await sha256Hex(wasm);
  const serializedMetadata = {
    ...metadata,
    integrity: {
      algorithm: "sha256",
      wasm: digest,
    },
  };
  const safeBaseName = sanitizeBaseName(baseName);
  return {
    wasm: {
      name: `${safeBaseName}.wasm`,
      mediaType: "application/wasm",
      bytes: wasm.slice(),
    },
    metadata: {
      name: `${safeBaseName}.onda.json`,
      mediaType: "application/json",
      text: serializeProcessorMetadata(serializedMetadata),
      value: serializedMetadata,
    },
  };
}

export async function loadProcessorArtifactFiles(
  wasmInput,
  metadataInput,
) {
  const metadata = parseProcessorMetadata(metadataInput, "webassembly_module");
  const artifact = validateProcessorArtifact({ wasm: wasmInput, metadata });
  const expectedDigest = metadata.integrity?.wasm;
  if (
    metadata.integrity?.algorithm !== "sha256"
    || typeof expectedDigest !== "string"
    || !/^[0-9a-f]{64}$/.test(expectedDigest)
  ) {
    throw new OndaArtifactError(
      "processor descriptor is missing a valid SHA-256 Wasm integrity digest",
    );
  }
  const actualDigest = await sha256Hex(artifact.wasm);
  if (actualDigest !== expectedDigest) {
    throw new OndaArtifactError(
      `processor Wasm integrity mismatch; expected ${expectedDigest}, got ${actualDigest}`,
    );
  }
  return artifact;
}

export function writeDelegateBatch(
  memory,
  batchAddress,
  storageAddress,
  capacityBytes,
) {
  const view = writableDataView(memory);
  requireMemoryRange(view, batchAddress, DELEGATE_BATCH_SIZE_BYTES, "delegate batch");
  requireInteger(storageAddress, "storageAddress", 0);
  requireInteger(capacityBytes, "capacityBytes", 0);
  if (storageAddress > 0xffff_ffff || capacityBytes > 0xffff_ffff) {
    throw new OndaArtifactError("delegate batch addresses and sizes must fit u32");
  }
  view.setUint32(batchAddress, storageAddress, true);
  view.setUint32(batchAddress + 4, capacityBytes, true);
  view.setUint32(batchAddress + 8, 0, true);
  view.setUint32(batchAddress + 12, 0, true);
  view.setUint32(batchAddress + 16, 0, true);
}

export function readDelegateBatch(memory, batchAddress) {
  const view = writableDataView(memory);
  requireMemoryRange(view, batchAddress, DELEGATE_BATCH_SIZE_BYTES, "delegate batch");
  return {
    storageAddress: view.getUint32(batchAddress, true),
    capacityBytes: view.getUint32(batchAddress + 4, true),
    usedBytes: view.getUint32(batchAddress + 8, true),
    recordCount: view.getUint32(batchAddress + 12, true),
    overflowCount: view.getUint32(batchAddress + 16, true),
  };
}

export function writePrintBatch(memory, batchAddress, storageAddress, capacityBytes) {
  const view = writableDataView(memory);
  requireMemoryRange(view, batchAddress, PRINT_BATCH_SIZE_BYTES, "print batch");
  requireInteger(storageAddress, "storageAddress", 0);
  requireInteger(capacityBytes, "capacityBytes", 0);
  if (storageAddress > 0xffff_ffff || capacityBytes > 0xffff_ffff) {
    throw new OndaArtifactError("print batch addresses and sizes must fit u32");
  }
  view.setUint32(batchAddress, storageAddress, true);
  view.setUint32(batchAddress + 4, capacityBytes, true);
  view.setUint32(batchAddress + 8, 0, true);
  view.setUint32(batchAddress + 12, 0, true);
  view.setUint32(batchAddress + 16, 0, true);
}

export function readPrintBatch(memory, batchAddress) {
  const view = writableDataView(memory);
  requireMemoryRange(view, batchAddress, PRINT_BATCH_SIZE_BYTES, "print batch");
  return {
    storageAddress: view.getUint32(batchAddress, true),
    capacityBytes: view.getUint32(batchAddress + 4, true),
    usedBytes: view.getUint32(batchAddress + 8, true),
    recordCount: view.getUint32(batchAddress + 12, true),
    overflowCount: view.getUint32(batchAddress + 16, true),
  };
}

export function writeExecutionOutput(
  memory,
  outputAddress,
  delegateBatchAddress = 0,
  printBatchAddress = 0,
) {
  const view = writableDataView(memory);
  requireMemoryRange(view, outputAddress, EXECUTION_OUTPUT_SIZE_BYTES, "execution output");
  requireInteger(delegateBatchAddress, "delegateBatchAddress", 0);
  requireInteger(printBatchAddress, "printBatchAddress", 0);
  if (delegateBatchAddress > 0xffff_ffff || printBatchAddress > 0xffff_ffff) {
    throw new OndaArtifactError("execution output addresses must fit u32");
  }
  view.setUint32(outputAddress, delegateBatchAddress, true);
  view.setUint32(outputAddress + 4, printBatchAddress, true);
  view.setUint32(outputAddress + 8, 0, true);
}

export function resetExecutionOutput(memory, outputAddress) {
  const view = writableDataView(memory);
  requireMemoryRange(view, outputAddress, EXECUTION_OUTPUT_SIZE_BYTES, "execution output");
  const delegateBatchAddress = view.getUint32(outputAddress, true);
  const printBatchAddress = view.getUint32(outputAddress + 4, true);
  for (const [batchAddress, sizeBytes, label] of [
    [delegateBatchAddress, DELEGATE_BATCH_SIZE_BYTES, "delegate batch"],
    [printBatchAddress, PRINT_BATCH_SIZE_BYTES, "print batch"],
  ]) {
    if (batchAddress !== 0) {
      requireMemoryRange(view, batchAddress, sizeBytes, label);
    }
  }
  for (const batchAddress of [delegateBatchAddress, printBatchAddress]) {
    if (batchAddress === 0) continue;
    view.setUint32(batchAddress + 8, 0, true);
    view.setUint32(batchAddress + 12, 0, true);
    view.setUint32(batchAddress + 16, 0, true);
  }
  view.setUint32(outputAddress + 8, 0, true);
}

export function decodePrintRecords(
  storage,
  usedBytes,
  logSites,
  byteOrder = "little_endian",
) {
  const bytes = asUint8Array(storage);
  requireInteger(usedBytes, "usedBytes", 0);
  if (usedBytes > bytes.byteLength) {
    throw new OndaArtifactError("print batch usedBytes exceeds storage");
  }
  if (!Array.isArray(logSites)) {
    throw new OndaArtifactError("log-site metadata must be an array");
  }
  const littleEndian = byteOrder === "little_endian";
  if (!littleEndian && byteOrder !== "big_endian") {
    throw new OndaArtifactError("byteOrder must be little_endian or big_endian");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, usedBytes);
  const records = [];
  let cursor = 0;
  while (cursor < usedBytes) {
    if (usedBytes - cursor < PRINT_RECORD_HEADER_SIZE_BYTES) {
      throw new OndaArtifactError("print storage ends in a partial record header");
    }
    const siteIndex = view.getUint32(cursor, littleEndian);
    const payloadByteLength = view.getUint32(cursor + 4, littleEndian);
    const sequence = view.getUint32(cursor + 8, littleEndian);
    const payloadOffset = cursor + PRINT_RECORD_HEADER_SIZE_BYTES;
    const end = payloadOffset + payloadByteLength;
    if (!Number.isSafeInteger(end) || end > usedBytes) {
      throw new OndaArtifactError("print storage ends in a partial payload");
    }
    const site = logSites[siteIndex];
    if (!site) {
      throw new OndaArtifactError(`print record references unknown log site ${siteIndex}`);
    }
    if (payloadByteLength !== site.payload_size_bytes) {
      throw new OndaArtifactError(
        `print record ${siteIndex} has ${payloadByteLength} payload bytes; expected ${site.payload_size_bytes}`,
      );
    }
    const values = [];
    let valueOffset = payloadOffset;
    for (const scalar of site.argument_types) {
      switch (scalar) {
        case "f32":
          values.push({ type: scalar, value: view.getFloat32(valueOffset, littleEndian) });
          valueOffset += 4;
          break;
        case "f64":
          values.push({ type: scalar, value: view.getFloat64(valueOffset, littleEndian) });
          valueOffset += 8;
          break;
        case "i32":
          values.push({ type: scalar, value: view.getInt32(valueOffset, littleEndian) });
          valueOffset += 4;
          break;
        case "i64":
          values.push({ type: scalar, value: view.getBigInt64(valueOffset, littleEndian) });
          valueOffset += 8;
          break;
        case "bool": {
          const value = view.getUint8(valueOffset);
          if (value > 1) throw new OndaArtifactError("print payload contains an invalid bool");
          values.push({ type: scalar, value: value !== 0 });
          valueOffset += 1;
          break;
        }
        default:
          throw new OndaArtifactError(`unsupported print scalar '${String(scalar)}'`);
      }
    }
    if (valueOffset !== end) {
      throw new OndaArtifactError(`print record ${siteIndex} payload has trailing bytes`);
    }
    records.push({
      siteIndex,
      sequence,
      label: site.label,
      source: site.source,
      lexicalOwner: site.lexical_owner,
      declaration: site.declaration,
      values,
    });
    cursor = end;
  }
  return records;
}

function shortestF32(value) {
  for (let precision = 1; precision <= 9; precision += 1) {
    const candidate = value.toPrecision(precision);
    if (Object.is(Math.fround(Number(candidate)), value)) return candidate;
  }
  return value.toPrecision(9);
}

function canonicalFloat(value, width) {
  if (Number.isNaN(value)) return "NaN";
  if (value === Infinity) return "inf";
  if (value === -Infinity) return "-inf";
  if (Object.is(value, -0)) return "-0.0";
  if (value === 0) return "0.0";
  const negative = value < 0;
  const shortest = width === 32 ? shortestF32(Math.fround(Math.abs(value))) : Math.abs(value).toString();
  const [mantissa, exponentText] = shortest.toLowerCase().split("e");
  let digits = mantissa.replace(".", "");
  const leading = digits.match(/^0*/u)[0].length;
  digits = digits.slice(leading);
  const decimalPosition = (mantissa.indexOf(".") < 0 ? mantissa.length : mantissa.indexOf("."))
    + (exponentText === undefined ? 0 : Number(exponentText)) - leading;
  const magnitude = Math.abs(value);
  let rendered;
  if (magnitude >= 1e-6 && magnitude < 1e21) {
    if (decimalPosition <= 0) {
      rendered = `0.${"0".repeat(-decimalPosition)}${digits}`;
    } else if (decimalPosition >= digits.length) {
      rendered = `${digits}${"0".repeat(decimalPosition - digits.length)}.0`;
    } else {
      rendered = `${digits.slice(0, decimalPosition)}.${digits.slice(decimalPosition)}`;
    }
  } else {
    digits = digits.replace(/0+$/u, "");
    const coefficient = digits.length === 1 ? digits : `${digits[0]}.${digits.slice(1)}`;
    rendered = `${coefficient}e${decimalPosition - 1}`;
  }
  return negative ? `-${rendered}` : rendered;
}

function canonicalPrintValue(entry) {
  switch (entry.type) {
    case "f32": return canonicalFloat(entry.value, 32);
    case "f64": return canonicalFloat(entry.value, 64);
    case "i32": return String(entry.value);
    case "i64": return entry.value.toString();
    case "bool": return entry.value ? "true" : "false";
    default: throw new OndaArtifactError(`unsupported print scalar '${String(entry.type)}'`);
  }
}

function escapedPrintLabel(label) {
  let escaped = "";
  for (const character of label) {
    switch (character) {
      case "\0": escaped += "\\0"; break;
      case "\\": escaped += "\\\\"; break;
      case "\n": escaped += "\\n"; break;
      case "\r": escaped += "\\r"; break;
      case "\t": escaped += "\\t"; break;
      default: {
        const codePoint = character.codePointAt(0);
        const mustEscape = codePoint <= 0x1f
          || (codePoint >= 0x7f && codePoint <= 0x9f)
          || codePoint === 0x2028
          || codePoint === 0x2029;
        escaped += mustEscape ? `\\u{${codePoint.toString(16)}}` : character;
      }
    }
  }
  return escaped;
}

export function formatPrintBatch(memory, printBatchAddress, metadata) {
  const batch = readPrintBatch(memory, printBatchAddress);
  const bytes = asUint8Array(memory.buffer ?? memory);
  if (batch.usedBytes > batch.capacityBytes) {
    throw new OndaArtifactError("print batch usedBytes exceeds capacityBytes");
  }
  if (batch.storageAddress + batch.usedBytes > bytes.byteLength) {
    throw new OndaArtifactError("print batch storage exceeds WebAssembly memory");
  }
  const formatted = formatPrintRecords(
    bytes.subarray(batch.storageAddress, batch.storageAddress + batch.usedBytes),
    batch.usedBytes,
    metadata,
    batch.overflowCount,
  );
  if (formatted.entries.length !== batch.recordCount) {
    throw new OndaArtifactError("print batch recordCount does not match packed storage");
  }
  return formatted;
}

export function formatPrintRecords(storage, usedBytes, metadata, overflowCount = 0) {
  requireInteger(overflowCount, "overflowCount", 0);
  const records = decodePrintRecords(
    storage,
    usedBytes,
    metadata.metadata?.log_sites ?? metadata.log_sites,
    metadata.target?.byte_order ?? "little_endian",
  );
  let text = "";
  for (const record of records) {
    const values = record.values.map(canonicalPrintValue).join(" ");
    if (record.label !== null) {
      text += escapedPrintLabel(record.label);
      if (values.length > 0) text += `: ${values}`;
    } else {
      text += values;
    }
    text += "\n";
  }
  return { text, entries: records, overflowCount };
}

export function decodeDelegateRecords(
  storage,
  usedBytes,
  delegates,
  byteOrder = "little_endian",
) {
  const bytes = asUint8Array(storage);
  requireInteger(usedBytes, "usedBytes", 0);
  if (usedBytes > bytes.byteLength) {
    throw new OndaArtifactError("delegate batch usedBytes exceeds storage");
  }
  if (!Array.isArray(delegates)) {
    throw new OndaArtifactError("delegate metadata must be an array");
  }
  const littleEndian = byteOrder === "little_endian";
  if (!littleEndian && byteOrder !== "big_endian") {
    throw new OndaArtifactError("byteOrder must be little_endian or big_endian");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, usedBytes);
  const records = [];
  let cursor = 0;
  while (cursor < usedBytes) {
    if (usedBytes - cursor < DELEGATE_RECORD_HEADER_SIZE_BYTES) {
      throw new OndaArtifactError("delegate storage ends in a partial record header");
    }
    const delegateIndex = view.getUint32(cursor, littleEndian);
    const payloadByteLength = view.getUint32(cursor + 4, littleEndian);
    const sequence = view.getUint32(cursor + 8, littleEndian);
    const payloadOffset = cursor + DELEGATE_RECORD_HEADER_SIZE_BYTES;
    const end = payloadOffset + payloadByteLength;
    if (!Number.isSafeInteger(end) || end > usedBytes) {
      throw new OndaArtifactError("delegate storage ends in a partial payload");
    }
    const delegate = delegates[delegateIndex];
    if (!delegate) {
      throw new OndaArtifactError(`delegate record references unknown delegate ${delegateIndex}`);
    }
    records.push({
      delegateIndex,
      sequence,
      name: delegate.name,
      payloadByteLength,
      payload: bytes.slice(payloadOffset, end),
      values: decodeDelegatePayload(
        view,
        payloadOffset,
        payloadByteLength,
        delegate,
        littleEndian,
      ),
    });
    cursor = end;
  }
  return records;
}

function decodeDelegatePayload(view, start, size, delegate, littleEndian) {
  let cursor = start;
  const end = start + size;
  const values = {};
  for (const param of delegate.params) {
    let count = param.array_len;
    if (param.is_slice) {
      if (cursor + 4 > end) {
        throw new OndaArtifactError(`delegate '${delegate.name}' has a truncated slice length`);
      }
      count = view.getInt32(cursor, littleEndian);
      cursor += 4;
      if (count < 0) {
        throw new OndaArtifactError(`delegate '${delegate.name}' has a negative slice length`);
      }
    }
    const byteLength = count * param.element_size_bytes;
    if (!Number.isSafeInteger(byteLength) || cursor + byteLength > end) {
      throw new OndaArtifactError(`delegate '${delegate.name}' has a truncated '${param.name}' payload`);
    }
    const entries = [];
    for (let index = 0; index < count; index += 1) {
      entries.push(readPayloadScalar(view, cursor, param.scalar, littleEndian));
      cursor += param.element_size_bytes;
    }
    values[param.name] = param.is_array || param.is_slice ? entries : entries[0];
  }
  if (cursor !== end) {
    throw new OndaArtifactError(`delegate '${delegate.name}' payload has trailing bytes`);
  }
  return values;
}

function readPayloadScalar(view, address, scalar, littleEndian) {
  switch (scalar) {
    case "bool": return view.getUint8(address) !== 0;
    case "i32": return view.getInt32(address, littleEndian);
    case "i64": return view.getBigInt64(address, littleEndian);
    case "f32": return view.getFloat32(address, littleEndian);
    case "f64": return view.getFloat64(address, littleEndian);
    default: throw new OndaArtifactError(`unsupported delegate scalar '${String(scalar)}'`);
  }
}

function writableDataView(memory) {
  if (memory instanceof DataView) return memory;
  if (memory instanceof WebAssembly.Memory) return new DataView(memory.buffer);
  if (memory instanceof ArrayBuffer) return new DataView(memory);
  if (ArrayBuffer.isView(memory)) {
    return new DataView(memory.buffer, memory.byteOffset, memory.byteLength);
  }
  throw new OndaArtifactError("delegate batch memory must be WebAssembly memory or a buffer view");
}

function requireMemoryRange(view, address, size, label) {
  requireInteger(address, `${label} address`, 0);
  if (address + size > view.byteLength) {
    throw new OndaArtifactError(`${label} exceeds memory`);
  }
}

function asUint8Array(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new OndaArtifactError("processor artifact wasm must be bytes");
}

function requireString(value, path, allowEmpty = false) {
  if (typeof value !== "string" || (!allowEmpty && value.length === 0)) {
    throw new OndaArtifactError(
      `${path} must be ${allowEmpty ? "a string" : "a non-empty string"}`,
    );
  }
}

function requireLiteral(value, path, expected) {
  if (value !== expected) {
    throw new OndaArtifactError(
      `${path} must be '${expected}' for processor ABI version ${PROCESSOR_ABI_VERSION}`,
    );
  }
}

function requireBoolean(value, path) {
  if (typeof value !== "boolean") {
    throw new OndaArtifactError(`${path} must be a boolean`);
  }
}

function requireNullableInteger(value, path, minimum) {
  if (value === undefined) {
    throw new OndaArtifactError(`${path} must be present`);
  }
  if (value !== null) requireInteger(value, path, minimum);
}

function requireNullableFinite(value, path) {
  if (value === undefined) {
    throw new OndaArtifactError(`${path} must be present`);
  }
  if (value !== null && !Number.isFinite(value)) {
    throw new OndaArtifactError(`${path} must be a finite number or null`);
  }
}

function requireNullableString(value, path) {
  if (value === undefined) {
    throw new OndaArtifactError(`${path} must be present`);
  }
  if (value !== null) requireString(value, path, true);
}

function requireStringArray(value, path) {
  if (!Array.isArray(value)) {
    throw new OndaArtifactError(`${path} must be an array`);
  }
  for (const entry of value) requireString(entry, `${path}[]`);
}

function requireNullableStringArray(value, path) {
  if (value === undefined) {
    throw new OndaArtifactError(`${path} must be present`);
  }
  if (value !== null) requireStringArray(value, path);
}

function requireScalarElementSize(value, path) {
  const expected = value.scalar === "bool" ? 1
    : value.scalar === "f32" || value.scalar === "i32" ? 4
    : 8;
  if (value.element_size_bytes !== expected) {
    throw new OndaArtifactError(`${path}.element_size_bytes does not match scalar`);
  }
}

function requireScalarLayout(value, path) {
  requireScalarElementSize(value, path);
  if (value.byte_size !== value.element_size_bytes * value.array_len) {
    throw new OndaArtifactError(`${path}.byte_size does not match its scalar array shape`);
  }
}

function requireScalar(value, path) {
  if (!["f32", "f64", "i32", "i64", "bool"].includes(value)) {
    throw new OndaArtifactError(`${path} must name a MIR scalar type`);
  }
}

function requireInteger(value, path, minimum) {
  if (!Number.isSafeInteger(value) || value < minimum) {
    throw new OndaArtifactError(
      `${path} must be a safe integer greater than or equal to ${minimum}`,
    );
  }
}

function requirePositiveFinite(value, path) {
  if (!Number.isFinite(value) || value <= 0) {
    throw new OndaArtifactError(`${path} must be a positive finite number`);
  }
}

function sanitizeBaseName(value) {
  if (typeof value !== "string") {
    throw new OndaArtifactError("artifact baseName must be a string");
  }
  const result = value.trim().replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!result || result === "." || result === "..") {
    throw new OndaArtifactError("artifact baseName has no usable characters");
  }
  return result;
}

async function sha256Hex(bytes) {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    throw new OndaArtifactError(
      "Web Crypto is required to create an integrity-checked processor artifact",
    );
  }
  const digest = new Uint8Array(await subtle.digest("SHA-256", bytes));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
