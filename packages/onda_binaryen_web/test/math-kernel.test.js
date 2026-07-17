import assert from "node:assert/strict";
import test from "node:test";

import { ONDA_MATH_KERNEL_WASM } from "../src/math-kernel.generated.js";

const module = new WebAssembly.Module(ONDA_MATH_KERNEL_WASM);
const instance = new WebAssembly.Instance(module);
const math = instance.exports;
const conversionBuffer = new ArrayBuffer(8);
const conversionView = new DataView(conversionBuffer);

test("embedded math kernel is standalone and exposes both scalar widths", () => {
  assert.deepEqual(WebAssembly.Module.imports(module), []);
  for (const intrinsic of [
    "sin",
    "cos",
    "tan",
    "tanh",
    "atan",
    "atan2",
    "exp",
    "log",
    "pow",
    "fma",
  ]) {
    assert.equal(typeof math[`onda_math_${intrinsic}_f32`], "function");
    assert.equal(typeof math[`onda_math_${intrinsic}_f64`], "function");
  }
});

test("embedded f32 and f64 FMA perform one correctly rounded operation", () => {
  assert.equal(
    f32ToBits(math.onda_math_fma_f32(1 + 2 ** -12, 1 - 2 ** -12, -1)),
    0xb3800000,
    "separate f32 multiply/add incorrectly produces +0",
  );
  assert.equal(
    f32ToBits(math.onda_math_fma_f32(
      f32FromBits(0x00000001),
      f32FromBits(0x3f000000),
      f32FromBits(0x00000001),
    )),
    0x00000002,
    "the exact f32 subnormal midpoint must round to even",
  );
  assert.equal(
    f32ToBits(math.onda_math_fma_f32(
      f32FromBits(0x7f7fffff),
      2,
      f32FromBits(0xff7fffff),
    )),
    0x7f7fffff,
    "an overflowed intermediate product must cancel before rounding",
  );

  assert.equal(
    f64ToBits(math.onda_math_fma_f64(1 + 2 ** -27, 1 - 2 ** -27, -1)),
    0xbc90000000000000n,
    "separate f64 multiply/add incorrectly produces +0",
  );
  assert.equal(
    f64ToBits(math.onda_math_fma_f64(
      f64FromBits(0x0000000000000001n),
      0.5,
      f64FromBits(0x0000000000000001n),
    )),
    0x0000000000000002n,
    "the exact f64 subnormal midpoint must round to even",
  );
  assert.equal(
    f64ToBits(math.onda_math_fma_f64(
      f64FromBits(0x7fefffffffffffffn),
      2,
      f64FromBits(0xffefffffffffffffn),
    )),
    0x7fefffffffffffffn,
    "an overflowed intermediate product must cancel before rounding",
  );
});

test("embedded math functions match reference values across both widths", () => {
  const unary = {
    sin: [-1e6, -Math.PI, -0, 0.25, Math.PI, 1e6],
    cos: [-1e6, -Math.PI, -0, 0.25, Math.PI, 1e6],
    tan: [-100, -1, -0, 0.25, 1, 100],
    tanh: [-100, -1, -0, 0.25, 1, 100],
    atan: [-1e20, -1, -0, 0.25, 1, 1e20],
    exp: [-100, -1, -0, 0.25, 1, 80],
    log: [2 ** -126, 0.25, 1, 10, Number.POSITIVE_INFINITY],
  };
  for (const [intrinsic, inputs] of Object.entries(unary)) {
    for (const scalar of ["f32", "f64"]) {
      const fn = math[`onda_math_${intrinsic}_${scalar}`];
      for (const input of inputs) {
        const argument = scalar === "f32" ? Math.fround(input) : input;
        const expected = Math[intrinsic](argument);
        const actual = fn(argument);
        assertClose(actual, expected, scalar, `${intrinsic}_${scalar}(${input})`);
      }
    }
  }

  const binary = {
    atan2: [[0, 1], [-0, 1], [1, -1], [-1, -1], [1e20, 1]],
    pow: [[2, -3], [0.25, 2.5], [-2, 3], [-2, 0.5], [1, Infinity]],
  };
  for (const [intrinsic, inputs] of Object.entries(binary)) {
    for (const scalar of ["f32", "f64"]) {
      const fn = math[`onda_math_${intrinsic}_${scalar}`];
      for (const operands of inputs) {
        const args = scalar === "f32" ? operands.map(Math.fround) : operands;
        // C/LLVM pow follows IEC 60559 here, while JavaScript Math.pow(1,
        // +/-Infinity) returns NaN. Onda follows the native backend contract.
        const expected = intrinsic === "pow"
          && Math.abs(args[0]) === 1
          && !Number.isFinite(args[1])
          ? 1
          : Math[intrinsic](...args);
        const actual = fn(...args);
        assertClose(
          actual,
          expected,
          scalar,
          `${intrinsic}_${scalar}(${operands.join(", ")})`,
        );
      }
    }
  }
});

function assertClose(actual, expected, scalar, context) {
  if (Number.isNaN(expected)) {
    assert.ok(Number.isNaN(actual), `${context}: expected NaN, got ${actual}`);
    return;
  }
  if (!Number.isFinite(expected) || Object.is(expected, -0)) {
    assert.ok(Object.is(actual, expected), `${context}: ${actual} != ${expected}`);
    return;
  }
  const roundedExpected = scalar === "f32" ? Math.fround(expected) : expected;
  const absoluteTolerance = scalar === "f32" ? 2e-6 : 2e-14;
  const tolerance = absoluteTolerance * Math.max(1, Math.abs(roundedExpected));
  assert.ok(
    Math.abs(actual - roundedExpected) <= tolerance,
    `${context}: ${actual} != ${roundedExpected} (tolerance ${tolerance})`,
  );
}

function f32FromBits(bits) {
  conversionView.setUint32(0, bits, true);
  return conversionView.getFloat32(0, true);
}

function f32ToBits(value) {
  conversionView.setFloat32(0, value, true);
  return conversionView.getUint32(0, true);
}

function f64FromBits(bits) {
  conversionView.setBigUint64(0, bits, true);
  return conversionView.getFloat64(0, true);
}

function f64ToBits(value) {
  conversionView.setFloat64(0, value, true);
  return conversionView.getBigUint64(0, true);
}
