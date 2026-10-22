import assert from "node:assert/strict";
import test from "node:test";
import { compileTrustedMir, SUPPORTED_MIR_SCHEMA_VERSION } from "../src/index.js";

const local = (data) => ({ kind: "local", data });
const constant = (type, value) => ({ kind: "constant", data: { type, value } });
const place = (kind, data) => ({ base: { kind, data }, projections: [] });
const source = { file: null, line: 0, column: 0, end_line: 0, end_column: 0 };
const statement = (kind, data) => ({ kind: { kind, data }, source });
const assign = (id, kind, data) => statement("assign", { destination: place("local", id), value: { kind, data } });

function pointwise(scalar = "f32", recurrence = false) {
  const body = [
    assign(3, "process_frame", { offset: local(0) }),
    assign(4, "input_load", { input: 0, element: null, bounds: "unchecked", frame: local(3) }),
    assign(5, "binary", { op: "multiply", lhs: local(4), rhs: recurrence ? local(6) : constant(scalar, 0.5) }),
    assign(6, "binary", { op: "add", lhs: local(5), rhs: constant(scalar, 0.25) }),
    statement("output_store", { output: 0, element: null, bounds: "unchecked", frame: local(3), value: local(6) }),
    assign(0, "binary", { op: "add", lhs: local(0), rhs: constant("i32", 1) }),
  ];
  const func = (name, kind, params, locals, statements) => ({
    name, kind: { kind }, params, locals: locals.map((ty) => ({ ty, name: null })), results: [],
    attributes: { origin: "compiler_generated", inline: "auto", runtime_context: true },
    body: { statements }, source,
  });
  return {
    schema_version: SUPPORTED_MIR_SCHEMA_VERSION, config: { sample_rate: 48000, block_size: 17 },
    source_files: [], log_sites: [], types: [scalar, 'i32', 'bool'].map((data) => ({ kind: "scalar", data })), structs: [], state: [], const_data: [],
    interface: { inputs: [{ name: "in1", ty: 0, default: null, range: null }], outputs: [{ name: "out1", ty: 0 }], control_outputs: [], params: [], buffers: [], buffer_arrays: [], events: [], delegates: [] },
    functions: [func("init", "init", [], [], []), func("process", "process",
      ['start_frame', 'frames', 'flags'].map((name) => ({ name, ty: 1, mode: "value" })), [1, 1, 2, 1, 0, 0, 0], [
        assign(0, "use", constant("i32", 0)), assign(1, "load", place("parameter", 1)),
        assign(6, "use", constant(scalar, 1)),
        statement("loop", { body: { statements: [
          assign(2, "compare", { op: "less", lhs: local(0), rhs: local(1) }),
          statement("if", { condition: local(2), then_block: { statements: body }, else_block: { statements: [statement("break")] } }),
        ] } }),
      ])], entry_points: { init: 0, process: 1 },
  };
}

async function execute(artifact, scalar, offset, segments, special = false) {
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base, onda_process } = instance.exports;
  const base = Number(__heap_base.value);
  const width = scalar === 'f32' ? 4 : 8;
  const data = base + 64;
  const output = offset === null ? data + 256 : data + offset * width;
  const view = new DataView(memory.buffer);
  view.setUint32(base, data, true);
  view.setUint32(base + 4, output, true);
  const samples = scalar === 'f32' ? new Float32Array(memory.buffer, data, 40) : new Float64Array(memory.buffer, data, 40);
  samples.fill(0);
  for (let i = 0; i < 17; i++) samples[i] = (i - 8) / 7;
  if (special) samples.set([NaN, Infinity, -Infinity, -0, Number.MIN_VALUE]);
  for (const [start, frames] of segments) {
    assert.equal(onda_process(0, 0, base, base + 4, start, frames, 3, 0, 0, 0, 0, 0), 0);
  }
  return Array.from(scalar === 'f32' ? new Float32Array(memory.buffer, output, 17) : new Float64Array(memory.buffer, output, 17));
}

for (const scalar of ['f32', 'f64']) {
  test(`sample SIMD preserves ${scalar} arithmetic, tails, segmentation, and aliasing`, async () => {
    const mir = pointwise(scalar);
    const vector = compileTrustedMir(mir, { simd: true, emitText: true });
    const fallback = compileTrustedMir(mir, { simd: false });
    assert.match(vector.wat, new RegExp(`${scalar === 'f32' ? 'f32x4' : 'f64x2'}\\.mul`));
    for (const offset of [null, 0, 1, -1]) {
      for (const segments of [[[0, 17]], [[0, 1], [1, 7], [8, 9]], [[0, 0], [0, 17]]]) {
        assert.deepEqual(await execute(vector, scalar, offset, segments), await execute(fallback, scalar, offset, segments));
      }
    }
    assert.deepEqual(await execute(vector, scalar, null, [[0, 17]], true), await execute(fallback, scalar, null, [[0, 17]], true));
  });
}

test('loop-carried arithmetic stays scalar', async () => {
  const mir = pointwise('f32', true);
  const artifact = compileTrustedMir(mir, { simd: true, emitText: true });
  assert.doesNotMatch(artifact.wat, /f32x4\.mul/);
  assert.deepEqual(await execute(artifact, 'f32', null, [[0, 17]]), await execute(compileTrustedMir(mir, { simd: false }), 'f32', null, [[0, 17]]));
});
