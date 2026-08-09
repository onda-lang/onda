import assert from "node:assert/strict";
import test from "node:test";

import {
  colonIndentText,
  editorViewportMargins,
  validProjectPath,
} from "./editor.js";

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

test("project paths require the canonical Unicode spelling", () => {
  assert.equal(validProjectPath("\u{e9}.onda"), true);
  assert.equal(validProjectPath("e\u{301}.onda"), false);
});

test("mobile viewport margins keep the caret beyond the sticky gutter", () => {
  const editor = { left: 12, right: 380, top: 100, bottom: 500 };
  const viewport = {
    offsetLeft: 72,
    offsetTop: 0,
    width: 320,
    height: 640,
  };

  assert.deepEqual(editorViewportMargins(editor, viewport, 44), {
    top: 0,
    bottom: 0,
    left: 120,
    right: 4,
  });
});
