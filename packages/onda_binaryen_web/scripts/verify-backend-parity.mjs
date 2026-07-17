import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

import {
  compileTrustedMir as compileMir,
  createDefaultImports,
} from "../src/index.js";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? resolve(repoDir, process.env.CARGO_TARGET_DIR)
  : join(repoDir, "target");
const ondaCli =
  process.env.ONDA_CLI ??
  join(cargoTargetDir, "debug", process.platform === "win32" ? "onda.exe" : "onda");
const temporary = mkdtempSync(join(tmpdir(), "onda-backend-parity-"));
const sampleRate = 48_000;
const blockSize = 4;
const absoluteTolerance = 1e-6;
const relativeTolerance = 1e-6;

const scenarios = [
  {
    name: "params, calls, tuples, and persistent state",
    source: join(packageDir, "test/fixtures/language-slice.onda"),
    blocks: 2,
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
    name: "oversampled dual-oscillator corpus example",
    source: join(
      repoDir,
      "examples/foundations/dual_osc_oversampled_8x.onda",
    ),
    blocks: 3,
  },
  {
    name: "oversampled saturator input interpolation",
    source: join(
      repoDir,
      "examples/processors-and-graphs/saw_filter_saturator.onda",
    ),
    blocks: 3,
  },
  {
    name: "processor-array initialization, indexed dispatch, and block updates",
    source: join(
      repoDir,
      "examples/processors-and-graphs/proc_array_init_harmonics.onda",
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
    name: "integer overflow, masked shifts, NaN comparison, and saturating casts",
    source: join(packageDir, "test/fixtures/numeric-edge-parity.onda"),
    blocks: 2,
  },
];

try {
  let comparedSamples = 0;
  let maximumAbsoluteError = 0;

  for (const [scenarioIndex, scenario] of scenarios.entries()) {
    const mirPath = join(temporary, `scenario-${scenarioIndex}.mir.msgpack`);
    compileSourceToMir(scenario.source, mirPath);

    const native = await renderNativeBlocks(scenario);
    const wasm = await renderWasmBlocks(mirPath, scenario);
    const comparison = compareChannels(
      scenario.name,
      native.channels,
      wasm.channels,
    );
    compareSnapshots(scenario.name, native.snapshots, wasm.snapshots);
    comparedSamples += comparison.samples;
    maximumAbsoluteError = Math.max(
      maximumAbsoluteError,
      comparison.maximumAbsoluteError,
    );
  }

  process.stdout.write(
    `Verified native LLVM/MIR-Binaryen parity: ${scenarios.length} scenarios, ${comparedSamples} samples, max abs error ${maximumAbsoluteError.toExponential(3)} (abs ${absoluteTolerance}, rel ${relativeTolerance})\n`,
  );
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

function compileSourceToMir(source, mirPath) {
  execFileSync(
    "cargo",
    [
      "run",
      "-q",
      "-p",
      "onda_cli",
      "--",
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

async function renderNativeBlocks(scenario) {
  const { source } = scenario;
  const actions = scenarioActions(scenario);
  const daemon = createNativeDaemon();
  let requestId = 0;
  const request = (body) => daemon.request({ id: ++requestId, ...body });
  const renderedBlocks = [];
  const snapshots = [];
  let savedSnapshot = null;
  try {
    await request({
      command: "initialize",
      sample_rate_hz: sampleRate,
      block_frames: blockSize,
      fast_math: false,
    });
    await request({ command: "run_start", path: source });
    for (const action of actions) {
      let response;
      if (action.kind === "render") {
        response = await request({ command: "run_render", path: source });
      } else if (action.kind === "segments") {
        response = await request({
          command: "run_render_segments",
          path: source,
          segments: action.segments,
        });
      } else if (action.kind === "event") {
        await request({
          command: "run_trigger_event",
          path: source,
          name: action.name,
          values: action.values,
        });
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
      const { channels, frames } = response.result;
      if (!Array.isArray(channels) || frames !== blockSize) {
        throw new Error("native LLVM render returned an invalid block shape");
      }
      renderedBlocks.push(channels);
    }
  } finally {
    await daemon.close();
  }
  return { channels: concatenateBlocks(renderedBlocks), snapshots };
}

function createNativeDaemon() {
  const child = spawn(ondaCli, ["daemon", "stdio"], {
    cwd: repoDir,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  return {
    async request(request) {
      child.stdin.write(`${JSON.stringify(request)}\n`);
      const next = await lines.next();
      if (next.done) throw new Error("native daemon closed before responding");
      const response = JSON.parse(next.value);
      if (!response.ok) {
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

async function renderWasmBlocks(mirPath, scenario) {
  const artifact = compileMir(readFileSync(mirPath));
  if (!WebAssembly.validate(artifact.wasm)) {
    throw new Error("Binaryen emitted invalid WebAssembly");
  }
  const { instance } = await WebAssembly.instantiate(
    artifact.wasm,
    createDefaultImports(),
  );
  const { memory, __heap_base, onda_init, onda_process } = instance.exports;
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

  writeParameterDefaults(memory, params, metadata.metadata.params);
  let view = new DataView(memory.buffer);
  inputPointers.forEach((pointer, index) =>
    view.setUint32(inputTable + index * 4, pointer, true),
  );
  outputPointers.forEach((pointer, index) =>
    view.setUint32(outputTable + index * 4, pointer, true),
  );
  bufferDataPointers.forEach((pointer, index) => {
    view.setUint32(bufferPointers + index * 4, pointer, true);
    view.setInt32(bufferFrames + index * 4, blockSize, true);
    view.setInt32(
      bufferChannels + index * 4,
      bufferChannelCount(buffers[index]),
      true,
    );
    view.setFloat32(bufferSampleRates + index * 4, sampleRate, true);
  });

  onda_init(params, state);
  const processSegment = (startFrame, frames, flags) => onda_process(
      inputTable,
      outputTable,
      startFrame,
      frames,
      flags,
      params,
      state,
      bufferPointers,
      bufferFrames,
      bufferChannels,
      bufferSampleRates,
    );
  const renderedBlocks = [];
  const snapshots = [];
  let savedSnapshot = null;
  for (const action of scenarioActions(scenario)) {
    if (action.kind === "event") {
      triggerWasmEvent({
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
      });
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
      onda_init(params, state);
      restoreWasmState(memory, state, metadata, savedSnapshot);
      continue;
    }
    if (action.kind === "render") {
      processSegment(0, blockSize, 3);
    } else if (action.kind === "segments") {
      for (const segment of action.segments) {
        processSegment(segment.start_frame, segment.frames, segment.flags);
      }
    } else {
      throw new Error(`unknown Wasm parity action '${String(action.kind)}'`);
    }
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
  }
  return { channels: concatenateBlocks(renderedBlocks), snapshots };
}

function scenarioActions(scenario) {
  return scenario.actions ?? Array.from(
    { length: scenario.blocks },
    () => ({ kind: "render" }),
  );
}

function triggerWasmEvent(context) {
  const { action, artifact, instance, memory, allocate } = context;
  const event = artifact.metadata.metadata.events.find(
    (candidate) => candidate.name === action.name,
  );
  if (!event) throw new Error(`missing Wasm event '${action.name}'`);
  if (event.has_dynamic_payload) {
    throw new Error("parity event helper only supports fixed scalar payloads");
  }
  if (event.params.length !== action.values.length) {
    throw new Error(`event '${action.name}' payload arity mismatch`);
  }
  const payload = allocate(event.payload_size_bytes, 8);
  for (const [index, param] of event.params.entries()) {
    if (!param.scalar || param.is_array || param.is_slice) {
      throw new Error("parity event helper only supports scalar payloads");
    }
    writeScalar(
      memory,
      payload + param.byte_offset,
      param.scalar,
      action.values[index],
    );
  }
  instance.exports[event.export](
    payload,
    context.params,
    context.state,
    context.bufferPointers,
    context.bufferFrames,
    context.bufferChannels,
    context.bufferSampleRates,
  );
}

function flattenPorts(ports) {
  return ports.flatMap((port) =>
    Array.from({ length: port.channel_count }, () => ({ scalar: port.scalar })),
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
        statePointer + entry.storage_byte_offset,
        entry.byte_size,
      ),
      entry.byte_offset,
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
      statePointer + entry.storage_byte_offset,
      entry.byte_size,
    ).set(snapshot.subarray(entry.byte_offset, entry.byte_offset + entry.byte_size));
  }
}

function writeParameterDefaults(memory, paramsPointer, params) {
  for (const param of params) {
    if (param.default === null) continue;
    const values = flattenConstants(param.default);
    const elementSize = scalarSize(param.scalar);
    if (values.length !== param.array_length) {
      throw new Error(
        `parameter '${param.name}' default has ${values.length} values, expected ${param.array_length}`,
      );
    }
    for (const [index, value] of values.entries()) {
      if (value.type !== param.scalar) {
        throw new Error(
          `parameter '${param.name}' default type '${value.type}' does not match '${param.scalar}'`,
        );
      }
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
    case "bool":
      view.setUint8(pointer, value ? 1 : 0);
      break;
    case "i32":
      view.setInt32(pointer, value, true);
      break;
    case "i64":
      view.setBigInt64(pointer, BigInt(value), true);
      break;
    case "f32":
      view.setFloat32(pointer, decodeFloat(value, 32), true);
      break;
    case "f64":
      view.setFloat64(pointer, decodeFloat(value, 64), true);
      break;
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function decodeFloat(value, width) {
  if (typeof value === "number") return value;
  const digits = value.startsWith("0x") ? value.slice(2) : "";
  if (digits.length !== width / 4) {
    throw new Error(`invalid f${width} bit-pattern scalar '${String(value)}'`);
  }
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
      return [...new BigInt64Array(memory.buffer, pointer, length)].map(Number);
    case "f32":
      return [...new Float32Array(memory.buffer, pointer, length)];
    case "f64":
      return [...new Float64Array(memory.buffer, pointer, length)];
    default:
      throw new Error(`unsupported scalar type '${String(scalar)}'`);
  }
}

function scalarSize(scalar) {
  switch (scalar) {
    case "bool":
      return 1;
    case "i32":
    case "f32":
      return 4;
    case "i64":
    case "f64":
      return 8;
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

function compareChannels(label, nativeChannels, wasmChannels) {
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
  return { samples, maximumAbsoluteError };
}

function compareSnapshots(label, nativeSnapshots, wasmSnapshots) {
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
        throw new Error(
          `${label}: snapshot ${snapshot} byte ${byte} differs (LLVM ${native[byte]}, Wasm ${wasm[byte]})`,
        );
      }
    }
  }
}
