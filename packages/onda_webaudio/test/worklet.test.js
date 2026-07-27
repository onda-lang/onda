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
  Processor = constructor;
};

await import("../src/worklet.js");

const wasm = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0,
  1, 4, 1, 96, 0, 0,
  3, 3, 2, 0, 0,
  5, 3, 1, 0, 1,
  6, 7, 1, 127, 0, 65, 128, 8, 11,
  7, 51, 4,
  6, 109, 101, 109, 111, 114, 121, 2, 0,
  11, 95, 95, 104, 101, 97, 112, 95, 98, 97, 115, 101, 3, 0,
  9, 111, 110, 100, 97, 95, 105, 110, 105, 116, 0, 0,
  12, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 0, 1,
  10, 7, 2, 2, 0, 11, 2, 0, 11,
]);

const highBitHeapBaseWasm = new Uint8Array([
  ...wasm.slice(0, 24),
  6, 10, 1, 127, 0, 65, 128, 128, 128, 128, 120, 11,
  ...wasm.slice(33),
]);

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
      buffers: [{
        name: "samples",
        scalar: "f32",
        static_channels: 1,
      }],
    },
  };
}

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
