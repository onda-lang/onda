export const PROCESSOR_ARTIFACT_FORMAT = "onda-processor";
export const PROCESSOR_ARTIFACT_FORMAT_VERSION = 3;
export const PROCESSOR_ABI_VERSION = 1;

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
  requireInteger(metadata.runtime?.state_size_bytes, "runtime.state_size_bytes", 0);
  requireInteger(metadata.runtime?.state_align_bytes, "runtime.state_align_bytes", 1);
  requireInteger(metadata.runtime?.param_size_bytes, "runtime.param_size_bytes", 0);
  requireInteger(metadata.runtime?.param_align_bytes, "runtime.param_align_bytes", 1);
  requireInteger(
    metadata.runtime?.snapshot_size_bytes,
    "runtime.snapshot_size_bytes",
    0,
  );
  requireString(metadata.exports?.init, "exports.init");
  requireString(metadata.exports?.process, "exports.process");
  if (!Array.isArray(metadata.exports?.events)) {
    throw new OndaArtifactError("exports.events must be an array");
  }
  if (!Array.isArray(metadata.integration?.required_symbols)) {
    throw new OndaArtifactError("integration.required_symbols must be an array");
  }
  for (const name of metadata.integration.required_symbols) {
    requireString(name, "integration.required_symbols[]");
  }
  if (metadata.integration?.one_processor_per_artifact !== true) {
    throw new OndaArtifactError(
      "integration.one_processor_per_artifact must be true for ABI version 1",
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
  ]) {
    if (!Array.isArray(metadata.metadata[field])) {
      throw new OndaArtifactError(`metadata.${field} must be an array`);
    }
  }
  return metadata;
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
  const imports = WebAssembly.Module.imports(module);
  if (imports.length !== 0) {
    throw new OndaArtifactError(
      `processor artifact has unexpected imports: ${imports.map((entry) => `${entry.module}.${entry.name}`).join(", ")}`,
    );
  }
  const exports = new Set(
    WebAssembly.Module.exports(module).map((entry) => entry.name),
  );
  for (const name of metadata.integration.required_symbols) {
    if (!exports.has(name)) {
      throw new OndaArtifactError(
        `processor artifact is missing required export '${name}'`,
      );
    }
  }
  return { wasm, metadata };
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

function asUint8Array(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new OndaArtifactError("processor artifact wasm must be bytes");
}

function requireString(value, path) {
  if (typeof value !== "string" || value.length === 0) {
    throw new OndaArtifactError(`${path} must be a non-empty string`);
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
