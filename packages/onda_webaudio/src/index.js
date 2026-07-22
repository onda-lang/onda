import {
  validateProcessorArtifact,
  validateProcessorModule,
} from "@onda-lang/processor-abi";

export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";

const registrationByContext = new WeakMap();

export function flattenedAudioChannelCount(ports = []) {
  if (!Array.isArray(ports)) {
    throw new Error("processor audio ports must be an array");
  }
  return ports.reduce((count, port, index) => {
    const channels = Number(port?.array_len);
    if (!Number.isSafeInteger(channels) || channels <= 0) {
      throw new Error(
        `processor audio port ${index} has invalid array_len '${String(port?.array_len)}'`,
      );
    }
    return count + channels;
  }, 0);
}

export function ondaAudioWorkletNodeOptions(artifact, options = {}) {
  const validated = validateExecutableArtifact(
    artifact,
    options.compiledModule === undefined,
  );
  return audioWorkletNodeOptionsFromValidated(
    validated,
    options,
    options.compiledModule !== undefined,
  );
}

function audioWorkletNodeOptionsFromValidated(
  { wasm, metadata },
  options,
  validateCompiledModule,
) {
  const inputChannels = flattenedAudioChannelCount(metadata.metadata.inputs);
  const outputChannels = flattenedAudioChannelCount(metadata.metadata.outputs);
  if (inputChannels > 32 || outputChannels > 32) {
    throw new Error("Web Audio supports at most 32 flattened channels per Onda node");
  }
  if (inputChannels === 0 && outputChannels === 0) {
    throw new Error(
      "the Web Audio adapter requires at least one audio input or output to establish the render quantum",
    );
  }
  if (validateCompiledModule) {
    validateProcessorModule(options.compiledModule, metadata);
  }
  const nodeOptions = {
    ...options.nodeOptions,
    numberOfInputs: inputChannels ? 1 : 0,
    numberOfOutputs: outputChannels ? 1 : 0,
    channelCount: Math.max(inputChannels, 1),
    channelCountMode: "explicit",
    channelInterpretation: "discrete",
    processorOptions: {
      ...options.nodeOptions?.processorOptions,
      ...(options.compiledModule === undefined
        ? { wasmBytes: wasm }
        : { wasmModule: options.compiledModule }),
      metadata,
      params: options.params ?? {},
      buffers: options.buffers ?? {},
      eventPayloadCapacityBytes: options.eventPayloadCapacityBytes,
    },
  };
  if (outputChannels) {
    nodeOptions.outputChannelCount = [outputChannels];
  } else {
    delete nodeOptions.outputChannelCount;
  }
  return nodeOptions;
}

export async function registerOndaAudioWorklet(
  context,
  workletUrl = new URL("./worklet.js", import.meta.url),
) {
  if (!context?.audioWorklet?.addModule) {
    throw new Error("an AudioContext with AudioWorklet support is required");
  }
  let registration = registrationByContext.get(context);
  if (!registration) {
    registration = context.audioWorklet.addModule(workletUrl);
    registrationByContext.set(context, registration);
  }
  try {
    await registration;
  } catch (error) {
    registrationByContext.delete(context);
    throw error;
  }
}

export async function createOndaAudioProcessor(context, artifact, options = {}) {
  const validated = validateExecutableArtifact(artifact, false);
  validateContextSampleRate(context, validated.metadata);
  if (options.compiledModule !== undefined) {
    validateProcessorModule(options.compiledModule, validated.metadata);
  }
  const [, compiledModule] = await Promise.all([
    registerOndaAudioWorklet(context, options.workletUrl),
    options.compiledModule === undefined
      ? compileValidatedProcessorModule(validated)
      : Promise.resolve(options.compiledModule),
  ]);
  const NodeConstructor = options.AudioWorkletNode ?? globalThis.AudioWorkletNode;
  if (typeof NodeConstructor !== "function") {
    throw new Error("AudioWorkletNode is not available in this environment");
  }
  const node = new NodeConstructor(
    context,
    ONDA_AUDIO_WORKLET_PROCESSOR_NAME,
    audioWorkletNodeOptionsFromValidated(
      validated,
      { ...options, compiledModule },
      false,
    ),
  );
  return new OndaAudioProcessor(node);
}

export async function compileOndaProcessorModule(artifact) {
  return compileValidatedProcessorModule(
    validateExecutableArtifact(artifact, false),
  );
}

async function compileValidatedProcessorModule({ wasm, metadata }) {
  const module = await WebAssembly.compile(wasm);
  validateProcessorModule(module, metadata);
  return module;
}

export class OndaAudioProcessor {
  constructor(node) {
    this.node = node;
    this.nextRequestId = 1;
    this.pending = new Map();
    this.handleMessage = (event) => {
      const message = event.data ?? {};
      if (message.requestId === undefined) return;
      const pending = this.pending.get(message.requestId);
      if (!pending) return;
      this.pending.delete(message.requestId);
      if (message.type === "onda-error") {
        pending.reject(new Error(message.error));
      } else {
        pending.resolve(message);
      }
    };
    node.port.addEventListener("message", this.handleMessage);
    node.port.start?.();
  }

  request(type, fields = {}, transfer = []) {
    const requestId = this.nextRequestId++;
    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      try {
        this.node.port.postMessage({ type, ...fields, requestId }, transfer);
      } catch (error) {
        this.pending.delete(requestId);
        reject(error);
      }
    });
  }

  setParam(param, value) {
    return this.request("set-param", { param, value });
  }

  trigger(event, values = {}) {
    return this.request("event", { event, values });
  }

  reset() {
    return this.request("reset");
  }

  async snapshot() {
    return (await this.request("snapshot")).bytes;
  }

  restoreSnapshot(snapshot) {
    const bytes = snapshot instanceof Uint8Array
      ? snapshot.slice()
      : new Uint8Array(snapshot.slice(0));
    return this.request("restore-snapshot", { snapshot: bytes }, [bytes.buffer]);
  }

  async readControlOutputs() {
    return (await this.request("read-control-outputs")).values;
  }

  async readBuffer(buffer) {
    return this.request("read-buffer", { buffer });
  }

  close(reason = new Error("Onda AudioWorklet processor closed")) {
    this.node.port.removeEventListener("message", this.handleMessage);
    for (const pending of this.pending.values()) pending.reject(reason);
    this.pending.clear();
  }
}

function validateExecutableArtifact(artifact, inspectModule = true) {
  const { wasm, metadata } = validateProcessorArtifact(artifact, { inspectModule });
  if (
    metadata.integration?.profile?.kind !== "core_webassembly_module"
    || metadata?.target?.pointer_model !== "linear_memory_offset"
    || metadata?.target?.pointer_width_bits !== 32
  ) {
    throw new Error("the Web Audio adapter requires an Onda wasm32 module artifact");
  }
  for (const field of ["inputs", "outputs"]) {
    if (!Array.isArray(metadata.metadata?.[field])) {
      throw new Error(`processor metadata is missing '${field}'`);
    }
  }
  return { wasm, metadata };
}

function validateContextSampleRate(context, metadata) {
  const actual = Number(context?.sampleRate);
  const compiled = Number(metadata.compile.sample_rate);
  if (!Number.isFinite(actual) || actual <= 0) {
    throw new Error("an AudioContext with a valid sampleRate is required");
  }
  if (actual !== compiled) {
    throw new Error(
      `processor was compiled for ${compiled} Hz but the AudioContext runs at ${actual} Hz; recompile for the actual context sample rate`,
    );
  }
}
