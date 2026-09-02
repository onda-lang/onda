// Shared browser IDE controller used by the website and standalone example.
import { closeCompletion, startCompletion } from "@codemirror/autocomplete";
import { createCompiler } from "@onda-lang/wasm-compiler";
import {
  compileOndaProcessorModule,
  createOndaAudioProcessorInitialized,
  flattenedAudioChannelCount,
} from "@onda-lang/webaudio";

import {
  encodeOndaBuffer,
  prepareBufferBinding,
  prepareBufferBindings,
} from "./browser-buffers.js";
import { compilationKey } from "./compile-cache.js";
import { normalizeStoredProject, OndaProjectEditor } from "./editor.js";
import { loadExampleProject } from "./examples.js";
import { OndaBrowserLsp } from "./lsp-client.js";
import { BrowserMidiInputs, isMidiKeyboardEditingTarget } from "./midi.js";
import { BrowserMicrophoneInput } from "./microphone.js";
import { BrowserRunViewHost, BrowserScopeSource } from "./run-view-host.js";
import {
  decodeSharedSession,
  encodeSharedSession,
  sharedSessionHash,
} from "./share.js";
import {
  createProjectArchive,
  decodeProjectArchive,
  sourceGraphWithWorkspaceDocuments,
} from "./project-archive.js";
import defaultSource from "./default.onda";

const statusEl = document.querySelector("[data-status]");
const editorEl = document.querySelector("[data-editor]");
const fileTabsEl = document.querySelector("[data-file-tabs]");
const newProjectButton = document.querySelector("[data-new-project]");
const openProjectButton = document.querySelector("[data-open-project]");
const downloadProjectButton = document.querySelector("[data-download-project]");
const newFileButton = document.querySelector("[data-new-file]");
const shareProjectButton = document.querySelector("[data-share-project]");
const sampleRateEl = document.querySelector("[data-sample-rate]");
const blockSizeEl = document.querySelector("[data-block-size]");
const editorFontSizeEl = document.querySelector("[data-editor-font-size]");
const runViewFrame = document.querySelector("[data-run-view]");

const pageParams = new URLSearchParams(window.location.search);
const smokeMode = pageParams.has("smoke");
const requestedExample = pageParams.get("example");
const projectStorageKey = "onda.browser-ide.project.v1";
const editorFontSizeStorageKey = "onda.browser-ide.editor-font-size.v1";
const compileOptionsStorageKey = "onda.browser-ide.compile-options.v1";
const hostedAssets = globalThis.__ONDA_PLAYGROUND_ASSETS__ ?? {};
const supportedSampleRates = new Set([44_100, 48_000]);
const supportedBlockSizes = new Set([128, 256, 512, 1024, 2048]);

let compiler = null;
let languageServer = null;
let artifact = null;
let artifactCompilationKey = null;
let compiledModule = null;
let context = null;
let audioProcessor = null;
let projectEditor = null;
let compiling = false;
let processingRequested = false;
let runGeneration = 0;
let projectSaveTimer = 0;
let editorFontSize = 14;
let needsBundledExample = false;
let sharedSession = null;
let sharedSessionError = null;
let requestedExampleProject = null;
let requestedExampleError = null;
const bufferFiles = new Map();
const microphoneInput = new BrowserMicrophoneInput();
let midiInputs = null;

runViewFrame.src = hostedAssets.runViewUrl ?? "./run.html";
const runView = new BrowserRunViewHost(runViewFrame, {
  start: () => runProject(),
  stop: () => stopExecution(),
  resetParams: () => resetRunParams(),
  setParam: (name, value) => audioProcessor?.setParam(name, value),
  triggerEvent: (name, values) => audioProcessor?.trigger(name, values),
  midiEventsChanged: (events) => midiInputs?.setDeclared(events),
  refreshMidiInputs: async () => {
    try {
      await midiInputs?.refresh(true);
      runView.showError("");
    } catch (error) {
      runView.showError(error);
    }
  },
  setMidiInputDevice: async (name) => {
    try {
      await midiInputs?.select(name);
      runView.showError("");
    } catch (error) {
      runView.showError(error);
    }
  },
  bindBufferFile: (name, file) => bindBufferFile(name, file),
  clearBuffer: (name) => clearBuffer(name),
  error: (error) => {
    runView.setError(error);
    setErrorStatus();
  },
});
midiInputs = new BrowserMidiInputs({
  onState: ({ devices, current }) => runView.setMidiInputs(devices, current),
  onEvent: (name, values) => audioProcessor?.trigger(name, values),
  onError: (error) => {
    runView.setError(error);
    setErrorStatus();
  },
});
const scopeSource = new BrowserScopeSource(runView);

document.addEventListener("keydown", (event) => {
  if (
    event.repeat
    || event.isComposing
    || event.ctrlKey
    || event.metaKey
    || event.altKey
    || isMidiKeyboardEditingTarget(event.target)
  ) return;
  runView.sendComputerKey(event.code, true);
}, { capture: true });
document.addEventListener("keyup", (event) => {
  runView.sendComputerKey(event.code, false);
}, { capture: true });
window.addEventListener("blur", () => runView.releaseVirtualMidiNotes());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) runView.releaseVirtualMidiNotes();
});

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = `status ${kind}`.trim();
}

function setErrorStatus() {
  setStatus("Error", "fail");
}

function reportSmokeResult(result) {
  if (!smokeMode) return;
  fetch(hostedAssets.smokeResultUrl ?? "./__result", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(result),
  }).catch(() => {});
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  return String(error?.message ?? error);
}

function compileOptions() {
  const sampleRate = Number(sampleRateEl.value);
  const blockSize = Number(blockSizeEl.value);
  if (!supportedSampleRates.has(sampleRate)) {
    throw new Error("sample rate must be 44100 or 48000 Hz");
  }
  if (!supportedBlockSizes.has(blockSize)) {
    throw new Error("block size must be 128, 256, 512, 1024, or 2048 frames");
  }
  return { sampleRate, blockSize };
}

function validEditorFontSize(value) {
  const fontSize = Number(value);
  return Number.isInteger(fontSize) && fontSize >= 8 && fontSize <= 40
    ? fontSize
    : null;
}

function restoreEditorFontSize() {
  try {
    const fontSize = validEditorFontSize(localStorage.getItem(editorFontSizeStorageKey));
    if (fontSize !== null) editorFontSize = fontSize;
  } catch {
    // The default remains available when browser storage is disabled.
  }
  editorFontSizeEl.value = String(editorFontSize);
}

function restoreCompileOptions() {
  try {
    const stored = JSON.parse(localStorage.getItem(compileOptionsStorageKey));
    const sampleRate = Number(stored?.sampleRate);
    const blockSize = Number(stored?.blockSize);
    if (supportedSampleRates.has(sampleRate)) {
      sampleRateEl.value = String(sampleRate);
    }
    if (supportedBlockSizes.has(blockSize)) {
      blockSizeEl.value = String(blockSize);
    }
  } catch {
    // The HTML defaults remain available when storage is disabled or malformed.
  }
}

function saveCompileOptions(options) {
  try {
    localStorage.setItem(compileOptionsStorageKey, JSON.stringify(options));
  } catch {
    // Compile controls still work for this page when browser storage is disabled.
  }
}

function applyEditorFontSize(fontSize) {
  editorFontSize = fontSize;
  projectEditor.setFontSize(fontSize);
  try {
    localStorage.setItem(editorFontSizeStorageKey, String(fontSize));
  } catch {
    // Font sizing still works for this page when browser storage is disabled.
  }
}

function handlePlaygroundShortcut(event) {
  if (event.isComposing || event.altKey || (!event.ctrlKey && !event.metaKey)) return;
  const run = event.key === "Enter";
  const stop = event.key === "." || event.code === "Period";
  if (!run && !stop) return;
  event.preventDefault();
  if (event.repeat) return;
  if (run) void runProject();
  else void stopExecution();
}

async function runProject() {
  if (!compiler || compiling) return;
  processingRequested = true;
  let options;
  const generation = ++runGeneration;
  try {
    options = compileOptions();
    reserveAudioContext(options.sampleRate);
  } catch (error) {
    runView.setError(error);
    setErrorStatus();
    return;
  }
  compiling = true;
  const project = projectEditor.compilerProject();
  const key = compilationKey(project, options);
  const needsCompilation = !artifact || artifactCompilationKey !== key;
  if (needsCompilation) {
    runView.setCompiling(project.entry, options);
    setStatus("Compiling");
  } else {
    setStatus("Starting");
  }
  try {
    if (needsCompilation) {
      await languageServer.syncProject(project);
      if (generation !== runGeneration) return;
      const { artifact: compiledArtifact } = await compiler.compileWorkspace(project, options);
      if (generation !== runGeneration) return;
      const nextCompiledModule = await compileOndaProcessorModule(compiledArtifact);
      if (generation !== runGeneration) return;
      artifact = compiledArtifact;
      compiledModule = nextCompiledModule;
      artifactCompilationKey = key;
    }
    localStorage.setItem(projectStorageKey, JSON.stringify(projectEditor.project()));
    runView.setArtifact(artifact, bufferFiles);
    await startAudio();
    setStatus("Playing", "playing");
    const runViewDocument = runViewFrame.contentDocument;
    const editorBindings = smokeMode ? await smokeEditorBindings() : {};
    const themeFollowsPage = smokeMode
      ? await smokeThemeSync(runViewDocument)
      : undefined;
    reportSmokeResult({
      ok: true,
      surface: "playground",
      wasmBytes: artifact.wasm.byteLength,
      backend: artifact.metadata.backend,
      schemaVersion: artifact.metadata.mir_schema_version,
      blockSize: artifact.metadata.compile.block_size,
      projectFiles: projectEditor.paths().length,
      projectPaths: projectEditor.paths(),
      sharedSessionLoaded: Boolean(sharedSession),
      exampleLoaded: requestedExampleProject ? requestedExample : null,
      buffers: artifact.metadata.metadata.buffers.length,
      hasLineNumbers: Boolean(editorEl.querySelector(".cm-lineNumbers")),
      hasLspDiagnostics: Boolean(editorEl.querySelector(".cm-gutter-lint")),
      hasSharedRunView: Boolean(runViewFrame.contentWindow),
      hasSeparateRunButton: Boolean(document.querySelector("[data-run-project]")),
      hasMasterGain: Boolean(document.querySelector("[data-gain]")),
      hasDeviceControls: !runViewDocument?.querySelector("#refresh-devices")?.hidden,
      pageTheme: resolvedPageTheme(),
      runViewTheme: runViewDocument?.documentElement.dataset.theme ?? null,
      runViewMatchesEditorBackground: Boolean(runViewDocument)
        && getComputedStyle(runViewDocument.documentElement).backgroundColor
          === getComputedStyle(editorEl.querySelector(".cm-editor")).backgroundColor,
      scrollbarsMatch: Boolean(runViewDocument)
        && getComputedStyle(document.documentElement).scrollbarColor
          === getComputedStyle(runViewDocument.documentElement).scrollbarColor,
      hasMainFileTab: Boolean(fileTabsEl.querySelector(".project-file-main")),
      hasEntryFileLabel: fileTabsEl.textContent.toLowerCase().includes("entry"),
      hasTabFileMenu: Boolean(fileTabsEl.querySelector(".project-file-menu-trigger")),
      hasDeleteFileButton: Boolean(document.querySelector("[data-delete-file]")),
      hasShareButton: Boolean(shareProjectButton),
      hasOpenProjectButton: Boolean(openProjectButton),
      hasDownloadProjectButton: Boolean(downloadProjectButton),
      microphonePermissionRequested: microphoneInput.permissionRequested,
      tabStripFits: fileTabsEl.scrollWidth <= fileTabsEl.clientWidth,
      sampleRateLabels: [...sampleRateEl.options].map((option) => option.textContent),
      themeFollowsPage,
      ...editorBindings,
    });
  } catch (error) {
    await closeAudioContext();
    runView.setError(error);
    setErrorStatus();
    reportSmokeResult({ ok: false, error: errorMessage(error) });
  } finally {
    compiling = false;
  }
}

async function smokeThemeSync(runViewDocument) {
  const root = document.documentElement;
  const original = root.dataset.theme || "auto";
  const alternate = resolvedPageTheme() === "dark" ? "light" : "dark";
  root.dataset.theme = alternate;
  await nextAnimationFrame();
  await nextAnimationFrame();
  const followed = runViewDocument?.documentElement.dataset.theme === alternate;
  root.dataset.theme = original;
  await nextAnimationFrame();
  await nextAnimationFrame();
  return followed;
}

function nextAnimationFrame() {
  return new Promise((resolve) => requestAnimationFrame(resolve));
}

async function smokeEditorBindings() {
  const view = projectEditor.view;
  const initialProjectFileCount = projectEditor.paths().length;
  const initialPath = projectEditor.active;
  const originalState = view.state;
  const content = view.contentDOM;
  view.focus();

  const tabEvent = new KeyboardEvent("keydown", {
    key: "Tab",
    code: "Tab",
    bubbles: true,
    cancelable: true,
  });
  const tabCanceled = !content.dispatchEvent(tabEvent);
  const tabInsertedText = !view.state.doc.eq(originalState.doc);
  const tabKeptFocus = document.activeElement === content;
  view.setState(originalState);
  projectEditor.states.set(projectEditor.active, originalState);

  view.dispatch({ selection: { anchor: view.state.doc.length } });
  const completionStarted = startCompletion(view);
  const completionIcon = completionStarted
    ? await waitForElement(".cm-tooltip-autocomplete .cm-completionIcon")
    : null;
  const completionIconStyle = completionIcon ? getComputedStyle(completionIcon) : null;
  const completionIconsHandled = Boolean(
    completionIcon
    && completionIconStyle
    && completionIconStyle.maskImage !== "none",
  );
  closeCompletion(view);
  view.setState(originalState);
  projectEditor.states.set(projectEditor.active, originalState);

  const runEvent = new KeyboardEvent("keydown", {
    key: "Enter",
    code: "Enter",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  const modEnterHandled = !sampleRateEl.dispatchEvent(runEvent);

  const definitionModeEvent = new KeyboardEvent("keydown", {
    key: "Control",
    code: "ControlLeft",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  content.dispatchEvent(definitionModeEvent);
  const definitionCursorHandled = getComputedStyle(content).cursor === "pointer";
  content.dispatchEvent(new KeyboardEvent("keyup", {
    key: "Control",
    code: "ControlLeft",
    bubbles: true,
  }));

  const source = view.state.doc.toString();
  const definitionCandidate = source.includes("def soft_clip")
    ? {
        declaration: source.indexOf("def soft_clip") + "def ".length,
        use: source.lastIndexOf("soft_clip("),
      }
    : {
        declaration: source.indexOf("phase = 0.0"),
        use: source.lastIndexOf("phase = phase + incr") + "phase = ".length,
      };
  view.dispatch({
    selection: { anchor: definitionCandidate.use },
    scrollIntoView: true,
  });
  await nextAnimationFrame();
  const coords = view.coordsAtPos(definitionCandidate.use);
  const definitionEvent = new MouseEvent("mousedown", {
    button: 0,
    ctrlKey: true,
    clientX: (coords?.left ?? 0) + 1,
    clientY: ((coords?.top ?? 0) + (coords?.bottom ?? 0)) / 2,
    bubbles: true,
    cancelable: true,
  });
  const definitionGestureHandled = coords ? !content.dispatchEvent(definitionEvent) : false;
  const definitionNavigated = await projectEditor.pendingDefinitionNavigation;
  const definitionLine = view.state.doc.lineAt(definitionCandidate.declaration).number;
  const selectedLine = view.state.doc.lineAt(view.state.selection.main.head).number;
  const localDefinitionHandled = definitionGestureHandled
    && definitionNavigated
    && projectEditor.active === initialPath
    && selectedLine === definitionLine;

  let stdlibDefinitionHandled = null;
  let stdlibChainedDefinitionHandled = null;
  let stdlibTabCloseHandled = null;
  const stdlibUse = source.indexOf("std::osc::Saw");
  if (stdlibUse >= 0) {
    const sawOffset = stdlibUse + "std::osc::".length;
    projectEditor.select(initialPath, offsetToSmokePosition(view.state.doc, sawOffset));
    await nextAnimationFrame();
    const sawCoords = view.coordsAtPos(sawOffset);
    const stdlibEvent = new MouseEvent("mousedown", {
      button: 0,
      ctrlKey: true,
      clientX: (sawCoords?.left ?? 0) + 1,
      clientY: ((sawCoords?.top ?? 0) + (sawCoords?.bottom ?? 0)) / 2,
      bubbles: true,
      cancelable: true,
    });
    const stdlibGestureHandled = sawCoords ? !content.dispatchEvent(stdlibEvent) : false;
    const stdlibNavigated = await projectEditor.pendingDefinitionNavigation;
    const virtualInfo = projectEditor.documentInfo.get(projectEditor.active);
    const virtualPath = projectEditor.active;
    stdlibDefinitionHandled = stdlibGestureHandled
      && stdlibNavigated
      && virtualInfo?.kind === "library"
      && virtualInfo.readOnly === true;

    const librarySource = view.state.doc.toString();
    const phasorDeclaration = librarySource.indexOf("proc Phasor") + "proc ".length;
    const phasorUse = librarySource.indexOf("Phasor", librarySource.indexOf("proc Saw"));
    view.dispatch({ selection: { anchor: phasorUse }, scrollIntoView: true });
    await nextAnimationFrame();
    const phasorCoords = view.coordsAtPos(phasorUse);
    const chainedEvent = new MouseEvent("mousedown", {
      button: 0,
      ctrlKey: true,
      clientX: (phasorCoords?.left ?? 0) + 1,
      clientY: ((phasorCoords?.top ?? 0) + (phasorCoords?.bottom ?? 0)) / 2,
      bubbles: true,
      cancelable: true,
    });
    const chainedGestureHandled = phasorCoords ? !content.dispatchEvent(chainedEvent) : false;
    const chainedNavigated = await projectEditor.pendingDefinitionNavigation;
    stdlibChainedDefinitionHandled = chainedGestureHandled
      && chainedNavigated
      && projectEditor.active === virtualPath
      && view.state.doc.lineAt(view.state.selection.main.head).number
        === view.state.doc.lineAt(phasorDeclaration).number;

    const libraryTab = [...fileTabsEl.querySelectorAll(".project-file")]
      .find((tab) => tab.dataset.path === virtualPath);
    libraryTab?.querySelector(".project-file-close")?.click();
    stdlibTabCloseHandled = !projectEditor.states.has(virtualPath)
      && projectEditor.paths().length === initialProjectFileCount;
    projectEditor.select(initialPath);
  }

  const projectFileCount = projectEditor.paths().length;
  const smokePath = "smoke-close.onda";
  projectEditor.add(smokePath, "# Browser close-tab smoke test\n");
  const projectTab = [...fileTabsEl.querySelectorAll(".project-file")]
    .find((tab) => tab.dataset.path === smokePath);
    projectTab?.querySelector(".project-file-menu-trigger")?.click();
    document.querySelector(".project-file-menu-delete")?.click();
  const projectTabCloseHandled = !projectEditor.states.has(smokePath)
    && projectEditor.paths().length === projectFileCount;

  const originalFontSize = editorFontSize;
  editorFontSizeEl.value = "23";
  editorFontSizeEl.dispatchEvent(new Event("input", { bubbles: true }));
  const lineNumber = editorEl.querySelector(".cm-lineNumbers .cm-gutterElement");
  const editorFontSizeImmediately = getComputedStyle(content).fontSize;
  const lineNumberFontSizeImmediately = lineNumber
    ? getComputedStyle(lineNumber).fontSize
    : null;
  await nextAnimationFrame();
  const editorFontSizeAfterMeasure = getComputedStyle(content).fontSize;
  const lineNumberFontSizeAfterMeasure = lineNumber
    ? getComputedStyle(lineNumber).fontSize
    : null;
  editorFontSizeEl.value = String(originalFontSize);
  editorFontSizeEl.dispatchEvent(new Event("input", { bubbles: true }));
  const editorFontSizeHandled =
    editorFontSizeImmediately === "23px"
    && editorFontSizeAfterMeasure === "23px";
  const lineNumberFontSizeHandled =
    lineNumberFontSizeImmediately === "23px"
    && lineNumberFontSizeAfterMeasure === "23px";

  view.focus();
  const selectionBeforeScroll = view.state.selection;
  const scrollTabIndex = view.scrollDOM.getAttribute("tabindex");
  view.scrollDOM.tabIndex = -1;
  view.scrollDOM.focus({ preventScroll: true });
  view.scrollDOM.dispatchEvent(new Event("scroll"));
  await nextAnimationFrame();
  await nextAnimationFrame();
  const scrollPreservedTextFocus =
    view.hasFocus
    && view.state.selection.eq(selectionBeforeScroll);
  if (scrollTabIndex === null) view.scrollDOM.removeAttribute("tabindex");
  else view.scrollDOM.setAttribute("tabindex", scrollTabIndex);

  const caretProbe = `\n# ${"mobile-caret-scroll-".repeat(32)}`;
  const caretProbeFrom = view.state.doc.length;
  view.dispatch({
    changes: { from: caretProbeFrom, insert: caretProbe },
    selection: { anchor: caretProbeFrom + caretProbe.length },
    userEvent: "input.type",
  });
  await nextAnimationFrame();
  await nextAnimationFrame();
  await nextAnimationFrame();
  const caretBounds = view.coordsAtPos(view.state.selection.main.head);
  const editorBounds = view.scrollDOM.getBoundingClientRect();
  const gutterBounds = view.dom.querySelector(".cm-gutters")?.getBoundingClientRect();
  const viewport = window.visualViewport;
  const viewportLeft = viewport?.offsetLeft ?? editorBounds.left;
  const viewportRight = viewport
    ? viewportLeft + viewport.width
    : editorBounds.right;
  const visibleLeft = Math.max(
    editorBounds.left,
    gutterBounds?.right ?? editorBounds.left,
    viewportLeft,
  ) + 16;
  const visibleRight = Math.min(editorBounds.right, viewportRight) - 16;
  const mobileCaretVisibilityHandled = Boolean(caretBounds)
    && view.scrollDOM.scrollLeft > 0
    && caretBounds.left >= visibleLeft - 2
    && caretBounds.right <= visibleRight + 2;
  const mobileGutterScrollHandled = Boolean(gutterBounds)
    && view.scrollDOM.scrollLeft > 0
    && gutterBounds.right <= editorBounds.left + 2;

  const leftCaretProbeFrom = view.state.doc.length;
  view.dispatch({
    changes: { from: leftCaretProbeFrom, insert: "\n#" },
    selection: { anchor: leftCaretProbeFrom + 2 },
    userEvent: "input.type",
  });
  await nextAnimationFrame();
  await nextAnimationFrame();
  await nextAnimationFrame();
  const leftCaretBounds = view.coordsAtPos(view.state.selection.main.head);
  const leftEditorBounds = view.scrollDOM.getBoundingClientRect();
  const leftGutterBounds = view.dom.querySelector(".cm-gutters")?.getBoundingClientRect();
  const leftVisibleLeft = Math.max(
    leftEditorBounds.left,
    leftGutterBounds?.right ?? leftEditorBounds.left,
    viewportLeft,
  ) + 16;
  const leftVisibleRight = Math.min(leftEditorBounds.right, viewportRight) - 16;
  const mobileLeftCaretHandled = Boolean(leftCaretBounds)
    && view.scrollDOM.scrollLeft <= 1
    && leftCaretBounds.left >= leftVisibleLeft - 2
    && leftCaretBounds.right <= leftVisibleRight + 2;
  view.setState(originalState);
  projectEditor.states.set(projectEditor.active, originalState);

  const stopEvent = new KeyboardEvent("keydown", {
    key: ".",
    code: "Period",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  const ctrlPeriodHandled = !blockSizeEl.dispatchEvent(stopEvent);

  const runViewWindow = runViewFrame.contentWindow;
  const runViewDocument = runViewFrame.contentDocument;
  const runViewRunEvent = new runViewWindow.KeyboardEvent("keydown", {
    key: "Enter",
    code: "Enter",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  const runViewStopEvent = new runViewWindow.KeyboardEvent("keydown", {
    key: ".",
    code: "Period",
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  const runViewShortcutsHandled = !runViewDocument.body.dispatchEvent(runViewRunEvent)
    && !runViewDocument.body.dispatchEvent(runViewStopEvent);

  const project = projectEditor.project();
  const encodedSession = await encodeSharedSession(project);
  const decodedSession = await decodeSharedSession(sharedSessionHash(encodedSession));
  const shareRoundTripHandled = decodedSession.entry === project.entry
    && decodedSession.active === project.active
    && JSON.stringify(decodedSession.sources) === JSON.stringify(project.sources);

  return {
    tabInsertedText: tabCanceled && tabInsertedText,
    tabKeptFocus,
    completionIconsHandled,
    modEnterHandled,
    definitionCursorHandled,
    modClickHandled: localDefinitionHandled,
    stdlibDefinitionHandled,
    stdlibChainedDefinitionHandled,
    stdlibTabCloseHandled,
    projectTabCloseHandled,
    editorFontSizeHandled,
    lineNumberFontSizeHandled,
    scrollPreservedTextFocus,
    mobileCaretVisibilityHandled,
    mobileGutterScrollHandled,
    mobileLeftCaretHandled,
    ctrlPeriodHandled,
    runViewShortcutsHandled,
    shareRoundTripHandled,
  };
}

async function waitForElement(selector, attempts = 50) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    const element = document.querySelector(selector);
    if (element) return element;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  return null;
}

function offsetToSmokePosition(doc, offset) {
  const line = doc.lineAt(offset);
  return { line: line.number - 1, character: offset - line.from };
}

function resolvedPageTheme() {
  const selected = document.documentElement.dataset.theme;
  if (selected === "dark" || selected === "light") return selected;
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

async function startAudio() {
  if (!artifact) throw new Error("run the project to compile it first");
  if (audioProcessor) return;
  let metadata = artifact.metadata;
  const requestedSampleRate = Number(metadata.compile.sample_rate);
  const inputChannels = flattenedAudioChannelCount(metadata.metadata.inputs);
  const outputChannels = flattenedAudioChannelCount(metadata.metadata.outputs);
  if (inputChannels > 32 || outputChannels > 32) {
    throw new Error("Web Audio supports at most 32 flattened channels per node");
  }

  const AudioContextConstructor = globalThis.AudioContext ?? globalThis.webkitAudioContext;
  if (typeof AudioContextConstructor !== "function") {
    throw new Error("Web Audio is not available in this browser");
  }
  context ??= new AudioContextConstructor({ sampleRate: requestedSampleRate });
  try {
    if (context.sampleRate !== requestedSampleRate) {
      if (!supportedSampleRates.has(context.sampleRate)) {
        throw new Error(
          `browser selected unsupported ${context.sampleRate} Hz audio; use a 44100 or 48000 Hz output device`,
        );
      }
      sampleRateEl.value = String(context.sampleRate);
      const options = {
        sampleRate: context.sampleRate,
        blockSize: Number(metadata.compile.block_size),
      };
      await languageServer.setAnalysisOptions(options);
      const project = projectEditor.compilerProject();
      const { artifact: compiledArtifact } = await compiler.compileWorkspace(project, options);
      const nextCompiledModule = await compileOndaProcessorModule(compiledArtifact);
      artifact = compiledArtifact;
      compiledModule = nextCompiledModule;
      artifactCompilationKey = compilationKey(project, options);
      metadata = artifact.metadata;
      runView.setArtifact(artifact, bufferFiles);
    }
    const buffers = await prepareBufferBindings(metadata, bufferFiles, compiler);
    const params = Object.fromEntries(
      runView.state.params.map((param) => [param.name, param.value]),
    );
    audioProcessor = await createOndaAudioProcessorInitialized(context, artifact, {
      compiledModule,
      params,
      buffers,
      workletUrl: hostedAssets.workletUrl,
    });

    await microphoneInput.connect(context, audioProcessor.node, inputChannels);

    if (outputChannels) {
      audioProcessor.node.connect(context.destination);
      scopeSource.start(context, audioProcessor.node, outputChannels);
    }
    await Promise.race([
      context.resume().catch(() => {}),
      new Promise((resolve) => setTimeout(resolve, 250)),
    ]);
    runView.setRunning(
      context.sampleRate,
      context.state === "running" ? undefined : "Audio ready — click the page to enable output",
    );
  } catch (error) {
    scopeSource.stop();
    microphoneInput.disconnect();
    audioProcessor?.close();
    audioProcessor = null;
    await context.close();
    context = null;
    throw error;
  }
}

async function stopAudio() {
  if (!context) {
    runView.setStopped();
    setStatus("Stopped");
    return;
  }
  scopeSource.stop();
  microphoneInput.disconnect();
  audioProcessor?.node.disconnect();
  audioProcessor?.close();
  audioProcessor = null;
  await closeAudioContext();
  runView.setStopped();
  setStatus("Stopped");
}

async function stopExecution() {
  processingRequested = false;
  runGeneration += 1;
  await stopAudio();
}

function reserveAudioContext(sampleRate) {
  scopeSource.stop();
  microphoneInput.disconnect();
  audioProcessor?.node.disconnect();
  audioProcessor?.close();
  audioProcessor = null;
  if (context) void context.close();
  const AudioContextConstructor = globalThis.AudioContext ?? globalThis.webkitAudioContext;
  if (typeof AudioContextConstructor !== "function") {
    throw new Error("Web Audio is not available in this browser");
  }
  context = new AudioContextConstructor({ sampleRate });
  void context.resume().catch(() => {});
}

async function closeAudioContext() {
  if (!context) return;
  const closing = context;
  context = null;
  await closing.close();
}

async function resetRunParams() {
  if (!audioProcessor) return;
  await Promise.all(
    runView.state.params.map((param) => audioProcessor.setParam(param.name, param.value)),
  );
}

async function bindBufferFile(name, file) {
  if (!(file instanceof File)) throw new Error("the selected buffer is not a browser File");
  const buffer = artifact?.metadata.metadata.buffers.find((item) => item.name === name);
  if (!buffer) throw new Error(`unknown buffer '${name}'`);
  let binding;
  try {
    binding = await prepareBufferBinding(buffer, file, compiler);
  } catch (error) {
    runView.showError(error);
    setErrorStatus();
    return;
  }
  bufferFiles.set(name, file);
  runView.updateBufferFile(name, file, {
    frames: binding.frames,
    channels: binding.channels,
    sampleRate: binding.sampleRate,
  });
  if (context) {
    await restartAudioForBuffers();
  } else if (processingRequested) {
    await runProject();
  }
}

async function clearBuffer(name) {
  bufferFiles.delete(name);
  runView.updateBufferFile(name, null);
  if (context) await restartAudioForBuffers();
}

async function restartAudioForBuffers() {
  await stopAudio();
  await startAudio();
}

function scheduleProjectSave(project) {
  clearTimeout(projectSaveTimer);
  projectSaveTimer = setTimeout(() => {
    localStorage.setItem(projectStorageKey, JSON.stringify(project));
  }, 150);
}

async function loadToolchain() {
  setStatus("Loading");
  const compilerLoading = createCompiler({
    worker: true,
    workerUrl: hostedAssets.workerUrl,
    frontendWasm: hostedAssets.frontendWasm,
  });
  if (needsBundledExample) {
    projectEditor.replaceActiveSource(
      smokeMode ? `include "./smoke-buffer.onda"\n\n${defaultSource}` : defaultSource,
    );
    if (smokeMode) {
      projectEditor.add("smoke-buffer.onda", "buffers:\n  clip: buffer<f32>\n");
      bufferFiles.set("clip", smokeBufferFile());
      projectEditor.select("main.onda");
    }
  }

  compiler = await compilerLoading;
  languageServer = new OndaBrowserLsp(compiler, {
    onDiagnostics: (path, diagnostics) => projectEditor.setDocumentDiagnostics(path, diagnostics),
    onError: () => setErrorStatus(),
  });
  const capabilities = await languageServer.initialize(compileOptions());
  projectEditor.connectLanguageServer(languageServer, capabilities);
  await languageServer.syncProject(projectEditor.compilerProject());
  runView.setPath(projectEditor.entry);
  const initialProjectError = sharedSessionError ?? requestedExampleError;
  if (initialProjectError) {
    runView.setError(initialProjectError);
    setErrorStatus();
  } else {
    setStatus("Ready", "ready");
  }
  if (smokeMode) await runProject();
}

function smokeBufferFile() {
  const binary = atob(
    "UklGRiYAAABXQVZFZm10IBAAAAABAAEAgLsAAAB3AQACABAAZGF0YQIAAAAAAA==",
  );
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  return new File([bytes], "test-buffer.wav", { type: "audio/wav" });
}

function initializeEditor(initialSharedSession, initialExampleProject) {
  restoreEditorFontSize();
  restoreCompileOptions();
  let storedProject = normalizeStoredProject(initialSharedSession ?? initialExampleProject);
  if (!storedProject && !smokeMode && !requestedExample) {
    try {
      storedProject = normalizeStoredProject(JSON.parse(localStorage.getItem(projectStorageKey)));
    } catch {
      // Restore the bundled project if local state is malformed.
    }
  }
  needsBundledExample = !storedProject;
  projectEditor = new OndaProjectEditor({
    parent: editorEl,
    tabs: fileTabsEl,
    onError: () => setErrorStatus(),
    onChange: (project) => {
      if (!smokeMode) scheduleProjectSave(project);
      runView.markSourceDirty(project.entry);
      languageServer?.syncProject({ entry: project.entry, sources: project.sources });
    },
    onRenameFile: (path) => {
      const nextPath = prompt("Rename project file", path)?.trim();
      if (nextPath) editProject(() => projectEditor.rename(nextPath));
    },
    initialProject: storedProject ?? {
      entry: "main.onda",
      active: "main.onda",
      sources: { "main.onda": "# Loading the bundled Onda example…\n" },
    },
  });
  projectEditor.setFontSize(editorFontSize);
}

function editProject(action) {
  try {
    action();
  } catch (error) {
    setErrorStatus();
  }
}

async function createNewProject() {
  if (!window.confirm("Create a new project? This will delete your current project.")) return;
  await stopExecution();
  bufferFiles.clear();
  projectEditor.replaceProject({
    entry: "main.onda",
    active: "main.onda",
    sources: { "main.onda": "" },
  });
  runView.clearArtifact("main.onda");
  const url = new URL(window.location.href);
  url.searchParams.delete("example");
  url.hash = "";
  history.replaceState(null, "", url);
  setStatus("Ready", "ready");
}

async function openProjectFile(file) {
  if (!(file instanceof File)) return;
  try {
    await stopExecution();
    let project;
    let importedBuffers = new Map();
    if (/\.(?:onda|on)$/i.test(file.name)) {
      const path = file.name.replaceAll("\\", "/");
      project = {
        entry: path,
        active: path,
        sources: { [path]: await file.text() },
      };
    } else {
      const decoded = await decodeProjectArchive(
        await file.arrayBuffer(),
        compiler,
        chooseArchivedProject,
      );
      project = decoded.project;
      importedBuffers = new Map(
        [...decoded.buffers].map(([name, asset]) => [
          name,
          new File(
            [asset.bytes],
            asset.path.split("/").at(-1) ?? `${name}.ondabuffer`,
            { type: "application/vnd.onda.buffer" },
          ),
        ]),
      );
    }
    bufferFiles.clear();
    for (const [name, asset] of importedBuffers) bufferFiles.set(name, asset);
    projectEditor.replaceProject(project);
    artifact = null;
    compiledModule = null;
    artifactCompilationKey = null;
    runView.clearArtifact(project.entry);
    const url = new URL(window.location.href);
    url.searchParams.delete("example");
    url.hash = "";
    history.replaceState(null, "", url);
    setStatus("Project loaded", "ready");
  } catch (error) {
    runView.setError(error);
    setErrorStatus();
  }
}

function chooseArchivedProject(projectFilePaths) {
  const visiblePaths = projectFilePaths.slice(0, 20);
  const omitted = projectFilePaths.length - visiblePaths.length;
  const choices = visiblePaths
    .map((path, index) => `${index + 1}. ${path}`)
    .join("\n");
  const suffix = omitted > 0
    ? `\n…and ${omitted} more. Enter an exact manifest path to select an omitted project.`
    : "";
  const selection = window.prompt(
    `This archive contains multiple Onda projects. Enter a number or manifest path:\n\n${choices}${suffix}`,
    "1",
  );
  if (selection === null) throw new Error("project selection was cancelled");
  const value = selection.trim();
  const numbered = /^(?:0|[1-9][0-9]*)$/.test(value) ? Number(value) : 0;
  const selected = numbered >= 1 && numbered <= projectFilePaths.length
    ? projectFilePaths[numbered - 1]
    : value;
  if (!projectFilePaths.includes(selected)) {
    throw new Error(`'${value}' is not a project manifest in this archive`);
  }
  return selected;
}

async function downloadProject() {
  if (!compiler || downloadProjectButton.disabled) return;
  downloadProjectButton.disabled = true;
  try {
    const project = projectEditor.compilerProject();
    const options = compileOptions();
    const exportCompilation = await compiler.compileWorkspace(project, options);
    const exportArtifact = exportCompilation.artifact;
    const descriptors = new Map(
      exportArtifact.metadata.metadata.buffers.map((buffer) => [buffer.name, buffer]),
    );
    const encodedBuffers = new Map();
    for (const [name, file] of bufferFiles) {
      const descriptor = descriptors.get(name);
      if (!descriptor) {
        throw new Error(`bound buffer '${name}' is not declared by the current project`);
      }
      const binding = await prepareBufferBinding(descriptor, file, compiler);
      encodedBuffers.set(
        name,
        await encodeOndaBuffer(binding, descriptor.scalar, compiler),
      );
    }
    const image = await compiler.createProjectImage(
      sourceGraphWithWorkspaceDocuments(exportCompilation.sourceGraph, project.sources),
      encodedBuffers,
    );
    const assetFileNames = new Map(
      [...bufferFiles].map(([name, file]) => [name, file.name]),
    );
    const archive = await createProjectArchive(image.bytes, compiler, assetFileNames);
    const stem = project.entry.split("/").at(-1)?.replace(/\.(?:onda|on)$/i, "")
      || "onda-project";
    downloadBytes(archive, `${stem}-project.zip`, "application/zip");
    setStatus("Project downloaded", "ready");
  } catch (error) {
    runView.setError(error);
    setErrorStatus();
  } finally {
    downloadProjectButton.disabled = false;
  }
}

function chooseProjectFile() {
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".zip,.onda,.on,application/zip";
  input.addEventListener("change", () => {
    const [file] = input.files ?? [];
    if (file) void openProjectFile(file);
  }, { once: true });
  input.click();
}

function downloadBytes(bytes, filename, type) {
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

async function shareProject() {
  shareProjectButton.disabled = true;
  try {
    const encoded = await encodeSharedSession({
      ...projectEditor.project(),
    });
    const url = new URL(window.location.href);
    url.search = "";
    url.hash = sharedSessionHash(encoded);
    history.replaceState(null, "", url);

    let copied = false;
    if (navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(url.href);
        copied = true;
      } catch {
        // The explicit copy prompt below works when clipboard permission is unavailable.
      }
    }
    if (!copied) window.prompt("Copy this playground link", url.href);
    shareProjectButton.textContent = copied ? "Copied" : "Link ready";
    setStatus(copied ? "Link copied" : "Link ready", "ready");
    setTimeout(() => {
      shareProjectButton.textContent = "Share";
    }, 1800);
  } catch (error) {
    setErrorStatus();
  } finally {
    shareProjectButton.disabled = false;
  }
}

newProjectButton.addEventListener("click", () => void createNewProject());
openProjectButton.addEventListener("click", chooseProjectFile);
downloadProjectButton.addEventListener("click", () => void downloadProject());
newFileButton.addEventListener("click", () => {
  const path = prompt("New project-relative Onda file", "module.onda")?.trim();
  if (path) editProject(() => projectEditor.add(path));
});
shareProjectButton.addEventListener("click", () => void shareProject());

for (const select of [sampleRateEl, blockSizeEl]) {
  select.addEventListener("change", () => {
    const options = compileOptions();
    saveCompileOptions(options);
    languageServer?.setAnalysisOptions(options);
    setStatus("Ready", "ready");
  });
}
editorFontSizeEl.addEventListener("input", () => {
  const fontSize = validEditorFontSize(editorFontSizeEl.value);
  if (fontSize !== null) applyEditorFontSize(fontSize);
});
editorFontSizeEl.addEventListener("change", () => {
  const fontSize = validEditorFontSize(editorFontSizeEl.value);
  if (fontSize !== null) applyEditorFontSize(fontSize);
  editorFontSizeEl.value = String(editorFontSize);
});
document.addEventListener("keydown", handlePlaygroundShortcut, { capture: true });
document.addEventListener("pointerdown", () => {
  if (context?.state === "suspended") void context.resume().catch(() => {});
}, { capture: true });

window.addEventListener("pagehide", () => {
  scopeSource.stop();
  microphoneInput.close();
  runView.dispose();
  languageServer?.dispose();
  compiler?.dispose().catch(() => {});
});

try {
  sharedSession = await decodeSharedSession(window.location.hash);
  if (sharedSession && !normalizeStoredProject(sharedSession)) {
    throw new Error("the shared playground URL does not contain a valid project");
  }
} catch (error) {
  sharedSession = null;
  sharedSessionError = error;
}
if (!sharedSession && requestedExample) {
  try {
    requestedExampleProject = await loadExampleProject(
      hostedAssets.exampleCatalogUrl,
      requestedExample,
    );
    if (!normalizeStoredProject(requestedExampleProject)) {
      throw new Error("the playground example does not contain a valid project");
    }
  } catch (error) {
    requestedExampleProject = null;
    requestedExampleError = error;
  }
}
initializeEditor(sharedSession, requestedExampleProject);
const loading = loadToolchain().catch((error) => {
  runView.setError(error);
  setErrorStatus();
  reportSmokeResult({ ok: false, error: errorMessage(error) });
});
if (smokeMode) await loading;
