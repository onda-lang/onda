import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  createProcessorArtifactFiles,
  loadProcessorArtifactFiles,
  validateProcessorArtifact,
  validateProcessorModule,
  validateProcessorMetadata,
} from "../src/index.js";

if (!globalThis.crypto) globalThis.crypto = webcrypto;

test("validates the descriptor fixture shared with the Rust schema", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/processor-descriptor-v1.json", import.meta.url),
    "utf8",
  ));
  assert.equal(
    validateProcessorMetadata(fixture).format_version,
    PROCESSOR_ARTIFACT_FORMAT_VERSION,
  );

  const missingCanonicalField = structuredClone(fixture);
  delete missingCanonicalField.metadata.inputs[0].default_reprs;
  assert.throws(
    () => validateProcessorMetadata(missingCanonicalField),
    /default_reprs must be present/,
  );

  const inconsistentLayout = structuredClone(fixture);
  inconsistentLayout.metadata.inputs[0].byte_size = 8;
  assert.throws(
    () => validateProcessorMetadata(inconsistentLayout),
    /byte_size does not match/,
  );
});

test("rejects an unsupported snapshot format", () => {
  const fixture = metadata();
  fixture.runtime.snapshot_format_version = PROCESSOR_SNAPSHOT_FORMAT_VERSION + 1;
  assert.throws(
    () => validateProcessorMetadata(fixture),
    /unsupported processor snapshot version/,
  );
});

test("rejects runtime semantics not implemented by processor ABI v1", () => {
  for (const [field, value, expected] of [
    ["state_initialization", "host_initialized", "zeroed"],
    ["snapshot_byte_order", "big_endian", "little_endian"],
    ["snapshot_restore_base", "zeroed_state", "post_init_physical_state_image"],
  ]) {
    const fixture = metadata();
    fixture.runtime[field] = value;
    assert.throws(
      () => validateProcessorMetadata(fixture),
      new RegExp(`runtime\\.${field} must be '${expected}'`),
    );
  }

  const bigEndianWasm = metadata();
  bigEndianWasm.target.byte_order = "big_endian";
  assert.throws(
    () => validateProcessorMetadata(bigEndianWasm),
    /target\.byte_order must be 'little_endian'/,
  );
});

test("rejects metadata layouts outside or overlapping their runtime regions", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/processor-descriptor-v1.json", import.meta.url),
    "utf8",
  ));

  const stateOutOfBounds = structuredClone(fixture);
  stateOutOfBounds.metadata.states[0].physical_state_byte_offset = 16;
  assert.throws(
    () => validateProcessorMetadata(stateOutOfBounds),
    /exceeds runtime physical-state storage size 16/,
  );

  const snapshotGap = structuredClone(fixture);
  snapshotGap.metadata.states[0].packed_snapshot_byte_offset = 1;
  assert.throws(
    () => validateProcessorMetadata(snapshotGap),
    /packed_snapshot_byte_offset must be 0/,
  );

  const overlappingControlOutput = structuredClone(fixture);
  overlappingControlOutput.metadata.control_outputs = [{
    ...structuredClone(fixture.metadata.inputs[0]),
    name: "meter",
    byte_offset: null,
    state_byte_offset: 0,
  }];
  assert.throws(
    () => validateProcessorMetadata(overlappingControlOutput),
    /overlaps metadata\.states\[0\] in runtime physical-state storage/,
  );

  const paramOutOfBounds = structuredClone(fixture);
  paramOutOfBounds.metadata.params = [{
    ...structuredClone(fixture.metadata.inputs[0]),
    name: "gain",
    byte_offset: 16,
    default_reprs: ["0x00000000"],
  }];
  assert.throws(
    () => validateProcessorMetadata(paramOutOfBounds),
    /exceeds runtime parameter storage size 16/,
  );

  const slotGap = structuredClone(fixture);
  slotGap.metadata.inputs[0].slot_offset = 1;
  assert.throws(
    () => validateProcessorMetadata(slotGap),
    /metadata\.inputs\[0\]\.slot_offset must be 0/,
  );
});

const FIXTURE_MIR_SCHEMA_VERSION = 1;

const wasm = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0,
  1, 4, 1, 96, 0, 0,
  3, 3, 2, 0, 0,
  5, 3, 1, 0, 1,
  6, 7, 1, 127, 0, 65, 128, 8, 11,
  7, 51, 4,
  6, 109, 101, 109, 111, 114, 121, 2, 0,
  11, 95, 95, 104, 101, 97, 112, 95, 98, 97, 115, 101, 3, 0,
  9, 111, 110, 100, 97, 95, 105, 110, 105, 116, 0, 0,
  12, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 0, 1,
  10, 7, 2, 2, 0, 11, 2, 0, 11,
]);

function metadata() {
  return {
    format: "onda-processor",
    format_version: PROCESSOR_ARTIFACT_FORMAT_VERSION,
    abi_version: PROCESSOR_ABI_VERSION,
    artifact_kind: "webassembly_module",
    backend: "test",
    mir_schema_version: FIXTURE_MIR_SCHEMA_VERSION,
    target: {
      triple: "wasm32-unknown-unknown",
      cpu: "generic",
      features: "",
      reloc_model: "static",
      code_model: "default",
      opt_level: "4",
      abi_name: null,
      data_layout: "e-m:e-p:32:32-i64:64-n32:64-S128",
      pointer_width_bits: 32,
      byte_order: "little_endian",
      pointer_model: "linear_memory_offset",
      calling_convention: "core-wasm",
    },
    compile: { sample_rate: 48_000, block_size: 128, fast_math: false },
    runtime: {
      state_size_bytes: 0,
      state_align_bytes: 1,
      param_size_bytes: 0,
      param_align_bytes: 1,
      snapshot_size_bytes: 0,
      state_initialization: "zeroed",
      snapshot_format_version: PROCESSOR_SNAPSHOT_FORMAT_VERSION,
      snapshot_byte_order: "little_endian",
      snapshot_restore_base: "post_init_physical_state_image",
      requires_full_blocks: false,
    },
    exports: {
      memory: "memory",
      heap_base: "__heap_base",
      init: "onda_init",
      process: "onda_process",
      events: [],
    },
    integration: {
      required_symbols: ["memory", "__heap_base", "onda_init", "onda_process"],
      one_processor_per_artifact: true,
      profile: {
        kind: "core_webassembly_module",
        imports: [],
        memory_export: "memory",
        heap_base_export: "__heap_base",
      },
    },
    metadata: {
      states: [],
      inputs: [],
      outputs: [],
      control_outputs: [],
      params: [],
      buffers: [],
      events: [],
    },
  };
}

test("validates descriptor and WebAssembly export kinds together", () => {
  const artifact = { wasm, metadata: metadata() };
  assert.equal(validateProcessorArtifact(artifact).wasm.byteLength, wasm.byteLength);
  const module = new WebAssembly.Module(wasm);
  assert.equal(validateProcessorModule(module, artifact.metadata).module, module);
});

test("rejects descriptor exports omitted from required_symbols", () => {
  const value = metadata();
  value.integration.required_symbols.pop();
  assert.throws(
    () => validateProcessorArtifact({ wasm, metadata: value }),
    /missing executable export 'onda_process'/,
  );
});

test("round-trips integrity-associated artifact files", async () => {
  const files = await createProcessorArtifactFiles({ wasm, metadata: metadata() }, {
    baseName: "test-processor",
  });
  const loaded = await loadProcessorArtifactFiles(files.wasm.bytes, files.metadata.text);
  assert.deepEqual(loaded.wasm, wasm);
});
