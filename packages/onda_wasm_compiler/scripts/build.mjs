import { cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  generateRustThirdPartyLicenses,
} from "../../../scripts/generate-rust-third-party-licenses.mjs";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageRoot, "../..");
const distRoot = resolve(packageRoot, "dist");
const frontendOut = resolve(distRoot, "frontend");
const backendOut = resolve(distRoot, "backend");
const licensesOut = resolve(distRoot, "licenses");
const require = createRequire(import.meta.url);
const binaryenRoot = dirname(require.resolve("binaryen"));
const wasmOpt = resolve(binaryenRoot, "bin/wasm-opt");
const wasmOptLevel = 4;

run(process.execPath, [resolve(repoRoot, "scripts/sync-package-versions.mjs")]);

await rm(distRoot, { recursive: true, force: true });
await mkdir(frontendOut, { recursive: true });

// wasm-pack bundles its own wasm-opt release. Skip that implicit pass and run
// the workspace-pinned Binaryen below so the optimizer version and policy are
// identical in local, CI, website, and npm builds.
run("wasm-pack", [
  "build",
  resolve(repoRoot, "crates/onda_compiler_web"),
  "--target",
  "web",
  "--release",
  "--no-opt",
  "--out-dir",
  frontendOut,
  "--out-name",
  "onda_compiler_web",
]);

const frontendWasm = resolve(frontendOut, "onda_compiler_web_bg.wasm");
const optimizedFrontendWasm = resolve(frontendOut, "onda_compiler_web_bg.opt.wasm");
const unoptimizedBytes = (await stat(frontendWasm)).size;
run(process.execPath, [
  wasmOpt,
  `-O${wasmOptLevel}`,
  frontendWasm,
  "-o",
  optimizedFrontendWasm,
]);
const optimizedBytes = (await stat(optimizedFrontendWasm)).size;
if (optimizedBytes >= unoptimizedBytes) {
  throw new Error(
    `wasm-opt did not reduce the frontend Wasm (${unoptimizedBytes} -> ${optimizedBytes} bytes)`,
  );
}
await cp(optimizedFrontendWasm, frontendWasm);
await rm(optimizedFrontendWasm);

await cp(resolve(repoRoot, "packages/onda_binaryen_web/src"), backendOut, {
  recursive: true,
});
await rm(resolve(frontendOut, ".gitignore"), { force: true });
await mkdir(licensesOut, { recursive: true });
await cp(
  resolve(repoRoot, "packages/onda_binaryen_web/math-kernel/LICENSE-libm.txt"),
  resolve(licensesOut, "LIBM-LICENSE.txt"),
);
await cp(
  resolve(binaryenRoot, "LICENSE"),
  resolve(licensesOut, "BINARYEN-LICENSE.txt"),
);
await cp(resolve(repoRoot, "LICENSE"), resolve(licensesOut, "ONDA-LICENSE.txt"));
await generateRustThirdPartyLicenses(
  resolve(licensesOut, "RUST-DEPENDENCIES.txt"),
  resolve(repoRoot, "crates/onda_compiler_web/Cargo.toml"),
);

const packageManifest = JSON.parse(
  await readFile(resolve(packageRoot, "package.json"), "utf8"),
);
const frontendManifest = JSON.parse(
  await readFile(resolve(frontendOut, "package.json"), "utf8"),
);
if (frontendManifest.version !== packageManifest.version) {
  throw new Error(
    `frontend Wasm version ${frontendManifest.version} does not match package version ${packageManifest.version}`,
  );
}

await writeFile(
  resolve(distRoot, "build.json"),
  `${JSON.stringify({
    ondaVersion: packageManifest.version,
    binaryenVersion: packageManifest.dependencies.binaryen,
    frontendPackage: frontendManifest.name,
    frontendOptimization: {
      tool: "wasm-opt",
      binaryenVersion: packageManifest.dependencies.binaryen,
      optimizeLevel: wasmOptLevel,
      inputBytes: unoptimizedBytes,
      outputBytes: optimizedBytes,
    },
  }, null, 2)}\n`,
);
await writeFile(
  resolve(distRoot, "version.js"),
  `export const ONDA_VERSION = ${JSON.stringify(packageManifest.version)};\n`,
);

const reduction = ((1 - optimizedBytes / unoptimizedBytes) * 100).toFixed(1);
process.stdout.write(
  `Optimized frontend Wasm with Binaryen ${packageManifest.dependencies.binaryen} O${wasmOptLevel}: ${unoptimizedBytes} -> ${optimizedBytes} bytes (-${reduction}%)\n`,
);
process.stdout.write(`Built ${packageManifest.name} ${packageManifest.version}\n`);

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}
