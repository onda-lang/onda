import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import test from "node:test";

import { main } from "../bin/onda-wasm.js";
import { MAX_BLOCK_SIZE } from "../src/config.js";

test("onda-wasm rejects block sizes outside the MIR index boundary", async () => {
  await assert.rejects(
    main([
      "compile",
      "main.onda",
      "--block-size",
      String(MAX_BLOCK_SIZE + 1),
    ]),
    new RegExp(`--block-size requires an integer from 1 to ${MAX_BLOCK_SIZE}`),
  );
});

test("onda-wasm writes the reusable artifact pair", async () => {
  const temporary = await mkdtemp(resolve(tmpdir(), "onda-wasm-cli-"));
  try {
    const input = resolve(temporary, "main.onda");
    const output = resolve(temporary, "dist/processor.wasm");
    const watOutput = resolve(temporary, "dist/processor.wat");
    await writeFile(input, "sample:\n  out1 = 0.25\n");

    assert.equal(await main([
      "compile",
      input,
      "--root",
      temporary,
      "--output",
      output,
      "--wat-out",
      watOutput,
    ]), 0);

    const wasm = await readFile(output);
    const metadata = JSON.parse(
      await readFile(resolve(temporary, "dist/processor.onda.json"), "utf8"),
    );
    const wat = await readFile(watOutput, "utf8");
    assert.equal(WebAssembly.validate(wasm), true);
    assert.equal(metadata.format, "onda-processor");
    assert.match(metadata.integrity.wasm, /^[0-9a-f]{64}$/);
    assert.match(wat, /\(module/);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
