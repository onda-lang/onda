// Tests for the shared browser-playground buffer boundary.
import assert from "node:assert/strict";
import test from "node:test";

import {
  decodeOndaBuffer,
  encodeOndaBuffer,
  prepareBufferBindings,
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

test("omits unbound buffers so the worklet can install neutral descriptors", async () => {
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
  const projectApi = {
    async decodeBufferFile(bytes) {
      assert.deepEqual(bytes, wav);
      return {
        element: "f32",
        data: new Float32Array([0.25, -0.25]),
        frames: 2,
        channels: 1,
        sampleRate: 48_000,
      };
    },
  };
  const bindings = await prepareBufferBindings(metadata, files, projectApi);
  assert.deepEqual(Object.keys(bindings), ["clip"]);
  assert.equal(bindings.scratch, undefined);
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
  const projectApi = {
    async decodeBufferFile(bytes, path) {
      assert.deepEqual(bytes, wav);
      assert.equal(path, "clip.wav");
      return {
        element: "f32",
        data: new Float32Array([0.25, -0.25]),
        frames: 2,
        channels: 1,
        sampleRate: 48_000,
      };
    },
  };
  const bindings = await prepareBufferBindings(metadata, files, projectApi);

  assert.deepEqual([...bindings.clip.data], [0.25, -0.25]);
  assert.equal(bindings.clip.frames, 2);
});

test("canonical Onda buffers round-trip every typed payload byte", async () => {
  const encodedBindings = new Map();
  const projectApi = {
    async encodeBufferAsset(binding) {
      const encoded = new Uint8Array([encodedBindings.size]);
      encodedBindings.set(encoded, binding);
      return encoded;
    },
    async decodeBufferAsset(encoded) {
      return encodedBindings.get(encoded);
    },
  };
  const cases = [
    ["bool", new Uint8Array([0, 1])],
    ["i32", new Int32Array([-2, 7])],
    ["i64", new BigInt64Array([-2n, 9n])],
    ["f32", new Float32Array([0.25, -0.5])],
    ["f64", new Float64Array([0.25, -0.5])],
  ];
  for (const [scalar, data] of cases) {
    const encoded = await encodeOndaBuffer({
      data,
      frames: 2,
      channels: 1,
      sampleRate: 48_000,
    }, scalar, projectApi);
    const decoded = await decodeOndaBuffer(encoded, projectApi);
    assert.equal(decoded.scalar, scalar);
    assert.deepEqual([...decoded.data], [...data]);
  }
});
