// Tests for the shared browser-playground buffer boundary.
import assert from "node:assert/strict";
import test from "node:test";

import {
  decodeWav,
  prepareBufferBindings,
  UNBOUND_BUFFERS_MESSAGE,
} from "./browser-buffers.js";

function pcm16Wav({ channels = 2, sampleRate = 44_100, samples }) {
  const dataLength = samples.length * 2;
  const bytes = new Uint8Array(44 + dataLength);
  const view = new DataView(bytes.buffer);
  const text = (offset, value) => {
    for (let index = 0; index < value.length; index += 1) {
      bytes[offset + index] = value.charCodeAt(index);
    }
  };
  text(0, "RIFF");
  view.setUint32(4, 36 + dataLength, true);
  text(8, "WAVE");
  text(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, channels, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * channels * 2, true);
  view.setUint16(32, channels * 2, true);
  view.setUint16(34, 16, true);
  text(36, "data");
  view.setUint32(40, dataLength, true);
  samples.forEach((sample, index) => view.setInt16(44 + index * 2, sample, true));
  return bytes;
}

function float32Wav(samples) {
  const dataLength = samples.length * 4;
  const bytes = new Uint8Array(44 + dataLength);
  const view = new DataView(bytes.buffer);
  const text = (offset, value) => {
    for (let index = 0; index < value.length; index += 1) {
      bytes[offset + index] = value.charCodeAt(index);
    }
  };
  text(0, "RIFF");
  view.setUint32(4, 36 + dataLength, true);
  text(8, "WAVE");
  text(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 3, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, 48_000, true);
  view.setUint32(28, 48_000 * 4, true);
  view.setUint16(32, 4, true);
  view.setUint16(34, 32, true);
  text(36, "data");
  view.setUint32(40, dataLength, true);
  samples.forEach((sample, index) => view.setFloat32(44 + index * 4, sample, true));
  return bytes;
}

test("decodes WAV frames into Onda's native interleaved f32 layout", () => {
  const wav = pcm16Wav({ samples: [16_384, -16_384, 32_767, -32_768] });
  const decoded = decodeWav(wav);

  assert.equal(decoded.frames, 2);
  assert.equal(decoded.channels, 2);
  assert.equal(decoded.sampleRate, 44_100);
  assert.deepEqual([...decoded.data], [0.5, -0.5, 32_767 / 32_768, -1]);
});

test("decodes IEEE-float WAV data without resampling", () => {
  const decoded = decodeWav(float32Wav([0.125, -0.75]));
  assert.equal(decoded.sampleRate, 48_000);
  assert.deepEqual([...decoded.data], [0.125, -0.75]);
});

test("refuses to prepare bindings while any declared buffer is unbound", async () => {
  const wav = pcm16Wav({ channels: 1, sampleRate: 48_000, samples: [8_192, -8_192] });
  const metadata = {
    compile: { sample_rate: 48_000, block_size: 256 },
    metadata: {
      buffers: [
        { name: "clip", scalar: "f32", static_channels: 1 },
        { name: "scratch", scalar: "f32", static_channels: 2 },
      ],
    },
  };
  const files = new Map([[
    "clip",
    { name: "clip.wav", arrayBuffer: async () => wav.buffer },
  ]]);
  await assert.rejects(
    prepareBufferBindings(metadata, files),
    new RegExp(UNBOUND_BUFFERS_MESSAGE),
  );
});

test("prepares bindings from validated WAV files", async () => {
  const wav = pcm16Wav({ channels: 1, sampleRate: 48_000, samples: [8_192, -8_192] });
  const metadata = {
    compile: { sample_rate: 48_000, block_size: 256 },
    metadata: {
      buffers: [{ name: "clip", scalar: "f32", static_channels: 1 }],
    },
  };
  const files = new Map([[
    "clip",
    { name: "clip.wav", arrayBuffer: async () => wav.buffer },
  ]]);
  const bindings = await prepareBufferBindings(metadata, files);

  assert.deepEqual([...bindings.clip.data], [0.25, -0.25]);
  assert.equal(bindings.clip.frames, 2);
});
