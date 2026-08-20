import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";
import binaryen from "binaryen";
import * as backend from "../src/index.js";
import { MIR_OPERATION_CAPABILITIES } from "../src/operations.js";

import {
  OndaBinaryenError,
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  compileTrustedMir as compileMir,
  createProcessorArtifactFiles,
  createDefaultImports,
  loadProcessorArtifactFiles,
  parseProcessorMetadata,
  validateProcessorArtifact,
} from "../src/index.js";

if (!globalThis.crypto) globalThis.crypto = webcrypto;

const unknownSource = {
  file: null,
  line: 0,
  column: 0,
  end_line: 0,
  end_column: 0,
};

const type = (kind, data) => ({ kind, data });
const value = (kind, data) => ({ kind, data });
const local = (id) => value("local", id);
const constant = (scalar, data) =>
  value("constant", { type: scalar, value: data });
const place = (kind, id) => ({
  base: { kind, data: id },
  projections: [],
});
const statement = (kind, data) => ({
  kind: data === undefined ? { kind } : { kind, data },
  source: unknownSource,
});
const attributes = (origin = "source", inline = "auto") => ({
  origin,
  inline,
});
const assign = (destination, rvalue) =>
  statement("assign", { destination, value: rvalue });

function emittedFunction(wat, name) {
  const start = wat.indexOf(`(func $${name}`);
  assert.notEqual(start, -1, `missing emitted function '${name}'`);
  const next = wat.indexOf("\n (func ", start + 1);
  return wat.slice(start, next === -1 ? wat.length : next);
}

function emittedParameterizedFunction(wat, name) {
  const start = wat.indexOf(`(func $${name} (param`);
  assert.notEqual(start, -1, `missing emitted function '${name}'`);
  const next = wat.indexOf("\n (func ", start + 1);
  return wat.slice(start, next === -1 ? wat.length : next);
}

function matchCount(text, pattern) {
  return text.match(pattern)?.length ?? 0;
}

function callProcess(
  process,
  inputs,
  outputs,
  startFrame,
  frames,
  flags,
  params,
  state,
  buffers,
  bufferFrames,
  bufferChannels,
  bufferSampleRates,
) {
  return process(
    state,
    params,
    inputs,
    outputs,
    startFrame,
    frames,
    flags,
    buffers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
}

function executableMir() {
  const processLoop = statement("loop", {
    body: {
      statements: [
        assign(place("local", 1), {
          kind: "compare",
          data: { op: "less", lhs: local(0), rhs: local(4) },
        }),
        statement("if", {
          condition: local(1),
          then_block: {
            statements: [
              assign(place("local", 2), {
                kind: "load",
                data: place("state", 0),
              }),
              assign(place("local", 3), {
                kind: "load",
                data: place("param", 0),
              }),
              assign(place("local", 2), {
                kind: "binary",
                data: { op: "add", lhs: local(2), rhs: local(3) },
              }),
              assign(place("state", 0), { kind: "use", data: local(2) }),
              assign(place("local", 5), {
                kind: "process_frame",
                data: { offset: local(0) },
              }),
              statement("output_store", {
                output: 0,
                element: null,
                bounds: "unchecked",
                frame: local(5),
                value: local(2),
              }),
              assign(place("local", 0), {
                kind: "binary",
                data: { op: "add", lhs: local(0), rhs: constant("i32", 1) },
              }),
            ],
          },
          else_block: { statements: [statement("break")] },
        }),
      ],
    },
  });

  return {
    schema_version: backend.SUPPORTED_MIR_SCHEMA_VERSION,
    config: { sample_rate: 48_000, block_size: 4 },
    source_files: [],
    types: [type("scalar", "f32"), type("scalar", "bool"), type("scalar", "i32")],
    structs: [],
    interface: {
      inputs: [],
      outputs: [{ name: "out1", ty: 0 }],
      control_outputs: [],
      params: [
        {
          name: "step",
          ty: 0,
          default: {
            kind: "scalar",
            data: { type: "f32", value: 0.25 },
          },
          range: null,
          control: {
            scale: "linear",
            curve: null,
            unit: null,
            step: null,
            step_count: null,
          },
        },
      ],
      buffers: [],
      events: [],
    },
    state: [{ name: "phase", ty: 0, persistence: "snapshot" }],
    const_data: [],
    functions: [
      {
        name: "onda_init",
        kind: { kind: "init" },
        attributes: attributes("compiler_generated", "always"),
        params: [],
        results: [],
        locals: [],
        body: {
          statements: [
            assign(place("state", 0), {
              kind: "use",
              data: constant("f32", 0),
            }),
          ],
        },
        source: unknownSource,
      },
      {
        name: "onda_process",
        kind: { kind: "process" },
        attributes: attributes("compiler_generated", "always"),
        params: [
          { name: "start_frame", ty: 2, mode: "value" },
          { name: "frames", ty: 2, mode: "value" },
          { name: "flags", ty: 2, mode: "value" },
        ],
        results: [],
        locals: [
          { name: "$frame", ty: 2 },
          { name: null, ty: 1 },
          { name: null, ty: 0 },
          { name: null, ty: 0 },
          { name: "$end_frame", ty: 2 },
          { name: "$logical_frame", ty: 2 },
        ],
        body: {
          statements: [
            assign(place("local", 0), {
              kind: "use",
              data: constant("i32", 0),
            }),
            assign(place("local", 4), {
              kind: "load",
              data: place("parameter", 1),
            }),
            assign(place("local", 4), {
              kind: "binary",
              data: {
                op: "add",
                lhs: local(4),
                rhs: constant("i32", 0),
              },
            }),
            processLoop,
          ],
        },
        source: unknownSource,
      },
    ],
    entry_points: { init: 0, process: 1 },
  };
}

function forwardedBufferCollectionMir(varyingSelector = false) {
  const mir = executableMir();
  mir.config.block_size = 128;
  const spanType = mir.types.length;
  mir.types.push(type("buffer_span", {
    element: "f32",
    channels: "dynamic",
    access: "read_write",
    len: 2,
  }));
  mir.interface.buffers.push(
    {
      name: "bank[0]",
      element: "f32",
      channels: "dynamic",
      access: "read_write",
    },
    {
      name: "bank[1]",
      element: "f32",
      channels: "dynamic",
      access: "read_write",
    },
  );
  const process = mir.functions[1];
  const selectorLocal = process.locals.length;
  const resultLocal = selectorLocal + 1;
  process.locals.push(
    { name: "$slot", ty: 2 },
    { name: "$buffer_sample", ty: 0 },
  );
  process.body.statements.splice(
    3,
    0,
    assign(place("local", selectorLocal), {
      kind: "use",
      data: constant("i32", 1),
    }),
  );
  const thenBlock =
    process.body.statements[4].kind.data.body.statements[1].kind.data.then_block;
  thenBlock.statements.unshift(
    statement("call", {
      results: [resultLocal],
      function: 2,
      args: [
        {
          kind: "place",
          data: place("local", varyingSelector ? 0 : selectorLocal),
        },
        {
          kind: "buffer_span",
          data: { kind: "interface", data: { first: 0, len: 2 } },
        },
        { kind: "value", data: local(0) },
      ],
    }),
  );
  const outputStore = thenBlock.statements.find(
    (entry) => entry.kind.kind === "output_store",
  );
  outputStore.kind.data.value = local(resultLocal);
  mir.functions.push({
    name: "read_forwarded_buffer",
    kind: { kind: "user" },
    attributes: attributes("compiler_generated", "always"),
    params: [
      { name: "slot", ty: 2, mode: "read_write_reference" },
      { name: "clips", ty: spanType, mode: "value" },
      { name: "frame", ty: 2, mode: "value" },
    ],
    results: [0],
    locals: [
      { name: null, ty: 2 },
      { name: null, ty: 2 },
      { name: null, ty: 0 },
    ],
    body: {
      statements: [
        assign(place("local", 0), {
          kind: "load",
          data: place("parameter", 0),
        }),
        assign(place("local", 1), {
          kind: "load",
          data: place("parameter", 2),
        }),
        assign(place("local", 2), {
          kind: "buffer_param_load",
          data: {
            parameter: {
              kind: "array_element",
              data: {
                span: 1,
                selector: local(0),
                bounds: "clamp",
              },
            },
            channel: constant("i32", 0),
            index: local(1),
            bounds: "clamp",
          },
        }),
        statement("return", { values: [local(2)] }),
      ],
    },
    source: unknownSource,
  });
  return mir;
}

test("does not expose a partial validator for arbitrary MIR", () => {
  assert.equal("compileMir" in backend, false);
});

test("declares the complete MIR scalar operation capability matrix", () => {
  assert.deepEqual(Object.keys(MIR_OPERATION_CAPABILITIES.binary), [
    "add",
    "subtract",
    "multiply",
    "divide",
    "remainder",
    "bit_and",
    "bit_or",
    "bit_xor",
    "shift_left",
    "shift_right",
  ]);
  assert.deepEqual(MIR_OPERATION_CAPABILITIES.binary.remainder, [
    "f32",
    "f64",
    "i32",
    "i64",
  ]);
});

test("compiles floating-point remainder through the embedded libm kernel", () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "f64"));
  mir.functions[0].locals = [
    { name: null, ty: 0 },
    { name: null, ty: 0 },
    { name: null, ty: 0 },
    { name: null, ty: 3 },
    { name: null, ty: 3 },
    { name: null, ty: 3 },
  ];
  mir.functions[0].body.statements.unshift(
    assign(place("local", 0), { kind: "use", data: constant("f32", 5.5) }),
    assign(place("local", 1), { kind: "use", data: constant("f32", 2) }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "remainder", lhs: local(0), rhs: local(1) },
    }),
    assign(place("local", 3), { kind: "use", data: constant("f64", 5.5) }),
    assign(place("local", 4), { kind: "use", data: constant("f64", 2) }),
    assign(place("local", 5), {
      kind: "binary",
      data: { op: "remainder", lhs: local(3), rhs: local(4) },
    }),
  );
  const artifact = compileMir(mir, { emitText: true, optimize: false });
  assert.ok(artifact.wasm.byteLength > 0);
  assert.match(artifact.wat, /onda_math_remainder_f32/);
  assert.match(artifact.wat, /onda_math_remainder_f64/);
});

test("compiles versioned MIR into an executable persistent DSP module", async () => {
  const artifact = compileMir(executableMir(), { emitText: true });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.match(artifact.wat, /export "onda_process"/);
  assert.equal(artifact.metadata.backend, "binaryen-js");
  assert.equal(artifact.metadata.format, "onda-processor");
  assert.equal(
    artifact.metadata.format_version,
    backend.PROCESSOR_ARTIFACT_FORMAT_VERSION,
  );
  assert.equal(artifact.metadata.abi_version, backend.PROCESSOR_ABI_VERSION);
  assert.equal(
    artifact.metadata.runtime.snapshot_format_version,
    backend.PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  );
  assert.equal(artifact.metadata.artifact_kind, "webassembly_module");
  assert.equal(artifact.metadata.target.pointer_width_bits, 32);
  assert.equal(artifact.metadata.target.pointer_model, "linear_memory_offset");
  assert.equal(
    artifact.metadata.integration.profile.kind,
    "core_webassembly_module",
  );
  assert.equal(artifact.metadata.runtime.state_size_bytes, 16);
  assert.equal(artifact.metadata.runtime.snapshot_size_bytes, 4);
  assert.deepEqual(artifact.metadata.metadata.states, [
    {
      name: "phase",
      type_repr: "f32",
      scalar: "f32",
      array_len: 1,
      element_size_bytes: 4,
      packed_snapshot_byte_offset: 0,
      physical_state_byte_offset: 0,
      byte_size: 4,
      integer_range: null,
    },
  ]);
  assert.equal(artifact.metadata.runtime.requires_full_blocks, false);

  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const outputTable = heap;
  heap += 4;
  const output = heap;

  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  assert.equal(onda_init(params, state), 0);
  assert.equal(onda_process.length, 11);
  assert.equal(
    callProcess(onda_process, 0, outputTable, 0, 2, 1, params, state, 0, 0, 0, 0),
    0,
  );
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0, 0],
  );
  callProcess(onda_process, 0, outputTable, 2, 2, 2, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0.75, 1],
  );

  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [1.25, 1.5, 1.75, 2],
  );
});

test("preserves signed zero in processor metadata", () => {
  const mir = executableMir();
  mir.interface.params[0].default.data.value = -0;
  mir.interface.params[0].range = {
    min: { type: "f32", value: -0 },
    max: { type: "f32", value: 1 },
  };

  const artifact = compileMir(mir);
  const [param] = artifact.metadata.metadata.params;
  assert.deepEqual(param.default_reprs, ["-0"]);
  assert.equal(param.range_min_repr, "-0");
  assert.equal(param.range_max_repr, "1");
});

test("serializes a reusable Wasm artifact with integrity metadata", async () => {
  const artifact = compileMir(executableMir());
  assert.equal(validateProcessorArtifact(artifact).metadata, artifact.metadata);
  const files = await createProcessorArtifactFiles(artifact, {
    baseName: "test processor",
  });
  assert.equal(files.wasm.name, "test-processor.wasm");
  assert.equal(files.metadata.name, "test-processor.onda.json");
  assert.deepEqual(files.wasm.bytes, artifact.wasm);
  assert.match(files.metadata.value.integrity.wasm, /^[0-9a-f]{64}$/);
  assert.equal(
    parseProcessorMetadata(files.metadata.text, "webassembly_module").format,
    "onda-processor",
  );
  assert.deepEqual(
    (await loadProcessorArtifactFiles(files.wasm.bytes, files.metadata.text)).wasm,
    artifact.wasm,
  );
  const tampered = files.wasm.bytes.slice();
  tampered[tampered.length - 1] ^= 1;
  await assert.rejects(
    () => loadProcessorArtifactFiles(tampered, files.metadata.text),
    /integrity mismatch|not valid WebAssembly/,
  );
});

test("passes an addressable slice element to a reference parameter", async () => {
  const mir = executableMir();
  mir.types.push(
    type("array", { element: 0, len: 2 }),
    type("slice", { element: "f32", access: "read_write" }),
  );
  mir.state.push({ name: "values", ty: 3, persistence: "snapshot" });
  mir.functions[1].locals.push({ name: "values_view", ty: 4 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "make_slice",
      data: {
        source: { kind: "place", data: place("state", 1) },
        start: constant("i32", 0),
        len: constant("i32", 2),
        bounds: "unchecked",
        access: "read_write",
      },
    }),
    statement("call", {
      results: [],
      function: 2,
      args: [
        {
          kind: "slice_element",
          data: {
            slice: local(6),
            index: constant("i32", 1),
            bounds: "clamp",
          },
        },
      ],
    }),
  );
  mir.functions.push({
    name: "set_value",
    kind: { kind: "user" },
    attributes: attributes(),
    params: [{ name: "value", ty: 0, mode: "read_write_reference" }],
    results: [],
    locals: [],
    body: {
      statements: [
        assign(place("parameter", 0), {
          kind: "use",
          data: constant("f32", 7),
        }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getFloat32(state + 8, true), 7);
});

test("spills an address-taken scalar local around reference calls", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "promoted_phase", ty: 0 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "load",
      data: place("state", 0),
    }),
    statement("call", {
      results: [6],
      function: 2,
      args: [{ kind: "place", data: place("local", 6) }],
    }),
    assign(place("state", 0), { kind: "use", data: local(6) }),
  );
  mir.functions.push({
    name: "update_promoted_phase",
    kind: { kind: "user" },
    attributes: attributes(),
    params: [{ name: "phase", ty: 0, mode: "read_write_reference" }],
    results: [0],
    locals: [],
    body: {
      statements: [
        assign(place("parameter", 0), {
          kind: "use",
          data: constant("f32", 7),
        }),
        statement("return", { values: [constant("f32", 9)] }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  instance.exports.onda_init(params, state);
  callProcess(instance.exports.onda_process, 0, 0, 0, 0, 0, params, state, 0, 0, 0, 0);
  assert.equal(new DataView(instance.exports.memory.buffer).getFloat32(state, true), 9);
});

test("does not promote a scalar reference that aliases a writable argument", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "aliased_value", ty: 0 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "use",
      data: constant("f32", 1),
    }),
    statement("call", {
      results: [6],
      function: 2,
      args: [
        { kind: "place", data: place("local", 6) },
        { kind: "place", data: place("local", 6) },
      ],
    }),
    assign(place("state", 0), { kind: "use", data: local(6) }),
  );
  mir.functions.push({
    name: "write_then_read_alias",
    kind: { kind: "user" },
    attributes: attributes(),
    params: [
      { name: "read", ty: 0, mode: "read_only_reference" },
      { name: "write", ty: 0, mode: "read_write_reference" },
    ],
    results: [0],
    locals: [{ name: "result", ty: 0 }],
    body: {
      statements: [
        assign(place("parameter", 1), {
          kind: "use",
          data: constant("f32", 7),
        }),
        assign(place("local", 0), {
          kind: "load",
          data: place("parameter", 0),
        }),
        statement("return", { values: [local(0)] }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  instance.exports.onda_init(params, state);
  callProcess(instance.exports.onda_process, 0, 0, 0, 0, 0, params, state, 0, 0, 0, 0);
  assert.equal(new DataView(instance.exports.memory.buffer).getFloat32(state, true), 7);
});

test("does not promote a scalar reference written through a transitive call", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "forwarded_value", ty: 0 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "use",
      data: constant("f32", 1),
    }),
    statement("call", {
      results: [],
      function: 2,
      args: [{ kind: "place", data: place("local", 6) }],
    }),
    assign(place("state", 0), { kind: "use", data: local(6) }),
  );
  mir.functions.push(
    {
      name: "forward_write",
      kind: { kind: "user" },
      attributes: attributes(),
      params: [{ name: "value", ty: 0, mode: "read_write_reference" }],
      results: [],
      locals: [],
      body: {
        statements: [
          statement("call", {
            results: [],
            function: 3,
            args: [{ kind: "place", data: place("parameter", 0) }],
          }),
        ],
      },
      source: unknownSource,
    },
    {
      name: "write_forwarded_value",
      kind: { kind: "user" },
      attributes: attributes(),
      params: [{ name: "value", ty: 0, mode: "read_write_reference" }],
      results: [],
      locals: [],
      body: {
        statements: [
          assign(place("parameter", 0), {
            kind: "use",
            data: constant("f32", 7),
          }),
        ],
      },
      source: unknownSource,
    },
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  instance.exports.onda_init(params, state);
  callProcess(instance.exports.onda_process, 0, 0, 0, 0, 0, params, state, 0, 0, 0, 0);
  assert.equal(new DataView(instance.exports.memory.buffer).getFloat32(state, true), 7);
});

test("vectorizes contiguous slice fills with a scalar tail", async () => {
  const mir = executableMir();
  mir.types.push(
    type("array", { element: 0, len: 10 }),
    type("slice", { element: "f32", access: "read_write" }),
  );
  mir.state.push({ name: "values", ty: 3, persistence: "snapshot" });
  mir.functions[1].locals.push({ name: "values_view", ty: 4 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "make_slice",
      data: {
        source: { kind: "place", data: place("state", 1) },
        start: constant("i32", 0),
        len: constant("i32", 10),
        bounds: "unchecked",
        access: "read_write",
      },
    }),
    statement("slice_fill", {
      destination: local(6),
      value: constant("f32", 3.5),
    }),
  );

  const vectorized = compileMir(mir, { emitText: true });
  const scalar = compileMir(mir, { emitText: true, simd: false });
  assert.match(vectorized.wat, /v128\.store/);
  assert.doesNotMatch(scalar.wat, /v128\.store/);

  const { instance } = await WebAssembly.instantiate(vectorized.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + vectorized.metadata.runtime.param_size_bytes;
  instance.exports.onda_init(params, state);
  callProcess(instance.exports.onda_process, 0, 0, 0, 0, 0, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(instance.exports.memory.buffer, state + 4, 10)],
    Array.from({ length: 10 }, () => 3.5),
  );
});

test("returns failures for invalid checked make_slice and empty-slice access", async () => {
  const makeMir = ({ start, len, bounds, load }) => {
    const mir = executableMir();
    mir.types.push(
      type("array", { element: 0, len: 4 }),
      type("slice", { element: "f32", access: "read_write" }),
    );
    mir.state.push(
      { name: "values", ty: 3, persistence: "snapshot" },
      { name: "slice_len", ty: 2, persistence: "snapshot" },
    );
    mir.functions[1].locals.push(
      { name: "view", ty: 4 },
      { name: "loaded", ty: 0 },
    );
    const statements = [
      assign(place("local", 6), {
        kind: "make_slice",
        data: {
          source: { kind: "place", data: place("state", 1) },
          start: constant("i32", start),
          len: constant("i32", len),
          bounds,
          access: "read_write",
        },
      }),
      assign(place("state", 2), {
        kind: "slice_len",
        data: local(6),
      }),
    ];
    if (load) {
      statements.push(
        assign(place("local", 7), {
          kind: "slice_load",
          data: {
            slice: local(6),
            index: constant("i32", 0),
            bounds: "clamp",
          },
        }),
      );
    }
    const thenStatements =
      mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
        .then_block.statements;
    thenStatements.unshift(...statements);
    return mir;
  };

  const clamped = compileMir(
    makeMir({ start: -2, len: 99, bounds: "clamp", load: false }),
  );
  const { instance: clampedInstance } = await WebAssembly.instantiate(
    clamped.wasm,
  );
  const clampedParams = Number(clampedInstance.exports.__heap_base.value);
  const clampedState =
    clampedParams + clamped.metadata.runtime.param_size_bytes;
  const clampedOutputTable =
    clampedState + clamped.metadata.runtime.state_size_bytes;
  const clampedOutput = clampedOutputTable + 4;
  const clampedView = new DataView(clampedInstance.exports.memory.buffer);
  clampedView.setUint32(clampedOutputTable, clampedOutput, true);
  clampedInstance.exports.onda_init(clampedParams, clampedState);
  callProcess(clampedInstance.exports.onda_process,
    0,
    clampedOutputTable,
    0,
    1,
    3,
    clampedParams,
    clampedState,
    0,
    0,
    0,
    0,
  );
  assert.equal(clampedView.getInt32(clampedState + 20, true), 4);

  for (const mir of [
    makeMir({ start: 5, len: 0, bounds: "checked", load: false }),
    makeMir({ start: 4, len: 0, bounds: "checked", load: true }),
  ]) {
    const artifact = compileMir(mir);
    const { instance } = await WebAssembly.instantiate(artifact.wasm);
    const params = Number(instance.exports.__heap_base.value);
    const state = params + artifact.metadata.runtime.param_size_bytes;
    const outputTable = state + artifact.metadata.runtime.state_size_bytes;
    new DataView(instance.exports.memory.buffer).setUint32(
      outputTable,
      outputTable + 4,
      true,
    );
    instance.exports.onda_init(params, state);
    assert.equal(
      callProcess(instance.exports.onda_process,
          0,
          outputTable,
          0,
          1,
          3,
          params,
          state,
          0,
          0,
          0,
          0,
        ),
      PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
    );
  }
});

test("returns generated failures through nested MIR calls", async () => {
  const mir = executableMir();
  const process = mir.functions[mir.entry_points.process];
  const resultLocal = process.locals.length;
  process.locals.push({ name: "quotient", ty: 1 });
  process.body.statements.unshift(
    statement("call", {
      results: [resultLocal],
      function: mir.functions.length,
      args: [],
    }),
  );
  mir.functions.push({
    name: "failing_quotient",
    kind: { kind: "user" },
    attributes: { origin: "source", inline: "never" },
    params: [],
    results: [1],
    locals: [{ name: "result", ty: 1 }],
    body: {
      statements: [
        assign(place("local", 0), {
          kind: "binary",
          data: {
            op: "divide",
            lhs: constant("i32", 1),
            rhs: constant("i32", 0),
          },
        }),
        statement("return", { values: [local(0)] }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  assert.equal(instance.exports.onda_init(params, state), 0);
  assert.equal(
    callProcess(
      instance.exports.onda_process,
      0,
      0,
      0,
      0,
      0,
      params,
      state,
      0,
      0,
      0,
      0,
    ),
    PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  );
});

test("omits failure propagation after non-failing helper calls", () => {
  const mir = executableMir();
  const init = mir.functions[mir.entry_points.init];
  const arrayType = mir.types.length;
  mir.types.push(type("array", { element: 0, len: 4 }));
  const floatResult = init.locals.length;
  const clampResult = floatResult + 1;
  init.locals.push(
    { name: "quotient", ty: 0 },
    { name: "clamped", ty: 0 },
  );
  const floatFunction = mir.functions.length;
  const clampFunction = floatFunction + 1;
  init.body.statements.unshift(
    statement("call", {
      results: [floatResult],
      function: floatFunction,
      args: [],
    }),
    statement("call", {
      results: [clampResult],
      function: clampFunction,
      args: [],
    }),
  );
  const clampedElement = place("local", 1);
  clampedElement.projections.push({
    kind: "index",
    data: { index: constant("i32", 9), bounds: "clamp" },
  });
  mir.functions.push(
    {
      name: "floating_quotient",
      kind: { kind: "user" },
      attributes: { origin: "source", inline: "never" },
      params: [],
      results: [0],
      locals: [{ name: "result", ty: 0 }],
      body: {
        statements: [
          assign(place("local", 0), {
            kind: "binary",
            data: {
              op: "divide",
              lhs: constant("f32", 1),
              rhs: constant("f32", 2),
            },
          }),
          statement("return", { values: [local(0)] }),
        ],
      },
      source: unknownSource,
    },
    {
      name: "clamped_array_element",
      kind: { kind: "user" },
      attributes: { origin: "source", inline: "never" },
      params: [],
      results: [0],
      locals: [
        { name: "result", ty: 0 },
        { name: "values", ty: arrayType },
      ],
      body: {
        statements: [
          assign(place("local", 0), {
            kind: "load",
            data: clampedElement,
          }),
          statement("return", { values: [local(0)] }),
        ],
      },
      source: unknownSource,
    },
  );

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  const failureReads = artifact.wat.match(
    /\(global\.get \$+onda\.runtime_failure\)/g,
  ) ?? [];
  assert.equal(failureReads.length, 1);
});

test("propagates checked fixed-array failures through helper calls", async () => {
  const mir = executableMir();
  const arrayType = mir.types.length;
  mir.types.push(type("array", { element: 0, len: 4 }));
  const process = mir.functions[mir.entry_points.process];
  const resultLocal = process.locals.length;
  process.locals.push({ name: "checked", ty: 0 });
  process.body.statements.unshift(
    statement("call", {
      results: [resultLocal],
      function: mir.functions.length,
      args: [],
    }),
  );
  const checkedElement = place("local", 1);
  checkedElement.projections.push({
    kind: "index",
    data: { index: constant("i32", 9), bounds: "checked" },
  });
  mir.functions.push({
    name: "checked_array_element",
    kind: { kind: "user" },
    attributes: { origin: "source", inline: "never" },
    params: [],
    results: [0],
    locals: [
      { name: "result", ty: 0 },
      { name: "values", ty: arrayType },
    ],
    body: {
      statements: [
        assign(place("local", 0), {
          kind: "load",
          data: checkedElement,
        }),
        statement("return", { values: [local(0)] }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const params = Number(instance.exports.__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  assert.equal(instance.exports.onda_init(params, state), 0);
  assert.equal(
    callProcess(
      instance.exports.onda_process,
      0,
      0,
      0,
      0,
      0,
      params,
      state,
      0,
      0,
      0,
      0,
    ),
    PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  );
});

test("lowers current-schema fixed-array and slice reference windows", async () => {
  const mir = executableMir();
  mir.types.push(
    type("array", { element: 0, len: 4 }),
    type("array", { element: 0, len: 2 }),
    type("slice", { element: "f32", access: "read_write" }),
  );
  mir.state.push({ name: "values", ty: 3, persistence: "snapshot" });
  mir.functions[1].locals.push({ name: "view", ty: 5 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "make_slice",
      data: {
        source: { kind: "place", data: place("state", 1) },
        start: constant("i32", 0),
        len: constant("i32", 4),
        bounds: "unchecked",
        access: "read_write",
      },
    }),
    statement("call", {
      results: [],
      function: 2,
      args: [
        {
          kind: "array_window",
          data: {
            array: place("state", 1),
            start: constant("i32", 0),
            bounds: "checked",
          },
        },
      ],
    }),
    statement("call", {
      results: [],
      function: 2,
      args: [
        {
          kind: "slice_window",
          data: {
            slice: local(6),
            start: constant("i32", 2),
            bounds: "checked",
          },
        },
      ],
    }),
  );
  const first = place("parameter", 0);
  first.projections.push({
    kind: "index",
    data: { index: constant("i32", 0), bounds: "unchecked" },
  });
  const second = place("parameter", 0);
  second.projections.push({
    kind: "index",
    data: { index: constant("i32", 1), bounds: "unchecked" },
  });
  mir.functions.push({
    name: "write_pair",
    kind: { kind: "user" },
    attributes: attributes(),
    params: [
      {
        name: "values",
        ty: 4,
        mode: "read_write_reference",
      },
    ],
    results: [],
    locals: [],
    body: {
      statements: [
        assign(first, {
          kind: "use",
          data: constant("f32", 7),
        }),
        assign(second, {
          kind: "use",
          data: constant("f32", 8),
        }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, state + 4, 4)],
    [7, 8, 7, 8],
  );
});

test("rejects incompatible MIR schema versions before code generation", () => {
  const mir = executableMir();
  const incompatibleVersion = backend.SUPPORTED_MIR_SCHEMA_VERSION + 1;
  mir.schema_version = incompatibleVersion;
  assert.throws(
    () => compileMir(mir),
    (error) =>
      error instanceof OndaBinaryenError &&
      error.message ===
        `unsupported MIR schema version ${incompatibleVersion}; expected ${backend.SUPPORTED_MIR_SCHEMA_VERSION}`,
  );
});

test("uses an explicit Binaryen O4 speed policy by default", () => {
  const artifact = compileMir(executableMir());
  assert.deepEqual(artifact.metadata.optimization, {
    enabled: true,
    level: 4,
    shrink_level: 0,
    fast_math: false,
    simd: true,
    inline_functions_with_loops: false,
  });

  const custom = compileMir(executableMir(), {
    optimizeLevel: 2,
    shrinkLevel: 1,
  });
  assert.deepEqual(custom.metadata.optimization, {
    enabled: true,
    level: 2,
    shrink_level: 1,
    fast_math: false,
    simd: true,
    inline_functions_with_loops: false,
  });
  assert.throws(
    () => compileMir(executableMir(), { optimizeLevel: 5 }),
    /optimizeLevel must be an integer from 0 through 4/,
  );
});

test("restores process-global Binaryen optimization policy", () => {
  const previousFastMath = binaryen.getFastMath();
  const previousLoopInlining = binaryen.getAllowInliningFunctionsWithLoops();
  binaryen.setFastMath(true);
  binaryen.setAllowInliningFunctionsWithLoops(true);
  try {
    const artifact = compileMir(executableMir(), {
      fastMath: false,
      allowInliningFunctionsWithLoops: false,
    });
    assert.equal(artifact.metadata.optimization.fast_math, false);
    assert.equal(
      artifact.metadata.optimization.inline_functions_with_loops,
      false,
    );
    assert.equal(binaryen.getFastMath(), true);
    assert.equal(binaryen.getAllowInliningFunctionsWithLoops(), true);
  } finally {
    binaryen.setFastMath(previousFastMath);
    binaryen.setAllowInliningFunctionsWithLoops(previousLoopInlining);
  }
});

test("rejects audio I/O frames not produced by process_frame", () => {
  const mir = executableMir();
  const loop = mir.functions[1].body.statements[3];
  const thenStatements =
    loop.kind.data.body.statements[1].kind.data.then_block.statements;
  const outputStore = thenStatements.find(
    (entry) => entry.kind.kind === "output_store",
  );
  outputStore.kind.data.frame = constant("i32", -1);
  assert.throws(
    () => compileMir(mir),
    /audio output store frame must come directly from process_frame/,
  );
});

test("rejects noncanonical current-schema process entry signatures", () => {
  const mutations = [
    {
      change: (mir) => mir.functions[1].params.pop(),
      message: /exactly three parameters/,
    },
    {
      change: (mir) => { mir.functions[1].params[0].name = "frames"; },
      message: /parameter 0 must be named 'start_frame'/,
    },
    {
      change: (mir) => { mir.functions[1].params[1].mode = "read_only_reference"; },
      message: /parameter 'frames' must use value passing mode/,
    },
    {
      change: (mir) => { mir.functions[1].params[2].ty = 0; },
      message: /parameter 'flags' must have type i32/,
    },
    {
      change: (mir) => { mir.functions[1].results = [2]; },
      message: /must not return values/,
    },
  ];

  for (const { change, message } of mutations) {
    const mir = executableMir();
    change(mir);
    assert.throws(() => compileMir(mir), message);
  }
});

test("rejects block sizes outside the signed process ABI", () => {
  const mir = executableMir();
  mir.config.block_size = 0x8000_0000;
  assert.throws(() => compileMir(mir), /fit the signed i32 process ABI/);
});

test("rejects compile-time layouts that exceed the wasm32 address space", () => {
  const stateMir = executableMir();
  stateMir.types.push(type("array", { element: 0, len: 500_000_000 }));
  stateMir.state = ["a", "b", "c"].map((name) => ({
    name,
    ty: 3,
    persistence: "snapshot",
  }));
  stateMir.functions[0].body.statements = [];
  stateMir.functions[1].body.statements = [];
  assert.throws(
    () => compileMir(stateMir),
    /physical state storage must fit within the wasm32 4 GiB address space/,
  );

  const audioMir = executableMir();
  audioMir.config.block_size = 0x4000_0000;
  assert.throws(
    () => compileMir(audioMir),
    /audio port 0 channel storage must fit within the wasm32 4 GiB address space/,
  );

  const combinedMir = executableMir();
  combinedMir.types.push(
    type("array", { element: 0, len: 600_000_000 }),
    type("array", { element: 0, len: 500_000_000 }),
  );
  combinedMir.state = [{
    name: "large_state",
    ty: 3,
    persistence: "snapshot",
  }];
  combinedMir.functions[0].locals = [{ name: null, ty: 4 }];
  combinedMir.functions[0].body.statements = [];
  combinedMir.functions[1].body.statements = [];
  assert.throws(
    () => compileMir(combinedMir),
    /static, parameter, and physical state storage must fit within the wasm32 4 GiB address space/,
  );
});

test("rejects recursive MIR call graphs as unbounded realtime work", () => {
  const mir = executableMir();
  const userFunction = (name, callee) => ({
    name,
    kind: { kind: "user" },
    attributes: attributes(),
    params: [],
    results: [],
    locals: [],
    body: {
      statements: [
        statement("call", {
          results: [],
          function: callee,
          args: [],
        }),
      ],
    },
    source: unknownSource,
  });
  mir.functions.push(userFunction("first", 3), userFunction("second", 2));

  assert.throws(
    () => compileMir(mir),
    /recursive call cycle is not realtime-safe: first -> second -> first/,
  );
});

test("preserves current-schema i64 and non-finite constants exactly", async () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "i64"));
  mir.functions[0].locals.push({ name: "wide", ty: 3 });
  mir.functions[0].body.statements.unshift(
    assign(place("local", 0), {
      kind: "use",
      data: constant("i64", "9223372036854775807"),
    }),
  );
  mir.const_data.push(
    {
      name: "wide_values",
      element: "i64",
      values: [
        { type: "i64", value: "-9223372036854775808" },
        { type: "i64", value: "9223372036854775807" },
      ],
    },
    {
      name: "special_f32",
      element: "f32",
      values: [{ type: "f32", value: "0xffc01234" }],
    },
    {
      name: "special_f64",
      element: "f64",
      values: [{ type: "f64", value: "0x7ff0000000000000" }],
    },
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const view = new DataView(instance.exports.memory.buffer);
  assert.equal(view.getBigInt64(1024, true), -(1n << 63n));
  assert.equal(view.getBigInt64(1032, true), (1n << 63n) - 1n);
  assert.equal(view.getUint32(1040, true), 0xffc01234);
  assert.equal(view.getBigUint64(1048, true), 0x7ff0000000000000n);
});

test("rejects lossy numeric current-schema i64 constants", () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "i64"));
  mir.functions[0].locals.push({ name: "wide", ty: 3 });
  mir.functions[0].body.statements.unshift(
    assign(place("local", 0), {
      kind: "use",
      data: constant("i64", Number.MAX_SAFE_INTEGER),
    }),
  );
  assert.throws(
    () => compileMir(mir),
    new RegExp(
      `schema ${backend.SUPPORTED_MIR_SCHEMA_VERSION} i64 values must be canonical decimal strings`,
    ),
  );
});

test("links the complete LLVM math surface into a self-contained module", async () => {
  const mir = executableMir();
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  const intrinsic = (name, args) => ({
    kind: "intrinsic",
    data: { intrinsic: name, args },
  });
  const x = local(3);
  const half = constant("f32", 0.5);
  const terms = [
    intrinsic("sin", [x]),
    intrinsic("cos", [x]),
    intrinsic("tan", [x]),
    intrinsic("tanh", [x]),
    intrinsic("atan", [x]),
    intrinsic("atan2", [x, half]),
    intrinsic("exp", [x]),
    intrinsic("log", [constant("f32", 1.25)]),
    intrinsic("sqrt", [x]),
    intrinsic("pow", [x, constant("f32", 2)]),
    intrinsic("abs", [constant("f32", -0.25)]),
    intrinsic("floor", [x]),
    intrinsic("ceil", [x]),
    intrinsic("round", [x]),
    intrinsic("trunc", [x]),
    intrinsic("min", [x, half]),
    intrinsic("max", [x, half]),
    intrinsic("fma", [x, constant("f32", 2), constant("f32", 0.125)]),
  ];
  const firstMathLocal = mir.functions[1].locals.length;
  mir.functions[1].locals.push(
    ...terms.map((_, index) => ({ name: `math_${index}`, ty: 0 })),
  );
  const mathStatements = terms.map((term, index) =>
    assign(place("local", firstMathLocal + index), term));
  const sumStatements = [
    assign(place("local", 2), {
      kind: "use",
      data: local(firstMathLocal),
    }),
    ...terms.slice(1).map((_, index) =>
      assign(place("local", 2), {
        kind: "binary",
        data: {
          op: "add",
          lhs: local(2),
          rhs: local(firstMathLocal + index + 1),
        },
      })),
  ];
  thenStatements.splice(2, 1, ...mathStatements, ...sumStatements);
  const artifact = compileMir(mir);
  assert.deepEqual(artifact.metadata.integration.profile.imports, []);
  assert.deepEqual(
    WebAssembly.Module.imports(new WebAssembly.Module(artifact.wasm)),
    [],
  );

  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  const expectedTerms = [
    Math.sin(0.25),
    Math.cos(0.25),
    Math.tan(0.25),
    Math.tanh(0.25),
    Math.atan(0.25),
    Math.atan2(0.25, 0.5),
    Math.exp(0.25),
    Math.log(1.25),
    Math.sqrt(0.25),
    0.25 ** 2,
    Math.abs(-0.25),
    Math.floor(0.25),
    Math.ceil(0.25),
    0,
    Math.trunc(0.25),
    Math.min(0.25, 0.5),
    Math.max(0.25, 0.5),
    0.25 * 2 + 0.125,
  ].map(Math.fround);
  const expected = expectedTerms.reduce(
    (sum, term) => Math.fround(sum + term),
  );
  const actual = new Float32Array(memory.buffer, output, 4)[0];
  assert.ok(Math.abs(actual - expected) < 2e-6, `${actual} != ${expected}`);
});

test("implements LLVM half-away-from-zero round without reserving the math kernel", async () => {
  for (const scalar of ["f32", "f64"]) {
    const mir = executableMir();
    if (scalar === "f64") {
      mir.types[0] = type("scalar", "f64");
      mir.interface.params[0].default.data = { type: "f64", value: 0 };
      mir.functions[0].body.statements[0].kind.data.value = {
        kind: "use",
        data: constant("f64", 0),
      };
    }
    const thenStatements =
      mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
        .then_block.statements;
    thenStatements[2].kind.data.value = {
      kind: "intrinsic",
      data: { intrinsic: "round", args: [local(3)] },
    };

    const artifact = compileMir(mir);
    assert.deepEqual(artifact.metadata.integration.profile.imports, []);
    const { instance } = await WebAssembly.instantiate(artifact.wasm);
    const { memory, __heap_base, onda_init, onda_process } = instance.exports;
    const params = Number(__heap_base.value);
    assert.ok(params < 32 * 1024, "round alone must not reserve the software kernel");
    const state = params + 16;
    const outputTable = state + artifact.metadata.runtime.state_size_bytes;
    const elementSize = scalar === "f32" ? 4 : 8;
    const output = Math.ceil((outputTable + 4) / elementSize) * elementSize;
    const view = new DataView(memory.buffer);
    view.setUint32(outputTable, output, true);
    onda_init(params, state);

    const nearHalf = scalar === "f32" ? Math.fround(0.5 - 2 ** -25) : 0.5 - 2 ** -54;
    for (const input of [
      -1.5,
      -0.5,
      -nearHalf,
      -0,
      nearHalf,
      0.5,
      1.5,
      scalar === "f32" ? 2 ** 23 : 2 ** 52,
    ]) {
      if (scalar === "f32") view.setFloat32(params, input, true);
      else view.setFloat64(params, input, true);
      callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
      const actual = scalar === "f32"
        ? view.getFloat32(output, true)
        : view.getFloat64(output, true);
      const expected = input < 0 ? -Math.round(-input) : Math.round(input);
      assert.ok(Object.is(actual, expected), `${scalar}: round(${input}) = ${actual}`);
    }
  }
});

test("links strict f32 and f64 FMA into the generated Wasm module", async () => {
  for (const scalar of ["f32", "f64"]) {
    const mir = executableMir();
    if (scalar === "f64") {
      mir.types[0] = type("scalar", "f64");
      mir.interface.params[0].default.data = {
        type: "f64",
        value: 0.25,
      };
      mir.functions[0].body.statements[0].kind.data.value = {
        kind: "use",
        data: constant("f64", 0),
      };
    }
    const thenStatements =
      mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
        .then_block.statements;
    const operands = scalar === "f32"
      ? [1 + 2 ** -12, 1 - 2 ** -12, -1]
      : [1 + 2 ** -27, 1 - 2 ** -27, -1];
    thenStatements[2].kind.data.value = {
      kind: "intrinsic",
      data: {
        intrinsic: "fma",
        args: operands.map((operand) => constant(scalar, operand)),
      },
    };

    const artifact = compileMir(mir);
    assert.deepEqual(artifact.metadata.integration.profile.imports, []);
    const imports = WebAssembly.Module.imports(
      new WebAssembly.Module(artifact.wasm),
    );
    assert.deepEqual(imports, []);

    const { instance } = await WebAssembly.instantiate(artifact.wasm);
    const { memory, __heap_base, onda_init, onda_process } = instance.exports;
    const params = Number(__heap_base.value);
    const state = params + 16;
    const outputTable = state + artifact.metadata.runtime.state_size_bytes;
    const elementSize = scalar === "f32" ? 4 : 8;
    const output = Math.ceil((outputTable + 4) / elementSize) * elementSize;
    const view = new DataView(memory.buffer);
    if (scalar === "f32") view.setFloat32(params, 0.25, true);
    else view.setFloat64(params, 0.25, true);
    view.setUint32(outputTable, output, true);
    onda_init(params, state);
    callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);

    if (scalar === "f32") {
      assert.equal(view.getUint32(output, true), 0xb3800000);
    } else {
      assert.equal(view.getBigUint64(output, true), 0xbc90000000000000n);
    }
  }
});

test("lowers signed integer abs, min, and max intrinsics", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "integer", ty: 2 });
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.splice(
    3,
    0,
    assign(place("local", 6), {
      kind: "intrinsic",
      data: {
        intrinsic: "min",
        args: [constant("i32", 7), constant("i32", -3)],
      },
    }),
    assign(place("local", 6), {
      kind: "intrinsic",
      data: {
        intrinsic: "max",
        args: [local(6), constant("i32", -2)],
      },
    }),
    assign(place("local", 6), {
      kind: "intrinsic",
      data: { intrinsic: "abs", args: [local(6)] },
    }),
    assign(place("local", 3), {
      kind: "cast",
      data: { value: local(6), to: "f32" },
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(3) },
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [2.25, 4.5, 6.75, 9],
  );
});

test("implements MIR wrapping semantics for signed division overflow", async () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "i64"));
  mir.state.push(
    { name: "wrapped_i32", ty: 2, persistence: "snapshot" },
    { name: "wrapped_i64", ty: 3, persistence: "snapshot" },
  );
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.unshift(
    assign(place("state", 1), {
      kind: "binary",
      data: {
        op: "divide",
        lhs: constant("i32", -0x8000_0000),
        rhs: constant("i32", -1),
      },
    }),
    assign(place("state", 2), {
      kind: "binary",
      data: {
        op: "divide",
        lhs: constant("i64", "-9223372036854775808"),
        rhs: constant("i64", "-1"),
      },
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getInt32(state + 4, true), -0x8000_0000);
  assert.equal(view.getBigInt64(state + 8, true), -(1n << 63n));
});

test("implements MIR saturating float-to-integer casts with NaN mapping to zero", async () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "i64"));
  mir.state.push(
    { name: "nan_i32", ty: 2, persistence: "snapshot" },
    { name: "negative_i64", ty: 3, persistence: "snapshot" },
    { name: "positive_i32", ty: 2, persistence: "snapshot" },
  );
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.unshift(
    assign(place("state", 1), {
      kind: "cast",
      data: {
        value: constant("f32", "0x7fc00000"),
        to: "i32",
      },
    }),
    assign(place("state", 2), {
      kind: "cast",
      data: {
        value: constant("f64", "0xfff0000000000000"),
        to: "i64",
      },
    }),
    assign(place("state", 3), {
      kind: "cast",
      data: {
        value: constant("f64", "0x7ff0000000000000"),
        to: "i32",
      },
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getInt32(state + 4, true), 0);
  assert.equal(view.getBigInt64(state + 8, true), -(1n << 63n));
  assert.equal(view.getInt32(state + 16, true), 0x7fff_ffff);
});

test("maps NaN to the lower bound for MIR range clamps", async () => {
  const mir = executableMir();
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements[1] = assign(place("local", 3), {
    kind: "intrinsic",
    data: {
      intrinsic: "range_clamp",
      args: [
        constant("f32", "0x7fc00000"),
        constant("f32", -1),
        constant("f32", 1),
      ],
    },
  });

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual([...new Float32Array(memory.buffer, output, 1)], [-1]);
});

test("wraps i32 and full-domain i64 MIR ranges exactly", async () => {
  const mir = executableMir();
  mir.types.push(type("scalar", "i64"));
  mir.state.push(
    { name: "wrapped_i32", ty: 2, persistence: "snapshot" },
    { name: "wrapped_i64", ty: 3, persistence: "snapshot" },
  );
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.unshift(
    assign(place("state", 1), {
      kind: "intrinsic",
      data: {
        intrinsic: "range_wrap",
        args: [
          constant("i32", 7),
          constant("i32", 0),
          constant("i32", 2),
        ],
      },
    }),
    assign(place("state", 2), {
      kind: "intrinsic",
      data: {
        intrinsic: "range_wrap",
        args: [
          constant("i64", "-1"),
          constant("i64", "-9223372036854775808"),
          constant("i64", "9223372036854775807"),
        ],
      },
    }),
  );

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  const rangeCheck = artifact.wat.indexOf("i32.le_u");
  const remainder = artifact.wat.indexOf("i32.rem_u");
  assert.ok(rangeCheck !== -1 && rangeCheck < remainder, artifact.wat);
  assert.match(
    artifact.wat,
    /\(i32\.le_u\s+\(i32\.const 7\)\s+\(i32\.const 2\)\s*\)/,
    "a zero lower bound should eliminate the distance subtraction",
  );
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getInt32(state + 4, true), 1);
  assert.equal(view.getBigInt64(state + 8, true), -1n);
});

test("makes repeated source local names unique for Binaryen", () => {
  const mir = executableMir();
  mir.functions[1].locals[0].name = "reused";
  mir.functions[1].locals[1].name = "reused";
  const artifact = compileMir(mir, { emitText: true, optimize: false });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.match(artifact.wat, /reused\.local0/);
  assert.match(artifact.wat, /reused\.local1/);
});

test("accepts legal zero-frame boundaries and rejects invalid process segments", async () => {
  const artifact = compileMir(executableMir());
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const process = (...args) => callProcess(instance.exports.onda_process, ...args);

  assert.equal(process(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0), 0);
  assert.equal(process(0, 0, 2, 0, 1, 0, 0, 0, 0, 0, 0), 0);
  assert.equal(process(0, 0, 4, 0, 3, 0, 0, 0, 0, 0, 0), 0);

  for (const args of [
    [0, 0, -1, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, -1, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 2, 3, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, -1, 0, 0, 0, 0, 0, 0],
  ]) {
    assert.equal(
      process(...args),
      PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
    );
  }
});

test("forwards position-independent process flags on zero-frame calls", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "$forwarded_flags", ty: 2 });
  mir.functions[1].body.statements.unshift(
    assign(place("local", 6), {
      kind: "load",
      data: place("parameter", 2),
    }),
    assign(place("local", 3), {
      kind: "cast",
      data: { value: local(6), to: "f32" },
    }),
    assign(place("state", 0), { kind: "use", data: local(3) }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + artifact.metadata.runtime.param_size_bytes;
  const view = new DataView(memory.buffer);
  onda_init(params, state);

  for (const [startFrame, flags] of [[4, 1], [0, 2], [2, 3], [2, 0]]) {
    callProcess(onda_process, 0, 0, startFrame, 0, flags, params, state, 0, 0, 0, 0);
    assert.equal(view.getFloat32(state, true), flags);
  }
});

test("lowers MIR multi-value returns and calls through Binaryen", async () => {
  const mir = executableMir();
  mir.functions[1].locals.push({ name: "$integer_result", ty: 2 });
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.splice(
    0,
    3,
    statement("call", { results: [2, 6], function: 2, args: [] }),
    assign(place("local", 3), {
      kind: "cast",
      data: { value: local(6), to: "f32" },
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(3) },
    }),
  );
  mir.functions.push({
    name: "pair",
    kind: { kind: "user" },
    attributes: attributes(),
    params: [],
    results: [0, 2],
    locals: [],
    body: {
      statements: [
        statement("return", {
          values: [constant("f32", 1.5), constant("i32", 2)],
        }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.match(artifact.wat, /\(result f32 i32\)/);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [3.5, 3.5, 3.5, 3.5],
  );
});

test("exports packed scalar and fixed-array event handlers", async () => {
  const mir = executableMir();
  mir.types.push(type("array", { element: 0, len: 2 }));
  mir.interface.events.push({
    name: "set_phase",
    params: [
      { name: "step", ty: 2, default: null },
      {
        name: "values",
        ty: 3,
        default: {
          kind: "aggregate",
          data: [
            { kind: "scalar", data: { type: "f32", value: 0.25 } },
            { kind: "scalar", data: { type: "f32", value: 0.75 } },
          ],
        },
      },
    ],
    handler: 2,
  });
  const arrayElement = place("event_param", 1);
  arrayElement.projections.push({
    kind: "index",
    data: { index: constant("i32", 1), bounds: "clamp" },
  });
  mir.functions.push({
    name: "onda_event::set_phase",
    kind: { kind: "event", data: 0 },
    attributes: attributes("compiler_generated", "always"),
    params: [],
    results: [],
    locals: [
      { name: null, ty: 2 },
      { name: null, ty: 0 },
      { name: null, ty: 0 },
      { name: null, ty: 0 },
    ],
    body: {
      statements: [
        assign(place("local", 0), {
          kind: "load",
          data: place("event_param", 0),
        }),
        assign(place("local", 1), {
          kind: "cast",
          data: { value: local(0), to: "f32" },
        }),
        assign(place("local", 2), { kind: "load", data: arrayElement }),
        assign(place("local", 3), {
          kind: "binary",
          data: { op: "add", lhs: local(1), rhs: local(2) },
        }),
        assign(place("state", 0), { kind: "use", data: local(3) }),
      ],
    },
    source: unknownSource,
  });

  const artifact = compileMir(mir);
  assert.deepEqual(artifact.metadata.exports.events, ["onda_event_0"]);
  assert.deepEqual(
    artifact.metadata.metadata.events[0].params.map((param) => param.byte_offset),
    [0, 4],
  );
  assert.deepEqual(
    artifact.metadata.metadata.events[0].params.map((param) => param.is_slice),
    [false, false],
  );
  assert.equal(artifact.metadata.metadata.events[0].payload_size_bytes, 12);

  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process, onda_event_0 } =
    instance.exports;
  assert.equal(onda_event_0.length, 7);
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const payload = heap;
  heap += 12;
  const outputTable = heap;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setInt32(payload, 3, true);
  view.setFloat32(payload + 4, 2, true);
  view.setFloat32(payload + 8, 4, true);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  onda_event_0(payload, params, state, 0, 0, 0, 0);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [7.25, 7.5, 7.75, 8],
  );
});

test("stores control outputs in their state-backed ABI slots", async () => {
  const mir = executableMir();
  mir.state.push({
    name: "meter_storage",
    ty: 0,
    persistence: "control_mirror",
  });
  mir.interface.control_outputs.push({ name: "meter", ty: 0, mirror: 1 });
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.splice(
    4,
    0,
    statement("control_output_store", {
      output: 0,
      element: null,
      bounds: "unchecked",
      value: local(2),
    }),
  );

  const artifact = compileMir(mir);
  const meter = artifact.metadata.metadata.control_outputs[0];
  assert.equal(meter.state_byte_offset, 4);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getFloat32(state + meter.state_byte_offset, true), 1);
});

test("preserves indexing for fixed arrays of length one", async () => {
  const mir = executableMir();
  mir.types.push(type("array", { element: 0, len: 1 }));
  mir.interface.outputs[0].ty = 3;
  const outputStore =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements.find((entry) => entry.kind.kind === "output_store")
      .kind.data;
  outputStore.element = constant("i32", 0);
  outputStore.bounds = "clamp";

  const artifact = compileMir(mir);
  assert.equal(artifact.metadata.metadata.outputs[0].type_repr, "f32[1]");
  assert.equal(artifact.metadata.metadata.outputs[0].array_len, 1);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  const params = Number(__heap_base.value);
  const state = params + 16;
  const outputTable = state + artifact.metadata.runtime.state_size_bytes;
  const output = outputTable + 4;
  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0.75, 1],
  );
});

test("loads the current audio output frame through explicit MIR", async () => {
  const mir = executableMir();
  const loop = mir.functions[1].body.statements[3];
  const branch = loop.kind.data.body.statements[1];
  const statements = branch.kind.data.then_block.statements;
  const storeIndex = statements.findIndex(
    (entry) => entry.kind.kind === "output_store",
  );
  statements.splice(
    storeIndex + 1,
    0,
    assign(place("local", 3), {
      kind: "output_load",
      data: {
        output: 0,
        element: null,
        bounds: "unchecked",
        frame: local(5),
      },
    }),
    statement("output_store", {
      output: 0,
      element: null,
      bounds: "unchecked",
      frame: local(5),
      value: local(3),
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const outputTable = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process, 0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);

  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0.75, 1],
  );
});

test("reports reachable buffer writes separately from declared access", () => {
  const mir = executableMir();
  mir.interface.buffers.push(
    {
      name: "written",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
    {
      name: "read_only_use",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
  );
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements.unshift(
    assign(place("local", 2), {
      kind: "buffer_load",
      data: {
        buffer: 1,
        channel: null,
        index: local(0),
        bounds: "clamp",
      },
    }),
    statement("buffer_store", {
      buffer: 0,
      channel: null,
      index: local(0),
      value: local(2),
      bounds: "clamp",
    }),
  );

  const artifact = compileMir(mir);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.may_write),
    [true, false],
  );
});

test("reports writes to a constant-selected buffer collection slot precisely", () => {
  const mir = executableMir();
  for (let slot = 0; slot < 4; slot += 1) {
    mir.interface.buffers.push({
      name: `bank[${slot}]`,
      element: "f32",
      channels: "mono",
      access: "read_write",
    });
  }
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements.unshift(
    statement("buffer_store", {
      buffer: {
        kind: "array_element",
        data: {
          first: 0,
          len: 4,
          selector: constant("i32", 2),
          bounds: "clamp",
        },
      },
      channel: null,
      index: local(0),
      value: constant("f32", 1),
      bounds: "clamp",
    }),
  );

  const artifact = compileMir(mir);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.may_write),
    [false, false, true, false],
  );
});

test("translates writable slots through nested buffer collection subspans", () => {
  const mir = executableMir();
  const bankType = mir.types.length;
  mir.types.push(type("buffer_span", {
    element: "f32",
    channels: "mono",
    access: "read_write",
    len: 4,
  }));
  const subspanType = mir.types.length;
  mir.types.push(type("buffer_span", {
    element: "f32",
    channels: "mono",
    access: "read_write",
    len: 2,
  }));
  for (let slot = 0; slot < 4; slot += 1) {
    mir.interface.buffers.push({
      name: `bank[${slot}]`,
      element: "f32",
      channels: "mono",
      access: "read_write",
    });
  }
  mir.functions[1].body.statements.splice(
    3,
    0,
    statement("call", {
      results: [],
      function: 2,
      args: [{
        kind: "buffer_span",
        data: { kind: "interface", data: { first: 0, len: 4 } },
      }],
    }),
  );
  mir.functions.push(
    {
      name: "forward_subspan",
      kind: { kind: "user" },
      attributes: attributes("compiler_generated", "always"),
      params: [{ name: "bank", ty: bankType, mode: "value" }],
      results: [],
      locals: [],
      body: {
        statements: [
          statement("call", {
            results: [],
            function: 3,
            args: [{
              kind: "buffer_span",
              data: {
                kind: "parameter",
                data: { span: 0, start: 1, len: 2 },
              },
            }],
          }),
        ],
      },
      source: unknownSource,
    },
    {
      name: "write_subspan_slot",
      kind: { kind: "user" },
      attributes: attributes("compiler_generated", "always"),
      params: [{ name: "bank", ty: subspanType, mode: "value" }],
      results: [],
      locals: [],
      body: {
        statements: [
          statement("buffer_param_store", {
            parameter: {
              kind: "array_element",
              data: {
                span: 0,
                selector: constant("i32", 1),
                bounds: "clamp",
              },
            },
            channel: null,
            index: constant("i32", 0),
            value: constant("f32", 1),
            bounds: "clamp",
          }),
        ],
      },
      source: unknownSource,
    },
  );

  const artifact = compileMir(mir);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.may_write),
    [false, false, true, false],
  );
});

test("caches direct and constant-selected buffer descriptors before sample loops", () => {
  const mir = executableMir();
  mir.interface.buffers.push(
    {
      name: "bank[0]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
    {
      name: "bank[1]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
  );
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements.unshift(
    assign(place("local", 2), {
      kind: "buffer_load",
      data: {
        buffer: 0,
        channel: null,
        index: local(0),
        bounds: "clamp",
      },
    }),
    assign(place("local", 3), {
      kind: "buffer_load",
      data: {
        buffer: {
          kind: "array_element",
          data: {
            first: 0,
            len: 2,
            selector: constant("i32", 1),
            bounds: "clamp",
          },
        },
        channel: null,
        index: local(0),
        bounds: "clamp",
      },
    }),
  );

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  const process = emittedFunction(artifact.wat, "$onda.fn.1");
  const loop = process.indexOf("(loop $$onda.loop");
  assert.notEqual(loop, -1);
  const entry = process.slice(0, loop);
  const sampleLoop = process.slice(loop);
  assert.equal(matchCount(entry, /global\.get \$\$onda\.buffers/g), 2);
  assert.equal(matchCount(entry, /global\.get \$\$onda\.buffer_frames/g), 2);
  assert.doesNotMatch(sampleLoop, /global\.get \$\$onda\.(buffers|buffer_frames)/);
  assert.match(process, /buffer\.buffers\.0\.generated/);
  assert.match(process, /buffer\.buffers\.1\.generated/);
});

test("hoists forwarded invariant buffer descriptors before sample loops", () => {
  const artifact = compileMir(forwardedBufferCollectionMir(), {
    emitText: true,
  });
  const process = emittedParameterizedFunction(artifact.wat, "$onda.abi.process");
  const loop = process.indexOf("(loop $$onda.loop");
  assert.notEqual(loop, -1);
  const entry = process.slice(0, loop);
  const sampleLoop = process.slice(loop);
  assert.match(
    entry,
    /global\.get \$\$onda\.(buffers|buffer_frames|buffer_channels)/,
  );
  assert.doesNotMatch(
    sampleLoop,
    /global\.get \$\$onda\.(buffers|buffer_frames|buffer_channels)/,
  );
});

test("refreshes hoisted forwarded descriptors for every process call", async () => {
  const artifact = compileMir(forwardedBufferCollectionMir());
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const outputTable = heap;
  heap += 4;
  const bufferPointers = heap;
  heap += 8;
  const bufferFrames = heap;
  heap += 8;
  const bufferChannels = heap;
  heap += 8;
  const bufferSampleRates = heap;
  heap += 8;
  const firstBinding = heap;
  heap += 4;
  const secondBinding = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(outputTable, output, true);
  view.setUint32(bufferPointers, firstBinding, true);
  view.setUint32(bufferPointers + 4, firstBinding, true);
  view.setInt32(bufferFrames, 1, true);
  view.setInt32(bufferFrames + 4, 1, true);
  view.setInt32(bufferChannels, 1, true);
  view.setInt32(bufferChannels + 4, 1, true);
  view.setFloat32(bufferSampleRates, 48_000, true);
  view.setFloat32(bufferSampleRates + 4, 48_000, true);
  view.setFloat32(firstBinding, 21, true);
  view.setFloat32(secondBinding, 43, true);
  onda_init(params, state);

  callProcess(
    onda_process,
    0,
    outputTable,
    0,
    1,
    0,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.equal(view.getFloat32(output, true), 21);

  view.setUint32(bufferPointers + 4, secondBinding, true);
  callProcess(
    onda_process,
    0,
    outputTable,
    0,
    1,
    0,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.equal(view.getFloat32(output, true), 43);
});

test("keeps forwarded varying buffer descriptors inside sample loops", () => {
  const artifact = compileMir(forwardedBufferCollectionMir(true), {
    emitText: true,
  });
  const process = emittedParameterizedFunction(artifact.wat, "$onda.abi.process");
  const loop = process.slice(process.indexOf("(loop $$onda.loop"));
  assert.match(
    loop,
    /global\.get \$\$onda\.(buffers|buffer_frames|buffer_channels)/,
  );
});

test("preserves the loop-entry value of a loop-carried buffer selector", async () => {
  const mir = executableMir();
  mir.interface.buffers.push(
    {
      name: "bank[0]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
    {
      name: "bank[1]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
  );
  mir.state.push({ name: "slot", ty: 2, persistence: "snapshot" });
  mir.functions[0].body.statements.push(
    assign(place("state", 1), {
      kind: "use",
      data: constant("i32", 0),
    }),
  );

  const process = mir.functions[1];
  const selector = process.locals.length;
  process.locals.push({ name: "$slot", ty: 2 });
  process.body.statements.splice(
    3,
    0,
    assign(place("local", selector), {
      kind: "load",
      data: place("state", 1),
    }),
  );
  const thenBlock =
    process.body.statements[4].kind.data.body.statements[1].kind.data.then_block;
  thenBlock.statements = [
    assign(place("local", 2), {
      kind: "buffer_sample_rate",
      data: {
        kind: "array_element",
        data: {
          first: 0,
          len: 2,
          selector: local(selector),
          bounds: "clamp",
        },
      },
    }),
    assign(place("local", 5), {
      kind: "process_frame",
      data: { offset: local(0) },
    }),
    statement("output_store", {
      output: 0,
      element: null,
      bounds: "unchecked",
      frame: local(5),
      value: local(2),
    }),
    assign(place("local", selector), {
      kind: "use",
      data: constant("i32", 1),
    }),
    assign(place("local", 0), {
      kind: "binary",
      data: { op: "add", lhs: local(0), rhs: constant("i32", 1) },
    }),
  ];
  process.body.statements.push(
    assign(place("state", 1), {
      kind: "use",
      data: local(selector),
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const outputs = heap;
  heap += 4;
  const bufferPointers = heap;
  heap += 8;
  const bufferFrames = heap;
  heap += 8;
  const bufferChannels = heap;
  heap += 8;
  const bufferSampleRates = heap;
  heap += 8;
  const bufferData = heap;
  heap += 8;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(outputs, output, true);
  view.setUint32(bufferPointers, bufferData, true);
  view.setUint32(bufferPointers + 4, bufferData + 4, true);
  for (let index = 0; index < 2; index += 1) {
    view.setInt32(bufferFrames + index * 4, 1, true);
    view.setInt32(bufferChannels + index * 4, 1, true);
  }
  view.setFloat32(bufferSampleRates, 10_000, true);
  view.setFloat32(bufferSampleRates + 4, 20_000, true);
  onda_init(params, state);

  assert.equal(
    callProcess(
      onda_process,
      0,
      outputs,
      0,
      0,
      0,
      params,
      state,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
    ),
    0,
  );
  assert.equal(
    view.getInt32(state + 4, true),
    0,
    "a zero-frame call must not execute the loop-carried selector assignment",
  );

  const status = callProcess(
    onda_process,
    0,
    outputs,
    0,
    4,
    3,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.equal(status, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [10_000, 20_000, 20_000, 20_000],
  );
});

test("caches static audio channel pointers before sample loops", () => {
  const artifact = compileMir(executableMir(), {
    emitText: true,
    optimize: false,
  });
  const process = emittedFunction(artifact.wat, "$onda.fn.1");
  const loop = process.indexOf("(loop $$onda.loop");
  assert.notEqual(loop, -1);
  const entry = process.slice(0, loop);
  const sampleLoop = process.slice(loop);
  assert.equal(matchCount(entry, /global\.get \$\$onda\.outputs/g), 1);
  assert.doesNotMatch(sampleLoop, /global\.get \$\$onda\.outputs/);
  assert.match(process, /audio\.outputs\.0\.generated/);
});

test("keeps dynamic audio array channel selection inside sample loops", () => {
  const mir = executableMir();
  const outputArray = mir.types.length;
  mir.types.push(type("array", { element: 0, len: 2 }));
  mir.interface.outputs[0].ty = outputArray;
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  const outputStore = thenBlock.statements.find(
    (entry) => entry.kind.kind === "output_store",
  );
  outputStore.kind.data.element = local(0);
  outputStore.kind.data.bounds = "clamp";

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  const process = emittedFunction(artifact.wat, "$onda.fn.1");
  const loop = process.indexOf("(loop $$onda.loop");
  assert.notEqual(loop, -1);
  const entry = process.slice(0, loop);
  const sampleLoop = process.slice(loop);
  assert.doesNotMatch(entry, /audio\.outputs\.[01]\.generated/);
  assert.match(sampleLoop, /global\.get \$\$onda\.outputs/);
});

test("resolves a varying collection selector once per buffer operation", () => {
  const mir = executableMir();
  mir.interface.buffers.push(
    {
      name: "bank[0]",
      element: "f32",
      channels: "dynamic",
      access: "read_write",
    },
    {
      name: "bank[1]",
      element: "f32",
      channels: "dynamic",
      access: "read_write",
    },
  );
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements.unshift(
    assign(place("local", 2), {
      kind: "buffer_load",
      data: {
        buffer: {
          kind: "array_element",
          data: {
            first: 0,
            len: 2,
            selector: local(0),
            bounds: "clamp",
          },
        },
        channel: constant("i32", 1),
        index: local(0),
        bounds: "clamp",
      },
    }),
  );

  const artifact = compileMir(mir, { emitText: true, optimize: false });
  const process = emittedFunction(artifact.wat, "$onda.fn.1");
  const loop = process.slice(process.indexOf("(loop $$onda.loop"));
  assert.equal(
    matchCount(loop, /local\.set \$buffer\.descriptor_index\.generated/g),
    1,
  );
  assert.equal(matchCount(loop, /global\.get \$\$onda\.buffers/g), 1);
  assert.equal(matchCount(loop, /global\.get \$\$onda\.buffer_frames/g), 1);
  assert.equal(matchCount(loop, /global\.get \$\$onda\.buffer_channels/g), 1);
});

test("loads, stores, and queries externally bound buffers", async () => {
  const mir = executableMir();
  mir.interface.buffers.push({
    name: "table",
    element: "f32",
    channels: "mono",
    access: "read_write",
  });
  mir.functions[1].locals.push(
    { name: "$buffer_len", ty: 2 },
    { name: "$buffer_channels", ty: 2 },
    { name: "$metadata_f32", ty: 0 },
  );
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements = [
    assign(place("local", 2), {
      kind: "buffer_load",
      data: {
        buffer: 0,
        channel: null,
        index: local(0),
        bounds: "clamp",
      },
    }),
    assign(place("local", 6), { kind: "buffer_len", data: 0 }),
    assign(place("local", 3), {
      kind: "cast",
      data: { value: local(6), to: "f32" },
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(3) },
    }),
    assign(place("local", 7), { kind: "buffer_channels", data: 0 }),
    assign(place("local", 8), {
      kind: "cast",
      data: { value: local(7), to: "f32" },
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(8) },
    }),
    assign(place("local", 3), {
      kind: "buffer_sample_rate",
      data: 0,
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(3) },
    }),
    statement("buffer_store", {
      buffer: 0,
      channel: null,
      index: local(0),
      value: local(2),
      bounds: "clamp",
    }),
    assign(place("local", 5), {
      kind: "process_frame",
      data: { offset: local(0) },
    }),
    statement("output_store", {
      output: 0,
      element: null,
      bounds: "unchecked",
      frame: local(5),
      value: local(2),
    }),
    assign(place("local", 0), {
      kind: "binary",
      data: { op: "add", lhs: local(0), rhs: constant("i32", 1) },
    }),
  ];

  const artifact = compileMir(mir);
  assert.deepEqual(artifact.metadata.metadata.buffers, [
    {
      name: "table",
      type_repr: "buffer<f32>",
      scalar: "f32",
      element_size_bytes: 4,
      channels: "mono",
      static_channels: 1,
      access: "read_write",
      may_write: true,
    },
  ]);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += 16;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const bufferPointers = heap;
  heap += 4;
  const bufferFrames = heap;
  heap += 4;
  const bufferChannels = heap;
  heap += 4;
  const bufferSampleRates = heap;
  heap += 4;
  const bufferData = heap;
  heap += 16;
  const outputTable = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(bufferPointers, bufferData, true);
  view.setInt32(bufferFrames, 4, true);
  view.setInt32(bufferChannels, 1, true);
  view.setFloat32(bufferSampleRates, 10, true);
  new Float32Array(memory.buffer, bufferData, 4).set([1, 2, 3, 4]);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  callProcess(onda_process,
    0,
    outputTable,
    0,
    4,
    3,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [16, 17, 18, 19],
  );
  assert.deepEqual(
    [...new Float32Array(memory.buffer, bufferData, 4)],
    [16, 17, 18, 19],
  );

  view.setUint32(bufferPointers, 0, true);
  view.setInt32(bufferFrames, 1, true);
  new Float32Array(memory.buffer, output, 4).fill(0);
  for (let invocation = 0; invocation < 2; invocation += 1) {
    callProcess(
      onda_process,
      0,
      outputTable,
      0,
      4,
      3,
      params,
      state,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
    );
    assert.deepEqual(
      [...new Float32Array(memory.buffer, output, 4)],
      [12, 12, 12, 12],
    );
  }
});

test("forwards dynamically selected proc buffer parameters without copying", async () => {
  const mir = executableMir();
  const bufferType = mir.types.length;
  mir.types.push(
    type("buffer", {
      element: "f32",
      channels: "mono",
      access: "read_write",
    }),
  );
  const bufferSpanType = mir.types.length;
  mir.types.push(
    type("buffer_span", {
      element: "f32",
      channels: "mono",
      access: "read_write",
      len: 2,
    }),
  );
  mir.interface.buffers.push(
    {
      name: "bank[0]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
    {
      name: "bank[1]",
      element: "f32",
      channels: "mono",
      access: "read_write",
    },
  );

  const processBody =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  processBody.splice(
    0,
    3,
    statement("call", {
      results: [2],
      function: 2,
      args: [
        { kind: "value", data: local(0) },
        {
          kind: "buffer_span",
          data: { kind: "interface", data: { first: 0, len: 2 } },
        },
      ],
    }),
  );

  const selectedParameter = {
    kind: "array_element",
    data: {
      span: 1,
      selector: local(0),
      bounds: "clamp",
    },
  };
  mir.functions.push(
    {
      name: "select_buffer",
      kind: { kind: "user" },
      attributes: attributes("compiler_generated", "always"),
      params: [
        { name: "slot", ty: 2, mode: "value" },
        { name: "bank", ty: bufferSpanType, mode: "value" },
      ],
      results: [0],
      locals: [
        { name: "$slot", ty: 2 },
        { name: "$sample", ty: 0 },
      ],
      body: {
        statements: [
          assign(place("local", 0), {
            kind: "load",
            data: place("parameter", 0),
          }),
          statement("call", {
            results: [1],
            function: 3,
            args: [{ kind: "buffer_param", data: selectedParameter }],
          }),
          statement("return", { values: [local(1)] }),
        ],
      },
      source: unknownSource,
    },
    {
      name: "increment_buffer",
      kind: { kind: "user" },
      attributes: attributes(),
      params: [
        { name: "buffer", ty: bufferType, mode: "read_write_reference" },
      ],
      results: [0],
      locals: [
        { name: "$sample", ty: 0 },
        { name: "$incremented", ty: 0 },
      ],
      body: {
        statements: [
          assign(place("local", 0), {
            kind: "buffer_param_load",
            data: {
              parameter: { kind: "direct", data: 0 },
              channel: null,
              index: constant("i32", 0),
              bounds: "clamp",
            },
          }),
          assign(place("local", 1), {
            kind: "binary",
            data: {
              op: "add",
              lhs: local(0),
              rhs: constant("f32", 1),
            },
          }),
          statement("buffer_param_store", {
            parameter: { kind: "direct", data: 0 },
            channel: null,
            index: constant("i32", 0),
            value: local(1),
            bounds: "clamp",
          }),
          statement("return", { values: [local(0)] }),
        ],
      },
      source: unknownSource,
    },
  );

  const artifact = compileMir(mir);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.may_write),
    [true, true],
  );
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += artifact.metadata.runtime.param_size_bytes;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const bufferPointers = heap;
  heap += 8;
  const bufferFrames = heap;
  heap += 8;
  const bufferChannels = heap;
  heap += 8;
  const bufferSampleRates = heap;
  heap += 8;
  const firstBuffer = heap;
  heap += 4;
  const secondBuffer = heap;
  heap += 4;
  const outputTable = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(bufferPointers, firstBuffer, true);
  view.setUint32(bufferPointers + 4, secondBuffer, true);
  view.setInt32(bufferFrames, 1, true);
  view.setInt32(bufferFrames + 4, 1, true);
  view.setInt32(bufferChannels, 1, true);
  view.setInt32(bufferChannels + 4, 1, true);
  view.setFloat32(bufferSampleRates, 48_000, true);
  view.setFloat32(bufferSampleRates + 4, 48_000, true);
  view.setFloat32(firstBuffer, 10, true);
  view.setFloat32(secondBuffer, 20, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);

  callProcess(
    onda_process,
    0,
    outputTable,
    0,
    4,
    3,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [10, 20, 21, 22],
  );
  assert.equal(view.getFloat32(firstBuffer, true), 11);
  assert.equal(view.getFloat32(secondBuffer, true), 23);
});

test("clamps multichannel buffer coordinates independently", async () => {
  const mir = executableMir();
  mir.interface.buffers.push({
    name: "stereo",
    element: "f32",
    channels: { static: 2 },
    access: "read_write",
  });
  const thenBlock =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block;
  thenBlock.statements = [
    assign(place("local", 2), {
      kind: "buffer_load",
      data: {
        buffer: 0,
        channel: constant("i32", -1),
        index: constant("i32", 99),
        bounds: "clamp",
      },
    }),
    assign(place("local", 3), {
      kind: "buffer_load",
      data: {
        buffer: 0,
        channel: constant("i32", 99),
        index: constant("i32", -1),
        bounds: "clamp",
      },
    }),
    assign(place("local", 2), {
      kind: "binary",
      data: { op: "add", lhs: local(2), rhs: local(3) },
    }),
    assign(place("local", 5), {
      kind: "process_frame",
      data: { offset: local(0) },
    }),
    statement("output_store", {
      output: 0,
      element: null,
      bounds: "unchecked",
      frame: local(5),
      value: local(2),
    }),
    assign(place("local", 0), {
      kind: "binary",
      data: { op: "add", lhs: local(0), rhs: constant("i32", 1) },
    }),
  ];

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += artifact.metadata.runtime.param_size_bytes;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const bufferPointers = heap;
  heap += 4;
  const bufferFrames = heap;
  heap += 4;
  const bufferChannels = heap;
  heap += 4;
  const bufferSampleRates = heap;
  heap += 4;
  const bufferData = heap;
  heap += 16;
  const outputTable = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(bufferPointers, bufferData, true);
  view.setInt32(bufferFrames, 2, true);
  view.setInt32(bufferChannels, 2, true);
  view.setFloat32(bufferSampleRates, 48_000, true);
  new Float32Array(memory.buffer, bufferData, 4).set([10, 20, 30, 40]);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);

  callProcess(onda_process,
    0,
    outputTable,
    0,
    1,
    3,
    params,
    state,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
  );
  assert.equal(new Float32Array(memory.buffer, output, 1)[0], 50);
});

test("returns a failure for overlapping slice copies with unequal strides", async () => {
  const mir = executableMir();
  mir.types.push(
    type("slice", { element: "f32", access: "read_write" }),
  );
  mir.interface.buffers.push({
    name: "bus",
    element: "f32",
    channels: { static: 2 },
    access: "read_write",
  });
  mir.functions[1].locals.push(
    { name: "channel", ty: 3 },
    { name: "whole", ty: 3 },
  );
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements.unshift(
    assign(place("local", 6), {
      kind: "make_slice",
      data: {
        source: {
          kind: "buffer",
          data: {
            buffer: 0,
            channel: constant("i32", 0),
          },
        },
        start: constant("i32", 0),
        len: constant("i32", 2),
        bounds: "unchecked",
        access: "read_write",
      },
    }),
    assign(place("local", 7), {
      kind: "make_slice",
      data: {
        source: {
          kind: "buffer",
          data: {
            buffer: 0,
            channel: null,
          },
        },
        start: constant("i32", 0),
        len: constant("i32", 4),
        bounds: "unchecked",
        access: "read_write",
      },
    }),
    statement("slice_copy", {
      destination: local(6),
      source: local(7),
    }),
  );

  const artifact = compileMir(mir);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
  let heap = Number(__heap_base.value);
  const params = heap;
  heap += artifact.metadata.runtime.param_size_bytes;
  const state = heap;
  heap += artifact.metadata.runtime.state_size_bytes;
  const bufferPointers = heap;
  heap += 4;
  const bufferFrames = heap;
  heap += 4;
  const bufferChannels = heap;
  heap += 4;
  const bufferSampleRates = heap;
  heap += 4;
  const bufferData = heap;
  heap += 16;
  const outputTable = heap;
  heap += 4;
  const output = heap;
  const view = new DataView(memory.buffer);
  view.setUint32(bufferPointers, bufferData, true);
  view.setInt32(bufferFrames, 2, true);
  view.setInt32(bufferChannels, 2, true);
  view.setFloat32(bufferSampleRates, 48_000, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  assert.equal(
    callProcess(onda_process,
        0,
        outputTable,
        0,
        1,
        3,
        params,
        state,
        bufferPointers,
        bufferFrames,
        bufferChannels,
        bufferSampleRates,
      ),
    PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  );
});
