import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(new URL("./live.js", import.meta.url), "utf8");
const bindings = source.slice(source.indexOf("async function bindBufferFile("),
  source.indexOf("async function restartAudioForBuffers("));

function host() {
  const requests = [], updates = [], errors = [];
  const context = vm.createContext({
    File, compiler: {}, artifact: { metadata: { metadata: { buffers: [{ name: "clip" }] } } },
    runGeneration: 1, bufferFiles: new Map(), pendingBufferBindings: new Map(),
    context: null, processingRequested: false,
    prepareBufferBinding: () => new Promise((resolve, reject) => requests.push({ resolve, reject })),
    runView: { updateBufferFile: (...args) => updates.push(args), showError: error => errors.push(error) },
    setErrorStatus() {},
  });
  vm.runInContext(bindings, context);
  return { context, requests, updates, errors,
    bind: file => context.bindBufferFile("clip", file),
    clear: () => context.clearBuffer("clip"),
  };
}

for (const invalidate of ["source", "compile", "clear"]) {
  for (const fails of [false, true]) {
    test(`buffer decode ignores ${fails ? "failure" : "success"} after ${invalidate}`, async () => {
      const view = host();
      const file = new File(["audio"], "old.wav");
      const binding = view.bind(file);
      if (invalidate === "source") view.context.runGeneration++;
      else if (invalidate === "compile") view.context.artifact = {};
      else await view.clear();
      if (fails) view.requests[0].reject(new Error("obsolete decode"));
      else view.requests[0].resolve({ frames: 10 });
      await binding;
      assert.equal(view.context.bufferFiles.size, 0);
      assert.deepEqual(view.errors, []);
      assert.equal(view.updates.length, invalidate === "clear" ? 1 : 0);
      assert.equal(view.context.pendingBufferBindings.size, 0);
    });
  }
}

test("the latest buffer selection wins when decodes finish in reverse order", async () => {
  const view = host();
  const old = new File(["old"], "old.wav");
  const latest = new File(["latest"], "latest.wav");
  const first = view.bind(old), second = view.bind(latest);
  const metadata = { frames: 20 };
  view.requests[1].resolve(metadata);
  await second;
  view.requests[0].resolve({ frames: 10 });
  await first;
  assert.equal(view.context.bufferFiles.get("clip"), latest);
  assert.deepEqual(view.updates, [["clip", latest, metadata]]);
  assert.equal(view.context.pendingBufferBindings.size, 0);
});

test("a current buffer decode failure still reports the error", async () => {
  const view = host();
  const binding = view.bind(new File(["invalid"], "bad.wav"));
  const error = new Error("invalid audio");
  view.requests[0].reject(error);
  await binding;
  assert.deepEqual(view.errors, [error]);
  assert.equal(view.context.pendingBufferBindings.size, 0);
});
