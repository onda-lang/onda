import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageRoot, "../..");
const abiRoot = resolve(repoRoot, "packages/onda_processor_abi");
const backendRoot = resolve(repoRoot, "packages/onda_binaryen_web");
const webAudioRoot = resolve(repoRoot, "packages/onda_webaudio");
const temporary = await mkdtemp(resolve(tmpdir(), "onda-wasm-package-"));
const require = createRequire(import.meta.url);
const binaryenRoot = dirname(require.resolve("binaryen"));
const skipBuild = process.argv.slice(2).includes("--skip-build");

try {
  if (!skipBuild) run("npm", ["run", "build"]);
  const buildInfo = JSON.parse(
    await readFile(resolve(packageRoot, "dist/build.json"), "utf8"),
  );
  const optimization = buildInfo.frontendOptimization;
  if (
    optimization?.tool !== "wasm-opt"
    || optimization.optimizeLevel !== 4
    || !Number.isInteger(optimization.inputBytes)
    || !Number.isInteger(optimization.outputBytes)
    || optimization.outputBytes >= optimization.inputBytes
  ) {
    throw new Error("compiler build does not record an effective wasm-opt O4 frontend pass");
  }
  const compilerPack = runJson("npm", [
    "pack",
    packageRoot,
    "--json",
    "--ignore-scripts",
    "--pack-destination",
    temporary,
  ])[0];
  const packedPaths = new Set(compilerPack.files.map((file) => file.path));
  for (const required of [
    "bin/onda-wasm.js",
    "dist/frontend/onda_compiler_web_bg.wasm",
    "dist/build.json",
    "dist/version.js",
    "dist/licenses/BINARYEN-LICENSE",
    "dist/licenses/LIBM-LICENSE",
    "src/index.d.ts",
    "src/index.js",
  ]) {
    if (!packedPaths.has(required)) {
      throw new Error(`packed compiler is missing ${required}`);
    }
  }
  const packedFrontendWasm = compilerPack.files.find(
    (file) => file.path === "dist/frontend/onda_compiler_web_bg.wasm",
  );
  if (packedFrontendWasm?.size !== optimization.outputBytes) {
    throw new Error(
      `packed frontend Wasm size ${String(packedFrontendWasm?.size)} does not match optimized build ${optimization.outputBytes}`,
    );
  }

  const binaryenPack = runJson("npm", [
    "pack",
    binaryenRoot,
    "--json",
    "--pack-destination",
    temporary,
  ])[0];
  const abiPack = packWorkspacePackage(abiRoot);
  const backendPack = packWorkspacePackage(backendRoot);
  const webAudioPack = packWorkspacePackage(webAudioRoot);
  const consumer = resolve(temporary, "consumer");
  const compilerTarball = resolve(temporary, compilerPack.filename);
  const binaryenTarball = resolve(temporary, binaryenPack.filename);
  const abiTarball = resolve(temporary, abiPack.filename);
  const backendTarball = resolve(temporary, backendPack.filename);
  const webAudioTarball = resolve(temporary, webAudioPack.filename);
  await mkdir(consumer);
  await writeFile(resolve(consumer, "package.json"), JSON.stringify({
    name: "onda-wasm-compiler-smoke",
    private: true,
    type: "module",
  }));

  run("npm", [
    "install",
    "--ignore-scripts",
    "--no-package-lock",
    binaryenTarball,
    abiTarball,
    backendTarball,
    webAudioTarball,
    compilerTarball,
  ], consumer);
  await writeFile(resolve(consumer, "smoke.mjs"), `
import { validateProcessorArtifact } from "@onda-lang/processor-abi";
import { compileTrustedMir } from "@onda-lang/binaryen-web";
import { MIR_SCHEMA_VERSION, createCompiler } from "@onda-lang/wasm-compiler";
import { flattenedAudioChannelCount } from "@onda-lang/webaudio";
if (typeof compileTrustedMir !== "function") throw new Error("missing packaged Binaryen backend");
if (flattenedAudioChannelCount([{ array_len: 2 }]) !== 2) throw new Error("invalid packaged Web Audio adapter");
const compiler = await createCompiler();
const artifact = await compiler.compileSource("sample:\\n  out1 = 0.125\\n");
validateProcessorArtifact(artifact);
if (artifact.metadata.mir_schema_version !== MIR_SCHEMA_VERSION) throw new Error("schema mismatch");
process.stdout.write(String(artifact.wasm.byteLength));
`);
  const smoke = run(process.execPath, [resolve(consumer, "smoke.mjs")], consumer);
  const bytes = Number(smoke.stdout);
  if (!Number.isInteger(bytes) || bytes <= 0) {
    throw new Error(`packed compiler smoke returned invalid size '${smoke.stdout}'`);
  }
  process.stdout.write(
    `Verified packed Onda web workspace ${compilerPack.version}: ${[abiPack, backendPack, webAudioPack, compilerPack].map((entry) => `${entry.name} ${entry.size} bytes`).join("; ")}; artifact ${bytes} bytes\n`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function packWorkspacePackage(root) {
  return runJson("npm", [
    "pack",
    root,
    "--json",
    "--ignore-scripts",
    "--pack-destination",
    temporary,
  ])[0];
}

function runJson(command, args, cwd = packageRoot) {
  const result = run(command, args, cwd);
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`failed to decode ${command} JSON output: ${error.message}\n${result.stdout}`);
  }
}

function run(command, args, cwd = packageRoot) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      npm_config_cache: resolve(temporary, "npm-cache"),
      npm_config_update_notifier: "false",
    },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} exited with status ${result.status}\n${result.stdout}\n${result.stderr}`,
    );
  }
  return result;
}
