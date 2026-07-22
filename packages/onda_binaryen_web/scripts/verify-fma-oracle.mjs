import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { ONDA_MATH_KERNEL_WASM } from "../src/math-kernel.generated.js";

const conversionBuffer = new ArrayBuffer(8);
const conversionView = new DataView(conversionBuffer);
const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const temporary = mkdtempSync(join(tmpdir(), "onda-fma-oracle-"));
const oracle = join(
  temporary,
  process.platform === "win32" ? "fma-oracle.exe" : "fma-oracle",
);
const { instance: mathKernel } = await WebAssembly.instantiate(
  ONDA_MATH_KERNEL_WASM,
);

try {
  execFileSync(
    "rustc",
    [join(packageDir, "scripts/fma-oracle.rs"), "-O", "-o", oracle],
    { stdio: "inherit" },
  );

  const vectors = [...fixedVectors(), ...randomVectors(10_000)];
  const input = `${vectors
    .map(({ scalar, operands }) =>
      [scalar, ...operands.map((value) => hex(value, scalar))].join(" "),
    )
    .join("\n")}\n`;
  const expected = (await runOracle(oracle, input))
    .trimEnd()
    .split("\n");
  if (expected.length !== vectors.length) {
    throw new Error(
      `native FMA oracle returned ${expected.length} results for ${vectors.length} vectors`,
    );
  }

  let nanResults = 0;
  vectors.forEach(({ scalar, operands }, index) => {
    const expectedBits = BigInt(`0x${expected[index]}`);
    const actualBits = scalar === "f32"
      ? BigInt(valueToF32Bits(mathKernel.exports.onda_math_fma_f32(
        ...operands.map(f32BitsToValue),
      )))
      : valueToF64Bits(mathKernel.exports.onda_math_fma_f64(
        ...operands.map(f64BitsToValue),
      ));
    if (isNaNBits(expectedBits, scalar)) {
      nanResults += 1;
      if (!isNaNBits(actualBits, scalar)) {
        failVector(index, scalar, operands, expectedBits, actualBits);
      }
    } else if (actualBits !== expectedBits) {
      failVector(index, scalar, operands, expectedBits, actualBits);
    }
  });

  process.stdout.write(
    `Verified exact FMA support against Rust mul_add: ${vectors.length} vectors (${nanResults} NaN results)\n`,
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

function runOracle(executable, input) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, [], { stdio: ["pipe", "pipe", "inherit"] });
    let output = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      output += chunk;
    });
    child.on("error", rejectPromise);
    child.on("close", (status, signal) => {
      if (status === 0) resolvePromise(output);
      else {
        rejectPromise(
          new Error(
            `native FMA oracle exited with ${signal ? `signal ${signal}` : `status ${status}`}`,
          ),
        );
      }
    });
    child.stdin.end(input);
  });
}

function* fixedVectors() {
  const f32 = valuesToF32Bits;
  const f64 = valuesToF64Bits;
  const cases = [
    // Exact cancellation that a separate multiply/add rounds to zero.
    { scalar: "f32", operands: f32(1 + 2 ** -12, 1 - 2 ** -12, -1) },
    { scalar: "f64", operands: f64(1 + 2 ** -27, 1 - 2 ** -27, -1) },
    // Underflow midpoint and overflow cancellation.
    { scalar: "f32", operands: [0x00000001n, 0x3f000000n, 0x00000001n] },
    { scalar: "f64", operands: [0x0000000000000001n, 0x3fe0000000000000n, 0x0000000000000001n] },
    { scalar: "f32", operands: [0x7f7fffffn, 0x40000000n, 0xff7fffffn] },
    { scalar: "f64", operands: [0x7fefffffffffffffn, 0x4000000000000000n, 0xffefffffffffffffn] },
    // Signed zero and exact non-zero cancellation.
    { scalar: "f32", operands: [0x80000000n, 0x40000000n, 0x80000000n] },
    { scalar: "f32", operands: [0x80000000n, 0x40000000n, 0x00000000n] },
    { scalar: "f64", operands: [0x8000000000000000n, 0x4000000000000000n, 0x8000000000000000n] },
    { scalar: "f64", operands: [0x8000000000000000n, 0x4000000000000000n, 0x0000000000000000n] },
    { scalar: "f32", operands: [0x80000001n, 0x3e800000n, 0x80000000n] },
    { scalar: "f64", operands: [0x8000000000000001n, 0x3fd0000000000000n, 0x8000000000000000n] },
    { scalar: "f32", operands: f32(1, 1, -1) },
    { scalar: "f64", operands: f64(1, 1, -1) },
    // NaN and infinity invalid/propagation cases.
    { scalar: "f32", operands: [0x7f800000n, 0x00000000n, 0x3f800000n] },
    { scalar: "f32", operands: [0x7f800000n, 0x40000000n, 0xff800000n] },
    { scalar: "f32", operands: [0x7fc01234n, 0x3f800000n, 0x3f800000n] },
    { scalar: "f64", operands: [0x7ff0000000000000n, 0x0000000000000000n, 0x3ff0000000000000n] },
    { scalar: "f64", operands: [0x7ff0000000000000n, 0x4000000000000000n, 0xfff0000000000000n] },
    { scalar: "f64", operands: [0x7ff8000000001234n, 0x3ff0000000000000n, 0x3ff0000000000000n] },
  ];
  yield* cases;
}

function* randomVectors(countPerScalar) {
  let state = 0x4f4e44415f464d41n;
  const next = () => {
    state = BigInt.asUintN(64, state ^ (state << 13n));
    state ^= state >> 7n;
    state = BigInt.asUintN(64, state ^ (state << 17n));
    return state;
  };
  for (let index = 0; index < countPerScalar; index += 1) {
    yield {
      scalar: "f32",
      operands: [next() & 0xffffffffn, next() & 0xffffffffn, next() & 0xffffffffn],
    };
    yield { scalar: "f64", operands: [next(), next(), next()] };
  }
}

function hex(value, scalar) {
  return value.toString(16).padStart(scalar === "f32" ? 8 : 16, "0");
}

function isNaNBits(bits, scalar) {
  if (scalar === "f32") {
    return (bits & 0x7f800000n) === 0x7f800000n && (bits & 0x007fffffn) !== 0n;
  }
  return (
    (bits & 0x7ff0000000000000n) === 0x7ff0000000000000n
    && (bits & 0x000fffffffffffffn) !== 0n
  );
}

function failVector(index, scalar, operands, expected, actual) {
  throw new Error(
    `FMA vector ${index} (${scalar} ${operands.map((value) => hex(value, scalar)).join(" ")}) expected ${hex(expected, scalar)}, got ${hex(actual, scalar)}`,
  );
}

function valuesToF32Bits(...values) {
  return values.map((value) => {
    conversionView.setFloat32(0, value, true);
    return BigInt(conversionView.getUint32(0, true));
  });
}

function valuesToF64Bits(...values) {
  return values.map((value) => {
    conversionView.setFloat64(0, value, true);
    return conversionView.getBigUint64(0, true);
  });
}

function f32BitsToValue(bits) {
  conversionView.setUint32(0, Number(bits), true);
  return conversionView.getFloat32(0, true);
}

function valueToF32Bits(value) {
  conversionView.setFloat32(0, value, true);
  return conversionView.getUint32(0, true);
}

function f64BitsToValue(bits) {
  conversionView.setBigUint64(0, bits, true);
  return conversionView.getFloat64(0, true);
}

function valueToF64Bits(value) {
  conversionView.setFloat64(0, value, true);
  return conversionView.getBigUint64(0, true);
}
