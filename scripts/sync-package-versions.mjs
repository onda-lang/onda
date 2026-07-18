import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checkOnly = process.argv.slice(2).includes("--check");
const cargoToml = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
const workspaceVersion = cargoToml.match(
  /\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/,
)?.[1];

if (!workspaceVersion) {
  throw new Error("failed to read [workspace.package].version from Cargo.toml");
}

const mismatches = [];
const packageDirectories = await ondaPackageDirectories();
for (const packageDirectory of packageDirectories) {
  await synchronizeJson(resolve(packageDirectory, "package.json"), [["version"]]);
  const lockPath = resolve(packageDirectory, "package-lock.json");
  try {
    await synchronizeJson(lockPath, [
      ["version"],
      ["packages", "", "version"],
    ]);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
}

await synchronizeCargoLock();

if (mismatches.length > 0) {
  throw new Error(
    `Onda version ${workspaceVersion} is not synchronized:\n${mismatches.map((entry) => `- ${entry}`).join("\n")}`,
  );
}

const action = checkOnly ? "Verified" : "Synchronized";
process.stdout.write(
  `${action} Onda version ${workspaceVersion} across Cargo.lock and ${packageDirectories.length} npm packages\n`,
);

async function ondaPackageDirectories() {
  const packagesRoot = resolve(repoRoot, "packages");
  const entries = await readdir(packagesRoot, { withFileTypes: true });
  const result = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const directory = resolve(packagesRoot, entry.name);
    try {
      const manifest = JSON.parse(
        await readFile(resolve(directory, "package.json"), "utf8"),
      );
      if (typeof manifest.name === "string" && manifest.name.startsWith("@onda-lang/")) {
        result.push(directory);
      }
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  return result.sort();
}

async function synchronizeJson(path, versionPaths) {
  const input = await readFile(path, "utf8");
  const value = JSON.parse(input);
  let changed = false;
  for (const segments of versionPaths) {
    let target = value;
    for (const segment of segments.slice(0, -1)) {
      target = target?.[segment];
    }
    const field = segments.at(-1);
    const label = segments.map((segment) => segment || "<root>").join(".");
    if (!target || !(field in target)) {
      mismatches.push(`${relativePath(path)} is missing ${label}`);
      continue;
    }
    if (target[field] === workspaceVersion) continue;
    if (checkOnly) {
      mismatches.push(
        `${relativePath(path)} ${label} is ${String(target[field])}`,
      );
    } else {
      target[field] = workspaceVersion;
      changed = true;
    }
  }
  if (changed) await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

async function synchronizeCargoLock() {
  const lockPath = resolve(repoRoot, "Cargo.lock");
  let lock = await readFile(lockPath, "utf8");
  let changed = false;
  const cratesRoot = resolve(repoRoot, "crates");
  const entries = await readdir(cratesRoot, { withFileTypes: true });
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const manifestPath = resolve(cratesRoot, entry.name, "Cargo.toml");
    let manifest;
    try {
      manifest = await readFile(manifestPath, "utf8");
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    if (!/^version\.workspace = true$/m.test(manifest)) continue;
    const name = manifest.match(/^name = "([^"]+)"$/m)?.[1];
    if (!name) throw new Error(`failed to read package name from ${relativePath(manifestPath)}`);
    const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const pattern = new RegExp(
      `(\\[\\[package\\]\\]\\nname = "${escapedName}"\\nversion = ")([^"]+)(")`,
    );
    const lockedVersion = lock.match(pattern)?.[2];
    if (lockedVersion !== workspaceVersion) {
      if (checkOnly || lockedVersion === undefined) {
        mismatches.push(
          `Cargo.lock package ${name} is ${String(lockedVersion)}`,
        );
      } else {
        lock = lock.replace(pattern, `$1${workspaceVersion}$3`);
        changed = true;
      }
    }
  }
  if (changed) await writeFile(lockPath, lock);
}

function relativePath(path) {
  return path.slice(repoRoot.length + 1);
}
