import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
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

test("the shared run view keeps output compact and latched directly after the scope", async () => {
  const [runView, runHost, egui] = await Promise.all([
    readFile(resolve(repoRoot, "ui/run/run.html"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_run/src/lib.rs"), "utf8"),
    readFile(resolve(repoRoot, "crates/onda_egui/src/lib.rs"), "utf8"),
  ]);

  assert.match(
    runView,
    /id="scope-section"[\s\S]*?<\/section>\s*<section class="events" id="log-section"/,
  );
  assert.match(runView, /\.log-output \{[\s\S]*?max-height: 120px/);
  assert.match(
    runView,
    /\.log-output \{[\s\S]*?width: 100%;[\s\S]*?overflow-x: hidden;[\s\S]*?overflow-y: auto;/,
  );
  assert.match(
    runView,
    /logSection\.style\.display = state\.logRevealed === true \? "" : "none"/,
  );
  assert.match(egui, /ScrollArea::vertical\(\)[\s\S]*?\.auto_shrink\(\[false, true\]\)/);
  assert.match(runHost, /if !text\.is_empty\(\) \|\| !entries\.is_empty\(\) \{\s*self\.state\.log_revealed = true/);
  const clearLog = runHost.match(/pub fn clear_log\(&mut self\) \{[\s\S]*?\n    \}/)?.[0];
  assert.ok(clearLog);
  assert.doesNotMatch(clearLog, /log_revealed/);
  assert.doesNotMatch(runView, /Waiting for print output/);
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

test("the shared run view preserves explicit false scalar values", async () => {
  const runView = await readFile(resolve(repoRoot, "ui/run/run.html"), "utf8");
  const helper = runView.match(/function booleanScalarValue\(value\) \{[\s\S]*?\n      \}/)?.[0];

  assert.ok(helper);
  const booleanScalarValue = Function(`return (${helper})`)();
  assert.equal(booleanScalarValue(false), false);
  assert.equal(booleanScalarValue(true), true);
  assert.equal(booleanScalarValue(0), false);
  assert.equal(booleanScalarValue(1), true);
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
  assert.match(runView, /function knobValueArcPath\(normalized\)/);
  assert.match(runView, /valueArc\.setAttribute\("d", knobValueArcPath\(ratio\)\)/);
  assert.match(
    runView,
    /\.param-knob-ring-value \{[\s\S]*?stroke-linecap: butt;/,
  );
  assert.doesNotMatch(runView, /valueArc\.setAttribute\("stroke-dasharray"/);
  assert.match(
    runView,
    /KNOB_ARC_START_DEGREES \+ 90 \+ ratio \* KNOB_ARC_SWEEP_DEGREES/,
  );
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
