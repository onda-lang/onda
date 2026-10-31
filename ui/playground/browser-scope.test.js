import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { meterWorklet } from "./meter-worklet.js";
import { BrowserScopeSource, scopePacket } from "./browser-scope.js";

test("scope packets preserve channel peaks, signed extrema, and non-finite sanitation", () => {
  const packet = scopePacket(
    [
      Float32Array.from([-2, 0.25, 0.5, 1]),
      Float32Array.from([NaN, Infinity, -0.5, 0.75]),
    ],
    2,
  );
  const view = new DataView(packet.buffer);
  assert.equal(new TextDecoder().decode(packet.subarray(0, 4)), "OSQ1");
  assert.equal(view.getUint16(4, true), 2);
  assert.equal(view.getUint16(6, true), 2);
  assert.deepEqual(
    Array.from({ length: 8 }, (_, i) => view.getInt16(8 + i * 2, true)),
    [-32767, 8192, 16384, 32767, 0, 0, -16384, 24575],
  );
  assert.equal(
    scopePacket([new Float32Array(1024), new Float32Array(1024)], 348).length,
    2792,
  );
  assert.equal(scopePacket([new Float32Array(1024)], 4096).length, 8 + 512 * 4);
  assert.equal(scopePacket([new Float32Array(1024)], 512).length, 8 + 512 * 4);
  assert.equal(scopePacket([Float32Array.from([0, 1])], 512).length, 16);
  assert.equal(scopePacket([], 512).length, 8);
});

test("scope capture disconnects waveform processing in meter-only and hidden views", async (t) => {
  const saved = {
    document: globalThis.document,
    AudioWorkletNode: globalThis.AudioWorkletNode,
  };
  t.after(() => Object.assign(globalThis, saved));
  let reads = 0;
  const listeners = new Map();
  globalThis.document = {
    hidden: false,
    addEventListener: (type, fn) => listeners.set(type, fn),
    removeEventListener: (type) => listeners.delete(type),
  };
  const enabled = [];
  globalThis.AudioWorkletNode = class {
    port = { postMessage: (value) => enabled.push(value), close() {} };
    connect() {}
    disconnect() {}
  };
  const packets = [];
  const source = new BrowserScopeSource({
    postVisualization: (frame) => packets.push({ ...frame, packet: frame.packet?.slice() }),
  });
  const disconnect = t.mock.fn();
  const context = {
    destination: {},
    audioWorklet: { async addModule() {} },
    createChannelSplitter: () => ({ connect() {}, disconnect }),
    createGain: () => ({ gain: { value: 1 }, connect() {}, disconnect }),
    createAnalyser: () => ({
      fftSize: 1024,
      connect() {},
      disconnect,
      getFloatTimeDomainData(values) {
        reads++;
        values.fill(0.5);
      },
    }),
  };
  source.configure({ columns: 64, visible: true });
  await source.start(context, { connect() {}, disconnect() {} }, 2);
  const tick = () => {
    source.meter.port.onmessage({ data: { channels: 2, peaks: [0.5, 1.2] } });
  };
  packets.length = 0;
  tick();
  assert.equal(reads, 2);
  assert.equal(packets[0].packet.length, 8 + 2 * 64 * 4);
  assert.deepEqual(packets[0].peaks, [0.5, 1.2]);
  source.configure({ columns: 64, visible: false, meter: true });
  assert.equal(source.analysers.length, 0);
  assert.equal(enabled.at(-1), true);
  tick();
  assert.equal(packets[1].packet, undefined);
  assert.deepEqual(packets[1].peaks, [0.5, 1.2]);
  assert.equal(reads, 2);
  source.configure({ columns: 128, visible: true });
  tick();
  assert.equal(packets[2].packet.length, 8 + 2 * 128 * 4);
  document.hidden = true;
  listeners.get("visibilitychange")();
  tick();
  assert.equal(packets.length, 3);
  assert.equal(reads, 4);
  document.hidden = false;
  listeners.get("visibilitychange")();
  tick();
  assert.equal(packets.length, 4);
  source.dispose();
  assert.equal(listeners.size, 0);
  assert.equal(enabled.at(-1), "stop");
  assert.equal(disconnect.mock.callCount(), 12);
});

test("one audio clock publishes meters and waveform together at 30 Hz", (t) => {
  const saved = { document: globalThis.document };
  globalThis.document = {
    hidden: false,
    addEventListener() {},
    removeEventListener() {},
  };
  const frames = [];
  const source = new BrowserScopeSource({
    postVisualization(frame) { frames.push(frame); },
  });
  t.after(() => {
    source.dispose();
    Object.assign(globalThis, saved);
  });
  source.analysers = [{ getFloatTimeDomainData() {}, disconnect() {} }];
  source.samples = [new Float32Array(1024)];
  let Processor;
  vm.runInNewContext(meterWorklet, {
    AudioWorkletProcessor: class { port = { postMessage: frame => source.capture(frame) }; },
    sampleRate: 48000,
    registerProcessor: (_, type) => { Processor = type; },
  });
  const meter = new Processor();
  meter.port.onmessage({ data: true });
  const input = [new Float32Array(128).fill(0.5)];
  source.configure({ columns: 128, visible: true });
  for (let block = 0; block < 375; block++) {
    source.configure({ columns: 128 + block % 128, visible: true });
    meter.process([input]);
  }
  assert.equal(frames.length, 30);
  assert.ok(frames.every(frame => frame.peaks[0] === 0.5 && frame.packet));
  source.configure({ columns: 128, visible: false, meter: true });
  frames.length = 0;
  for (let block = 0; block < 375; block++) meter.process([input]);
  assert.equal(frames.length, 30);
  assert.ok(frames.every(frame => frame.peaks[0] === 0.5 && !frame.packet));
});


test("audio-thread metering retains per-channel transients and stops when disabled", () => {
  let Processor;
  const messages = [];
  vm.runInNewContext(meterWorklet, {
    AudioWorkletProcessor: class { port = { postMessage: (value) => messages.push(JSON.parse(JSON.stringify(value))) }; },
    sampleRate: 3840,
    registerProcessor: (_, type) => { Processor = type; },
  });
  const meter = new Processor();
  const inputs = Array.from({ length: 8 }, () => new Float32Array(64));
  inputs[7][3] = -1.5;
  meter.process([inputs]);
  assert.equal(messages.length, 0);
  meter.port.onmessage({ data: true });
  meter.process([inputs]);
  inputs[7].fill(0);
  meter.process([inputs]);
  assert.deepEqual(messages.pop(), { channels: 8, peaks: [0, 0, 0, 0, 0, 0, 0, 1.5] });
  meter.process([inputs.slice(0, 4)]);
  meter.process([inputs.slice(0, 4)]);
  assert.deepEqual(messages.pop(), { channels: 4, peaks: [0, 0, 0, 0] });
  meter.port.onmessage({ data: false });
  for (let i = 0; i < 8; i++) meter.process([inputs]);
  assert.equal(messages.length, 0);
  meter.port.onmessage({ data: "stop" });
  assert.equal(meter.process([inputs]), false);
});
