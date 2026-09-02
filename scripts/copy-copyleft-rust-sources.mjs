import { cp, mkdir, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export async function copyCopyleftRustSources(output) {
  const outputPath = resolve(output);
  const relativeOutput = relative(repoRoot, outputPath);
  if (relativeOutput.startsWith(`..${sep}`) || relativeOutput === ".." || relativeOutput === "") {
    throw new Error("copyleft source output must be a subdirectory of the repository");
  }

  const packages = cargoMetadata().packages
    .filter(({ license }) => license?.includes("MPL-2.0"))
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
  if (packages.length === 0) throw new Error("Cargo metadata contains no MPL-2.0 packages");
  const workspacePackage = packages.find(({ source }) => source === null);
  if (workspacePackage) {
    throw new Error(`cannot copy workspace package ${workspacePackage.name} as an external source`);
  }

  await rm(outputPath, { recursive: true, force: true });
  await mkdir(outputPath, { recursive: true });
  for (const pkg of packages) {
    await cp(dirname(pkg.manifest_path), resolve(outputPath, `${pkg.name}-${pkg.version}`), {
      recursive: true,
    });
  }

  await writeFile(resolve(outputPath, "README.txt"), [
    "MPL-2.0 dependency source code",
    "==============================",
    "",
    "These directories are the exact Cargo-locked source packages used to build Onda.",
    "They accompany the executable distribution under Mozilla Public License 2.0.",
    "",
    ...packages.map(({ name, version }) => `- ${name} ${version}`),
    "",
  ].join("\n"));
}

function cargoMetadata() {
  const result = spawnSync(process.platform === "win32" ? "cargo.exe" : "cargo", [
    "metadata",
    "--format-version",
    "1",
    "--locked",
  ], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`cargo metadata failed:\n${result.stderr.trim()}`);
  }
  return JSON.parse(result.stdout);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const output = process.argv[2];
  if (!output) {
    throw new Error("usage: node scripts/copy-copyleft-rust-sources.mjs <output>");
  }
  await copyCopyleftRustSources(output);
}
