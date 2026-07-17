import { webcrypto } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

import {
  compileTrustedMir,
  createProcessorArtifactFiles,
} from "../../../packages/onda_binaryen_web/src/index.js";

if (!globalThis.crypto) {
  Object.defineProperty(globalThis, "crypto", { value: webcrypto });
}

const [mirInput, outputDirectory] = process.argv.slice(2);
if (!mirInput || !outputDirectory) {
  throw new Error("usage: node build-artifact.mjs <input.mir.msgpack> <output-directory>");
}

const mir = await readFile(resolve(mirInput));
const artifact = compileTrustedMir(mir, {
  optimize: true,
  optimizeLevel: 4,
  shrinkLevel: 0,
  fastMath: false,
  simd: true,
});
const files = await createProcessorArtifactFiles(artifact, {
  baseName: "sample-player",
});

await mkdir(resolve(outputDirectory), { recursive: true });
await Promise.all([
  writeFile(resolve(outputDirectory, files.wasm.name), files.wasm.bytes),
  writeFile(resolve(outputDirectory, files.metadata.name), files.metadata.text),
]);

process.stdout.write(
  `Wrote ${files.wasm.name} (${files.wasm.bytes.byteLength} bytes) and ${files.metadata.name}\n`,
);
