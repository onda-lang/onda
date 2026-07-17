import assert from "node:assert/strict";
import test from "node:test";

import {
  createExactMathImports,
  exactFmaF32Bits,
  exactFmaF64Bits,
  ONDA_EXACT_MATH_ABI_VERSION,
  ONDA_EXACT_MATH_IMPORT_MODULE,
} from "../src/index.js";

test("exact FMA support has an explicit versioned bit ABI", () => {
  assert.equal(ONDA_EXACT_MATH_ABI_VERSION, 1);
  assert.equal(ONDA_EXACT_MATH_IMPORT_MODULE, "onda_exact_math_v1");
  const support = createExactMathImports()[ONDA_EXACT_MATH_IMPORT_MODULE];
  assert.equal(typeof support.fma_f32_bits, "function");
  assert.equal(typeof support.fma_f64_bits, "function");

  // Exercise the signed JS values used by Wasm's i32/i64 import boundary.
  assert.equal(
    BigInt.asUintN(32, BigInt(support.fma_f32_bits(-2147483648, 0x40000000, -2147483648))),
    0x80000000n,
  );
  assert.equal(
    BigInt.asUintN(
      64,
      support.fma_f64_bits(
        BigInt.asIntN(64, 0x8000000000000000n),
        0x4000000000000000n,
        BigInt.asIntN(64, 0x8000000000000000n),
      ),
    ),
    0x8000000000000000n,
  );
});

test("f32 FMA is fused across hard rounding and cancellation cases", () => {
  assert.equal(
    exactFmaF32Bits(...f32Bits(1 + 2 ** -12, 1 - 2 ** -12, -1)),
    0xb3800000,
    "separate f32 multiply/add incorrectly produces +0",
  );
  assert.equal(
    exactFmaF32Bits(0x00000001, 0x3f000000, 0x00000001),
    0x00000002,
    "the exact 1.5-min-subnormal midpoint must round to the even subnormal",
  );
  assert.equal(
    exactFmaF32Bits(0x7f7fffff, 0x40000000, 0xff7fffff),
    0x7f7fffff,
    "overflowed intermediate product must cancel before rounding",
  );
});

test("f64 FMA is fused across hard rounding and cancellation cases", () => {
  assert.equal(
    exactFmaF64Bits(...f64Bits(1 + 2 ** -27, 1 - 2 ** -27, -1)),
    0xbc90000000000000n,
    "separate f64 multiply/add incorrectly produces +0",
  );
  assert.equal(
    exactFmaF64Bits(
      0x0000000000000001n,
      0x3fe0000000000000n,
      0x0000000000000001n,
    ),
    0x0000000000000002n,
    "the exact 1.5-min-subnormal midpoint must round to the even subnormal",
  );
  assert.equal(
    exactFmaF64Bits(
      0x7fefffffffffffffn,
      0x4000000000000000n,
      0xffefffffffffffffn,
    ),
    0x7fefffffffffffffn,
    "overflowed intermediate product must cancel before rounding",
  );
});

test("exact FMA handles NaN, infinity, cancellation zero, and signed zero deterministically", () => {
  assert.equal(exactFmaF32Bits(0x7fa01234, 0x3f800000, 0x3f800000), 0x7fc00000);
  assert.equal(
    exactFmaF64Bits(
      0x7ff0000000001234n,
      0x3ff0000000000000n,
      0x3ff0000000000000n,
    ),
    0x7ff8000000000000n,
  );
  assert.equal(exactFmaF32Bits(0x7f800000, 0, 0x3f800000), 0x7fc00000);
  assert.equal(
    exactFmaF64Bits(0x7ff0000000000000n, 0n, 0x3ff0000000000000n),
    0x7ff8000000000000n,
  );
  assert.equal(
    exactFmaF32Bits(0x7f800000, 0x40000000, 0xff800000),
    0x7fc00000,
  );
  assert.equal(
    exactFmaF64Bits(
      0x7ff0000000000000n,
      0x4000000000000000n,
      0xfff0000000000000n,
    ),
    0x7ff8000000000000n,
  );
  assert.equal(exactFmaF32Bits(0xff800000, 0x40000000, 0), 0xff800000);
  assert.equal(
    exactFmaF64Bits(0xfff0000000000000n, 0x4000000000000000n, 0n),
    0xfff0000000000000n,
  );
  assert.equal(exactFmaF32Bits(...f32Bits(1, 1, -1)), 0);
  assert.equal(exactFmaF64Bits(...f64Bits(1, 1, -1)), 0n);
  assert.equal(exactFmaF32Bits(0x80000000, 0x40000000, 0x80000000), 0x80000000);
  assert.equal(exactFmaF32Bits(0x80000000, 0x40000000, 0), 0);
  assert.equal(exactFmaF32Bits(0x80000001, 0x3e800000, 0x80000000), 0x80000000);
  assert.equal(
    exactFmaF64Bits(
      0x8000000000000000n,
      0x4000000000000000n,
      0x8000000000000000n,
    ),
    0x8000000000000000n,
  );
  assert.equal(
    exactFmaF64Bits(0x8000000000000000n, 0x4000000000000000n, 0n),
    0n,
  );
  assert.equal(
    exactFmaF64Bits(
      0x8000000000000001n,
      0x3fd0000000000000n,
      0x8000000000000000n,
    ),
    0x8000000000000000n,
  );
});

const conversionBuffer = new ArrayBuffer(8);
const conversionView = new DataView(conversionBuffer);

function f32Bits(...values) {
  return values.map((value) => {
    conversionView.setFloat32(0, value, true);
    return conversionView.getUint32(0, true);
  });
}

function f64Bits(...values) {
  return values.map((value) => {
    conversionView.setFloat64(0, value, true);
    return conversionView.getBigUint64(0, true);
  });
}
