import assert from "node:assert/strict";
import test from "node:test";
import { canonicalF32Number, PayloadPlan, writeEventInput } from "../src/index.js";

const scalar = (encoding) => ({ kind: "scalar", encoding });
const note = { kind: "struct", name: "Note", fields: [
  { name: "enabled", ty: scalar("bool") },
  { name: "gain", ty: scalar("f64") },
  { name: "bins", ty: { kind: "array", len: 2, element: scalar("i32") } },
  { name: "pair", ty: { kind: "tuple", elements: [scalar("i64"), scalar("f32")] } },
] };
const schema = { params: [
  { name: "prefix", ty: scalar("bool") },
  { name: "notes", ty: { kind: "slice", element: note } },
  { name: "tail", ty: scalar("i64"), default: "-9223372036854775808" },
] };

test("f32 host values hide widening noise without changing their bits", () => {
  const value = canonicalF32Number(0.15);
  assert.equal(value, 0.15);
  assert.equal(Math.fround(value), Math.fround(0.15));
  assert.ok(Object.is(canonicalF32Number(-0), -0));

  const plan = new PayloadPlan({ params: [{ name: "gain", ty: scalar("f32") }] });
  assert.deepEqual(plan.decode(plan.encode({ gain: 0.15 })), { gain: 0.15 });
  assert.equal(JSON.stringify(plan.decode(plan.encode({ gain: 0.15 }))), '{"gain":0.15}');
});

test("structured payloads use one length and canonical nested SoA tensors", () => {
  const plan = new PayloadPlan(schema);
  const values = { prefix: true, notes: [
    { enabled: true, gain: -0, bins: [1, 2], pair: [9007199254740993n, 0.25] },
    { enabled: false, gain: 4.5, bins: [3, 4], pair: [-9223372036854775808n, 0.5] },
  ] };
  const bytes = plan.encode(values);
  assert.equal(bytes.length, 1 + 4 + 2 * (1 + 8 + 8 + 8 + 4) + 8);
  assert.equal(new DataView(bytes.buffer).getInt32(1, true), 2);
  assert.deepEqual([...bytes.slice(5, 7)], [1, 0]);
  const decoded = plan.decode(bytes);
  assert.deepEqual(decoded, { ...values, tail: -9223372036854775808n });
  assert.ok(Object.is(decoded.notes[0].gain, -0));
  assert.equal(plan.requiredWorkspace(bytes), plan.sizes([2]).workspace);
  assert.equal(plan.tensors.length, 7);
  assert.equal(plan.parameters[1].lengthParameter, 1);
  assert.equal(plan.abiParameterCount, 8);
  const empty = plan.encode({ prefix: false, notes: [] });
  assert.deepEqual(plan.decode(empty), { prefix: false, notes: [], tail: -9223372036854775808n });
});

test("nested struct arrays retain independent outer and inner tensor axes", () => {
  const plan = new PayloadPlan({ params: [{ name: "patches", ty: { kind: "array", len: 2, element: {
    kind: "struct", name: "Patch", fields: [
      { name: "notes", ty: { kind: "array", len: 2, element: note } },
      { name: "tail", ty: scalar("i32") },
    ],
  } } }] });
  const make = (gain) => ({ enabled: true, gain, bins: [gain + 1, gain + 2], pair: [BigInt(gain + 3), gain + 4] });
  const patches = [{ notes: [make(1), make(10)], tail: 7 }, { notes: [make(100), make(1000)], tail: 9 }];
  assert.deepEqual(plan.decode(plan.encode({ patches })), { patches });
  assert.deepEqual(plan.tensors.map((leaf) => leaf.shape), [[2, 2], [2, 2], [2, 2, 2], [2, 2], [2, 2], [2]]);
});

test("payload preflight rejects every truncation, trailing bytes, negative and overflowing lengths", () => {
  const plan = new PayloadPlan(schema);
  const bytes = plan.encode({ prefix: false, notes: [] });
  for (let size = 0; size < bytes.length; size += 1) assert.throws(() => plan.decode(bytes.subarray(0, size)), /truncated/);
  assert.throws(() => plan.decode(new Uint8Array(bytes.length + 1)), /trailing/);
  for (const length of [-1, 0x7fffffff]) {
    const invalid = bytes.slice();
    new DataView(invalid.buffer).setInt32(1, length, true);
    assert.throws(() => plan.decode(invalid), /negative|exceeds/);
  }
  assert.throws(() => plan.encode({ prefix: true, notes: [], tail: Number.MAX_SAFE_INTEGER + 1 }), /exact/);
});

test("zero-leaf payload types are rejected", () => {
  const empty = { kind: "struct", name: "Empty", fields: [] };
  for (const ty of [
    empty,
    { kind: "struct", name: "Wrapper", fields: [{ name: "empty", ty: empty }] },
    { kind: "struct", name: "Mixed", fields: [{ name: "value", ty: scalar("f32") }, { name: "empty", ty: empty }] },
    { kind: "slice", element: empty },
  ]) {
    assert.throws(
      () => new PayloadPlan({ params: [{ name: "value", ty }] }),
      /at least one (?:field|scalar)/,
    );
  }
});

test("event input descriptors require aligned storage", () => {
  const memory = new ArrayBuffer(64);
  writeEventInput(memory, 8, 24, 4, 32, 16);
  assert.deepEqual([...new Uint32Array(memory, 8, 4)], [24, 4, 32, 16]);
  assert.throws(() => writeEventInput(memory, 8, 24, 4, 33, 16), /misaligned/);
});

test("plans isolate their schema and reject invalid defaults and value fields", () => {
  const schema = { params: [{ name: "gain", ty: { kind: "scalar", encoding: "f32" }, default: "0.5" }] };
  const plan = new PayloadPlan(schema);
  schema.params[0].ty.encoding = "i32";
  assert.deepEqual(plan.decode(plan.encode({})), { gain: 0.5 });
  assert.throws(() => { plan.tensors[0].elements = 100; }, TypeError);
  schema.params[0].ty.encoding = "f32";
  schema.params[0].default = "garbage";
  assert.throws(() => new PayloadPlan(schema), /floating-point/);
});

test("projects aggregate defaults into flattened ABI parameters", () => {
  const plan = new PayloadPlan({ params: [
    { name: "pair", ty: { kind: "tuple", elements: [scalar("i32"), scalar("f32")] }, default: ["+01", "0.8"] },
    { name: "values", ty: { kind: "array", len: 2, element: scalar("i64") }, default: ["-2", "3"] },
  ] });
  assert.equal(plan.matchesAbiDefault(0, ["1"]), true);
  assert.equal(plan.matchesAbiDefault(1, ["0.800000011920929"]), true);
  assert.equal(plan.matchesAbiDefault(2, ["-2", "+03"]), true);
  assert.equal(plan.matchesAbiDefault(2, ["3", "-2"]), false);
  assert.deepEqual(plan.decode(plan.encode({})), { pair: [1, 0.8], values: [-2n, 3n] });
});

test("i32 defaults use the same signed-decimal grammar as Rust", () => {
  for (const value of ["0x10", "1e2", "1.0", "", " 1", "1 "]) {
    assert.throws(
      () => new PayloadPlan({ params: [{ name: "value", ty: scalar("i32"), default: value }] }),
      /i32 payload default/,
    );
  }
  const valid = [
    ["+01", 1],
    ["-0", 0],
    ["2147483647", 2147483647],
    ["-2147483648", -2147483648],
  ];
  for (const [value, expected] of valid) {
    const plan = new PayloadPlan({ params: [{ name: "value", ty: scalar("i32"), default: value }] });
    assert.deepEqual(plan.decode(plan.encode({})), { value: expected });
  }
});

test("integer ranges use the same signed-decimal grammar as Rust", () => {
  const ranged = (encoding, min, max) => ({
    params: [{
      name: "value",
      ty: {
        kind: "scalar",
        encoding,
        integer_range: {
          min: { type: encoding, value: min },
          max: { type: encoding, value: max },
          mode: "clamp",
        },
      },
    }],
  });

  for (const value of [16, " 16", "16 ", "0x10", "1e2", "1.0", ""]) {
    assert.throws(() => new PayloadPlan(ranged("i64", value, "16")), /integer range/);
    assert.throws(() => new PayloadPlan(ranged("i64", "-16", value)), /integer range/);
  }
  assert.throws(() => new PayloadPlan(ranged("i32", "-2147483649", "0")), /integer range/);
  assert.throws(() => new PayloadPlan(ranged("i64", "1", "0")), /integer range/);
  for (const [encoding, min, max] of [
    ["i32", "+01", "2147483647"],
    ["i64", "-0", "9223372036854775807"],
  ]) {
    const plan = new PayloadPlan(ranged(encoding, min, max));
    assert.deepEqual(plan.tensors[0].domain, {
      min: BigInt(min),
      max: BigInt(max),
      wrap: false,
    });
  }
});

test("i64 values reject non-integer JavaScript coercions", () => {
  const plan = new PayloadPlan({ params: [{ name: "value", ty: scalar("i64") }] });
  for (const value of [null, false, [], {}]) {
    assert.throws(() => plan.encode({ value }), /i64 payload value/);
  }
});

test("parameter defaults ignore inherited object properties", () => {
  const names = ["constructor", "toString", "__proto__"];
  const plan = new PayloadPlan({ params: names.map((name, index) => ({
    name,
    ty: scalar("i32"),
    default: String(index + 1),
  })) });
  const defaults = Object.fromEntries(names.map((name, index) => [name, index + 1]));
  assert.deepEqual(plan.decode(plan.encode({})), defaults);

  const explicit = Object.fromEntries(names.map((name, index) => [name, index + 4]));
  assert.deepEqual(plan.decode(plan.encode(explicit)), explicit);
});

test("top-level payload values reject unknown or excess parameters", () => {
  const plan = new PayloadPlan({ params: [
    { name: "gain", ty: scalar("f32") },
    { name: "enabled", ty: scalar("bool"), default: "true" },
  ] });
  assert.throws(() => plan.encode({ gain: 1, typo: 2 }), /unexpected payload parameter 'typo'/);
  assert.throws(() => plan.encode([1, true, 2]), /parameter count exceeds schema/);
  assert.throws(() => plan.encode(1), /ordered array or named object/);
  assert.deepEqual(plan.decode(plan.encode({ gain: 1 })), { gain: 1, enabled: true });
  assert.deepEqual(plan.decode(plan.encode([1])), { gain: 1, enabled: true });
});

test("struct values require exactly their declared own fields", () => {
  const plan = new PayloadPlan({ params: [{
    name: "item",
    ty: { kind: "struct", name: "Item", fields: [{ name: "value", ty: scalar("i32") }] },
  }] });
  const inherited = Object.create({ value: 7 });
  inherited.unrelated = 9;
  assert.throws(() => plan.encode({ item: inherited }), /exactly its declared fields/);
  assert.throws(() => plan.encode({ item: { value: 7, unrelated: 9 } }), /exactly its declared fields/);
  assert.deepEqual(plan.decode(plan.encode({ item: { value: 7 } })), { item: { value: 7 } });
});
