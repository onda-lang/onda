import assert from "node:assert/strict";
import test from "node:test";
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

test("scope capture suspends hidden/collapsed views", (t) => {
  const saved = {
    document: globalThis.document,
    setInterval: globalThis.setInterval,
    clearInterval: globalThis.clearInterval,
  };
  t.after(() => Object.assign(globalThis, saved));
  const frames = new Map();
  let next = 0,
    reads = 0;
  const listeners = new Map();
  globalThis.document = {
    hidden: false,
    addEventListener: (type, fn) => listeners.set(type, fn),
    removeEventListener: (type) => listeners.delete(type),
  };
  globalThis.setInterval = (fn) => {
    frames.set(++next, fn);
    return next;
  };
  globalThis.clearInterval = (id) => frames.delete(id);
  const packets = [];
  const source = new BrowserScopeSource({
    postScope() {},
    postScopePacket: (packet) => packets.push(packet.slice()),
  });
  const disconnect = t.mock.fn();
  const context = {
    destination: {},
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
  source.start(context, { connect() {} }, 2);
  const tick = () => {
    const [id, fn] = frames.entries().next().value;
    fn();
  };
  tick();
  assert.equal(reads, 2);
  assert.equal(packets[0].length, 8 + 2 * 64 * 4);
  source.configure({ columns: 64, visible: false });
  assert.equal(frames.size, 0);
  source.capture();
  assert.equal(reads, 2);
  source.configure({ columns: 128, visible: true });
  tick();
  assert.equal(packets[1].length, 8 + 2 * 128 * 4);
  document.hidden = true;
  listeners.get("visibilitychange")();
  assert.equal(frames.size, 0);
  source.capture();
  assert.equal(reads, 4);
  document.hidden = false;
  listeners.get("visibilitychange")();
  assert.equal(frames.size, 1);
  source.dispose();
  assert.equal(frames.size, 0);
  assert.equal(listeners.size, 0);
  assert.equal(disconnect.mock.callCount(), 4);
});

test("scope captures target 30 Hz independently of display rate and resizing", (t) => {
  const saved = {
    document: globalThis.document,
    requestAnimationFrame: globalThis.requestAnimationFrame,
    cancelAnimationFrame: globalThis.cancelAnimationFrame,
  };
  t.mock.timers.enable({ apis: ["setInterval"] });
  const frames = new Map();
  let next = 0;
  globalThis.document = {
    hidden: false,
    addEventListener() {},
    removeEventListener() {},
  };
  globalThis.requestAnimationFrame = (fn) => {
    frames.set(++next, fn);
    return next;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  let captures = 0;
  const source = new BrowserScopeSource({
    postScope() {},
    postScopePacket() { captures++; },
  });
  t.after(() => {
    source.dispose();
    Object.assign(globalThis, saved);
  });
  source.analysers = [{ getFloatTimeDomainData() {}, disconnect() {} }];
  source.samples = [new Float32Array(1024)];
  source.schedule();
  for (let time = 0; time < 1000; time += 16) {
    source.configure({ columns: 128 + time % 128, visible: true });
    for (const [id, fn] of [...frames]) {
      frames.delete(id);
      fn(time);
    }
    t.mock.timers.tick(16);
  }
  assert.equal(captures, 30, "scope capture must be independent of UI frame rate");
});
