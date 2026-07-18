import assert from "node:assert/strict";
import test from "node:test";

import { completionIconMasks, completionType } from "./completions.js";

test("maps every Onda LSP completion kind to a font-independent icon", () => {
  const expected = new Map([
    [2, "method"],
    [3, "function"],
    [4, "constructor"],
    [5, "field"],
    [6, "variable"],
    [9, "namespace"],
    [10, "property"],
    [14, "keyword"],
    [17, "file"],
    [21, "constant"],
    [22, "struct"],
    [23, "event"],
    [25, "type"],
  ]);

  for (const [kind, type] of expected) {
    assert.equal(completionType(kind), type);
    assert.match(completionIconMasks[type], /^url\("data:image\/svg\+xml,/);
  }
  assert.equal(completionType(999), "text");
  assert.ok(completionIconMasks.text);
});
