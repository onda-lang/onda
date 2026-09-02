import { mkdir } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export async function generateRustThirdPartyLicenses(output, manifestPath) {
  const outputPath = resolve(output);
  await mkdir(dirname(outputPath), { recursive: true });

  const scope = manifestPath
    ? ["--manifest-path", resolve(manifestPath)]
    : ["--workspace"];
  const result = spawnSync(process.platform === "win32" ? "cargo.exe" : "cargo", [
    "about",
    "generate",
    ...scope,
    "--locked",
    "--fail",
    resolve(repoRoot, "scripts/licenses/rust-notices.hbs"),
    "--output-file",
    outputPath,
  ], {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: "inherit",
  });

  if (result.error?.code === "ENOENT") {
    throw new Error(
      "cargo-about is required; install cargo-about 0.9.2 with the cli feature",
    );
  }
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`cargo about generate exited with status ${result.status}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const output = process.argv[2];
  if (!output) {
    throw new Error("usage: node scripts/generate-rust-third-party-licenses.mjs <output>");
  }
  await generateRustThirdPartyLicenses(output, process.argv[3]);
}
