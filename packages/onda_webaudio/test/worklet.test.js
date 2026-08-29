import assert from "node:assert/strict";
import test from "node:test";

let Processor;

globalThis.sampleRate = 48_000;
globalThis.AudioWorkletProcessor = class {
  constructor() {
    this.port = {
      onmessage: null,
      postMessage() {},
    };
  }
};
globalThis.registerProcessor = (_name, constructor) => {
  Processor = class InitializedTestProcessor extends constructor {
    constructor(options = {}) {
      super({
        ...options,
        processorOptions: {
          initialize: true,
          ...options.processorOptions,
        },
      });
    }
  };
};

await import("../src/worklet.js");

const wasm = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0,
  1, 5, 1, 96, 0, 1, 127,
  3, 3, 2, 0, 0,
  5, 3, 1, 0, 1,
  6, 7, 1, 127, 0, 65, 128, 8, 11,
  7, 61, 4,
  6, 109, 101, 109, 111, 114, 121, 2, 0,
  11, 95, 95, 104, 101, 97, 112, 95, 98, 97, 115, 101, 3, 0,
  19, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 111, 114, 95, 105, 110, 105, 116, 0, 0,
  12, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 0, 1,
  10, 11, 2, 4, 0, 65, 0, 11, 4, 0, 65, 0, 11,
]);

const highBitHeapBaseWasm = new Uint8Array([
  ...wasm.slice(0, 25),
  6, 10, 1, 127, 0, 65, 128, 128, 128, 128, 120, 11,
  ...wasm.slice(34),
]);

const processFailureWasm = wasm.slice();
processFailureWasm[processFailureWasm.length - 2] = 1;

function metadata() {
  return {
    artifact_kind: "webassembly_module",
    target: {
      pointer_model: "linear_memory_offset",
      pointer_width_bits: 32,
    },
    compile: { sample_rate: 48_000, block_size: 128 },
    runtime: {
      state_size_bytes: 0,
      param_size_bytes: 0,
      snapshot_size_bytes: 0,
    },
    metadata: {
      states: [],
      inputs: [],
      outputs: [{
        name: "out1",
        type_repr: "f32",
        scalar: "f32",
        array_len: 1,
        element_size_bytes: 4,
        slot_offset: 0,
      }],
      control_outputs: [],
      params: [],
      events: [],
      delegates: [],
      buffers: [{
        name: "samples",
        scalar: "f32",
        static_channels: 1,
      }],
    },
  };
}

test("worklet remains silent until explicit full initialization", () => {
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: metadata(),
      initialize: false,
    },
  });
  const output = new Float32Array([1, 1, 1]);

  assert.equal(processor.process([], [[output]]), true);
  assert.deepEqual([...output], [0, 0, 0]);
  assert.throws(() => processor.init(0), /full initialization is required/);
  processor.init(1);
  assert.equal(processor.initialized, true);
  assert.equal(processor.process, processor.processInitialized);
});

test("failed live initialization returns the worklet to the silent pending state", () => {
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: metadata(),
    },
  });
  const originalCheckExecutionStatus = processor.checkExecutionStatus;
  let printFlushes = 0;
  let delegateFlushes = 0;
  processor.flushPrint = () => { printFlushes += 1; };
  processor.flushDelegates = () => { delegateFlushes += 1; };
  processor.checkExecutionStatus = () => {
    throw new Error("simulated processor init failure");
  };

  assert.throws(() => processor.init(1), /simulated processor init failure/);
  assert.equal(printFlushes, 1);
  assert.equal(delegateFlushes, 1);
  assert.equal(processor.initialized, false);
  assert.equal(processor.process, processor.processPending);
  assert.throws(() => processor.init(0), /full initialization is required/);

  const output = new Float32Array([1, 1, 1]);
  assert.equal(processor.process([], [[output]]), true);
  assert.deepEqual([...output], [0, 0, 0]);

  processor.checkExecutionStatus = originalCheckExecutionStatus;
  processor.init(1);
  assert.equal(processor.initialized, true);
  assert.equal(processor.process, processor.processInitialized);
});

test("failed processing invalidates the worklet and keeps later callbacks silent", () => {
  const processor = new Processor({
    processorOptions: {
      wasmBytes: processFailureWasm,
      metadata: metadata(),
    },
  });
  const messages = [];
  processor.port.postMessage = (message) => messages.push(message);
  const output = new Float32Array([1, 1, 1]);

  assert.equal(processor.process([], [[output]]), true);
  assert.deepEqual([...output], [0, 0, 0]);
  assert.equal(processor.initialized, false);
  assert.equal(processor.process, processor.processPending);
  assert.equal(processor.blockCursor, 0);
  assert.equal(messages.length, 1);
  assert.equal(messages[0].type, "onda-error");

  output.fill(1);
  assert.equal(processor.process([], [[output]]), true);
  assert.deepEqual([...output], [0, 0, 0]);
  assert.equal(messages.length, 1);
});

test("failed event execution invalidates the worklet", () => {
  const descriptor = metadata();
  descriptor.metadata.events = [{
    name: "fail",
    export: "onda_process",
    params: [],
  }];
  const processor = new Processor({
    processorOptions: {
      wasmBytes: processFailureWasm,
      metadata: descriptor,
    },
  });

  assert.throws(
    () => processor.dispatchEvent("fail", []),
    /event 'fail' failed with Onda execution status 1/,
  );
  assert.equal(processor.initialized, false);
  assert.equal(processor.process, processor.processPending);
});

test("worklet captures raw delegate records only while subscribed", () => {
  const descriptor = metadata();
  descriptor.metadata.buffers = [];
  descriptor.metadata.delegates = [{
    name: "report",
    params: [
      { name: "code", scalar: "i32", array_len: 1, is_slice: false, element_size_bytes: 4 },
      { name: "values", scalar: "f32", array_len: 0, is_slice: true, element_size_bytes: 4 },
    ],
  }];
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
      delegateCapacityBytes: 24,
    },
  });
  const messages = [];
  processor.port.postMessage = (message) => messages.push(message);
  const view = new DataView(processor.memory.buffer);
  assert.equal(view.getUint32(processor.executionOutputPtr, true), 0);
  processor.flushDelegates("unsubscribed process segment");
  assert.equal(messages.length, 0);

  processor.handleMessage({
    type: "delegate-subscription",
    enabled: true,
    subscriptionId: 7,
  });
  assert.equal(
    view.getUint32(processor.executionOutputPtr, true),
    processor.delegateBatchPtr,
  );
  const storage = processor.delegateStoragePtr;
  view.setUint32(storage, 0, true);
  view.setUint32(storage + 4, 16, true);
  view.setInt32(storage + 8, 7, true);
  view.setInt32(storage + 12, 2, true);
  view.setFloat32(storage + 16, 1.25, true);
  view.setFloat32(storage + 20, -2.5, true);
  view.setUint32(processor.delegateBatchPtr + 8, 24, true);
  view.setUint32(processor.delegateBatchPtr + 12, 1, true);
  processor.flushDelegates("process segment");
  assert.equal(messages.length, 1);
  assert.equal(messages[0].type, "onda-delegate-records");
  assert.equal(messages[0].operation, "process segment");
  assert.equal(messages[0].recordCount, 1);
  assert.equal(messages[0].overflowCount, 0);
  assert.equal(messages[0].transportDropCount, 0);
  assert.equal(messages[0].subscriptionId, 7);
  assert.deepEqual([...messages[0].storage], [
    0, 0, 0, 0, 16, 0, 0, 0,
    7, 0, 0, 0, 2, 0, 0, 0,
    0, 0, 160, 63, 0, 0, 32, 192,
  ]);

  const firstStorage = messages[0].storage;
  processor.handleMessage({ type: "delegate-ack", storage: firstStorage });
  assert.equal(processor.delegateTransport.inFlight, 0);
  assert.equal(
    processor.delegateTransport.availableBuffers.length,
    processor.delegateTransport.poolSize,
  );

  processor.flushDelegates("reused process segment");
  assert.equal(messages.length, 2);
  assert.equal(messages[1].storage.buffer, firstStorage.buffer);
  processor.handleMessage({
    type: "delegate-ack",
    storage: messages[1].storage,
  });

  const heldStorage = processor.delegateTransport.availableBuffers.splice(0);
  processor.delegateTransport.inFlight = processor.delegateTransport.poolSize;
  view.setUint32(processor.delegateBatchPtr + 16, 2, true);
  processor.flushDelegates("saturated process segment");
  assert.equal(messages.length, 2);
  assert.equal(processor.delegateTransport.pendingDrops, 1);
  processor.handleMessage({ type: "delegate-ack", storage: heldStorage[0] });
  assert.equal(messages.length, 3);
  assert.equal(messages[2].operation, "transport");
  assert.equal(messages[2].recordCount, 0);
  assert.equal(messages[2].overflowCount, 2);
  assert.equal(messages[2].transportDropCount, 1);

  processor.handleMessage({
    type: "delegate-subscription",
    enabled: false,
    subscriptionId: 7,
  });
  assert.equal(view.getUint32(processor.executionOutputPtr, true), 0);
});

test("worklet transports raw print records without formatting", () => {
  const descriptor = metadata();
  descriptor.metadata.buffers = [];
  descriptor.metadata.log_sites = [{
    index: 0,
    label: "value",
    source: { file: null, line: 1, column: 1, end_line: 1, end_column: 10 },
    lexical_owner: "program",
    declaration: "sample",
    argument_types: ["i32"],
    payload_size_bytes: 4,
  }];
  const inactive = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
      printCapacityBytes: 16,
    },
  });
  assert.notEqual(inactive.printBatchPtr, 0);
  assert.equal(
    new DataView(inactive.memory.buffer).getUint32(inactive.executionOutputPtr + 4, true),
    0,
  );

  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
      printCapacityBytes: 16,
      printCollectionEnabled: true,
      printSubscriptionId: 7,
    },
  });
  const messages = [];
  processor.port.postMessage = (message) => messages.push(message);
  const view = new DataView(processor.memory.buffer);
  view.setUint32(processor.printStoragePtr, 0, true);
  view.setUint32(processor.printStoragePtr + 4, 4, true);
  view.setInt32(processor.printStoragePtr + 8, 42, true);
  view.setUint32(processor.printBatchPtr + 8, 12, true);
  view.setUint32(processor.printBatchPtr + 12, 1, true);
  processor.flushPrint("process segment");
  assert.equal(messages.length, 1);
  assert.equal(messages[0].type, "onda-print-records");
  assert.equal(messages[0].subscriptionId, 7);
  assert.equal(messages[0].recordCount, 1);
  assert.equal(messages[0].transportDropCount, 0);
  assert.deepEqual(
    [...messages[0].storage.subarray(0, messages[0].usedBytes)],
    [0, 0, 0, 0, 4, 0, 0, 0, 42, 0, 0, 0],
  );

  processor.handleMessage({
    type: "print-subscription",
    enabled: false,
    subscriptionId: 7,
  });
  assert.equal(view.getUint32(processor.executionOutputPtr + 4, true), 0);
  processor.flushPrint("process segment");
  assert.equal(messages.length, 1);
});

test("worklet uses null pointers only for absent processor surfaces", () => {
  const descriptor = metadata();
  descriptor.metadata.buffers = [];
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
    },
  });

  assert.equal(processor.paramsPtr, 0);
  assert.equal(processor.statePtr, 0);
  assert.equal(processor.bufferPointersPtr, 0);
  assert.equal(processor.bufferFramesPtr, 0);
  assert.equal(processor.bufferChannelsPtr, 0);
  assert.equal(processor.bufferSampleRatesPtr, 0);
});

test("worklet rejects empty external-buffer bindings", () => {
  assert.throws(
    () => new Processor({
      processorOptions: {
        wasmBytes: wasm,
        metadata: metadata(),
        buffers: { samples: { data: [] } },
      },
    }),
    /requires non-empty bound data/,
  );
});

test("worklet publishes initial buffer descriptors after Wasm memory growth", () => {
  const pageBytes = 64 * 1024;
  const data = new Float32Array(pageBytes / Float32Array.BYTES_PER_ELEMENT);
  data[0] = 0.25;
  data[data.length - 1] = 0.75;

  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: metadata(),
      buffers: { samples: { data } },
    },
  });
  const binding = processor.bufferBindings[0];
  const view = new DataView(processor.memory.buffer);
  const samples = new Float32Array(
    processor.memory.buffer,
    binding.pointer,
    data.length,
  );

  assert.ok(processor.memory.buffer.byteLength > pageBytes);
  assert.equal(
    view.getUint32(processor.bufferPointersPtr, true),
    binding.pointer,
  );
  assert.equal(view.getInt32(processor.bufferFramesPtr, true), data.length);
  assert.equal(view.getInt32(processor.bufferChannelsPtr, true), 1);
  assert.equal(view.getFloat32(processor.bufferSampleRatesPtr, true), 48_000);
  assert.equal(samples[0], 0.25);
  assert.equal(samples[samples.length - 1], 0.75);
});

test("worklet prepares neutral unbound descriptors with a null buffer entry", () => {
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: metadata(),
    },
  });
  const buffer = processor.readBuffer("samples");

  assert.equal(buffer.frames, 1);
  assert.equal(buffer.channels, 1);
  assert.equal(buffer.sampleRate, 48_000);
  assert.deepEqual(buffer.data, [0]);
  assert.equal(
    new DataView(processor.memory.buffer).getUint32(processor.bufferPointersPtr, true),
    0,
  );
  assert.equal(processor.bufferBindings[0].bound, false);
});

test("worklet accepts nullable logical buffer-array bindings", () => {
  const descriptor = metadata();
  descriptor.metadata.buffers = [
    { name: "bank[0]", scalar: "f32", static_channels: 1 },
    { name: "bank[1]", scalar: "f32", static_channels: 1 },
  ];
  descriptor.metadata.buffer_arrays = [{ name: "bank", first_buffer: 0, len: 2 }];
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
      buffers: { bank: [null, { data: [0.25, 0.5] }] },
    },
  });

  assert.equal(processor.bufferBindings[0].bound, false);
  assert.equal(processor.bufferBindings[1].bound, true);
  assert.deepEqual(processor.readBuffer("bank[0]").data, [0]);
  assert.deepEqual(processor.readBuffer("bank[1]").data, [0.25, 0.5]);
});

test("worklet validates and reports the f32 buffer sample rate seen by Wasm", () => {
  for (const sampleRate of [Number.MIN_VALUE, Number.MAX_VALUE]) {
    assert.throws(
      () => new Processor({
        processorOptions: {
          wasmBytes: wasm,
          metadata: metadata(),
          buffers: { samples: { data: [1], sampleRate } },
        },
      }),
      /invalid sample rate/,
    );
  }

  const requestedSampleRate = 44_100.1;
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: metadata(),
      buffers: {
        samples: { data: [1], sampleRate: requestedSampleRate },
      },
    },
  });
  const storedSampleRate = Math.fround(requestedSampleRate);

  assert.equal(processor.readBuffer("samples").sampleRate, storedSampleRate);
  assert.equal(
    new DataView(processor.memory.buffer).getFloat32(
      processor.bufferSampleRatesPtr,
      true,
    ),
    storedSampleRate,
  );
});

test("worklet interprets the wasm32 heap base as an unsigned address", () => {
  assert.equal(WebAssembly.validate(highBitHeapBaseWasm), true);
  const originalAlloc = Processor.prototype.alloc;
  let allocationCursor = 2_048;
  Processor.prototype.alloc = function allocateInTestMemory(size, align) {
    allocationCursor = this.alignUp(allocationCursor, align);
    const pointer = allocationCursor;
    allocationCursor += size;
    return pointer;
  };

  try {
    const processor = new Processor({
      processorOptions: {
        wasmBytes: highBitHeapBaseWasm,
        metadata: metadata(),
        buffers: { samples: { data: [0] } },
      },
    });
    assert.equal(processor.heap, 0x8000_0000);
  } finally {
    Processor.prototype.alloc = originalAlloc;
  }
});

test("worklet writes adapter-canonical parameter values without reconversion", () => {
  const descriptor = metadata();
  descriptor.runtime.param_size_bytes = 4;
  descriptor.metadata.buffers = [];
  descriptor.metadata.params = [{
    name: "mode",
    type_repr: "i32",
    scalar: "i32",
    array_len: 1,
    element_size_bytes: 4,
    slot_offset: 0,
    byte_offset: 0,
    state_byte_offset: null,
    byte_size: 4,
    default_reprs: ["0"],
    range_min_repr: "0",
    range_max_repr: "10",
    param_control: {
      scale: "linear",
      curve: null,
      unit: null,
      step_repr: "2",
      step_count: 5,
    },
  }];
  const processor = new Processor({
    processorOptions: {
      wasmBytes: wasm,
      metadata: descriptor,
    },
  });
  const view = new DataView(processor.memory.buffer);

  processor.setParam("mode", 4);
  assert.equal(view.getInt32(processor.paramsPtr, true), 4);
  processor.setParam("mode", 10);
  assert.equal(view.getInt32(processor.paramsPtr, true), 10);
});
