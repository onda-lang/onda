import assert from "node:assert/strict";
import test from "node:test";
import { diagnosticCount, forEachDiagnostic } from "@codemirror/lint";
import { lineNumberMarkers } from "@codemirror/view";

import {
  OndaProjectEditor,
  colonIndentText,
  editorGuttersAreFixed,
  editorViewportMargins,
  ondaSemanticTokenColors,
  preferredCaretScrollLeft,
  semanticTokenClassNames,
  validProjectPath,
} from "./editor.js";

test("diagnostics mark line numbers without adding a gutter", () => {
  const previousWindow = globalThis.window;
  globalThis.window = { innerWidth: 1024 };
  try {
    const editor = {
      active: "other.onda",
      states: new Map(),
      diagnostics: new Map(),
      scheduleSemanticTokens() {},
      renderFiles() {},
    };
    const path = "main.onda";
    editor.states.set(path, OndaProjectEditor.prototype.createState.call(editor, path, "bad\n"));
    const markers = () => {
      const found = [];
      for (const set of editor.states.get(path).facet(lineNumberMarkers)) {
        set.between(0, 3, (_from, _to, marker) => found.push(marker));
      }
      return found;
    };
    const range = {
      start: { line: 0, character: 0 },
      end: { line: 0, character: 1 },
    };
    OndaProjectEditor.prototype.setDocumentDiagnostics.call(editor, path, [
      { range, severity: 2, message: "warning" },
      { range, severity: 1, message: "error" },
    ]);
    assert.equal(diagnosticCount(editor.states.get(path)), 1);
    assert.equal(markers()[0].elementClass, "cm-onda-diagnostic-error");
    assert.match(markers()[0].tooltip, /Line 1\nwarning: warning/);
    assert.match(markers()[0].tooltip, /error: error/);

    OndaProjectEditor.prototype.setDocumentDiagnostics.call(editor, path, []);
    assert.equal(diagnosticCount(editor.states.get(path)), 0);
    assert.deepEqual(markers(), []);

    OndaProjectEditor.prototype.setDocumentDiagnostics.call(editor, path, [{
      range: {
        start: { line: 0, character: 3 },
        end: { line: 0, character: 4 },
      },
      severity: 1,
      message: "end-of-line error",
    }]);
    const ranges = [];
    forEachDiagnostic(editor.states.get(path), (_diagnostic, from, to) => {
      ranges.push([from, to]);
    });
    assert.deepEqual(ranges, [[2, 3]]);

    editor.states.set(path, editor.states.get(path).update({
      changes: { from: 0, insert: "\n" },
    }).state);
    assert.equal(markers()[0].number, 2);
    assert.match(markers()[0].tooltip, /^Line 2\n/);
  } finally {
    globalThis.window = previousWindow;
  }
});

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
