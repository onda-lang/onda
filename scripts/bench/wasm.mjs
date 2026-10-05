// One isolated Node/V8 worker; scripts/bench/run.py supplies its memory/time cgroup.
import { readFileSync, writeFileSync } from "node:fs";
import { performance } from "node:perf_hooks";
import { compileTrustedMir, createDefaultImports } from "../../packages/onda_binaryen_web/src/index.js";
import { scalarSize, writeParameterDefaults } from "../../packages/onda_binaryen_web/scripts/wasm-memory.mjs";

const config = JSON.parse(readFileSync(process.argv[2], "utf8"));
const { prefix, block_size: blockSize } = config;
const percentile = (values, q) => [...values].sort((a, b) => a - b)[Math.round((values.length - 1) * q)];
const mirBytes = readFileSync(`${prefix}.mir.msgpack`);
const started = performance.now();
const artifact = compileTrustedMir(mirBytes, {
  optimizeLevel: config.wasm_opt_level, fastMath: config.fast_math, emitText: config.ir,
});
const compileMs = performance.now() - started;
writeFileSync(`${prefix}.wasm`, artifact.wasm);
writeFileSync(`${prefix}.wasm-metadata.json`, JSON.stringify(artifact.metadata, null, 2));
if (config.ir) writeFileSync(`${prefix}.wat`, artifact.wat);
const result = { compile_ms: compileMs, compile_includes_text: config.ir,
  wasm_bytes: artifact.wasm.length, node_version: process.version, v8_version: process.versions.v8 };
if (config.mode === "inspect") {
  console.log(JSON.stringify(result));
  process.exit(0);
}
const instantiateStarted = performance.now();
const { instance } = await WebAssembly.instantiate(artifact.wasm, createDefaultImports());
result.instantiate_ms = performance.now() - instantiateStarted;
const { memory, __heap_base, onda_processor_init: initialize, onda_process: execute } = instance.exports;
const meta = artifact.metadata;
const ports = (list) => list.map((port) => {
  if (port.array_len !== 1) throw new Error("runner requires scalar ports");
  return port.scalar;
});
if (meta.metadata.buffers.length) throw new Error("runner does not bind external buffers");
const inputTypes = ports(meta.metadata.inputs), outputTypes = ports(meta.metadata.outputs);
if (!outputTypes.length) throw new Error("runner requires an output");
let heap = Number(__heap_base.value);
const allocate = (bytes) => {
  heap = Math.ceil(heap / 16) * 16;
  const pointer = heap;
  heap += Math.max(bytes, 1);
  const pages = Math.ceil(heap / 65536) - memory.buffer.byteLength / 65536;
  if (pages > 0) memory.grow(pages);
  return pointer;
};
const params = allocate(meta.runtime.param_size_bytes);
const state = allocate(meta.runtime.state_size_bytes);
const inputPointers = inputTypes.map((type) => allocate(blockSize * scalarSize(type)));
const outputPointers = outputTypes.map((type) => allocate(blockSize * scalarSize(type)));
const inputTable = allocate(inputPointers.length * 4), outputTable = allocate(outputPointers.length * 4);
const view = new DataView(memory.buffer);
const writeScalar = (pointer, type, value) => {
  switch (type) {
    case "f32": view.setFloat32(pointer, value, true); break;
    case "f64": view.setFloat64(pointer, value, true); break;
    case "i32": view.setInt32(pointer, Math.trunc(Math.fround(value * 64)), true); break;
    case "i64": view.setBigInt64(pointer, BigInt(Math.trunc(Math.fround(value * 64))), true); break;
    case "bool": view.setUint8(pointer, value > 0 ? 1 : 0); break;
    default: throw new Error(`unsupported port type ${type}`);
  }
};
const readScalar = (data, pointer, type) => {
  switch (type) {
    case "f32": return data.getFloat32(pointer, true);
    case "f64": return data.getFloat64(pointer, true);
    case "i32": return data.getInt32(pointer, true);
    case "i64": return data.getBigInt64(pointer, true);
    case "bool": return data.getUint8(pointer);
    default: throw new Error(`unsupported port type ${type}`);
  }
};
inputPointers.forEach((pointer, channel) => {
  view.setUint32(inputTable + channel * 4, pointer, true);
  let seed = (config.input_seed + channel) >>> 0;
  for (let frame = 0; frame < blockSize; frame++) {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    let x;
    switch (config.input_pattern) {
      case "zero": x = 0; break;
      case "impulse": x = frame === 0 ? 1 : 0; break;
      case "ramp": x = Math.fround((frame % 257 - 128) / 128); break;
      default: x = Math.fround(Math.fround(Math.fround((seed >>> 8) / 16777216) * Math.fround(1.6)) - Math.fround(0.8));
    }
    writeScalar(pointer + frame * scalarSize(inputTypes[channel]), inputTypes[channel], x);
  }
});
outputPointers.forEach((pointer, channel) => view.setUint32(outputTable + channel * 4, pointer, true));
writeParameterDefaults(memory, params, meta.metadata.params);
const check = (status) => { if (status !== 0) throw new Error(`execution status ${status}`); };
const initStarted = performance.now();
check(initialize(params, state, 1, 0, 0, 0, 0, 0));
const initMs = performance.now() - initStarted;
const processBlock = () => check(execute(state, params, inputTable, outputTable, 0, blockSize, 3, 0, 0, 0, 0));
let maxError = 0, validationSamples = 0, coldMaxUs = 0;
const native = config.compare_native ? JSON.parse(readFileSync(`${prefix}.native.json`, "utf8")) : null;
if (native && (JSON.stringify(native.inputs) !== JSON.stringify(inputTypes) || JSON.stringify(native.outputs) !== JSON.stringify(outputTypes))) {
  throw new Error("native/Wasm port layouts differ");
}
const blockBytes = blockSize * outputTypes.reduce((sum, type) => sum + scalarSize(type), 0);
const validate = (extension, cold = false) => {
  const bytes = Buffer.alloc(blockBytes * config.validation_blocks);
  const expected = native ? readFileSync(`${prefix}.native.${extension}`) : null;
  if (expected && expected.length !== bytes.length) throw new Error("native snapshot byte length differs");
  const reference = expected ? new DataView(expected.buffer, expected.byteOffset, expected.byteLength) : null;
  let offset = 0;
  for (let block = 0; block < config.validation_blocks; block++) {
    const start = performance.now(); processBlock();
    if (cold) coldMaxUs = Math.max(coldMaxUs, (performance.now() - start) * 1000);
    outputPointers.forEach((pointer, channel) => {
      const type = outputTypes[channel], width = scalarSize(type);
      for (let frame = 0; frame < blockSize; frame++) {
        const actual = readScalar(view, pointer + frame * width, type);
        const numeric = Number(actual);
        if (!Number.isFinite(numeric) || (config.max_output_abs !== null && Math.abs(numeric) > config.max_output_abs)) {
          throw new Error(`output violates finite/error-bound contract: ${actual}`);
        }
        if (reference) {
          const expectedValue = readScalar(reference, offset + frame * width, type);
          let error, failed;
          if (type === "f32" || type === "f64") {
            error = Math.abs(actual - expectedValue);
            failed = !Number.isFinite(expectedValue) || error > config.atol + config.rtol * Math.max(Math.abs(actual), Math.abs(expectedValue));
            if (config.bitwise) {
              const method = type === "f32" ? "getUint32" : "getBigUint64";
              failed ||= view[method](pointer + frame * width, true) !== reference[method](offset + frame * width, true);
            }
          } else {
            error = Number(actual > expectedValue ? actual - expectedValue : expectedValue - actual);
            failed = actual !== expectedValue;
          }
          // Scaled error-output fixtures have an independent bound; their tiny
          // numerical diagnostics need not be identical across math backends.
          if (config.max_output_abs === null && failed) {
            throw new Error(`parity block ${block}, channel ${channel}, frame ${frame}: ${actual} vs ${expectedValue}${config.bitwise ? " (bitwise)" : ""}`);
          }
          maxError = Math.max(maxError, error);
        }
        validationSamples++;
      }
      bytes.set(new Uint8Array(memory.buffer, pointer, blockSize * width), offset);
      offset += blockSize * width;
    });
  }
  writeFileSync(`${prefix}.wasm.${extension}`, bytes);
};
validate("output.bin", true);
for (let i = 0; i < config.warmup_blocks; i++) processBlock();
validate("warm-output.bin");
let iterations = 256;
for (;;) {
  const start = performance.now();
  for (let i = 0; i < iterations; i++) processBlock();
  if (performance.now() - start >= config.round_ms) break;
  iterations *= 2;
  if (!Number.isSafeInteger(iterations)) throw new Error("iteration overflow");
}
const rounds = [];
for (let r = 0; r < config.repetitions; r++) {
  const start = performance.now();
  for (let i = 0; i < iterations; i++) processBlock();
  rounds.push((performance.now() - start) * 1e6 / (iterations * blockSize));
}
const latencies = [];
for (let i = 0; i < config.latency_blocks; i++) {
  const start = performance.now(); processBlock(); latencies.push((performance.now() - start) * 1000);
}
for (const [channel, pointer] of outputPointers.entries()) {
  for (let frame = 0; frame < blockSize; frame++) {
    const x = Number(readScalar(view, pointer + frame * scalarSize(outputTypes[channel]), outputTypes[channel]));
    if (!Number.isFinite(x) || (config.max_output_abs !== null && Math.abs(x) > config.max_output_abs)) throw new Error("invalid final output");
  }
}
const median = percentile(rounds, 0.5);
console.log(JSON.stringify({ ...result, init_ms: initMs, state_bytes: meta.runtime.state_size_bytes,
  inputs: inputTypes, outputs: outputTypes, iterations, ns_per_frame: median, rounds_ns_per_frame: rounds,
  mad_ns: percentile(rounds.map(x => Math.abs(x - median)), 0.5), min_ns: percentile(rounds, 0), max_ns: percentile(rounds, 1),
  block_p50_us: percentile(latencies, 0.5), block_p99_us: percentile(latencies, 0.99), block_max_us: percentile(latencies, 1),
  cold_max_us: coldMaxUs, validation_samples: validationSamples, parity_max_abs: native ? maxError : null,
  validation: config.max_output_abs !== null ? "bounded-error" : native ? "native-parity" : "finite",
  wasm_to_native: native ? median / native.ns_per_frame : null,
}));
