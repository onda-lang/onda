import assert from "node:assert/strict";
import { copyFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { compileTrustedMir as compileMir } from "../src/index.js";

const unknownSource = {
  file: null,
  line: 0,
  column: 0,
  end_line: 0,
  end_column: 0,
};

const constant = (type, value) => ({
  kind: "constant",
  data: { type, value },
});
const local = (id) => ({ kind: "local", data: id });
const place = (kind, id) => ({
  base: { kind, data: id },
  projections: [],
});
const statement = (kind, data) => ({
  kind: data === undefined ? { kind } : { kind, data },
  source: unknownSource,
});
const assign = (destination, value) =>
  statement("assign", { destination, value });

function f64PassthroughMir() {
  return {
    schema_version: 5,
    config: { sample_rate: 48_000, block_size: 4 },
    source_files: [],
    types: [
      { kind: "scalar", data: "f64" },
      { kind: "scalar", data: "i32" },
      { kind: "scalar", data: "bool" },
    ],
    structs: [],
    interface: {
      inputs: [{ name: "in1", ty: 0, default: null, range: null }],
      outputs: [{ name: "out1", ty: 0 }],
      control_outputs: [],
      params: [
        {
          name: "special",
          ty: 0,
          default: {
            kind: "scalar",
            data: { type: "f64", value: "0x7ff0000000000000" },
          },
          range: null,
        },
      ],
      buffers: [],
      events: [],
    },
    state: [],
    const_data: [],
    functions: [
      {
        name: "onda_init",
        kind: { kind: "init" },
        attributes: { origin: "compiler_generated", inline: "always" },
        params: [],
        results: [],
        locals: [],
        body: { statements: [] },
        source: unknownSource,
      },
      {
        name: "onda_process",
        kind: { kind: "process" },
        attributes: { origin: "compiler_generated", inline: "always" },
        params: [
          { name: "start_frame", ty: 1, mode: "value" },
          { name: "frames", ty: 1, mode: "value" },
          { name: "flags", ty: 1, mode: "value" },
        ],
        results: [],
        locals: [
          { name: "$frame", ty: 1 },
          { name: "$continue", ty: 2 },
          { name: "$sample", ty: 0 },
          { name: "$end_frame", ty: 1 },
          { name: "$logical_frame", ty: 1 },
        ],
        body: {
          statements: [
            assign(place("local", 0), {
              kind: "use",
              data: constant("i32", 0),
            }),
            assign(place("local", 3), {
              kind: "load",
              data: place("parameter", 1),
            }),
            statement("loop", {
              body: {
                statements: [
                  assign(place("local", 1), {
                    kind: "compare",
                    data: {
                      op: "less",
                      lhs: local(0),
                      rhs: local(3),
                    },
                  }),
                  statement("if", {
                    condition: local(1),
                    then_block: {
                      statements: [
                        assign(place("local", 4), {
                          kind: "process_frame",
                          data: { offset: local(0) },
                        }),
                        assign(place("local", 2), {
                          kind: "input_load",
                          data: {
                            input: 0,
                            element: null,
                            bounds: "unchecked",
                            frame: local(4),
                          },
                        }),
                        statement("output_store", {
                          output: 0,
                          element: null,
                          bounds: "unchecked",
                          frame: local(4),
                          value: local(2),
                        }),
                        assign(place("local", 0), {
                          kind: "binary",
                          data: {
                            op: "add",
                            lhs: local(0),
                            rhs: constant("i32", 1),
                          },
                        }),
                      ],
                    },
                    else_block: { statements: [statement("break")] },
                  }),
                ],
              },
            }),
          ],
        },
        source: unknownSource,
      },
    ],
    entry_points: { init: 0, process: 1 },
  };
}

function f32PassthroughMir() {
  const mir = f64PassthroughMir();
  mir.types[0] = { kind: "scalar", data: "f32" };
  mir.interface.params[0].default = {
    kind: "scalar",
    data: { type: "f32", value: 0 },
  };
  return mir;
}

let WorkletProcessor;
let registeredProcessorName;
globalThis.AudioWorkletProcessor = class {
  constructor() {
    this.port = {
      messages: [],
      onmessage: null,
      postMessage: (message) => this.port.messages.push(message),
    };
  }
};
globalThis.registerProcessor = (name, processor) => {
  registeredProcessorName = name;
  WorkletProcessor = processor;
};

const workletFixture = await mkdtemp(join(tmpdir(), "onda-worklet-test-"));
try {
  await copyFile(
    fileURLToPath(
      new URL(
        "../../onda_webaudio/src/worklet.js",
        import.meta.url,
      ),
    ),
    join(workletFixture, "onda-wasm-processor.js"),
  );
  await import(pathToFileURL(join(workletFixture, "onda-wasm-processor.js")));
} finally {
  await rm(workletFixture, { recursive: true, force: true });
}

test("AudioWorklet module registers the public processor name", () => {
  assert.equal(registeredProcessorName, "onda-wasm-processor");
  assert.equal(typeof WorkletProcessor, "function");
});

test("AudioWorklet accepts a module compiled outside the rendering thread", () => {
  const artifact = compileMir(f32PassthroughMir());
  const module = new WebAssembly.Module(artifact.wasm);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmModule: module,
      metadata: artifact.metadata,
    },
  });
  const input = new Float32Array([0.25, -0.5, 0.75, 1]);
  const output = new Float32Array(4);

  assert.equal(processor.process([[input]], [[output]]), true);
  assert.deepEqual([...output], [...input]);
});

test("AudioWorklet reuses cached f32 views in the render callback", () => {
  const artifact = compileMir(f32PassthroughMir());
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });
  const inputView = processor.inputViews[0];
  const outputView = processor.outputViews[0];
  const dataView = processor.memoryView();
  const input = new Float32Array([0.25, -0.5, 0.75, 1]);
  const output = new Float32Array(4);
  const memoryView = processor.memoryView;
  processor.memoryView = () => {
    throw new Error("render callback requested a fresh DataView");
  };

  try {
    for (let iteration = 0; iteration < 16; iteration += 1) {
      assert.equal(processor.process([[input]], [[output]]), true);
    }
  } finally {
    processor.memoryView = memoryView;
  }

  assert.deepEqual([...output], [...input]);
  assert.equal(processor.inputViews[0], inputView);
  assert.equal(processor.outputViews[0], outputView);
  assert.equal(processor.memoryView(), dataView);
});

test("AudioWorklet bounds dynamic event storage before rendering starts", () => {
  const artifact = compileMir(f32PassthroughMir());
  artifact.metadata.metadata.events = [{
    name: "load",
    export: "onda_event_0",
    payload_size_bytes: null,
    payload_min_size_bytes: 4,
    has_dynamic_payload: true,
    params: [{
      name: "values",
      scalar: "f32",
      is_slice: true,
    }],
  }];
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
      eventPayloadCapacityBytes: 8,
    },
  });
  const memory = processor.memory.buffer;
  const heap = processor.heap;

  assert.throws(
    () => processor.dispatchEvent("load", {
      values: new Float32Array([1, 2]),
    }),
    /requires 12 payload bytes; configured capacity is 8/,
  );
  assert.equal(processor.memory.buffer, memory);
  assert.equal(processor.heap, heap);
});

test("AudioWorklet marshals Web Audio f32 through f64 MIR input/output storage", () => {
  const artifact = compileMir(f64PassthroughMir());
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });
  const input = new Float32Array([0.25, -0.5, 0.75, 1]);
  const output = new Float32Array(4);

  assert.equal(
    processor.memoryView().getFloat64(processor.paramsPtr, true),
    Infinity,
  );
  assert.equal(processor.alignUp(0x8000_0001, 16), 0x8000_0010);
  assert.equal(processor.process([[input]], [[output]]), true);
  assert.deepEqual([...output], [...input]);
});

test("AudioWorklet does not special-case parameters named freq as f32 scalars", () => {
  const scalarMir = f64PassthroughMir();
  scalarMir.interface.params[0].name = "freq";
  const scalarArtifact = compileMir(scalarMir);
  const scalarProcessor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: scalarArtifact.wasm,
      metadata: scalarArtifact.metadata,
    },
  });
  const scalarInfo = scalarArtifact.metadata.metadata.params[0];

  assert.equal(
    scalarProcessor.readStorage(
      scalarProcessor.paramsPtr + scalarInfo.byte_offset,
      scalarInfo,
    ),
    Infinity,
  );
  scalarProcessor.process(
    [[new Float32Array(4)]],
    [[new Float32Array(4)]],
  );
  assert.equal(
    scalarProcessor.readStorage(
      scalarProcessor.paramsPtr + scalarInfo.byte_offset,
      scalarInfo,
    ),
    Infinity,
  );

  const arrayMir = f64PassthroughMir();
  arrayMir.types.push({
    kind: "array",
    data: { element: 0, len: 2 },
  });
  arrayMir.interface.params[0] = {
    name: "freq",
    ty: 3,
    default: {
      kind: "aggregate",
      data: [
        { kind: "scalar", data: { type: "f64", value: 1.25 } },
        { kind: "scalar", data: { type: "f64", value: -2.5 } },
      ],
    },
    range: null,
  };
  const arrayArtifact = compileMir(arrayMir);
  const arrayProcessor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: arrayArtifact.wasm,
      metadata: arrayArtifact.metadata,
    },
  });
  const arrayInfo = arrayArtifact.metadata.metadata.params[0];

  assert.deepEqual(
    arrayProcessor.readStorage(
      arrayProcessor.paramsPtr + arrayInfo.byte_offset,
      arrayInfo,
    ),
    [1.25, -2.5],
  );
  arrayProcessor.process(
    [[new Float32Array(4)]],
    [[new Float32Array(4)]],
  );
  assert.deepEqual(
    arrayProcessor.readStorage(
      arrayProcessor.paramsPtr + arrayInfo.byte_offset,
      arrayInfo,
    ),
    [1.25, -2.5],
  );
});

test("AudioWorklet sets scalar and fixed-array params from metadata", () => {
  const mir = f64PassthroughMir();
  mir.interface.params[0] = {
    name: "gain",
    ty: 0,
    default: {
      kind: "scalar",
      data: { type: "f64", value: 0.5 },
    },
    range: null,
  };
  mir.types.push({
    kind: "array",
    data: { element: 1, len: 2 },
  });
  mir.interface.params.push({
    name: "steps",
    ty: 3,
    default: {
      kind: "aggregate",
      data: [
        { kind: "scalar", data: { type: "i32", value: 1 } },
        { kind: "scalar", data: { type: "i32", value: 2 } },
      ],
    },
    range: null,
  });
  const artifact = compileMir(mir);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
      params: { gain: 1.5 },
    },
  });
  const [gain, steps] = artifact.metadata.metadata.params;

  processor.setParam("gain", 2.25);
  processor.port.onmessage({
    data: {
      type: "set-param",
      param: "steps",
      value: new Int32Array([7, -8]),
    },
  });

  assert.equal(
    processor.readStorage(processor.paramsPtr + gain.byte_offset, gain),
    2.25,
  );
  assert.deepEqual(
    processor.readStorage(processor.paramsPtr + steps.byte_offset, steps),
    [7, -8],
  );
  assert.deepEqual(processor.port.messages, []);
});

test("AudioWorklet reset zeroes physical state before re-running init", () => {
  const mir = f64PassthroughMir();
  mir.interface.params[0] = {
    name: "initial",
    ty: 0,
    default: {
      kind: "scalar",
      data: { type: "f64", value: 1.5 },
    },
    range: null,
  };
  mir.state = [
    { name: "initialized", ty: 0, persistence: "snapshot" },
    { name: "untouched", ty: 0, persistence: "snapshot" },
  ];
  mir.functions[0].body.statements = [
    assign(place("state", 0), {
      kind: "load",
      data: place("param", 0),
    }),
  ];
  const artifact = compileMir(mir);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
      params: { initial: 2.5 },
    },
  });
  const [initialized, untouched] = artifact.metadata.metadata.states;
  const view = processor.memoryView();
  const initializedAddress =
    processor.statePtr + initialized.storage_byte_offset;
  const untouchedAddress =
    processor.statePtr + untouched.storage_byte_offset;

  assert.equal(view.getFloat64(initializedAddress, true), 2.5);
  assert.equal(view.getFloat64(untouchedAddress, true), 0);

  view.setFloat64(initializedAddress, 100, true);
  view.setFloat64(untouchedAddress, 200, true);
  processor.setParam("initial", 7.25);
  processor.blockCursor = 3;
  processor.port.onmessage({ data: { type: "reset" } });

  assert.equal(view.getFloat64(initializedAddress, true), 7.25);
  assert.equal(view.getFloat64(untouchedAddress, true), 0);
  assert.equal(processor.blockCursor, 0);
  assert.deepEqual(processor.port.messages, []);
});

test("AudioWorklet snapshots persistent state and restores from a post-init base", () => {
  const mir = f64PassthroughMir();
  mir.state = [
    { name: "remembered", ty: 0, persistence: "snapshot" },
    { name: "scratch", ty: 0, persistence: "instance_scratch" },
  ];
  mir.functions[0].body.statements = [
    assign(place("state", 1), {
      kind: "use",
      data: constant("f64", 9),
    }),
  ];
  const artifact = compileMir(mir);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });
  const remembered = artifact.metadata.metadata.states[0];
  const rememberedAddress = processor.statePtr + remembered.storage_byte_offset;
  const scratchAddress = processor.statePtr + 8;
  const view = processor.memoryView();

  view.setFloat64(rememberedAddress, 42.5, true);
  view.setFloat64(scratchAddress, 100, true);
  processor.port.onmessage({ data: { type: "snapshot", requestId: 7 } });
  const reply = processor.port.messages.at(-1);
  assert.equal(reply.type, "snapshot");
  assert.equal(reply.requestId, 7);
  assert.equal(new DataView(reply.bytes.buffer).getFloat64(0, true), 42.5);

  view.setFloat64(rememberedAddress, -1, true);
  view.setFloat64(scratchAddress, -2, true);
  processor.blockCursor = 3;
  processor.port.onmessage({
    data: { type: "restore-snapshot", requestId: 8, snapshot: reply.bytes },
  });
  assert.equal(view.getFloat64(rememberedAddress, true), 42.5);
  assert.equal(view.getFloat64(scratchAddress, true), 9);
  assert.equal(processor.blockCursor, 0);
  assert.deepEqual(processor.port.messages.at(-1), {
    type: "onda-ok",
    operation: "restore-snapshot",
    requestId: 8,
  });
});

test("AudioWorklet supplies the exact FMA support ABI", () => {
  const mir = f64PassthroughMir();
  const thenStatements =
    mir.functions[1].body.statements[2].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements[1].kind.data.value = {
    kind: "intrinsic",
    data: {
      intrinsic: "fma",
      args: [
        constant("f64", 1 + 2 ** -27),
        constant("f64", 1 - 2 ** -27),
        constant("f64", -1),
      ],
    },
  };
  const artifact = compileMir(mir);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });
  const output = new Float32Array(4);

  assert.equal(
    processor.process([[new Float32Array(4)]], [[output]]),
    true,
  );
  assert.deepEqual(
    [...output],
    Array.from({ length: 4 }, () => Math.fround(-(2 ** -54))),
  );
});

test("AudioWorklet can allocate valid state layouts larger than 16 MiB", () => {
  const mir = f64PassthroughMir();
  mir.types.push(
    { kind: "scalar", data: "f32" },
    { kind: "array", data: { element: 3, len: 5_000_000 } },
  );
  mir.state.push({
    name: "large_scratch",
    ty: 4,
    persistence: "instance_scratch",
  });
  const artifact = compileMir(mir);
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });

  assert.ok(artifact.metadata.runtime.state_size_bytes > 16 * 1024 * 1024);
  assert.ok(
    processor.memory.buffer.byteLength >=
      processor.statePtr + artifact.metadata.runtime.state_size_bytes,
  );
});

test("AudioWorklet segments arbitrary callback sizes across compile blocks", () => {
  const artifact = compileMir(f64PassthroughMir());
  const processor = new WorkletProcessor({
    processorOptions: {
      wasmBytes: artifact.wasm,
      metadata: artifact.metadata,
    },
  });
  const calls = [];
  const invoke = processor.invokeProcessSegment.bind(processor);
  processor.invokeProcessSegment = (startFrame, frames, flags) => {
    calls.push([startFrame, frames, flags]);
    return invoke(startFrame, frames, flags);
  };

  for (const values of [
    [0.25, -0.5],
    [0.75, 1, -1],
    [2, 3, 4, 5, 6, 7],
  ]) {
    const input = Float32Array.from(values);
    const output = new Float32Array(values.length);
    assert.equal(processor.process([[input]], [[output]]), true);
    assert.deepEqual([...output], [...input]);
  }

  assert.deepEqual(calls, [
    [0, 2, 1],
    [2, 2, 2],
    [0, 1, 1],
    [1, 3, 2],
    [0, 3, 1],
  ]);
  assert.equal(processor.blockCursor, 3);
});
