import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sourcePath = resolve(repoRoot, "docs/web-api.md");
const packageDirectories = [
  "onda_processor_abi",
  "onda_binaryen_web",
  "onda_wasm_compiler",
  "onda_webaudio",
];

const source = await readFile(sourcePath, "utf8");
const withoutFrontMatter = source.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n+/, "");
const packaged = [
  "<!-- Generated from docs/web-api.md; do not edit this packaged copy. -->",
  "",
  withoutFrontMatter,
].join("\n");

await Promise.all(packageDirectories.map((directory) => writeFile(
  resolve(repoRoot, "packages", directory, "api.md"),
  packaged,
)));

process.stdout.write(`Staged Web API reference in ${packageDirectories.length} packages\n`);
