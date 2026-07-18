import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const temporary = await mkdtemp(resolve(tmpdir(), "onda-wasm-package-"));

try {
  run("npm", ["run", "build"]);
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

  const binaryenPack = runJson("npm", [
    "pack",
    resolve(packageRoot, "node_modules/binaryen"),
    "--json",
    "--pack-destination",
    temporary,
  ])[0];
  const consumer = resolve(temporary, "consumer");
  const compilerTarball = resolve(temporary, compilerPack.filename);
  const binaryenTarball = resolve(temporary, binaryenPack.filename);
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
    compilerTarball,
  ], consumer);
  await writeFile(resolve(consumer, "smoke.mjs"), `
import { MIR_SCHEMA_VERSION, createCompiler } from "@onda-lang/wasm-compiler";
const compiler = await createCompiler();
const artifact = await compiler.compileSource("sample:\\n  out1 = 0.125\\n");
if (!WebAssembly.validate(artifact.wasm)) throw new Error("invalid packaged artifact");
if (artifact.metadata.mir_schema_version !== MIR_SCHEMA_VERSION) throw new Error("schema mismatch");
process.stdout.write(String(artifact.wasm.byteLength));
`);
  const smoke = run(process.execPath, [resolve(consumer, "smoke.mjs")], consumer);
  const bytes = Number(smoke.stdout);
  if (!Number.isInteger(bytes) || bytes <= 0) {
    throw new Error(`packed compiler smoke returned invalid size '${smoke.stdout}'`);
  }
  process.stdout.write(
    `Verified packed ${compilerPack.name}@${compilerPack.version}: ${compilerPack.size} bytes; artifact ${bytes} bytes\n`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
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
