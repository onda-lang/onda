import { readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checkOnly = process.argv.slice(2).includes("--check");
const cargoToml = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
const formatVersions = JSON.parse(
  await readFile(resolve(repoRoot, "format-versions.json"), "utf8"),
);
const workspaceVersion = cargoToml.match(
  /\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/,
)?.[1];

if (!workspaceVersion) {
  throw new Error("failed to read [workspace.package].version from Cargo.toml");
}

for (const [name, version] of Object.entries(formatVersions)) {
  if (!Number.isInteger(version) || version < 1) {
    throw new Error(`format-versions.json ${name} must be a positive integer`);
  }
}

const mismatches = [];
await synchronizeFormatVersions();
const packageDirectories = await ondaPackageDirectories();
for (const packageDirectory of packageDirectories) {
  const manifestPath = resolve(packageDirectory, "package.json");
  await synchronizeJson(manifestPath, [["version"]]);
  await synchronizeInternalNpmDependencies(manifestPath);
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

await synchronizeRootNpmLock(packageDirectories);

await synchronizeCargoLock();

if (mismatches.length > 0) {
  throw new Error(
    `Onda version ${workspaceVersion} is not synchronized:\n${mismatches.map((entry) => `- ${entry}`).join("\n")}`,
  );
}

const action = checkOnly ? "Verified" : "Synchronized";
process.stdout.write(
  `${action} Onda version ${workspaceVersion} and format versions across Cargo, npm, TypeScript, C, and example sources\n`,
);

async function synchronizeFormatVersions() {
  const v = formatVersions;
  await synchronizeText("crates/onda_mir/src/lib.rs", [
    constant(/pub const MIR_SCHEMA_VERSION: u32 = \d+;/, `pub const MIR_SCHEMA_VERSION: u32 = ${v.mir_schema};`),
  ]);
  await synchronizeText("crates/onda_project/src/buffer.rs", [
    constant(/pub const ONDA_BUFFER_FORMAT_VERSION: u32 = \d+;/, `pub const ONDA_BUFFER_FORMAT_VERSION: u32 = ${v.buffer_asset};`),
  ]);
  await synchronizeText("crates/onda_project/src/image.rs", [
    constant(/pub const ONDA_PROJECT_IMAGE_FORMAT_VERSION: u32 = \d+;/, `pub const ONDA_PROJECT_IMAGE_FORMAT_VERSION: u32 = ${v.project_image};`),
  ]);
  await synchronizeText("crates/onda_processor_abi/src/lib.rs", [
    constant(/pub const PROCESSOR_ARTIFACT_FORMAT_VERSION: u32 = \d+;/, `pub const PROCESSOR_ARTIFACT_FORMAT_VERSION: u32 = ${v.processor_artifact};`),
    constant(/pub const PROCESSOR_ABI_VERSION: u32 = \d+;/, `pub const PROCESSOR_ABI_VERSION: u32 = ${v.processor_abi};`),
    constant(/pub const PROCESSOR_SNAPSHOT_FORMAT_VERSION: u32 = \d+;/, `pub const PROCESSOR_SNAPSHOT_FORMAT_VERSION: u32 = ${v.processor_snapshot};`),
  ]);
  await synchronizeText("packages/onda_processor_abi/src/index.js", [
    constant(/export const PROCESSOR_ARTIFACT_FORMAT_VERSION = \d+;/, `export const PROCESSOR_ARTIFACT_FORMAT_VERSION = ${v.processor_artifact};`),
    constant(/export const PROCESSOR_ABI_VERSION = \d+;/, `export const PROCESSOR_ABI_VERSION = ${v.processor_abi};`),
    constant(/export const PROCESSOR_SNAPSHOT_FORMAT_VERSION = \d+;/, `export const PROCESSOR_SNAPSHOT_FORMAT_VERSION = ${v.processor_snapshot};`),
  ]);
  await synchronizeText("packages/onda_processor_abi/src/index.d.ts", [
    constant(/export const PROCESSOR_ARTIFACT_FORMAT_VERSION: \d+;/, `export const PROCESSOR_ARTIFACT_FORMAT_VERSION: ${v.processor_artifact};`),
    constant(/export const PROCESSOR_ABI_VERSION: \d+;/, `export const PROCESSOR_ABI_VERSION: ${v.processor_abi};`),
    constant(/export const PROCESSOR_SNAPSHOT_FORMAT_VERSION: \d+;/, `export const PROCESSOR_SNAPSHOT_FORMAT_VERSION: ${v.processor_snapshot};`),
    constant(/  format_version: \d+;/, `  format_version: ${v.processor_artifact};`),
    constant(/  abi_version: \d+;/, `  abi_version: ${v.processor_abi};`),
    constant(/  snapshot_format_version: \d+;/, `  snapshot_format_version: ${v.processor_snapshot};`),
  ]);
  await synchronizeText("packages/onda_binaryen_web/src/constants.js", [
    constant(/export const SUPPORTED_MIR_SCHEMA_VERSION = \d+;/, `export const SUPPORTED_MIR_SCHEMA_VERSION = ${v.mir_schema};`),
  ]);
  await synchronizeText("include/onda_processor_abi.h", [
    constant(/#define ONDA_PROCESSOR_ABI_VERSION \d+u/, `#define ONDA_PROCESSOR_ABI_VERSION ${v.processor_abi}u`),
  ]);
  await synchronizeText("examples/native/raw_processor_object/generate_config.py", [
    constant(/PROCESSOR_ARTIFACT_FORMAT_VERSION = \d+/, `PROCESSOR_ARTIFACT_FORMAT_VERSION = ${v.processor_artifact}`),
    constant(/PROCESSOR_ABI_VERSION = \d+/, `PROCESSOR_ABI_VERSION = ${v.processor_abi}`),
  ]);
}

function constant(pattern, replacement) {
  return { pattern, replacement };
}

async function synchronizeText(path, replacements) {
  const absolutePath = resolve(repoRoot, path);
  const input = await readFile(absolutePath, "utf8");
  let output = input;
  for (const { pattern, replacement } of replacements) {
    const matches = output.match(new RegExp(pattern.source, "g")) ?? [];
    if (matches.length !== 1) {
      throw new Error(`${path} must contain exactly one ${pattern}`);
    }
    output = output.replace(pattern, replacement);
  }
  if (output === input) return;
  if (checkOnly) {
    mismatches.push(`${path} has stale format versions`);
  } else {
    await writeFile(absolutePath, output);
  }
}

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

async function synchronizeInternalNpmDependencies(path) {
  const value = JSON.parse(await readFile(path, "utf8"));
  let changed = false;
  for (const field of [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
  ]) {
    for (const [name, version] of Object.entries(value[field] ?? {})) {
      if (!name.startsWith("@onda-lang/") || version === workspaceVersion) continue;
      if (checkOnly) {
        mismatches.push(`${relativePath(path)} ${field}.${name} is ${String(version)}`);
      } else {
        value[field][name] = workspaceVersion;
        changed = true;
      }
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

async function synchronizeRootNpmLock(packageDirectories) {
  const lockPath = resolve(repoRoot, "package-lock.json");
  let input;
  try {
    input = await readFile(lockPath, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw error;
  }
  const value = JSON.parse(input);
  let changed = false;
  for (const packageDirectory of packageDirectories) {
    const key = relativePath(packageDirectory).replaceAll("\\", "/");
    const entry = value.packages?.[key];
    if (!entry || !("version" in entry)) {
      mismatches.push(`package-lock.json is missing packages.${key}.version`);
      continue;
    }
    if (entry.version !== workspaceVersion) {
      if (checkOnly) {
        mismatches.push(`package-lock.json packages.${key}.version is ${String(entry.version)}`);
      } else {
        entry.version = workspaceVersion;
        changed = true;
      }
    }
    for (const field of [
      "dependencies",
      "devDependencies",
      "optionalDependencies",
      "peerDependencies",
    ]) {
      for (const [name, version] of Object.entries(entry[field] ?? {})) {
        if (!name.startsWith("@onda-lang/") || version === workspaceVersion) continue;
        if (checkOnly) {
          mismatches.push(
            `package-lock.json packages.${key}.${field}.${name} is ${String(version)}`,
          );
        } else {
          entry[field][name] = workspaceVersion;
          changed = true;
        }
      }
    }
  }
  if (changed) await writeFile(lockPath, `${JSON.stringify(value, null, 2)}\n`);
}

function relativePath(path) {
  return path.slice(repoRoot.length + 1);
}
