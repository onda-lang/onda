const statusEl = document.querySelector("[data-status]");
const outputEl = document.querySelector("[data-output]");

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = `status ${kind}`.trim();
}

function setOutput(value) {
  outputEl.textContent = JSON.stringify(value, null, 2);
}

function withTimeout(promise, ms, label) {
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      setTimeout(() => {
        reject(new Error(`${label} timed out after ${ms}ms`));
      }, ms);
    }),
  ]);
}

async function reportResult(result) {
  try {
    await fetch("./__result", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(result),
    });
  } catch (_error) {
    // Keep the page result visible even if the local report channel fails.
  }
}

function reportStatus(status, extra = {}) {
  return reportResult({
    status,
    ...extra,
  });
}

function summarize(samples, sampleRate) {
  let maxAbs = 0;
  let sumAbs = 0;
  let zeroCrossings = 0;
  let prev = samples[0] ?? 0;

  for (let i = 0; i < samples.length; i += 1) {
    const value = samples[i];
    const abs = Math.abs(value);
    if (abs > maxAbs) {
      maxAbs = abs;
    }
    sumAbs += abs;
    if (i > 0 && prev <= 0 && value > 0) {
      zeroCrossings += 1;
    }
    prev = value;
  }

  return {
    maxAbs,
    meanAbs: samples.length > 0 ? sumAbs / samples.length : 0,
    zeroCrossings,
    estimatedHz: samples.length > 0 ? (zeroCrossings * sampleRate) / samples.length : 0,
    firstSamples: Array.from(samples.slice(0, 16), (value) => Number(value.toFixed(6))),
  };
}

async function run() {
  await reportStatus("main_started");
  setStatus("Loading wasm and metadata...");
  const [wasmResponse, metaResponse] = await Promise.all([
    fetch("./sine_wasm.wasm"),
    fetch("./sine_wasm.omni.json"),
  ]);
  if (!wasmResponse.ok) {
    throw new Error(`failed to load wasm: ${wasmResponse.status}`);
  }
  if (!metaResponse.ok) {
    throw new Error(`failed to load metadata: ${metaResponse.status}`);
  }

  const wasmBytes = await wasmResponse.arrayBuffer();
  const metadata = await metaResponse.json();
  await reportStatus("assets_loaded", {
    wasmBytes: wasmBytes.byteLength,
  });
  const sampleRate = 48_000;
  const quantum = 128;
  const quanta = 32;
  const length = quantum * quanta;

  setStatus("Rendering offline AudioWorklet graph...");
  const context = new OfflineAudioContext({
    numberOfChannels: 1,
    length,
    sampleRate,
  });
  await reportStatus("context_created");
  await reportStatus("worklet_loading");
  await withTimeout(
    context.audioWorklet.addModule("./omni-sine-processor.js"),
    10_000,
    "audioWorklet.addModule",
  );
  await reportStatus("worklet_loaded");

  const node = new AudioWorkletNode(context, "omni-sine-processor", {
    numberOfInputs: 0,
    numberOfOutputs: 1,
    outputChannelCount: [1],
    channelCount: 1,
    processorOptions: {
      wasmBytes,
      metadata,
      frequency: 440,
    },
  });

  const processorError = new Promise((_, reject) => {
    node.addEventListener("processorerror", () => {
      reject(new Error("AudioWorklet processor crashed"));
    });
  });

  node.connect(context.destination);
  await reportStatus("render_started");
  const rendered = await Promise.race([
    withTimeout(context.startRendering(), 10_000, "OfflineAudioContext.startRendering"),
    processorError,
  ]);
  await reportStatus("render_complete");
  const channel = rendered.getChannelData(0);
  const summary = summarize(channel, sampleRate);
  const ok =
    summary.maxAbs > 0.05 &&
    summary.maxAbs <= 1.1 &&
    summary.zeroCrossings >= 10 &&
    summary.zeroCrossings <= 40;

  const result = {
    ok,
    sampleRate,
    frames: length,
    ...summary,
  };
  setOutput(result);
  setStatus(ok ? "PASS" : "FAIL", ok ? "pass" : "fail");
  document.body.dataset.result = ok ? "pass" : "fail";
  await reportResult(result);
}

run().catch((error) => {
  const result = {
    ok: false,
    error: String(error && error.stack ? error.stack : error),
  };
  setOutput(result);
  setStatus("FAIL", "fail");
  document.body.dataset.result = "fail";
  void reportResult(result);
});
