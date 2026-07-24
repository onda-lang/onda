export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME: "onda-wasm-processor";

export type { OndaProcessorArtifact, OndaProcessorMetadata } from "@onda-lang/processor-abi";
export {
  constrainParamPlain,
  paramNormalizedToPlain,
  paramPlainToNormalized,
} from "@onda-lang/processor-abi";
import type {
  OndaProcessorArtifact,
  OndaProcessorMetadata,
} from "@onda-lang/processor-abi";

export interface OndaAudioProcessorOptions {
  workletUrl?: string | URL;
  /** Initial plain Onda parameter values, keyed or ordered by descriptor parameter. */
  params?: Record<string, unknown> | unknown[];
  buffers?: Record<string, unknown> | unknown[];
  /** Preallocated capacity for dynamic event payloads. Defaults to 64 KiB. */
  eventPayloadCapacityBytes?: number;
  /** Reusable module compiled outside the audio rendering thread. */
  compiledModule?: WebAssembly.Module;
  nodeOptions?: AudioWorkletNodeOptions;
  AudioWorkletNode?: typeof AudioWorkletNode;
}

export function flattenedAudioChannelCount(ports?: unknown[]): number;
export function ondaAudioWorkletNodeOptions(
  artifact: OndaProcessorArtifact,
  options?: OndaAudioProcessorOptions,
): AudioWorkletNodeOptions;
export function registerOndaAudioWorklet(
  context: BaseAudioContext,
  workletUrl?: string | URL,
): Promise<void>;
export function createOndaAudioProcessor(
  context: BaseAudioContext,
  artifact: OndaProcessorArtifact,
  options?: OndaAudioProcessorOptions,
): Promise<OndaAudioProcessor>;
export function compileOndaProcessorModule(
  artifact: OndaProcessorArtifact,
): Promise<WebAssembly.Module>;

export class OndaAudioProcessor {
  constructor(node: AudioWorkletNode, metadata?: OndaProcessorMetadata | null);
  readonly node: AudioWorkletNode;
  readonly metadata: OndaProcessorMetadata | null;
  request(type: string, fields?: Record<string, unknown>, transfer?: Transferable[]): Promise<any>;
  /** Set a plain Onda value; ranged scalar values are clamped and snapped. */
  setParam(param: string | number, value: unknown): Promise<any>;
  /** Map a host value in [0, 1] through the descriptor and set the resulting plain value. */
  setParamNormalized(param: string | number, value: number): Promise<any>;
  trigger(event: string | number, values?: Record<string, unknown> | unknown[]): Promise<any>;
  reset(): Promise<any>;
  snapshot(): Promise<Uint8Array>;
  restoreSnapshot(snapshot: Uint8Array | ArrayBuffer): Promise<any>;
  readControlOutputs(): Promise<Record<string, unknown>>;
  readBuffer(buffer: string | number): Promise<any>;
  close(reason?: Error): void;
}
