import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
test("the browser smoke check follows the project-only share contract", async () => {
  const playground = await readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8");

  assert.match(playground, /encodeSharedSession\(project\)/);
  assert.doesNotMatch(playground, /decodedSession\.(?:sampleRate|blockSize)/);
  assert.match(playground, /hasOpenProjectButton: Boolean\(openProjectButton\)/);
  assert.match(playground, /hasDownloadProjectButton: Boolean\(downloadProjectButton\)/);
});

test("the browser editor starts loaded projects as soon as the compiler is ready", async () => {
  const playground = await readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8");

  assert.match(
    playground,
    /if \(initialProjectError\)[\s\S]*?else \{[\s\S]*?setStatus\("Ready"\);\s+await runProject\(\);/,
  );
  assert.match(
    playground,
    /async function openProjectFile[\s\S]*?setStatus\("Project loaded"\);\s+await runProject\(\);/,
  );
});

test("browser compile settings live in the shared run UI and recompile on change", async () => {
  const [playground, examplePage, websitePage, runViewHost] = await Promise.all([
    readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8"),
    readFile(resolve(repoRoot, "examples/web/onda_wasm_playground/index.html"), "utf8"),
    readFile(resolve(repoRoot, "website/playground/index.html"), "utf8"),
    readFile(resolve(repoRoot, "ui/playground/run-view-host.js"), "utf8"),
  ]);

  for (const page of [examplePage, websitePage]) {
    assert.doesNotMatch(page, /data-sample-rate|data-block-size/);
    assert.match(page, /class="editor-font-size"/);
  }
  assert.match(runViewHost, /blockSamples: 512/);
  assert.match(
    playground,
    /async function applyCompileOptions\(settings\)[\s\S]*?setAnalysisOptions\(options\)[\s\S]*?compiler \? runProject\(\) : null/,
  );
  assert.match(
    playground,
    /if \(compiling\) \{\s+rerunRequested = true;\s+runGeneration \+= 1;/,
  );
  assert.match(
    playground,
    /setRunSettings: \(\{ sampleRateHz, blockSamples \}\) =>\s+applyCompileOptions/,
  );
});
