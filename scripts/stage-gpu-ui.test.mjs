import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { gpuUiFiles, gpuUiVersion } from "./gpu-ui-assets.mjs";
import { stageGpuUi } from "./stage-gpu-ui.mjs";

const hash = bytes => createHash("sha256").update(bytes).digest("hex");

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "onda-gpu-staging-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const source = join(root, "source");
  const destination = join(root, "destination");
  const manifest = { version: gpuUiVersion(), files: {} };
  for (const name of gpuUiFiles()) {
    const bytes = Buffer.from(`fixture: ${name}`);
    await mkdir(dirname(join(source, name)), { recursive: true });
    await writeFile(join(source, name), bytes);
    manifest.files[name] = hash(bytes);
  }
  const manifestPath = join(source, "pkg/build.json");
  const save = () => writeFile(manifestPath, JSON.stringify(manifest, null, 2) + "\n");
  await save();
  return { source, destination, manifest, manifestPath, save };
}

test("stages every verified asset and preserves the original manifest", async t => {
  const f = await fixture(t);
  await writeFile(join(f.source, "pkg/fonts/unlisted.ttf"), "not certified");
  await stageGpuUi(f.destination, f.source);
  for (const name of [...Object.keys(f.manifest.files), "pkg/build.json"]) {
    assert.deepEqual(await readFile(join(f.destination, name)), await readFile(join(f.source, name)));
  }
  await assert.rejects(readFile(join(f.destination, "pkg/fonts/unlisted.ttf")), { code: "ENOENT" });
});

const failures = [
  ["stale version", async f => { f.manifest.version = "0.0.0"; await f.save(); }, /expected/],
  ["missing manifest", f => rm(f.manifestPath), /ENOENT/],
  ["malformed manifest", f => writeFile(f.manifestPath, "{"), /JSON/],
  ["invalid file map", async f => { f.manifest.files = []; await f.save(); }, /Invalid.*files/],
  ["missing runtime entry", async f => { delete f.manifest.files["host.js"]; await f.save(); }, /missing host.js/],
  ["missing font entry", async f => { delete f.manifest.files["pkg/fonts/Inter.ttf"]; await f.save(); }, /missing.*Inter.ttf/],
  ["missing notice entry", async f => { delete f.manifest.files["pkg/OFL-Inter.txt"]; await f.save(); }, /missing.*OFL-Inter.txt/],
  ["missing asset", f => rm(join(f.source, "pkg/onda_ui_bg.wasm")), /ENOENT/],
  ["empty certified asset", async f => {
    await writeFile(join(f.source, "host.js"), "");
    f.manifest.files["host.js"] = hash("");
    await f.save();
  }, /asset changed.*host.js/],
  ["invalid hash", async f => { f.manifest.files["host.js"] = "bad"; await f.save(); }, /Invalid.*entry/],
  ["unsafe path", async f => { f.manifest.files["../outside.js"] = hash("outside"); await f.save(); }, /Invalid.*entry/],
];
for (const name of ["host.js", "pkg/onda_ui.js", "pkg/onda_ui_bg.wasm", "pkg/fonts/NotoEmoji.ttf", "pkg/OFL-Inter.txt"]) {
  failures.push([`changed ${name}`, f => writeFile(join(f.source, name), "changed"), /asset changed since build/]);
}

for (const [name, corrupt, expected] of failures) {
  test(`rejects ${name} before creating or changing the destination`, async t => {
    const f = await fixture(t);
    await corrupt(f);
    const rejects = () => assert.rejects(stageGpuUi(f.destination, f.source), error => {
      assert.match(error.message, expected);
      assert.match(error.message, /npm run build:gpu-ui/);
      return true;
    });
    await rejects();
    await assert.rejects(readdir(f.destination), { code: "ENOENT" });
    await mkdir(f.destination);
    await writeFile(join(f.destination, "host.js"), "previous build");
    await rejects();
    assert.deepEqual(await readdir(f.destination), ["host.js"]);
    assert.equal(await readFile(join(f.destination, "host.js"), "utf8"), "previous build");
  });
}
