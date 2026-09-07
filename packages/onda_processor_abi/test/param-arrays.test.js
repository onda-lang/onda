import assert from "node:assert/strict";
import test from "node:test";
import { createParamControl, paramElementMetadata } from "../src/index.js";

for (const scalar of ["f32", "f64", "i32", "i64", "bool"]) {
  test(`${scalar} array elements reuse scalar domains and preserve shape`, () => {
    const width = scalar === "bool" ? 1 : scalar.endsWith("64") ? 8 : 4;
    const param = {
      name: "values", scalar, type_repr: `${scalar}[2]`, array_len: 2,
      element_size_bytes: width, byte_size: 2 * width, byte_offset: 16, slot_offset: 3,
      default_reprs: scalar === "bool" ? ["true", "false"] : ["2", "4"],
      range_min_repr: scalar === "bool" ? null : "0",
      range_max_repr: scalar === "bool" ? null : "10",
      param_control: scalar === "bool" ? null : { scale: "linear", curve: null, unit: null, step_repr: "2", step_count: 5 },
    };
    const element = paramElementMetadata(param, 1);
    assert.equal(element.name, "values[1]");
    assert.equal(element.byte_offset, 16 + width);
    assert.deepEqual(element.default_reprs, [param.default_reprs[1]]);
    const control = createParamControl(param, 1);
    assert.equal(control.constrainPlain(100), scalar === "bool" ? true : 10);
    assert.equal(control.normalizedToPlain(0), scalar === "bool" ? false : 0);
    assert.equal(param.array_len, 2);
    assert.throws(() => paramElementMetadata(param, 2), /out of bounds/);
    assert.throws(() => paramElementMetadata(param, -1), /out of bounds/);
  });
}
