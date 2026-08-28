const ONDA_PROCESS_BEGIN_BLOCK = 1 << 0;
const ONDA_PROCESS_END_BLOCK = 1 << 1;
const ONDA_INIT_PRESERVE_PINNED = 0;
const ONDA_INIT_FULL = 1;
const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";
const DEFAULT_EVENT_PAYLOAD_CAPACITY_BYTES = 64 * 1024;
const DEFAULT_DELEGATE_CAPACITY_BYTES = 64 * 1024;
const DEFAULT_PRINT_CAPACITY_BYTES = 64 * 1024;
const MAX_RECORD_TRANSPORT_BATCHES = 32;
const DELEGATE_BATCH_SIZE_BYTES = 20;
const PRINT_BATCH_SIZE_BYTES = 20;
const EXECUTION_OUTPUT_SIZE_BYTES = 8;
const HOST_LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

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
    this.blockSize = Number(metadata.compile?.block_size ?? 128);
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
    this.executionOutputPtr = this.delegateBatchPtr || this.printBatchPtr
      ? this.alloc(EXECUTION_OUTPUT_SIZE_BYTES, 4)
      : 0;
    if (this.executionOutputPtr) {
      const view = new DataView(this.memory.buffer);
      view.setUint32(this.executionOutputPtr, 0, true);
      view.setUint32(this.executionOutputPtr + 4, this.printBatchPtr, true);
    }
    this.delegateCollectionEnabled = false;
    this.delegateSubscriptionId = 0;
    this.delegateTransport = this.createRecordTransport("onda-delegate-records");
    this.printTransport = this.createRecordTransport("onda-print-records");
    this.writeParamDefaults();
    this.writeInitialParams(processorOptions.params ?? {});
    this.ensureInputCapacity(this.blockSize);
    this.ensureOutputCapacity(this.blockSize);
    this.viewsReady = true;
    this.refreshMemoryCache(true);
    this.invalidateState();
    if (processorOptions.initialize === true) {
      this.init(ONDA_INIT_FULL);
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
    throw new Error(`unsupported ABI scalar '${String(scalar)}'`);
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

  setParam(selector, value) {
    const paramId = Number.isInteger(selector)
      ? selector
      : this.paramInfo.findIndex((param) => param.name === selector);
    const param = this.paramInfo[paramId];
    if (!param) {
      throw new Error(`unknown Onda parameter '${String(selector)}'`);
    }
    if (value === undefined) {
      throw new Error(`Onda parameter '${param.name}' requires a value`);
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
      throw new Error(`Onda parameter '${param.name}' has invalid storage metadata`);
    }
    this.writeStorage(
      this.paramsPtr + offset,
      param,
      value,
    );
  }

  init(mode) {
    if (mode !== ONDA_INIT_PRESERVE_PINNED && mode !== ONDA_INIT_FULL) {
      throw new Error(`invalid Onda init mode '${String(mode)}'`);
    }
    if (mode === ONDA_INIT_PRESERVE_PINNED && !this.initialized) {
      throw new Error("full initialization is required before preserving pinned state");
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

  runInitialization(mode, afterInitialize) {
    // Reinitializing state does not create a compile-block boundary. Retain
    // the host-side position so the next process segment cannot synthesize an
    // extra BEGIN_BLOCK or postpone the matching END_BLOCK.
    const blockCursor = this.initialized ? this.blockCursor : 0;
    // Generated initialization mutates the live image in place. Stop exposing
    // it before entering Wasm so a failure, including one in snapshot overlay,
    // leaves the processor on the silent pending path.
    this.invalidateState();
    this.refreshMemoryCache();
    const status = this.exports.onda_processor_init(
      this.paramsPtr,
      this.statePtr,
      mode,
      this.executionOutputPtr,
    );
    this.flushPrint("processor init");
    this.flushDelegates("processor init");
    this.checkExecutionStatus(status, "processor init");
    afterInitialize?.();
    this.commitInitializedState(blockCursor);
  }

  requireInitialized(operation) {
    if (!this.initialized) {
      throw new Error(`full initialization is required before ${operation}`);
    }
  }

  checkExecutionStatus(status, operation) {
    if (status === 0) return;
    this.invalidateState();
    throw new Error(`${operation} failed with Onda execution status ${String(status)}`);
  }

  createSnapshot() {
    this.requireInitialized("snapshot");
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
    if (snapshot.byteLength !== this.snapshotSizeBytes) {
      throw new Error(
        `snapshot has ${snapshot.byteLength} bytes; expected ${this.snapshotSizeBytes}`,
      );
    }
    // The ABI restore base is a fresh post-init image, so scratch and
    // control-mirror state never leak across a restore. Initialization and
    // overlay form one lifecycle transition: neither partial result is ready.
    this.runInitialization(ONDA_INIT_FULL, () => {
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
    });
  }

  normalizeSnapshotIntegerRange(entry, state, offset, byteSize) {
    const range = entry.integer_range;
    if (range === null || range === undefined) return;
    if (entry.array_len !== 1 || (entry.scalar !== "i32" && entry.scalar !== "i64")) {
      throw new Error(`state '${String(entry.name)}' has an invalid integer range`);
    }
    const mode = range.mode;
    if (mode !== "clamp" && mode !== "wrap") {
      throw new Error(`state '${String(entry.name)}' has an invalid integer range mode`);
    }
    const view = new DataView(state.buffer, state.byteOffset + offset, byteSize);
    if (entry.scalar === "i32") {
      const min = Number(range.min?.value);
      const max = Number(range.max?.value);
      if (!Number.isInteger(min) || !Number.isInteger(max) || min > max || byteSize !== 4) {
        throw new Error(`state '${String(entry.name)}' has invalid i32 range bounds`);
      }
      const value = view.getInt32(0, true);
      const normalized = mode === "clamp"
        ? Math.min(Math.max(value, min), max)
        : min + (((value - min) % (max - min + 1)) + (max - min + 1)) % (max - min + 1);
      view.setInt32(0, normalized, true);
      return;
    }
    let min;
    let max;
    try {
      min = BigInt(range.min?.value);
      max = BigInt(range.max?.value);
    } catch {
      throw new Error(`state '${String(entry.name)}' has invalid i64 range bounds`);
    }
    if (min > max || byteSize !== 8) {
      throw new Error(`state '${String(entry.name)}' has invalid i64 range bounds`);
    }
    const value = view.getBigInt64(0, true);
    const width = max - min + 1n;
    const normalized = mode === "clamp"
      ? (value < min ? min : (value > max ? max : value))
      : min + ((value - min) % width + width) % width;
    view.setBigInt64(0, normalized, true);
  }

  snapshotBytes(value) {
    if (value instanceof Uint8Array) return value;
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new Error("snapshot restore requires byte storage");
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
      throw new Error(
        `state '${String(entry.name)}' has invalid snapshot metadata`,
      );
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
    try {
      if (message.type === "set-param") {
        this.setParam(
          message.param ?? message.name ?? message.index,
          message.value,
        );
        this.postResponse(message, { type: "onda-ok", operation: message.type });
      } else if (message.type === "init") {
        this.init(message.mode);
        this.postResponse(message, { type: "onda-ok", operation: message.type });
      } else if (message.type === "event") {
        this.dispatchEvent(
          message.event ?? message.name ?? message.index,
          message.values ?? message.args ?? {},
        );
        this.postResponse(message, { type: "onda-ok", operation: message.type });
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
        this.postResponse(message, { type: "onda-ok", operation: message.type });
      } else if (message.type === "delegate-subscription") {
        this.setDelegateSubscription(message.enabled, message.subscriptionId);
      } else if (message.type === "delegate-ack") {
        this.ackRecordTransport(this.delegateTransport);
      } else if (message.type === "print-ack") {
        this.ackRecordTransport(this.printTransport);
      } else {
        throw new Error(`unknown Onda worklet operation '${String(message.type)}'`);
      }
    } catch (error) {
      this.port.postMessage({
        type: "onda-error",
        operation: message.type ?? "unknown",
        requestId: message.requestId,
        error: String(error && error.message ? error.message : error),
      });
    }
  }

  dispatchEvent(selector, values) {
    this.requireInitialized("event dispatch");
    const eventId = Number.isInteger(selector)
      ? selector
      : this.eventInfo.findIndex((event) => event.name === selector);
    const event = this.eventInfo[eventId];
    if (!event) {
      throw new Error(`unknown Onda event '${String(selector)}'`);
    }
    let payloadSize = 0;
    for (let paramId = 0; paramId < event.params.length; paramId += 1) {
      const param = event.params[paramId];
      const value = this.eventValue(event, param, paramId, values);
      if (param.is_slice) {
        const length = this.sequenceLength(
          value,
          `event '${event.name}' slice '${param.name}'`,
        );
        payloadSize = this.checkedEventPayloadSize(
          payloadSize,
          4 + length * this.scalarByteSize(param.scalar),
          event.name,
        );
      } else {
        this.validateStorageValue(param, value);
        payloadSize = this.checkedEventPayloadSize(
          payloadSize,
          Number(param.byte_size ?? 0),
          event.name,
        );
      }
    }
    if (payloadSize > this.eventPayloadCapacity) {
      throw new Error(
        `event '${event.name}' requires ${payloadSize} payload bytes; configured capacity is ${this.eventPayloadCapacity}`,
      );
    }

    let offset = 0;
    const view = this.memoryView();
    for (let paramId = 0; paramId < event.params.length; paramId += 1) {
      const param = event.params[paramId];
      const value = this.eventValue(event, param, paramId, values);
      const address = this.eventPayloadPtr + offset;
      if (param.is_slice) {
        const length = this.sequenceLength(
          value,
          `event '${event.name}' slice '${param.name}'`,
        );
        view.setInt32(address, length, true);
        this.writeScalarValues(address + 4, param.scalar, value, length, view);
        offset += 4 + length * this.scalarByteSize(param.scalar);
      } else {
        this.writeStorage(address, param, value, view);
        offset += Number(param.byte_size ?? 0);
      }
    }
    const handler = this.exports[event.export];
    if (typeof handler !== "function") {
      throw new Error(`missing WebAssembly export '${event.export}'`);
    }
    const status = handler(
      this.eventPayloadPtr,
      this.paramsPtr,
      this.statePtr,
      this.bufferPointersPtr,
      this.bufferFramesPtr,
      this.bufferChannelsPtr,
      this.bufferSampleRatesPtr,
      this.executionOutputPtr,
    );
    this.flushPrint(`event '${event.name}'`);
    this.flushDelegates(`event '${event.name}'`);
    this.checkExecutionStatus(status, `event '${event.name}'`);
  }

  eventValue(event, param, paramId, values) {
    const supplied = Array.isArray(values)
      ? values[paramId]
      : values?.[param.name];
    const value = supplied === undefined
      ? this.metadataDefaultValue(param)
      : supplied;
    if (value === undefined) {
      throw new Error(
        `event '${event.name}' requires parameter '${param.name}'`,
      );
    }
    return value;
  }

  checkedEventPayloadSize(current, additional, eventName) {
    const next = current + additional;
    if (
      !Number.isSafeInteger(additional)
      || additional < 0
      || !Number.isSafeInteger(next)
      || next > 0x7fff_ffff
    ) {
      throw new Error(`event '${eventName}' payload exceeds the 32-bit ABI limit`);
    }
    return next;
  }

  readControlOutputs() {
    this.requireInitialized("reading control outputs");
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

  flushDelegates(operation) {
    if (!this.delegateCollectionEnabled || !this.delegateBatchPtr) return;
    const view = this.memoryView();
    const usedBytes = view.getUint32(this.delegateBatchPtr + 8, true);
    const recordCount = view.getUint32(this.delegateBatchPtr + 12, true);
    const overflowCount = view.getUint32(this.delegateBatchPtr + 16, true);
    if (usedBytes > this.delegateCapacity) {
      throw new Error("processor returned an invalid delegate byte count");
    }
    if (!usedBytes && !recordCount && !overflowCount) {
      this.flushPendingRecordLoss(this.delegateTransport);
      return;
    }
    if (this.deferSaturatedRecordBatch(
      this.delegateTransport,
      recordCount,
      overflowCount,
    )) return;
    const storage = new Uint8Array(
      this.memory.buffer,
      this.delegateStoragePtr,
      usedBytes,
    ).slice();
    this.postRecordBatch(
      this.delegateTransport,
      operation,
      storage,
      recordCount,
      overflowCount,
    );
  }

  saturatedAdd(lhs, rhs) {
    return Math.min(0xffff_ffff, lhs + rhs);
  }

  flushPrint(operation) {
    if (!this.printBatchPtr) return;
    const view = this.memoryView();
    const usedBytes = view.getUint32(this.printBatchPtr + 8, true);
    const recordCount = view.getUint32(this.printBatchPtr + 12, true);
    const overflowCount = view.getUint32(this.printBatchPtr + 16, true);
    if (usedBytes > this.printCapacity) {
      throw new Error("processor returned an invalid print byte count");
    }
    if (!usedBytes && !recordCount && !overflowCount) {
      this.flushPendingRecordLoss(this.printTransport);
      return;
    }
    if (this.deferSaturatedRecordBatch(
      this.printTransport,
      recordCount,
      overflowCount,
    )) return;
    const storage = new Uint8Array(
      this.memory.buffer,
      this.printStoragePtr,
      usedBytes,
    ).slice();
    this.postRecordBatch(
      this.printTransport,
      operation,
      storage,
      recordCount,
      overflowCount,
    );
  }

  createRecordTransport(messageType) {
    return {
      messageType,
      inFlight: 0,
      pendingDrops: 0,
      pendingOverflow: 0,
    };
  }

  deferSaturatedRecordBatch(transport, recordCount, overflowCount) {
    if (transport.inFlight >= MAX_RECORD_TRANSPORT_BATCHES) {
      transport.pendingDrops = this.saturatedAdd(transport.pendingDrops, recordCount);
      transport.pendingOverflow = this.saturatedAdd(
        transport.pendingOverflow,
        overflowCount,
      );
      return true;
    }
    return false;
  }

  postRecordBatch(transport, operation, storage, recordCount, overflowCount) {
    const transportDropCount = transport.pendingDrops;
    const combinedOverflow = this.saturatedAdd(
      transport.pendingOverflow,
      overflowCount,
    );
    transport.pendingDrops = 0;
    transport.pendingOverflow = 0;
    transport.inFlight += 1;
    this.port.postMessage({
      type: transport.messageType,
      operation,
      storage,
      usedBytes: storage.byteLength,
      recordCount,
      overflowCount: combinedOverflow,
      transportDropCount,
      ...(transport === this.delegateTransport
        ? { subscriptionId: this.delegateSubscriptionId }
        : {}),
    }, [storage.buffer]);
  }

  flushPendingRecordLoss(transport) {
    if (
      transport.inFlight >= MAX_RECORD_TRANSPORT_BATCHES
      || (!transport.pendingDrops && !transport.pendingOverflow)
    ) {
      return;
    }
    this.postRecordBatch(
      transport,
      "transport",
      new Uint8Array(0),
      0,
      0,
    );
  }

  ackRecordTransport(transport) {
    transport.inFlight = Math.max(0, transport.inFlight - 1);
    this.flushPendingRecordLoss(transport);
  }

  setDelegateSubscription(enabled, subscriptionId) {
    this.delegateCollectionEnabled = enabled === true && this.delegateStoragePtr !== 0;
    this.delegateSubscriptionId = Number.isSafeInteger(subscriptionId)
      ? subscriptionId
      : 0;
    this.delegateTransport.pendingDrops = 0;
    this.delegateTransport.pendingOverflow = 0;
    if (this.delegateBatchPtr) {
      const view = this.memoryView();
      view.setUint32(this.delegateBatchPtr + 8, 0, true);
      view.setUint32(this.delegateBatchPtr + 12, 0, true);
      view.setUint32(this.delegateBatchPtr + 16, 0, true);
      view.setUint32(
        this.executionOutputPtr,
        this.delegateCollectionEnabled ? this.delegateBatchPtr : 0,
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
      : this.bufferInfo.findIndex((buffer) => buffer.name === selector);
    const binding = this.bufferBindings[bufferId];
    if (!binding) {
      throw new Error(`unknown Onda buffer '${String(selector)}'`);
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
    return info?.is_slice !== true && /\[[0-9]+\]$/.test(info?.type_repr ?? "");
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
      throw new Error(
        isFixedArray
          ? `'${info.name}' requires exactly ${length} ${info.scalar} value(s)`
          : `'${info.name}' requires one ${info.scalar} value`,
      );
    }
    if (isFixedArray && value.length !== length) {
      throw new Error(
        `'${info.name}' requires exactly ${length} ${info.scalar} value(s)`,
      );
    }
    return length;
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
          target[index] = BigInt(values[index]);
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
    throw new Error(`unsupported ABI scalar '${String(scalar)}'`);
  }

  writeScalar(address, scalar, value, view = this.memoryView()) {
    if (scalar === "bool") view.setUint8(address, value ? 1 : 0);
    else if (scalar === "i32") view.setInt32(address, Number(value), true);
    else if (scalar === "i64") view.setBigInt64(address, BigInt(value), true);
    else if (scalar === "f32") view.setFloat32(address, Number(value), true);
    else if (scalar === "f64") view.setFloat64(address, Number(value), true);
    else throw new Error(`unsupported ABI scalar '${String(scalar)}'`);
  }

  readScalar(address, scalar, view = this.memoryView()) {
    if (scalar === "bool") return view.getUint8(address) !== 0;
    if (scalar === "i32") return view.getInt32(address, true);
    if (scalar === "i64") return view.getBigInt64(address, true);
    if (scalar === "f32") return view.getFloat32(address, true);
    if (scalar === "f64") return view.getFloat64(address, true);
    throw new Error(`unsupported ABI scalar '${String(scalar)}'`);
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
    throw new Error("AudioWorklet callback has no audio channel from which to derive its frame count");
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
          target[startFrame + frame] = Math.trunc(source[callbackOffset + frame]);
        }
        return;
      }
      if (info.scalar === "i64") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = BigInt(
            Math.trunc(source[callbackOffset + frame]),
          );
        }
        return;
      }
      if (info.scalar === "bool") {
        for (let frame = 0; frame < segmentFrames; frame += 1) {
          target[startFrame + frame] = source[callbackOffset + frame] !== 0 ? 1 : 0;
        }
        return;
      }
      throw new Error(`unsupported audio input scalar '${String(info.scalar)}'`);
    }

    for (let frame = 0; frame < segmentFrames; frame += 1) {
      const value = source[callbackOffset + frame];
      this.writeScalar(
        info.pointer + (startFrame + frame) * info.elementSize,
        info.scalar,
        info.scalar === "bool"
          ? value !== 0
          : info.scalar === "i64"
            ? BigInt(Math.trunc(value))
            : info.scalar === "i32"
              ? Math.trunc(value)
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
          throw new Error("AudioWorklet input channels have inconsistent block sizes");
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
          throw new Error("AudioWorklet output channels have inconsistent block sizes");
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
      const flags = (startFrame === 0 ? ONDA_PROCESS_BEGIN_BLOCK : 0)
        | (endsBlock ? ONDA_PROCESS_END_BLOCK : 0);

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
      this.flushPrint("process");
      if (status !== 0) {
        this.invalidateState();
        this.clearOutputs(outputs);
        this.port.postMessage({
          type: "onda-error",
          operation: "process",
          error: `processor process failed with Onda execution status ${String(status)}`,
        });
        return true;
      }
      this.flushDelegates("process");
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
