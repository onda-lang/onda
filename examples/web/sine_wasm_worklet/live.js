const toggleButton = document.querySelector("[data-toggle]");
const statusEl = document.querySelector("[data-status]");
const frequencyEl = document.querySelector("[data-frequency]");
const frequencyValueEl = document.querySelector("[data-frequency-value]");
const gainEl = document.querySelector("[data-gain]");
const gainValueEl = document.querySelector("[data-gain-value]");

let wasmBytes = null;
let metadata = null;
let context = null;
let node = null;
let gainNode = null;

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = `status ${kind}`.trim();
}

function updateFrequencyUi() {
  frequencyValueEl.textContent = frequencyEl.value;
}

function updateGainUi() {
  gainValueEl.textContent = Number(gainEl.value).toFixed(2);
}

async function loadAssets() {
  setStatus("Loading assets...");
  const [wasmResponse, metaResponse] = await Promise.all([
    fetch("./sine_wasm.wasm"),
    fetch("./sine_wasm.onda.json"),
  ]);
  if (!wasmResponse.ok) {
    throw new Error(`failed to load wasm: ${wasmResponse.status}`);
  }
  if (!metaResponse.ok) {
    throw new Error(`failed to load metadata: ${metaResponse.status}`);
  }
  wasmBytes = await wasmResponse.arrayBuffer();
  metadata = await metaResponse.json();
}

function postFrequency() {
  if (!node) {
    return;
  }
  node.port.postMessage({
    type: "frequency",
    value: Number(frequencyEl.value),
  });
}

function applyGain() {
  if (!gainNode) {
    return;
  }
  gainNode.gain.value = Number(gainEl.value);
}

async function startAudio() {
  if (!wasmBytes || !metadata) {
    throw new Error("assets are not loaded");
  }

  setStatus("Starting audio...");
  const sampleRate = Number(metadata.compile?.sample_rate ?? 48_000);
  context = new AudioContext({ sampleRate });
  await context.audioWorklet.addModule("./onda-sine-processor.js");

  node = new AudioWorkletNode(context, "onda-sine-processor", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [1],
    channelCount: 1,
    processorOptions: {
      wasmBytes,
      metadata,
      frequency: Number(frequencyEl.value),
    },
  });

  gainNode = new GainNode(context, {
    gain: Number(gainEl.value),
  });

  node.connect(gainNode);
  gainNode.connect(context.destination);
  await context.resume();
  setStatus(`Running at ${context.sampleRate} Hz`, "ready");
  toggleButton.textContent = "Stop Audio";
}

async function stopAudio() {
  setStatus("Stopping audio...");
  if (node) {
    node.disconnect();
    node = null;
  }
  if (gainNode) {
    gainNode.disconnect();
    gainNode = null;
  }
  if (context) {
    await context.close();
    context = null;
  }
  setStatus("Ready", "ready");
  toggleButton.textContent = "Start Audio";
}

toggleButton.addEventListener("click", async () => {
  toggleButton.disabled = true;
  try {
    if (context) {
      await stopAudio();
    } else {
      await startAudio();
    }
  } catch (error) {
    setStatus(String(error && error.message ? error.message : error), "fail");
  } finally {
    toggleButton.disabled = false;
  }
});

frequencyEl.addEventListener("input", () => {
  updateFrequencyUi();
  postFrequency();
});

gainEl.addEventListener("input", () => {
  updateGainUi();
  applyGain();
});

updateFrequencyUi();
updateGainUi();

loadAssets()
  .then(() => {
    toggleButton.disabled = false;
    toggleButton.textContent = "Start Audio";
    setStatus("Ready", "ready");
  })
  .catch((error) => {
    toggleButton.disabled = true;
    setStatus(String(error && error.message ? error.message : error), "fail");
  });
