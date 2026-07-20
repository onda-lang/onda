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
    "compile-cache.js",
    "compile-cache.test.js",
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

test("both playground hosts expose new-patch and stable shortcut controls", async () => {
  const hosts = await Promise.all([
    readFile(resolve(repoRoot, "website/playground/index.html"), "utf8"),
    readFile(resolve(exampleRoot, "index.html"), "utf8"),
  ]);

  for (const host of hosts) {
    assert.match(host, /data-new-patch/);
    assert.match(host, /Ctrl\/Cmd \+ Enter/);
    assert.match(host, /Ctrl \+ Period/);
    assert.match(host, /data-status>Loading<\/span>/);
  }

  const styles = await readFile(resolve(repoRoot, "website/assets/site/styles.css"), "utf8");
  assert.match(styles, /\.play-workspace[^\n]+align-items: stretch/);
  assert.match(styles, /\.play-run-panel \{ display: flex/);
  assert.match(styles, /\.play-intro-meta \.status[^\n]+inline-size:[^\n]+border-radius: 5px/);
  assert.match(hosts[1], /\.status \{[^}]+inline-size:[^}]+border-radius: 6px/s);

  const playground = await readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8");
  assert.doesNotMatch(playground, /setStatus\([^\n]*(?:Ctrl|Cmd|Period)/);
  assert.match(playground, /setStatus\("Compiling"\)/);
  assert.match(playground, /setStatus\("Error", "fail"\)/);
});
