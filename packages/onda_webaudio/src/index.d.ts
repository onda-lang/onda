export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME: "onda-wasm-processor";

export interface OndaProcessorArtifact {
  wasm: Uint8Array | ArrayBuffer;
  metadata: Record<string, any>;
}

export interface OndaAudioProcessorOptions {
  workletUrl?: string | URL;
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
  constructor(node: AudioWorkletNode);
  readonly node: AudioWorkletNode;
  request(type: string, fields?: Record<string, unknown>, transfer?: Transferable[]): Promise<any>;
  setParam(param: string | number, value: unknown): Promise<any>;
  trigger(event: string | number, values?: Record<string, unknown> | unknown[]): Promise<any>;
  reset(): Promise<any>;
  snapshot(): Promise<Uint8Array>;
  restoreSnapshot(snapshot: Uint8Array | ArrayBuffer): Promise<any>;
  readControlOutputs(): Promise<Record<string, unknown>>;
  readBuffer(buffer: string | number): Promise<any>;
  close(reason?: Error): void;
}
