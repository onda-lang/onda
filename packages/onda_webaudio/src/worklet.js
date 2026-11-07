import { ONDA_RUNTIME_ERROR as ERROR, STATUS_OPERATIONS, failureStatus } from "./runtime-status.js";
import { openExecutionStatusRing, writeExecutionStatus } from "./execution-status-ring.js";
import { paramSmoothingSamples, ParamSmoothing } from "./param-metadata.js";
import {
  EXECUTION_OPERATION_EVENT,
  EXECUTION_OPERATION_INIT,
  EXECUTION_OPERATION_PROCESS,
  EXECUTION_OPERATION_TRANSPORT,
  openExecutionOutputRing,
  writeExecutionOutputRing,
} from "./execution-output-ring.js";
import {
  PROCESSOR_BEGIN_BLOCK,
  PROCESSOR_END_BLOCK,
  PROCESSOR_EXECUTION_OK,
  PROCESSOR_EXECUTION_INPUT_REJECTED,
  PROCESSOR_INIT_FULL,
  PROCESSOR_INIT_PRESERVE_PINNED,
} from "./processor-constants.js";
const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";
const DEFAULT_EVENT_PAYLOAD_CAPACITY_BYTES = 64 * 1024;
const DEFAULT_DELEGATE_CAPACITY_BYTES = 64 * 1024;
const DEFAULT_PRINT_CAPACITY_BYTES = 64 * 1024;

const DELEGATE_BATCH_SIZE_BYTES = 20;
const PRINT_BATCH_SIZE_BYTES = 20;
const EXECUTION_OUTPUT_SIZE_BYTES = 12;
const HOST_LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

const DECIMAL_I64 = /^-?[0-9]+$/;

// Match Onda's float-to-integer casts at the host audio boundary.
function audioI32(value) {
  if (value >= 2147483647) return 2147483647;
  if (value <= -2147483648) return -2147483648;
  return Math.trunc(value) || 0;
}

function audioI64(value) {
  if (value >= 2 ** 63) return 9223372036854775807n;
  if (value <= -(2 ** 63)) return -9223372036854775808n;
  return Number.isNaN(value) ? 0n : BigInt(Math.trunc(value));
}

function exactI64(value) {
  let integer;
  if (typeof value === "bigint") {
    integer = value;
  } else if (typeof value === "number" && Number.isSafeInteger(value)) {
    integer = BigInt(value);
  } else if (typeof value === "string" && DECIMAL_I64.test(value)) {
    integer = BigInt(value);
  } else {
    throw ERROR.INVALID_PARAMETER;
  }
  if (BigInt.asIntN(64, integer) !== integer) {
    throw ERROR.INVALID_PARAMETER;
  }
  return integer;
}

class OndaWasmProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();

    const processorOptions = options.processorOptions ?? {};
    const wasmBytes = processorOptions.wasmBytes;
    const wasmModule = processorOptions.wasmModule;
    const metadata = processorOptions.metadata ?? {};

    if (
      metadata.artifact_kind !== "webassembly_module"
      || metadata.target?.pointer_model !== "linear_memory_offset"
      || metadata.target?.pointer_width_bits !== 32
    ) {
      throw new Error("expected an Onda wasm32 executable-module artifact");
    }

    let module;
    if (wasmModule !== undefined) {
      if (!(wasmModule instanceof WebAssembly.Module)) {
        throw new Error("processorOptions.wasmModule must be a WebAssembly.Module");
      }
      module = wasmModule;
    } else {
      const bytes =
        wasmBytes instanceof Uint8Array ? wasmBytes : new Uint8Array(wasmBytes);
      module = new WebAssembly.Module(bytes);
    }
    const instance = new WebAssembly.Instance(module);
    this.exports = instance.exports;
    this.memory = this.exports.memory;

    if (!(this.memory instanceof WebAssembly.Memory)) {
      throw new Error("expected exported wasm memory");
    }

    this.memoryBuffer = null;
    this.dataViewCache = null;
    this.viewsReady = false;
    this.allocationLocked = false;
    this.stateBytes = null;
    this.inputViews = [];
    this.outputViews = [];
    this.delegateRecordView = null;
    this.printRecordView = null;

    const heapBase = Number(
      this.exports.__heap_base?.value ?? this.exports.__heap_base ?? 0,
    );
    if (
      !Number.isInteger(heapBase)
      || heapBase < -0x8000_0000
      || heapBase > 0xffff_ffff
    ) {
      throw new Error(`invalid wasm32 heap base: ${heapBase}`);
    }
    this.heap = heapBase >>> 0;
    this.stateSizeBytes = Number(metadata.runtime?.state_size_bytes ?? 0);
    this.paramInfo = Array.isArray(metadata.metadata?.params) ? metadata.metadata.params : [];
    this.eventInfo = Array.isArray(metadata.metadata?.events)
      ? metadata.metadata.events
      : [];
    this.delegateInfo = Array.isArray(metadata.metadata?.delegates)
      ? metadata.metadata.delegates
      : [];
    this.logSiteInfo = Array.isArray(metadata.metadata?.log_sites)
      ? metadata.metadata.log_sites
      : [];
    this.controlOutputInfo = Array.isArray(metadata.metadata?.control_outputs)
      ? metadata.metadata.control_outputs
      : [];
    this.bufferInfo = Array.isArray(metadata.metadata?.buffers)
      ? metadata.metadata.buffers
      : [];
    this.bufferArrayInfo = Array.isArray(metadata.metadata?.buffer_arrays)
      ? metadata.metadata.buffer_arrays
      : [];
    this.inputInfo = Array.isArray(metadata.metadata?.inputs)
      ? metadata.metadata.inputs
      : [];
    this.outputInfo = Array.isArray(metadata.metadata?.outputs)
      ? metadata.metadata.outputs
      : [];
    this.snapshotInfo = Array.isArray(metadata.metadata?.states)
      ? metadata.metadata.states
      : [];
    this.snapshotSizeBytes = Number(metadata.runtime?.snapshot_size_bytes ?? 0);
    this.inputChannels = this.flattenAudioChannels(this.inputInfo, "input");
    this.outputChannels = this.flattenAudioChannels(this.outputInfo, "output");
    this.inputCount = this.inputChannels.length;
    this.outputCount = this.outputChannels.length;
    if (this.inputCount === 0 && this.outputCount === 0) {
      throw new Error(
        "the Onda Web Audio processor requires at least one audio input or output",
      );
    }
    this.blockSize = Number(metadata.compile?.block_size ?? 512);
    this.compileSampleRate = Number(metadata.compile?.sample_rate ?? sampleRate);
    if (!Number.isInteger(this.blockSize) || this.blockSize <= 0) {
      throw new Error(`invalid compile-time block size: ${this.blockSize}`);
    }
    if (!Number.isFinite(this.compileSampleRate) || this.compileSampleRate <= 0) {
      throw new Error(`invalid compile-time sample rate: ${this.compileSampleRate}`);
    }
    const renderSampleRate = Number(globalThis.sampleRate);
    if (
      Number.isFinite(renderSampleRate)
      && renderSampleRate > 0
      && renderSampleRate !== this.compileSampleRate
    ) {
      throw new Error(
        `processor was compiled for ${this.compileSampleRate} Hz but the AudioWorklet runs at ${renderSampleRate} Hz`,
      );
    }
    this.inputPtrs = [];
    this.inputCapacityFrames = 0;
    this.outputPtrs = [];
    this.outputCapacityFrames = 0;
    this.blockCursor = 0;

    const paramBytes = Number(metadata.runtime?.param_size_bytes ?? 0);
    if (!Number.isInteger(paramBytes) || paramBytes < 0) {
      throw new Error(`invalid parameter storage size: ${paramBytes}`);
    }
    this.paramSizeBytes = paramBytes;

    this.paramsPtr = paramBytes ? this.alloc(paramBytes, 4) : 0;
    this.statePtr = this.stateSizeBytes ? this.alloc(this.stateSizeBytes, 16) : 0;
    this.inPtrsPtr = this.inputCount ? this.alloc(this.inputCount * 4, 4) : 0;
    this.outPtrsPtr = this.outputCount ? this.alloc(this.outputCount * 4, 4) : 0;
    const bufferTableBytes = this.bufferInfo.length * 4;
    this.bufferPointersPtr = this.bufferInfo.length
      ? this.alloc(bufferTableBytes, 4)
      : 0;
    this.bufferFramesPtr = this.bufferInfo.length
      ? this.alloc(bufferTableBytes, 4)
      : 0;
    this.bufferChannelsPtr = this.bufferInfo.length
      ? this.alloc(bufferTableBytes, 4)
      : 0;
    this.bufferSampleRatesPtr = this.bufferInfo.length
      ? this.alloc(bufferTableBytes, 4)
      : 0;
    this.bufferBindings = [];
    this.bindInitialBuffers(processorOptions.buffers ?? {});
    this.eventPayloadCapacity = this.configuredEventPayloadCapacity(
      processorOptions.eventPayloadCapacityBytes,
    );
    this.eventPayloadPtr = this.eventPayloadCapacity
      ? this.alloc(this.eventPayloadCapacity, 8)
      : 0;
    // Each tensor and dynamic prefix adds at most seven alignment bytes.
    // Reserve before rendering; dispatch never grows memory for preparation.
    this.eventWorkspaceCapacity = this.eventPayloadCapacity + 7 * Math.max(0,
      ...this.eventInfo.map((event) => event.params.length * 2));
    this.eventWorkspacePtr = this.eventWorkspaceCapacity ? this.alloc(this.eventWorkspaceCapacity, 8) : 0;
    this.eventInputPtr = this.eventInfo.length ? this.alloc(16, 4) : 0;
    this.delegateCapacity = this.configuredDelegateCapacity(
      processorOptions.delegateCapacityBytes,
    );
    this.delegateStoragePtr = this.delegateInfo.length && this.delegateCapacity
      ? this.alloc(this.delegateCapacity, 8)
      : 0;
    this.delegateBatchPtr = this.delegateInfo.length
      ? this.alloc(DELEGATE_BATCH_SIZE_BYTES, 4)
      : 0;
    if (this.delegateBatchPtr) {
      const view = new DataView(this.memory.buffer);
      view.setUint32(this.delegateBatchPtr, this.delegateStoragePtr, true);
      view.setUint32(this.delegateBatchPtr + 4, this.delegateCapacity, true);
      view.setUint32(this.delegateBatchPtr + 8, 0, true);
      view.setUint32(this.delegateBatchPtr + 12, 0, true);
      view.setUint32(this.delegateBatchPtr + 16, 0, true);
    }
    this.printCapacity = this.configuredPrintCapacity(
      processorOptions.printCapacityBytes,
    );
    this.printStoragePtr = this.logSiteInfo.length && this.printCapacity
      ? this.alloc(this.printCapacity, 8)
      : 0;
    this.printBatchPtr = this.logSiteInfo.length
      ? this.alloc(PRINT_BATCH_SIZE_BYTES, 4)
      : 0;
    if (this.printBatchPtr) {
      const view = new DataView(this.memory.buffer);
      view.setUint32(this.printBatchPtr, this.printStoragePtr, true);
      view.setUint32(this.printBatchPtr + 4, this.printCapacity, true);
      view.setUint32(this.printBatchPtr + 8, 0, true);
      view.setUint32(this.printBatchPtr + 12, 0, true);
      view.setUint32(this.printBatchPtr + 16, 0, true);
    }
    this.printCollectionEnabled = processorOptions.printCollectionEnabled === true
      && this.printStoragePtr !== 0;
    this.printSubscriptionId = Number.isSafeInteger(processorOptions.printSubscriptionId)
      ? processorOptions.printSubscriptionId
      : 0;
    this.executionOutputPtr = this.delegateBatchPtr || this.printBatchPtr
      ? this.alloc(EXECUTION_OUTPUT_SIZE_BYTES, 4)
      : 0;
    if (this.executionOutputPtr) {
      const view = new DataView(this.memory.buffer);
      view.setUint32(this.executionOutputPtr, 0, true);
      view.setUint32(
        this.executionOutputPtr + 4,
        this.printCollectionEnabled ? this.printBatchPtr : 0,
        true,
      );
      view.setUint32(this.executionOutputPtr + 8, 0, true);
    }
    this.delegateCollectionEnabled = false;
    this.delegateSubscriptionId = 0;
    this.executionOutputRing = processorOptions.executionOutputRing === undefined
      || processorOptions.executionOutputRing === null
      ? null
      : openExecutionOutputRing(processorOptions.executionOutputRing);
    if (
      this.executionOutputRing
      && (
        this.executionOutputRing.delegateCapacity !== this.delegateCapacity
        || this.executionOutputRing.printCapacity !== this.printCapacity
      )
    ) {
      throw new Error("execution-output ring capacities do not match processor storage");
    }
    this.pendingDelegateDrops = 0;
    this.pendingDelegateOverflow = 0;
    this.pendingPrintDrops = 0;
    this.pendingPrintOverflow = 0;
    this.executionOutputEntry = {
      operation: 0,
      operationIndex: 0,
      delegateSubscriptionId: 0,
      delegateUsed: 0,
      delegateRecordCount: 0,
      delegateOverflowCount: 0,
      delegateTransportDropCount: 0,
      printSubscriptionId: 0,
      printUsed: 0,
      printRecordCount: 0,
      printOverflowCount: 0,
      printTransportDropCount: 0,
    };
    this.executionStatusRing = processorOptions.executionStatusRing == null
      ? null : openExecutionStatusRing(processorOptions.executionStatusRing);
    this.statusMessage = { type: "onda-status", requestId: 0, operation: 0, status: 0 };
    this.statusReadyMessage = { type: "onda-status-ready" };
    this.transportFailed = false;
    this.eventIds = new Map(this.eventInfo.map((event, index) => [event.name, index]));
    this.bufferIds = new Map(this.bufferInfo.map((buffer, index) => [buffer.name, index]));
    this.paramAddresses = new Map();
    this.arrayParamAddresses = [];
    this.paramInfo.forEach((info, index) => {
      const address = { info };
      this.paramAddresses.set(index, address);
      this.paramAddresses.set(info.name, address);
      if (this.isFixedArray(info)) {
        this.arrayParamAddresses.push({ info, prefix: `${info.name}[` });
      }
    });
    this.writeParamDefaults();
    this.paramDefaults = new Uint8Array(this.memory.buffer, this.paramsPtr, this.paramSizeBytes).slice();
    this.writeInitialParams(processorOptions.params ?? {});
    this.paramSmoothing = new ParamSmoothing(
      this.paramInfo,
      paramSmoothingSamples(processorOptions.paramSmoothingSeconds ?? 0, this.compileSampleRate, this.blockSize),
      this.memoryView(),
      this.paramsPtr,
    );
    this.ensureInputCapacity(this.blockSize);
    this.ensureOutputCapacity(this.blockSize);
    this.viewsReady = true;
    this.refreshMemoryCache(true);
    this.invalidateState();
    if (processorOptions.initialize === true) {
      this.init(PROCESSOR_INIT_FULL);
    }
    this.allocationLocked = true;
    this.port.onmessage = (event) => this.handleMessage(event.data ?? {});
  }

  ensureMemoryCapacity(requiredBytes) {
    const pageBytes = 64 * 1024;
    const current = this.memory.buffer.byteLength;
    if (requiredBytes <= current) {
      return;
    }
    const extraPages = Math.ceil((requiredBytes - current) / pageBytes);
    this.memory.grow(extraPages);
  }

  alignUp(value, align) {
    return Math.ceil(value / align) * align;
  }

  alloc(size, align) {
    if (this.allocationLocked) {
      throw new Error("Onda worklet memory allocation is locked after construction");
    }
    const ptr = this.alignUp(this.heap, align);
    const next = ptr + size;
    this.ensureMemoryCapacity(next);
    this.heap = next;
    return ptr;
  }

  refreshMemoryCache(force = false) {
    const buffer = this.memory.buffer;
    if (!force && buffer === this.memoryBuffer) return;
    this.memoryBuffer = buffer;
    this.dataViewCache = new DataView(buffer);
    if (!this.viewsReady) return;

    this.paramBytes = new Uint8Array(buffer, this.paramsPtr, this.paramSizeBytes);
    this.stateBytes = new Uint8Array(
      buffer,
      this.statePtr,
      this.stateSizeBytes,
    );
    this.inputViews = this.inputChannels.map((channel, index) => {
      channel.pointer = this.inputPtrs[index];
      return this.scalarView(
        channel.pointer,
        channel.scalar,
        this.inputCapacityFrames,
      );
    });
    this.outputViews = this.outputChannels.map((channel, index) => {
      channel.pointer = this.outputPtrs[index];
      return this.scalarView(
        channel.pointer,
        channel.scalar,
        this.outputCapacityFrames,
      );
    });
    this.eventPayloadView = new Uint8Array(buffer, this.eventPayloadPtr, this.eventPayloadCapacity);
    this.delegateRecordView = this.delegateStoragePtr
      ? new Uint8Array(buffer, this.delegateStoragePtr, this.delegateCapacity)
      : null;
    this.printRecordView = this.printStoragePtr
      ? new Uint8Array(buffer, this.printStoragePtr, this.printCapacity)
      : null;
  }

  memoryView() {
    this.refreshMemoryCache();
    return this.dataViewCache;
  }

  scalarView(address, scalar, length) {
    if (!HOST_LITTLE_ENDIAN) return null;
    if (address % this.scalarByteSize(scalar) !== 0) return null;
    if (scalar === "bool") return new Uint8Array(this.memoryBuffer, address, length);
    if (scalar === "i32") return new Int32Array(this.memoryBuffer, address, length);
    if (scalar === "i64") return new BigInt64Array(this.memoryBuffer, address, length);
    if (scalar === "f32") return new Float32Array(this.memoryBuffer, address, length);
    if (scalar === "f64") return new Float64Array(this.memoryBuffer, address, length);
    throw ERROR.INVALID_STORAGE;
  }

  configuredEventPayloadCapacity(configuredCapacity) {
    let minimum = 0;
    let dynamic = false;
    for (const event of this.eventInfo) {
      minimum = Math.max(
        minimum,
        Number(event.payload_size_bytes ?? event.payload_min_size_bytes ?? 0),
      );
      dynamic ||= event.has_dynamic_payload === true;
    }
    const capacity = configuredCapacity
      ?? (dynamic ? Math.max(minimum, DEFAULT_EVENT_PAYLOAD_CAPACITY_BYTES) : minimum);
    if (
      !Number.isSafeInteger(capacity)
      || capacity < minimum
      || capacity < 0
      || capacity > 0x7fff_ffff
    ) {
      throw new Error(
        `event payload capacity must be an integer from ${minimum} through 2147483647 bytes`,
      );
    }
    return capacity;
  }

  configuredDelegateCapacity(configuredCapacity) {
    const capacity = configuredCapacity
      ?? (this.delegateInfo.length ? DEFAULT_DELEGATE_CAPACITY_BYTES : 0);
    if (
      !Number.isSafeInteger(capacity)
      || capacity < 0
      || capacity > 0x7fff_ffff
    ) {
      throw new Error(
        "delegate capacity must be an integer from 0 through 2147483647 bytes",
      );
    }
    return capacity;
  }

  configuredPrintCapacity(configuredCapacity) {
    const capacity = configuredCapacity
      ?? (this.logSiteInfo.length ? DEFAULT_PRINT_CAPACITY_BYTES : 0);
    if (!Number.isSafeInteger(capacity) || capacity < 0 || capacity > 0x7fff_ffff) {
      throw new Error(
        "print capacity must be an integer from 0 through 2147483647 bytes",
      );
    }
    return capacity;
  }

  flattenAudioChannels(ports, kind) {
    const channels = [];
    for (const port of ports) {
      const channelOffset = Number(port.slot_offset);
      const channelCount = Number(port.array_len);
      const scalar = port.scalar ?? "f32";
      const elementSize = this.scalarByteSize(scalar);
      if (
        !Number.isInteger(channelOffset) ||
        channelOffset !== channels.length ||
        !Number.isInteger(channelCount) ||
        channelCount <= 0
      ) {
        throw new Error(`invalid flattened Onda ${kind} channel metadata`);
      }
      if (
        port.element_size_bytes !== undefined &&
        Number(port.element_size_bytes) !== elementSize
      ) {
        throw new Error(
          `Onda ${kind} '${port.name}' has inconsistent scalar byte width`,
        );
      }
      for (let channel = 0; channel < channelCount; channel += 1) {
        channels.push({
          name: port.name,
          scalar,
          elementSize,
        });
      }
    }
    return channels;
  }

  writeInitialParams(values) {
    if (Array.isArray(values)) {
      values.forEach((value, index) => this.setParam(index, value));
      return;
    }
    if (values && typeof values === "object") {
      for (const [name, value] of Object.entries(values)) {
        this.setParam(name, value);
      }
      return;
    }
    throw new Error("processorOptions.params must be an array or object");
  }

  writeParamDefaults() {
    if (this.paramDefaults) {
      this.refreshMemoryCache();
      this.paramBytes.set(this.paramDefaults);
      return;
    }
    for (const param of this.paramInfo) {
      const value = this.metadataDefaultValue(param);
      if (value !== undefined) {
        this.writeStorage(
          this.paramsPtr + Number(param.byte_offset),
          param,
          value,
        );
      }
    }
  }

  setParam(selector, value, explicitElement = null) {
    let param = this.paramAddresses.get(selector)?.info;
    let element = explicitElement;
    if (!param && typeof selector === "string" && selector.endsWith("]") && explicitElement === null) {
      for (const address of this.arrayParamAddresses) {
        if (!selector.startsWith(address.prefix)) continue;
        const start = address.prefix.length;
        const end = selector.length - 1;
        if (start === end || (selector[start] === "0" && end - start > 1)) break;
        element = 0;
        for (let index = start; index < end; index += 1) {
          const digit = selector.charCodeAt(index) - 48;
          if (digit < 0 || digit > 9) throw ERROR.INVALID_PARAMETER;
          element = element * 10 + digit;
        }
        param = address.info;
        break;
      }
    }
    if (!param || value === undefined) throw ERROR.INVALID_PARAMETER;
    if (element !== null && (!Number.isSafeInteger(element) || element < 0 || element >= param.array_len)) {
      throw ERROR.INVALID_PARAMETER;
    }
    // Validate every element before storing any: rejection must not partially
    // update parameters or smoothing targets (notably an invalid i64 array tail).
    if (element === null) this.validateStorageValue(param, value);
    if (element !== null || !this.isFixedArray(param)) {
      this.validateScalarValue(param.scalar, value);
    } else {
      for (let index = 0; index < param.array_len; index += 1) {
        this.validateScalarValue(param.scalar, value[index]);
      }
    }
    const offset = Number(param.byte_offset);
    const byteSize = Number(param.byte_size);
    const length = Number(param.array_len);
    const expectedByteSize = length * this.scalarByteSize(param.scalar);
    if (
      !Number.isInteger(offset) ||
      offset < 0 ||
      !Number.isInteger(byteSize) ||
      !Number.isInteger(length) ||
      length <= 0 ||
      byteSize !== expectedByteSize ||
      offset + byteSize > this.paramSizeBytes
    ) {
      throw ERROR.INVALID_PARAMETER;
    }
    if (element !== null) {
      this.setParamScalar(param, element, value);
      return;
    }
    if (!this.paramSmoothing?.isEnabledFor(param)) {
      this.writeStorage(this.paramsPtr + offset, param, value);
      return;
    }
    this.validateStorageValue(param, value);
    if (this.isFixedArray(param)) {
      for (let index = 0; index < length; index += 1) {
        this.setParamScalar(param, index, value[index]);
      }
    } else {
      this.setParamScalar(param, 0, value);
    }
  }

  setParamScalar(param, element, value) {
    if (this.paramSmoothing?.setTarget(param, element, value, this.initialized)) return;
    this.writeScalar(this.paramsPtr + Number(param.byte_offset) + element * this.scalarByteSize(param.scalar), param.scalar, value);
  }

  init(mode) {
    if (mode !== PROCESSOR_INIT_PRESERVE_PINNED && mode !== PROCESSOR_INIT_FULL) {
      throw ERROR.INVALID_INIT_MODE;
    }
    if (mode === PROCESSOR_INIT_PRESERVE_PINNED && !this.initialized) {
      throw ERROR.UNINITIALIZED;
    }
    this.runInitialization(mode);
  }

  invalidateState() {
    this.initialized = false;
    this.process = this.processPending;
    this.blockCursor = 0;
  }

  commitInitializedState(blockCursor) {
    this.initialized = true;
    this.process = this.processInitialized;
    this.blockCursor = blockCursor;
  }

  runInitialization(mode, afterInitialize, collectOutput = true) {
    if (this.transportFailed) throw ERROR.TRANSPORT_OVERFLOW;
    // Reinitializing state does not create a compile-block boundary. Retain
    // the host-side position so the next process segment cannot synthesize an
    // extra BEGIN_BLOCK or postpone the matching END_BLOCK.
    const blockCursor = this.initialized ? this.blockCursor : 0;
    // Generated initialization mutates the live image in place. Stop exposing
    // it before entering Wasm so a failure, including one in snapshot overlay,
    // leaves the processor on the silent pending path.
    this.invalidateState();
    this.refreshMemoryCache();
    if (collectOutput) this.prepareExecutionOutput();
    this.paramSmoothing.settle(this.memoryView(), this.paramsPtr);
    const status = this.exports.onda_processor_init(
      this.paramsPtr,
      this.statePtr,
      mode,
      this.bufferPointersPtr,
      this.bufferFramesPtr,
      this.bufferChannelsPtr,
      this.bufferSampleRatesPtr,
      collectOutput ? this.executionOutputPtr : 0,
    );
    // Generated failures retain diagnostic prints but clear delegates.
    if (collectOutput) {
      this.publishExecutionOutput(
        EXECUTION_OPERATION_INIT,
        0,
      );
    }
    this.checkExecutionStatus(status);
    afterInitialize?.();
    this.commitInitializedState(blockCursor);
  }

  requireInitialized() {
    if (!this.initialized) {
      throw ERROR.UNINITIALIZED;
    }
  }

  checkExecutionStatus(status, inputRejectionAllowed = false) {
    if (status === PROCESSOR_EXECUTION_OK) return;
    if (!inputRejectionAllowed || status !== PROCESSOR_EXECUTION_INPUT_REJECTED) this.invalidateState();
    throw status;
  }

  createSnapshot() {
    this.requireInitialized();
    const snapshot = new Uint8Array(this.snapshotSizeBytes);
    this.refreshMemoryCache();
    const state = this.stateBytes;
    for (const entry of this.snapshotInfo) {
      const packedOffset = Number(entry.packed_snapshot_byte_offset);
      const physicalOffset = Number(entry.physical_state_byte_offset);
      const byteSize = Number(entry.byte_size);
      this.validateSnapshotEntry(entry, packedOffset, physicalOffset, byteSize);
      snapshot.set(
        state.subarray(physicalOffset, physicalOffset + byteSize),
        packedOffset,
      );
    }
    return snapshot;
  }

  restoreSnapshot(value) {
    const snapshot = this.snapshotBytes(value);
    // The ABI restore base is a fresh post-init image, so scratch and
    // control-mirror state never leak across a restore. Initialization and
    // overlay form one lifecycle transition: neither partial result is ready.
    this.runInitialization(PROCESSOR_INIT_FULL, () => {
      const state = this.stateBytes;
      for (const entry of this.snapshotInfo) {
        const packedOffset = Number(entry.packed_snapshot_byte_offset);
        const physicalOffset = Number(entry.physical_state_byte_offset);
        const byteSize = Number(entry.byte_size);
        this.validateSnapshotEntry(entry, packedOffset, physicalOffset, byteSize);
        state.set(
          snapshot.subarray(packedOffset, packedOffset + byteSize),
          physicalOffset,
        );
        this.normalizeSnapshotIntegerRange(entry, state, physicalOffset, byteSize);
      }
    }, false);
  }

  normalizeSnapshotIntegerRange(entry, state, offset, byteSize) {
    const range = entry.integer_range;
    if (range === null || range === undefined) return;
    if (entry.scalar !== "i32" && entry.scalar !== "i64") {
      throw ERROR.INVALID_STORAGE;
    }
    const mode = range.mode;
    if (mode !== "clamp" && mode !== "wrap") {
      throw ERROR.INVALID_STORAGE;
    }
    const view = new DataView(state.buffer, state.byteOffset + offset, byteSize);
    if (entry.scalar === "i32") {
      const min = Number(range.min?.value);
      const max = Number(range.max?.value);
      if (!Number.isInteger(min) || !Number.isInteger(max) || min > max || byteSize !== entry.array_len * 4) {
        throw ERROR.INVALID_STORAGE;
      }
      for (let element = 0; element < byteSize; element += 4) {
        const value = view.getInt32(element, true);
        const normalized = mode === "clamp"
          ? Math.min(Math.max(value, min), max)
          : min + (((value - min) % (max - min + 1)) + (max - min + 1)) % (max - min + 1);
        view.setInt32(element, normalized, true);
      }
      return;
    }
    let min;
    let max;
    try {
      min = BigInt(range.min?.value);
      max = BigInt(range.max?.value);
    } catch {
      throw ERROR.INVALID_STORAGE;
    }
    if (min > max || byteSize !== entry.array_len * 8) {
      throw ERROR.INVALID_STORAGE;
    }
    const width = max - min + 1n;
    for (let element = 0; element < byteSize; element += 8) {
      const value = view.getBigInt64(element, true);
      const normalized = mode === "clamp"
        ? (value < min ? min : (value > max ? max : value))
        : min + ((value - min) % width + width) % width;
      view.setBigInt64(element, normalized, true);
    }
  }

  snapshotBytes(value) {
    if (!(value instanceof ArrayBuffer) && !ArrayBuffer.isView(value)) throw ERROR.INVALID_SNAPSHOT;
    if (value.byteLength !== this.snapshotSizeBytes) throw ERROR.INVALID_SNAPSHOT;
    if (value instanceof Uint8Array) return value;
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }

  validateSnapshotEntry(entry, packedOffset, physicalOffset, byteSize) {
    if (
      !Number.isSafeInteger(packedOffset)
      || packedOffset < 0
      || !Number.isSafeInteger(physicalOffset)
      || physicalOffset < 0
      || !Number.isSafeInteger(byteSize)
      || byteSize < 0
      || packedOffset + byteSize > this.snapshotSizeBytes
      || physicalOffset + byteSize > this.stateSizeBytes
    ) {
      throw ERROR.INVALID_STORAGE;
    }
  }

  postResponse(message, response, always = false, transfer = []) {
    if (!always && message.requestId === undefined) return;
    this.port.postMessage(
      message.requestId === undefined
        ? response
        : { ...response, requestId: message.requestId },
      transfer,
    );
  }

  handleMessage(message) {
    if (this.transportFailed) return;
    try {
      if (message.type === "set-param") {
        this.setParam(
          message.param ?? message.name ?? message.index,
          message.value,
          message.element ?? null,
        );
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "set-param-smoothing") {
        const seconds = message.seconds;
        const count = Math.ceil(seconds * this.compileSampleRate);
        if (!Number.isFinite(seconds) || seconds < 0 || !Number.isSafeInteger(count)) {
          throw ERROR.INVALID_SMOOTHING;
        }
        const samples = count > this.blockSize ? count : 0;
        this.paramSmoothing.setDuration(samples, this.memoryView(), this.paramsPtr);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "reset-params") {
        this.writeParamDefaults();
        this.paramSmoothing.reset(this.memoryView(), this.paramsPtr);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "init") {
        this.init(message.mode);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "event") {
        this.dispatchEvent(
          message.event ?? message.name ?? message.index,
          message.payload,
        );
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "read-control-outputs") {
        this.postResponse(message, {
          type: "control-outputs",
          values: this.readControlOutputs(),
        }, true);
      } else if (message.type === "read-buffer") {
        this.postResponse(
          message,
          { type: "buffer", ...this.readBuffer(message.buffer) },
          true,
        );
      } else if (message.type === "snapshot") {
        const snapshot = this.createSnapshot();
        this.postResponse(
          message,
          { type: "snapshot", bytes: snapshot },
          true,
          [snapshot.buffer],
        );
      } else if (message.type === "restore-snapshot") {
        this.restoreSnapshot(message.snapshot ?? message.bytes);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "delegate-subscription") {
        this.setDelegateSubscription(message.enabled, message.subscriptionId);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else if (message.type === "print-subscription") {
        this.setPrintSubscription(message.enabled, message.subscriptionId);
        this.publishStatus(message.requestId, STATUS_OPERATIONS.indexOf(message.type), 0);
      } else {
        throw ERROR.UNKNOWN_OPERATION;
      }
    } catch (error) {
      const status = failureStatus(error);
      if (status === ERROR.UNEXPECTED_FAILURE || status === ERROR.INVALID_STORAGE || status === ERROR.INVALID_OUTPUT) {
        this.invalidateState();
      }
      this.publishStatus(message.requestId, Math.max(0, STATUS_OPERATIONS.indexOf(message.type)), status);
    } finally {
      // Control-path wakeup for hosts without Atomics.waitAsync. Render failures
      // still only write shared storage and are picked up by the polling reader.
      if (this.executionStatusRing && message.notifyStatus) {
        this.port.postMessage(this.statusReadyMessage);
      }
    }
  }

  publishStatus(requestId, operation, status) {
    if (requestId === undefined && status === 0) return;
    if (this.executionStatusRing) {
      if (!writeExecutionStatus(this.executionStatusRing, requestId ?? 0, operation, status)) {
        this.transportFailed = true;
        this.invalidateState();
      }
      return;
    }
    // Reuse a prepared packet. Structured cloning by MessagePort can still
    // allocate; browsers without shared memory have no allocation-free transport.
    const message = this.statusMessage;
    message.requestId = requestId ?? 0;
    message.operation = operation;
    message.status = status;
    this.port.postMessage(message);
  }

  dispatchEvent(selector, payload) {
    this.requireInitialized();
    if (Number.isInteger(selector) && selector >= this.eventInfo.length) return;
    const eventId = Number.isInteger(selector)
      ? selector
      : this.eventIds.get(selector);
    const event = this.eventInfo[eventId];
    if (!event) {
      throw ERROR.UNKNOWN_EVENT;
    }
    if (!(payload instanceof Uint8Array)) throw ERROR.INVALID_PAYLOAD;
    const payloadSize = payload.byteLength;
    if (payloadSize > this.eventPayloadCapacity) {
      throw ERROR.PAYLOAD_CAPACITY;
    }
    const view = this.memoryView();
    this.eventPayloadView.set(payload);
    const handler = this.exports[event.export];
    if (typeof handler !== "function") {
      throw ERROR.INVALID_STORAGE;
    }
    view.setUint32(this.eventInputPtr, this.eventPayloadPtr, true);
    view.setUint32(this.eventInputPtr + 4, payloadSize, true);
    view.setUint32(this.eventInputPtr + 8, this.eventWorkspacePtr, true);
    view.setUint32(this.eventInputPtr + 12, this.eventWorkspaceCapacity, true);
    const status = handler(
      this.eventInputPtr,
      this.paramsPtr,
      this.statePtr,
      this.bufferPointersPtr,
      this.bufferFramesPtr,
      this.bufferChannelsPtr,
      this.bufferSampleRatesPtr,
      this.executionOutputPtr,
    );
    this.publishExecutionOutput(
      EXECUTION_OPERATION_EVENT,
      eventId,
    );
    this.checkExecutionStatus(status, true);
  }

  readControlOutputs() {
    this.requireInitialized();
    return Object.fromEntries(
      this.controlOutputInfo.map((output) => [
        output.name,
        this.readStorage(
          this.statePtr + Number(output.state_byte_offset),
          output,
        ),
      ]),
    );
  }

  saturatedAdd(lhs, rhs) {
    return Math.min(0xffff_ffff, lhs + rhs);
  }

  publishExecutionOutput(operation, operationIndex) {
    if (!this.executionOutputRing) return;
    if (
      !this.delegateCollectionEnabled
      && !this.printCollectionEnabled
      && !this.pendingDelegateDrops
      && !this.pendingDelegateOverflow
      && !this.pendingPrintDrops
      && !this.pendingPrintOverflow
    ) return;
    const view = this.memoryView();
    const delegateUsed = this.delegateCollectionEnabled && this.delegateBatchPtr
      ? view.getUint32(this.delegateBatchPtr + 8, true)
      : 0;
    const delegateRecordCount = this.delegateCollectionEnabled && this.delegateBatchPtr
      ? view.getUint32(this.delegateBatchPtr + 12, true)
      : 0;
    const delegateOverflowCount = this.delegateCollectionEnabled && this.delegateBatchPtr
      ? view.getUint32(this.delegateBatchPtr + 16, true)
      : 0;
    const printUsed = this.printCollectionEnabled && this.printBatchPtr
      ? view.getUint32(this.printBatchPtr + 8, true)
      : 0;
    const printRecordCount = this.printCollectionEnabled && this.printBatchPtr
      ? view.getUint32(this.printBatchPtr + 12, true)
      : 0;
    const printOverflowCount = this.printCollectionEnabled && this.printBatchPtr
      ? view.getUint32(this.printBatchPtr + 16, true)
      : 0;
    if (delegateUsed > this.delegateCapacity) {
      throw ERROR.INVALID_OUTPUT;
    }
    if (printUsed > this.printCapacity) {
      throw ERROR.INVALID_OUTPUT;
    }
    const hasCurrent = delegateUsed
      || delegateRecordCount
      || delegateOverflowCount
      || printUsed
      || printRecordCount
      || printOverflowCount;
    const hasPending = this.pendingDelegateDrops
      || this.pendingDelegateOverflow
      || this.pendingPrintDrops
      || this.pendingPrintOverflow;
    if (!hasCurrent && !hasPending) return;

    const entry = this.executionOutputEntry;
    entry.operation = hasCurrent ? operation : EXECUTION_OPERATION_TRANSPORT;
    entry.operationIndex = hasCurrent ? operationIndex : 0;
    entry.delegateSubscriptionId = this.delegateSubscriptionId;
    entry.delegateUsed = delegateUsed;
    entry.delegateRecordCount = delegateRecordCount;
    entry.delegateOverflowCount = this.saturatedAdd(
      this.pendingDelegateOverflow,
      delegateOverflowCount,
    );
    entry.delegateTransportDropCount = this.pendingDelegateDrops;
    entry.printSubscriptionId = this.printSubscriptionId;
    entry.printUsed = printUsed;
    entry.printRecordCount = printRecordCount;
    entry.printOverflowCount = this.saturatedAdd(
      this.pendingPrintOverflow,
      printOverflowCount,
    );
    entry.printTransportDropCount = this.pendingPrintDrops;
    if (writeExecutionOutputRing(
      this.executionOutputRing,
      entry,
      this.delegateRecordView,
      this.printRecordView,
    )) {
      this.pendingDelegateDrops = 0;
      this.pendingDelegateOverflow = 0;
      this.pendingPrintDrops = 0;
      this.pendingPrintOverflow = 0;
      return;
    }
    this.pendingDelegateDrops = this.saturatedAdd(
      this.pendingDelegateDrops,
      delegateRecordCount,
    );
    this.pendingDelegateOverflow = this.saturatedAdd(
      this.pendingDelegateOverflow,
      delegateOverflowCount,
    );
    this.pendingPrintDrops = this.saturatedAdd(
      this.pendingPrintDrops,
      printRecordCount,
    );
    this.pendingPrintOverflow = this.saturatedAdd(
      this.pendingPrintOverflow,
      printOverflowCount,
    );
  }

  setDelegateSubscription(enabled, subscriptionId) {
    this.delegateCollectionEnabled = enabled === true && this.delegateStoragePtr !== 0;
    this.delegateSubscriptionId = Number.isSafeInteger(subscriptionId)
      ? subscriptionId
      : 0;
    this.pendingDelegateDrops = 0;
    this.pendingDelegateOverflow = 0;
    if (this.delegateBatchPtr) {
      const view = this.memoryView();
      this.resetRecordBatch(view, this.delegateBatchPtr);
      view.setUint32(
        this.executionOutputPtr,
        this.delegateCollectionEnabled ? this.delegateBatchPtr : 0,
        true,
      );
    }
  }

  setPrintSubscription(enabled, subscriptionId) {
    this.printCollectionEnabled = enabled === true && this.printStoragePtr !== 0;
    this.printSubscriptionId = Number.isSafeInteger(subscriptionId)
      ? subscriptionId
      : 0;
    this.pendingPrintDrops = 0;
    this.pendingPrintOverflow = 0;
    if (this.printBatchPtr) {
      const view = this.memoryView();
      this.resetRecordBatch(view, this.printBatchPtr);
      view.setUint32(
        this.executionOutputPtr + 4,
        this.printCollectionEnabled ? this.printBatchPtr : 0,
        true,
      );
    }
  }

  bindInitialBuffers(options) {
    this.bufferInfo.forEach((buffer, bufferId) => {
      const supplied = this.initialBufferOption(options, buffer, bufferId);
      const declaredChannels = Number(buffer.static_channels ?? 0);
      const fallbackChannels = declaredChannels || 1;
      if (supplied === undefined || supplied === null) {
        const sampleRate = Math.fround(this.compileSampleRate);
        this.writeBufferDescriptor(bufferId, 0, 1, fallbackChannels, sampleRate);
        this.bufferBindings[bufferId] = {
          ...buffer,
          pointer: 0,
          frames: 1,
          channels: fallbackChannels,
          sampleRate,
          bound: false,
        };
        return;
      }
      const descriptor =
        Array.isArray(supplied) || ArrayBuffer.isView(supplied)
          ? { data: supplied }
          : supplied;
      const data = descriptor?.data;
      const length = this.sequenceLength(data, `Onda buffer '${buffer.name}'`);
      const sampleRate = Math.fround(
        Number(
          descriptor.sampleRate ?? descriptor.sample_rate ?? this.compileSampleRate,
        ),
      );
      if (!Number.isFinite(sampleRate) || sampleRate <= 0) {
        throw new Error(`Onda buffer '${buffer.name}' has an invalid sample rate`);
      }
      if (length === 0) {
        throw new Error(
          `Onda buffer '${buffer.name}' requires non-empty bound data`,
        );
      }
      const channels = Number(descriptor.channels ?? declaredChannels);
      if (!Number.isInteger(channels) || channels <= 0) {
        throw new Error(`Onda buffer '${buffer.name}' requires channels > 0`);
      }
      if (declaredChannels && channels !== declaredChannels) {
        throw new Error(
          `Onda buffer '${buffer.name}' requires ${declaredChannels} channel(s)`,
        );
      }
      const frames = Number(descriptor.frames ?? length / channels);
      if (
        !Number.isInteger(frames)
        || frames <= 0
        || frames * channels !== length
      ) {
        throw new Error(
          `Onda buffer '${buffer.name}' data does not match its frame/channel shape`,
        );
      }
      const elementSize = this.scalarByteSize(buffer.scalar);
      const pointer = this.alloc(length * elementSize, elementSize);
      this.writeScalarValues(pointer, buffer.scalar, data, length);
      this.writeBufferDescriptor(bufferId, pointer, frames, channels, sampleRate);
      this.bufferBindings[bufferId] = {
        ...buffer,
        pointer,
        frames,
        channels,
        sampleRate,
        bound: true,
      };
    });
  }

  writeBufferDescriptor(bufferId, pointer, frames, channels, sampleRate) {
    const view = this.memoryView();
    view.setUint32(this.bufferPointersPtr + bufferId * 4, pointer, true);
    view.setInt32(this.bufferFramesPtr + bufferId * 4, frames, true);
    view.setInt32(this.bufferChannelsPtr + bufferId * 4, channels, true);
    view.setFloat32(this.bufferSampleRatesPtr + bufferId * 4, sampleRate, true);
  }

  initialBufferOption(options, buffer, bufferId) {
    if (Array.isArray(options)) {
      return options[bufferId];
    }
    if (!options || typeof options !== "object") {
      throw new Error("processorOptions.buffers must be an array or object");
    }
    if (Object.prototype.hasOwnProperty.call(options, buffer.name)) {
      return options[buffer.name];
    }
    const group = this.bufferArrayInfo.find((candidate) => {
      const first = Number(candidate.first_buffer);
      const len = Number(candidate.len);
      return bufferId >= first && bufferId < first + len;
    });
    if (!group || !Object.prototype.hasOwnProperty.call(options, group.name)) {
      return undefined;
    }
    const supplied = options[group.name];
    if (supplied === undefined || supplied === null) {
      return undefined;
    }
    if (!Array.isArray(supplied)) {
      throw new Error(`Onda buffer array '${group.name}' must be an array`);
    }
    return supplied[bufferId - Number(group.first_buffer)];
  }

  readBuffer(selector) {
    const bufferId = Number.isInteger(selector)
      ? selector
      : this.bufferIds.get(selector);
    const binding = this.bufferBindings[bufferId];
    if (!binding) {
      throw ERROR.UNKNOWN_BUFFER;
    }
    const length = binding.frames * binding.channels;
    const elementSize = this.scalarByteSize(binding.scalar);
    const view = this.memoryView();
    return {
      name: binding.name,
      frames: binding.frames,
      channels: binding.channels,
      sampleRate: binding.sampleRate,
      data: Array.from({ length }, (_, index) =>
        this.readScalar(
          binding.pointer + index * elementSize,
          binding.scalar,
          view,
        ),
      ),
    };
  }

  constantValue(constant) {
    if (constant?.kind === "scalar") {
      return this.decodeConstantScalar(constant.data);
    }
    if (constant?.kind === "aggregate" && Array.isArray(constant.data)) {
      return constant.data.map((value) => this.constantValue(value));
    }
    return undefined;
  }

  metadataDefaultValue(info) {
    if (!Array.isArray(info?.default_reprs)) return undefined;
    const values = info.default_reprs.map((value) =>
      this.decodeConstantScalar({ type: info.scalar, value })
    );
    return this.isFixedArray(info) ? values : values[0];
  }

  isFixedArray(info) {
    return info?.is_array
      ?? (info?.is_slice !== true && /\[[0-9]+\]$/.test(info?.type_repr ?? ""));
  }

  decodeConstantScalar(scalar) {
    const type = scalar?.type;
    const value = scalar?.value;
    if (typeof value !== "string") return value;
    if (type === "bool") {
      if (value === "true") return true;
      if (value === "false") return false;
      throw new Error(`invalid bool constant '${value}'`);
    }
    if (type === "i32") return Number(value);
    if (type === "i64") return BigInt(value);
    if (type !== "f32" && type !== "f64") return value;
    if (!value.startsWith("0x")) {
      const decoded = Number(value);
      if (Number.isNaN(decoded) && value !== "NaN") {
        throw new Error(`invalid ${type} constant '${value}'`);
      }
      return decoded;
    }
    const width = type === "f32" ? 32 : 64;
    const digits = value.startsWith("0x") ? value.slice(2) : "";
    if (digits.length !== width / 4 || !/^[0-9a-f]+$/.test(digits)) {
      throw new Error(`invalid MIR ${type} bit-pattern constant '${String(value)}'`);
    }
    const bytes = new ArrayBuffer(width / 8);
    const view = new DataView(bytes);
    if (width === 32) {
      view.setUint32(0, Number.parseInt(digits, 16), false);
      return view.getFloat32(0, false);
    }
    view.setBigUint64(0, BigInt(value), false);
    return view.getFloat64(0, false);
  }

  sequenceLength(value, description) {
    const valid = Array.isArray(value)
      || (ArrayBuffer.isView(value) && typeof value.length === "number");
    if (!valid || !Number.isSafeInteger(value.length) || value.length < 0) {
      throw new Error(`${description} requires array data`);
    }
    return value.length;
  }

  validateStorageValue(info, value) {
    const length = Number(info.array_len);
    const isFixedArray = this.isFixedArray(info);
    const isArrayValue = Array.isArray(value)
      || (ArrayBuffer.isView(value) && typeof value.length === "number");
    if (isFixedArray !== isArrayValue) {
      throw ERROR.INVALID_PARAMETER;
    }
    if (isFixedArray && value.length !== length) throw ERROR.INVALID_PARAMETER;
    return length;
  }

  validateScalarValue(scalar, value) {
    if (scalar === "i64") {
      exactI64(value);
    } else if (value === undefined || value === null || typeof value === "object" || typeof value === "symbol" || typeof value === "function") {
      throw ERROR.INVALID_PARAMETER;
    }
  }

  writeStorage(address, info, value, view = this.memoryView()) {
    const length = this.validateStorageValue(info, value);
    if (!this.isFixedArray(info)) {
      this.writeScalar(address, info.scalar, value, view);
      return;
    }
    this.writeScalarValues(address, info.scalar, value, length, view);
  }

  writeScalarValues(address, scalar, values, length, view = this.memoryView()) {
    const size = this.scalarByteSize(scalar);
    const target = this.scalarView(address, scalar, length);
    if (target !== null) {
      if (scalar === "bool") {
        for (let index = 0; index < length; index += 1) {
          target[index] = values[index] ? 1 : 0;
        }
      } else if (scalar === "i64") {
        for (let index = 0; index < length; index += 1) {
          target[index] = exactI64(values[index]);
        }
      } else {
        target.set(values);
      }
      return;
    }
    for (let index = 0; index < length; index += 1) {
      this.writeScalar(address + index * size, scalar, values[index], view);
    }
  }

  readStorage(address, info) {
    const length = Number(info.array_len);
    const size = this.scalarByteSize(info.scalar);
    const view = this.memoryView();
    const values = Array.from({ length }, (_, index) =>
      this.readScalar(address + index * size, info.scalar, view),
    );
    return this.isFixedArray(info) ? values : values[0];
  }

  scalarByteSize(scalar) {
    if (scalar === "bool") return 1;
    if (scalar === "i32" || scalar === "f32") return 4;
    if (scalar === "i64" || scalar === "f64") return 8;
    throw ERROR.INVALID_STORAGE;
  }

  writeScalar(address, scalar, value, view = this.memoryView()) {
    if (scalar === "bool") view.setUint8(address, value ? 1 : 0);
    else if (scalar === "i32") view.setInt32(address, Number(value), true);
    else if (scalar === "i64") view.setBigInt64(address, exactI64(value), true);
    else if (scalar === "f32") view.setFloat32(address, Number(value), true);
    else if (scalar === "f64") view.setFloat64(address, Number(value), true);
    else throw ERROR.INVALID_STORAGE;
  }

  readScalar(address, scalar, view = this.memoryView()) {
    if (scalar === "bool") return view.getUint8(address) !== 0;
    if (scalar === "i32") return view.getInt32(address, true);
    if (scalar === "i64") return view.getBigInt64(address, true);
    if (scalar === "f32") return view.getFloat32(address, true);
    if (scalar === "f64") return view.getFloat64(address, true);
    throw ERROR.INVALID_STORAGE;
  }

  ensureInputCapacity(frames) {
    if (frames <= this.inputCapacityFrames) {
      return;
    }

    this.inputPtrs = [];
    for (const channel of this.inputChannels) {
      const ptr = this.alloc(frames * channel.elementSize, 16);
      this.inputPtrs.push(ptr);
    }

    const view = this.memoryView();
    for (let channel = 0; channel < this.inputCount; channel += 1) {
      view.setUint32(this.inPtrsPtr + channel * 4, this.inputPtrs[channel], true);
    }
    this.inputCapacityFrames = frames;
  }

  ensureOutputCapacity(frames) {
    if (frames <= this.outputCapacityFrames) {
      return;
    }

    this.outputPtrs = [];
    for (const channel of this.outputChannels) {
      const ptr = this.alloc(frames * channel.elementSize, 16);
      this.outputPtrs.push(ptr);
    }

    const view = this.memoryView();
    for (let channel = 0; channel < this.outputCount; channel += 1) {
      const ptr = this.outputPtrs[channel];
      view.setUint32(this.outPtrsPtr + channel * 4, ptr, true);
    }
    this.outputCapacityFrames = frames;
  }

  audioFrameCount(inputs, outputs) {
    for (let busId = 0; busId < outputs.length; busId += 1) {
      const bus = outputs[busId];
      if (bus.length > 0) return bus[0].length;
    }
    for (let busId = 0; busId < inputs.length; busId += 1) {
      const bus = inputs[busId];
      if (bus.length > 0) return bus[0].length;
    }
    throw ERROR.INVALID_AUDIO_SHAPE;
  }

  copyInputSamples(
    source,
    info,
    target,
    callbackOffset,
    startFrame,
    segmentFrames,
    view,
  ) {
    if (target !== null) {
      if (info.scalar === "f32") {
        if (callbackOffset === 0 && segmentFrames === source.length) {
          target.set(source, startFrame);
        } else {
          for (let frame = 0; frame < segmentFrames; frame += 1) {
            target[startFrame + frame] = source[callbackOffset + frame];
          }
        }
        return;
      }
      if (info.scalar === "f64") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = source[callbackOffset + frame];
        }
        return;
      }
      if (info.scalar === "i32") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = audioI32(source[callbackOffset + frame]);
        }
        return;
      }
      if (info.scalar === "i64") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = audioI64(source[callbackOffset + frame]);
        }
        return;
      }
      if (info.scalar === "bool") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = source[callbackOffset + frame] !== 0 ? 1 : 0;
        }
        return;
      }
      throw ERROR.INVALID_STORAGE;
    }

    for (let frame = 0; frame < segmentFrames; frame += 1) {
      const value = source[callbackOffset + frame];
      this.writeScalar(
        info.pointer + (startFrame + frame) * info.elementSize,
        info.scalar,
        info.scalar === "bool"
          ? value !== 0
          : info.scalar === "i64"
            ? audioI64(value)
            : info.scalar === "i32"
              ? audioI32(value)
              : value,
        view,
      );
    }
  }

  zeroInputSamples(info, target, startFrame, segmentFrames, view) {
    if (target !== null) {
      target.fill(
        info.scalar === "i64" ? 0n : 0,
        startFrame,
        startFrame + segmentFrames,
      );
      return;
    }
    for (let frame = 0; frame < segmentFrames; frame += 1) {
      this.writeScalar(
        info.pointer + (startFrame + frame) * info.elementSize,
        info.scalar,
        info.scalar === "i64" ? 0n : 0,
        view,
      );
    }
  }

  marshalInputSegment(
    inputs,
    callbackFrames,
    callbackOffset,
    startFrame,
    segmentFrames,
  ) {
    const view = this.dataViewCache;
    let inputChannel = 0;
    for (let busId = 0; busId < inputs.length; busId += 1) {
      const bus = inputs[busId];
      for (let busChannel = 0; busChannel < bus.length; busChannel += 1) {
        const source = bus[busChannel];
        if (source.length !== callbackFrames) {
          throw ERROR.INVALID_AUDIO_SHAPE;
        }
        if (inputChannel < this.inputCount) {
          const info = this.inputChannels[inputChannel];
          this.copyInputSamples(
            source,
            info,
            this.inputViews[inputChannel],
            callbackOffset,
            startFrame,
            segmentFrames,
            view,
          );
        }
        inputChannel += 1;
      }
    }

    for (; inputChannel < this.inputCount; inputChannel += 1) {
      const info = this.inputChannels[inputChannel];
      this.zeroInputSamples(
        info,
        this.inputViews[inputChannel],
        startFrame,
        segmentFrames,
        view,
      );
    }
  }

  copyOutputSamples(
    destination,
    info,
    source,
    callbackOffset,
    startFrame,
    segmentFrames,
    view,
  ) {
    if (source !== null) {
      if (
        info.scalar === "f32"
        && callbackOffset === 0
        && startFrame === 0
        && segmentFrames === destination.length
        && source.length === destination.length
      ) {
        destination.set(source);
        return;
      }
      if (info.scalar === "bool") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          destination[callbackOffset + frame] = source[startFrame + frame] ? 1 : 0;
        }
        return;
      }
      for (let frame = 0; frame < segmentFrames; frame += 1) {
        destination[callbackOffset + frame] = Number(source[startFrame + frame]);
      }
      return;
    }

    for (let frame = 0; frame < segmentFrames; frame += 1) {
      const value = this.readScalar(
        info.pointer + (startFrame + frame) * info.elementSize,
        info.scalar,
        view,
      );
      destination[callbackOffset + frame] = info.scalar === "bool"
        ? (value ? 1 : 0)
        : Number(value);
    }
  }

  marshalOutputSegment(
    outputs,
    callbackFrames,
    callbackOffset,
    startFrame,
    segmentFrames,
  ) {
    const view = this.dataViewCache;
    let outputChannel = 0;
    for (let busId = 0; busId < outputs.length; busId += 1) {
      const bus = outputs[busId];
      for (let busChannel = 0; busChannel < bus.length; busChannel += 1) {
        const destination = bus[busChannel];
        if (destination.length !== callbackFrames) {
          throw ERROR.INVALID_AUDIO_SHAPE;
        }
        if (outputChannel < this.outputCount) {
          const info = this.outputChannels[outputChannel];
          this.copyOutputSamples(
            destination,
            info,
            this.outputViews[outputChannel],
            callbackOffset,
            startFrame,
            segmentFrames,
            view,
          );
        } else {
          destination.fill(
            0,
            callbackOffset,
            callbackOffset + segmentFrames,
          );
        }
        outputChannel += 1;
      }
    }
  }

  invokeProcessSegment(startFrame, frames, flags) {
    this.prepareExecutionOutput();
    return this.exports.onda_process(
      this.statePtr,
      this.paramsPtr,
      this.inPtrsPtr,
      this.outPtrsPtr,
      startFrame,
      frames,
      flags,
      this.bufferPointersPtr,
      this.bufferFramesPtr,
      this.bufferChannelsPtr,
      this.bufferSampleRatesPtr,
      this.executionOutputPtr,
    );
  }

  resetRecordBatch(view, batchPtr) {
    if (!batchPtr) return;
    view.setUint32(batchPtr + 8, 0, true);
    view.setUint32(batchPtr + 12, 0, true);
    view.setUint32(batchPtr + 16, 0, true);
  }

  prepareExecutionOutput() {
    if (!this.executionOutputPtr) return;
    const view = this.memoryView();
    this.resetRecordBatch(
      view,
      view.getUint32(this.executionOutputPtr, true),
    );
    this.resetRecordBatch(
      view,
      view.getUint32(this.executionOutputPtr + 4, true),
    );
    view.setUint32(this.executionOutputPtr + 8, 0, true);
  }

  clearOutputs(outputs) {
    for (const bus of outputs) {
      for (const channel of bus) channel.fill(0);
    }
  }

  processPending(_inputs, outputs) {
    this.clearOutputs(outputs);
    return true;
  }

  processInitialized(inputs, outputs) {
    try {
      return this.render(inputs, outputs);
    } catch (error) {
      this.invalidateState();
      this.clearOutputs(outputs);
      this.publishStatus(0, 1, failureStatus(error));
      return true;
    }
  }

  render(inputs, outputs) {
    this.refreshMemoryCache();
    const frames = this.audioFrameCount(inputs, outputs);

    let callbackOffset = 0;
    while (callbackOffset < frames) {
      const startFrame = this.blockCursor;
      const segmentFrames = Math.min(
        frames - callbackOffset,
        this.blockSize - startFrame,
      );
      const endsBlock = startFrame + segmentFrames === this.blockSize;
      const flags = (startFrame === 0 ? PROCESSOR_BEGIN_BLOCK : 0)
        | (endsBlock ? PROCESSOR_END_BLOCK : 0);

      if (startFrame === 0) {
        this.paramSmoothing.beginBlock(this.blockSize, this.dataViewCache, this.paramsPtr);
      }

      this.marshalInputSegment(
        inputs,
        frames,
        callbackOffset,
        startFrame,
        segmentFrames,
      );
      const status = this.invokeProcessSegment(
        startFrame,
        segmentFrames,
        flags,
      );
      this.publishExecutionOutput(
        EXECUTION_OPERATION_PROCESS,
        0,
      );
      if (status !== PROCESSOR_EXECUTION_OK) {
        this.invalidateState();
        this.clearOutputs(outputs);
        this.publishStatus(0, 1, status);
        return true;
      }
      this.marshalOutputSegment(
        outputs,
        frames,
        callbackOffset,
        startFrame,
        segmentFrames,
      );

      callbackOffset += segmentFrames;
      this.blockCursor = endsBlock ? 0 : startFrame + segmentFrames;
    }

    return true;
  }
}

registerProcessor(ONDA_AUDIO_WORKLET_PROCESSOR_NAME, OndaWasmProcessor);
