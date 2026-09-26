import {
  PayloadPlan,
  decodeDelegateBatch,
  formatPrintBatch,
  resetExecutionOutput,
  writeDelegateBatch,
  writeEventInput,
  writeExecutionOutput,
  writePrintBatch,
} from "@onda-lang/processor-abi";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

import {
  PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
  compileTrustedMir as compileMir,
  createDefaultImports,
} from "../src/index.js";
import { resolveOndaCli } from "./onda-cli.mjs";
import { scalarSize, writeParameterDefaults } from "./wasm-memory.mjs";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const ondaCli = resolveOndaCli(repoDir);
const temporary = mkdtempSync(join(tmpdir(), "onda-backend-parity-"));
const sampleRate = 48_000;
const blockSize = 4;
const absoluteTolerance = 1e-6;
const relativeTolerance = 1e-6;
const wasmConfigurations = [
  { name: "Binaryen O4", options: {} },
  { name: "Binaryen unoptimized", options: { optimize: false } },
];

const scenarios = [
  {
    name: "borrowed branch aliases and explicit aggregate arguments",
    source: join(packageDir, "test/fixtures/structured-data-regressions.onda"),
    blocks: 3,
  },
  {
    name: "nested aggregate routing through proc events and delegates",
    source: join(packageDir, "test/fixtures/structured-message-routing.onda"),
    expectedDelegateNames: ["configured", "configured"],
    actions: [
      { kind: "event", name: "exercise", values: [] },
      { kind: "render" },
      { kind: "event", name: "exercise", values: [] },
      { kind: "render" },
    ],
  },
  {
    name: "structured host slices, one prefix, normalized fields, and empty slices",
    source: join(packageDir, "test/fixtures/structured-slice-messages.onda"),
    actions: [
      { kind: "event", name: "configure", values: [true, [
        { enabled: true, gain: 4.0, bins: [1, 2], mode: "-9223372036854775808" },
        { enabled: false, gain: 6.0, bins: [3, 4], mode: "9223372036854775807" },
      ], 3] },
      { kind: "render" },
      { kind: "event", name: "configure", values: [false, [], 5] },
      { kind: "render" },
    ],
  },
  {
    name: "structured messages, tuple defaults, proc routing, and host publication",
    source: join(packageDir, "test/fixtures/structured-messages.onda"),
    expectedDelegateNames: ["configured", "configured"],
    actions: [
      { kind: "event", name: "configure", values: [] },
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "event", name: "change", values: [[7, 11]] },
      { kind: "render" },
      { kind: "restore" },
      { kind: "render" },
    ],
  },
  {
    name: "init, event, block, and sample print batches",
    source: join(packageDir, "test/fixtures/execution-output-parity.onda"),
    expectedDelegateNames: Array(8).fill("observed"),
    expectedPrintText:
      "init: -2147483648 -9223372036854775808 true 1 1 0.0\n"
      + "init: -2147483648 -9223372036854775808 true 4 1 1.0\n"
      + "init: -2147483648 -9223372036854775808 true 4 4 2.0\n"
      + "event: 9223372036854775807 false\n"
      + "block: 0\nsample: -0.0 1.25\n"
      + "event: -9223372036854775808 false\n"
      + "block: 4\n",
    actions: [
      { kind: "event", name: "report", values: ["9223372036854775807"] },
      { kind: "render" },
      { kind: "event", name: "report", values: ["-9223372036854775808"] },
      { kind: "render" },
    ],
  },
  {
    name: "init, proc, and task selections across snapshots and dispatch",
    source: join(packageDir, "test/fixtures/retained-scopes.onda"),
    actions: [
      { kind: "segments", segments: [{ start_frame: 0, frames: 2, flags: 1 }] },
      { kind: "snapshot" },
      { kind: "event", name: "change", values: [] },
      { kind: "segments", segments: [{ start_frame: 2, frames: 2, flags: 2 }] },
      { kind: "render" },
      { kind: "restore" },
      { kind: "segments", segments: [{ start_frame: 2, frames: 2, flags: 2 }] },
      { kind: "render" },
    ],
  },
  {
    name: "retained block data, branch selections, snapshots, and events",
    source: join(packageDir, "test/fixtures/retained-data.onda"),
    actions: [
      { kind: "segments", segments: [{ start_frame: 0, frames: 2, flags: 1 }] },
      { kind: "snapshot" },
      { kind: "event", name: "alter", values: [] },
      { kind: "segments", segments: [{ start_frame: 2, frames: 2, flags: 2 }] },
      { kind: "restore" },
      { kind: "segments", segments: [{ start_frame: 2, frames: 2, flags: 2 }] },
      { kind: "event", name: "alter", values: [] },
      { kind: "render" },
    ],
  },
  {
    name: "structured data construction, aliases, replacement, and fixed returns",
    source: join(packageDir, "test/fixtures/structured-data.onda"),
    blocks: 3,
  },
  {
    name: "nested aggregate, fixed-array, and slice mutation semantics",
    source: join(packageDir, "test/fixtures/slice-semantics.onda"),
    actions: [
      { kind: "event", name: "seed", values: [2, [1, 3, 5], 0.5] },
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "render" },
    ],
  },
  {
    name: "params, calls, tuples, and persistent state",
    source: join(packageDir, "test/fixtures/language-slice.onda"),
    blocks: 2,
  },
  {
    name: "loops, endpoint induction, branches, and short-circuit control flow",
    source: join(packageDir, "test/fixtures/control-flow-parity.onda"),
    blocks: 2,
    expectedChannels: [
      Array(8).fill(4),
      Array(8).fill(40),
      Array(8).fill(8),
      Array(8).fill(2),
      Array(8).fill(3),
      Array(8).fill(7),
    ],
  },
  {
    name: "parameter arrays across initialization, events, and snapshots",
    source: join(packageDir, "test/fixtures/param-array-snapshots.onda"),
    actions: [
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "event", name: "capture", values: [] },
      { kind: "render" },
    ],
  },
  {
    name: "external buffer reads, writes, and metadata",
    source: join(packageDir, "test/fixtures/buffer-slice.onda"),
    // The second block observes values written during the first block.
    blocks: 2,
  },
  {
    name: "multichannel buffer frame and whole-slice lengths",
    source: join(packageDir, "test/fixtures/stereo-buffer-len.onda"),
    blocks: 2,
  },
  {
    name: "canonical processor oversampling schedule",
    source: join(packageDir, "test/fixtures/oversampling-parity.onda"),
    blocks: 3,
  },
  {
    name: "canonical top-level oversampling schedule",
    source: join(
      packageDir,
      "test/fixtures/top-level-oversampling-parity.onda",
    ),
    blocks: 3,
  },
  {
    name: "host event dispatch and scalar payload layout",
    source: join(packageDir, "test/fixtures/event-parity.onda"),
    actions: [
      { kind: "render" },
      {
        kind: "event",
        name: "note_on",
        values: [72, 0.25, true],
      },
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "render" },
      { kind: "restore" },
      { kind: "render" },
    ],
  },
  {
    name: "zero-frame notifications and segmented process scheduling",
    source: join(packageDir, "test/fixtures/language-slice.onda"),
    actions: [
      {
        kind: "segments",
        segments: [
          { start_frame: 0, frames: 0, flags: 1 },
          { start_frame: 0, frames: 2, flags: 0 },
          { start_frame: 2, frames: 2, flags: 2 },
        ],
      },
      { kind: "render" },
    ],
  },
  {
    name: "cooperative task lifecycle, snapshots, and pinned proc init",
    source: join(
      packageDir,
      "test/fixtures/task-lifecycle-parity.onda",
    ),
    actions: [
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "event", name: "soft_init", values: [] },
      { kind: "render" },
      { kind: "restore" },
      { kind: "render" },
      { kind: "event", name: "hard_init", values: [] },
      { kind: "render" },
      { kind: "render" },
    ],
  },
  {
    name: "top-level cooperative task lifecycle, reset, and snapshots",
    source: join(
      packageDir,
      "test/fixtures/top-level-task-lifecycle-parity.onda",
    ),
    actions: [
      { kind: "render" },
      { kind: "snapshot" },
      { kind: "render" },
      { kind: "event", name: "restart", values: [] },
      { kind: "render" },
      { kind: "restore" },
      { kind: "render" },
    ],
  },
  {
    name: "integer overflow, masked shifts, NaN comparison, and saturating casts",
    source: join(packageDir, "test/fixtures/numeric-edge-parity.onda"),
    blocks: 2,
  },
  {
    name: "oracle-checked i32 and i64 operation semantics",
    source: join(packageDir, "test/fixtures/integer-semantics-parity.onda"),
    actions: integerSemanticsActions(),
  },
  {
    name: "stdlib delay reads across ring-index wrap boundaries",
    source: join(packageDir, "test/fixtures/delay-wrap-parity.onda"),
    blocks: 5,
    expectedChannels: [[0, 0, 0, ...Array.from({ length: 17 }, (_, index) => index + 1)]],
  },
  {
    name: "process runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "render" },
      { kind: "event", name: "set_i32_divisor", values: [0] },
      { kind: "render_failure" },
    ],
  },
  {
    name: "i64 process runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "event", name: "set_i64_divisor", values: ["0"] },
      { kind: "render_failure" },
    ],
  },
  {
    name: "i32 division event runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "event_failure", name: "divide_i32_now", values: [0] },
    ],
  },
  {
    name: "i64 division event runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "event_failure", name: "divide_i64_now", values: ["0"] },
    ],
  },
  {
    name: "i32 remainder event runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "event_failure", name: "remainder_i32_now", values: [0] },
    ],
  },
  {
    name: "i64 remainder event runtime safety failures",
    source: join(packageDir, "test/fixtures/runtime-failure-parity.onda"),
    actions: [
      { kind: "event_failure", name: "remainder_i64_now", values: ["0"] },
    ],
  },
  {
    name: "complete strict scalar operation and cast surface",
    source: join(packageDir, "test/fixtures/scalar-semantics-parity.onda"),
    actions: [{ kind: "render" }, { kind: "snapshot" }],
  },
  {
    name: "i32 and i64 range normalization across signed-domain edges",
    source: join(packageDir, "test/fixtures/range-semantics-parity.onda"),
    actions: rangeSemanticsActions(),
  },
  {
    name: "strict float non-finite, signed-zero, and width-rounding behavior",
    source: join(packageDir, "test/fixtures/strict-float-parity.onda"),
    actions: [{ kind: "render" }, { kind: "snapshot" }],
  },
  {
    name: "complete f32/f64 math intrinsic surface",
    source: join(packageDir, "test/fixtures/math-intrinsics-parity.onda"),
    blocks: 3,
    comparison: "approximate",
  },
];

try {
  let exactSamples = 0;
  let approximateSamples = 0;
  let maximumAbsoluteError = 0;

  for (const [scenarioIndex, scenario] of scenarios.entries()) {
    const mirPath = join(temporary, `scenario-${scenarioIndex}.mir.msgpack`);
    compileSourceToMir(scenario.source, mirPath);
    const mir = readFileSync(mirPath);
    const artifacts = wasmConfigurations.map((configuration) => ({
      configuration,
      artifact: compileMir(mir, configuration.options),
    }));
    const native = await renderNativeBlocks(scenario, artifacts[0].artifact.metadata);
    verifyChannelExpectations(`${scenario.name} (LLVM)`, scenario, native);
    verifySnapshotExpectations(
      `${scenario.name} (LLVM)`,
      artifacts[0].artifact.metadata,
      scenario,
      native.snapshots,
    );
    verifyExecutionExpectations(`${scenario.name} (LLVM)`, scenario, native);
    for (const { configuration, artifact } of artifacts) {
      const label = `${scenario.name} (${configuration.name})`;
      const wasm = await renderWasmBlocks(artifact, scenario);
      verifyChannelExpectations(label, scenario, wasm);
      verifySnapshotExpectations(
        label,
        artifact.metadata,
        scenario,
        wasm.snapshots,
      );
      verifyExecutionExpectations(label, scenario, wasm);
      const comparison = compareChannels(
        label,
        native,
        wasm,
        scenario.comparison ?? "exact",
      );
      compareSnapshots(
        label,
        native.snapshots,
        wasm.snapshots,
        artifact.metadata,
      );
      compareExecutionBatches(label, native, wasm);
      if (comparison.mode === "exact") exactSamples += comparison.samples;
      else approximateSamples += comparison.samples;
      maximumAbsoluteError = Math.max(
        maximumAbsoluteError,
        comparison.maximumAbsoluteError,
      );
    }
  }

  process.stdout.write(
    `Verified native LLVM/MIR-Binaryen parity: ${scenarios.length} scenarios x ${wasmConfigurations.length} Binaryen configurations, ${exactSamples} bit-exact samples, ${approximateSamples} approximate samples, max approximate abs error ${maximumAbsoluteError.toExponential(3)} (abs ${absoluteTolerance}, rel ${relativeTolerance})\n`,
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

function compileSourceToMir(source, mirPath) {
  execFileSync(
    ondaCli,
    [
      "compile",
      source,
      "--emit",
      "mir-messagepack",
      "--output",
      mirPath,
      "--sample-rate",
      String(sampleRate),
      "--block-size",
      String(blockSize),
    ],
    { cwd: repoDir, stdio: "inherit" },
  );
}

async function renderNativeBlocks(scenario, metadata) {
  const { source } = scenario;
  const actions = scenarioActions(scenario);
  const daemon = createNativeDaemon();
  let requestId = 0;
  const request = (body, options) =>
    daemon.request({ id: ++requestId, ...body }, options);
  const renderedBlocks = [];
  const renderedBitBlocks = [];
  const snapshots = [];
  const delegateBatches = [];
  const printBatches = [];
  let savedSnapshot = null;
  try {
    await request({
      command: "initialize",
      sample_rate_hz: sampleRate,
      block_frames: blockSize,
      fast_math: false,
    });
    const start = await request({ command: "run_start", path: source });
    delegateBatches.push(emptyDelegateBatch());
    printBatches.push(canonicalPrintBatch(start.result.print, metadata));
    for (const buffer of metadata.metadata.buffers) {
      const channels = bufferChannelCount(buffer);
      const binding = await request({
        command: "run_bind_buffer",
        path: source,
        name: buffer.name,
        samples: Array(blockSize * channels).fill(0),
        channels,
        sample_rate_hz: sampleRate,
      });
      delegateBatches.push(emptyDelegateBatch());
      printBatches.push(canonicalPrintBatch(binding.result.print, metadata));
    }
    for (const action of actions) {
      let response;
      if (action.kind === "render") {
        response = await request({
          command: "run_render",
          path: source,
          include_sample_bits: (scenario.comparison ?? "exact") === "exact",
        });
      } else if (action.kind === "render_failure") {
        response = await request({
          command: "run_render",
          path: source,
        }, { allowFailure: true });
        requireNativeRuntimeSafetyFailure(response, "native LLVM render");
        collectNativeExecutionBatches(
          response,
          metadata,
          delegateBatches,
          printBatches,
        );
        continue;
      } else if (action.kind === "segments") {
        response = await request({
          command: "run_render_segments",
          path: source,
          segments: action.segments,
          include_sample_bits: (scenario.comparison ?? "exact") === "exact",
        });
      } else if (action.kind === "event") {
        response = await request({
          command: "run_trigger_event",
          path: source,
          name: action.name,
          values: action.values,
        });
        collectNativeExecutionBatches(
          response,
          metadata,
          delegateBatches,
          printBatches,
        );
        continue;
      } else if (action.kind === "event_failure") {
        response = await request({
          command: "run_trigger_event",
          path: source,
          name: action.name,
          values: action.values,
        }, { allowFailure: true });
        requireNativeRuntimeSafetyFailure(
          response,
          `native LLVM event '${action.name}'`,
        );
        collectNativeExecutionBatches(
          response,
          metadata,
          delegateBatches,
          printBatches,
        );
        continue;
      } else if (action.kind === "snapshot") {
        response = await request({ command: "run_snapshot", path: source });
        savedSnapshot = response.result.bytes;
        snapshots.push(Uint8Array.from(savedSnapshot));
        continue;
      } else if (action.kind === "restore") {
        if (savedSnapshot === null) {
          throw new Error("native parity restore has no preceding snapshot");
        }
        await request({
          command: "run_restore",
          path: source,
          bytes: savedSnapshot,
        });
        continue;
      } else {
        throw new Error(`unknown native parity action '${String(action.kind)}'`);
      }
      collectNativeExecutionBatches(
        response,
        metadata,
        delegateBatches,
        printBatches,
      );
      const { channels, channel_bits: channelBits, frames } = response.result;
      const exact = (scenario.comparison ?? "exact") === "exact";
      if (
        !Array.isArray(channels) ||
        frames !== blockSize ||
        (exact && !Array.isArray(channelBits))
      ) {
        throw new Error("native LLVM render returned an invalid block shape");
      }
      renderedBlocks.push(channels);
      if (exact) renderedBitBlocks.push(channelBits);
    }
  } finally {
    await daemon.close();
  }
  return {
    channels: concatenateBlocks(renderedBlocks),
    channelBits: concatenateBlocks(renderedBitBlocks),
    snapshots,
    delegateBatches,
    printBatches,
  };
}

function collectNativeExecutionBatches(response, metadata, delegates, prints) {
  delegates.push(canonicalDelegateBatch({
    occurrences: response.result.delegate_occurrences ?? [],
    overflowCount: response.result.delegate_overflow_count ?? 0,
  }));
  prints.push(canonicalPrintBatch(response.result.print, metadata));
}

function createNativeDaemon() {
  const child = spawn(ondaCli, ["daemon", "stdio"], {
    cwd: repoDir,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  return {
    async request(request, { allowFailure = false } = {}) {
      child.stdin.write(`${JSON.stringify(request)}\n`);
      const next = await lines.next();
      if (next.done) throw new Error("native daemon closed before responding");
      const response = JSON.parse(next.value);
      if (!response.ok && !allowFailure) {
        throw new Error(
          `native LLVM request ${response.id ?? "?"} failed: ${response.error ?? "unknown error"}`,
        );
      }
      return response;
    },
    async close() {
      child.stdin.end();
      const [code, signal] = await once(child, "close");
      if (code !== 0) {
        throw new Error(
          `native daemon exited with ${signal ? `signal ${signal}` : `status ${code}`}`,
        );
      }
    },
  };
}

async function renderWasmBlocks(artifact, scenario) {
  if (!WebAssembly.validate(artifact.wasm)) {
    throw new Error("Binaryen emitted invalid WebAssembly");
  }
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
  const inputChannels = flattenPorts(metadata.metadata.inputs);
  const outputChannels = flattenPorts(metadata.metadata.outputs);
  const inputTable = inputChannels.length
    ? allocate(inputChannels.length * 4, 4)
    : 0;
  const outputTable = outputChannels.length
    ? allocate(outputChannels.length * 4, 4)
    : 0;
  const inputPointers = inputChannels.map((channel) =>
    allocate(blockSize * scalarSize(channel.scalar)),
  );
  const outputPointers = outputChannels.map((channel) =>
    allocate(blockSize * scalarSize(channel.scalar)),
  );

  const buffers = metadata.metadata.buffers;
  const bufferPointers = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferFrames = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferChannels = buffers.length ? allocate(buffers.length * 4, 4) : 0;
  const bufferSampleRates = buffers.length
    ? allocate(buffers.length * 4, 4)
    : 0;
  const bufferDataPointers = buffers.map((buffer) => {
    if (buffer.scalar !== "f32") {
      throw new Error(
        `native parity runner only supports f32 external buffers, got '${buffer.scalar}'`,
      );
    }
    return allocate(blockSize * bufferChannelCount(buffer) * 4);
  });
  const delegateBatch = allocate(20, 4);
  const delegateStorage = allocate(64 * 1024, 8);
  const printBatch = allocate(20, 4);
  const printStorage = allocate(64 * 1024, 8);
  const executionOutput = allocate(12, 4);
  const initExecutionOutput = allocate(12, 4);

  writeDelegateBatch(memory, delegateBatch, delegateStorage, 64 * 1024);
  writePrintBatch(memory, printBatch, printStorage, 64 * 1024);
  writeExecutionOutput(memory, executionOutput, delegateBatch, printBatch);
  writeExecutionOutput(memory, initExecutionOutput, 0, printBatch);

  writeParameterDefaults(memory, params, metadata.metadata.params);
  let view = new DataView(memory.buffer);
  inputPointers.forEach((pointer, index) =>
    view.setUint32(inputTable + index * 4, pointer, true),
  );
  outputPointers.forEach((pointer, index) =>
    view.setUint32(outputTable + index * 4, pointer, true),
  );
  bufferDataPointers.forEach((_, index) => {
    view.setUint32(bufferPointers + index * 4, 0, true);
    view.setInt32(bufferFrames + index * 4, 1, true);
    view.setInt32(
      bufferChannels + index * 4,
      bufferChannelCount(buffers[index]),
      true,
    );
    view.setFloat32(bufferSampleRates + index * 4, sampleRate, true);
  });

  const delegateBatches = [];
  const printBatches = [];
  const initialize = () => {
    resetExecutionOutput(memory, initExecutionOutput);
    requireExecutionSuccess(
      onda_processor_init(
        params,
        state,
        1,
        bufferPointers,
        bufferFrames,
        bufferChannels,
        bufferSampleRates,
        initExecutionOutput,
      ),
      "processor init",
    );
    delegateBatches.push(emptyDelegateBatch());
    printBatches.push(canonicalPrintBatch(
      formatPrintBatch(memory, printBatch, metadata),
      metadata,
    ));
  };
  initialize();
  bufferDataPointers.forEach((pointer, index) => {
    view.setUint32(bufferPointers + index * 4, pointer, true);
    view.setInt32(bufferFrames + index * 4, blockSize, true);
    initialize();
  });
  resetExecutionOutput(memory, executionOutput);
  const processSegmentStatus = (startFrame, frames, flags) =>
    onda_process(
      state,
      params,
      inputTable,
      outputTable,
      startFrame,
      frames,
      flags,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
      executionOutput,
    );
  const processSegment = (startFrame, frames, flags) =>
    requireExecutionSuccess(
      processSegmentStatus(startFrame, frames, flags),
      "processor process",
    );
  const renderedBlocks = [];
  const renderedBitBlocks = [];
  const snapshots = [];
  let savedSnapshot = null;
  for (const action of scenarioActions(scenario)) {
    resetExecutionOutput(memory, executionOutput);
    if (action.kind === "event" || action.kind === "event_failure") {
      const status = triggerWasmEvent({
        action,
        artifact,
        instance,
        memory,
        allocate,
        params,
        state,
        bufferPointers,
        bufferFrames,
        bufferChannels,
        bufferSampleRates,
        executionOutput,
      });
      if (action.kind === "event_failure") {
        requireRuntimeSafetyFailure(status, `Wasm event '${action.name}'`);
      } else {
        requireExecutionSuccess(status, `processor event '${action.name}'`);
      }
      collectWasmExecutionBatches(
        memory,
        metadata,
        delegateBatch,
        printBatch,
        delegateBatches,
        printBatches,
      );
      continue;
    }
    if (action.kind === "snapshot") {
      savedSnapshot = snapshotWasmState(memory, state, metadata);
      snapshots.push(savedSnapshot);
      continue;
    }
    if (action.kind === "restore") {
      if (savedSnapshot === null) {
        throw new Error("Wasm parity restore has no preceding snapshot");
      }
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
        "processor restore init",
      );
      restoreWasmState(memory, state, metadata, savedSnapshot);
      continue;
    }
    // Native render requests provide fresh zeroed output buffers. Match that
    // setup when a segmented request writes only part of the buffer.
    for (const [index, pointer] of outputPointers.entries()) {
      const bytes = blockSize * scalarSize(outputChannels[index].scalar);
      new Uint8Array(memory.buffer, pointer, bytes).fill(0);
    }
    if (action.kind === "render") {
      processSegment(0, blockSize, 3);
    } else if (action.kind === "render_failure") {
      const status = processSegmentStatus(0, blockSize, 3);
      requireRuntimeSafetyFailure(status, "Wasm render");
      collectWasmExecutionBatches(
        memory,
        metadata,
        delegateBatch,
        printBatch,
        delegateBatches,
        printBatches,
      );
      continue;
    } else if (action.kind === "segments") {
      for (const segment of action.segments) {
        processSegment(segment.start_frame, segment.frames, segment.flags);
      }
    } else {
      throw new Error(`unknown Wasm parity action '${String(action.kind)}'`);
    }
    collectWasmExecutionBatches(
      memory,
      metadata,
      delegateBatch,
      printBatch,
      delegateBatches,
      printBatches,
    );
    renderedBlocks.push(
      outputPointers.map((pointer, index) =>
        readScalars(
          memory,
          pointer,
          outputChannels[index].scalar,
          blockSize,
        ),
      ),
    );
    if ((scenario.comparison ?? "exact") === "exact") {
      renderedBitBlocks.push(
        outputPointers.map((pointer, index) => {
          const scalar = outputChannels[index].scalar;
          if (scalar !== "f32") {
            throw new Error(
              `bit-exact parity requires f32 outputs, got '${scalar}'`,
            );
          }
          return [...new Uint32Array(memory.buffer, pointer, blockSize)];
        }),
      );
    }
  }
  return {
    channels: concatenateBlocks(renderedBlocks),
    channelBits: concatenateBlocks(renderedBitBlocks),
    snapshots,
    delegateBatches,
    printBatches,
  };
}

function collectWasmExecutionBatches(
  memory,
  metadata,
  delegateBatch,
  printBatch,
  delegates,
  prints,
) {
  delegates.push(canonicalDelegateBatch(
    decodeDelegateBatch(memory, delegateBatch, metadata),
  ));
  prints.push(canonicalPrintBatch(
    formatPrintBatch(memory, printBatch, metadata),
    metadata,
  ));
}

function scenarioActions(scenario) {
  return scenario.actions ?? Array.from(
    { length: scenario.blocks },
    () => ({ kind: "render" }),
  );
}

function integerSemanticsActions() {
  const cases = {
    i32: [
      [-2_147_483_648, -1, 0],
      [2_147_483_647, 3, 31],
      [-17, 5, 35],
      [1_431_655_765, -1_431_655_766, -1],
      [-1, -2_147_483_648, 32],
    ].concat(integerStressCases(32, 24)),
    i64: [
      ["-9223372036854775808", "-1", "0"],
      ["9223372036854775807", "3", "63"],
      ["-17", "5", "67"],
      ["6148914691236517205", "-6148914691236517206", "-1"],
      ["-1", "-9223372036854775808", "64"],
    ].concat(integerStressCases(64, 24)),
  };
  return Object.entries(cases).flatMap(([scalar, entries]) =>
    entries.flatMap(([lhs, rhs, shift]) => [
      { kind: "event", name: `exercise_${scalar}`, values: [lhs, rhs, shift] },
      {
        kind: "snapshot",
        expected: integerOperationExpectations(
          scalar,
          BigInt(lhs),
          BigInt(rhs),
          BigInt(shift),
        ),
      },
    ])
  ).concat({ kind: "render" });
}

function integerStressCases(bits, count) {
  const values = deterministicSignedIntegers(bits, count * 3);
  return Array.from({ length: count }, (_, index) => {
    const lhs = values[index * 3];
    const rhs = values[index * 3 + 1] || 1n;
    const shift = values[index * 3 + 2];
    return bits === 64
      ? [lhs.toString(), rhs.toString(), shift.toString()]
      : [Number(lhs), Number(rhs), Number(shift)];
  });
}

function deterministicSignedIntegers(bits, count) {
  let state = 0x9e37_79b9_7f4a_7c15n;
  return Array.from({ length: count }, () => {
    state ^= state << 13n;
    state ^= state >> 7n;
    state ^= state << 17n;
    state = BigInt.asUintN(64, state);
    return BigInt.asIntN(bits, state);
  });
}

function integerOperationExpectations(scalar, lhs, rhs, shift) {
  const bits = scalar === "i64" ? 64 : 32;
  const wrap = (value) => BigInt.asIntN(bits, value);
  const shiftCount = BigInt.asUintN(bits, shift) & BigInt(bits - 1);
  const prefix = `${scalar}_`;
  return {
    [`${prefix}negate`]: wrap(-lhs),
    [`${prefix}bit_not`]: wrap(~lhs),
    [`${prefix}add`]: wrap(lhs + rhs),
    [`${prefix}subtract`]: wrap(lhs - rhs),
    [`${prefix}multiply`]: wrap(lhs * rhs),
    [`${prefix}divide`]: wrap(lhs / rhs),
    [`${prefix}remainder`]: wrap(lhs % rhs),
    [`${prefix}bit_and`]: wrap(lhs & rhs),
    [`${prefix}bit_or`]: wrap(lhs | rhs),
    [`${prefix}bit_xor`]: wrap(lhs ^ rhs),
    [`${prefix}shift_left`]: wrap(lhs << shiftCount),
    [`${prefix}shift_right`]: wrap(lhs >> shiftCount),
    [`${prefix}abs`]: lhs < 0n ? wrap(-lhs) : lhs,
    [`${prefix}minimum`]: lhs < rhs ? lhs : rhs,
    [`${prefix}maximum`]: lhs > rhs ? lhs : rhs,
    [`${prefix}equal`]: lhs === rhs ? 1n : 0n,
    [`${prefix}not_equal`]: lhs !== rhs ? 1n : 0n,
    [`${prefix}less`]: lhs < rhs ? 1n : 0n,
    [`${prefix}less_equal`]: lhs <= rhs ? 1n : 0n,
    [`${prefix}greater`]: lhs > rhs ? 1n : 0n,
    [`${prefix}greater_equal`]: lhs >= rhs ? 1n : 0n,
    [`${prefix}${scalar === "i64" ? "cast_narrow" : "cast_wide"}`]:
      scalar === "i64" ? BigInt.asIntN(32, lhs) : lhs,
  };
}

function rangeSemanticsActions() {
  const cases = {
    i32: {
      values: [
        -2_147_483_648,
        -2_147_483_647,
        -2_000_000_001,
        -13_920,
        -9,
        -8,
        -7,
        -4,
        -3,
        -1,
        0,
        3,
        4,
        42,
        99,
        100,
        107,
        108,
        95_999,
        96_000,
        2_000_000_001,
        2_147_483_646,
        2_147_483_647,
      ].concat(rangeStressValues(32, 24)),
      bounds: {
        non_power: [0n, 95_999n, "wrap"],
        signed: [-3n, 3n, "wrap"],
        power_two: [100n, 107n, "wrap"],
        negative_power_two: [-8n, -1n, "wrap"],
        cross_zero_power_two: [-4n, 3n, "wrap"],
        singleton: [42n, 42n, "wrap"],
        singleton_min: [-2_147_483_648n, -2_147_483_648n, "wrap"],
        singleton_max: [2_147_483_647n, 2_147_483_647n, "wrap"],
        min_window: [-2_147_483_648n, -2_147_483_642n, "wrap"],
        max_window: [2_147_483_641n, 2_147_483_647n, "wrap"],
        large_non_power: [-2_000_000_000n, 2_000_000_000n, "wrap"],
        without_max: [-2_147_483_648n, 2_147_483_646n, "wrap"],
        without_min: [-2_147_483_647n, 2_147_483_647n, "wrap"],
        full: [-2_147_483_648n, 2_147_483_647n, "wrap"],
        clamped: [-100n, 100n, "clamp"],
      },
    },
    i64: {
      values: [
        "-9223372036854775808",
        "-9223372036854775807",
        "-5000000000000000001",
        "-13920",
        "-9",
        "-8",
        "-7",
        "-4",
        "-3",
        "-1",
        "0",
        "3",
        "4",
        "42",
        "99",
        "100",
        "107",
        "108",
        "95999",
        "96000",
        "5000000000000000001",
        "9223372036854775806",
        "9223372036854775807",
      ].concat(rangeStressValues(64, 24)),
      bounds: {
        non_power: [0n, 95_999n, "wrap"],
        signed: [-3n, 3n, "wrap"],
        power_two: [100n, 107n, "wrap"],
        negative_power_two: [-8n, -1n, "wrap"],
        cross_zero_power_two: [-4n, 3n, "wrap"],
        singleton: [42n, 42n, "wrap"],
        singleton_min: [
          -9_223_372_036_854_775_808n,
          -9_223_372_036_854_775_808n,
          "wrap",
        ],
        singleton_max: [
          9_223_372_036_854_775_807n,
          9_223_372_036_854_775_807n,
          "wrap",
        ],
        min_window: [
          -9_223_372_036_854_775_808n,
          -9_223_372_036_854_775_802n,
          "wrap",
        ],
        max_window: [
          9_223_372_036_854_775_801n,
          9_223_372_036_854_775_807n,
          "wrap",
        ],
        large_non_power: [
          -5_000_000_000_000_000_000n,
          5_000_000_000_000_000_000n,
          "wrap",
        ],
        without_max: [
          -9_223_372_036_854_775_808n,
          9_223_372_036_854_775_806n,
          "wrap",
        ],
        without_min: [
          -9_223_372_036_854_775_807n,
          9_223_372_036_854_775_807n,
          "wrap",
        ],
        full: [
          -9_223_372_036_854_775_808n,
          9_223_372_036_854_775_807n,
          "wrap",
        ],
        clamped: [-100n, 100n, "clamp"],
      },
    },
  };
  return Object.entries(cases).flatMap(([scalar, specification]) =>
    specification.values.flatMap((value) => {
      const integer = BigInt(value);
      const expected = Object.fromEntries(
        Object.entries(specification.bounds).map(
          ([name, [lower, upper, mode]]) => [
            `${scalar}_${name}`,
            mode === "clamp"
              ? (integer < lower ? lower : integer > upper ? upper : integer)
              : wrapInteger(integer, lower, upper),
          ],
        ),
      );
      return [
        { kind: "event", name: `set_${scalar}`, values: [value] },
        { kind: "snapshot", expected },
      ];
    }),
  ).concat({ kind: "render" });
}

function rangeStressValues(bits, count) {
  const values = deterministicSignedIntegers(bits, count);
  return bits === 64 ? values.map(String) : values.map(Number);
}

function wrapInteger(value, lower, upper) {
  const width = upper - lower + 1n;
  return lower + ((value - lower) % width + width) % width;
}

function triggerWasmEvent(context) {
  const { action, artifact, instance, memory, allocate } = context;
  const event = artifact.metadata.metadata.events.find(
    (candidate) => candidate.name === action.name,
  );
  if (!event) throw new Error(`missing Wasm event '${action.name}'`);
  const plan = new PayloadPlan(event.schema);
  const bytes = plan.encode(action.values);
  const payload = allocate(bytes.length, 8);
  new Uint8Array(memory.buffer, payload, bytes.length).set(bytes);
  const workspaceBytes = plan.requiredWorkspace(bytes);
  const workspace = allocate(workspaceBytes, 8);
  const descriptor = allocate(16, 4);
  writeEventInput(
    memory,
    descriptor,
    payload,
    bytes.length,
    workspace,
    workspaceBytes,
  );
  return instance.exports[event.export](
    descriptor,
    context.params,
    context.state,
    context.bufferPointers,
    context.bufferFrames,
    context.bufferChannels,
    context.bufferSampleRates,
    context.executionOutput,
  );
}

function requireExecutionSuccess(status, operation) {
  if (status !== 0) {
    throw new Error(`${operation} failed with execution status ${status}`);
  }
}

function requireRuntimeSafetyFailure(status, operation) {
  if (status !== PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE) {
    throw new Error(
      `${operation} returned execution status ${status}, expected runtime safety failure ${PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE}`,
    );
  }
}

function requireNativeRuntimeSafetyFailure(response, operation) {
  if (response.ok) throw new Error(`${operation} unexpectedly succeeded`);
  const expected =
    `runtime safety check (${PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE})`;
  if (!String(response.error).includes(expected)) {
    throw new Error(
      `${operation} failed for an unexpected reason: ${response.error ?? "unknown error"}`,
    );
  }
}

function flattenPorts(ports) {
  return ports.flatMap((port) =>
    Array.from({ length: port.array_len }, () => ({ scalar: port.scalar })),
  );
}

function concatenateBlocks(blocks) {
  const channelCount = blocks[0]?.length ?? 0;
  return Array.from({ length: channelCount }, (_, channel) =>
    blocks.flatMap((block) => block[channel]),
  );
}

function snapshotWasmState(memory, statePointer, metadata) {
  const snapshot = new Uint8Array(metadata.runtime.snapshot_size_bytes);
  for (const entry of metadata.metadata.states) {
    snapshot.set(
      new Uint8Array(
        memory.buffer,
        statePointer + entry.physical_state_byte_offset,
        entry.byte_size,
      ),
      entry.packed_snapshot_byte_offset,
    );
  }
  return snapshot;
}

function restoreWasmState(memory, statePointer, metadata, snapshot) {
  if (snapshot.byteLength !== metadata.runtime.snapshot_size_bytes) {
    throw new Error("Wasm snapshot size does not match MIR metadata");
  }
  for (const entry of metadata.metadata.states) {
    new Uint8Array(
      memory.buffer,
      statePointer + entry.physical_state_byte_offset,
      entry.byte_size,
    ).set(snapshot.subarray(
      entry.packed_snapshot_byte_offset,
      entry.packed_snapshot_byte_offset + entry.byte_size,
    ));
  }
}

function readScalars(memory, pointer, scalar, length) {
  switch (scalar) {
    case "bool":
      return [...new Uint8Array(memory.buffer, pointer, length)].map(Boolean);
    case "i32":
      return [...new Int32Array(memory.buffer, pointer, length)];
    case "i64":
      return [...new BigInt64Array(memory.buffer, pointer, length)].map(Number);
    case "f32":
      return [...new Float32Array(memory.buffer, pointer, length)];
    case "f64":
      return [...new Float64Array(memory.buffer, pointer, length)];
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function bufferChannelCount(buffer) {
  if (buffer.channels === "mono" || buffer.channels === "dynamic") return 1;
  if (buffer.channels === "static") return Math.max(buffer.static_channels, 1);
  throw new Error(
    `unsupported buffer channel shape '${String(buffer.channels)}'`,
  );
}

function emptyDelegateBatch() {
  return { occurrences: [], overflowCount: 0 };
}

function canonicalDelegateBatch(batch) {
  return {
    occurrences: (batch?.occurrences ?? []).map((occurrence) => ({
      sequence: occurrence.sequence,
      index: occurrence.index ?? occurrence.delegateIndex,
      name: occurrence.name,
      values: canonicalValue(occurrence.values),
    })),
    overflowCount: batch?.overflowCount ?? 0,
  };
}

function canonicalPrintBatch(batch, metadata) {
  return {
    text: batch?.text ?? "",
    entries: (batch?.entries ?? []).map((entry) => ({
      sequence: entry.sequence,
      siteIndex: entry.site_index ?? entry.siteIndex,
      label: entry.label,
      source: canonicalPrintSource(entry.source, metadata),
      lexicalOwner: entry.lexical_owner ?? entry.lexicalOwner,
      declaration: entry.declaration,
      values: canonicalValue(entry.values),
    })),
    overflowCount: batch?.overflow_count ?? batch?.overflowCount ?? 0,
  };
}

function canonicalPrintSource(source, metadata) {
  const canonical = { ...source };
  if (Number.isInteger(canonical.file)) {
    canonical.file = metadata.metadata.source_files[canonical.file]?.path ?? null;
  }
  return canonicalValue(canonical);
}

function canonicalValue(value) {
  if (typeof value === "number") {
    if (Number.isNaN(value)) return "NaN";
    if (value === Infinity) return "Infinity";
    if (value === -Infinity) return "-Infinity";
    if (Object.is(value, -0)) return "-0.0";
    return value;
  }
  if (typeof value === "bigint") return value.toString();
  if (Array.isArray(value)) return value.map(canonicalValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonicalValue(nested)]),
    );
  }
  return value;
}

function compareExecutionBatches(label, native, wasm) {
  compareCanonicalValue(
    `${label}: delegate batches`,
    native.delegateBatches,
    wasm.delegateBatches,
  );
  compareCanonicalValue(
    `${label}: print batches`,
    native.printBatches,
    wasm.printBatches,
  );
}

function verifyExecutionExpectations(label, scenario, result) {
  if (scenario.expectedDelegateNames !== undefined) {
    const actual = result.delegateBatches.flatMap((batch) =>
      batch.occurrences.map((occurrence) => occurrence.name)
    );
    compareCanonicalValue(
      `${label}: expected delegate names`,
      scenario.expectedDelegateNames,
      actual,
    );
  }
  if (scenario.expectedPrintText !== undefined) {
    const actual = result.printBatches.map((batch) => batch.text).join("");
    if (actual !== scenario.expectedPrintText) {
      throw new Error(
        `${label}: print text differs\nExpected: ${JSON.stringify(scenario.expectedPrintText)}\nActual: ${JSON.stringify(actual)}`,
      );
    }
  }
}

function compareCanonicalValue(label, expected, actual) {
  const expectedJson = JSON.stringify(expected);
  const actualJson = JSON.stringify(actual);
  if (expectedJson !== actualJson) {
    throw new Error(
      `${label} differ\nLLVM: ${expectedJson}\nWasm: ${actualJson}`,
    );
  }
}

function compareChannels(label, native, wasm, mode) {
  if (mode === "exact") {
    return compareExactChannels(label, native.channelBits, wasm.channelBits);
  }
  if (mode !== "approximate") {
    throw new Error(`${label}: unknown comparison mode '${String(mode)}'`);
  }
  return compareApproximateChannels(label, native.channels, wasm.channels);
}

function verifyChannelExpectations(label, scenario, result) {
  if (scenario.expectedChannels === undefined) return;
  const expected = scenario.expectedChannels;
  if (result.channels.length !== expected.length) {
    throw new Error(
      `${label}: returned ${result.channels.length} channels, expected ${expected.length}`,
    );
  }
  for (let channel = 0; channel < expected.length; channel += 1) {
    if (result.channels[channel].length !== expected[channel].length) {
      throw new Error(
        `${label}: channel ${channel} returned ${result.channels[channel].length} samples, expected ${expected[channel].length}`,
      );
    }
    for (let frame = 0; frame < expected[channel].length; frame += 1) {
      if (!Object.is(result.channels[channel][frame], expected[channel][frame])) {
        throw new Error(
          `${label}: channel ${channel}, frame ${frame} is ${result.channels[channel][frame]}, expected ${expected[channel][frame]}`,
        );
      }
    }
  }
}

function compareExactChannels(label, nativeChannels, wasmChannels) {
  if (nativeChannels.length !== wasmChannels.length) {
    throw new Error(
      `${label}: channel count differs (LLVM ${nativeChannels.length}, Wasm ${wasmChannels.length})`,
    );
  }
  let samples = 0;
  for (let channel = 0; channel < nativeChannels.length; channel += 1) {
    const native = nativeChannels[channel];
    const wasm = wasmChannels[channel];
    if (native.length !== wasm.length) {
      throw new Error(
        `${label}: channel ${channel} length differs (LLVM ${native.length}, Wasm ${wasm.length})`,
      );
    }
    for (let frame = 0; frame < native.length; frame += 1) {
      const expected = native[frame] >>> 0;
      const actual = wasm[frame] >>> 0;
      // MIR specifies NaN behavior, but not a canonical sign or payload.
      if (expected !== actual && !(isF32NaN(expected) && isF32NaN(actual))) {
        throw new Error(
          `${label}: sample bits differ at channel ${channel}, frame ${frame} (LLVM ${hexU32(expected)}, Wasm ${hexU32(actual)})`,
        );
      }
      samples += 1;
    }
  }
  return { mode: "exact", samples, maximumAbsoluteError: 0 };
}

function isF32NaN(bits) {
  return (bits & 0x7f80_0000) === 0x7f80_0000 &&
    (bits & 0x007f_ffff) !== 0;
}

function hexU32(value) {
  return `0x${value.toString(16).padStart(8, "0")}`;
}

function compareApproximateChannels(label, nativeChannels, wasmChannels) {
  if (nativeChannels.length !== wasmChannels.length) {
    throw new Error(
      `${label}: channel count differs (LLVM ${nativeChannels.length}, Wasm ${wasmChannels.length})`,
    );
  }
  let samples = 0;
  let maximumAbsoluteError = 0;
  for (let channel = 0; channel < nativeChannels.length; channel += 1) {
    const native = nativeChannels[channel];
    const wasm = wasmChannels[channel];
    if (native.length !== wasm.length) {
      throw new Error(
        `${label}: channel ${channel} length differs (LLVM ${native.length}, Wasm ${wasm.length})`,
      );
    }
    for (let frame = 0; frame < native.length; frame += 1) {
      const expected = native[frame];
      const actual = wasm[frame];
      if (!Number.isFinite(expected) || !Number.isFinite(actual)) {
        throw new Error(
          `${label}: non-finite sample at channel ${channel}, frame ${frame} (LLVM ${expected}, Wasm ${actual})`,
        );
      }
      const absoluteError = Math.abs(expected - actual);
      const allowedError =
        absoluteTolerance +
        relativeTolerance * Math.max(Math.abs(expected), Math.abs(actual));
      if (absoluteError > allowedError) {
        throw new Error(
          `${label}: sample mismatch at channel ${channel}, frame ${frame}: LLVM ${expected}, Wasm ${actual}, abs error ${absoluteError}, allowed ${allowedError}`,
        );
      }
      samples += 1;
      maximumAbsoluteError = Math.max(maximumAbsoluteError, absoluteError);
    }
  }
  return { mode: "approximate", samples, maximumAbsoluteError };
}

function compareSnapshots(label, nativeSnapshots, wasmSnapshots, metadata) {
  if (nativeSnapshots.length !== wasmSnapshots.length) {
    throw new Error(
      `${label}: snapshot count differs (LLVM ${nativeSnapshots.length}, Wasm ${wasmSnapshots.length})`,
    );
  }
  for (let snapshot = 0; snapshot < nativeSnapshots.length; snapshot += 1) {
    const native = nativeSnapshots[snapshot];
    const wasm = wasmSnapshots[snapshot];
    if (native.byteLength !== wasm.byteLength) {
      throw new Error(
        `${label}: snapshot ${snapshot} size differs (LLVM ${native.byteLength}, Wasm ${wasm.byteLength})`,
      );
    }
    for (let byte = 0; byte < native.byteLength; byte += 1) {
      if (native[byte] !== wasm[byte]) {
        const state = metadata.metadata.states.find((candidate) =>
          byte >= candidate.packed_snapshot_byte_offset
          && byte < candidate.packed_snapshot_byte_offset + candidate.byte_size,
        );
        const owner = state
          ? ` in state '${state.name}' (${state.scalar}, byte ${byte - state.packed_snapshot_byte_offset})`
          : "";
        throw new Error(
          `${label}: snapshot ${snapshot} byte ${byte}${owner} differs (LLVM ${native[byte]}, Wasm ${wasm[byte]})`,
        );
      }
    }
  }
}

function verifySnapshotExpectations(label, metadata, scenario, snapshots) {
  const expected = scenarioActions(scenario)
    .filter((action) => action.kind === "snapshot")
    .map((action) => action.expected ?? null);
  if (expected.length !== snapshots.length) {
    throw new Error(
      `${label}: expected ${expected.length} snapshots, got ${snapshots.length}`,
    );
  }
  const states = new Map(
    metadata.metadata.states.map((state) => [state.name, state]),
  );
  for (let snapshotIndex = 0; snapshotIndex < snapshots.length; snapshotIndex += 1) {
    if (expected[snapshotIndex] === null) continue;
    const view = new DataView(
      snapshots[snapshotIndex].buffer,
      snapshots[snapshotIndex].byteOffset,
      snapshots[snapshotIndex].byteLength,
    );
    for (const [name, expectedValue] of Object.entries(expected[snapshotIndex])) {
      const state = states.get(name);
      if (!state) throw new Error(`${label}: expected state '${name}' is missing`);
      const offset = state.packed_snapshot_byte_offset;
      const actual = state.scalar === "i32"
        ? BigInt(view.getInt32(offset, true))
        : state.scalar === "i64"
          ? view.getBigInt64(offset, true)
          : state.scalar === "bool"
            ? BigInt(view.getUint8(offset))
          : null;
      if (actual === null) {
        throw new Error(
          `${label}: expected integer state '${name}', got '${state.scalar}'`,
        );
      }
      if (actual !== expectedValue) {
        throw new Error(
          `${label}: snapshot ${snapshotIndex} state '${name}' is ${actual}, expected ${expectedValue}`,
        );
      }
    }
  }
}
