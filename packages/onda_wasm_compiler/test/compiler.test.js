import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  MIR_SCHEMA_VERSION,
  ONDA_VERSION,
  OndaCompileError,
  createCompiler,
  createProcessorArtifactFiles,
} from "../src/index.js";

const SOURCE = `params:
  gain = 0.5 { 0.0, 1.0 }

sample:
  out1 = gain
`;

test("compiles Onda source to a complete processor artifact", async () => {
  const compiler = await createCompiler();
  const artifact = await compiler.compileSource(SOURCE, {
    sampleRate: 48_000,
    blockSize: 128,
  });

  const manifest = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  );
  assert.equal(ONDA_VERSION, manifest.version);
  assert.equal(MIR_SCHEMA_VERSION, 5);
  assert.equal(WebAssembly.validate(artifact.wasm), true);
  assert.equal(artifact.metadata.mir_schema_version, MIR_SCHEMA_VERSION);
  assert.equal(artifact.metadata.compile.sample_rate, 48_000);
  assert.equal(artifact.metadata.compile.block_size, 128);
  assert.equal(artifact.metadata.artifact_kind, "webassembly_module");

  const files = await createProcessorArtifactFiles(artifact, { baseName: "gain" });
  assert.equal(files.wasm.name, "gain.wasm");
  assert.equal(files.metadata.name, "gain.onda.json");
  assert.match(files.metadata.text, /"integrity"/);
});

test("compiles an in-memory project through the same product API", async () => {
  const compiler = await createCompiler();
  const artifact = await compiler.compileProject({
    entry: "main.onda",
    sources: { "main.onda": SOURCE },
  });
  assert.equal(WebAssembly.validate(artifact.wasm), true);
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
      return true;
    },
  );
});

test("offers an asynchronous browser-worker client", async () => {
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
      const value = message.type === "compileSource"
        ? { wasm: new Uint8Array([0, 97, 115, 109]), metadata: {} }
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
  const artifact = await compiler.compileSource(SOURCE);
  assert.deepEqual([...artifact.wasm], [0, 97, 115, 109]);
  await compiler.dispose();
  assert.equal(compiler.worker.terminated, true);
});
