import { readFile } from "node:fs/promises";

import { loadProcessorArtifactFiles } from "./artifact.js";

function parsePcm16Wave(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const text = (offset, length) =>
    String.fromCharCode(...bytes.subarray(offset, offset + length));
  if (text(0, 4) !== "RIFF" || text(8, 4) !== "WAVE") {
    throw new Error("impulse.wav is not a RIFF/WAVE file");
  }

  let format = null;
  let dataOffset = 0;
  let dataBytes = 0;
  for (let offset = 12; offset + 8 <= bytes.byteLength;) {
    const chunk = text(offset, 4);
    const size = view.getUint32(offset + 4, true);
    const body = offset + 8;
    if (chunk === "fmt ") {
      format = {
        encoding: view.getUint16(body, true),
        channels: view.getUint16(body + 2, true),
        sampleRate: view.getUint32(body + 4, true),
        bitsPerSample: view.getUint16(body + 14, true),
      };
    } else if (chunk === "data") {
      dataOffset = body;
      dataBytes = size;
    }
    offset = body + size + (size & 1);
  }
  if (!format || format.encoding !== 1 || format.bitsPerSample !== 16 || !dataBytes) {
    throw new Error("smoke test requires PCM16 impulse.wav data");
  }

  const sampleCount = dataBytes / 2;
  const data = new Float32Array(sampleCount);
  for (let index = 0; index < sampleCount; index += 1) {
    data[index] = view.getInt16(dataOffset + index * 2, true) / 32768;
  }
  return {
    data,
    frames: sampleCount / format.channels,
    channels: format.channels,
    sampleRate: format.sampleRate,
  };
}

let WorkletProcessor = null;
globalThis.AudioWorkletProcessor = class {
  constructor() {
    this.port = {
      messages: [],
      onmessage: null,
      postMessage: (message) => this.port.messages.push(message),
    };
  }
};
globalThis.registerProcessor = (_name, constructor) => {
  WorkletProcessor = constructor;
};
await import("./onda-wasm-processor.js");

const [wasm, metadata, wave] = await Promise.all([
  readFile(new URL("./sample-player.wasm", import.meta.url)),
  readFile(new URL("./sample-player.onda.json", import.meta.url), "utf8"),
  readFile(new URL("./impulse.wav", import.meta.url)),
]);
const artifact = await loadProcessorArtifactFiles(wasm, metadata);
const clip = parsePcm16Wave(wave);
const processor = new WorkletProcessor({
  processorOptions: {
    wasmBytes: artifact.wasm,
    metadata: artifact.metadata,
    params: { speed: 1 },
    buffers: { clip },
    initialize: true,
  },
});
processor.port.onmessage({
  data: { type: "event", event: "play", values: { enabled: true }, requestId: 1 },
});
if (processor.port.messages.some((message) => message.type === "onda-error")) {
  throw new Error(`worklet rejected play event: ${JSON.stringify(processor.port.messages)}`);
}

let renderedPeak = 0;
for (let offset = 0; offset < clip.frames && renderedPeak === 0; offset += 128) {
  const left = new Float32Array(128);
  const right = new Float32Array(128);
  processor.process([], [[left, right]]);
  for (const sample of [...left, ...right]) {
    renderedPeak = Math.max(renderedPeak, Math.abs(sample));
  }
}
if (!(renderedPeak > 0)) {
  throw new Error("sample-player worklet rendered silence from impulse.wav");
}

process.stdout.write(
  `Verified AOT sample player: ${clip.frames} frames, ${clip.channels} channels, peak ${renderedPeak.toFixed(6)}\n`,
);
