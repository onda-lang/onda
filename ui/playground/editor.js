// Shared CodeMirror editor used by every browser-playground host.
import { autocompletion } from "@codemirror/autocomplete";
import { indentWithTab } from "@codemirror/commands";
import {
  HighlightStyle,
  StreamLanguage,
  indentUnit,
  syntaxHighlighting,
} from "@codemirror/language";
import { lintGutter, setDiagnostics } from "@codemirror/lint";
import { EditorState, Prec, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  hoverTooltip,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { minimalSetup } from "codemirror";
import { tags } from "@lezer/highlight";

import { projectUriToPath } from "./lsp-client.js";
import { completionIconMasks, completionType } from "./completions.js";
import { reorderMap } from "./tab-order.js";

const sectionWords = new Set([
  "ins", "inputs", "outs", "outputs", "params", "kins", "kouts",
  "buffers", "events", "init", "block", "sample", "graph",
]);
const declarationWords = new Set([
  "const", "def", "event", "proc", "processor", "struct", "namespace",
]);
const nameFollowing = new Set([
  "def", "event", "proc", "processor", "struct", "namespace",
]);
const keywordWords = new Set([
  "if", "elif", "else", "for", "in", "while", "loop", "break",
  "continue", "return", "assert", "import", "include", "use", "pub",
  "as", "pin",
]);
const typeWords = new Set(["f32", "f64", "i32", "i64", "bool", "buffer"]);
const constantWords = new Set([
  "true", "false", "PI", "TWO_PI", "TWOPI", "SR", "SAMPLE_RATE",
  "SAMPLERATE", "HOST_SR", "HOST_SAMPLE_RATE", "HOST_SAMPLERATE",
  "BS", "BLOCK_SIZE", "BLOCKSIZE",
]);

const ondaLanguage = StreamLanguage.define({
  name: "onda",
  startState: () => ({ lineStart: true, expectedName: false }),
  blankLine: (state) => {
    state.lineStart = true;
    state.expectedName = false;
  },
  tokenTable: {
    section: tags.heading,
    declaration: tags.definitionKeyword,
    function: tags.function(tags.variableName),
    constant: tags.constant(tags.name),
  },
  token(stream, state) {
    if (stream.sol()) state.lineStart = true;
    if (stream.eatSpace()) return null;
    if (stream.peek() === "#") {
      stream.skipToEnd();
      return "comment";
    }
    if (stream.peek() === '"') {
      stream.next();
      let escaped = false;
      while (!stream.eol()) {
        const character = stream.next();
        if (character === '"' && !escaped) break;
        escaped = character === "\\" && !escaped;
        if (character !== "\\") escaped = false;
      }
      state.lineStart = false;
      return "string";
    }
    if (stream.match(/^\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/)) {
      state.lineStart = false;
      return "number";
    }
    if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
      const word = stream.current();
      let token = null;
      if (state.lineStart && sectionWords.has(word)) token = "section";
      else if (declarationWords.has(word)) token = "declaration";
      else if (state.expectedName) token = "function";
      else if (keywordWords.has(word)) token = "keyword";
      else if (typeWords.has(word)) token = "typeName";
      else if (constantWords.has(word)) token = "constant";
      else if (/^(?:in|out|kout|param|kin|buf)\d+$/.test(word)) token = "constant";
      else if (stream.match(/^\s*\(/, false)) token = "function";
      state.expectedName = nameFollowing.has(word);
      state.lineStart = false;
      return token;
    }
    if (stream.match(/^(?:>>\[[^\]\n]+\]|<<\[[^\]\n]+\]|\.\.=|\.\.|>>|<<|==|!=|<=|>=|&&|\|\||::|@(?:sample|block)|[+\-*/%=&|^~!<>])/)) {
      state.lineStart = false;
      return "operator";
    }
    stream.next();
    state.lineStart = false;
    return null;
  },
});

const ondaHighlightStyle = HighlightStyle.define([
  { tag: [tags.heading, tags.definitionKeyword], color: "var(--syntax-section)", fontWeight: "650" },
  { tag: tags.keyword, color: "var(--syntax-keyword)" },
  { tag: tags.typeName, color: "var(--syntax-type)" },
  { tag: tags.number, color: "var(--syntax-number)" },
  { tag: tags.string, color: "var(--syntax-string)" },
  { tag: tags.constant(tags.name), color: "var(--syntax-constant)" },
  { tag: tags.function(tags.variableName), color: "var(--syntax-function)" },
  { tag: tags.operator, color: "var(--syntax-operator)" },
  { tag: tags.comment, color: "var(--syntax-comment)", fontStyle: "italic" },
]);

const setSemanticTokens = StateEffect.define();
const semanticTokenField = StateField.define({
  create: () => Decoration.none,
  update(tokens, transaction) {
    tokens = tokens.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(setSemanticTokens)) tokens = effect.value;
    }
    return tokens;
  },
  provide: (field) => EditorView.decorations.from(field),
});

function visibleEditorMargins(view) {
  const viewport = window.visualViewport;
  if (!viewport) return null;
  const editor = view.scrollDOM.getBoundingClientRect();
  const viewportTop = viewport.offsetTop;
  const viewportBottom = viewportTop + viewport.height;
  const viewportLeft = viewport.offsetLeft;
  const viewportRight = viewportLeft + viewport.width;
  const gutterWidth =
    view.dom.querySelector(".cm-gutters")?.getBoundingClientRect().width ?? 0;
  const padding = 16;
  return {
    top: Math.max(0, viewportTop - editor.top + padding),
    bottom: Math.max(0, editor.bottom - viewportBottom + padding),
    left: Math.max(0, viewportLeft - editor.left) + gutterWidth + padding,
    right: Math.max(0, editor.right - viewportRight + padding),
  };
}

function hasCompactEditingViewport() {
  const viewportWidth = window.visualViewport?.width ?? window.innerWidth;
  return viewportWidth <= 720 || window.matchMedia?.("(pointer: coarse)").matches;
}

function hiddenCaretAxes(view, leftComfort = 0) {
  const caret = view.coordsAtPos(view.state.selection.main.head);
  if (!caret) return { horizontal: true, vertical: true };
  const editor = view.scrollDOM.getBoundingClientRect();
  const viewport = window.visualViewport;
  const viewportLeft = viewport?.offsetLeft ?? 0;
  const viewportTop = viewport?.offsetTop ?? 0;
  const viewportRight = viewportLeft + (viewport?.width ?? window.innerWidth);
  const viewportBottom = viewportTop + (viewport?.height ?? window.innerHeight);
  const gutterWidth =
    view.dom.querySelector(".cm-gutters")?.getBoundingClientRect().width ?? 0;
  const padding = 16;
  const left = Math.max(editor.left, viewportLeft) + gutterWidth + padding;
  const right = Math.min(editor.right, viewportRight) - padding;
  const top = Math.max(editor.top, viewportTop) + padding;
  const bottom = Math.min(editor.bottom, viewportBottom) - padding;
  const comfortableLeft = left + Math.max(0, right - left) * leftComfort;
  return {
    horizontal: caret.right < comfortableLeft || caret.left > right,
    vertical: caret.bottom < top || caret.top > bottom,
  };
}

export function colonIndentText(lineBeforeCursor) {
  const indentation = lineBeforeCursor.match(/^[ \t]*/)?.[0] ?? "";
  const code = lineBeforeCursor.replace(/(?:^\s*|\s+)#.*$/, "").trimEnd();
  return code.endsWith(":") ? `\n${indentation}  ` : null;
}

function insertIndentedNewline({ state, dispatch }) {
  if (state.readOnly) return false;
  const selection = state.selection.main;
  if (!selection.empty) return false;
  const line = state.doc.lineAt(selection.head);
  const insertion = colonIndentText(state.sliceDoc(line.from, selection.head));
  if (insertion === null) return false;
  dispatch(state.update(
    state.replaceSelection(insertion),
    { scrollIntoView: true, userEvent: "input" },
  ));
  return true;
}

const ondaEditorTheme = EditorView.theme({
  "&": {
    height: "100%",
    color: "var(--code-ink)",
    backgroundColor: "var(--code-bg)",
    font: ".84rem/1.55 var(--mono)",
  },
  ".cm-scroller": { overflow: "auto", fontFamily: "inherit" },
  ".cm-content": { minHeight: "100%", padding: "1rem 0", caretColor: "var(--code-ink)" },
  ".cm-line": { padding: "0 1rem" },
  ".cm-gutters": {
    color: "var(--muted)",
    backgroundColor: "var(--code-bg)",
    borderRight: "1px solid var(--line)",
  },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 .65rem 0 .75rem" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "color-mix(in srgb, var(--soft) 58%, transparent)" },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
    backgroundColor: "color-mix(in srgb, var(--ink) 24%, transparent) !important",
  },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--code-ink)" },
  ".cm-panels, .cm-tooltip": { color: "var(--text)", backgroundColor: "var(--surface)" },
  ".cm-tooltip": { border: "1px solid var(--line)" },
  ".cm-completionIcon": {
    width: "1em",
    height: "1em",
    marginRight: ".55em",
    paddingRight: "0",
    color: "var(--muted)",
    backgroundColor: "currentColor",
    verticalAlign: "-.12em",
    opacity: ".82",
    WebkitMaskPosition: "center",
    maskPosition: "center",
    WebkitMaskRepeat: "no-repeat",
    maskRepeat: "no-repeat",
    WebkitMaskSize: "contain",
    maskSize: "contain",
  },
  ".cm-completionIcon::after": { content: "none !important" },
  ...Object.fromEntries(
    Object.entries(completionIconMasks).map(([type, mask]) => [
      `.cm-completionIcon-${type}`,
      { WebkitMaskImage: mask, maskImage: mask },
    ]),
  ),
  ".cm-panels input": { color: "var(--ink)", backgroundColor: "var(--bg)" },
  ".cm-diagnostic-error": { borderLeftColor: "#f06b78" },
  ".cm-lintRange-error": { backgroundImage: "none", borderBottom: "2px wavy #f06b78" },
  ".cm-onda-hover": { maxWidth: "38rem", padding: ".65rem .8rem", whiteSpace: "pre-wrap" },
  ".cm-onda-semantic-enumMember": { color: "var(--syntax-constant)" },
  ".cm-onda-semantic-variable": { color: "var(--code-ink)" },
  ".cm-onda-semantic-port": { color: "var(--syntax-constant)" },
  ".cm-onda-semantic-parameter": { color: "var(--syntax-number)" },
  ".cm-onda-semantic-function": { color: "var(--syntax-function)" },
  ".cm-onda-semantic-type": { color: "var(--syntax-type)" },
  ".cm-onda-semantic-namespace": { color: "var(--syntax-section)" },
  ".cm-onda-semantic-state": { color: "var(--syntax-string)" },
  "&.cm-onda-definition-mode .cm-content": { cursor: "pointer" },
  "&.cm-focused": { outline: "none" },
  "@media (pointer: coarse), (max-width: 720px)": {
    // iOS zooms focused editable content below 16px, which can pan the caret
    // underneath CodeMirror's sticky line-number gutter.
    ".cm-content": { fontSize: "max(16px, 1em)" },
  },
});

export function validProjectPath(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= 160
    && !value.startsWith("/")
    && !value.includes("\\")
    && !value.split("/").some((segment) => !segment || segment === "." || segment === "..")
    && /\.(?:onda|on)$/.test(value);
}

export function normalizeStoredProject(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const sources = Object.fromEntries(
    Object.entries(value.sources ?? {}).filter(
      ([path, source]) => validProjectPath(path) && typeof source === "string",
    ),
  );
  const paths = Object.keys(sources);
  if (!paths.length) return null;
  const entry = paths.includes(value.entry) ? value.entry : paths[0];
  const active = paths.includes(value.active) ? value.active : entry;
  return { entry, active, sources };
}

export class OndaProjectEditor {
  constructor({ parent, tabs, onChange, onActiveFile, onError, initialProject }) {
    this.parent = parent;
    this.tabs = tabs;
    this.onChange = onChange;
    this.onActiveFile = onActiveFile;
    this.onError = onError;
    this.entry = initialProject.entry;
    this.active = initialProject.active;
    this.languageServer = null;
    this.semanticTokenTypes = [];
    this.diagnostics = new Map();
    this.semanticRefreshTimer = 0;
    this.states = new Map();
    this.documentInfo = new Map();
    this.draggedTabPath = null;
    this.pendingDefinitionNavigation = Promise.resolve(false);
    for (const [path, source] of Object.entries(initialProject.sources)) {
      this.documentInfo.set(path, { kind: "project", label: path, readOnly: false });
      this.states.set(path, this.createState(path, source));
    }
    this.tabs.addEventListener("dragover", (event) => this.dragTabOver(event));
    this.tabs.addEventListener("drop", (event) => this.dropTab(event));
    this.view = new EditorView({ state: this.states.get(this.active), parent });
    this.visualViewportResize = () =>
      this.keepCaretVisible(this.view, { centerVertically: true });
    window.visualViewport?.addEventListener("resize", this.visualViewportResize);
    this.renderFiles();
  }

  keepCaretVisible(
    view,
    { centerVertically = false, refocus = false, leftComfort = 0 } = {},
  ) {
    if (!view.hasFocus) return;
    cancelAnimationFrame(this.caretVisibilityFrame);
    this.caretVisibilityFrame = requestAnimationFrame(() => {
      if (!view.hasFocus) return;
      const compact = hasCompactEditingViewport();
      const hidden = refocus
        ? hiddenCaretAxes(view, leftComfort)
        : { horizontal: false, vertical: false };
      view.dispatch({
        effects: EditorView.scrollIntoView(view.state.selection.main.head, {
          x: hidden.horizontal ? "center" : "nearest",
          y: hidden.vertical || (centerVertically && compact) ? "center" : "nearest",
          xMargin: compact ? 32 : 16,
          yMargin: 16,
        }),
      });
    });
  }

  preserveFocusThroughPointer(view) {
    if (!view.hasFocus) return;
    const editorWindow = view.dom.ownerDocument.defaultView ?? window;
    const finish = () => {
      editorWindow.removeEventListener("pointerup", finish);
      editorWindow.removeEventListener("pointercancel", finish);
      requestAnimationFrame(() => {
        const activeElement = view.root.activeElement;
        if (
          view.dom.isConnected
          && !view.hasFocus
          && (!activeElement || !view.dom.contains(activeElement))
        ) {
          view.focus();
        }
      });
    };
    editorWindow.addEventListener("pointerup", finish);
    editorWindow.addEventListener("pointercancel", finish);
  }

  setFontSize(fontSize) {
    this.view.dom.style.fontSize = `${fontSize}px`;
    this.view.requestMeasure();
  }

  createState(path, source, { readOnly = false } = {}) {
    return EditorState.create({
      doc: source,
      extensions: [
        minimalSetup,
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        lintGutter(),
        ondaLanguage,
        syntaxHighlighting(ondaHighlightStyle),
        semanticTokenField,
        autocompletion({ override: [(context) => this.complete(path, context)] }),
        hoverTooltip((view, position) => this.hover(path, view, position), {
          hoverTime: 350,
        }),
        ondaEditorTheme,
        EditorState.tabSize.of(2),
        indentUnit.of("  "),
        Prec.high(keymap.of([
          { key: "Enter", run: insertIndentedNewline },
          indentWithTab,
        ])),
        EditorState.readOnly.of(readOnly),
        EditorView.editable.of(!readOnly),
        EditorView.scrollMargins.of(visibleEditorMargins),
        EditorView.updateListener.of((update) => {
          this.states.set(path, update.state);
          if (update.docChanged) {
            this.onChange?.(this.project());
          }
          if ((update.docChanged || update.selectionSet) && update.view.hasFocus) {
            const deletingBackward = update.docChanged && (
              update.transactions.some((transaction) =>
                transaction.isUserEvent("delete.backward")
              )
              || (
                update.state.doc.length < update.startState.doc.length
                && update.state.selection.main.head
                  <= update.startState.selection.main.head
              )
            );
            this.keepCaretVisible(update.view, {
              refocus: update.docChanged,
              leftComfort: deletingBackward ? 0.25 : 0,
            });
          }
        }),
        EditorView.domEventHandlers({
          pointerdown: (_event, view) => {
            this.preserveFocusThroughPointer(view);
            return false;
          },
          keydown: (event, view) => {
            this.updateDefinitionCursor(path, view, event);
            return false;
          },
          keyup: (event, view) => {
            this.updateDefinitionCursor(path, view, event);
            return false;
          },
          mousemove: (event, view) => {
            this.updateDefinitionCursor(path, view, event);
            return false;
          },
          mouseleave: (_event, view) => {
            view.dom.classList.remove("cm-onda-definition-mode");
            return false;
          },
          mousedown: (event, view) => {
            if (event.button !== 0 || (!event.metaKey && !event.ctrlKey)) return false;
            const offset = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (offset === null) return false;
            event.preventDefault();
            view.dispatch({ selection: { anchor: offset } });
            this.pendingDefinitionNavigation = this.goToDefinition(path, offset).catch((error) => {
              this.onError?.(error);
              return false;
            });
            return true;
          },
        }),
      ],
    });
  }

  connectLanguageServer(languageServer, capabilities) {
    this.languageServer = languageServer;
    this.semanticTokenTypes = capabilities?.semanticTokensProvider?.legend?.tokenTypes ?? [];
    this.scheduleSemanticTokens(this.active, 0);
  }

  project() {
    return {
      entry: this.entry,
      active: this.isProjectDocument() ? this.active : this.entry,
      sources: Object.fromEntries(
        [...this.states]
          .filter(([path]) => this.isProjectDocument(path))
          .map(([path, state]) => [path, state.doc.toString()]),
      ),
    };
  }

  compilerProject() {
    const { entry, sources } = this.project();
    return { entry, sources };
  }

  paths() {
    return [...this.states.keys()].filter((path) => this.isProjectDocument(path));
  }

  allPaths() {
    return [...this.states.keys()];
  }

  isProjectDocument(path = this.active) {
    return this.documentInfo.get(path)?.kind === "project";
  }

  replaceActiveSource(source) {
    const state = this.createState(this.active, source);
    this.states.set(this.active, state);
    this.view.setState(state);
    this.onChange?.(this.project());
  }

  replaceProject(project) {
    const normalized = normalizeStoredProject(project);
    if (!normalized) throw new Error("a project must contain at least one valid Onda file");
    clearTimeout(this.semanticRefreshTimer);
    this.entry = normalized.entry;
    this.active = normalized.active;
    this.diagnostics.clear();
    this.states.clear();
    this.documentInfo.clear();
    for (const [path, source] of Object.entries(normalized.sources)) {
      this.documentInfo.set(path, { kind: "project", label: path, readOnly: false });
      this.states.set(path, this.createState(path, source));
    }
    this.view.setState(this.states.get(this.active));
    this.renderFiles();
    this.onChange?.(this.project());
    this.onActiveFile?.(this.active);
    this.scheduleSemanticTokens(this.active, 0);
    this.view.focus();
  }

  select(path, position) {
    if (!this.states.has(path)) return;
    if (path !== this.active) {
      this.active = path;
      this.view.setState(this.states.get(path));
      this.renderFiles();
      this.onChange?.(this.project());
      this.onActiveFile?.(path);
    }
    if (position) {
      const offset = lspPositionToOffset(this.view.state.doc, position);
      this.view.dispatch({
        selection: { anchor: offset },
        scrollIntoView: true,
      });
    }
    this.view.focus();
    this.scheduleSemanticTokens(path, 0);
  }

  add(path, source = "") {
    this.assertAvailablePath(path);
    this.documentInfo.set(path, { kind: "project", label: path, readOnly: false });
    this.states.set(path, this.createState(path, source));
    this.select(path);
  }

  rename(path) {
    if (!this.isProjectDocument()) throw new Error("read-only library documents cannot be renamed");
    if (path === this.active) return;
    this.assertAvailablePath(path);
    const previous = this.active;
    const source = this.view.state.doc.toString();
    const entries = [...this.states].map(([name, state]) =>
      name === previous ? [path, this.createState(path, source)] : [name, state]
    );
    this.states = new Map(entries);
    this.documentInfo.delete(previous);
    this.documentInfo.set(path, { kind: "project", label: path, readOnly: false });
    this.active = path;
    if (this.entry === previous) this.entry = path;
    this.view.setState(this.states.get(path));
    this.renderFiles();
    this.onChange?.(this.project());
    this.onActiveFile?.(path);
  }

  close(path) {
    const info = this.documentInfo.get(path);
    if (!info || !this.states.has(path)) return;
    const deletesProjectFile = info.kind === "project";
    if (deletesProjectFile && this.paths().length <= 1) {
      throw new Error("a project must contain at least one file");
    }

    const orderedPaths = this.allPaths();
    const closedIndex = orderedPaths.indexOf(path);
    const wasActive = path === this.active;
    this.states.delete(path);
    this.documentInfo.delete(path);
    this.diagnostics.delete(path);

    if (deletesProjectFile && this.entry === path) {
      this.entry = this.paths()[0];
    }
    if (wasActive) {
      const remainingPaths = this.allPaths();
      const next = remainingPaths[Math.min(closedIndex, remainingPaths.length - 1)]
        ?? this.entry;
      this.active = next;
      this.view.setState(this.states.get(next));
      this.onActiveFile?.(next);
      this.scheduleSemanticTokens(next, 0);
    }
    this.renderFiles();
    if (deletesProjectFile) this.onChange?.(this.project());
  }

  setMain() {
    if (!this.isProjectDocument()) throw new Error("a library document cannot be the main file");
    if (this.entry === this.active) return;
    this.entry = this.active;
    this.renderFiles();
    this.onChange?.(this.project());
  }

  setDocumentDiagnostics(path, diagnostics) {
    this.diagnostics.set(path, diagnostics);
    const state = this.states.get(path);
    if (!state) return;
    const codemirrorDiagnostics = diagnostics.map((diagnostic) => ({
      from: lspPositionToOffset(state.doc, diagnostic.range?.start),
      to: Math.max(
        lspPositionToOffset(state.doc, diagnostic.range?.start),
        lspPositionToOffset(state.doc, diagnostic.range?.end),
      ),
      severity: lspSeverity(diagnostic.severity),
      message: diagnostic.message ?? "Onda analysis error",
      source: diagnostic.source ?? "onda",
    }));
    const transaction = setDiagnostics(state, codemirrorDiagnostics);
    if (path === this.active) {
      this.view.dispatch(transaction);
    } else {
      this.states.set(path, state.update(transaction).state);
    }
    this.scheduleSemanticTokens(path);
    this.renderFiles();
  }

  scheduleSemanticTokens(path, delay = 80) {
    if (!this.languageServer || path !== this.active || !this.isProjectDocument(path)) return;
    clearTimeout(this.semanticRefreshTimer);
    this.semanticRefreshTimer = setTimeout(() => {
      void this.refreshSemanticTokens(path);
    }, delay);
  }

  async refreshSemanticTokens(path) {
    if (!this.languageServer || path !== this.active) return;
    try {
      const result = await this.languageServer.semanticTokens(path);
      if (path !== this.active) return;
      const ranges = decodeSemanticTokens(
        this.view.state.doc,
        result?.data ?? [],
        this.semanticTokenTypes,
      );
      this.view.dispatch({ effects: setSemanticTokens.of(Decoration.set(ranges, true)) });
    } catch {
      // Diagnostics remain usable while a partial document cannot be tokenized.
    }
  }

  async complete(path, context) {
    if (!this.languageServer || path !== this.active || !this.isProjectDocument(path)) return null;
    const word = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*$/);
    if (!context.explicit && !word && !/[.:,( ]$/.test(context.state.sliceDoc(Math.max(0, context.pos - 1), context.pos))) {
      return null;
    }
    const position = offsetToLspPosition(context.state.doc, context.pos);
    const result = await this.languageServer.completion(path, position, {
      triggerKind: context.explicit ? 1 : 2,
      triggerCharacter: context.explicit ? undefined : context.state.sliceDoc(context.pos - 1, context.pos),
    });
    return {
      from: word?.from ?? context.pos,
      options: (result?.items ?? []).map((item) => ({
        label: item.label,
        detail: item.detail,
        info: item.documentation?.value ?? item.documentation,
        type: completionType(item.kind),
        apply: item.insertText ?? item.label,
      })),
      filter: true,
    };
  }

  async hover(path, view, offset) {
    if (!this.languageServer || path !== this.active || !this.isProjectDocument(path)) return null;
    const hover = await this.languageServer.hover(
      path,
      offsetToLspPosition(view.state.doc, offset),
    );
    if (!hover?.contents) return null;
    const value = typeof hover.contents === "string"
      ? hover.contents
      : hover.contents.value ?? hover.contents.map?.((entry) => entry.value ?? entry).join("\n") ?? "";
    const range = hover.range ?? {
      start: offsetToLspPosition(view.state.doc, offset),
      end: offsetToLspPosition(view.state.doc, offset),
    };
    return {
      pos: lspPositionToOffset(view.state.doc, range.start),
      end: lspPositionToOffset(view.state.doc, range.end),
      above: true,
      create() {
        const dom = document.createElement("div");
        dom.className = "cm-onda-hover";
        dom.textContent = value.replace(/^```onda\n?|\n?```$/g, "");
        return { dom };
      },
    };
  }

  async goToDefinition(path, offset = this.view.state.selection.main.head) {
    if (!this.languageServer || path !== this.active) return false;
    const info = this.documentInfo.get(path);
    if (!info) return false;
    const position = offsetToLspPosition(this.view.state.doc, offset);
    const location = info.kind === "project"
      ? await this.languageServer.definition(path, position)
      : await this.languageServer.definitionAtUri(info.uri, position);
    const target = Array.isArray(location) ? location[0] : location;
    const targetPath = projectUriToPath(target?.uri ?? target?.targetUri);
    const range = target?.range ?? target?.targetSelectionRange;
    if (!range?.start) return false;
    if (targetPath && this.states.has(targetPath)) {
      this.select(targetPath, range.start);
      return true;
    }
    const targetUri = target?.uri ?? target?.targetUri;
    if (!targetUri) return false;
    const document = await this.languageServer.virtualDocument(targetUri);
    if (!document?.text || !document?.path) return false;
    const virtualPath = `virtual:${document.uri}`;
    if (!this.states.has(virtualPath)) {
      this.documentInfo.set(virtualPath, {
        kind: "library",
        label: document.path,
        readOnly: true,
        uri: document.uri,
      });
      this.states.set(
        virtualPath,
        this.createState(virtualPath, document.text, { readOnly: true }),
      );
    }
    this.select(virtualPath, range.start);
    return true;
  }

  assertAvailablePath(path) {
    if (!validProjectPath(path)) {
      throw new Error("use a relative .onda or .on path without empty, '.' or '..' segments");
    }
    if (this.states.has(path)) throw new Error(`'${path}' already exists`);
  }

  updateDefinitionCursor(path, view, event) {
    const enabled = this.documentInfo.has(path) && (event.ctrlKey || event.metaKey);
    view.dom.classList.toggle("cm-onda-definition-mode", enabled);
  }

  startTabDrag(path, tab, event) {
    this.draggedTabPath = path;
    tab.dataset.dragging = "true";
    this.tabs.dataset.dragging = "true";
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", path);
    }
  }

  dragTabOver(event) {
    if (!this.draggedTabPath) return;
    const dragged = this.tabs.querySelector(
      `.project-file[data-path="${CSS.escape(this.draggedTabPath)}"]`,
    );
    if (!dragged) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    const target = event.target.closest?.(".project-file");
    if (!target || target.parentElement !== this.tabs) {
      this.tabs.append(dragged);
      return;
    }
    if (target === dragged) return;
    const bounds = target.getBoundingClientRect();
    const beforeTarget = event.clientX < bounds.left + bounds.width / 2;
    this.tabs.insertBefore(dragged, beforeTarget ? target : target.nextSibling);
  }

  dropTab(event) {
    if (!this.draggedTabPath) return;
    event.preventDefault();
    const previousOrder = this.allPaths();
    const orderedPaths = [...this.tabs.querySelectorAll(".project-file")]
      .map((tab) => tab.dataset.path);
    this.draggedTabPath = null;
    delete this.tabs.dataset.dragging;
    this.states = reorderMap(this.states, orderedPaths);
    const changed = this.allPaths().some((path, index) => path !== previousOrder[index]);
    this.renderFiles();
    if (changed) this.onChange?.(this.project());
  }

  cancelTabDrag(path) {
    if (this.draggedTabPath !== path) return;
    this.draggedTabPath = null;
    delete this.tabs.dataset.dragging;
    this.renderFiles();
  }

  renderFiles() {
    this.tabs.replaceChildren();
    const projectFileCount = this.paths().length;
    let activeTab = null;
    for (const path of this.allPaths()) {
      const info = this.documentInfo.get(path);
      const tab = document.createElement("div");
      const selectButton = document.createElement("button");
      const errorCount = this.diagnostics.get(path)?.filter((diagnostic) => diagnostic.severity === 1).length ?? 0;
      tab.className = "project-file";
      tab.dataset.path = path;
      tab.dataset.active = String(path === this.active);
      tab.dataset.kind = info?.kind ?? "project";
      tab.dataset.errors = String(errorCount);
      tab.setAttribute("role", "presentation");
      tab.draggable = true;
      tab.addEventListener("dragstart", (event) => this.startTabDrag(path, tab, event));
      tab.addEventListener("dragend", () => this.cancelTabDrag(path));
      selectButton.type = "button";
      selectButton.className = "project-file-select";
      selectButton.setAttribute("role", "tab");
      selectButton.setAttribute("aria-selected", String(path === this.active));
      const label = info?.label ?? path;
      selectButton.title = path === this.entry ? `${label} (main file)` : label;
      const name = document.createElement("span");
      name.className = "project-file-name";
      name.textContent = label;
      selectButton.append(name);
      if (path === this.entry) {
        const main = document.createElement("span");
        main.className = "project-file-main";
        main.textContent = "main";
        selectButton.append(main);
      } else if (info?.kind === "library") {
        const library = document.createElement("span");
        library.className = "project-file-library";
        library.textContent = "stdlib";
        selectButton.append(library);
      }
      if (errorCount) {
        const errors = document.createElement("span");
        errors.className = "project-file-errors";
        errors.textContent = String(errorCount);
        errors.title = `${errorCount} error${errorCount === 1 ? "" : "s"}`;
        selectButton.append(errors);
      }
      selectButton.addEventListener("click", () => this.select(path));

      const closeButton = document.createElement("button");
      const isLastProjectFile = info?.kind === "project" && projectFileCount <= 1;
      closeButton.type = "button";
      closeButton.className = "project-file-close";
      closeButton.textContent = "×";
      closeButton.disabled = isLastProjectFile;
      closeButton.setAttribute(
        "aria-label",
        info?.kind === "library" ? `Close ${label}` : `Delete ${label}`,
      );
      closeButton.title = isLastProjectFile
        ? "A project must contain at least one file"
        : info?.kind === "library" ? `Close ${label}` : `Delete ${label}`;
      closeButton.addEventListener("click", () => {
        try {
          this.close(path);
        } catch (error) {
          this.onError?.(error);
        }
      });

      tab.append(selectButton, closeButton);
      this.tabs.append(tab);
      if (path === this.active) activeTab = tab;
    }
    if (activeTab) {
      const tabBounds = activeTab.getBoundingClientRect();
      const stripBounds = this.tabs.getBoundingClientRect();
      if (tabBounds.left < stripBounds.left) {
        this.tabs.scrollLeft -= stripBounds.left - tabBounds.left;
      } else if (tabBounds.right > stripBounds.right) {
        this.tabs.scrollLeft += tabBounds.right - stripBounds.right;
      }
    }
  }
}

function lspPositionToOffset(doc, position) {
  const lineNumber = Math.min(doc.lines, Math.max(1, Number(position?.line ?? 0) + 1));
  const line = doc.line(lineNumber);
  return Math.min(line.to, line.from + Math.max(0, Number(position?.character ?? 0)));
}

function offsetToLspPosition(doc, offset) {
  const line = doc.lineAt(Math.min(doc.length, Math.max(0, offset)));
  return { line: line.number - 1, character: offset - line.from };
}

function lspSeverity(severity) {
  return ({ 1: "error", 2: "warning", 3: "info", 4: "hint" })[severity] ?? "error";
}

function decodeSemanticTokens(doc, data, tokenTypes) {
  const ranges = [];
  let lineNumber = 0;
  let character = 0;
  for (let index = 0; index + 4 < data.length; index += 5) {
    const deltaLine = data[index];
    lineNumber += deltaLine;
    character = deltaLine === 0 ? character + data[index + 1] : data[index + 1];
    if (lineNumber >= doc.lines) continue;
    const line = doc.line(lineNumber + 1);
    const from = Math.min(line.to, line.from + character);
    const to = Math.min(line.to, from + data[index + 2]);
    const type = tokenTypes[data[index + 3]];
    if (to > from && type) {
      ranges.push(Decoration.mark({ class: `cm-onda-semantic-${type}` }).range(from, to));
    }
  }
  return ranges;
}
