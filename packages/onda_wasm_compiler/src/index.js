import {
  OndaArtifactError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
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
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    const { mir, sourceFiles } = consumeFrontendCompilation(frontendCompilation);
    const artifact = compileMirTransport(
      mir,
      compile.codegen,
      this.compileTrustedMir,
      sourceFiles,
    );
    return { artifact, sourceFiles };
  }

  async compileProject(project, options = {}) {
    if (!project || typeof project !== "object" || Array.isArray(project)) {
      throw configurationError("project must contain an entry and source map");
    }
    if (typeof project.entry !== "string" || project.entry.length === 0) {
      throw configurationError("project.entry must be a non-empty string");
    }
    if (!project.sources || typeof project.sources !== "object" || Array.isArray(project.sources)) {
      throw configurationError("project.sources must be an object of paths to source strings");
    }
    for (const [path, source] of Object.entries(project.sources)) {
      if (typeof source !== "string") {
        throw configurationError(`project source '${path}' must be a string`);
      }
    }

    const compile = normalizeCompileOptions(options);
    let frontendCompilation;
    try {
      frontendCompilation = this.frontend.compile_project_to_mir_messagepack(
        project.entry,
        JSON.stringify(project.sources),
        compile.sampleRate,
        compile.blockSize,
      );
    } catch (error) {
      throw diagnosticsFromFrontend(error);
    }
    const { mir, sourceFiles } = consumeFrontendCompilation(frontendCompilation);
    const artifact = compileMirTransport(
      mir,
      compile.codegen,
      this.compileTrustedMir,
      sourceFiles,
    );
    return { artifact, sourceFiles };
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
    const compile = normalizeCompileOptions(options);
    this.lsp ??= new this.frontend.OndaLsp();
    try {
      this.lsp.set_analysis_options(compile.sampleRate, compile.blockSize);
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

  compileProject(project, options = {}) {
    return this.request("compileProject", { project, options });
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
  if (
    options.codegen !== undefined
    && (!options.codegen || typeof options.codegen !== "object" || Array.isArray(options.codegen))
  ) {
    throw configurationError("codegen options must be an object");
  }
  return { sampleRate, blockSize, codegen: options.codegen ?? {} };
}

function consumeFrontendCompilation(compilation) {
  try {
    const mir = compilation.take_mir();
    const sourceFiles = normalizeSourceFiles(JSON.parse(compilation.source_files_json()));
    return { mir, sourceFiles };
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
