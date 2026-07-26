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
    assert.match(host, /class="project-file-add"[^>]+data-new-file[^>]*>\+<\/button>/);
    assert.doesNotMatch(host, /data-new-file>New file<\/button>/);
    assert.match(host, /Ctrl\/⌘ ↵/);
    assert.match(host, /Ctrl\/⌘ \./);
    assert.match(host, /data-status>Loading<\/span>/);
    assert.match(host, /data-editor-font-size/);
    assert.match(host, /<option value="2048">2048 frames<\/option>/);
  }

  const styles = await readFile(resolve(repoRoot, "website/assets/site/styles.css"), "utf8");
  assert.match(styles, /\.play-workspace[^\n]+align-items: stretch/);
  assert.match(styles, /\.play-run-panel \{ display: flex/);
  assert.match(styles, /\.status::before[^\n]+border-radius: 50%/);
  assert.match(hosts[1], /\.status::before \{[^}]+border-radius: 50%/s);

  const playground = await readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8");
  assert.doesNotMatch(playground, /setStatus\([^\n]*(?:Ctrl|Cmd|Period)/);
  assert.match(playground, /setStatus\("Compiling"\)/);
  assert.match(playground, /setStatus\("Error", "fail"\)/);
  assert.match(
    playground,
    /createNewPatch\(\) \{\s+if \(!window\.confirm\("Create a new patch\? This will delete your current project\."\)\) return;\s+await stopExecution\(\);/,
  );
});

test("the browser buffer picker stays hidden and cleans up after cancellation", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /input\.hidden = true/);
  assert.match(runView, /input\.addEventListener\("cancel", cleanup, \{ once: true \}\)/);
  assert.match(runView, /window\.addEventListener\("focus", cleanupAfterCancel, \{ once: true \}\)/);
});

test("the shared run view only shows its scope during playback", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /scopeSection\.style\.display = state\.running \? "block" : "none"/);
  assert.doesNotMatch(runView, /scopeSection\.style\.display = state\.connected/);
});

test("the empty native run view owns its compile settings", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /id="run-sample-rate"/);
  assert.match(runView, /id="run-block-size"/);
  assert.match(runView, /blockFrames: 256/);
  assert.match(runView, /type: "setRunSettings"/);
});

test("the loaded run view includes active compile settings in its status", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /formatRunStatus\(status, state\.sampleRateHz, state\.blockFrames\)/);
  assert.match(runView, /" \\u2014 " \+ formatBufferSampleRate\(sampleRate\)/);
  assert.match(runView, /" \\u00b7 " \+ blockFrames \+ " frames"/);
});

test("the shared run view offers a device-cached knob layout", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /data-param-layout="sliders" aria-pressed="true">Sliders/);
  assert.match(runView, /data-param-layout="knobs" aria-pressed="false">Knobs/);
  assert.match(runView, /onda\.run-view\.param-layout\.v1/);
  assert.match(runView, /localStorage\.setItem\(PARAM_LAYOUT_STORAGE_KEY, paramLayout\)/);
  assert.match(runView, /function createKnobControl\(/);
  assert.match(runView, /knob\.setAttribute\("role", "slider"\)|initializeRangeControl\(knob, param\)/);
  assert.match(runView, /paramLayout === "knobs" \? createKnobControl : createSliderControl/);
  assert.match(
    runView,
    /<div class="params-title">Params<\/div>\s*<div class="param-layout-toggle"[\s\S]*?<\/div>\s*<button\s+class="section-toggle params-disclosure"\s+id="params-toggle"/,
  );
});

test("wide floating-point controls retain fractional precision", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /const FLOAT_CONTROL_TARGET_STEPS = 2000;/);
  assert.match(runView, /const FLOAT_CONTROL_MIN_STEP = 0\.0001;/);
  assert.match(runView, /const FLOAT_CONTROL_MAX_STEP = 0\.1;/);
  assert.match(runView, /return Math\.min\(Math\.pow\(10, exponent\), FLOAT_CONTROL_MAX_STEP\)/);
  assert.match(runView, /if \(param\.type === "i32" \|\| param\.type === "i64"\) \{\s+return 1;/);
});

test("the shared run view uses the processor ABI parameter conversions", async () => {
  const [runView, webview, websiteBuild] = await Promise.all([
    readFile(resolve(repoRoot, "ui/run/run.html"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_webview/src/lib.rs"), "utf8"),
    readFile(resolve(repoRoot, "scripts/build-website-playground.mjs"), "utf8"),
  ]);

  assert.match(runView, /globalThis\.__ONDA_PARAM_CONTROL_V2__\.createParamControl/);
  assert.match(runView, /\.plainToNormalized\(value\)/);
  assert.match(runView, /\.normalizedToPlain\(normalized\)/);
  assert.match(runView, /\.constrainPlain\(value\)/);
  assert.match(runView, /String\(step\)\.toLowerCase\(\)\.split\("e"\)/);
  assert.doesNotMatch(runView, /Math\.log\(plain \/ min\)/);
  assert.doesNotMatch(runView, /snapped\.toFixed/);
  assert.match(webview, /packages\/onda_processor_abi\/src\/param-control\.js/);
  assert.match(websiteBuild, /src\/param-control\.js/);
});

test("the browser smoke check follows the project-only share contract", async () => {
  const playground = await readFile(resolve(repoRoot, "ui/playground/live.js"), "utf8");

  assert.match(playground, /encodeSharedSession\(project\)/);
  assert.doesNotMatch(playground, /decodedSession\.(?:sampleRate|blockSize)/);
});

test("both native hosts preserve runtime options across unload", async () => {
  const [egui, webview] = await Promise.all([
    readFile(resolve(repoRoot, "crates/onda_egui/src/lib.rs"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_webview/src/lib.rs"), "utf8"),
  ]);

  assert.match(egui, /self\.options = controller\.options\(\)\.clone\(\)/);
  assert.match(webview, /\*options = current\.options\(\)\.clone\(\)/);
});
