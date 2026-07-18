import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageRoot, "../..");
const distRoot = resolve(packageRoot, "dist");
const frontendOut = resolve(distRoot, "frontend");
const backendOut = resolve(distRoot, "backend");
const licensesOut = resolve(distRoot, "licenses");

run(process.execPath, [resolve(repoRoot, "scripts/sync-package-versions.mjs")]);

await rm(distRoot, { recursive: true, force: true });
await mkdir(frontendOut, { recursive: true });

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

await cp(resolve(repoRoot, "packages/onda_binaryen_web/src"), backendOut, {
  recursive: true,
});
await rm(resolve(frontendOut, ".gitignore"), { force: true });
await mkdir(licensesOut, { recursive: true });
await cp(
  resolve(repoRoot, "packages/onda_binaryen_web/math-kernel/LICENSE-libm.txt"),
  resolve(licensesOut, "LIBM-LICENSE"),
);
await cp(
  resolve(packageRoot, "node_modules/binaryen/LICENSE"),
  resolve(licensesOut, "BINARYEN-LICENSE"),
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
  }, null, 2)}\n`,
);
await writeFile(
  resolve(distRoot, "version.js"),
  `export const ONDA_VERSION = ${JSON.stringify(packageManifest.version)};\n`,
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
