import {
  ONDA_RUNTIME_ERROR, RUNTIME_ERROR_MESSAGES, STATUS_OPERATIONS,
} from "./runtime-status.js";
import {
  createExecutionStatusRing, openExecutionStatusRing, drainExecutionStatusRing,
  STATUS_WAKE, wakeExecutionStatusRing, MAX_IN_FLIGHT_REQUESTS,
} from "./execution-status-ring.js";
export { ONDA_RUNTIME_ERROR } from "./runtime-status.js";

import { paramAddress, paramElementAddress, paramSmoothingSamples } from "./param-metadata.js";
import {
  PayloadPlan,
  createParamControl,
  paramElementMetadata,
  decodeDelegateRecords,
  formatPrintRecords,
  validateProcessorArtifact,
  validateProcessorModule,
} from "@onda-lang/processor-abi";
import {
  EXECUTION_OPERATION_EVENT,
  EXECUTION_OPERATION_INIT,
  EXECUTION_OPERATION_PROCESS,
  EXECUTION_OPERATION_TRANSPORT,
  EXECUTION_OUTPUT_RING_WAKE_INDEX,
  createExecutionOutputRing,
  drainExecutionOutputRing,
  openExecutionOutputRing,
} from "./execution-output-ring.js";
import {
  PROCESSOR_INIT_FULL,
  PROCESSOR_INIT_PRESERVE_PINNED,
} from "./processor-constants.js";

export {
  createParamDomain,
  createParamControl,
  constrainParamPlain,
  paramNormalizedToPlain,
  paramPlainToNormalized,
  PROCESSOR_EXECUTION_INPUT_REJECTED,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
} from "@onda-lang/processor-abi";

export const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";
export const ONDA_INIT_PRESERVE_PINNED = PROCESSOR_INIT_PRESERVE_PINNED;
export const ONDA_INIT_FULL = PROCESSOR_INIT_FULL;

export class OndaExecutionError extends Error {
  constructor(message, operation, status) {
    super(message);
    this.name = "OndaExecutionError";
    this.operation = operation;
    this.status = status;
  }
}

export class OndaRuntimeError extends Error {
  constructor(operation, code) {
    super(`${operation}: ${RUNTIME_ERROR_MESSAGES[code] ?? "unknown runtime error"}`);
    this.name = "OndaRuntimeError";
    this.operation = operation;
    this.code = code;
  }
}

const registrationByContext = new WeakMap();
const DEFAULT_DELEGATE_CAPACITY_BYTES = 64 * 1024;
const DEFAULT_PRINT_CAPACITY_BYTES = 64 * 1024;

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
  paramSmoothingSamples(options.paramSmoothingSeconds ?? 0, metadata.compile.sample_rate, metadata.compile.block_size);
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
  const delegateCapacityBytes = configuredExecutionOutputCapacity(
    metadata.metadata?.delegates,
    options.delegateCapacityBytes,
    DEFAULT_DELEGATE_CAPACITY_BYTES,
    "delegate",
  );
  const printCapacityBytes = configuredExecutionOutputCapacity(
    metadata.metadata?.log_sites,
    options.printCapacityBytes,
    DEFAULT_PRINT_CAPACITY_BYTES,
    "print",
  );
  const executionOutputRing = createExecutionOutputRing(
    delegateCapacityBytes,
    printCapacityBytes,
  );
  if (options.onPrint !== undefined && executionOutputRing === null) {
    throw new Error(
      "print delivery requires SharedArrayBuffer; enable cross-origin isolation",
    );
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
      paramSmoothingSeconds: options.paramSmoothingSeconds ?? 0,
      params: constrainInitialParamValues(
        metadata.metadata.params,
        options.params ?? {},
      ),
      buffers: options.buffers ?? {},
      eventPayloadCapacityBytes: options.eventPayloadCapacityBytes,
      delegateCapacityBytes,
      printCapacityBytes,
      executionOutputRing,
      executionStatusRing: createExecutionStatusRing(),
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

function configuredExecutionOutputCapacity(
  descriptors,
  configured,
  defaultCapacity,
  name,
) {
  const capacity = configured
    ?? (Array.isArray(descriptors) && descriptors.length ? defaultCapacity : 0);
  if (!Number.isSafeInteger(capacity) || capacity < 0 || capacity > 0x7fff_ffff) {
    throw new Error(
      `${name} capacity must be an integer from 0 through 2147483647 bytes`,
    );
  }
  return capacity;
}

function paramElementInfo(info, element, cache = null) {
  if (!cache) return paramElementMetadata(info, element);
  let elements = cache.get(info);
  if (!elements) {
    elements = new Map();
    cache.set(info, elements);
  }
  if (!elements.has(element)) elements.set(element, paramElementMetadata(info, element));
  return elements.get(element);
}

function paramInfoFor(params, selector, cache = null) {
  const { info, element } = paramAddress(params, selector);
  return element === null ? info : paramElementInfo(info, element, cache);
}

function paramElementInfoFor(params, selector, element, cache = null) {
  const address = paramElementAddress(params, selector, element);
  return paramElementInfo(address.info, address.element, cache);
}

function preparedParamControl(info, cache = null) {
  if (info.type_repr !== info.scalar) return null;
  if (info.scalar !== "bool" && info.param_control === null) return null;
  let control = cache?.get(info);
  if (!control) {
    control = createParamControl(info);
    cache?.set(info, control);
  }
  return control;
}

function constrainParamValue(info, value, cache = null, elements = null) {
  if (info.type_repr !== info.scalar) {
    if ((!Array.isArray(value) && !ArrayBuffer.isView(value)) || value.length !== info.array_len) {
      throw new Error(`Onda parameter '${info.name}' requires exactly ${info.array_len} values`);
    }
    return Array.from(value, (item, index) => constrainParamValue(
      paramElementInfo(info, index, elements), item, cache, elements,
    ));
  }
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
  const nodeOptions = audioWorkletNodeOptionsFromValidated(
    validated,
    { ...options, compiledModule, initialize: false },
    false,
  );
  const node = new NodeConstructor(
    context,
    ONDA_AUDIO_WORKLET_PROCESSOR_NAME,
    nodeOptions,
  );
  const processor = new OndaAudioProcessor(
    node,
    validated.metadata,
    options.onPrint,
    nodeOptions.processorOptions.executionOutputRing,
    nodeOptions.processorOptions.executionStatusRing,
  );
  if (!initialize) return processor;
  try {
    await processor.init(ONDA_INIT_FULL);
    return processor;
  } catch (error) {
    // Initialization publishes diagnostic prints before reporting its status. Consume any
    // already-published records while the construction-time listener is still installed.
    try {
      processor.drainExecutionOutput();
    } catch {
      // Preserve the initialization failure if consuming its diagnostics also fails.
    }
    processor.close(error);
    try {
      node.disconnect?.();
    } catch {
      // Preserve the initialization failure if node teardown also fails.
    }
    throw error;
  }
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
  constructor(
    node,
    metadata = null,
    initialPrintListener = undefined,
    executionOutputRing = null,
    executionStatusRing = null,
  ) {
    if (
      initialPrintListener !== undefined
      && typeof initialPrintListener !== "function"
    ) {
      throw new TypeError("initial print listener must be a function");
    }
    this.node = node;
    this.metadata = metadata;
    this.paramInfo = metadata?.metadata?.params ?? null;
    this.eventPlans = new WeakMap();
    this.paramControls = new WeakMap();
    this.paramElements = new WeakMap();
    this.executionStatusRing = executionStatusRing === null
      ? null : openExecutionStatusRing(executionStatusRing);
    this.statusPoll = null;
    this.statusDrainActive = false;
    this.statusNeedsWake = this.executionStatusRing !== null
      && typeof Atomics.waitAsync !== "function";
    this.requestQueue = [];
    this.inFlightRequests = new Set();
    this.sendingRequests = false;
    this.requestPumpScheduled = false;
    this.nextRequestId = 1;
    this.pending = new Map();
    this.executionErrorListeners = new Set();
    this.delegateListeners = new Set();
    this.delegateSubscriptionId = 0;
    this.printListeners = new Set(
      initialPrintListener === undefined ? [] : [initialPrintListener],
    );
    this.printSubscriptionId = initialPrintListener === undefined ? 0 : 1;
    this.executionOutputRing = executionOutputRing === null
      ? null
      : openExecutionOutputRing(executionOutputRing);
    if (initialPrintListener !== undefined && !this.executionOutputRing) {
      throw new Error(
        "print delivery requires SharedArrayBuffer; enable cross-origin isolation",
      );
    }
    this.executionOutputWaitGeneration = 0;
    this.executionOutputPoll = null;
    this.executionOutputDrainActive = false;
    this.executionOutputReading = false;
    this.closed = false;
    this.closeReason = null;
    this.handleMessage = (event) => {
      const message = event.data ?? {};
      if (message.type === "onda-status-ready") {
        this.drainExecutionStatus();
        return;
      }
      if (message.type === "onda-status") {
        this.consumeExecutionStatus(message.requestId, message.operation, message.status);
        return;
      }
      const pending = this.pending.get(message.requestId);
      if (!pending) return;
      this.pending.delete(message.requestId);
      pending.resolve(message);
      this.finishRequest(message.requestId);
    };
    this.handleProcessorError = (event) => {
      const error = event.error instanceof Error
        ? event.error
        : new Error(event.message || "Onda AudioWorklet processor failed");
      this.notifyListeners(this.executionErrorListeners, error);
      this.close(error);
    };
    node.addEventListener("processorerror", this.handleProcessorError);
    node.port.addEventListener("message", this.handleMessage);
    node.port.start?.();
    if (this.printListeners.size !== 0) this.startExecutionOutputDrain();
    this.startExecutionStatusDrain();
  }

  startExecutionStatusDrain() {
    const ring = this.executionStatusRing;
    if (!ring) return;
    if (typeof Atomics.waitAsync !== "function") {
      this.statusPoll = setInterval(() => this.drainExecutionStatus(), 16);
      this.statusPoll.unref?.();
      return;
    }
    const wait = async () => {
      while (!this.closed) {
        const expected = Atomics.load(ring, STATUS_WAKE);
        this.drainExecutionStatus();
        if (this.closed) return;
        const result = Atomics.waitAsync(ring, STATUS_WAKE, expected);
        if (result.async) await result.value;
      }
    };
    void wait().catch((error) => this.close(error));
  }

  drainExecutionStatus() {
    if (!this.executionStatusRing || this.closed || this.statusDrainActive) return 0;
    this.statusDrainActive = true;
    try {
      return drainExecutionStatusRing(this.executionStatusRing,
        (id, operation, status) => this.consumeExecutionStatus(id, operation, status));
    } finally {
      this.statusDrainActive = false;
      if (this.requestQueue.length !== 0) this.scheduleRequests();
    }
  }

  consumeExecutionStatus(requestId, operation, status) {
    // Preserve diagnostic delivery before resolving/rejecting its execution call.
    this.drainExecutionOutput();
    const name = STATUS_OPERATIONS[operation] ?? "unknown";
    const error = status < 0
      ? new OndaRuntimeError(name, status)
      : status > 0
        ? new OndaExecutionError(
          `${name} failed with Onda execution status ${status}`, name, status)
        : null;
    if (status === ONDA_RUNTIME_ERROR.TRANSPORT_OVERFLOW) {
      this.notifyListeners(this.executionErrorListeners, error);
      this.close(error);
      return;
    }
    if (!requestId) {
      if (error) this.notifyListeners(this.executionErrorListeners, error);
      return;
    }
    const pending = this.pending.get(requestId);
    if (!pending) return;
    this.pending.delete(requestId);
    if (error) pending.reject(error);
    else pending.resolve({ type: "onda-ok", operation: name, requestId });
    this.finishRequest(requestId);
  }

  finishRequest(requestId) {
    if (this.inFlightRequests.delete(requestId)) this.scheduleRequests();
  }

  scheduleRequests() {
    if (this.closed || this.requestPumpScheduled) return;
    this.requestPumpScheduled = true;
    queueMicrotask(() => {
      this.requestPumpScheduled = false;
      this.sendQueuedRequests();
    });
  }

  sendQueuedRequests() {
    if (this.closed || this.sendingRequests || this.statusDrainActive
      || this.requestQueue.length === 0
      || this.inFlightRequests.size >= MAX_IN_FLIGHT_REQUESTS) return;
    this.sendingRequests = true;
    try {
      // Consume asynchronous failures before reusing credits, including when a
      // data reply on the port (rather than a ring status) released the credit.
      this.drainExecutionStatus();
      while (!this.closed && this.requestQueue.length !== 0
        && this.inFlightRequests.size < MAX_IN_FLIGHT_REQUESTS) {
        const { message, transfer } = this.requestQueue.shift();
        this.inFlightRequests.add(message.requestId);
        try {
          this.node.port.postMessage(message, transfer);
        } catch (error) {
          this.pending.get(message.requestId)?.reject(error);
          this.pending.delete(message.requestId);
          this.finishRequest(message.requestId);
        }
      }
    } finally {
      this.sendingRequests = false;
    }
  }

  startExecutionOutputDrain() {
    if (!this.executionOutputRing || this.executionOutputDrainActive) return;
    this.executionOutputDrainActive = true;
    this.drainExecutionOutput();
    if (typeof Atomics.waitAsync !== "function") {
      this.executionOutputPoll = setInterval(() => {
        if (!this.closed) this.drainExecutionOutput();
      }, 16);
      return;
    }
    const generation = ++this.executionOutputWaitGeneration;
    const wait = async () => {
      while (!this.closed && generation === this.executionOutputWaitGeneration) {
        const expected = Atomics.load(
          this.executionOutputRing.control,
          EXECUTION_OUTPUT_RING_WAKE_INDEX,
        );
        this.drainExecutionOutput();
        if (this.closed || generation !== this.executionOutputWaitGeneration) return;
        const result = Atomics.waitAsync(
          this.executionOutputRing.control,
          EXECUTION_OUTPUT_RING_WAKE_INDEX,
          expected,
        );
        if (result.async) await result.value;
      }
    };
    void wait().catch((error) => this.reportListenerError(error));
  }

  stopExecutionOutputDrain() {
    if (!this.executionOutputDrainActive) return;
    this.executionOutputDrainActive = false;
    this.executionOutputWaitGeneration += 1;
    if (this.executionOutputPoll !== null) {
      clearInterval(this.executionOutputPoll);
      this.executionOutputPoll = null;
    }
    if (!this.executionOutputRing) return;
    Atomics.add(
      this.executionOutputRing.control,
      EXECUTION_OUTPUT_RING_WAKE_INDEX,
      1,
    );
    Atomics.notify(
      this.executionOutputRing.control,
      EXECUTION_OUTPUT_RING_WAKE_INDEX,
    );
  }

  reportListenerError(error) {
    queueMicrotask(() => {
      throw error;
    });
  }

  notifyListeners(listeners, value) {
    for (const listener of listeners) {
      try {
        listener(value);
      } catch (error) {
        this.reportListenerError(error);
      }
    }
  }

  drainExecutionOutput() {
    if (!this.executionOutputRing || this.executionOutputReading) return 0;
    this.executionOutputReading = true;
    try {
      return drainExecutionOutputRing(this.executionOutputRing, (entry) => {
        try {
          this.consumeExecutionOutput(entry);
        } catch (error) {
          this.reportListenerError(error);
        }
      });
    } finally {
      this.executionOutputReading = false;
    }
  }

  consumeExecutionOutput(entry) {
    const operation = this.executionOperationName(
      entry.operation,
      entry.operationIndex,
    );
    const records = [];
    let print = null;
    let delegate = null;
    if (
      entry.printSubscriptionId === this.printSubscriptionId
      && this.printListeners.size !== 0
      && (
        entry.printUsed
        || entry.printRecordCount
        || entry.printOverflowCount
        || entry.printTransportDropCount
      )
    ) {
      const formatted = formatPrintRecords(
        entry.printStorage,
        entry.printUsed,
        this.metadata,
        entry.printOverflowCount,
      );
      if (formatted.entries.length !== entry.printRecordCount) {
        throw new Error("processor print count does not match packed storage");
      }
      const lines = formatted.text.match(/.*\n/g) ?? [];
      if (lines.length !== formatted.entries.length) {
        throw new Error("formatted print output does not match its decoded entries");
      }
      print = {
        overflowCount: formatted.overflowCount,
        transportDropCount: entry.printTransportDropCount,
      };
      formatted.entries.forEach((decoded, index) => records.push({
        kind: "print",
        sequence: decoded.sequence,
        entry: decoded,
        line: lines[index],
      }));
    }
    if (
      entry.delegateSubscriptionId === this.delegateSubscriptionId
      && this.delegateListeners.size !== 0
      && (
        entry.delegateUsed
        || entry.delegateRecordCount
        || entry.delegateOverflowCount
        || entry.delegateTransportDropCount
      )
    ) {
      const decodedRecords = decodeDelegateRecords(
        entry.delegateStorage,
        entry.delegateUsed,
        this.metadata?.metadata?.delegates,
        this.metadata?.target?.byte_order,
      );
      if (decodedRecords.length !== entry.delegateRecordCount) {
        throw new Error("processor delegate count does not match packed storage");
      }
      delegate = {
        overflowCount: entry.delegateOverflowCount,
        transportDropCount: entry.delegateTransportDropCount,
      };
      decodedRecords.forEach((record) => {
        const occurrence = {
          sequence: record.sequence,
          index: record.delegateIndex,
          name: record.name,
          values: record.values,
        };
        records.push({
          kind: "delegate",
          sequence: occurrence.sequence,
          occurrence,
        });
      });
    }
    this.deliverExecutionOutput(records, print, delegate, operation);
  }

  executionOperationName(operation, operationIndex) {
    if (operation === EXECUTION_OPERATION_INIT) return "processor init";
    if (operation === EXECUTION_OPERATION_PROCESS) return "process";
    if (operation === EXECUTION_OPERATION_TRANSPORT) return "transport";
    if (operation === EXECUTION_OPERATION_EVENT) {
      const event = this.metadata?.metadata?.events?.[operationIndex];
      if (!event) throw new Error("execution-output ring references an unknown event");
      return `event '${event.name}'`;
    }
    throw new Error("execution-output ring contains an unknown operation");
  }

  deliverExecutionOutput(records, print, delegate, operation) {
    records.sort((lhs, rhs) => lhs.sequence - rhs.sequence);
    let cursor = 0;
    while (cursor < records.length) {
      const kind = records[cursor].kind;
      let end = cursor + 1;
      while (end < records.length && records[end].kind === kind) end += 1;
      const batchRecords = records.slice(cursor, end);
      if (kind === "print") {
        const meta = print ?? { overflowCount: 0, transportDropCount: 0 };
        print = null;
        const batch = {
          type: "onda-print",
          operation,
          text: batchRecords.map((record) => record.line).join(""),
          entries: batchRecords.map((record) => record.entry),
          ...meta,
        };
        this.notifyListeners(this.printListeners, batch);
      } else {
        const meta = delegate ?? { overflowCount: 0, transportDropCount: 0 };
        delegate = null;
        const batch = {
          type: "onda-delegates",
          operation,
          occurrences: batchRecords.map((record) => record.occurrence),
          ...meta,
        };
        this.notifyListeners(this.delegateListeners, batch);
      }
      cursor = end;
    }
    if (print && (print.overflowCount || print.transportDropCount)) {
      const batch = {
        type: "onda-print",
        operation,
        text: "",
        entries: [],
        ...print,
      };
      this.notifyListeners(this.printListeners, batch);
    }
    if (
      delegate
      && (delegate.overflowCount || delegate.transportDropCount)
    ) {
      const batch = {
        type: "onda-delegates",
        operation,
        occurrences: [],
        ...delegate,
      };
      this.notifyListeners(this.delegateListeners, batch);
    }
  }

  request(type, fields = {}, transfer = []) {
    if (this.closed) {
      return Promise.reject(this.closeReason);
    }
    // IDs travel as u32; never reuse an ID that still belongs to queued work.
    let requestId;
    do {
      requestId = this.nextRequestId;
      this.nextRequestId = (this.nextRequestId + 1) >>> 0 || 1;
    } while (this.pending.has(requestId));
    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
      this.requestQueue.push({
        message: { ...fields, type, requestId, notifyStatus: this.statusNeedsWake }, transfer,
      });
      this.sendQueuedRequests();
    });
  }

  setParam(param, value) {
    try {
      if (!Array.isArray(this.paramInfo)) {
        return this.request("set-param", { param, value });
      }
      const info = paramInfoFor(this.paramInfo ?? [], param, this.paramElements);
      return this.request("set-param", {
        param,
        value: constrainParamValue(info, value, this.paramControls, this.paramElements),
      });
    } catch (error) {
      return Promise.reject(error);
    }
  }

  setParamSmoothingSeconds(seconds) {
    try {
      paramSmoothingSamples(seconds, this.metadata?.compile?.sample_rate ?? 1,
        this.metadata?.compile?.block_size ?? 0);
      return this.request("set-param-smoothing", { seconds });
    } catch (error) {
      return Promise.reject(error);
    }
  }

  resetParams() {
    return this.request("reset-params");
  }

  setParamElement(param, element, value) {
    try {
      if (!Array.isArray(this.paramInfo)) {
        throw new Error(
          "setParamElement requires processor metadata; construct the adapter with createOndaAudioProcessor()",
        );
      }
      const info = paramElementInfoFor(
        this.paramInfo,
        param,
        element,
        this.paramElements,
      );
      return this.request("set-param", {
        param,
        element,
        value: constrainParamValue(info, value, this.paramControls, this.paramElements),
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
      const info = paramInfoFor(this.paramInfo, param, this.paramElements);
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

  setParamElementNormalized(param, element, value) {
    try {
      if (!Array.isArray(this.paramInfo)) {
        throw new Error(
          "setParamElementNormalized requires processor metadata; construct the adapter with createOndaAudioProcessor()",
        );
      }
      const info = paramElementInfoFor(
        this.paramInfo,
        param,
        element,
        this.paramElements,
      );
      const control = preparedParamControl(info, this.paramControls);
      if (!control) {
        throw new Error(`Onda parameter '${info.name}' has no scalar host-control domain`);
      }
      return this.request("set-param", {
        param,
        element,
        value: control.normalizedToPlain(value),
      });
    } catch (error) {
      return Promise.reject(error);
    }
  }

  trigger(event, values = {}) {
    try {
      this.assertOpen();
      const events = this.metadata?.metadata?.events;
      if (Number.isInteger(event)) {
        if (!Array.isArray(events)) {
          throw new Error(
            "event triggers require processor metadata; construct the adapter with createOndaAudioProcessor()",
          );
        }
        if (event >= events.length) {
          const payload = new Uint8Array();
          return this.request("event", { event, payload }, [payload.buffer]);
        }
      }
      const info = Number.isInteger(event)
        ? events?.[event]
        : events?.find((entry) => entry.name === event);
      if (!info) throw new Error(`unknown Onda event '${String(event)}'`);
      let plan = this.eventPlans.get(info);
      if (!plan) { plan = new PayloadPlan(info.schema); this.eventPlans.set(info, plan); }
      const payload = plan.encode(values);
      return this.request("event", { event, payload }, [payload.buffer]);
    } catch (error) { return Promise.reject(error); }
  }

  onExecutionError(listener) {
    this.assertOpen();
    if (typeof listener !== "function") {
      throw new TypeError("execution-error listener must be a function");
    }
    this.executionErrorListeners.add(listener);
    return () => this.executionErrorListeners.delete(listener);
  }

  onDelegates(listener) {
    this.assertOpen();
    if (typeof listener !== "function") {
      throw new TypeError("delegate listener must be a function");
    }
    this.requireExecutionOutputRing("delegate");
    const wasEmpty = this.delegateListeners.size === 0;
    this.delegateListeners.add(listener);
    if (wasEmpty) {
      this.delegateSubscriptionId = (this.delegateSubscriptionId + 1) >>> 0;
      if (this.delegateSubscriptionId === 0) this.delegateSubscriptionId = 1;
      try {
        this.startExecutionOutputDrain();
        this.node.port.postMessage({
          type: "delegate-subscription",
          enabled: true,
          subscriptionId: this.delegateSubscriptionId,
        });
      } catch (error) {
        this.delegateListeners.delete(listener);
        if (this.printListeners.size === 0) this.stopExecutionOutputDrain();
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
        if (this.printListeners.size === 0) this.stopExecutionOutputDrain();
      }
      return removed;
    };
  }

  onPrint(listener) {
    this.assertOpen();
    if (typeof listener !== "function") {
      throw new TypeError("print listener must be a function");
    }
    this.requireExecutionOutputRing("print");
    const wasEmpty = this.printListeners.size === 0;
    this.printListeners.add(listener);
    if (wasEmpty) {
      this.printSubscriptionId = (this.printSubscriptionId + 1) >>> 0;
      if (this.printSubscriptionId === 0) this.printSubscriptionId = 1;
      try {
        this.startExecutionOutputDrain();
        this.node.port.postMessage({
          type: "print-subscription",
          enabled: true,
          subscriptionId: this.printSubscriptionId,
        });
      } catch (error) {
        this.printListeners.delete(listener);
        if (this.delegateListeners.size === 0) this.stopExecutionOutputDrain();
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
        if (this.delegateListeners.size === 0) this.stopExecutionOutputDrain();
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
    try {
      this.assertOpen();
      if (!(snapshot instanceof Uint8Array) && !(snapshot instanceof ArrayBuffer)) {
        throw new OndaRuntimeError("restore-snapshot", ONDA_RUNTIME_ERROR.INVALID_SNAPSHOT);
      }
      const bytes = snapshot instanceof Uint8Array
        ? snapshot.slice()
        : new Uint8Array(snapshot.slice(0));
      return this.request("restore-snapshot", { snapshot: bytes }, [bytes.buffer]);
    } catch (error) {
      return Promise.reject(error);
    }
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
    this.node.removeEventListener("processorerror", this.handleProcessorError);
    this.stopExecutionOutputDrain();
    if (this.statusPoll !== null) clearInterval(this.statusPoll);
    if (this.executionStatusRing) wakeExecutionStatusRing(this.executionStatusRing);
    this.requestQueue.length = 0;
    this.inFlightRequests.clear();
    for (const pending of this.pending.values()) pending.reject(this.closeReason);
    this.pending.clear();
    this.executionErrorListeners.clear();
    this.delegateListeners.clear();
    this.printListeners.clear();
  }

  assertOpen() {
    if (this.closed) throw this.closeReason;
  }

  requireExecutionOutputRing(kind) {
    if (this.executionOutputRing) return;
    throw new Error(
      `${kind} delivery requires SharedArrayBuffer; enable cross-origin isolation`,
    );
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
