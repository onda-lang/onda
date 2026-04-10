class OndaSineProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();

    const processorOptions = options.processorOptions ?? {};
    const wasmBytes = processorOptions.wasmBytes;
    const metadata = processorOptions.metadata ?? {};
    this.frequency = Math.fround(processorOptions.frequency ?? 440);

    const bytes =
      wasmBytes instanceof Uint8Array ? wasmBytes : new Uint8Array(wasmBytes);
    const module = new WebAssembly.Module(bytes);
    const instance = new WebAssembly.Instance(module, {});
    this.exports = instance.exports;
    this.memory = this.exports.memory;
    this.port.onmessage = (event) => {
      const message = event.data ?? {};
      if (message.type === "frequency") {
        this.frequency = Math.fround(message.value ?? this.frequency);
      }
    };

    if (!(this.memory instanceof WebAssembly.Memory)) {
      throw new Error("expected exported wasm memory");
    }

    this.heap = Number(this.exports.__heap_base?.value ?? this.exports.__heap_base ?? 0);
    this.stateSizeBytes = Number(metadata.runtime?.state_size_bytes ?? 0);
    this.paramInfo = Array.isArray(metadata.metadata?.params) ? metadata.metadata.params : [];
    this.outputCount = Number(metadata.metadata?.outputs?.length ?? 1);
    this.outputPtrs = [];
    this.outputCapacityFrames = 0;

    let paramBytes = 4;
    for (const param of this.paramInfo) {
      const end = Number(param.byte_offset ?? 0) + Number(param.byte_size ?? 0);
      if (end > paramBytes) {
        paramBytes = end;
      }
    }

    this.paramsPtr = this.alloc(paramBytes, 4);
    this.statePtr = this.alloc(this.stateSizeBytes, 16);
    this.outPtrsPtr = this.alloc(this.outputCount * 4, 4);
    this.writeParamF32("freq", this.frequency);
    this.ensureOutputCapacity(128);
    this.exports.onda_init(this.paramsPtr, this.statePtr);
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
    const mask = align - 1;
    return (value + mask) & ~mask;
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

  writeParamF32(name, value) {
    const param = this.paramInfo.find((entry) => entry.name === name);
    const offset = Number(param?.byte_offset ?? 0);
    this.memoryView().setFloat32(this.paramsPtr + offset, Math.fround(value), true);
  }

  ensureOutputCapacity(frames) {
    if (frames <= this.outputCapacityFrames) {
      return;
    }

    const view = this.memoryView();
    this.outputPtrs = [];
    for (let channel = 0; channel < this.outputCount; channel += 1) {
      const ptr = this.alloc(frames * 4, 16);
      this.outputPtrs.push(ptr);
      view.setUint32(this.outPtrsPtr + channel * 4, ptr, true);
    }
    this.outputCapacityFrames = frames;
  }

  process(_inputs, outputs) {
    const outChannels = outputs[0];
    if (!outChannels || outChannels.length === 0) {
      return true;
    }

    const frames = outChannels[0].length;
    this.ensureOutputCapacity(frames);
    this.writeParamF32("freq", this.frequency);
    this.exports.onda_process(
      0,
      this.outPtrsPtr,
      frames,
      this.paramsPtr,
      this.statePtr,
      0,
      0,
      0,
      0,
    );

    for (let channel = 0; channel < outChannels.length; channel += 1) {
      const wasmChannel = new Float32Array(
        this.memory.buffer,
        this.outputPtrs[channel],
        frames,
      );
      outChannels[channel].set(wasmChannel);
    }

    return true;
  }
}

registerProcessor("onda-sine-processor", OndaSineProcessor);
