import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  decodeDelegateRecords,
  formatPrintBatch,
  readDelegateBatch,
  resetExecutionOutput,
  writeDelegateBatch,
  writeExecutionOutput,
  writePrintBatch,
} from "@onda-lang/processor-abi";

import {
  MIR_SCHEMA_VERSION,
  ONDA_VERSION,
  OndaCompileError,
  OndaCompilerError,
  createCompiler,
  createProcessorArtifactFiles,
} from "../src/index.js";

const SOURCE = `params:
  gain = 0.5 { 0.0, 1.0 }

sample:
  out1 = gain
`;

function pcm16Wav(samples) {
  const dataLength = samples.length * 2;
  const bytes = new Uint8Array(44 + dataLength);
  const view = new DataView(bytes.buffer);
  const text = (offset, value) => {
    for (let index = 0; index < value.length; index += 1) {
      bytes[offset + index] = value.charCodeAt(index);
    }
  };
  text(0, "RIFF");
  view.setUint32(4, 36 + dataLength, true);
  text(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, 48_000, true);
  view.setUint32(28, 96_000, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  text(36, "data");
  view.setUint32(40, dataLength, true);
  samples.forEach((sample, index) => view.setInt16(44 + index * 2, sample, true));
  return bytes;
}

test("retries direct frontend initialization after a failure", async () => {
  await assert.rejects(
    createCompiler({ frontendWasm: new Uint8Array([0]) }),
    /failed to initialize the Onda frontend Wasm/,
  );
  const compiler = await createCompiler();
  const { artifact } = await compiler.compileSource(SOURCE);
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  await compiler.dispose();
  await compiler.dispose();
  await assert.rejects(compiler.compileSource(SOURCE), /compiler was disposed/);
});

test("compiles Onda source to a complete processor artifact", async () => {
  const compiler = await createCompiler();
  const { artifact, sourceFiles } = await compiler.compileSource(SOURCE, {
    sampleRate: 48_000,
    blockSize: 128,
  });

  const manifest = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  );
  assert.equal(ONDA_VERSION, manifest.version);
  assert.equal(Number.isInteger(MIR_SCHEMA_VERSION), true);
  assert.equal(MIR_SCHEMA_VERSION > 0, true);
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.equal(artifact.metadata.mir_schema_version, MIR_SCHEMA_VERSION);
  assert.equal(artifact.metadata.compile.sample_rate, 48_000);
  assert.equal(artifact.metadata.compile.block_size, 128);
  assert.equal(artifact.metadata.artifact_kind, "webassembly_module");
  assert.deepEqual(sourceFiles, []);

  const files = await createProcessorArtifactFiles(artifact, { baseName: "gain" });
  assert.equal(files.wasm.name, "gain.wasm");
  assert.equal(files.metadata.name, "gain.onda.json");
  assert.match(files.metadata.text, /"integrity"/);
});

test("initialization observes bound buffers in top-level and proc init", async () => {
  const compiler = await createCompiler();
  const { artifact } = await compiler.compileSource(`proc Reader:
  buffers:
    source: f32
  init:
    first = source[0]
  sample:
    out1 = first

buffers:
  source: f32
init:
  selected = source[1]
  reader = Reader(source = source)
sample:
  out1 = selected + reader()
`);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const {
    memory,
    __heap_base: heapBase,
    onda_processor_init: initialize,
  } = instance.exports;
  let heap = Number(heapBase.value);
  const allocate = (size, align = 4) => {
    heap = Math.ceil(heap / align) * align;
    const address = heap;
    heap += Math.max(size, 1);
    return address;
  };
  const params = allocate(artifact.metadata.runtime.param_size_bytes);
  const state = allocate(artifact.metadata.runtime.state_size_bytes, 16);
  const samples = allocate(8);
  const bufferPointers = allocate(4);
  const bufferFrames = allocate(4);
  const bufferChannels = allocate(4);
  const bufferSampleRates = allocate(4);
  const view = new DataView(memory.buffer);
  view.setFloat32(samples, 2.0, true);
  view.setFloat32(samples + 4, 5.0, true);
  view.setUint32(bufferPointers, samples, true);
  view.setInt32(bufferFrames, 2, true);
  view.setInt32(bufferChannels, 1, true);
  view.setFloat32(bufferSampleRates, 48_000, true);

  assert.equal(initialize(
    params,
    state,
    1,
    bufferPointers,
    bufferFrames,
    bufferChannels,
    bufferSampleRates,
    0,
  ), 0);

  const stateInfo = artifact.metadata.metadata.states;
  const stateValue = (name) => {
    const entry = stateInfo.find((candidate) => candidate.name === name);
    assert.ok(entry, `missing state metadata for ${name}`);
    return view.getFloat32(
      state + Number(entry.physical_state_byte_offset),
      true,
    );
  };
  assert.equal(stateValue("reader.first"), 2.0);
  assert.equal(stateValue("selected"), 5.0);
});

test("compiles and publishes dynamic delegate payloads end to end", async () => {
  const compiler = await createCompiler();
  const { artifact } = await compiler.compileSource(`delegate report(singleton: i32[1], code: i32, values: f32[], tags: i32[])

event trigger(singleton: i32[1], values: f32[], tags: i32[]):
  report(singleton, 7, values, tags)

sample:
  out1 = 0.0
`);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const {
    memory,
    __heap_base: heapBase,
    onda_processor_init: initialize,
    onda_event_0: trigger,
  } = instance.exports;
  let heap = Number(heapBase.value);
  const allocate = (size) => {
    const address = heap;
    heap = (heap + Math.max(size, 1) + 15) & ~15;
    return address;
  };
  const params = allocate(artifact.metadata.runtime.param_size_bytes);
  const state = allocate(artifact.metadata.runtime.state_size_bytes);
  const payload = allocate(32);
  const batchAddress = allocate(20);
  const storageAddress = allocate(48);
  const executionOutputAddress = allocate(12);
  const view = new DataView(memory.buffer);
  let cursor = payload;
  view.setInt32(cursor, 23, true);
  cursor += 4;
  view.setInt32(cursor, 2, true);
  cursor += 4;
  view.setFloat32(cursor, 1.25, true);
  cursor += 4;
  view.setFloat32(cursor, -2.5, true);
  cursor += 4;
  view.setInt32(cursor, 3, true);
  cursor += 4;
  for (const tag of [11, -4, 99]) {
    view.setInt32(cursor, tag, true);
    cursor += 4;
  }
  writeDelegateBatch(memory, batchAddress, storageAddress, 48);
  writeExecutionOutput(memory, executionOutputAddress, batchAddress, 0);
  assert.equal(initialize(params, state, 1, 0, 0, 0, 0, 0), 0);
  assert.equal(trigger(payload, params, state, 0, 0, 0, 0, executionOutputAddress), 0);
  const batch = readDelegateBatch(memory, batchAddress);
  assert.deepEqual(batch, {
    storageAddress,
    capacityBytes: 48,
    usedBytes: 48,
    recordCount: 1,
    overflowCount: 0,
  });
  const records = decodeDelegateRecords(
    new Uint8Array(memory.buffer, storageAddress, 48),
    batch.usedBytes,
    artifact.metadata.metadata.delegates,
  );
  assert.equal(records[0].name, "report");
  assert.equal(records[0].sequence, 0);
  assert.equal(artifact.metadata.metadata.delegates[0].params[0].is_array, true);
  assert.deepEqual(records[0].values, {
    singleton: [23],
    code: 7,
    values: [1.25, -2.5],
    tags: [11, -4, 99],
  });
  await compiler.dispose();
});

test("compiles and formats authored prints end to end", async () => {
  const compiler = await createCompiler();
  const { artifact } = await compiler.compileSource(`event report(value: i64):
  print("event", value)

init:
  print("boot")

sample:
  out1 = 0.0
`);
  const { instance } = await WebAssembly.instantiate(artifact.wasm);
  const { memory, __heap_base: heapBase, onda_processor_init: initialize, onda_event_0: report } =
    instance.exports;
  let heap = Number(heapBase.value);
  const allocate = (size) => {
    const address = heap;
    heap = (heap + Math.max(size, 1) + 15) & ~15;
    return address;
  };
  const params = allocate(artifact.metadata.runtime.param_size_bytes);
  const state = allocate(artifact.metadata.runtime.state_size_bytes);
  const payload = allocate(8);
  const batch = allocate(20);
  const storage = allocate(128);
  const output = allocate(12);
  writePrintBatch(memory, batch, storage, 128);
  writeExecutionOutput(memory, output, 0, batch);
  resetExecutionOutput(memory, output);
  assert.equal(initialize(params, state, 1, 0, 0, 0, 0, output), 0);
  assert.equal(formatPrintBatch(memory, batch, artifact.metadata).text, "boot\n");
  new DataView(memory.buffer).setBigInt64(payload, 9_007_199_254_740_993n, true);
  resetExecutionOutput(memory, output);
  assert.equal(report(payload, params, state, 0, 0, 0, 0, output), 0);
  assert.equal(
    formatPrintBatch(memory, batch, artifact.metadata).text,
    "event: 9007199254740993\n",
  );
  await compiler.dispose();
});

test("compile constants preserve every public JavaScript value type", async () => {
  const compiler = await createCompiler();
  const source = `config const Enabled: bool = false
config const Size: i32 = 1
config const Wide: i64 = i64(1)
config const Gain: f32 = 0.0
config const Phase: f64 = 0.0
config const Flags: bool[] = [false]
config const Indices: i32[] = [0]
config const WideValues: i64[] = [i64(0)]
config const Coefficients: f32[2] = [0.0, 0.0]
config const Precise: f64[] = [0.0]
config const NegativeZero: f64 = 0.0
config const NegativeZeros32: f32[] = [0.0]
config const NegativeZeros64: f64[] = [0.0]
config const SpecialValues: f64[] = [0.0]
namespace Checks:
  assert(Enabled)
  assert(Size == 8)
  assert(Wide == i64(9007199254740993))
  assert(Gain == f32(0.25))
  assert(Phase == f64(0.125))
  assert(Flags[1])
  assert(Indices[1] == 13)
  assert(WideValues[0] == i64(9007199254740993))
  assert(Coefficients[1] == f32(0.75))
  assert(Precise[0] == f64(0.125))
  assert((f64(1.0) / NegativeZero) < f64(0.0))
  assert((f32(1.0) / NegativeZeros32[0]) < f32(0.0))
  assert((f64(1.0) / NegativeZeros64[0]) < f64(0.0))
sample:
  out1 = 0.0
`;
  const constants = {
    Enabled: true,
    Size: 8,
    Wide: 9_007_199_254_740_993n,
    Gain: 0.25,
    Phase: 0.125,
    Flags: new Uint8Array([0, 1]),
    Indices: new Int32Array([12, 13]),
    WideValues: new BigInt64Array([9_007_199_254_740_993n]),
    Coefficients: new Float32Array([0.5, 0.75]),
    Precise: new Float64Array([0.125]),
    NegativeZero: -0,
    NegativeZeros32: new Float32Array([-0]),
    NegativeZeros64: new Float64Array([-0]),
    SpecialValues: new Float64Array([
      Number.NaN,
      Number.POSITIVE_INFINITY,
      Number.NEGATIVE_INFINITY,
    ]),
  };
  const inspected = await compiler.inspectSourceConstants(source, { constants });
  const inspectedByName = Object.fromEntries(inspected.map((descriptor) => [
    descriptor.name,
    descriptor,
  ]));
  assert.equal(inspectedByName.Enabled.value, true);
  assert.equal(inspectedByName.Size.value, 8);
  assert.equal(inspectedByName.Wide.value, 9_007_199_254_740_993n);
  assert.equal(inspectedByName.Gain.value, 0.25);
  assert.equal(inspectedByName.Flags.value instanceof Uint8Array, true);
  assert.equal(inspectedByName.Indices.value instanceof Int32Array, true);
  assert.equal(inspectedByName.WideValues.value instanceof BigInt64Array, true);
  assert.equal(inspectedByName.Coefficients.value instanceof Float32Array, true);
  assert.equal(inspectedByName.Precise.value instanceof Float64Array, true);
  assert.equal(Number.isNaN(inspectedByName.SpecialValues.value[0]), true);
  assert.equal(inspectedByName.SpecialValues.value[1], Number.POSITIVE_INFINITY);
  assert.equal(inspectedByName.SpecialValues.value[2], Number.NEGATIVE_INFINITY);
  assert.equal(inspectedByName.Coefficients.kind, "fixed-array");
  assert.equal(inspectedByName.Coefficients.elementCount, 2);
  assert.equal(inspectedByName.Flags.kind, "array");
  assert.equal(Object.is(inspectedByName.NegativeZero.value, -0), true);
  assert.equal(Object.is(inspectedByName.NegativeZeros32.value[0], -0), true);
  assert.equal(Object.is(inspectedByName.NegativeZeros64.value[0], -0), true);

  const compiled = await compiler.compileSource(source, { constants });
  assert.equal(WebAssembly.validate(compiled.artifact.wasm), true);

  const workspace = await compiler.compileWorkspace({
    entry: "main.onda",
    sources: { "main.onda": source },
  }, { constants });
  const inspectedWorkspace = await compiler.inspectWorkspaceConstants({
    entry: "main.onda",
    sources: { "main.onda": source },
  }, { constants });
  assert.deepEqual(
    inspectedWorkspace.map(({ name, value }) => ({ name, value })),
    inspected.map(({ name, value }) => ({ name, value })),
  );
  const image = await compiler.createProjectImage(workspace.sourceGraph);
  const inspectedImage = await compiler.inspectProjectImageConstants(image.bytes, { constants });
  assert.deepEqual(
    inspectedImage.map(({ name, value }) => ({ name, value })),
    inspected.map(({ name, value }) => ({ name, value })),
  );
  const replayed = await compiler.compileProjectImage(image.bytes, { constants });
  assert.equal(WebAssembly.validate(replayed.artifact.wasm), true);
  assert.throws(
    () => compiler.frontend.compile_to_mir_messagepack(
      "config const Values: f32[] = []\n",
      48_000,
      128,
      JSON.stringify([{ name: "Values", element: "invalid", array: true, values: [] }]),
    ),
    (error) => String(error).includes("unknown element type 'invalid'"),
  );
  await compiler.dispose();
});

test("compile constant inspection applies partial overrides and retains authored values", async () => {
  const compiler = await createCompiler();
  const descriptors = await compiler.inspectSourceConstants(`
config const TEST: f32 = 0.25
config const YOYO: f32 = 0.75
config const NAN: f32 = 0.0
config const POSITIVE_INFINITY: f64 = 0.0
config const NEGATIVE_INFINITY: f64 = 0.0
sample:
  out1 = TEST + YOYO
`, {
    constants: {
      TEST: 0.5,
      NAN: Number.NaN,
      POSITIVE_INFINITY: Number.POSITIVE_INFINITY,
      NEGATIVE_INFINITY: Number.NEGATIVE_INFINITY,
    },
  });

  assert.deepEqual(
    descriptors.map(({ name, element, kind, elementCount, value }) => ({
      name,
      element,
      kind,
      elementCount,
      value,
    })),
    [
      { name: "TEST", element: "f32", kind: "scalar", elementCount: 1, value: 0.5 },
      { name: "YOYO", element: "f32", kind: "scalar", elementCount: 1, value: 0.75 },
      { name: "NAN", element: "f32", kind: "scalar", elementCount: 1, value: Number.NaN },
      {
        name: "POSITIVE_INFINITY",
        element: "f64",
        kind: "scalar",
        elementCount: 1,
        value: Number.POSITIVE_INFINITY,
      },
      {
        name: "NEGATIVE_INFINITY",
        element: "f64",
        kind: "scalar",
        elementCount: 1,
        value: Number.NEGATIVE_INFINITY,
      },
    ],
  );
  await compiler.dispose();
});

test("compiles fixed buffer arrays with explicit contiguous group metadata", async () => {
  const compiler = await createCompiler();
  const { artifact } = await compiler.compileSource(`buffers:
  bank: f32 {3}
  single: f32 {1}

sample:
  out1 = bank[99][0] + single[0][0]
`);

  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.name),
    ["bank[0]", "bank[1]", "bank[2]", "single[0]"],
  );
  assert.deepEqual(artifact.metadata.metadata.buffer_arrays, [
    { name: "bank", first_buffer: 0, len: 3 },
    { name: "single", first_buffer: 3, len: 1 },
  ]);
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  await compiler.dispose();
});

test("compiles an in-memory project through the same product API", async () => {
  const compiler = await createCompiler();
  const { artifact, sourceFiles, sourceGraph } = await compiler.compileWorkspace({
    entry: "main.onda",
    sources: {
      "main.onda": `include "./level.onda"

buffers:
  clip: buffer<f32>

sample:
  out1 = level()
`,
      "level.onda": `def level() -> f32:
  return 0.25
`,
    },
  }, {
    sampleRate: 48_000,
    blockSize: 256,
  });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.deepEqual(sourceFiles, ["main.onda", "level.onda"]);
  assert.equal(artifact.metadata.compile.block_size, 256);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.name),
    ["clip"],
  );
  assert.equal(sourceGraph.entry, "main.onda");
  assert.match(sourceGraph.stdlibDigest, /^sha256:[0-9a-f]{64}$/);
  assert.deepEqual(
    sourceGraph.documents.map((document) => document.path),
    ["level.onda", "main.onda"],
  );
  assert.deepEqual(sourceGraph.resolutions, [{
    source: "main.onda",
    kind: "include",
    specifier: "./level.onda",
    target: "level.onda",
  }]);
});

test("decodes WAV files through the canonical project decoder", async () => {
  const compiler = await createCompiler();
  const decoded = await compiler.decodeBufferFile(
    pcm16Wav([-32_768, 32_767]),
    "clip.wav",
  );
  assert.equal(decoded.element, "f32");
  assert.equal(decoded.frames, 2);
  assert.equal(decoded.channels, 1);
  assert.equal(decoded.sampleRate, 48_000);
  assert.deepEqual([...decoded.data], [-1, 32_767 / 32_768]);
  await compiler.dispose();
});

test("project images and typed assets round-trip through the public compiler API", async () => {
  const compiler = await createCompiler();
const source = `buffers:
  sequence: buffer<i64>
  sequence_copy: buffer<i64>

sample:
  out1 = 0.0
`;
  const compiled = await compiler.compileWorkspace({
    entry: "main.onda",
    sources: { "main.onda": source },
  });
  const asset = await compiler.encodeBufferAsset({
    element: "i64",
    frames: 2,
    channels: 1,
    sampleRate: 48_000,
    data: new BigInt64Array([-7n, 9223372036854775807n]),
  });
  const decoded = await compiler.decodeBufferAsset(asset);
  assert.equal(decoded.element, "i64");
  assert.deepEqual([...decoded.data], [-7n, 9223372036854775807n]);

  const imageSourceGraph = {
    ...compiled.sourceGraph,
    documents: [
      ...compiled.sourceGraph.documents,
      { path: "scratch.onda", contents: "incomplete work in progress\n" },
    ],
  };
  const image = await compiler.createProjectImage(
    imageSourceGraph,
    new Map([["sequence", asset], ["sequence_copy", asset]]),
  );
  assert.match(image.contentDigest, /^sha256:[0-9a-f]{64}$/);
  assert.equal(image.buffers[0].element, "i64");
  assert.equal(image.buffers[0].assetId, image.buffers[1].assetId);
  assert.equal(image.sourceGraph.stdlibDigest, compiled.sourceGraph.stdlibDigest);

  const replayed = await compiler.compileProjectImage(image.bytes, { blockSize: 256 });
  assert.equal(WebAssembly.validate(replayed.artifact.wasm), true);
  assert.deepEqual(replayed.sourceFiles, ["main.onda"]);
  assert.deepEqual(replayed.sourceGraph, imageSourceGraph);

  const wrongAsset = await compiler.encodeBufferAsset({
    element: "f32",
    frames: 2,
    channels: 1,
    sampleRate: 48_000,
    data: new Float32Array([1, 2]),
  });
  const invalidImage = await compiler.createProjectImage(
    compiled.sourceGraph,
    new Map([["sequence", wrongAsset]]),
  );
  await assert.rejects(
    compiler.compileProjectImage(invalidImage.bytes),
    /requires i64, but its asset contains f32/,
  );

  const materialized = await compiler.materializeProjectImage(
    image.bytes,
    new Map([["sequence", "Original Sequence.wav"]]),
  );
  assert.deepEqual(materialized.directories, ["assets", "code"]);
  assert.deepEqual(
    materialized.files.map((file) => file.path),
    [
      "assets/Original Sequence.ondabuffer",
      "code/main.onda",
      "code/scratch.onda",
      "project.ondaproject",
    ],
  );
  const loadedFiles = await compiler.loadProjectFiles(new Map(
    materialized.files.map((file) => [file.path, file.bytes]),
  ));
  assert.equal(loadedFiles.sourceGraph.entry, "code/main.onda");
  assert.deepEqual(loadedFiles.sourceGraph.documents, [
    { path: "code/main.onda", contents: source },
    { path: "code/scratch.onda", contents: "incomplete work in progress\n" },
  ]);
  const multiProjectFiles = new Map(
    materialized.files.map((file) => [file.path, file.bytes]),
  );
  multiProjectFiles.set(
    "alternate.ondaproject",
    new TextEncoder().encode(JSON.stringify({ entry: "code/main.onda" })),
  );
  await assert.rejects(
    compiler.loadProjectFiles(multiProjectFiles),
    (error) => {
      assert.equal(error instanceof OndaCompilerError, true);
      assert.match(String(error.cause), /more than one .ondaproject file/);
      return true;
    },
  );
  const selectedFiles = await compiler.loadProjectFiles(
    multiProjectFiles,
    "alternate.ondaproject",
  );
  assert.equal(selectedFiles.sourceGraph.entry, "code/main.onda");
  const capabilities = await compiler.projectCapabilities();
  assert.equal(capabilities.imageFormatVersion, image.formatVersion);
  assert.equal(capabilities.stdlibDigest, compiled.sourceGraph.stdlibDigest);
  await compiler.dispose();
});

test("project file builder rejects excess files before retaining them", async () => {
  const compiler = await createCompiler();
  const builder = new compiler.frontend.WebMaterializedProjectBuilder();
  const maxFiles = 4096 + 4096 + 1;
  try {
    for (let index = 0; index < maxFiles; index += 1) {
      builder.add_file(`empty/${index}`, new Uint8Array());
    }
    assert.throws(
      () => builder.add_file("empty/overflow", new Uint8Array()),
      /more than 8193 files/,
    );
  } finally {
    builder.free();
    await compiler.dispose();
  }
});

test("confines in-memory projects inside the browser virtual namespace", async () => {
  const compiler = await createCompiler();
  for (const source of [
    `include "../outside.onda"\n`,
    `include "/tmp/outside.onda"\n`,
  ]) {
    await assert.rejects(
      compiler.compileWorkspace({
        entry: "main.onda",
        sources: { "main.onda": source },
      }),
      (error) => {
        assert.equal(error instanceof OndaCompileError, true);
        assert.match(error.diagnostics[0].message, /escapes project root/);
        assert.deepEqual(error.sourceFiles, ["main.onda"]);
        assert.deepEqual(error.unresolvedSourceFiles, []);
        return true;
      },
    );
  }
});

test("returns structured frontend diagnostics", async () => {
  const compiler = await createCompiler();
  await assert.rejects(
    compiler.compileSource("sample:\n  out1 = missing_name\n"),
    (error) => {
      assert.equal(error instanceof OndaCompileError, true);
      assert.equal(error.diagnostics.length > 0, true);
      assert.equal(typeof error.diagnostics[0].message, "string");
      assert.equal(error.diagnostics[0].stage, "semantic");
      assert.deepEqual(error.sourceFiles, []);
      return true;
    },
  );
});

test("returns contributing project sources with semantic failures", async () => {
  const compiler = await createCompiler();
  await assert.rejects(
    compiler.compileWorkspace({
      entry: "main.onda",
      sources: {
        "main.onda": "import dsp\nsample:\n  out1 = DSP::missing()\n",
        "dsp.onda": "namespace DSP:\n  const value = 1.0\n",
        "unused.onda": "const unused = 1.0\n",
      },
    }),
    (error) => {
      assert.equal(error instanceof OndaCompileError, true);
      assert.equal(error.diagnostics[0].stage, "semantic");
      assert.deepEqual(error.sourceFiles, ["main.onda", "dsp.onda"]);
      assert.deepEqual(error.unresolvedSourceFiles, []);
      return true;
    },
  );
});

test("returns unresolved project source candidates with parse failures", async () => {
  const compiler = await createCompiler();
  await assert.rejects(
    compiler.compileWorkspace({
      entry: "main.onda",
      sources: {
        "main.onda": "import dsp/filter\n",
      },
    }),
    (error) => {
      assert.equal(error instanceof OndaCompileError, true);
      assert.deepEqual(error.sourceFiles, ["main.onda"]);
      assert.deepEqual(
        error.unresolvedSourceFiles,
        ["dsp/filter.onda", "dsp/filter.on"],
      );
      return true;
    },
  );
});

test("runs the Onda LSP protocol inside frontend Wasm", async () => {
  const compiler = await createCompiler();
  await assert.rejects(
    compiler.setLspAnalysisOptions({ constants: { Size: 8 } }),
    /constants are compile-request inputs/,
  );
  const initialized = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { processId: null, capabilities: {} },
  });
  assert.equal(initialized.length, 1);
  assert.equal(initialized[0].id, 1);
  assert.equal(initialized[0].result.serverInfo.name, "onda");
  assert.equal(initialized[0].result.capabilities.hoverProvider, true);

  const diagnostics = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    method: "textDocument/didOpen",
    params: {
      textDocument: {
        uri: "file:///onda-project/main.onda",
        languageId: "onda",
        version: 1,
        text: "sample:\n  out1 = missing_name\n",
      },
    },
  });
  const published = diagnostics.find(
    (message) => message.method === "textDocument/publishDiagnostics",
  );
  assert.equal(published.params.uri, "file:///onda-project/main.onda");
  assert.equal(published.params.diagnostics.length > 0, true);
  assert.equal(published.params.diagnostics[0].source, "onda");

  const completion = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    id: 2,
    method: "textDocument/completion",
    params: {
      textDocument: { uri: "file:///onda-project/main.onda" },
      position: { line: 0, character: 0 },
    },
  });
  assert.equal(completion[0].id, 2);
  assert.equal(Array.isArray(completion[0].result.items), true);
  assert.equal(
    completion[0].result.items.some((item) => item.label === "sample"),
    true,
  );

  await compiler.sendLspMessage({
    jsonrpc: "2.0",
    method: "textDocument/didOpen",
    params: {
      textDocument: {
        uri: "file:///onda-project/stdlib.onda",
        languageId: "onda",
        version: 1,
        text: "import std/osc\n\ninit:\n  osc = std::osc::Saw()\n",
      },
    },
  });
  const definition = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    id: 3,
    method: "textDocument/definition",
    params: {
      textDocument: { uri: "file:///onda-project/stdlib.onda" },
      position: { line: 3, character: 21 },
    },
  });
  assert.match(definition[0].result.uri, /^onda-stdlib:\/\/\/std\/osc\.onda$/);
  const virtualDocument = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    id: 4,
    method: "onda/virtualDocument",
    params: { uri: definition[0].result.uri },
  });
  assert.equal(virtualDocument[0].result.path, "std/osc.onda");
  assert.equal(virtualDocument[0].result.readOnly, true);
  assert.match(virtualDocument[0].result.text, /proc Saw/);

  const stdlibSource = virtualDocument[0].result.text;
  const phasorUse = stdlibSource.lastIndexOf("Phasor");
  const beforePhasor = stdlibSource.slice(0, phasorUse + 1);
  const stdlibDefinition = await compiler.sendLspMessage({
    jsonrpc: "2.0",
    id: 5,
    method: "textDocument/definition",
    params: {
      textDocument: { uri: virtualDocument[0].result.uri },
      position: {
        line: beforePhasor.split("\n").length - 1,
        character: beforePhasor.length - beforePhasor.lastIndexOf("\n") - 1,
      },
    },
  });
  assert.equal(stdlibDefinition[0].result.uri, "onda-stdlib:///std/osc.onda");
  assert.equal(stdlibDefinition[0].result.range.start.line, 1);
});

test("offers an asynchronous browser-worker client", async () => {
  let receivedCompileOptions;
  class FakeWorker {
    constructor(url, options) {
      assert.match(String(url), /worker\.js$/);
      assert.equal(options.type, "module");
      this.listeners = new Map();
      this.terminated = false;
    }

    addEventListener(type, listener) {
      this.listeners.set(type, listener);
    }

    removeEventListener(type) {
      this.listeners.delete(type);
    }

    postMessage(message) {
      if (message.type === "compileSource") {
        receivedCompileOptions = message.options;
      }
      const value = message.type === "compileSource"
        ? {
          artifact: { wasm: new Uint8Array([0, 97, 115, 109]), metadata: {} },
          sourceFiles: [],
        }
        : message.type === "inspectSourceConstants"
          ? [{ name: "Wide", element: "i64", kind: "scalar", elementCount: 1, value: 1n }]
        : message.type === "lspMessage"
          ? [{ jsonrpc: "2.0", id: message.message.id, result: null }]
          : null;
      queueMicrotask(() => {
        this.listeners.get("message")?.({
          data: { type: "result", requestId: message.requestId, value },
        });
      });
    }

    terminate() {
      this.terminated = true;
    }
  }

  const compiler = await createCompiler({ worker: true, Worker: FakeWorker });
  const constants = {
    Wide: 9_007_199_254_740_993n,
    Window: new Float32Array([0.25, 0.75]),
  };
  const { artifact, sourceFiles } = await compiler.compileSource(SOURCE, { constants });
  assert.deepEqual([...artifact.wasm], [0, 97, 115, 109]);
  assert.deepEqual(sourceFiles, []);
  assert.equal(receivedCompileOptions.constants.Wide, constants.Wide);
  assert.deepEqual(receivedCompileOptions.constants.Window, constants.Window);
  assert.deepEqual(
    await compiler.inspectSourceConstants(SOURCE),
    [{ name: "Wide", element: "i64", kind: "scalar", elementCount: 1, value: 1n }],
  );
  assert.deepEqual(
    await compiler.sendLspMessage({ jsonrpc: "2.0", id: 9, method: "shutdown" }),
    [{ jsonrpc: "2.0", id: 9, result: null }],
  );
  await compiler.dispose();
  assert.equal(compiler.worker.terminated, true);
  await compiler.dispose();
  await assert.rejects(compiler.compileSource(SOURCE), /compiler was disposed/);
});

test("terminates a worker whose frontend initialization fails", async () => {
  let worker;
  class FailingWorker {
    constructor() {
      worker = this;
      this.listeners = new Map();
      this.terminated = false;
    }

    addEventListener(type, listener) {
      this.listeners.set(type, listener);
    }

    removeEventListener(type) {
      this.listeners.delete(type);
    }

    postMessage(message) {
      queueMicrotask(() => {
        this.listeners.get("message")?.({
          data: {
            type: "error",
            requestId: message.requestId,
            error: { message: "frontend initialization failed" },
          },
        });
      });
    }

    terminate() {
      this.terminated = true;
    }
  }

  await assert.rejects(
    createCompiler({ worker: true, Worker: FailingWorker }),
    /frontend initialization failed/,
  );
  assert.equal(worker.terminated, true);
  assert.equal(worker.listeners.size, 0);
});
