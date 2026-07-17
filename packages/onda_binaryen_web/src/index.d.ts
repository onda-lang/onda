export const PROCESSOR_ARTIFACT_FORMAT: "onda-processor";
export const PROCESSOR_ARTIFACT_FORMAT_VERSION: 3;
export const PROCESSOR_ABI_VERSION: 1;

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

export interface OndaBinaryenOptions {
  optimize?: boolean;
  optimizeLevel?: 0 | 1 | 2 | 3 | 4;
  shrinkLevel?: 0 | 1 | 2;
  fastMath?: boolean;
  simd?: boolean;
  allowInliningFunctionsWithLoops?: boolean;
  emitText?: boolean;
}

export class OndaBinaryenError extends Error {}
export class OndaArtifactError extends Error {}
export function compileMir(
  mir: string | ArrayBuffer | ArrayBufferView | object,
  options?: OndaBinaryenOptions,
): OndaProcessorArtifact;
export function compileTrustedMir(
  mir: string | ArrayBuffer | ArrayBufferView | object,
  options?: OndaBinaryenOptions,
): OndaProcessorArtifact;
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
