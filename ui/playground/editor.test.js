import assert from "node:assert/strict";
import test from "node:test";

import {
  colonIndentText,
  editorGuttersAreFixed,
  editorViewportMargins,
  ondaSemanticTokenColors,
  preferredCaretScrollLeft,
  semanticTokenClassNames,
  validProjectPath,
} from "./editor.js";

test("event and delegate semantic tokens use callable highlighting", () => {
  assert.deepEqual(Object.keys(ondaSemanticTokenColors), [
    "enumMember",
    "variable",
    "port",
    "parameter",
    "function",
    "type",
    "namespace",
    "state",
    "keyword",
    "number",
    "event",
    "delegate",
  ]);
  assert.equal(ondaSemanticTokenColors.event, ondaSemanticTokenColors.function);
  assert.equal(ondaSemanticTokenColors.delegate, ondaSemanticTokenColors.function);
});

test("semantic token classes preserve declaration modifiers", () => {
  assert.equal(
    semanticTokenClassNames("event", 1, ["declaration"]),
    "cm-onda-semantic-event cm-onda-semantic-mod-declaration",
  );
  assert.equal(
    semanticTokenClassNames("event", 0, ["declaration"]),
    "cm-onda-semantic-event",
  );
});

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

test("only desktop editors keep line-number gutters fixed", () => {
  assert.equal(editorGuttersAreFixed(390, false), false);
  assert.equal(editorGuttersAreFixed(1024, true), false);
  assert.equal(editorGuttersAreFixed(1024, false), true);
});

test("compact editors return to the line start only when the caret still fits", () => {
  const geometry = {
    editor: { left: 0, right: 360 },
    viewport: { offsetLeft: 0, width: 360 },
    scrollLeft: 30,
  };

  assert.equal(preferredCaretScrollLeft({
    ...geometry,
    caret: { left: 55, right: 56 },
    preserveScroll: false,
  }), 0);
  assert.equal(preferredCaretScrollLeft({
    ...geometry,
    caret: { left: 330, right: 331 },
    preserveScroll: false,
  }), 30);
  assert.equal(preferredCaretScrollLeft({
    ...geometry,
    caret: { left: 55, right: 56 },
    preserveScroll: true,
  }), 30);
});
