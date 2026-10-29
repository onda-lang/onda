import init, { CanvasUi } from "./pkg/onda_ui.js";
import { browserBridge } from "./browser.js";
import { computerKey, wireJson } from "./protocol.js";
import { createTextInput } from "./text-input.js";

const canvas = document.getElementById("onda-ui");
const status = document.getElementById("status");
const vscode =
  typeof acquireVsCodeApi === "function" ? acquireVsCodeApi() : null;
const send = (message) => {
  if (message.type === "copyText") {
    copyText(message.text);
    return;
  }
  if (message.type === "viewState" && vscode) {
    vscode.setState(message.state);
    return;
  }
  if (browser) browser.send(message);
  else if (vscode) vscode.postMessage(message);
  else if (window.ipc) window.ipc.postMessage(JSON.stringify(message));
  else
    window.dispatchEvent(new CustomEvent("ondaCommand", { detail: message }));
};
let ui;
let frame = 0;
let viewStateTimer = 0;
let repaintTimer = 0;
let fontRetryTimer = 0;
let suspended = false;
let pendingState = [];
let pendingScope;
let pendingReady;
let renderError;
let sourcePath;
const pendingValues = { params: new Map(), inputControls: new Map() };
let scopeConfiguration = "";
const fallbackFonts = new Set();
const fontRetries = new Map();
const browser = browserBridge(canvas, {
  message: receive,
  target: (x, y) => ui?.buffer_target(x, y),
  onInput: (event) => input(event),
});
const editor = createTextInput(canvas, {
  snapshot: () => ui ? JSON.parse(ui.text_edit()) : null,
  input,
  gesture: (event, stop = false) => browser?.gesture(event, stop),
});
let themeMode = "auto";
const mediaTheme = matchMedia("(prefers-color-scheme: dark)");
function resolvedTheme(mode) {
  if (mode !== "auto") return mode;
  if (vscode) {
    if (document.body.classList.contains("vscode-light") || document.body.classList.contains("vscode-high-contrast-light")) return "light";
    if (document.body.classList.contains("vscode-dark") || document.body.classList.contains("vscode-high-contrast")) return "dark";
  }
  return mediaTheme.matches ? "dark" : "light";
}
function refreshTheme() {
  if (ui && themeMode === "auto") receive({ type:"state", state:{themeMode:"auto"} });
}
mediaTheme.addEventListener("change", refreshTheme);
new MutationObserver(refreshTheme).observe(document.body, {attributes:true, attributeFilter:["class"]});

function report(error) {
  if (renderError) return;
  status.hidden = false;
  console.error(error);
  renderError = `Could not render Onda UI.\n${error?.message || error}`;
  status.textContent = renderError;
  send({ type: "runViewError", message: renderError });
}
async function copyText(text) {
  try {
    if (!navigator.clipboard) throw new Error("Clipboard access is unavailable");
    await navigator.clipboard.writeText(text);
  } catch (error) {
    receive({ type: "state", state: {
      error: `Could not copy clipboard text: ${error?.message || error}`,
    } });
  }
}
function commands() {
  const result = JSON.parse(ui.commands());
  for (const command of result) send(command);
  clearTimeout(viewStateTimer);
  const delay = ui.view_state_delay();
  viewStateTimer = delay === undefined ? 0 : setTimeout(() => {
    viewStateTimer = 0;
    if (ui) commands();
  }, Math.ceil(delay));
  scopeConfig();
  editor.sync();
  return result;
}
function schedule() {
  if (frame || suspended || document.hidden || !ui) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    try {
      flushValues();
      if (pendingScope) {
        const scope = pendingScope;
        pendingScope = undefined;
        if (scope.packet instanceof Uint8Array) {
          ui.scope_packet(scope.packet);
        } else if (typeof scope.packet === "string") {
          const bytes =
            typeof Uint8Array.fromBase64 === "function"
              ? Uint8Array.fromBase64(scope.packet)
              : Uint8Array.from(atob(scope.packet), (char) =>
                  char.charCodeAt(0),
                );
          ui.scope_packet(bytes);
        } else {
          ui.scope_samples(
            scope.channels || 0,
            Float32Array.from(scope.samples || []),
          );
        }
      }
      canvas.style.cursor = ui.cursor();
      loadFallbackFonts();
      if (!ui.render()) schedule();
      else if (pendingReady) {
        send(pendingReady);
        pendingReady = undefined;
      }
      clearTimeout(repaintTimer);
      const delay = ui.repaint_delay();
      repaintTimer = delay === undefined ? 0 : setTimeout(schedule, Math.ceil(delay));
      editor.sync();
    } catch (error) {
      report(error);
    }
  });
}
function loadFallbackFonts() {
  clearTimeout(fontRetryTimer);
  fontRetryTimer = 0;
  const now = performance.now();
  let nextRetry = Infinity;
  // The renderer only reports missing fonts when its scene revision changes.
  const names = new Set([...fontRetries.keys(), ...JSON.parse(ui.fallback_fonts())]);
  for (const name of names) {
    if (fallbackFonts.has(name)) continue;
    const retry = fontRetries.get(name);
    if (retry && now < retry.at) {
      nextRetry = Math.min(nextRetry, retry.at);
      continue;
    }
    fallbackFonts.add(name);
    (async () => {
      const response = await fetch(new URL(`./pkg/fonts/${name}`, import.meta.url));
      if (!response.ok) throw new Error(`Could not load font ${name}: ${response.status}`);
      const bytes = new Uint8Array(await response.arrayBuffer());
      if (!ui) return;
      ui.load_font(name, bytes);
      fontRetries.delete(name);
      schedule();
    })().catch(error => {
      fallbackFonts.delete(name);
      const delay = Math.min((retry?.delay || 500) * 2, 30000);
      fontRetries.set(name, {delay, at:performance.now() + delay});
      console.warn(error);
      schedule();
    });
  }
  if (Number.isFinite(nextRetry))
    fontRetryTimer = setTimeout(schedule, Math.max(0, nextRetry - performance.now()));
}
function flushValues() {
  const state = {};
  for (const [bank, values] of Object.entries(pendingValues)) {
    if (values.size) {
      state[bank] = [...values.values()];
      values.clear();
    }
  }
  if (Object.keys(state).length)
    ui.message(wireJson({ type: "parameterValues", state }));
}
function receive(message) {
  if (!message) return;
  if (message.type === "requestRunViewReady") {
    if (renderError) send({ type: "runViewError", message: renderError });
    else if (ui) send({ type: "webviewReady" });
    return;
  }
  if (message.type === "computerKey") {
    const key = computerKey(message);
    if (key) input({ type: "computerKey", key, pressed: message.pressed });
    return;
  }
  if (message.type === "releaseVirtualMidiNotes") {
    input({ type: "releaseComputerKeys" });
    return;
  }
  if (
    ![
      "state",
      "parameterValues",
      "scopeData",
      "midiActivity",
      "bufferFileDrag",
      "sourceFileDrag",
    ].includes(message.type)
  )
    return;
  if (message.type === "scopeData") {
    pendingScope = message;
    schedule();
    return;
  }
  if (!ui) {
    pendingState.push(message);
    return;
  }
  try {
    if (message.type === "parameterValues") {
      for (const [bank, values] of Object.entries(pendingValues)) {
        for (const value of message.state?.[bank] || [])
          if (typeof value.name === "string") values.set(value.name, value);
      }
    } else {
      if (vscode && message.type === "state") {
        const state = { ...message.state, supportsViewState: true };
        const saved = vscode.getState();
        if (!state.viewState && saved?.path === state.path) state.viewState = saved;
        message = { ...message, state };
      }
      const state = message.state;
      if (state?.themeMode) {
        themeMode = state.themeMode;
        const theme = resolvedTheme(themeMode);
        document.documentElement.dataset.theme = theme;
        message = { ...message, state: { ...state, themeMode: theme } };
      }
      if (
        state?.viewState?.reset ||
        (Object.hasOwn(state || {}, "path") && state.path !== sourcePath)
      )
        pendingScope = undefined;
      if (Object.hasOwn(state || {}, "path")) sourcePath = state.path;
      for (const [bank, values] of Object.entries(pendingValues))
        if (Array.isArray(message.state?.[bank])) values.clear();
      ui.message(wireJson(message));
      commands();
      if (message.type === "state")
        pendingReady = {
          type: "runViewReady",
          readyId: message.state?.viewState?.readyId ?? pendingReady?.readyId,
        };
    }
    schedule();
  } catch (error) {
    report(error);
  }
}
window._onHostMessage = receive;
if (!browser)
  window.addEventListener("message", (event) => receive(event.data));

function input(event) {
  return updateInput(() => ui.input(JSON.stringify(event)))?.commands;
}
function updateInput(action) {
  if (!ui) return;
  try {
    flushValues();
    const value = action();
    canvas.style.cursor = ui.cursor();
    const result = commands();
    schedule();
    return { value, commands: result };
  } catch (error) {
    report(error);
  }
}
canvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || (event.pointerType !== "touch" && !event.isPrimary)) return;
  canvas.focus();
  canvas.setPointerCapture(event.pointerId);
  if (event.pointerType === "touch") event.preventDefault();
  const result = event.pointerType === "touch" ? input({
    type: "touchStart",
    id: event.pointerId,
    x: event.offsetX,
    y: event.offsetY,
  }) : undefined;
  browser?.gesture(
    event,
    ui?.pointer_stops(event.offsetX,event.offsetY) || result?.some((command) => command.type === "stop"),
  );
});
// Mouse compatibility events carry the platform's click count; pointerdown does not.
canvas.addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.sourceCapabilities?.firesTouchEvents) return;
  input({ type: "pointerDown", x: event.offsetX, y: event.offsetY, clicks: event.detail });
});
canvas.addEventListener("pointermove", (event) => {
  if (event.pointerType !== "touch" && !event.isPrimary) return;
  input({
    type: event.pointerType === "touch" ? "touchMove" : "pointerMove",
    id: event.pointerId,
    x: event.offsetX,
    y: event.offsetY,
    fine: event.shiftKey,
  });
});
canvas.addEventListener("pointerleave", () => input({ type: "pointerLeave" }));
canvas.addEventListener("pointerup", (event) => {
  if (event.button === 0 && (event.isPrimary || event.pointerType === "touch")) {
    const result = input({ type:event.pointerType === "touch" ? "touchEnd" : "pointerUp", id:event.pointerId });
    browser?.gesture(event,result?.some(command => command.type === "stop"));
  }
});
canvas.addEventListener("pointercancel", event => {
  if (event.pointerType === "touch") input({ type:"touchCancel", id:event.pointerId });
  else if (event.isPrimary) input({ type: "focusLost" });
});
canvas.addEventListener("lostpointercapture", event =>
  input({ type:event.pointerType === "touch" ? "touchCancel" : "pointerUp", id:event.pointerId }),
);
canvas.addEventListener("blur", event => {
  if (event.relatedTarget !== editor.element) input({ type: "focusLost" });
});
window.addEventListener("blur", () => input({ type: "focusLost" }));
function wheel(event) {
  if (event.ctrlKey || event.metaKey) return;
  const horizontal = event.shiftKey || Math.abs(event.deltaX) > Math.abs(event.deltaY);
  const delta = (event.shiftKey ? event.deltaY : horizontal ? event.deltaX : event.deltaY) *
    (event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? canvas.clientHeight : 1);
  const rect = canvas.getBoundingClientRect();
  const result = updateInput(() => {
    ui.input(JSON.stringify({type:"pointerMove",x:event.clientX-rect.left,y:event.clientY-rect.top,fine:event.shiftKey}));
    return ui.scroll(delta, horizontal);
  });
  if (result?.value) event.preventDefault();
}
for (const target of [canvas, editor.element])
  target.addEventListener("wheel", wheel, { passive: false });
function keyInput(event) {
  if (
    (event.ctrlKey || event.metaKey) &&
    ["s", ".", "enter"].includes(event.key.toLowerCase())
  ) {
    event.preventDefault();
    if (event.repeat) return;
    return input({ type: "key", key: event.key === "." ? "Stop" : "Play" });
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
    event.preventDefault();
    return input({ type: "key", key: "SelectAll" });
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v")
    return;
  if (
    [
      "Enter",
      "Escape",
      "Backspace",
      "Delete",
      "ArrowLeft",
      "ArrowRight",
      "ArrowUp",
      "ArrowDown",
      "Home",
      "End",
      "Tab",
    ].includes(event.key)
  ) {
    event.preventDefault();
    return input({ type: "key", key: event.key, shift: event.shiftKey });
  } else if (!event.ctrlKey && !event.metaKey && event.key.length === 1) {
    event.preventDefault();
    if (!event.repeat) input({ type: "computerKey", key: computerKey(event), pressed: true });
    return input({ type: "text", text: event.key });
  }
}
canvas.addEventListener("keydown", (event) => {
  if (event.defaultPrevented) return;
  const result = keyInput(event);
  browser?.gesture(event, result?.some(command => command.type === "stop"));
});
document.addEventListener("keyup", (event) =>
  input({ type: "computerKey", key: computerKey(event), pressed: false }),
);
canvas.addEventListener("paste", (event) => {
  event.preventDefault();
  input({ type: "text", text: event.clipboardData.getData("text/plain") });
});
function resize() {
  if (!ui) return;
  const scale = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(canvas.clientWidth * scale));
  const height = Math.max(1, Math.round(canvas.clientHeight * scale));
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  ui.resize(width, height, scale);
  editor.sync();
  scopeConfig();
  schedule();
}
function scopeConfig() {
  if (!ui) return;
  const configuration = {
    type: "scopeConfig",
    columns: ui.scope_columns(),
    visible: !suspended && !document.hidden && ui.scope_enabled(),
  };
  const signature = JSON.stringify(configuration);
  if (signature !== scopeConfiguration) {
    scopeConfiguration = signature;
    send(configuration);
  }
}
new ResizeObserver(resize).observe(canvas);
window.addEventListener("resize", resize);
function watchDisplayScale() {
  // Moving between displays can change pixel density without changing CSS size.
  matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`).addEventListener(
    "change",
    () => {
      resize();
      watchDisplayScale();
    },
    { once: true },
  );
}
watchDisplayScale();
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    clearTimeout(repaintTimer);
    clearTimeout(fontRetryTimer);
    input({ type: "focusLost" });
  }
  resize();
});
window.addEventListener("pagehide", (event) => {
  suspended = true;
  input({ type: "focusLost" });
  clearTimeout(viewStateTimer);
  clearTimeout(repaintTimer);
  clearTimeout(fontRetryTimer);
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  if (ui && !event.persisted) {
    ui.free();
    ui = undefined;
  }
});
window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    suspended = false;
    resize();
  }
});

try {
  await init();
  const forceWebgl =
    new URLSearchParams(location.search).get("backend") === "webgl";
  const adapter =
    !forceWebgl && navigator.gpu
      ? await navigator.gpu.requestAdapter().catch(() => null)
      : null;
  ui = await CanvasUi.create(
    canvas,
    Boolean(adapter) && adapter.info?.isFallbackAdapter !== true,
  );
  canvas.dataset.backend =
    adapter && adapter.info?.isFallbackAdapter !== true ? "webgpu" : "webgl";
  status.hidden = true;
  resize();
  for (const message of pendingState) receive(message);
  pendingState = [];
  send({ type: "webviewReady" });
  schedule();
} catch (error) {
  report(error);
}
