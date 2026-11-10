import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createParamControl, createParamDomain, PayloadPlan, validateProcessorMetadata } from "../src/index.js";

const enumeration = { name: "dsp::Mode", members: [
  { name: "LowPass", label: "Low Pass" }, { name: "HighPass", label: "High Pass" },
] };
const scalar = { kind: "scalar", encoding: "i32", enumeration,
  integer_range: { min: { type: "i32", value: "0" }, max: { type: "i32", value: "1" }, mode: "clamp" } };

test("nested enum payloads retain four-byte ordinals and their derived domain", () => {
  const plan = new PayloadPlan({ params: [{ name: "settings", ty: { kind: "struct", name: "Settings", fields: [
    { name: "mode", ty: scalar }, { name: "modes", ty: { kind: "array", len: 2, element: scalar } },
  ] } }] });
  const bytes = plan.encode({ settings: { mode: 99, modes: [-99, 1] } });
  assert.equal(bytes.byteLength, 12);
  assert.deepEqual(plan.decode(bytes), { settings: { mode: 99, modes: [-99, 1] } });
  assert.deepEqual(plan.tensors[0].domain, { min: 0n, max: 1n, wrap: false });
  assert.equal(plan.schema.params[0].ty.fields[0].ty.enumeration.members[1].label, "High Pass");
});

test("enum schemas reject invalid names, storage, and ranges", () => {
  for (const invalid of [
    { ...scalar, enumeration: { ...enumeration, members: [] } },
    { ...scalar, enumeration: { ...enumeration, members: [enumeration.members[0], enumeration.members[0]] } },
    { ...scalar, encoding: "f32" },
    { ...scalar, integer_range: null },
    { ...scalar, integer_range: { ...scalar.integer_range, max: { type: "i32", value: "99" } } },
  ]) assert.throws(() => new PayloadPlan({ params: [{ name: "mode", ty: invalid }] }), /enum/);
});

test("enum controls support stepped ordinals including a single fixed choice", () => {
  for (const count of [1, 2]) {
    const param = { name: "mode", type_repr: "i32", scalar: "i32", array_len: 1,
      enumeration: { ...enumeration, members: enumeration.members.slice(0, count) },
      range_min_repr: "0", range_max_repr: String(count - 1), default_reprs: ["0"],
      param_control: { scale: "linear", curve: null, unit: null, step_repr: "1", step_count: count - 1 } };
    const control = createParamControl(param);
    assert.equal(control.constrainPlain(99), count - 1);
    assert.equal(control.normalizedToPlain(1), count - 1);
    assert.equal(control.plainToNormalized(0), 0);
  }
});

test("decoded singleton domains use the same i32 control conversions", () => {
  const domain = { scalar: "i32", minimum: 0, maximum: 0, scale: "linear", step: 1, stepCount: 0 };
  const control = createParamDomain(domain);
  for (const value of [-Infinity, -99, 0, 0.5, 1, 99, Infinity, NaN]) {
    assert.equal(control.constrainPlain(value), 0);
    assert.equal(control.normalizedToPlain(value), 0);
    assert.equal(control.plainToNormalized(value), 0);
  }
  for (const invalid of [
    { ...domain, scalar: "f32" }, { ...domain, scalar: "i64" },
    { ...domain, step: 2 }, { ...domain, stepCount: 1 }, { ...domain, curve: 1 },
  ]) assert.throws(() => createParamDomain(invalid));
});

test("enum parameter descriptors require their derived domain", () => {
  const fixture = JSON.parse(readFileSync(new URL("./fixtures/processor-descriptor-v7.json", import.meta.url), "utf8"));
  const param = fixture.metadata.params[0];
  Object.assign(param, { type_repr: "i32", scalar: "i32", enumeration,
    default_reprs: ["0"], range_min_repr: "0", range_max_repr: "1",
    param_control: { scale: "linear", curve: null, unit: null, step_repr: "1", step_count: 1 } });
  assert.doesNotThrow(() => validateProcessorMetadata(fixture));
  for (const fields of [
    { range_min_repr: null, range_max_repr: null, param_control: null },
    { range_min_repr: "0", range_max_repr: "1", param_control: null },
    { default_reprs: ["99"] },
  ]) {
    const invalid = structuredClone(fixture);
    Object.assign(invalid.metadata.params[0], fields);
    assert.throws(() => validateProcessorMetadata(invalid));
  }
});
