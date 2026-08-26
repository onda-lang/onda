import {
  OndaArtifactError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  createProcessorArtifactFiles,
  loadProcessorArtifactFiles,
  parseProcessorMetadata,
  serializeProcessorMetadata,
  validateProcessorArtifact,
  validateProcessorMetadata,
  validateProcessorModule,
} from "@onda-lang/processor-abi";
import { SUPPORTED_MIR_SCHEMA_VERSION } from "../dist/backend/constants.js";
import { OndaBinaryenError } from "../dist/backend/errors.js";
import { defaultFrontendInput } from "#onda-frontend-loader";
import { ONDA_VERSION } from "../dist/version.js";
import { MAX_BLOCK_SIZE } from "./config.js";

export const MIR_SCHEMA_VERSION = SUPPORTED_MIR_SCHEMA_VERSION;
export { ONDA_VERSION };

export {
  OndaArtifactError,
  OndaBinaryenError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  createProcessorArtifactFiles,
  loadProcessorArtifactFiles,
  parseProcessorMetadata,
  serializeProcessorMetadata,
  validateProcessorArtifact,
  validateProcessorMetadata,
  validateProcessorModule,
};

export class OndaCompilerError extends Error {
  constructor(message, { cause } = {}) {
    super(message);
    this.name = "OndaCompilerError";
    if (cause !== undefined) this.cause = cause;
  }
}

export class OndaCompileError extends OndaCompilerError {
  constructor(
    diagnostics,
    { cause, sourceFiles = [], unresolvedSourceFiles = [] } = {},
  ) {
    const normalized = normalizeDiagnostics(diagnostics);
    const first = normalized[0];
    const message = first
      ? `${first.stage}: ${first.message}`
      : "Onda compilation failed";
    super(message, { cause });
    this.name = "OndaCompileError";
    this.diagnostics = normalized;
    this.sourceFiles = normalizeSourceFiles(sourceFiles);
    this.unresolvedSourceFiles = normalizeSourceFiles(unresolvedSourceFiles);
  }
}

let toolchainInitialization;

class OndaCompiler {
  constructor(frontend, compileTrustedMir) {
    this.frontend = frontend;
    this.compileTrustedMir = compileTrustedMir;
    this.lsp = null;
  }

  async compileSource(source, options = {}) {
    if (typeof source !== "string") {
      throw configurationError("source must be a string");
    }
    const compile = normalizeCompileOptions(options);
    let frontendCompilation;
    try {
      frontendCompilation = this.frontend.compile_to_mir_messagepack(
        source,
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    const { mir, sourceFiles, sourceGraph } = consumeFrontendCompilation(frontendCompilation);
    const artifact = compileMirTransport(
      mir,
      compile.codegen,
      this.compileTrustedMir,
      sourceFiles,
    );
    return { artifact, sourceFiles, sourceGraph };
  }

  async inspectSourceConstants(source, options = {}) {
    if (typeof source !== "string") {
      throw configurationError("source must be a string");
    }
    const compile = normalizeCompileConstInspectionOptions(options);
    let encoded;
    try {
      encoded = this.frontend.inspect_source_compile_constants(
        source,
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    return decodeCompileConstDescriptors(encoded);
  }

  async compileWorkspace(workspace, options = {}) {
    workspace = normalizeWorkspace(workspace);
    const compile = normalizeCompileOptions(options);
    let frontendCompilation;
    try {
      frontendCompilation = this.frontend.compile_source_workspace_to_mir_messagepack(
        workspace.entry,
        JSON.stringify(workspace.sources),
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    const { mir, sourceFiles, sourceGraph } = consumeFrontendCompilation(frontendCompilation);
    const artifact = compileMirTransport(
      mir,
      compile.codegen,
      this.compileTrustedMir,
      sourceFiles,
    );
    return { artifact, sourceFiles, sourceGraph };
  }

  async inspectWorkspaceConstants(workspace, options = {}) {
    workspace = normalizeWorkspace(workspace);
    const compile = normalizeCompileConstInspectionOptions(options);
    let encoded;
    try {
      encoded = this.frontend.inspect_source_workspace_compile_constants(
        workspace.entry,
        JSON.stringify(workspace.sources),
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    return decodeCompileConstDescriptors(encoded);
  }

  async compileProjectImage(imageBytes, options = {}) {
    const bytes = normalizeBytes(imageBytes, "project image");
    const compile = normalizeCompileOptions(options);
    let frontendCompilation;
    try {
      frontendCompilation = this.frontend.compile_project_image_to_mir_messagepack(
        bytes,
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    const { mir, sourceFiles, sourceGraph } = consumeFrontendCompilation(frontendCompilation);
    const artifact = compileMirTransport(
      mir,
      compile.codegen,
      this.compileTrustedMir,
      sourceFiles,
    );
    return { artifact, sourceFiles, sourceGraph };
  }

  async inspectProjectImageConstants(imageBytes, options = {}) {
    const bytes = normalizeBytes(imageBytes, "project image");
    const compile = normalizeCompileConstInspectionOptions(options);
    let encoded;
    try {
      encoded = this.frontend.inspect_project_image_compile_constants(
        bytes,
        compile.sampleRate,
        compile.blockSize,
        compile.constantsJson,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    return decodeCompileConstDescriptors(encoded);
  }

  async createProjectImage(sourceGraph, buffers = new Map()) {
    const graph = normalizeSourceGraph(sourceGraph);
    const builder = new this.frontend.WebProjectImageBuilder(JSON.stringify(graph));
    try {
      for (const [name, bytes] of normalizeBufferAssetEntries(buffers)) {
        builder.add_buffer(name, bytes);
      }
      const bytes = builder.serialize();
      return {
        bytes,
        ...normalizeProjectImageInfo(JSON.parse(this.frontend.inspect_project_image(bytes))),
      };
    } catch (cause) {
      throw new OndaCompilerError("failed to create Onda project image", { cause });
    } finally {
      builder.free();
    }
  }

  async inspectProjectImage(imageBytes) {
    try {
      return normalizeProjectImageInfo(JSON.parse(this.frontend.inspect_project_image(
        normalizeBytes(imageBytes, "project image"),
      )));
    } catch (cause) {
      throw new OndaCompilerError("failed to inspect Onda project image", { cause });
    }
  }

  async loadProjectFiles(files, projectFilePath = null) {
    const builder = new this.frontend.WebMaterializedProjectBuilder();
    try {
      if (projectFilePath !== null) {
        builder.select_project(normalizeProjectFilePath(projectFilePath));
      }
      for (const [path, bytes] of normalizeProjectFileEntries(files)) {
        builder.add_file(path, bytes);
      }
      const bytes = builder.serialize();
      return {
        bytes,
        ...normalizeProjectImageInfo(JSON.parse(this.frontend.inspect_project_image(bytes))),
      };
    } catch (cause) {
      throw new OndaCompilerError("failed to load Onda project files", { cause });
    } finally {
      builder.free();
    }
  }

  async materializeProjectImage(imageBytes, assetFileNames = new Map()) {
    let plan;
    try {
      plan = this.frontend.materialize_project_image(
        normalizeBytes(imageBytes, "project image"),
        JSON.stringify(Object.fromEntries(normalizeAssetFileNameEntries(assetFileNames))),
      );
      const files = [];
      for (let index = 0; index < plan.file_count(); index += 1) {
        files.push({ path: plan.file_path(index), bytes: plan.file_bytes(index) });
      }
      return { directories: JSON.parse(plan.directories_json()), files };
    } catch (cause) {
      throw new OndaCompilerError("failed to materialize Onda project image", { cause });
    } finally {
      plan?.free();
    }
  }

  async encodeBufferAsset(binding) {
    const normalized = normalizeBufferBinding(binding);
    try {
      return this.frontend.encode_buffer_asset(
        normalized.element,
        normalized.frames,
        normalized.channels,
        normalized.sampleRate,
        encodeCanonicalPayload(normalized.element, normalized.data),
      );
    } catch (cause) {
      throw new OndaCompilerError("failed to encode Onda buffer asset", { cause });
    }
  }

  async decodeBufferAsset(bytes) {
    return this.#decodeBuffer(
      () => this.frontend.decode_buffer_asset(normalizeBytes(bytes, "buffer asset")),
      "failed to decode Onda buffer asset",
    );
  }

  async decodeBufferFile(bytes, path = "buffer") {
    return this.#decodeBuffer(
      () => this.frontend.decode_buffer_file(
        normalizeBytes(bytes, "buffer file"),
        String(path),
      ),
      "failed to decode buffer file",
    );
  }

  async #decodeBuffer(decode, message) {
    let decoded;
    try {
      decoded = decode();
      const element = decoded.element();
      const payload = decoded.canonical_payload();
      return {
        element,
        frames: decoded.frames(),
        channels: decoded.channels(),
        sampleRate: decoded.sample_rate(),
        data: decodeCanonicalPayload(element, payload),
      };
    } catch (cause) {
      throw new OndaCompilerError(message, { cause });
    } finally {
      decoded?.free();
    }
  }

  async projectCapabilities() {
    return {
      imageFormatVersion: this.frontend.project_image_format_version(),
      bufferAssetFormatVersion: this.frontend.buffer_asset_format_version(),
      stdlibDigest: this.frontend.current_stdlib_digest(),
    };
  }

  async sendLspMessage(message) {
    if (!message || typeof message !== "object" || Array.isArray(message)) {
      throw new OndaCompilerError("LSP message must be a JSON-RPC object");
    }
    this.lsp ??= new this.frontend.OndaLsp();
    try {
      const responses = JSON.parse(this.lsp.handle_message(JSON.stringify(message)));
      if (!Array.isArray(responses)) {
        throw new Error("Onda LSP returned a non-array response batch");
      }
      return responses;
    } catch (cause) {
      throw new OndaCompilerError("Onda LSP failed to handle a message", { cause });
    }
  }

  async setLspAnalysisOptions(options = {}) {
    const analysis = normalizeLspAnalysisOptions(options);
    this.lsp ??= new this.frontend.OndaLsp();
    try {
      this.lsp.set_analysis_options(analysis.sampleRate, analysis.blockSize);
    } catch (cause) {
      throw new OndaCompilerError("failed to configure Onda LSP analysis", { cause });
    }
  }

  async dispose() {
    this.lsp?.free();
    this.lsp = null;
  }
}

class WorkerOndaCompiler {
  constructor(worker) {
    this.worker = worker;
    this.nextRequestId = 1;
    this.pending = new Map();
    this.onMessage = (event) => this.handleMessage(event.data);
    this.onError = (event) => this.failAll(event.error ?? new Error(event.message));
    worker.addEventListener("message", this.onMessage);
    worker.addEventListener("error", this.onError);
  }

  initialize(frontendWasm) {
    return this.request("initialize", { frontendWasm });
  }

  compileSource(source, options = {}) {
    return this.request("compileSource", { source, options });
  }

  inspectSourceConstants(source, options = {}) {
    return this.request("inspectSourceConstants", { source, options });
  }

  compileWorkspace(workspace, options = {}) {
    return this.request("compileWorkspace", { workspace, options });
  }

  inspectWorkspaceConstants(workspace, options = {}) {
    return this.request("inspectWorkspaceConstants", { workspace, options });
  }

  compileProjectImage(imageBytes, options = {}) {
    return this.request("compileProjectImage", { imageBytes, options });
  }

  inspectProjectImageConstants(imageBytes, options = {}) {
    return this.request("inspectProjectImageConstants", { imageBytes, options });
  }

  createProjectImage(sourceGraph, buffers = new Map()) {
    return this.request("createProjectImage", { sourceGraph, buffers });
  }

  inspectProjectImage(imageBytes) {
    return this.request("inspectProjectImage", { imageBytes });
  }

  loadProjectFiles(files, projectFilePath = null) {
    return this.request("loadProjectFiles", { files, projectFilePath });
  }

  materializeProjectImage(imageBytes, assetFileNames = new Map()) {
    return this.request("materializeProjectImage", { imageBytes, assetFileNames });
  }

  encodeBufferAsset(binding) {
    return this.request("encodeBufferAsset", { binding });
  }

  decodeBufferAsset(bytes) {
    return this.request("decodeBufferAsset", { bytes });
  }

  decodeBufferFile(bytes, path = "buffer") {
    return this.request("decodeBufferFile", { bytes, path });
  }

  projectCapabilities() {
    return this.request("projectCapabilities");
  }

  sendLspMessage(message) {
    return this.request("lspMessage", { message });
  }

  setLspAnalysisOptions(options = {}) {
    return this.request("lspAnalysisOptions", { options });
  }

  async dispose() {
    try {
      await this.request("dispose");
    } finally {
      this.terminate(new OndaCompilerError("compiler worker was disposed"));
    }
  }

  terminate(error) {
    this.worker.removeEventListener("message", this.onMessage);
    this.worker.removeEventListener("error", this.onError);
    this.worker.terminate();
    this.failAll(error);
  }

  request(type, fields = {}) {
    const requestId = this.nextRequestId;
    this.nextRequestId += 1;
    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      this.worker.postMessage({ type, requestId, ...fields });
    });
  }

  handleMessage(message) {
    const pending = this.pending.get(message?.requestId);
    if (!pending) return;
    this.pending.delete(message.requestId);
    if (message.type === "result") {
      pending.resolve(message.value);
      return;
    }
    const error = message.error?.diagnostics
      ? new OndaCompileError(message.error.diagnostics, {
        sourceFiles: message.error.sourceFiles,
        unresolvedSourceFiles: message.error.unresolvedSourceFiles,
      })
      : new OndaCompilerError(message.error?.message ?? "compiler worker failed");
    if (message.error?.name) error.name = message.error.name;
    if (message.error?.stack) error.stack = message.error.stack;
    pending.reject(error);
  }

  failAll(error) {
    for (const { reject } of this.pending.values()) reject(error);
    this.pending.clear();
  }
}

export async function createCompiler(options = {}) {
  if (options.worker === true) {
    const WorkerConstructor = options.Worker ?? globalThis.Worker;
    if (typeof WorkerConstructor !== "function") {
      throw new OndaCompilerError(
        "worker compilation requires a browser Worker constructor",
      );
    }
    const workerUrl = options.workerUrl ?? new URL("./worker.js", import.meta.url);
    const compiler = new WorkerOndaCompiler(
      new WorkerConstructor(workerUrl, {
        type: "module",
        name: "onda-wasm-compiler",
      }),
    );
    try {
      await compiler.initialize(options.frontendWasm);
      return compiler;
    } catch (error) {
      compiler.terminate(
        new OndaCompilerError("compiler worker initialization failed", { cause: error }),
      );
      throw error;
    }
  }

  if (!toolchainInitialization) {
    let initialization;
    initialization = initializeToolchain(options.frontendWasm).catch((error) => {
      if (toolchainInitialization === initialization) {
        toolchainInitialization = undefined;
      }
      throw error;
    });
    toolchainInitialization = initialization;
  }
  const toolchain = await toolchainInitialization;
  return new OndaCompiler(toolchain.frontend, toolchain.compileTrustedMir);
}

async function initializeToolchain(frontendWasm) {
  const [frontend, backend] = await Promise.all([
    import("../dist/frontend/onda_compiler_web.js"),
    import("../dist/backend/index.js"),
  ]);
  const moduleOrPath = frontendWasm ?? await defaultFrontendInput();
  try {
    await frontend.default({ module_or_path: moduleOrPath });
  } catch (cause) {
    throw new OndaCompilerError("failed to initialize the Onda frontend Wasm", { cause });
  }
  const producerSchema = frontend.mir_schema_version();
  if (producerSchema !== SUPPORTED_MIR_SCHEMA_VERSION) {
    throw new OndaCompilerError(
      `MIR schema mismatch: frontend produces ${producerSchema}, Binaryen backend supports ${SUPPORTED_MIR_SCHEMA_VERSION}`,
    );
  }
  return { frontend, compileTrustedMir: backend.compileTrustedMir };
}

export function createDefaultImports() {
  return {};
}

function normalizeCompileOptions(options) {
  const compile = normalizeCompileInputOptions(options);
  if (
    options.codegen !== undefined
    && (!options.codegen || typeof options.codegen !== "object" || Array.isArray(options.codegen))
  ) {
    throw configurationError("codegen options must be an object");
  }
  return {
    ...compile,
    codegen: options.codegen ?? {},
  };
}

function normalizeCompileConstInspectionOptions(options) {
  const compile = normalizeCompileInputOptions(options);
  if (options.codegen !== undefined) {
    throw configurationError("codegen options do not apply to compile constant inspection");
  }
  return compile;
}

function normalizeCompileInputOptions(options) {
  return {
    ...normalizeContextOptions(options),
    constantsJson: JSON.stringify(normalizeCompileConstants(options.constants ?? {})),
  };
}

function normalizeLspAnalysisOptions(options) {
  const context = normalizeContextOptions(options);
  if (options.constants !== undefined) {
    throw configurationError(
      "constants are compile-request inputs and cannot be set as LSP analysis options",
    );
  }
  if (options.codegen !== undefined) {
    throw configurationError("codegen options cannot be set as LSP analysis options");
  }
  return context;
}

function normalizeContextOptions(options) {
  if (!options || typeof options !== "object" || Array.isArray(options)) {
    throw configurationError("compiler options must be an object");
  }
  const sampleRate = options.sampleRate ?? 48_000;
  const blockSize = options.blockSize ?? 128;
  if (typeof sampleRate !== "number" || !Number.isFinite(sampleRate) || sampleRate <= 0) {
    throw configurationError("sampleRate must be finite and greater than zero");
  }
  if (!Number.isInteger(blockSize) || blockSize <= 0 || blockSize > MAX_BLOCK_SIZE) {
    throw configurationError(`blockSize must be between 1 and ${MAX_BLOCK_SIZE} frames`);
  }
  return { sampleRate, blockSize };
}

function normalizeWorkspace(workspace) {
  if (!workspace || typeof workspace !== "object" || Array.isArray(workspace)) {
    throw configurationError("workspace must contain an entry and source map");
  }
  if (typeof workspace.entry !== "string" || workspace.entry.length === 0) {
    throw configurationError("workspace.entry must be a non-empty string");
  }
  if (!workspace.sources || typeof workspace.sources !== "object" || Array.isArray(workspace.sources)) {
    throw configurationError("workspace.sources must be an object of paths to source strings");
  }
  for (const [path, source] of Object.entries(workspace.sources)) {
    if (typeof source !== "string") {
      throw configurationError(`workspace source '${path}' must be a string`);
    }
  }
  return workspace;
}

function normalizeCompileConstants(constants) {
  if (
    !constants
    || typeof constants !== "object"
    || Array.isArray(constants)
    || ArrayBuffer.isView(constants)
  ) {
    throw configurationError("constants must be an object or Map");
  }
  const entries = constants instanceof Map
    ? [...constants.entries()]
    : Object.entries(constants);
  return entries.map(([rawName, value]) => {
    const name = String(rawName);
    if (name.length === 0) {
      throw configurationError("compile constant names must be non-empty");
    }
    if (typeof value === "boolean") {
      return { name, element: "bool", array: false, values: [value] };
    }
    if (typeof value === "number") {
      return {
        name,
        element: "number",
        array: false,
        values: [encodeCompileNumber(value)],
      };
    }
    if (typeof value === "bigint") {
      return { name, element: "i64", array: false, values: [value.toString()] };
    }
    if (value instanceof Uint8Array) {
      const values = [...value];
      if (values.some((item) => item !== 0 && item !== 1)) {
        throw configurationError(
          `boolean compile constant array '${name}' may contain only 0 or 1`,
        );
      }
      return {
        name,
        element: "bool",
        array: true,
        values: values.map((item) => item !== 0),
      };
    }
    if (value instanceof Int32Array) {
      return { name, element: "i32", array: true, values: [...value] };
    }
    if (value instanceof BigInt64Array) {
      return {
        name,
        element: "i64",
        array: true,
        values: [...value].map((item) => item.toString()),
      };
    }
    if (value instanceof Float32Array) {
      return {
        name,
        element: "f32",
        array: true,
        values: encodeCompileFloatArray(value),
      };
    }
    if (value instanceof Float64Array) {
      return {
        name,
        element: "f64",
        array: true,
        values: encodeCompileFloatArray(value),
      };
    }
    if (Array.isArray(value) && value.every((item) => typeof item === "boolean")) {
      return { name, element: "bool", array: true, values: value };
    }
    throw configurationError(
      `compile constant '${name}' must be a boolean, number, bigint, or matching typed array`,
    );
  });
}

function encodeCompileFloatArray(values) {
  return [...values].map(encodeCompileNumber);
}

function encodeCompileNumber(value) {
  if (Object.is(value, -0)) return "-0";
  if (Number.isNaN(value)) return "NaN";
  if (value === Number.POSITIVE_INFINITY) return "Infinity";
  if (value === Number.NEGATIVE_INFINITY) return "-Infinity";
  return value;
}

function decodeCompileConstDescriptors(encoded) {
  let descriptors;
  try {
    descriptors = JSON.parse(encoded);
  } catch (cause) {
    throw new OndaCompilerError("failed to decode compile constant descriptors", { cause });
  }
  if (!Array.isArray(descriptors)) {
    throw new OndaCompilerError("frontend returned invalid compile constant descriptors");
  }
  return descriptors.map((descriptor) => ({
    name: descriptor.name,
    element: descriptor.element,
    kind: descriptor.kind,
    elementCount: descriptor.element_count,
    value: decodeCompileConstValue(descriptor.element, descriptor.kind, descriptor.values),
  }));
}

function decodeCompileConstValue(element, kind, values) {
  const decoded = element === "i64"
    ? values.map((value) => BigInt(value))
    : element === "f32" || element === "f64"
      ? values.map(decodeCompileFloat)
      : values;
  if (kind === "scalar") return decoded[0];
  if (element === "bool") return Uint8Array.from(decoded, (value) => value ? 1 : 0);
  if (element === "i32") return Int32Array.from(decoded);
  if (element === "i64") return BigInt64Array.from(decoded);
  if (element === "f32") return Float32Array.from(decoded);
  if (element === "f64") return Float64Array.from(decoded);
  throw new OndaCompilerError(`frontend returned unknown compile constant element '${element}'`);
}

function decodeCompileFloat(value) {
  if (value === "-0") return -0;
  if (value === "NaN") return Number.NaN;
  if (value === "Infinity") return Number.POSITIVE_INFINITY;
  if (value === "-Infinity") return Number.NEGATIVE_INFINITY;
  return value;
}

function consumeFrontendCompilation(compilation) {
  try {
    const mir = compilation.take_mir();
    const sourceFiles = normalizeSourceFiles(JSON.parse(compilation.source_files_json()));
    const sourceGraph = normalizeReturnedSourceGraph(
      JSON.parse(compilation.source_image_json()),
    );
    return { mir, sourceFiles, sourceGraph };
  } finally {
    compilation.free();
  }
}

function compileMirTransport(mir, codegen, compileTrustedMir, sourceFiles) {
  try {
    return compileTrustedMir(mir, codegen);
  } catch (cause) {
    if (cause instanceof OndaCompileError) throw cause;
    throw new OndaCompileError([{
      stage: "codegen",
      code: 0,
      message: cause instanceof Error ? cause.message : String(cause),
      file: null,
      line: 0,
      column: 0,
      end_line: 0,
      end_column: 0,
      trace: [],
    }], { cause, sourceFiles });
  }
}

function diagnosticsFromFrontend(error) {
  const encoded = typeof error === "string" ? error : error?.message;
  if (typeof encoded === "string") {
    try {
      const failure = JSON.parse(encoded);
      if (Array.isArray(failure)) {
        return new OndaCompileError(failure, { cause: error });
      }
      if (failure && Array.isArray(failure.diagnostics)) {
        return new OndaCompileError(failure.diagnostics, {
          cause: error,
          sourceFiles: failure.source_files,
          unresolvedSourceFiles: failure.unresolved_source_files,
        });
      }
    } catch {}
  }
  return new OndaCompileError([{
    stage: "frontend",
    code: 0,
    message: encoded ?? String(error),
    file: null,
    line: 0,
    column: 0,
    end_line: 0,
    end_column: 0,
    trace: [],
  }], { cause: error });
}

function configurationError(message) {
  return new OndaCompileError([{
    stage: "configuration",
    code: 0,
    message,
    file: null,
    line: 0,
    column: 0,
    end_line: 0,
    end_column: 0,
    trace: [],
  }]);
}

function normalizeDiagnostics(diagnostics) {
  if (!Array.isArray(diagnostics)) return [];
  return diagnostics.map((diagnostic) => ({
    stage: String(diagnostic?.stage ?? "unknown"),
    code: Number.isInteger(diagnostic?.code) ? diagnostic.code : 0,
    message: String(diagnostic?.message ?? "compilation failed"),
    file: typeof diagnostic?.file === "string" ? diagnostic.file : null,
    line: Number.isInteger(diagnostic?.line) ? diagnostic.line : 0,
    column: Number.isInteger(diagnostic?.column) ? diagnostic.column : 0,
    end_line: Number.isInteger(diagnostic?.end_line) ? diagnostic.end_line : 0,
    end_column: Number.isInteger(diagnostic?.end_column) ? diagnostic.end_column : 0,
    trace: Array.isArray(diagnostic?.trace)
      ? diagnostic.trace.map((entry) => String(entry))
      : [],
  }));
}

function normalizeSourceFiles(sourceFiles) {
  if (!Array.isArray(sourceFiles)) return [];
  return sourceFiles.map((path) => String(path));
}

function normalizeBytes(value, context) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new OndaCompilerError(`${context} must be an ArrayBuffer or typed-array view`);
}

function normalizeSourceGraph(graph) {
  if (!graph || typeof graph !== "object" || Array.isArray(graph)) {
    throw new OndaCompilerError("source graph must be an object");
  }
  const entry = String(graph.entry ?? "");
  const stdlibDigest = String(graph.stdlibDigest ?? graph.stdlib_digest ?? "");
  if (!entry || !stdlibDigest) {
    throw new OndaCompilerError("source graph requires entry and stdlibDigest");
  }
  const documents = Array.isArray(graph.documents) ? graph.documents.map((document) => ({
    path: String(document?.path ?? ""),
    contents: String(document?.contents ?? ""),
  })) : [];
  const resolutions = Array.isArray(graph.resolutions) ? graph.resolutions.map((resolution) => ({
    source: String(resolution?.source ?? ""),
    kind: String(resolution?.kind ?? ""),
    specifier: String(resolution?.specifier ?? ""),
    target: String(resolution?.target ?? ""),
  })) : [];
  return {
    entry,
    stdlib_digest: stdlibDigest,
    documents,
    resolutions,
  };
}

function normalizeReturnedSourceGraph(graph) {
  if (!graph) return null;
  return {
    entry: String(graph.entry),
    stdlibDigest: String(graph.stdlib_digest),
    documents: graph.documents.map((document) => ({
      path: String(document.path),
      contents: String(document.contents),
    })),
    resolutions: graph.resolutions.map((resolution) => ({
      source: String(resolution.source),
      kind: String(resolution.kind),
      specifier: String(resolution.specifier),
      target: String(resolution.target),
    })),
  };
}

function normalizeProjectImageInfo(info) {
  return {
    formatVersion: Number(info.format_version),
    contentDigest: String(info.content_digest),
    sourceGraph: normalizeReturnedSourceGraph(info.sources),
    buffers: info.buffers.map((buffer) => ({
      name: String(buffer.name),
      assetId: String(buffer.asset_id),
      element: String(buffer.element),
      frames: Number(buffer.frames),
      channels: Number(buffer.channels),
      sampleRate: Number(buffer.sample_rate),
    })),
  };
}

function normalizeBufferAssetEntries(buffers) {
  const entries = buffers instanceof Map
    ? [...buffers]
    : buffers && typeof buffers === "object" && !Array.isArray(buffers)
      ? Object.entries(buffers)
      : null;
  if (!entries) throw new OndaCompilerError("project buffers must be a Map or object");
  return entries.map(([name, bytes]) => {
    if (typeof name !== "string" || !name) {
      throw new OndaCompilerError("project buffer names must be non-empty strings");
    }
    return [name, normalizeBytes(bytes, `project buffer '${name}'`)];
  });
}

function normalizeAssetFileNameEntries(fileNames) {
  const entries = fileNames instanceof Map
    ? [...fileNames]
    : fileNames && typeof fileNames === "object" && !Array.isArray(fileNames)
      ? Object.entries(fileNames)
      : null;
  if (!entries) throw new OndaCompilerError("asset filenames must be a Map or object");
  return entries.map(([name, fileName]) => {
    if (typeof name !== "string" || !name || typeof fileName !== "string" || !fileName) {
      throw new OndaCompilerError("asset filenames require non-empty buffer names and filenames");
    }
    return [name, fileName];
  });
}

function normalizeProjectFilePath(path) {
  if (typeof path !== "string" || !path) {
    throw new OndaCompilerError("selected project manifest must be a non-empty string");
  }
  return path;
}

function normalizeProjectFileEntries(files) {
  const entries = files instanceof Map
    ? [...files]
    : files && typeof files === "object" && !Array.isArray(files)
      ? Object.entries(files)
      : null;
  if (!entries) throw new OndaCompilerError("project files must be a Map or object");
  return entries.map(([path, bytes]) => {
    if (typeof path !== "string" || !path) {
      throw new OndaCompilerError("project file paths must be non-empty strings");
    }
    return [path, normalizeBytes(bytes, `project file '${path}'`)];
  });
}

function normalizeBufferBinding(binding) {
  if (!binding || typeof binding !== "object" || Array.isArray(binding)) {
    throw new OndaCompilerError("buffer binding must be an object");
  }
  const element = String(binding.element ?? binding.scalar ?? "");
  const frames = Number(binding.frames);
  const channels = Number(binding.channels);
  const sampleRate = Number(binding.sampleRate);
  const data = binding.data;
  const expectedConstructor = {
    bool: Uint8Array,
    i32: Int32Array,
    i64: BigInt64Array,
    f32: Float32Array,
    f64: Float64Array,
  }[element];
  if (!expectedConstructor || !(data instanceof expectedConstructor)) {
    throw new OndaCompilerError(`buffer element '${element}' requires ${expectedConstructor?.name ?? "a supported typed array"}`);
  }
  if (
    !Number.isInteger(frames) || frames <= 0
    || !Number.isInteger(channels) || channels <= 0
    || !Number.isFinite(sampleRate) || sampleRate <= 0
    || data.length !== frames * channels
  ) {
    throw new OndaCompilerError("buffer binding has an invalid shape or sample rate");
  }
  return { element, frames, channels, sampleRate, data };
}

function encodeCanonicalPayload(element, data) {
  if (element === "bool") {
    if (data.some((value) => value > 1)) {
      throw new OndaCompilerError("bool buffer values must be 0 or 1");
    }
    return data.slice();
  }
  const bytes = new Uint8Array(data.length * data.BYTES_PER_ELEMENT);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < data.length; index += 1) {
    const offset = index * data.BYTES_PER_ELEMENT;
    if (element === "i32") view.setInt32(offset, data[index], true);
    else if (element === "i64") view.setBigInt64(offset, data[index], true);
    else if (element === "f32") view.setFloat32(offset, data[index], true);
    else view.setFloat64(offset, data[index], true);
  }
  return bytes;
}

function decodeCanonicalPayload(element, payload) {
  if (element === "bool") return payload.slice();
  const elementBytes = element === "i32" || element === "f32" ? 4 : 8;
  const length = payload.byteLength / elementBytes;
  const output = element === "i32" ? new Int32Array(length)
    : element === "i64" ? new BigInt64Array(length)
      : element === "f32" ? new Float32Array(length)
        : new Float64Array(length);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  for (let index = 0; index < length; index += 1) {
    const offset = index * elementBytes;
    output[index] = element === "i32" ? view.getInt32(offset, true)
      : element === "i64" ? view.getBigInt64(offset, true)
        : element === "f32" ? view.getFloat32(offset, true)
          : view.getFloat64(offset, true);
  }
  return output;
}
