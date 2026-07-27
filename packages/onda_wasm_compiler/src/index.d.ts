export const ONDA_VERSION: string;
export const MIR_SCHEMA_VERSION: number;
export {
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
export type {
  OndaProcessorArtifact,
  OndaProcessorMetadata,
} from "@onda-lang/processor-abi";
import type { OndaProcessorArtifact } from "@onda-lang/processor-abi";

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

export interface OndaCompilationResult {
  artifact: OndaProcessorArtifact;
  /** Entry first, then transitive non-stdlib imports/includes in discovery order. */
  sourceFiles: string[];
}

export interface OndaCompilerInstance {
  compileSource(
    source: string,
    options?: OndaCompileOptions,
  ): Promise<OndaCompilationResult>;
  compileProject(
    project: OndaProject,
    options?: OndaCompileOptions,
  ): Promise<OndaCompilationResult>;
  sendLspMessage(message: OndaLspMessage): Promise<OndaLspMessage[]>;
  setLspAnalysisOptions(options?: OndaCompileOptions): Promise<void>;
  dispose(): Promise<void>;
}

export interface OndaLspMessage {
  jsonrpc: "2.0";
  id?: string | number | null;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
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
  frontendWasm?: string | URL | ArrayBuffer | ArrayBufferView | WebAssembly.Module;
  Worker?: OndaWorkerConstructor;
}

export class OndaCompilerError extends Error {
  cause?: unknown;
}

export class OndaCompileError extends OndaCompilerError {
  readonly diagnostics: OndaCompilerDiagnostic[];
  readonly sourceFiles: string[];
}

export class OndaBinaryenError extends Error {}

export function createCompiler(
  options?: DirectCompilerOptions | WorkerCompilerOptions,
): Promise<OndaCompilerInstance>;

export function createDefaultImports(): Record<string, never>;
