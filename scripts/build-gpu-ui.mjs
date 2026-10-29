import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, copyFileSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gpuUiFiles, gpuUiVersion } from "./gpu-ui-assets.mjs";
const root = fileURLToPath(new URL("../", import.meta.url));
const require = createRequire(new URL("../packages/onda_wasm_compiler/package.json", import.meta.url));
const wasmOpt = resolve(dirname(require.resolve("binaryen")), "bin/wasm-opt");
const version = gpuUiVersion();
function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`);
}
rmSync(resolve(root,"ui/gpu/pkg"), { recursive:true, force:true });
run("wasm-pack", ["build", "crates/onda_ui", "--target", "web", "--release", "--no-opt",
  "--out-dir", "../../ui/gpu/pkg", "--no-pack"]);
const wasm = resolve(root, "ui/gpu/pkg/onda_ui_bg.wasm");
run(process.execPath, [wasmOpt, "-O4", wasm, "-o", `${wasm}.opt`]);
renameSync(`${wasm}.opt`, wasm);
for (const name of readdirSync(resolve(root, "assets/fonts")))
  if (name.startsWith("OFL-") && name.endsWith(".txt"))
    copyFileSync(resolve(root, "assets/fonts", name), resolve(root, "ui/gpu/pkg", name));
cpSync(resolve(root, "assets/fonts/fallback"), resolve(root, "ui/gpu/pkg/fonts"), { recursive:true });
for (const name of ["Inter.ttf", "JetBrainsMono.ttf"])
  copyFileSync(resolve(root, "assets/fonts", name), resolve(root, "ui/gpu/pkg/fonts", name));

// Certify the completed build, not the checkout from which it is later staged.
const hashes = Object.fromEntries(gpuUiFiles().map(name => [name,
  createHash("sha256").update(readFileSync(resolve(root, "ui/gpu", name))).digest("hex"),
]));
writeFileSync(resolve(root, "ui/gpu/pkg/build.json"), JSON.stringify({ version, files: hashes }, null, 2) + "\n");
