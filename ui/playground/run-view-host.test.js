import assert from "node:assert/strict";
import test from "node:test";

import { mergeParams } from "./run-view-host.js";

function scalarParam(overrides = {}) {
  return {
    name: "freq",
    type_repr: "f32",
    scalar: "f32",
    array_len: 1,
    default_reprs: ["440"],
    range_min_repr: "20",
    range_max_repr: "1000",
    ...overrides,
  };
}

test("maps scalar artifact defaults and ranges to run-view params", () => {
  const params = mergeParams([
    scalarParam(),
    scalarParam({
      name: "enabled",
      type_repr: "bool",
      scalar: "bool",
      default_reprs: ["true"],
      range_min_repr: null,
      range_max_repr: null,
    }),
    scalarParam({
      name: "partials",
      type_repr: "f32[2]",
      array_len: 2,
      default_reprs: ["0.5", "0.25"],
    }),
  ], []);

  assert.deepEqual(params, [
    {
      index: 0,
      name: "freq",
      type: "f32",
      default: 440,
      rangeMin: 20,
      rangeMax: 1000,
      scalar: true,
      value: 440,
    },
    {
      index: 1,
      name: "enabled",
      type: "bool",
      default: true,
      rangeMin: null,
      rangeMax: null,
      scalar: true,
      value: true,
    },
  ]);
});

test("preserves an edited value only while the artifact param shape matches", () => {
  const [initial] = mergeParams([scalarParam()], []);
  const [preserved] = mergeParams([scalarParam()], [{ ...initial, value: 880 }]);
  const [reset] = mergeParams([
    scalarParam({ default_reprs: ["220"] }),
  ], [{ ...initial, value: 880 }]);

  assert.equal(preserved.value, 880);
  assert.equal(reset.value, 220);
});

test("decodes floating-point bit-pattern representations", () => {
  const [param] = mergeParams([
    scalarParam({
      scalar: "f64",
      type_repr: "f64",
      default_reprs: ["0x3ff8000000000000"],
      range_min_repr: "0x0000000000000000",
      range_max_repr: "0x4000000000000000",
    }),
  ], []);

  assert.equal(param.value, 1.5);
  assert.equal(param.rangeMin, 0);
  assert.equal(param.rangeMax, 2);
});
