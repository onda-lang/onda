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

test("both playground hosts keep only project-wide actions above the file tabs", async () => {
  const hosts = await Promise.all([
    readFile(resolve(repoRoot, "website/playground/index.html"), "utf8"),
    readFile(resolve(exampleRoot, "index.html"), "utf8"),
  ]);

  for (const host of hosts) {
    assert.match(host, /data-new-project>New project<\/button>/);
    assert.match(host, /data-open-project>Open project<\/button>/);
    assert.match(host, /data-download-project>Download project<\/button>/);
    assert.doesNotMatch(host, /data-rename-file|data-main-file/);
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
    /createNewProject\(\) \{\s+if \(!window\.confirm\("Create a new project\? This will delete your current project\."\)\) return;\s+await stopExecution\(\);/,
  );

  const editor = await readFile(resolve(repoRoot, "ui/playground/editor.js"), "utf8");
  assert.match(editor, /className = "project-file-menu-trigger"/);
  assert.match(editor, /action\("Rename", "project-file-menu-rename"/);
  assert.match(editor, /action\("Set as main", "project-file-menu-main"/);
  assert.match(editor, /"Delete",\s+"project-file-menu-delete"/);
});

test("the browser buffer picker stays hidden and cleans up after cancellation", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /input\.hidden = true/);
  assert.match(runView, /input\.addEventListener\("cancel", cleanup, \{ once: true \}\)/);
  assert.match(runView, /window\.addEventListener\("focus", cleanupAfterCancel, \{ once: true \}\)/);
});

test("the shared run view presents one Onda source and project importer", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /class="primary" id="choose-onda-file"/);
  assert.match(runView, /Open Onda source or project/);
  assert.match(runView, /drop an \.onda or \.ondaproject file/);
  assert.doesNotMatch(runView, /chooseProjectFolder|supportsProjectDirectorySelection/);
});

test("the shared run view only shows its scope when supported and during playback", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(
    runView,
    /scopeSection\.style\.display = supportsScope && state\.running \? "block" : "none"/,
  );
  assert.doesNotMatch(runView, /scopeSection\.style\.display = state\.connected/);
});

test("the shared run view renders host features from explicit capabilities", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  for (const capability of [
    "supportsSourceSelection",
    "supportsTransport",
    "supportsDeviceSelection",
    "supportsRunSettings",
    "supportsScope",
  ]) {
    assert.match(runView, new RegExp(`state\\.${capability} === true`));
  }
  assert.doesNotMatch(runView, /hostBridge\.mode === "wry"/);
  assert.doesNotMatch(runView, /hostBridge\.mode !== "browser"/);
});

test("the shared run view keeps parameter and event resets independent", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /id="reset-params"/);
  assert.match(runView, /id="reset-event-arguments"/);
  assert.match(runView, /type: "resetParams"/);
  assert.match(runView, /type: "resetEventArguments"/);
  assert.doesNotMatch(runView, /resetState|Reset state/);
  assert.match(runView, /\.shell \{[\s\S]*?grid-template-columns: minmax\(0, 1fr\)/);
  assert.match(
    runView,
    /\.header, \.buffers, \.events, \.params, \.scope-section \{[\s\S]*?width: 100%/,
  );
});

test("the empty native run view owns its compile settings", async () => {
  const [runView, webview] = await Promise.all([
    readFile(resolve(repoRoot, "ui/run/run.html"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_webview/src/lib.rs"), "utf8"),
  ]);

  assert.match(runView, /id="run-sample-rate"/);
  assert.match(runView, /id="run-block-size"/);
  assert.match(runView, /filePickerSettingsNode\.hidden = !supportsRunSettings/);
  assert.match(webview, /"supportsRunSettings": true/);
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
    /#params\[data-layout="sliders"\] \.param \{[\s\S]*?height: 80px;/,
  );
  assert.match(
    runView,
    /#params\[data-layout="knobs"\] \{[\s\S]*?minmax\(min\(100%, 140px\), 1fr\)/,
  );
  assert.match(
    runView,
    /grid-template-areas:\s+"heading value"\s+"control control";/,
  );
  assert.match(runView, /function paramDisplayName\(param\)/);
  assert.match(
    runView,
    /<div class="section-heading">\s*<button\s+class="section-toggle params-disclosure"\s+id="params-toggle"[\s\S]*?<\/button>\s*<div class="params-title">Params<\/div>\s*<button[^>]+id="reset-params"[\s\S]*?<\/div>\s*<div class="param-layout-toggle"/,
  );
  assert.match(runView, /classList\.add\("param-knob-ring-value"\)/);
  assert.match(runView, /valueArc\.style\.strokeDasharray = `\$\{ratio\} 1`/);
  assert.doesNotMatch(runView, /conic-gradient\(/);
});

test("the shared run view preserves controls across independent parameter updates", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");

  assert.match(runView, /const activeParamGestures = new Set\(\)/);
  assert.match(runView, /const paramBindings = new Map\(\)/);
  assert.match(
    runView,
    /if \(activeParamGestures\.has\(incoming\.name\)\) \{\s+binding\.param\.value = localValue;\s+\} else \{\s+binding\.setValue\(incoming\.value\);/,
  );
  assert.match(
    runView,
    /if \(updateRenderedParams\(state\.params\)\) \{\s+return;\s+\}\s+resetRenderedParams\(paramSignature\);/,
  );
  assert.equal(
    runView.match(/paramsNode\.replaceChildren\(\)/g)?.length,
    1,
    "parameter DOM replacement must be limited to schema/layout changes",
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

  assert.match(runView, /globalThis\.__ONDA_PARAM_CONTROL_V2__\.createParamDomain/);
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
  assert.match(playground, /hasOpenProjectButton: Boolean\(openProjectButton\)/);
  assert.match(playground, /hasDownloadProjectButton: Boolean\(downloadProjectButton\)/);
});

test("both native hosts preserve runtime options across unload", async () => {
  const [egui, webview] = await Promise.all([
    readFile(resolve(repoRoot, "crates/onda_egui/src/lib.rs"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_webview/src/lib.rs"), "utf8"),
  ]);

  assert.match(egui, /self\.options = controller\.options\(\)\.clone\(\)/);
  assert.match(webview, /\*options = current\.options\(\)\.clone\(\)/);
});
