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
    sources: {
      "main.onda": `include "./level.onda"

buffers:
  clip: buffer[f32]

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
  assert.equal(artifact.metadata.compile.block_size, 256);
  assert.deepEqual(
    artifact.metadata.metadata.buffers.map((buffer) => buffer.name),
    ["clip"],
  );
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

test("runs the Onda LSP protocol inside frontend Wasm", async () => {
  const compiler = await createCompiler();
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
  const artifact = await compiler.compileSource(SOURCE);
  assert.deepEqual([...artifact.wasm], [0, 97, 115, 109]);
  assert.deepEqual(
    await compiler.sendLspMessage({ jsonrpc: "2.0", id: 9, method: "shutdown" }),
    [{ jsonrpc: "2.0", id: 9, result: null }],
  );
  await compiler.dispose();
  assert.equal(compiler.worker.terminated, true);
});
