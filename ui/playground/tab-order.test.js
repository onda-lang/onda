import assert from "node:assert/strict";
import test from "node:test";

import { reorderMap } from "./tab-order.js";

test("reorders known tab keys and preserves their values", () => {
  const original = new Map([
    ["main.onda", { id: 1 }],
    ["filter.onda", { id: 2 }],
    ["osc.onda", { id: 3 }],
  ]);

  const reordered = reorderMap(original, ["osc.onda", "main.onda", "filter.onda"]);

  assert.deepEqual([...reordered.keys()], ["osc.onda", "main.onda", "filter.onda"]);
  assert.equal(reordered.get("main.onda"), original.get("main.onda"));
});

test("ignores unknown and duplicate keys without dropping tabs", () => {
  const original = new Map([
    ["main.onda", 1],
    ["filter.onda", 2],
    ["osc.onda", 3],
  ]);

  const reordered = reorderMap(original, ["filter.onda", "missing.onda", "filter.onda"]);

  assert.deepEqual([...reordered.keys()], ["filter.onda", "main.onda", "osc.onda"]);
});
