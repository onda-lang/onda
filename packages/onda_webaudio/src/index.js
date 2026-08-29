import {
  createParamControl,
  decodeDelegateRecords,
  formatPrintRecords,
  validateProcessorArtifact,
  validateProcessorModule,
} from "@onda-lang/processor-abi";

export {
  createParamDomain,
  createParamControl,
  constrainParamPlain,
  paramNormalizedToPlain,
  paramPlainToNormalized,
} from "@onda-lang/processor-abi";

export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";
export const ONDA_INIT_PRESERVE_PINNED = 0;
export const ONDA_INIT_FULL = 1;

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
  if (options.onPrint !== undefined && typeof options.onPrint !== "function") {
    throw new TypeError("initial print listener must be a function");
  }
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
      params: constrainInitialParamValues(
        metadata.metadata.params,
        options.params ?? {},
      ),
      buffers: options.buffers ?? {},
      eventPayloadCapacityBytes: options.eventPayloadCapacityBytes,
      delegateCapacityBytes: options.delegateCapacityBytes,
      printCapacityBytes: options.printCapacityBytes,
      printCollectionEnabled: options.onPrint !== undefined,
      printSubscriptionId: options.onPrint === undefined ? 0 : 1,
      initialize: options.initialize === true,
    },
  };
  if (outputChannels) {
    nodeOptions.outputChannelCount = [outputChannels];
  } else {
    delete nodeOptions.outputChannelCount;
  }
  return nodeOptions;
}

function paramInfoFor(paramInfo, selector) {
  const info = Number.isInteger(selector)
    ? paramInfo[selector]
    : paramInfo.find((candidate) => candidate.name === selector);
  if (!info) {
    throw new Error(`unknown Onda parameter '${String(selector)}'`);
  }
  return info;
}

function preparedParamControl(info, cache = null) {
  if (Number(info.array_len) !== 1) return null;
  if (info.scalar !== "bool" && info.param_control === null) return null;
  let control = cache?.get(info);
  if (!control) {
    control = createParamControl(info);
    cache?.set(info, control);
  }
  return control;
}

function constrainParamValue(info, value, cache = null) {
  return preparedParamControl(info, cache)?.constrainPlain(value) ?? value;
}

function constrainInitialParamValues(paramInfo, values) {
  if (Array.isArray(values)) {
    return values.map((value, index) => (
      value === undefined
        ? value
        : constrainParamValue(paramInfoFor(paramInfo, index), value)
    ));
  }
  if (values && typeof values === "object") {
    return Object.fromEntries(
      Object.entries(values).map(([name, value]) => [
        name,
        constrainParamValue(paramInfoFor(paramInfo, name), value),
      ]),
    );
  }
  throw new Error("params must be an array or object");
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
  return createOndaAudioProcessorImpl(context, artifact, options, false);
}

export async function createOndaAudioProcessorInitialized(
  context,
  artifact,
  options = {},
) {
  return createOndaAudioProcessorImpl(context, artifact, options, true);
}

async function createOndaAudioProcessorImpl(context, artifact, options, initialize) {
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
      { ...options, compiledModule, initialize },
      false,
    ),
  );
  return new OndaAudioProcessor(node, validated.metadata, options.onPrint);
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
  constructor(node, metadata = null, initialPrintListener = undefined) {
    if (
      initialPrintListener !== undefined
      && typeof initialPrintListener !== "function"
    ) {
      throw new TypeError("initial print listener must be a function");
    }
    this.node = node;
    this.metadata = metadata;
    this.paramInfo = metadata?.metadata?.params ?? null;
    this.paramControls = new WeakMap();
    this.nextRequestId = 1;
    this.pending = new Map();
    this.delegateListeners = new Set();
    this.delegateSubscriptionId = 0;
    this.printListeners = new Set(
      initialPrintListener === undefined ? [] : [initialPrintListener],
    );
    this.printSubscriptionId = initialPrintListener === undefined ? 0 : 1;
    this.pendingExecutionOutputs = new Map();
    this.closed = false;
    this.closeReason = null;
    this.handleMessage = (event) => {
      const message = event.data ?? {};
      if (message.type === "onda-delegate-records") {
        try {
          if (
            message.subscriptionId !== this.delegateSubscriptionId
            || this.delegateListeners.size === 0
          ) return;
          const records = decodeDelegateRecords(
            message.storage,
            message.usedBytes,
            this.metadata?.metadata?.delegates,
            this.metadata?.target?.byte_order,
          );
          if (records.length !== message.recordCount) {
            throw new Error("processor delegate count does not match packed storage");
          }
          const batch = {
            type: "onda-delegates",
            operation: message.operation,
            occurrences: records.map((record) => ({
              sequence: record.sequence,
              index: record.delegateIndex,
              name: record.name,
              values: record.values,
            })),
            overflowCount: message.overflowCount,
            transportDropCount: message.transportDropCount,
          };
          if (message.executionOutputId !== undefined) {
            this.queueExecutionOutput(message.executionOutputId, "delegate", batch);
            return;
          }
          for (const listener of this.delegateListeners) listener(batch);
        } finally {
          this.returnRecordStorage("delegate-ack", message.storage);
        }
        return;
      }
      if (message.type === "onda-print-records") {
        try {
          if (
            message.subscriptionId !== this.printSubscriptionId
            || this.printListeners.size === 0
          ) return;
          const formatted = formatPrintRecords(
            message.storage,
            message.usedBytes,
            this.metadata,
            message.overflowCount,
          );
          if (formatted.entries.length !== message.recordCount) {
            throw new Error("processor print count does not match packed storage");
          }
          const batch = {
            type: "onda-print",
            operation: message.operation,
            ...formatted,
            transportDropCount: message.transportDropCount,
          };
          if (message.executionOutputId !== undefined) {
            this.queueExecutionOutput(message.executionOutputId, "print", batch);
            return;
          }
          for (const listener of this.printListeners) listener(batch);
        } finally {
          this.returnRecordStorage("print-ack", message.storage);
        }
        return;
      }
      if (message.type === "onda-execution-output-end") {
        this.flushExecutionOutput(message.executionOutputId, message.operation);
        return;
      }
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

  returnRecordStorage(type, storage) {
    if (storage instanceof Uint8Array) {
      this.node.port.postMessage({ type, storage }, [storage.buffer]);
    } else {
      this.node.port.postMessage({ type });
    }
  }

  queueExecutionOutput(executionOutputId, kind, batch) {
    let pending = this.pendingExecutionOutputs.get(executionOutputId);
    if (!pending) {
      pending = { records: [], print: null, delegate: null };
      this.pendingExecutionOutputs.set(executionOutputId, pending);
    }
    if (kind === "print") {
      const lines = batch.text.match(/.*\n/g) ?? [];
      if (lines.length !== batch.entries.length) {
        throw new Error("formatted print output does not match its decoded entries");
      }
      pending.print = {
        overflowCount: batch.overflowCount,
        transportDropCount: batch.transportDropCount,
      };
      batch.entries.forEach((entry, index) => pending.records.push({
        kind,
        sequence: entry.sequence,
        entry,
        line: lines[index],
      }));
      return;
    }
    pending.delegate = {
      overflowCount: batch.overflowCount,
      transportDropCount: batch.transportDropCount,
    };
    batch.occurrences.forEach((occurrence) => pending.records.push({
      kind,
      sequence: occurrence.sequence,
      occurrence,
    }));
  }

  flushExecutionOutput(executionOutputId, operation) {
    const pending = this.pendingExecutionOutputs.get(executionOutputId);
    this.pendingExecutionOutputs.delete(executionOutputId);
    if (!pending) return;
    pending.records.sort((lhs, rhs) => lhs.sequence - rhs.sequence);
    let cursor = 0;
    while (cursor < pending.records.length) {
      const kind = pending.records[cursor].kind;
      let end = cursor + 1;
      while (end < pending.records.length && pending.records[end].kind === kind) end += 1;
      const records = pending.records.slice(cursor, end);
      if (kind === "print") {
        const meta = pending.print ?? { overflowCount: 0, transportDropCount: 0 };
        pending.print = null;
        const batch = {
          type: "onda-print",
          operation,
          text: records.map((record) => record.line).join(""),
          entries: records.map((record) => record.entry),
          ...meta,
        };
        for (const listener of this.printListeners) listener(batch);
      } else {
        const meta = pending.delegate ?? { overflowCount: 0, transportDropCount: 0 };
        pending.delegate = null;
        const batch = {
          type: "onda-delegates",
          operation,
          occurrences: records.map((record) => record.occurrence),
          ...meta,
        };
        for (const listener of this.delegateListeners) listener(batch);
      }
      cursor = end;
    }
    if (pending.print && (pending.print.overflowCount || pending.print.transportDropCount)) {
      const batch = {
        type: "onda-print",
        operation,
        text: "",
        entries: [],
        ...pending.print,
      };
      for (const listener of this.printListeners) listener(batch);
    }
    if (
      pending.delegate
      && (pending.delegate.overflowCount || pending.delegate.transportDropCount)
    ) {
      const batch = {
        type: "onda-delegates",
        operation,
        occurrences: [],
        ...pending.delegate,
      };
      for (const listener of this.delegateListeners) listener(batch);
    }
  }

  request(type, fields = {}, transfer = []) {
    if (this.closed) {
      return Promise.reject(this.closeReason);
    }
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
    try {
      if (!Array.isArray(this.paramInfo)) {
        return this.request("set-param", { param, value });
      }
      const info = paramInfoFor(this.paramInfo ?? [], param);
      return this.request("set-param", {
        param,
        value: constrainParamValue(info, value, this.paramControls),
      });
    } catch (error) {
      return Promise.reject(error);
    }
  }

  setParamNormalized(param, value) {
    try {
      if (!Array.isArray(this.paramInfo)) {
        throw new Error(
          "setParamNormalized requires processor metadata; construct the adapter with createOndaAudioProcessor()",
        );
      }
      const info = paramInfoFor(this.paramInfo, param);
      const control = preparedParamControl(info, this.paramControls);
      if (!control) {
        throw new Error(`Onda parameter '${info.name}' has no scalar host-control domain`);
      }
      return this.request("set-param", {
        param,
        value: control.normalizedToPlain(value),
      });
    } catch (error) {
      return Promise.reject(error);
    }
  }

  trigger(event, values = {}) {
    return this.request("event", { event, values });
  }

  onDelegates(listener) {
    this.assertOpen();
    if (typeof listener !== "function") {
      throw new TypeError("delegate listener must be a function");
    }
    const wasEmpty = this.delegateListeners.size === 0;
    this.delegateListeners.add(listener);
    if (wasEmpty) {
      this.delegateSubscriptionId = this.delegateSubscriptionId === Number.MAX_SAFE_INTEGER
        ? 1
        : this.delegateSubscriptionId + 1;
      try {
        this.node.port.postMessage({
          type: "delegate-subscription",
          enabled: true,
          subscriptionId: this.delegateSubscriptionId,
        });
      } catch (error) {
        this.delegateListeners.delete(listener);
        throw error;
      }
    }
    return () => {
      const removed = this.delegateListeners.delete(listener);
      if (removed && this.delegateListeners.size === 0) {
        this.node.port.postMessage({
          type: "delegate-subscription",
          enabled: false,
          subscriptionId: this.delegateSubscriptionId,
        });
      }
      return removed;
    };
  }

  onPrint(listener) {
    this.assertOpen();
    if (typeof listener !== "function") {
      throw new TypeError("print listener must be a function");
    }
    const wasEmpty = this.printListeners.size === 0;
    this.printListeners.add(listener);
    if (wasEmpty) {
      this.printSubscriptionId = this.printSubscriptionId === Number.MAX_SAFE_INTEGER
        ? 1
        : this.printSubscriptionId + 1;
      try {
        this.node.port.postMessage({
          type: "print-subscription",
          enabled: true,
          subscriptionId: this.printSubscriptionId,
        });
      } catch (error) {
        this.printListeners.delete(listener);
        throw error;
      }
    }
    return () => {
      const removed = this.printListeners.delete(listener);
      if (removed && this.printListeners.size === 0) {
        this.node.port.postMessage({
          type: "print-subscription",
          enabled: false,
          subscriptionId: this.printSubscriptionId,
        });
      }
      return removed;
    };
  }

  init(mode) {
    if (mode !== ONDA_INIT_PRESERVE_PINNED && mode !== ONDA_INIT_FULL) {
      return Promise.reject(new Error(`invalid Onda init mode '${String(mode)}'`));
    }
    return this.request("init", { mode });
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
    if (this.closed) return;
    this.closed = true;
    this.closeReason = reason instanceof Error ? reason : new Error(String(reason));
    if (this.delegateListeners.size !== 0) {
      try {
        this.node.port.postMessage({
          type: "delegate-subscription",
          enabled: false,
          subscriptionId: this.delegateSubscriptionId,
        });
      } catch {
        // The underlying node may already be gone; local closure still proceeds.
      }
    }
    if (this.printListeners.size !== 0) {
      try {
        this.node.port.postMessage({
          type: "print-subscription",
          enabled: false,
          subscriptionId: this.printSubscriptionId,
        });
      } catch {
        // The underlying node may already be gone; local closure still proceeds.
      }
    }
    this.node.port.removeEventListener("message", this.handleMessage);
    for (const pending of this.pending.values()) pending.reject(this.closeReason);
    this.pending.clear();
    this.pendingExecutionOutputs.clear();
    this.delegateListeners.clear();
    this.printListeners.clear();
  }

  assertOpen() {
    if (this.closed) throw this.closeReason;
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
