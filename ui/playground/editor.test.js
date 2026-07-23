import assert from "node:assert/strict";
import test from "node:test";

import { colonIndentText } from "./editor.js";

test("adds two spaces after an Onda block colon", () => {
  assert.equal(colonIndentText("sample:"), "\n  ");
  assert.equal(colonIndentText("  if enabled:"), "\n    ");
  assert.equal(colonIndentText("  init:  # state setup"), "\n    ");
});

test("leaves ordinary lines to CodeMirror's normal Enter handling", () => {
  assert.equal(colonIndentText("  gain = 0.5"), null);
  assert.equal(colonIndentText("# note:"), null);
  assert.equal(colonIndentText("  # note:"), null);
});
