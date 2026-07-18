import {
  createCompiler,
  createProcessorArtifactFiles,
} from "@onda-lang/wasm-compiler";
import {
  createOndaAudioProcessor,
  flattenedAudioChannelCount,
} from "@onda-lang/webaudio";

const compileButton = document.querySelector("[data-compile]");
const toggleButton = document.querySelector("[data-toggle]");
const resetButton = document.querySelector("[data-reset]");
const exportButton = document.querySelector("[data-export]");
const statusEl = document.querySelector("[data-status]");
const diagnosticsEl = document.querySelector("[data-diagnostics]");
const sourceEl = document.querySelector("[data-source]");
const sampleRateEl = document.querySelector("[data-sample-rate]");
const blockSizeEl = document.querySelector("[data-block-size]");
const gainEl = document.querySelector("[data-gain]");
const paramsEl = document.querySelector("[data-params]");
const eventsEl = document.querySelector("[data-events]");
const summaryEl = document.querySelector("[data-summary]");

const smokeMode = new URLSearchParams(window.location.search).has("smoke");
const sourceStorageKey = "onda.browser-playground.source";

let compiler = null;
let artifact = null;
let context = null;
let node = null;
let audioProcessor = null;
let gainNode = null;
let compiling = false;
let paramValues = {};

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = `status ${kind}`.trim();
}

function reportSmokeResult(result) {
  if (!smokeMode) return;
  fetch("./__result", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(result),
  }).catch(() => {});
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  return String(error?.message ?? error);
}

function decodeScalar(scalar) {
  const value = scalar?.value;
  if ((scalar?.type !== "f32" && scalar?.type !== "f64") || typeof value !== "string") {
    return value;
  }
  const width = scalar.type === "f32" ? 32 : 64;
  const digits = value.startsWith("0x") ? value.slice(2) : "";
  if (digits.length !== width / 4) return Number.NaN;
  const bytes = new ArrayBuffer(width / 8);
  const view = new DataView(bytes);
  if (width === 32) {
    view.setUint32(0, Number.parseInt(digits, 16), false);
    return view.getFloat32(0, false);
  }
  view.setBigUint64(0, BigInt(value), false);
  return view.getFloat64(0, false);
}

function decodeConstant(constant) {
  if (constant?.kind === "scalar") return decodeScalar(constant.data);
  if (constant?.kind === "aggregate") {
    return constant.data.map((entry) => decodeConstant(entry));
  }
  return undefined;
}

function parseScalar(text, scalar) {
  if (scalar === "bool") return text === true || text === "true" || text === "1";
  if (scalar === "i64") return BigInt(String(text).trim());
  if (scalar === "i32") return Math.trunc(Number(text));
  if (scalar === "f32") return Math.fround(Number(text));
  return Number(text);
}

function compilerDiagnostics(error) {
  if (Array.isArray(error?.diagnostics)) return error.diagnostics;
  const candidates = [
    typeof error === "string" ? error : null,
    typeof error?.message === "string" ? error.message : null,
  ].filter(Boolean);
  for (const candidate of candidates) {
    try {
      const parsed = JSON.parse(candidate);
      if (Array.isArray(parsed)) return parsed;
    } catch {
      // wasm-bindgen may wrap a string-valued exception; fall through.
    }
  }
  return null;
}

function showCompileError(error) {
  const diagnostics = compilerDiagnostics(error);
  if (!diagnostics) {
    diagnosticsEl.textContent = errorMessage(error);
    return;
  }
  diagnosticsEl.textContent = diagnostics
    .map((diagnostic) => {
      const location = diagnostic.file
        ? `${diagnostic.file}:${diagnostic.line || 0}:${diagnostic.column || 0}`
        : diagnostic.line
          ? `${diagnostic.line}:${diagnostic.column || 0}`
          : diagnostic.stage;
      const trace = Array.isArray(diagnostic.trace) && diagnostic.trace.length
        ? `\n  ${diagnostic.trace.join("\n  ")}`
        : "";
      return `${location} [${diagnostic.stage}] ${diagnostic.message}${trace}`;
    })
    .join("\n\n");
}

function defaultBuffers(metadata) {
  const frames = Number(metadata.compile.block_size);
  const sampleRate = Number(metadata.compile.sample_rate);
  return Object.fromEntries(
    metadata.metadata.buffers.map((buffer) => {
      const channels = Number(buffer.static_channels ?? 1);
      return [
        buffer.name,
        {
          data: new Array(frames * channels).fill(0),
          frames,
          channels,
          sampleRate,
        },
      ];
    }),
  );
}

function postParam(name, value) {
  paramValues[name] = value;
  audioProcessor?.setParam(name, value).catch((error) =>
    setStatus(errorMessage(error), "fail")
  );
}

function renderParams(metadata) {
  paramsEl.replaceChildren();
  paramValues = {};
  const params = metadata.metadata.params;
  if (!params.length) {
    paramsEl.innerHTML = '<span class="empty">This program has no parameters.</span>';
    return;
  }

  for (const param of params) {
    const wrapper = document.createElement("label");
    wrapper.className = "dynamic-control";
    const title = document.createElement("span");
    title.textContent = `${param.name} · ${param.type}`;
    wrapper.append(title);

    const initial = decodeConstant(param.default);
    paramValues[param.name] = initial;
    let input;

    if (param.is_array) {
      input = document.createElement("input");
      input.type = "text";
      input.value = Array.from(initial ?? []).join(", ");
      input.addEventListener("change", () => {
        try {
          const values = input.value
            .split(",")
            .map((entry) => parseScalar(entry, param.scalar));
          if (values.length !== Number(param.array_length)) {
            throw new Error(`expected ${param.array_length} values`);
          }
          postParam(param.name, values);
          input.setCustomValidity("");
        } catch (error) {
          input.setCustomValidity(errorMessage(error));
          input.reportValidity();
        }
      });
    } else if (param.scalar === "bool") {
      input = document.createElement("input");
      input.type = "checkbox";
      input.checked = Boolean(initial);
      input.addEventListener("change", () =>
        postParam(param.name, input.checked)
      );
    } else {
      const minimum = decodeScalar(param.range?.min);
      const maximum = decodeScalar(param.range?.max);
      const ranged = Number.isFinite(minimum) && Number.isFinite(maximum);
      input = document.createElement("input");
      const exactInteger = param.scalar === "i64";
      input.type = exactInteger ? "text" : ranged ? "range" : "number";
      if (ranged && !exactInteger) {
        input.min = String(minimum);
        input.max = String(maximum);
        input.step = String(
          param.scalar.startsWith("i")
            ? 1
            : Math.max((maximum - minimum) / 1000, Number.EPSILON),
        );
      } else if (!exactInteger) {
        input.step = param.scalar.startsWith("i") ? "1" : "any";
      }
      input.value = String(initial ?? 0);
      const valueEl = document.createElement("small");
      valueEl.textContent = input.value;
      input.addEventListener("input", () => {
        try {
          const value = parseScalar(input.value, param.scalar);
          valueEl.textContent = String(value);
          postParam(param.name, value);
        } catch (error) {
          valueEl.textContent = errorMessage(error);
        }
      });
      wrapper.append(input, valueEl);
      paramsEl.append(wrapper);
      continue;
    }

    wrapper.append(input);
    paramsEl.append(wrapper);
  }
}

function renderEvents(metadata) {
  eventsEl.replaceChildren();
  const events = metadata.metadata.events;
  if (!events.length) {
    eventsEl.innerHTML = '<span class="empty">This program has no events.</span>';
    return;
  }

  for (const event of events) {
    const wrapper = document.createElement("div");
    wrapper.className = "dynamic-control";
    const title = document.createElement("span");
    title.textContent = event.name;
    const defaults = Object.fromEntries(
      event.params.map((param) => [param.name, decodeConstant(param.default)]),
    );
    const editor = document.createElement("input");
    editor.type = "text";
    editor.value = event.params.length
      ? JSON.stringify(
          defaults,
          (_key, value) => typeof value === "bigint" ? value.toString() : value,
        )
      : "{}";
    editor.disabled = event.params.length === 0;
    const trigger = document.createElement("button");
    trigger.type = "button";
    trigger.textContent = "Trigger";
    trigger.addEventListener("click", () => {
      if (!node) {
        setStatus("Start audio before triggering events.", "fail");
        return;
      }
      try {
        const values = event.params.length ? JSON.parse(editor.value) : {};
        audioProcessor.trigger(event.name, values).catch((error) =>
          setStatus(errorMessage(error), "fail")
        );
        editor.setCustomValidity("");
      } catch (error) {
        editor.setCustomValidity(errorMessage(error));
        editor.reportValidity();
      }
    });
    wrapper.append(title, editor, trigger);
    eventsEl.append(wrapper);
  }
}

function renderArtifact(artifactValue) {
  const metadata = artifactValue.metadata;
  renderParams(metadata);
  renderEvents(metadata);
  const inputChannels = flattenedAudioChannelCount(metadata.metadata.inputs);
  const outputChannels = flattenedAudioChannelCount(metadata.metadata.outputs);
  summaryEl.textContent =
    `Schema ${metadata.mir_schema_version}; ${artifactValue.wasm.byteLength.toLocaleString()} Wasm bytes; ` +
    `${inputChannels} input channel(s), ${outputChannels} output channel(s), ` +
    `${metadata.metadata.params.length} parameter(s), ${metadata.metadata.events.length} event(s).`;
}

async function compileSource({ restart = false } = {}) {
  if (!compiler || compiling) return;
  compiling = true;
  compileButton.disabled = true;
  toggleButton.disabled = true;
  resetButton.disabled = true;
  exportButton.disabled = true;
  diagnosticsEl.textContent = "";
  setStatus("Compiling source to WebAssembly…");
  const wasRunning = Boolean(context);
  try {
    if (wasRunning) await stopAudio();
    const sampleRate = Number(sampleRateEl.value);
    const blockSize = Number(blockSizeEl.value);
    artifact = await compiler.compileSource(sourceEl.value, {
      sampleRate,
      blockSize,
    });
    localStorage.setItem(sourceStorageKey, sourceEl.value);
    renderArtifact(artifact);
    setStatus("Compiled and ready.", "ready");
    toggleButton.disabled = false;
    exportButton.disabled = false;
    if (restart || wasRunning) await startAudio();
    reportSmokeResult({
      ok: true,
      wasmBytes: artifact.wasm.byteLength,
      backend: artifact.metadata.backend,
      schemaVersion: artifact.metadata.mir_schema_version,
    });
  } catch (error) {
    artifact = null;
    showCompileError(error);
    setStatus("Compilation failed.", "fail");
    reportSmokeResult({ ok: false, error: errorMessage(error) });
  } finally {
    compiling = false;
    compileButton.disabled = false;
    toggleButton.disabled = !artifact;
    exportButton.disabled = !artifact;
    resetButton.disabled = !node;
  }
}

async function startAudio() {
  if (!artifact) throw new Error("compile a program first");
  if (context) return;
  setStatus("Starting AudioWorklet…");

  const metadata = artifact.metadata;
  const sampleRate = Number(metadata.compile.sample_rate);
  const inputChannels = flattenedAudioChannelCount(metadata.metadata.inputs);
  const outputChannels = flattenedAudioChannelCount(metadata.metadata.outputs);
  if (inputChannels > 32 || outputChannels > 32) {
    throw new Error("Web Audio supports at most 32 flattened channels per node");
  }

  context = new AudioContext({ sampleRate });
  try {
    audioProcessor = await createOndaAudioProcessor(context, artifact, {
      params: paramValues,
      buffers: defaultBuffers(metadata),
    });
    node = audioProcessor.node;

    if (outputChannels) {
      gainNode = new GainNode(context, { gain: Number(gainEl.value) });
      node.connect(gainNode);
      gainNode.connect(context.destination);
    }
    await context.resume();
  } catch (error) {
    audioProcessor?.close();
    audioProcessor = null;
    node = null;
    gainNode = null;
    await context.close();
    context = null;
    throw error;
  }
  toggleButton.textContent = "Stop audio";
  resetButton.disabled = false;
  setStatus(`Running at ${context.sampleRate} Hz.`, "ready");
}

async function stopAudio() {
  if (!context) return;
  setStatus("Stopping audio…");
  node?.disconnect();
  audioProcessor?.close();
  audioProcessor = null;
  gainNode?.disconnect();
  node = null;
  gainNode = null;
  await context.close();
  context = null;
  toggleButton.textContent = "Start audio";
  resetButton.disabled = true;
  setStatus(artifact ? "Compiled and ready." : "Ready.", "ready");
}

async function loadToolchain() {
  setStatus("Loading Onda compiler and Binaryen…");
  compiler = await createCompiler({ worker: true });
  const [sourceResponse] = await Promise.all([
    fetch("./default.onda"),
  ]);
  if (!sourceResponse.ok) {
    throw new Error(`failed to load example source: ${sourceResponse.status}`);
  }
  const exampleSource = await sourceResponse.text();
  sourceEl.value = smokeMode
    ? exampleSource
    : localStorage.getItem(sourceStorageKey) ?? exampleSource;
  compileButton.disabled = false;
  await compileSource();
}

compileButton.addEventListener("click", () => compileSource());
toggleButton.addEventListener("click", async () => {
  toggleButton.disabled = true;
  try {
    if (context) await stopAudio();
    else await startAudio();
  } catch (error) {
    setStatus(errorMessage(error), "fail");
  } finally {
    toggleButton.disabled = !artifact;
  }
});
resetButton.addEventListener("click", () => {
  audioProcessor?.reset().then(
    () => setStatus("DSP state reset.", "ready"),
    (error) => setStatus(errorMessage(error), "fail"),
  );
});
exportButton.addEventListener("click", async () => {
  if (!artifact) return;
  exportButton.disabled = true;
  try {
    const files = await createProcessorArtifactFiles(artifact, {
      baseName: "onda-processor",
    });
    downloadFile(files.wasm.name, files.wasm.bytes, files.wasm.mediaType);
    downloadFile(files.metadata.name, files.metadata.text, files.metadata.mediaType);
    setStatus("Exported reusable Wasm module and descriptor.", "ready");
  } catch (error) {
    setStatus(errorMessage(error), "fail");
  } finally {
    exportButton.disabled = !artifact;
  }
});

function downloadFile(name, contents, mediaType) {
  const url = URL.createObjectURL(new Blob([contents], { type: mediaType }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
gainEl.addEventListener("input", () => {
  if (gainNode) gainNode.gain.value = Number(gainEl.value);
});
sourceEl.addEventListener("keydown", (event) => {
  if (event.key === "Tab") {
    event.preventDefault();
    const start = sourceEl.selectionStart;
    const end = sourceEl.selectionEnd;
    sourceEl.setRangeText("  ", start, end, "end");
  }
  if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
    event.preventDefault();
    compileSource();
  }
});

loadToolchain().catch((error) => {
  compileButton.disabled = true;
  toggleButton.disabled = true;
  exportButton.disabled = true;
  showCompileError(error);
  setStatus(errorMessage(error), "fail");
  reportSmokeResult({ ok: false, error: errorMessage(error) });
});
