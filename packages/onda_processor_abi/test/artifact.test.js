import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import test from "node:test";

import {
  createProcessorArtifactFiles,
  loadProcessorArtifactFiles,
  validateProcessorArtifact,
  validateProcessorModule,
} from "../src/index.js";

if (!globalThis.crypto) globalThis.crypto = webcrypto;

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
    format_version: 3,
    abi_version: 1,
    artifact_kind: "webassembly_module",
    backend: "test",
    mir_schema_version: FIXTURE_MIR_SCHEMA_VERSION,
    target: {
      triple: "wasm32-unknown-unknown",
      pointer_width_bits: 32,
      byte_order: "little_endian",
      pointer_model: "linear_memory_offset",
      calling_convention: "core-wasm",
    },
    compile: { sample_rate: 48_000, block_size: 128 },
    runtime: {
      state_size_bytes: 0,
      state_align_bytes: 1,
      param_size_bytes: 0,
      param_align_bytes: 1,
      snapshot_size_bytes: 0,
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
