import assert from "node:assert/strict";
import test from "node:test";

import {
  centeredCaretScrollLeft,
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

test("visual viewport margins are independent of CodeMirror's gutter margin", () => {
  const editor = { left: 12, right: 380, top: 100, bottom: 500 };
  const viewport = {
    offsetLeft: 72,
    offsetTop: 0,
    width: 320,
    height: 640,
  };

  assert.deepEqual(editorViewportMargins(editor, viewport), {
    top: 0,
    bottom: 0,
  });
});

test("typing centers the caret in the visible code area", () => {
  const geometry = {
    editor: { left: 12, right: 380 },
    gutter: { right: 56 },
    viewport: { offsetLeft: 72, width: 320 },
    scrollLeft: 200,
    scrollWidth: 900,
    clientWidth: 368,
  };

  assert.equal(centeredCaretScrollLeft({
    ...geometry,
    caret: { left: 319, right: 321 },
  }), 294);
  assert.equal(centeredCaretScrollLeft({
    ...geometry,
    caret: { left: 79, right: 81 },
  }), 54);
});

test("caret centering respects both horizontal scroll limits", () => {
  const geometry = {
    editor: { left: 0, right: 360 },
    gutter: { right: 48 },
    viewport: { offsetLeft: 0, width: 360 },
    scrollWidth: 600,
    clientWidth: 360,
  };

  assert.equal(centeredCaretScrollLeft({
    ...geometry,
    caret: { left: 55, right: 57 },
    scrollLeft: 0,
  }), 0);
  assert.equal(centeredCaretScrollLeft({
    ...geometry,
    caret: { left: 500, right: 502 },
    scrollLeft: 200,
  }), 240);
});
