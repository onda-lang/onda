import assert from "node:assert/strict";
import test from "node:test";

import { compilationKey } from "./compile-cache.js";

const options = { sampleRate: 48_000, blockSize: 128 };

test("compilation identity ignores project source insertion order", () => {
  const left = {
    entry: "main.onda",
    sources: { "main.onda": "include lib", "lib.onda": "const x = 1" },
  };
  const right = {
    entry: "main.onda",
    sources: { "lib.onda": "const x = 1", "main.onda": "include lib" },
  };

  assert.equal(compilationKey(left, options), compilationKey(right, options));
});

test("compilation identity changes with source, entry, or compile options", () => {
  const project = { entry: "main.onda", sources: { "main.onda": "sample:\n  out1 = 0.0" } };
  const key = compilationKey(project, options);

  assert.notEqual(
    key,
    compilationKey(
      { ...project, sources: { "main.onda": "sample:\n  out1 = 1.0" } },
      options,
    ),
  );
  assert.notEqual(key, compilationKey({ ...project, entry: "other.onda" }, options));
  assert.notEqual(key, compilationKey(project, { ...options, blockSize: 256 }));
  assert.notEqual(key, compilationKey(project, { ...options, sampleRate: 44_100 }));
});
