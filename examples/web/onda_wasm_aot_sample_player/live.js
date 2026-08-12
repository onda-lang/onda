import { loadProcessorArtifactFiles } from "./artifact.js";
import { createOndaAudioProcessor } from "./onda-webaudio.js";

const audioButton = document.querySelector("[data-audio]");
const playButton = document.querySelector("[data-play]");
const stopPlaybackButton = document.querySelector("[data-stop-playback]");
const speedInput = document.querySelector("[data-speed]");
const speedValue = document.querySelector("[data-speed-value]");
const statusElement = document.querySelector("[data-status]");
const summaryElement = document.querySelector("[data-summary]");

let artifact = null;
let context = null;
let processor = null;

function setStatus(message, kind = "") {
  statusElement.textContent = message;
  statusElement.className = `status ${kind}`.trim();
}

function errorMessage(error) {
  return String(error?.message ?? error);
}

async function loadArtifact() {
  const [wasmResponse, metadataResponse] = await Promise.all([
    fetch("./sample-player.wasm"),
    fetch("./sample-player.onda.json"),
  ]);
  if (!wasmResponse.ok || !metadataResponse.ok) {
    throw new Error("sample-player artifact is missing; run the build script first");
  }
  artifact = await loadProcessorArtifactFiles(
    await wasmResponse.arrayBuffer(),
    await metadataResponse.text(),
  );
  const metadata = artifact.metadata;
  const optimizationLevel = Number(metadata.optimization?.level ?? 4);
  summaryElement.textContent =
    `${artifact.wasm.byteLength.toLocaleString()} Wasm bytes; ` +
    `${metadata.compile.sample_rate} Hz; ${metadata.compile.block_size}-frame compile blocks; ` +
    `Binaryen O${optimizationLevel}.`;
  audioButton.disabled = false;
  setStatus("Precompiled artifact verified and ready.", "ready");
}

async function decodeClip(audioContext) {
  const response = await fetch("./impulse.wav");
  if (!response.ok) throw new Error(`failed to load impulse.wav: ${response.status}`);
  const audioBuffer = await audioContext.decodeAudioData(await response.arrayBuffer());
  const frames = audioBuffer.length;
  const channels = audioBuffer.numberOfChannels;
  const data = new Float32Array(frames * channels);
  for (let channel = 0; channel < channels; channel += 1) {
    const source = audioBuffer.getChannelData(channel);
    for (let frame = 0; frame < frames; frame += 1) {
      data[frame * channels + channel] = source[frame];
    }
  }
  return { data, frames, channels, sampleRate: audioBuffer.sampleRate };
}

async function startAudio() {
  if (!artifact || context) return;
  setStatus("Loading impulse.wav and starting AudioWorklet…");
  context = new AudioContext({ sampleRate: artifact.metadata.compile.sample_rate });
  try {
    const clip = await decodeClip(context);
    processor = await createOndaAudioProcessor(context, artifact, {
      workletUrl: "./onda-wasm-processor.js",
      params: { speed: Number(speedInput.value) },
      buffers: { clip },
    });
    processor.node.connect(context.destination);
    await context.resume();
    await processor.trigger("play", { enabled: true });
    audioButton.textContent = "Stop audio";
    playButton.disabled = false;
    stopPlaybackButton.disabled = false;
    setStatus(
      `Playing ${clip.frames} frames × ${clip.channels} channel(s) from impulse.wav at ${clip.sampleRate} Hz.`,
      "ready",
    );
  } catch (error) {
    await stopAudio();
    throw error;
  }
}

async function stopAudio() {
  if (!context) return;
  processor?.node.disconnect();
  processor?.close();
  processor = null;
  await context.close();
  context = null;
  audioButton.textContent = "Start audio";
  playButton.disabled = true;
  stopPlaybackButton.disabled = true;
  setStatus("Precompiled artifact verified and ready.", "ready");
}

audioButton.addEventListener("click", async () => {
  audioButton.disabled = true;
  try {
    if (context) await stopAudio();
    else await startAudio();
  } catch (error) {
    setStatus(errorMessage(error), "fail");
  } finally {
    audioButton.disabled = !artifact;
  }
});

playButton.addEventListener("click", () => {
  processor?.trigger("play", { enabled: true }).catch((error) =>
    setStatus(errorMessage(error), "fail")
  );
});

stopPlaybackButton.addEventListener("click", () => {
  processor?.trigger("play", { enabled: false }).catch((error) =>
    setStatus(errorMessage(error), "fail")
  );
});

speedInput.addEventListener("input", () => {
  speedValue.value = Number(speedInput.value).toFixed(2);
  processor?.setParam("speed", Number(speedInput.value)).catch((error) =>
    setStatus(errorMessage(error), "fail")
  );
});

loadArtifact().catch((error) => {
  setStatus(errorMessage(error), "fail");
});
