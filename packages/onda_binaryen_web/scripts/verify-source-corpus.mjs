import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import {
  dirname,
  extname,
  join,
  relative,
  resolve,
  sep,
} from "node:path";
import { fileURLToPath } from "node:url";

import {
  SUPPORTED_MIR_SCHEMA_VERSION,
  compileTrustedMir as compileMir,
} from "../src/index.js";
import { resolveOndaCli } from "./onda-cli.mjs";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const corpusRoots = [
  join(repoDir, "examples"),
  join(packageDir, "test/fixtures"),
];

// Intentional negative .onda fixtures belong here with a reason. Keeping the
// exclusions explicit prevents a newly added source from silently escaping the
// continuous corpus gate.
const excludedSources = new Map([]);

const sampleRate = "48000";
const blockSize = "128";
const maxCommandOutput = 16 * 1024 * 1024;

const discovered = corpusRoots
  .flatMap((root) => ondaSourcesBelow(root))
  .sort((left, right) => repoPath(left).localeCompare(repoPath(right), "en"));
const discoveredNames = new Set(discovered.map(repoPath));
const staleExclusions = [...excludedSources.keys()].filter(
  (source) => !discoveredNames.has(source),
);
if (staleExclusions.length > 0) {
  throw new Error(
    `source-corpus exclusions do not exist:\n${staleExclusions
      .map((source) => `- ${source}`)
      .join("\n")}`,
  );
}

const sources = discovered.filter(
  (source) => !excludedSources.has(repoPath(source)),
);
if (sources.length === 0) {
  throw new Error("Onda source corpus is empty");
}

const ondaCli = resolveOndaCli(repoDir);
const temporary = mkdtempSync(join(tmpdir(), "onda-binaryen-corpus-"));
const failures = [];
let validModules = 0;
let wasmBytes = 0;

try {
  for (const [index, source] of sources.entries()) {
    const name = repoPath(source);
    const mirPath = join(
      temporary,
      `${String(index).padStart(4, "0")}.mir.msgpack`,
    );
    const prefix = `[${index + 1}/${sources.length}] ${name}`;

    try {
      compileSourceToMir(ondaCli, source, mirPath);
      const artifact = compileMir(readFileSync(mirPath));
      if (
        artifact.metadata.mir_schema_version !== SUPPORTED_MIR_SCHEMA_VERSION
      ) {
        throw new Error(
          `Binaryen metadata reports MIR schema ${String(
            artifact.metadata.mir_schema_version,
          )}`,
        );
      }
      if (!WebAssembly.validate(artifact.wasm)) {
        throw new Error("Binaryen output is not a valid WebAssembly module");
      }

      validModules += 1;
      wasmBytes += artifact.wasm.byteLength;
      process.stdout.write(
        `${prefix}: ${artifact.wasm.byteLength.toLocaleString("en-US")} Wasm bytes\n`,
      );
    } catch (error) {
      failures.push({
        source: name,
        message: errorMessage(error),
      });
      process.stderr.write(`${prefix}: FAILED\n`);
    }
  }
} finally {
  rmSync(temporary, { recursive: true, force: true });
}

const excludedCount = discovered.length - sources.length;
const exampleCount = sources.filter((source) =>
  repoPath(source).startsWith("examples/")
).length;
const fixtureCount = sources.length - exampleCount;
if (failures.length > 0) {
  process.stderr.write(
    `\nOnda source corpus failed: ${failures.length} failure(s), ` +
      `${validModules}/${sources.length} valid Wasm module(s), ` +
      `${exampleCount} example(s), ${fixtureCount} fixture(s), ` +
      `${excludedCount} explicit exclusion(s).\n`,
  );
  for (const failure of failures) {
    process.stderr.write(`\n--- ${failure.source} ---\n${failure.message}\n`);
  }
  process.exitCode = 1;
} else {
  process.stdout.write(
    `\nVerified full Onda source corpus: ${validModules}/${sources.length} ` +
      `schema-${SUPPORTED_MIR_SCHEMA_VERSION} MIR programs compiled to valid WebAssembly ` +
      `(${wasmBytes.toLocaleString("en-US")} total Wasm bytes, ` +
      `${exampleCount} example(s), ${fixtureCount} fixture(s), ` +
      `${excludedCount} explicit exclusion(s)).\n`,
  );
}

function ondaSourcesBelow(directory) {
  const sources = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      sources.push(...ondaSourcesBelow(path));
    } else if (entry.isFile() && extname(entry.name) === ".onda") {
      sources.push(path);
    }
  }
  return sources;
}

function repoPath(path) {
  return relative(repoDir, path).split(sep).join("/");
}

function compileSourceToMir(ondaCli, source, mirPath) {
  const result = spawnSync(
    ondaCli,
    [
      "compile",
      source,
      "--emit",
      "mir-messagepack",
      "--output",
      mirPath,
      "--sample-rate",
      sampleRate,
      "--block-size",
      blockSize,
    ],
    {
      cwd: repoDir,
      encoding: "utf8",
      maxBuffer: maxCommandOutput,
    },
  );
  // The managed command proxy used by local automation may attach EPERM after
  // a successful child exit while preserving status 0 and the complete
  // output. A real launch failure has a null/nonzero status.
  if (result.status !== 0 || result.signal) {
    throw new Error(
      `source-to-MIR command failed:\n${commandFailure(result)}`,
    );
  }
}

function commandFailure(result) {
  const details = [];
  if (result.error) details.push(result.error.stack ?? result.error.message);
  if (result.signal) details.push(`terminated by signal ${result.signal}`);
  if (result.status !== null) details.push(`exit status ${result.status}`);
  if (result.stdout?.trim()) details.push(`stdout:\n${result.stdout.trim()}`);
  if (result.stderr?.trim()) details.push(`stderr:\n${result.stderr.trim()}`);
  return details.join("\n") || "command failed without diagnostic output";
}

function errorMessage(error) {
  return error instanceof Error
    ? error.stack ?? error.message
    : String(error);
}
