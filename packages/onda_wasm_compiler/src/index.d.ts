export const ONDA_VERSION: string;
export const MIR_SCHEMA_VERSION: number;
export {
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
  constants?: Map<string, OndaCompileConstValue>
    | Record<string, OndaCompileConstValue>;
  codegen?: OndaCodegenOptions;
}

export type OndaCompileConstValue =
  | boolean
  | number
  | bigint
  | boolean[]
  | Uint8Array
  | Int32Array
  | BigInt64Array
  | Float32Array
  | Float64Array;

export interface OndaSourceWorkspace {
  entry: string;
  sources: Record<string, string>;
}

export type OndaSourceReferenceKind = "include" | "import";

export interface OndaSourceDocument {
  path: string;
  contents: string;
}

export interface OndaSourceResolution {
  source: string;
  kind: OndaSourceReferenceKind;
  specifier: string;
  target: string;
}

export interface OndaSourceGraph {
  entry: string;
  stdlibDigest: string;
  documents: OndaSourceDocument[];
  resolutions: OndaSourceResolution[];
}

export type OndaBufferElement = "bool" | "i32" | "i64" | "f32" | "f64";
export type OndaBufferData =
  | Uint8Array
  | Int32Array
  | BigInt64Array
  | Float32Array
  | Float64Array;

export interface OndaBufferAssetBinding {
  element: OndaBufferElement;
  frames: number;
  channels: number;
  sampleRate: number;
  data: OndaBufferData;
}

export interface OndaProjectBufferInfo {
  name: string;
  assetId: string;
  element: OndaBufferElement;
  frames: number;
  channels: number;
  sampleRate: number;
}

export interface OndaProjectImageInfo {
  formatVersion: number;
  contentDigest: string;
  sourceGraph: OndaSourceGraph;
  buffers: OndaProjectBufferInfo[];
}

export interface OndaSerializedProjectImage extends OndaProjectImageInfo {
  bytes: Uint8Array;
}

export interface OndaMaterializedProjectFile {
  path: string;
  bytes: Uint8Array;
}

export interface OndaProjectMaterialization {
  directories: string[];
  files: OndaMaterializedProjectFile[];
}

export interface OndaProjectCapabilities {
  imageFormatVersion: number;
  bufferAssetFormatVersion: number;
  stdlibDigest: string;
}

export interface OndaCompilationResult {
  artifact: OndaProcessorArtifact;
  /** Entry first, then transitive non-stdlib imports/includes in discovery order. */
  sourceFiles: string[];
  /** Exact resolved graph for successful multi-file/project-image compilation. */
  sourceGraph: OndaSourceGraph | null;
}

export interface OndaCompilerInstance {
  compileSource(
    source: string,
    options?: OndaCompileOptions,
  ): Promise<OndaCompilationResult>;
  compileWorkspace(
    workspace: OndaSourceWorkspace,
    options?: OndaCompileOptions,
  ): Promise<OndaCompilationResult>;
  compileProjectImage(
    imageBytes: ArrayBuffer | ArrayBufferView,
    options?: OndaCompileOptions,
  ): Promise<OndaCompilationResult>;
  createProjectImage(
    sourceGraph: OndaSourceGraph,
    buffers?: Map<string, ArrayBuffer | ArrayBufferView>
      | Record<string, ArrayBuffer | ArrayBufferView>,
  ): Promise<OndaSerializedProjectImage>;
  inspectProjectImage(
    imageBytes: ArrayBuffer | ArrayBufferView,
  ): Promise<OndaProjectImageInfo>;
  loadProjectFiles(
    files: Map<string, ArrayBuffer | ArrayBufferView>
      | Record<string, ArrayBuffer | ArrayBufferView>,
    projectFilePath?: string | null,
  ): Promise<OndaSerializedProjectImage>;
  materializeProjectImage(
    imageBytes: ArrayBuffer | ArrayBufferView,
    assetFileNames?: Map<string, string> | Record<string, string>,
  ): Promise<OndaProjectMaterialization>;
  encodeBufferAsset(binding: OndaBufferAssetBinding): Promise<Uint8Array>;
  decodeBufferAsset(bytes: ArrayBuffer | ArrayBufferView): Promise<OndaBufferAssetBinding>;
  decodeBufferFile(
    bytes: ArrayBuffer | ArrayBufferView,
    path?: string,
  ): Promise<OndaBufferAssetBinding>;
  projectCapabilities(): Promise<OndaProjectCapabilities>;
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
  /** Referenced non-stdlib source candidates which were not present. */
  readonly unresolvedSourceFiles: string[];
}

export class OndaBinaryenError extends Error {}

export function createCompiler(
  options?: DirectCompilerOptions | WorkerCompilerOptions,
): Promise<OndaCompilerInstance>;

export function createDefaultImports(): Record<string, never>;
