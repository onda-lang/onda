import assert from "node:assert/strict";
import test from "node:test";

import { decodeMirMessagePack } from "../src/messagepack.js";

test("decodes maps without exposing prototype mutation", () => {
  const key = new TextEncoder().encode("__proto__");
  const nestedKey = new TextEncoder().encode("polluted");
  const bytes = Uint8Array.from([
    0x81,
    0xa0 | key.length,
    ...key,
    0x81,
    0xa0 | nestedKey.length,
    ...nestedKey,
    0xc3,
  ]);

  const decoded = decodeMirMessagePack(bytes);
  assert.equal(Object.getPrototypeOf(decoded), null);
  assert.equal(decoded.__proto__.polluted, true);
  assert.equal({}.polluted, undefined);
});

test("rejects truncated collections before allocating their declared size", () => {
  assert.throws(
    () => decodeMirMessagePack(Uint8Array.from([0xdd, 0x7f, 0xff, 0xff, 0xff])),
    /truncated MessagePack MIR array/,
  );
});
