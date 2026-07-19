import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const exampleRoot = resolve(repoRoot, "examples/web/onda_wasm_playground");

test("the standalone example remains a thin host for ui/playground", async () => {
  const exampleFiles = new Set(await readdir(exampleRoot));
  for (const sharedFile of [
    "browser-buffers.js",
    "browser-buffers.test.js",
    "completions.js",
    "completions.test.js",
    "default.onda",
    "editor.js",
    "example-projects.test.js",
    "examples.js",
    "examples.test.js",
    "live.js",
    "lsp-client.js",
    "microphone.js",
    "microphone.test.js",
    "run-view-host.js",
    "share.js",
    "tab-order.js",
    "tab-order.test.js",
  ]) {
    assert.equal(exampleFiles.has(sharedFile), false, `${sharedFile} must remain owned by ui/playground`);
  }

  const bundler = await readFile(resolve(repoRoot, "scripts/bundle-web-playground.mjs"), "utf8");
  assert.match(bundler, /ui\/playground\/live\.js/);
  assert.doesNotMatch(bundler, /examples\/web\/onda_wasm_playground\/live\.js/);
});
