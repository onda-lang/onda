const ONDA_PROCESS_BEGIN_BLOCK = 1 << 0;
const ONDA_PROCESS_END_BLOCK = 1 << 1;
const ONDA_AUDIO_WORKLET_PROCESSOR_NAME = "onda-wasm-processor";

class OndaWasmProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();

    const processorOptions = options.processorOptions ?? {};
    const wasmBytes = processorOptions.wasmBytes;
    const metadata = processorOptions.metadata ?? {};

    if (
      metadata.artifact_kind !== "webassembly_module"
      || metadata.target?.pointer_model !== "linear_memory_offset"
      || metadata.target?.pointer_width_bits !== 32
    ) {
      throw new Error("expected an Onda wasm32 executable-module artifact");
    }

    const bytes =
      wasmBytes instanceof Uint8Array ? wasmBytes : new Uint8Array(wasmBytes);
    const module = new WebAssembly.Module(bytes);
    const instance = new WebAssembly.Instance(module);
    this.exports = instance.exports;
    this.memory = this.exports.memory;

    if (!(this.memory instanceof WebAssembly.Memory)) {
      throw new Error("expected exported wasm memory");
    }

    this.heap = Number(this.exports.__heap_base?.value ?? this.exports.__heap_base ?? 0);
    this.stateSizeBytes = Number(metadata.runtime?.state_size_bytes ?? 0);
    this.paramInfo = Array.isArray(metadata.metadata?.params) ? metadata.metadata.params : [];
    this.eventInfo = Array.isArray(metadata.metadata?.events)
      ? metadata.metadata.events
      : [];
    this.controlOutputInfo = Array.isArray(metadata.metadata?.control_outputs)
      ? metadata.metadata.control_outputs
      : [];
    this.bufferInfo = Array.isArray(metadata.metadata?.buffers)
      ? metadata.metadata.buffers
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
    this.blockSize = Number(metadata.compile?.block_size ?? 128);
    this.compileSampleRate = Number(metadata.compile?.sample_rate ?? sampleRate);
    if (!Number.isInteger(this.blockSize) || this.blockSize <= 0) {
      throw new Error(`invalid compile-time block size: ${this.blockSize}`);
    }
    this.inputPtrs = [];
    this.inputCapacityFrames = 0;
    this.outputPtrs = [];
    this.outputCapacityFrames = 0;
    this.blockCursor = 0;

    let paramBytes = Number(metadata.runtime?.param_size_bytes ?? 0);
    if (!Number.isInteger(paramBytes) || paramBytes < 0) {
      throw new Error(`invalid parameter storage size: ${paramBytes}`);
    }
    for (const param of this.paramInfo) {
      const end = Number(param.byte_offset ?? 0) + Number(param.byte_size ?? 0);
      if (end > paramBytes) {
        paramBytes = end;
      }
    }
    this.paramSizeBytes = paramBytes;

    this.paramsPtr = this.alloc(paramBytes, 4);
    this.statePtr = this.alloc(this.stateSizeBytes, 16);
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
    const eventPayloadBytes = this.eventInfo.reduce(
      (size, event) => Math.max(
        size,
        Number(event.payload_size_bytes ?? event.payload_min_size_bytes ?? 0),
      ),
      0,
    );
    this.eventPayloadCapacity = Math.max(1, eventPayloadBytes);
    this.eventPayloadPtr = this.alloc(this.eventPayloadCapacity, 8);
    this.writeParamDefaults();
    this.writeInitialParams(processorOptions.params ?? {});
    this.ensureInputCapacity(this.blockSize);
    this.ensureOutputCapacity(this.blockSize);
    this.reset();
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
    const ptr = this.alignUp(this.heap, align);
    const next = ptr + size;
    this.ensureMemoryCapacity(next);
    this.heap = next;
    return ptr;
  }

  memoryView() {
    return new DataView(this.memory.buffer);
  }

  flattenAudioChannels(ports, kind) {
    const channels = [];
    for (const port of ports) {
      const channelOffset = Number(port.channel_offset ?? channels.length);
      const channelCount = Number(port.channel_count ?? 1);
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

  audioInputValue(scalar, value) {
    const number = Number(value);
    if (scalar === "bool") return number !== 0;
    if (scalar === "i32") return Math.trunc(number);
    if (scalar === "i64") return BigInt(Math.trunc(number));
    if (scalar === "f32") return Math.fround(number);
    if (scalar === "f64") return number;
    throw new Error(`unsupported audio input scalar '${String(scalar)}'`);
  }

  audioOutputValue(scalar, value) {
    const number = scalar === "bool" ? (value ? 1 : 0) : Number(value);
    return Math.fround(number);
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
      const value = this.constantValue(param.default);
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
    const length = Number(param.array_length ?? 1);
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
    this.writeStorage(this.paramsPtr + offset, param, value);
  }

  reset() {
    new Uint8Array(
      this.memory.buffer,
      this.statePtr,
      this.stateSizeBytes,
    ).fill(0);
    this.blockCursor = 0;
    this.exports.onda_init(this.paramsPtr, this.statePtr);
  }

  createSnapshot() {
    const snapshot = new Uint8Array(this.snapshotSizeBytes);
    const state = new Uint8Array(
      this.memory.buffer,
      this.statePtr,
      this.stateSizeBytes,
    );
    for (const entry of this.snapshotInfo) {
      const packedOffset = Number(
        entry.byte_offset ?? entry.packed_snapshot_byte_offset,
      );
      const physicalOffset = Number(
        entry.storage_byte_offset ?? entry.physical_state_byte_offset,
      );
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
    // control-mirror state never leak across a restore.
    this.reset();
    const state = new Uint8Array(
      this.memory.buffer,
      this.statePtr,
      this.stateSizeBytes,
    );
    for (const entry of this.snapshotInfo) {
      const packedOffset = Number(
        entry.byte_offset ?? entry.packed_snapshot_byte_offset,
      );
      const physicalOffset = Number(
        entry.storage_byte_offset ?? entry.physical_state_byte_offset,
      );
      const byteSize = Number(entry.byte_size);
      this.validateSnapshotEntry(entry, packedOffset, physicalOffset, byteSize);
      state.set(
        snapshot.subarray(packedOffset, packedOffset + byteSize),
        physicalOffset,
      );
    }
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
      } else if (message.type === "reset") {
        this.reset();
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
    const eventId = Number.isInteger(selector)
      ? selector
      : this.eventInfo.findIndex((event) => event.name === selector);
    const event = this.eventInfo[eventId];
    if (!event) {
      throw new Error(`unknown Onda event '${String(selector)}'`);
    }
    let payloadSize = 0;
    const prepared = event.params.map((param, paramId) => {
      const supplied = Array.isArray(values) ? values[paramId] : values[param.name];
      const value = supplied === undefined
        ? this.constantValue(param.default)
        : supplied;
      if (value === undefined) {
        throw new Error(
          `event '${event.name}' requires parameter '${param.name}'`,
        );
      }
      const offset = payloadSize;
      if (param.is_slice) {
        if (!Array.isArray(value) && !ArrayBuffer.isView(value)) {
          throw new Error(
            `event '${event.name}' slice '${param.name}' requires array data`,
          );
        }
        const elements = Array.from(value);
        payloadSize += 4 + elements.length * this.scalarByteSize(param.scalar);
        return { param, offset, value: elements };
      }
      payloadSize += Number(param.byte_size ?? 0);
      return { param, offset, value };
    });
    this.ensureEventPayloadCapacity(payloadSize);
    for (const item of prepared) {
      const address = this.eventPayloadPtr + item.offset;
      if (item.param.is_slice) {
        this.memoryView().setInt32(address, item.value.length, true);
        const elementSize = this.scalarByteSize(item.param.scalar);
        item.value.forEach((entry, index) =>
          this.writeScalar(
            address + 4 + index * elementSize,
            item.param.scalar,
            entry,
          ),
        );
      } else {
        this.writeStorage(address, item.param, item.value);
      }
    }
    const handler = this.exports[event.export];
    if (typeof handler !== "function") {
      throw new Error(`missing WebAssembly export '${event.export}'`);
    }
    handler(
      this.eventPayloadPtr,
      this.paramsPtr,
      this.statePtr,
      this.bufferPointersPtr,
      this.bufferFramesPtr,
      this.bufferChannelsPtr,
      this.bufferSampleRatesPtr,
    );
  }

  ensureEventPayloadCapacity(requiredBytes) {
    if (requiredBytes <= this.eventPayloadCapacity) {
      return;
    }
    let capacity = this.eventPayloadCapacity;
    while (capacity < requiredBytes) capacity *= 2;
    this.eventPayloadPtr = this.alloc(capacity, 8);
    this.eventPayloadCapacity = capacity;
  }

  readControlOutputs() {
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

  bindInitialBuffers(options) {
    this.bufferInfo.forEach((buffer, bufferId) => {
      const supplied = Array.isArray(options)
        ? options[bufferId]
        : options[buffer.name];
      if (supplied === undefined) {
        throw new Error(`Onda buffer '${buffer.name}' is not bound`);
      }
      const descriptor =
        Array.isArray(supplied) || ArrayBuffer.isView(supplied)
          ? { data: supplied }
          : supplied;
      const data = descriptor?.data;
      if (!Array.isArray(data) && !ArrayBuffer.isView(data)) {
        throw new Error(`Onda buffer '${buffer.name}' requires array data`);
      }
      const declaredChannels = Number(buffer.static_channels ?? 0);
      const channels = Number(descriptor.channels ?? declaredChannels);
      if (!Number.isInteger(channels) || channels <= 0) {
        throw new Error(`Onda buffer '${buffer.name}' requires channels > 0`);
      }
      if (declaredChannels && channels !== declaredChannels) {
        throw new Error(
          `Onda buffer '${buffer.name}' requires ${declaredChannels} channel(s)`,
        );
      }
      const frames = Number(descriptor.frames ?? data.length / channels);
      if (
        !Number.isInteger(frames) ||
        frames <= 0 ||
        frames * channels !== data.length
      ) {
        throw new Error(
          `Onda buffer '${buffer.name}' data does not match its frame/channel shape`,
        );
      }
      const sampleRate = Number(
        descriptor.sampleRate ?? descriptor.sample_rate ?? this.compileSampleRate,
      );
      if (!Number.isFinite(sampleRate) || sampleRate <= 0) {
        throw new Error(`Onda buffer '${buffer.name}' has an invalid sample rate`);
      }
      const elementSize = this.scalarByteSize(buffer.scalar);
      const pointer = this.alloc(data.length * elementSize, elementSize);
      Array.from(data).forEach((value, index) =>
        this.writeScalar(pointer + index * elementSize, buffer.scalar, value),
      );
      const view = this.memoryView();
      view.setUint32(this.bufferPointersPtr + bufferId * 4, pointer, true);
      view.setInt32(this.bufferFramesPtr + bufferId * 4, frames, true);
      view.setInt32(this.bufferChannelsPtr + bufferId * 4, channels, true);
      view.setFloat32(
        this.bufferSampleRatesPtr + bufferId * 4,
        sampleRate,
        true,
      );
      this.bufferBindings[bufferId] = {
        ...buffer,
        pointer,
        frames,
        channels,
        sampleRate,
      };
    });
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
    return {
      name: binding.name,
      frames: binding.frames,
      channels: binding.channels,
      sampleRate: binding.sampleRate,
      data: Array.from({ length }, (_, index) =>
        this.readScalar(
          binding.pointer + index * elementSize,
          binding.scalar,
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

  decodeConstantScalar(scalar) {
    const type = scalar?.type;
    const value = scalar?.value;
    if ((type !== "f32" && type !== "f64") || typeof value !== "string") {
      return value;
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

  writeStorage(address, info, value) {
    const length = Number(info.array_length ?? 1);
    const isFixedArray = Boolean(info.is_array);
    const isArrayValue = Array.isArray(value)
      || (ArrayBuffer.isView(value) && typeof value.length === "number");
    if (isFixedArray !== isArrayValue) {
      throw new Error(
        isFixedArray
          ? `'${info.name}' requires exactly ${length} ${info.scalar} value(s)`
          : `'${info.name}' requires one ${info.scalar} value`,
      );
    }
    const values = isFixedArray ? Array.from(value) : [value];
    if (values.length !== length) {
      throw new Error(
        `'${info.name}' requires exactly ${length} ${info.scalar} value(s)`,
      );
    }
    const size = this.scalarByteSize(info.scalar);
    values.forEach((entry, index) =>
      this.writeScalar(address + index * size, info.scalar, entry),
    );
  }

  readStorage(address, info) {
    const length = Number(info.array_length ?? 1);
    const size = this.scalarByteSize(info.scalar);
    const values = Array.from({ length }, (_, index) =>
      this.readScalar(address + index * size, info.scalar),
    );
    return info.is_array ? values : values[0];
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
    for (const buses of [outputs, inputs]) {
      for (const bus of buses) {
        if (bus.length > 0) {
          return bus[0].length;
        }
      }
    }
    return this.blockSize;
  }

  marshalInputSegment(
    inputs,
    callbackFrames,
    callbackOffset,
    startFrame,
    segmentFrames,
  ) {
    const view = this.memoryView();
    let inputChannel = 0;
    for (const bus of inputs) {
      for (const source of bus) {
        if (source.length !== callbackFrames) {
          throw new Error("AudioWorklet input channels have inconsistent block sizes");
        }
        if (inputChannel < this.inputCount) {
          const info = this.inputChannels[inputChannel];
          const pointer = this.inputPtrs[inputChannel];
          for (let frame = 0; frame < segmentFrames; frame += 1) {
            this.writeScalar(
              pointer + (startFrame + frame) * info.elementSize,
              info.scalar,
              this.audioInputValue(
                info.scalar,
                source[callbackOffset + frame],
              ),
              view,
            );
          }
        }
        inputChannel += 1;
      }
    }

    for (; inputChannel < this.inputCount; inputChannel += 1) {
      const info = this.inputChannels[inputChannel];
      const pointer = this.inputPtrs[inputChannel];
      const zero = this.audioInputValue(info.scalar, 0);
      for (let frame = 0; frame < segmentFrames; frame += 1) {
        this.writeScalar(
          pointer + (startFrame + frame) * info.elementSize,
          info.scalar,
          zero,
          view,
        );
      }
    }
  }

  marshalOutputSegment(
    outputs,
    callbackFrames,
    callbackOffset,
    startFrame,
    segmentFrames,
  ) {
    const view = this.memoryView();
    let outputChannel = 0;
    for (const bus of outputs) {
      for (const destination of bus) {
        if (destination.length !== callbackFrames) {
          throw new Error("AudioWorklet output channels have inconsistent block sizes");
        }
        if (outputChannel < this.outputCount) {
          const info = this.outputChannels[outputChannel];
          const pointer = this.outputPtrs[outputChannel];
          for (let frame = 0; frame < segmentFrames; frame += 1) {
            const value = this.readScalar(
              pointer + (startFrame + frame) * info.elementSize,
              info.scalar,
              view,
            );
            destination[callbackOffset + frame] = this.audioOutputValue(
              info.scalar,
              value,
            );
          }
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
    );
  }

  process(inputs, outputs) {
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
      this.invokeProcessSegment(
        startFrame,
        segmentFrames,
        flags,
      );
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
