import assert from "node:assert/strict";
import test from "node:test";

import { OndaBrowserLsp } from "./lsp-client.js";

test("LSP analysis receives context settings independently of code generation", async () => {
  const updates = [];
  const compiler = {
    async setLspAnalysisOptions(options) { updates.push(options); },
    async sendLspMessage(message) {
      return message.id ? [{ id: message.id, result: { capabilities: {} } }] : [];
    },
  };
  const lsp = new OndaBrowserLsp(compiler);
  await lsp.initialize({ sampleRate: 48_000, blockSize: 512, codegen: { fastMath: true } });
  await lsp.setAnalysisOptions({ sampleRate: 44_100, blockSize: 256, codegen: { fastMath: false } });
  assert.deepEqual(updates, [
    { sampleRate: 48_000, blockSize: 512 },
    { sampleRate: 44_100, blockSize: 256 },
  ]);
  lsp.dispose();
});
