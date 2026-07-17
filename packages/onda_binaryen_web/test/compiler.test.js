import assert from "node:assert/strict";
import test from "node:test";

import {
  OndaBinaryenError,
  compileMir as compileUntrustedMir,
  compileTrustedMir as compileMir,
  createDefaultImports,
} from "../src/index.js";

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
    schema_version: 5,
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

test("requires an explicit trusted-producer boundary for unchecked MIR", () => {
  assert.throws(
    () => compileUntrustedMir(executableMir()),
    /unchecked bounds.*require compileTrustedMir/,
  );
});

test("compiles versioned MIR into an executable persistent DSP module", async () => {
  const artifact = compileMir(executableMir(), { emitText: true });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.match(artifact.wat, /export "onda_process"/);
  assert.equal(artifact.metadata.backend, "binaryen-js");
  assert.equal(artifact.metadata.runtime.state_size_bytes, 16);
  assert.equal(artifact.metadata.runtime.snapshot_size_bytes, 4);
  assert.deepEqual(artifact.metadata.metadata.states, [
    {
      name: "phase",
      type: "f32",
      scalar: "f32",
      array_length: 1,
      is_array: false,
      byte_offset: 0,
      storage_byte_offset: 0,
      byte_size: 4,
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
  onda_init(params, state);
  assert.equal(onda_process.length, 11);
  onda_process(0, outputTable, 0, 2, 1, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0, 0],
  );
  onda_process(0, outputTable, 2, 2, 2, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0.75, 1],
  );

  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [1.25, 1.5, 1.75, 2],
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getFloat32(state + 8, true), 7);
});

test("enforces checked make_slice ranges and traps indexed access to empty slices", async () => {
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
  clampedInstance.exports.onda_process(
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
    makeMir({ start: 5, len: 0, bounds: "trap", load: false }),
    makeMir({ start: 4, len: 0, bounds: "trap", load: true }),
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
    assert.throws(
      () =>
        instance.exports.onda_process(
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
      WebAssembly.RuntimeError,
    );
  }
});

test("lowers schema-5 fixed-array and slice reference windows", async () => {
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
            bounds: "trap",
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
            bounds: "trap",
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
  onda_process(0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.deepEqual(
    [...new Float32Array(memory.buffer, state + 4, 4)],
    [7, 8, 7, 8],
  );
});

test("rejects incompatible MIR schema versions before code generation", () => {
  const mir = executableMir();
  mir.schema_version = 2;
  assert.throws(
    () => compileMir(mir),
    (error) =>
      error instanceof OndaBinaryenError &&
      /unsupported MIR schema version 2; expected 5/.test(error.message),
  );
});

test("uses an explicit Binaryen O3 speed policy by default", () => {
  const artifact = compileMir(executableMir());
  assert.deepEqual(artifact.metadata.optimization, {
    enabled: true,
    level: 3,
    shrink_level: 0,
  });

  const custom = compileMir(executableMir(), {
    optimizeLevel: 2,
    shrinkLevel: 1,
  });
  assert.deepEqual(custom.metadata.optimization, {
    enabled: true,
    level: 2,
    shrink_level: 1,
  });
  assert.throws(
    () => compileMir(executableMir(), { optimizeLevel: 5 }),
    /optimizeLevel must be an integer from 0 through 4/,
  );
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

test("rejects noncanonical schema-v5 process entry signatures", () => {
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

test("preserves schema-v5 i64 and non-finite constants exactly", async () => {
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

test("rejects lossy numeric schema-v5 i64 constants", () => {
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
    /schema 5 i64 values must be canonical decimal strings/,
  );
});

test("declares and satisfies browser math imports for non-Wasm intrinsics", async () => {
  const mir = executableMir();
  const thenStatements =
    mir.functions[1].body.statements[3].kind.data.body.statements[1].kind.data
      .then_block.statements;
  thenStatements[2].kind.data.value = {
    kind: "intrinsic",
    data: { intrinsic: "sin", args: [local(3)] },
  };
  const artifact = compileMir(mir);
  assert.deepEqual(artifact.metadata.imports, [
    { module: "onda_math", name: "sin_f32" },
  ]);

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
  view.setFloat32(params, 0.25, true);
  view.setUint32(outputTable, output, true);
  onda_init(params, state);
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
  assert.ok(
    Math.abs(new Float32Array(memory.buffer, output, 4)[0] - Math.sin(0.25)) <
      1e-6,
  );
});

test("lowers f32 and f64 FMA through the exact versioned bit support ABI", async () => {
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
    assert.deepEqual(artifact.metadata.imports, [
      {
        module: "onda_exact_math_v1",
        name: `fma_${scalar}_bits`,
      },
    ]);
    const imports = WebAssembly.Module.imports(
      new WebAssembly.Module(artifact.wasm),
    );
    assert.deepEqual(imports, [
      {
        module: "onda_exact_math_v1",
        name: `fma_${scalar}_bits`,
        kind: "function",
      },
    ]);

    const { instance } = await WebAssembly.instantiate(
      artifact.wasm,
      createDefaultImports(),
    );
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
    onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);

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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 1, 3, params, state, 0, 0, 0, 0);
  assert.equal(view.getInt32(state + 4, true), 0);
  assert.equal(view.getBigInt64(state + 8, true), -(1n << 63n));
  assert.equal(view.getInt32(state + 16, true), 0x7fff_ffff);
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
  const process = instance.exports.onda_process;

  process(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
  process(0, 0, 2, 0, 1, 0, 0, 0, 0, 0, 0);
  process(0, 0, 4, 0, 3, 0, 0, 0, 0, 0, 0);

  assert.throws(
    () => process(0, 0, -1, 0, 0, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
  assert.throws(
    () => process(0, 0, 0, -1, 0, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
  assert.throws(
    () => process(0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
  assert.throws(
    () => process(0, 0, 2, 3, 0, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
  assert.throws(
    () => process(0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
  assert.throws(
    () => process(0, 0, 0, 0, -1, 0, 0, 0, 0, 0, 0),
    WebAssembly.RuntimeError,
  );
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
    onda_process(0, 0, startFrame, 0, flags, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
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
  assert.equal(artifact.metadata.metadata.outputs[0].is_array, true);
  assert.equal(artifact.metadata.metadata.outputs[0].channel_count, 1);
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);
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
  onda_process(0, outputTable, 0, 4, 3, params, state, 0, 0, 0, 0);

  assert.deepEqual(
    [...new Float32Array(memory.buffer, output, 4)],
    [0.25, 0.5, 0.75, 1],
  );
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
  onda_process(
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
});

test("traps overlapping slice copies with unequal strides", async () => {
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
  assert.throws(
    () =>
      onda_process(
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
    WebAssembly.RuntimeError,
  );
});
