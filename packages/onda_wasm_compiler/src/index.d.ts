export const ONDA_VERSION: string;
export const MIR_SCHEMA_VERSION: number;
export const PROCESSOR_ARTIFACT_FORMAT: "onda-processor";
export const PROCESSOR_ARTIFACT_FORMAT_VERSION: 3;
export const PROCESSOR_ABI_VERSION: 1;

export interface OndaCompilerDiagnostic {
  stage: string;
  code: number;
  message: string;
  file: string | null;
  line: number;
  column: number;
  end_line: number;
  end_column: number;
  trace: string[];
}

export interface OndaProcessorMetadata {
  format: "onda-processor";
  format_version: 3;
  artifact_kind: "webassembly_module" | "relocatable_object";
  abi_version: 1;
  backend: string;
  mir_schema_version: number;
  target: Record<string, any>;
  integration: Record<string, any>;
  compile: { sample_rate: number; block_size: number; [key: string]: any };
  exports: { init: string; process: string; events: string[]; [key: string]: any };
  runtime: Record<string, any>;
  metadata: Record<string, any>;
  [key: string]: any;
}

export interface OndaProcessorArtifact {
  wasm: Uint8Array;
  metadata: OndaProcessorMetadata;
  wat?: string;
}

export interface OndaCodegenOptions {
  optimize?: boolean;
  optimizeLevel?: 0 | 1 | 2 | 3 | 4;
  shrinkLevel?: 0 | 1 | 2;
  fastMath?: boolean;
  simd?: boolean;
  allowInliningFunctionsWithLoops?: boolean;
  emitText?: boolean;
}

export interface OndaCompileOptions {
  sampleRate?: number;
  blockSize?: number;
  codegen?: OndaCodegenOptions;
}

export interface OndaProject {
  entry: string;
  sources: Record<string, string>;
}

export interface OndaCompilerInstance {
  compileSource(
    source: string,
    options?: OndaCompileOptions,
  ): Promise<OndaProcessorArtifact>;
  compileProject(
    project: OndaProject,
    options?: OndaCompileOptions,
  ): Promise<OndaProcessorArtifact>;
  dispose(): Promise<void>;
}

export interface DirectCompilerOptions {
  worker?: false;
  frontendWasm?: string | URL | ArrayBuffer | ArrayBufferView | WebAssembly.Module;
}

export interface OndaWorkerLike {
  addEventListener(type: "message" | "error", listener: (event: any) => void): void;
  removeEventListener(type: "message" | "error", listener: (event: any) => void): void;
  postMessage(message: unknown): void;
  terminate(): void;
}

export interface OndaWorkerConstructor {
  new (
    url: string | URL,
    options: { type: "module"; name: string },
  ): OndaWorkerLike;
}

export interface WorkerCompilerOptions {
  worker: true;
  workerUrl?: string | URL;
  Worker?: OndaWorkerConstructor;
}

export class OndaCompilerError extends Error {
  cause?: unknown;
}

export class OndaCompileError extends OndaCompilerError {
  readonly diagnostics: OndaCompilerDiagnostic[];
}

export class OndaBinaryenError extends Error {}
export class OndaArtifactError extends Error {}

export function createCompiler(
  options?: DirectCompilerOptions | WorkerCompilerOptions,
): Promise<OndaCompilerInstance>;

export function createDefaultImports(): Record<string, never>;
export function validateProcessorMetadata(
  metadata: object,
  expectedKind?: string | null,
): OndaProcessorMetadata;
export function validateProcessorArtifact(
  artifact: OndaProcessorArtifact,
  options?: { inspectModule?: boolean },
): OndaProcessorArtifact;
export function serializeProcessorMetadata(metadata: object, space?: number): string;
export function parseProcessorMetadata(
  input: string | object,
  expectedKind?: string | null,
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
): Promise<OndaProcessorArtifact>;
