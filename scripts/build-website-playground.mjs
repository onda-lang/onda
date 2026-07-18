import { createHash } from "node:crypto";
import { cp, mkdir, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "esbuild";
import { bundlePlayground } from "./bundle-web-playground.mjs";
import { buildExampleProjectCatalog } from "./example-projects.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const compilerRoot = resolve(repoRoot, "packages/onda_wasm_compiler");
const webAudioRoot = resolve(repoRoot, "packages/onda_webaudio");
const abiRoot = resolve(repoRoot, "packages/onda_processor_abi");
const generatedRoot = resolve(repoRoot, "target/website-play");
const examplesRoot = resolve(repoRoot, "examples");
const cargoToml = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
const version = cargoToml.match(
  /\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/,
)?.[1];
if (!version) throw new Error("failed to read the Onda workspace version");

const compilerBuild = JSON.parse(
  await readFile(resolve(compilerRoot, "dist/build.json"), "utf8"),
);
const frontendOptimization = compilerBuild.frontendOptimization;
if (
  frontendOptimization?.tool !== "wasm-opt"
  || frontendOptimization.optimizeLevel !== 4
  || !Number.isInteger(frontendOptimization.inputBytes)
  || !Number.isInteger(frontendOptimization.outputBytes)
  || frontendOptimization.outputBytes >= frontendOptimization.inputBytes
) {
  throw new Error(
    "the website requires an O4 wasm-opt compiler frontend; run the wasm-compiler build first",
  );
}
const frontendWasm = resolve(
  compilerRoot,
  "dist/frontend/onda_compiler_web_bg.wasm",
);
const frontendWasmBytes = (await stat(frontendWasm)).size;
if (frontendWasmBytes !== frontendOptimization.outputBytes) {
  throw new Error(
    `compiler frontend size ${frontendWasmBytes} does not match optimized build metadata ${frontendOptimization.outputBytes}`,
  );
}

const assetsRoot = resolve(generatedRoot, "assets/play", `v${version}`);
await rm(generatedRoot, { recursive: true, force: true });
await mkdir(assetsRoot, { recursive: true });

await Promise.all([
  cp(resolve(compilerRoot, "src"), resolve(assetsRoot, "compiler/src"), {
    recursive: true,
  }),
  cp(resolve(compilerRoot, "dist/backend"), resolve(assetsRoot, "compiler/dist/backend"), {
    recursive: true,
  }),
  cp(resolve(compilerRoot, "dist/version.js"), resolve(assetsRoot, "compiler/dist/version.js")),
  cp(
    frontendWasm,
    resolve(assetsRoot, "onda_compiler_web_bg.wasm"),
  ),
  cp(resolve(abiRoot, "src/index.js"), resolve(assetsRoot, "processor-abi.js")),
  cp(resolve(webAudioRoot, "src/index.js"), resolve(assetsRoot, "webaudio.js")),
  cp(resolve(webAudioRoot, "src/worklet.js"), resolve(assetsRoot, "worklet.js")),
  bundlePlayground(resolve(assetsRoot, "playground.js")),
  buildExampleProjectCatalog(examplesRoot).then((catalog) => writeFile(
    resolve(assetsRoot, "example-projects.json"),
    `${JSON.stringify(catalog)}\n`,
  )),
  cp(resolve(repoRoot, "ui/run/run.html"), resolve(assetsRoot, "run.html")),
]);

await build({
  entryPoints: [resolve(compilerRoot, "src/worker.js")],
  outfile: resolve(assetsRoot, "compiler-worker.js"),
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  minify: true,
  legalComments: "none",
  // Binaryen's universal bundle keeps Node-only dynamic imports behind an
  // environment guard. Preserve those unreachable specifiers for browsers.
  external: ["node:*"],
});

await writeFile(
  resolve(generatedRoot, "manifest.json"),
  `${JSON.stringify({
    version,
    assetRoot: await contentAddressAssets(),
    frontendOptimization,
  }, null, 2)}\n`,
);
process.stdout.write(`Built versioned website playground assets for Onda ${version}\n`);

async function contentAddressAssets() {
  const files = await filesBelow(assetsRoot);
  const hash = createHash("sha256");
  for (const file of files) {
    hash.update(relative(assetsRoot, file));
    hash.update("\0");
    hash.update(await readFile(file));
    hash.update("\0");
  }
  const directory = `v${version}-${hash.digest("hex").slice(0, 12)}`;
  await rename(assetsRoot, resolve(dirname(assetsRoot), directory));
  return `assets/play/${directory}`;
}

async function filesBelow(directory) {
  const result = [];
  const entries = await readdir(directory, { withFileTypes: true });
  for (const entry of entries) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) result.push(...await filesBelow(path));
    else if (entry.isFile()) result.push(path);
  }
  return result.sort();
}
