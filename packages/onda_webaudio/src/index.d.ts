export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME: "onda-wasm-processor";
export const ONDA_INIT_PRESERVE_PINNED: 0;
export const ONDA_INIT_FULL: 1;
export type OndaInitMode = 0 | 1;

export type {
  OndaParamDomain,
  OndaPreparedParamControl,
  OndaProcessorArtifact,
  OndaProcessorMetadata,
} from "@onda-lang/processor-abi";
export {
  createParamDomain,
  createParamControl,
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
  /**
   * Initial external-buffer bindings keyed by physical name, grouped by logical array name, or
   * ordered by physical descriptor slot. Missing and null slots use neutral one-frame storage.
   */
  buffers?: Record<string, unknown> | unknown[];
  /** Preallocated capacity for dynamic event payloads. Defaults to 64 KiB. */
  eventPayloadCapacityBytes?: number;
  /**
   * Reusable call-scoped storage for delegate records. Defaults to 64 KiB; zero disables host
   * collection. Capacity is a host policy because occurrence counts and slice sizes may be dynamic.
   */
  delegateCapacityBytes?: number;
  /** Reusable call-scoped storage for print records. Defaults to 64 KiB; zero disables delivery. */
  printCapacityBytes?: number;
  /** Reusable module compiled outside the audio rendering thread. */
  compiledModule?: WebAssembly.Module;
  nodeOptions?: AudioWorkletNodeOptions;
  AudioWorkletNode?: typeof AudioWorkletNode;
  /** Low-level node-option builders only: initialize in the worklet constructor. */
  initialize?: boolean;
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
export function createOndaAudioProcessorInitialized(
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
  /**
   * Subscribe to batches decoded after generated execution. A nonzero overflowCount means complete
   * records were dropped for insufficient configured capacity. Returns an unsubscribe function.
   */
  onDelegates(
    listener: (batch: {
      type: "onda-delegates";
      operation: string;
      occurrences: Array<{ index: number; name: string; values: Record<string, unknown> }>;
      overflowCount: number;
    }) => void,
  ): () => boolean;
  onPrint(
    listener: (batch: {
      type: "onda-print";
      operation: string;
      text: string;
      entries: import("@onda-lang/processor-abi").OndaPrintEntry[];
      overflowCount: number;
      transportDropCount: number;
    }) => void,
  ): () => boolean;
  init(mode: OndaInitMode): Promise<any>;
  snapshot(): Promise<Uint8Array>;
  restoreSnapshot(snapshot: Uint8Array | ArrayBuffer): Promise<any>;
  readControlOutputs(): Promise<Record<string, unknown>>;
  readBuffer(buffer: string | number): Promise<any>;
  /** Idempotently closes this adapter. Pending and subsequent operations reject. */
  close(reason?: Error): void;
}
