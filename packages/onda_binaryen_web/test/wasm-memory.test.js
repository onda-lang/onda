import assert from "node:assert/strict";
import test from "node:test";

import {
  scalarSize,
  writeParameterDefaults,
} from "../scripts/wasm-memory.mjs";

test("writes exact scalar parameter representations", () => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  writeParameterDefaults(memory, 16, [
    {
      name: "values",
      scalar: "i64",
      array_len: 2,
      byte_offset: 0,
      default_reprs: ["9223372036854775807", "-9223372036854775808"],
    },
    {
      name: "narrow_specials",
      scalar: "f32",
      array_len: 2,
      byte_offset: 16,
      default_reprs: ["0x7f800000", "0x7fc12345"],
    },
    {
      name: "wide_negative_infinity",
      scalar: "f64",
      array_len: 1,
      byte_offset: 24,
      default_reprs: ["0xfff0000000000000"],
    },
    {
      name: "negative_zero",
      scalar: "f32",
      array_len: 1,
      byte_offset: 32,
      default_reprs: ["-0"],
    },
  ]);

  const view = new DataView(memory.buffer);
  assert.equal(view.getBigInt64(16, true), 9_223_372_036_854_775_807n);
  assert.equal(view.getBigInt64(24, true), -9_223_372_036_854_775_808n);
  assert.equal(view.getUint32(32, true), 0x7f80_0000);
  assert.equal(view.getUint32(36, true), 0x7fc1_2345);
  assert.equal(view.getBigUint64(40, true), 0xfff0_0000_0000_0000n);
  assert.equal(view.getUint32(48, true), 0x8000_0000);
});

test("validates scalar sizes and parameter default shapes", () => {
  assert.deepEqual(
    ["bool", "i32", "i64", "f32", "f64"].map(scalarSize),
    [1, 4, 8, 4, 8],
  );
  assert.throws(
    () => writeParameterDefaults(
      new WebAssembly.Memory({ initial: 1 }),
      0,
      [{
        name: "pair",
        scalar: "f32",
        array_len: 2,
        byte_offset: 0,
        default_reprs: ["0"],
      }],
    ),
    /default has 1 values, expected 2/,
  );
});
