import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { cpus, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import binaryen from "binaryen";
import {
  compileTrustedMir as compileMir,
  createDefaultImports,
} from "../src/index.js";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(repoDir, process.env.CARGO_TARGET_DIR)
  : join(repoDir, "target");
const executableSuffix = process.platform === "win32" ? ".exe" : "";
const ondaCli =
  process.env.ONDA_CLI ??
  join(targetDir, "debug", `onda${executableSuffix}`);
const nativeRunner = join(
  targetDir,
  "release",
  "examples",
  `benchmark_mir_native${executableSuffix}`,
);
const sampleRate = 48_000;
const blockSize = Number(process.env.ONDA_BENCH_BLOCK_SIZE ?? 128);
const iterations = Number(process.env.ONDA_BENCH_ITERATIONS ?? 2_000);
const repetitions = Number(process.env.ONDA_BENCH_REPETITIONS ?? 5);
const compileRepetitions = Number(
  process.env.ONDA_BENCH_COMPILE_REPETITIONS ?? 5,
);
const minimumRoundMs = Number(process.env.ONDA_BENCH_MIN_ROUND_MS ?? 50);
const binaryenOptimizeLevel = Number(
  process.env.ONDA_BENCH_BINARYEN_OPT_LEVEL ?? 4,
);
const binaryenStackIr = parseBooleanEnvironment(
  process.env.ONDA_BENCH_BINARYEN_STACK_IR,
  false,
  "ONDA_BENCH_BINARYEN_STACK_IR",
);
const requireLlvmWin = parseBooleanEnvironment(
  process.env.ONDA_BENCH_REQUIRE_LLVM_WIN,
  true,
  "ONDA_BENCH_REQUIRE_LLVM_WIN",
);
const minimumWasmToLlvmRatio = Number(
  process.env.ONDA_BENCH_MIN_WASM_TO_LLVM_RATIO ?? 1.0,
);
const scenarios = [
  {
    name: "language",
    source: join(packageDir, "test/fixtures/language-slice.onda"),
  },
  {
    name: "oversampling",
    source: join(packageDir, "test/fixtures/oversampling-parity.onda"),
  },
  {
    name: "saturator",
    source: join(
      repoDir,
      "examples/basic/saw_filter_saturator.onda",
    ),
  },
  {
    name: "math-intrinsics",
    source: join(packageDir, "test/fixtures/math-intrinsics-parity.onda"),
  },
  {
    name: "buffer-sequential",
    source: join(packageDir, "test/fixtures/benchmark-buffer-sequential.onda"),
  },
  {
    name: "buffer-interpolation",
    source: join(packageDir, "test/fixtures/benchmark-buffer-interpolation.onda"),
  },
  {
    name: "buffer-collection-constant",
    source: join(
      packageDir,
      "test/fixtures/benchmark-buffer-collection-constant.onda",
    ),
  },
  {
    name: "buffer-collection-invariant",
    source: join(
      packageDir,
      "test/fixtures/benchmark-buffer-collection-invariant.onda",
    ),
  },
  {
    name: "buffer-collection-forwarded-invariant",
    source: join(
      packageDir,
      "test/fixtures/benchmark-buffer-collection-forwarded-invariant.onda",
    ),
  },
  {
    name: "buffer-collection-varying",
    source: join(
      packageDir,
      "test/fixtures/benchmark-buffer-collection-varying.onda",
    ),
  },
];

validatePositiveInteger(blockSize, "block size");
validatePositiveInteger(iterations, "iterations");
validatePositiveInteger(repetitions, "repetitions");
validatePositiveInteger(compileRepetitions, "compile repetitions");
validatePositiveNumber(minimumRoundMs, "minimum timing-round milliseconds");
validateIntegerInRange(
  binaryenOptimizeLevel,
  0,
  4,
  "Binaryen optimization level",
);
validatePositiveNumber(
  minimumWasmToLlvmRatio,
  "minimum Wasm-to-LLVM runtime ratio",
);

pinBenchmarkProcess();

execFileSync(
  "cargo",
  ["build", "-q", "-p", "onda_cli"],
  { cwd: repoDir, stdio: "inherit" },
);
execFileSync(
  "cargo",
  [
    "build",
    "-q",
    "-p",
    "onda_examples",
    "--example",
    "benchmark_mir_native",
    "--release",
  ],
  { cwd: repoDir, stdio: "inherit" },
);

const temporary = mkdtempSync(join(tmpdir(), "onda-backend-benchmark-"));
const results = [];
const previousGenerateStackIr = binaryen.getGenerateStackIR();
const previousOptimizeStackIr = binaryen.getOptimizeStackIR();
binaryen.setGenerateStackIR(binaryenStackIr);
binaryen.setOptimizeStackIR(binaryenStackIr);
try {
  for (const [scenarioId, scenario] of scenarios.entries()) {
    const mirPath = join(temporary, `scenario-${scenarioId}.mir.msgpack`);
    const mirCompileStarted = performance.now();
    execFileSync(
      ondaCli,
      [
        "compile",
        scenario.source,
        "--emit",
        "mir-messagepack",
        "--output",
        mirPath,
        "--sample-rate",
        String(sampleRate),
        "--block-size",
        String(blockSize),
      ],
      { cwd: repoDir, stdio: "ignore" },
    );
    const mirCompileMs = performance.now() - mirCompileStarted;
    const mirTransport = readFileSync(mirPath);

    const binaryenSamples = [];
    let artifact;
    for (let repetition = 0; repetition <= compileRepetitions; repetition += 1) {
      const started = performance.now();
      artifact = compileMir(mirTransport, {
        optimizeLevel: binaryenOptimizeLevel,
      });
      const elapsed = performance.now() - started;
      if (repetition > 0) binaryenSamples.push(elapsed);
    }

    const instantiateSamples = [];
    for (let repetition = 0; repetition <= compileRepetitions; repetition += 1) {
      const started = performance.now();
      await WebAssembly.instantiate(artifact.wasm, createDefaultImports());
      const elapsed = performance.now() - started;
      if (repetition > 0) instantiateSamples.push(elapsed);
    }

    const binaryen = summarize(binaryenSamples);
    const instantiation = summarize(instantiateSamples);
    const wasmBenchmark = await prepareWasmBenchmark(artifact);
    const expectedOutputsPath = join(
      temporary,
      `scenario-${scenarioId}.first-block.f32le`,
    );
    writeFirstBlockFixture(expectedOutputsPath, wasmBenchmark.firstOutputs);
    const nativePreflightArgs = [
      nativeRunner,
      scenario.source,
      String(blockSize),
      String(iterations),
      String(repetitions),
      String(compileRepetitions),
      expectedOutputsPath,
      String(minimumRoundMs),
    ];
    const preflightOutput = execCaptured(
      process.env.RTK ?? "rtk",
      [...nativePreflightArgs, "--validate-only"],
      { cwd: repoDir, encoding: "utf8" },
    );
    const preflight = parseLastJsonLine(preflightOutput);
    const expectedParitySamples =
      wasmBenchmark.firstOutputs.length * blockSize;
    assertParitySummary(
      preflight,
      expectedParitySamples,
      wasmBenchmark.firstOutputs.length,
      `${scenario.name} preflight`,
    );
    validatePositiveInteger(
      preflight.recommended_iterations,
      `${scenario.name} native calibrated iteration count`,
    );

    const wasmIterations = wasmBenchmark.calibrate(
      iterations,
      minimumRoundMs,
    );
    const timedIterations = Math.max(
      iterations,
      preflight.recommended_iterations,
      wasmIterations,
    );
    const wasm = wasmBenchmark.measure(timedIterations);
    const nativeArgs = [
      nativeRunner,
      scenario.source,
      String(blockSize),
      String(timedIterations),
      String(repetitions),
      String(compileRepetitions),
      expectedOutputsPath,
      String(minimumRoundMs),
    ];
    const nativeOutput = execCaptured(
      process.env.RTK ?? "rtk",
      nativeArgs,
      { cwd: repoDir, encoding: "utf8" },
    );
    const native = parseLastJsonLine(nativeOutput);
    assertParitySummary(
      native,
      expectedParitySamples,
      wasmBenchmark.firstOutputs.length,
      `${scenario.name} measured native run`,
    );

    results.push({
      scenario: scenario.name,
      mirBytes: mirTransport.byteLength,
      wasmBytes: artifact.wasm.byteLength,
      iterations: timedIterations,
      sourceToMirMs: mirCompileMs,
      binaryen,
      instantiation,
      wasmProcess: wasm,
      llvmCompile: {
        median: native.compile_ms,
        mad: native.compile_mad_ms,
        minimum: native.compile_min_ms,
        maximum: native.compile_max_ms,
      },
      llvmProcess: {
        median: native.process_ns_per_frame,
        mad: native.process_mad_ns_per_frame,
        minimum: native.process_min_ns_per_frame,
        maximum: native.process_max_ns_per_frame,
      },
      parityMaxAbsoluteError: Math.max(
        preflight.parity_max_abs_error,
        native.parity_max_abs_error,
      ),
      wasmToLlvm: wasm.median / native.process_ns_per_frame,
    });
  }
} finally {
  binaryen.setGenerateStackIR(previousGenerateStackIr);
  binaryen.setOptimizeStackIR(previousOptimizeStackIr);
  rmSync(temporary, { recursive: true, force: true });
}

const llvmRegressions = requireLlvmWin
  ? results.filter((result) => result.wasmToLlvm < minimumWasmToLlvmRatio)
  : [];
const host = benchmarkHost();
process.stdout.write(
  [
    `Onda backend benchmark: ${blockSize}-frame blocks, at least ${iterations} blocks × ${repetitions} timing rounds`,
    `Host: ${host.cpuModel}; logical CPUs: ${host.logicalCpuCount}; allowed CPUs: ${host.allowedCpus}.`,
    `Binaryen O${binaryenOptimizeLevel}, strict arithmetic, SIMD enabled, StackIR ${binaryenStackIr ? "enabled" : "disabled"}; timing cells are median ± MAD across ${compileRepetitions} compile/instantiate samples or ${repetitions} throughput rounds.`,
    "First-block parity checks every f32 output sample (absolute and relative tolerance 1e-6) before throughput timing.",
    `Each scenario uses one shared native/Wasm block count calibrated to target at least ${fixed(minimumRoundMs)} ms per round.`,
    "Both throughput paths call the raw validated onda_process backend entry and reject nonzero execution status after every block; daemon/worklet adapter overhead is excluded.",
    requireLlvmWin
      ? `LLVM win gate: every Wasm/LLVM ratio must be at least ${minimumWasmToLlvmRatio.toFixed(2)}×.`
      : "LLVM win gate: disabled by ONDA_BENCH_REQUIRE_LLVM_WIN.",
    "",
    "| scenario | blocks/round | MIR KiB | Wasm KiB | source→MIR ms* | Binaryen ms | instantiate ms | LLVM JIT ms | LLVM ns/frame | Wasm ns/frame | Wasm/LLVM | parity max abs |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...results.map((result) =>
      `| ${result.scenario} | ${result.iterations} | ${kib(result.mirBytes)} | ${kib(result.wasmBytes)} | ${fixed(result.sourceToMirMs)} | ${timing(result.binaryen)} | ${timing(result.instantiation)} | ${timing(result.llvmCompile)} | ${timing(result.llvmProcess)} | ${timing(result.wasmProcess)} | ${result.wasmToLlvm.toFixed(2)}× | ${result.parityMaxAbsoluteError.toExponential(2)} |`
    ),
    "",
    "* source→MIR is measured through the native CLI process and includes process startup; it is not the browser compiler's in-page latency.",
    "One in-process compile/instantiate warmup is discarded before sampling; throughput uses 200 warmup blocks.",
    "MAD is the median absolute deviation. Scheduler activity, CPU frequency, thermals, and JIT/code-cache state can still move short development runs; repeat the whole command for consequential comparisons.",
    "",
  ].join("\n"),
);
if (llvmRegressions.length > 0) {
  process.stderr.write(
    `LLVM performance gate failed: ${llvmRegressions
      .map((result) =>
        `${result.scenario} was ${result.wasmToLlvm.toFixed(2)}× (required ${minimumWasmToLlvmRatio.toFixed(2)}×)`
      )
      .join(", ")}\n`,
  );
  process.exitCode = 1;
}

async function prepareWasmBenchmark(artifact) {
  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const { memory, __heap_base, onda_processor_init, onda_process } = instance.exports;
  const metadata = artifact.metadata;
  let heap = Number(__heap_base.value);
  const allocate = (bytes, alignment = 16) => {
    heap = Math.ceil(heap / alignment) * alignment;
    const pointer = heap;
    heap += Math.max(bytes, 1);
    const requiredPages = Math.ceil(heap / (64 * 1024));
    const currentPages = memory.buffer.byteLength / (64 * 1024);
    if (requiredPages > currentPages) memory.grow(requiredPages - currentPages);
    return pointer;
  };

  const params = allocate(metadata.runtime.param_size_bytes);
  const state = allocate(metadata.runtime.state_size_bytes);
  const inputs = flattenPorts(metadata.metadata.inputs);
  if (inputs.length) {
    throw new Error("benchmark scenarios must not require audio inputs");
  }
  const outputs = flattenPorts(metadata.metadata.outputs);
  if (!outputs.length || outputs.some((output) => output.scalar !== "f32")) {
    throw new Error("benchmark scenarios must expose scalar f32 audio outputs");
  }
  const outputTable = outputs.length ? allocate(outputs.length * 4, 4) : 0;
  const outputPointers = outputs.map((output) =>
    allocate(blockSize * scalarSize(output.scalar)),
  );
  let view = new DataView(memory.buffer);
  outputPointers.forEach((pointer, index) =>
    view.setUint32(outputTable + index * 4, pointer, true),
  );

  const buffers = metadata.metadata.buffers;
  const bufferPointers = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferFrames = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferChannels = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferSampleRates = buffers.length
    ? allocate(buffers.length * 4, 4)
    : 0;
  const bufferDataPointers = buffers.map((buffer) =>
    allocate(
      blockSize
        * benchmarkBufferChannelCount(buffer)
        * scalarSize(buffer.scalar),
      scalarSize(buffer.scalar),
    ),
  );
  writeParameterDefaults(memory, params, metadata.metadata.params);
  view = new DataView(memory.buffer);
  bufferDataPointers.forEach((pointer, index) => {
    view.setUint32(bufferPointers + index * 4, pointer, true);
    view.setInt32(bufferFrames + index * 4, blockSize, true);
    view.setInt32(
      bufferChannels + index * 4,
      benchmarkBufferChannelCount(buffers[index]),
      true,
    );
    view.setFloat32(bufferSampleRates + index * 4, sampleRate, true);
  });
  requireExecutionSuccess(
    onda_processor_init(
      params,
      state,
      1,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
      0,
    ),
    "processor init",
  );
  const process = () => requireExecutionSuccess(
    onda_process(
      state,
      params,
      0,
      outputTable,
      0,
      blockSize,
      3,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
    ),
    "processor process",
  );

  process();
  const firstOutputs = outputPointers.map((pointer, index) =>
    readScalars(
      memory,
      pointer,
      outputs[index].scalar,
      blockSize,
    ).map(Number),
  );
  return {
    firstOutputs,
    calibrate(minimumIterations, minimumMilliseconds) {
      for (let block = 0; block < 200; block += 1) process();
      let calibratedIterations = minimumIterations;
      while (true) {
        const started = performance.now();
        for (let block = 0; block < calibratedIterations; block += 1) {
          process();
        }
        if (performance.now() - started >= minimumMilliseconds) {
          return calibratedIterations;
        }
        calibratedIterations *= 2;
        if (!Number.isSafeInteger(calibratedIterations)) {
          throw new Error("WebAssembly benchmark calibration overflow");
        }
      }
    },
    measure(timedIterations) {
      const samples = [];
      let outputSink = 0;
      for (let repetition = 0; repetition < repetitions; repetition += 1) {
        const started = performance.now();
        for (let block = 0; block < timedIterations; block += 1) process();
        const elapsedNs = (performance.now() - started) * 1_000_000;
        samples.push(elapsedNs / (timedIterations * blockSize));
        view = new DataView(memory.buffer);
        outputSink += view.getFloat32(
          outputPointers[0] + (blockSize - 1) * 4,
          true,
        );
      }
      if (!Number.isFinite(outputSink)) {
        throw new Error("non-finite WebAssembly timing output");
      }
      return summarize(samples);
    },
  };
}

function writeFirstBlockFixture(path, channels) {
  const samples = channels.flat();
  const bytes = Buffer.allocUnsafe(samples.length * 4);
  samples.forEach((sample, index) => {
    if (!Number.isFinite(sample)) {
      throw new Error(
        `cannot benchmark a non-finite first-block sample at flattened index ${index}`,
      );
    }
    bytes.writeFloatLE(sample, index * 4);
  });
  writeFileSync(path, bytes);
}

function requireExecutionSuccess(status, operation) {
  if (status !== 0) {
    throw new Error(`${operation} failed with execution status ${status}`);
  }
}

function flattenPorts(ports) {
  return ports.flatMap((port) =>
    Array.from({ length: port.array_len }, () => ({ scalar: port.scalar })),
  );
}

function writeParameterDefaults(memory, paramsPointer, params) {
  for (const param of params) {
    const values = (param.default_reprs ?? []).map((value) => ({
      type: param.scalar,
      value: JSON.parse(value),
    }));
    const elementSize = scalarSize(param.scalar);
    for (const [index, value] of values.entries()) {
      writeScalar(
        memory,
        paramsPointer + param.byte_offset + index * elementSize,
        value.type,
        value.value,
      );
    }
  }
}

function flattenConstants(value) {
  if (value.kind === "scalar") return [value.data];
  if (value.kind === "aggregate") return value.data.flatMap(flattenConstants);
  throw new Error(`unsupported MIR constant kind '${String(value.kind)}'`);
}

function writeScalar(memory, pointer, scalar, value) {
  const view = new DataView(memory.buffer);
  switch (scalar) {
    case "bool": view.setUint8(pointer, value ? 1 : 0); break;
    case "i32": view.setInt32(pointer, value, true); break;
    case "i64": view.setBigInt64(pointer, BigInt(value), true); break;
    case "f32": view.setFloat32(pointer, decodeFloat(value, 32), true); break;
    case "f64": view.setFloat64(pointer, decodeFloat(value, 64), true); break;
    default: throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function decodeFloat(value, width) {
  if (typeof value === "number") return value;
  const digits = value.startsWith("0x") ? value.slice(2) : "";
  const bytes = new ArrayBuffer(width / 8);
  const view = new DataView(bytes);
  if (width === 32) {
    view.setUint32(0, Number.parseInt(digits, 16), false);
    return view.getFloat32(0, false);
  }
  view.setBigUint64(0, BigInt(`0x${digits}`), false);
  return view.getFloat64(0, false);
}

function readScalars(memory, pointer, scalar, length) {
  switch (scalar) {
    case "bool":
      return [...new Uint8Array(memory.buffer, pointer, length)].map(Boolean);
    case "i32":
      return [...new Int32Array(memory.buffer, pointer, length)];
    case "i64":
      return [...new BigInt64Array(memory.buffer, pointer, length)];
    case "f32":
      return [...new Float32Array(memory.buffer, pointer, length)];
    case "f64":
      return [...new Float64Array(memory.buffer, pointer, length)];
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function scalarSize(scalar) {
  if (scalar === "bool") return 1;
  if (scalar === "i32" || scalar === "f32") return 4;
  if (scalar === "i64" || scalar === "f64") return 8;
  throw new Error(`unsupported scalar type '${String(scalar)}'`);
}

function benchmarkBufferChannelCount(buffer) {
  if (buffer.channels === "mono") return 1;
  if (buffer.channels === "dynamic") return 2;
  if (buffer.channels === "static") return Math.max(buffer.static_channels, 1);
  throw new Error(
    `unsupported buffer channel shape '${String(buffer.channels)}'`,
  );
}

function parseLastJsonLine(output) {
  const line = output.trim().split("\n").findLast((entry) =>
    entry.trim().startsWith("{")
  );
  if (!line) throw new Error(`native benchmark returned no JSON: ${output}`);
  return JSON.parse(line);
}

function execCaptured(command, args, options) {
  try {
    return execFileSync(command, args, options);
  } catch (error) {
    // The managed command proxy may report EPERM after a successful child
    // exit while still providing the complete stdout and status 0.
    if (error?.status === 0 && error.stdout) {
      return String(error.stdout);
    }
    throw error;
  }
}

function summarize(values) {
  if (!values.length) {
    throw new Error("cannot summarize an empty benchmark sample set");
  }
  const sorted = [...values].sort((lhs, rhs) => lhs - rhs);
  const middle = sorted.length / 2;
  const median = Number.isInteger(middle)
    ? (sorted[middle - 1] + sorted[middle]) * 0.5
    : sorted[Math.floor(middle)];
  const deviations = sorted
    .map((value) => Math.abs(value - median))
    .sort((lhs, rhs) => lhs - rhs);
  const deviationMiddle = deviations.length / 2;
  const mad = Number.isInteger(deviationMiddle)
    ? (deviations[deviationMiddle - 1] + deviations[deviationMiddle]) * 0.5
    : deviations[Math.floor(deviationMiddle)];
  return {
    median,
    mad,
    minimum: sorted[0],
    maximum: sorted.at(-1),
  };
}

function assertParitySummary(summary, expectedSamples, expectedOutputs, label) {
  if (summary.outputs !== expectedOutputs) {
    throw new Error(
      `${label} returned ${summary.outputs} outputs, expected ${expectedOutputs}`,
    );
  }
  if (summary.parity_samples !== expectedSamples) {
    throw new Error(
      `${label} compared ${summary.parity_samples} samples, expected ${expectedSamples}`,
    );
  }
  if (
    !Number.isFinite(summary.parity_max_abs_error)
    || summary.parity_max_abs_error < 0
  ) {
    throw new Error(`${label} returned an invalid maximum absolute error`);
  }
}

function validatePositiveInteger(value, label) {
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`${label} must be a positive integer`);
  }
}

function validateIntegerInRange(value, minimum, maximum, label) {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error(
      `${label} must be an integer from ${minimum} through ${maximum}`,
    );
  }
}

function validatePositiveNumber(value, label) {
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${label} must be a positive number`);
  }
}

function parseBooleanEnvironment(value, fallback, label) {
  if (value === undefined) return fallback;
  if (["1", "true", "yes", "on"].includes(value.toLowerCase())) return true;
  if (["0", "false", "no", "off"].includes(value.toLowerCase())) return false;
  throw new Error(
    `${label} must be a boolean, got '${value}'`,
  );
}

function benchmarkHost() {
  const processors = cpus();
  let allowedCpus = "unavailable";
  if (process.platform === "linux") {
    try {
      const status = readFileSync("/proc/self/status", "utf8");
      allowedCpus = status.match(/^Cpus_allowed_list:\s*(.+)$/m)?.[1] ?? allowedCpus;
    } catch {
      // Affinity reporting is diagnostic only.
    }
  }
  return {
    cpuModel: processors[0]?.model?.trim() || "unknown CPU",
    logicalCpuCount: processors.length,
    allowedCpus,
  };
}

function pinBenchmarkProcess() {
  if (
    process.platform !== "linux"
    || process.env.ONDA_BENCH_AFFINITY_PINNED === "1"
    || parseBooleanEnvironment(
      process.env.ONDA_BENCH_DISABLE_AFFINITY,
      false,
      "ONDA_BENCH_DISABLE_AFFINITY",
    )
  ) {
    return;
  }
  const allowed = linuxAllowedCpus();
  const cpu = process.env.ONDA_BENCH_AFFINITY_CPU ?? firstCpuInList(allowed);
  if (cpu === null) {
    process.stderr.write(
      "Could not determine an allowed CPU; continuing without affinity pinning.\n",
    );
    return;
  }
  const child = spawnSync(
    "taskset",
    ["-c", String(cpu), process.execPath, ...process.argv.slice(1)],
    {
      cwd: process.cwd(),
      stdio: "inherit",
      env: {
        ...process.env,
        ONDA_BENCH_AFFINITY_PINNED: "1",
        ONDA_BENCH_AFFINITY_CPU: String(cpu),
      },
    },
  );
  if (child.error?.code === "ENOENT") {
    process.stderr.write(
      "taskset is unavailable; continuing without affinity pinning.\n",
    );
    return;
  }
  if (child.error) throw child.error;
  if (child.signal) {
    process.stderr.write(`Pinned benchmark terminated by ${child.signal}.\n`);
    process.exit(1);
  }
  process.exit(child.status ?? 1);
}

function linuxAllowedCpus() {
  try {
    const status = readFileSync("/proc/self/status", "utf8");
    return status.match(/^Cpus_allowed_list:\s*(.+)$/m)?.[1] ?? null;
  } catch {
    return null;
  }
}

function firstCpuInList(list) {
  const first = list?.split(",", 1)[0]?.split("-", 1)[0];
  return /^\d+$/.test(first ?? "") ? first : null;
}

function kib(bytes) {
  return (bytes / 1024).toFixed(1);
}

function fixed(value) {
  return Number(value).toFixed(2);
}

function timing(summary) {
  return `${fixed(summary.median)} ± ${fixed(summary.mad)}`;
}
