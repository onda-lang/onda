export const SUPPORTED_MIR_SCHEMA_VERSION: number;
export {
  OndaArtifactError,
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_BEGIN_BLOCK,
  PROCESSOR_END_BLOCK,
  PROCESSOR_EXECUTION_INPUT_REJECTED,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  PROCESSOR_FULL_BLOCK,
  PROCESSOR_INIT_FULL,
  PROCESSOR_INIT_PRESERVE_PINNED,
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
  OndaProcessorInitMode,
  OndaProcessorMetadata,
} from "@onda-lang/processor-abi";
import type { OndaProcessorArtifact } from "@onda-lang/processor-abi";

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
export function compileTrustedMir(
  mir: string | ArrayBuffer | ArrayBufferView | object,
  options?: OndaBinaryenOptions,
): OndaProcessorArtifact;
export function createDefaultImports(): Record<string, never>;
