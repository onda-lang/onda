import assert from "node:assert/strict";
import test from "node:test";
import { browserBridge } from "../gpu/browser.js";

test("GPU buffer drops accept one supported file and reject other drags", (t) => {
  const sent = [], messages = [], listeners = {};
  const parent = { postMessage: value => sent.push(value.message) };
  for (const name of ["window", "document"]) {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, name);
    t.after(() => {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    });
  }
  globalThis.window = { parent, location: { origin: "http://localhost" }, addEventListener() {} };
  globalThis.document = { addEventListener() {} };
  browserBridge({
    addEventListener: (type, listener) => { listeners[type] = listener; },
    getBoundingClientRect: () => ({ left: 10, top: 20 }),
  }, {
    message: value => messages.push(value),
    target: (x, y) => x === 40 && y === 60 ? "clip" : null,
  });
  const drag = (type, names, { types = ["Files"], protectedFiles = false } = {}) => {
    let prevented = false;
    const files = names.map(name => ({ name }));
    const dataTransfer = { types, files: protectedFiles ? [] : files, items: files.map(() => ({ kind: "file" })) };
    listeners[type]({ type, clientX: 50, clientY: 80, dataTransfer, preventDefault() { prevented = true; } });
    return { dataTransfer, prevented, file: files[0] };
  };
  for (const name of ["sample.WAV", "sample.ondabuffer"]) {
    const hover = drag("dragover", [name]);
    assert.equal(hover.dataTransfer.dropEffect, "copy");
    assert.equal(messages.at(-1).phase, "over");
    const drop = drag("drop", [name]);
    assert.deepEqual(sent.pop(), { type: "bindBufferFile", name: "clip", file: drop.file });
    assert.equal(messages.at(-1).phase, "leave");
  }
  assert.equal(drag("dragover", ["sample.wav"], { protectedFiles: true }).dataTransfer.dropEffect, "copy");
  for (const files of [["one.wav", "two.wav"], ["image.png"], []]) {
    const hover = drag("dragover", files);
    assert.equal(hover.dataTransfer.dropEffect, "none");
    assert.equal(messages.at(-1).phase, "leave");
    assert.equal(drag("drop", files).prevented, true);
    assert.equal(sent.length, 0);
  }
  assert.equal(drag("dragover", ["one.wav", "two.wav"], { protectedFiles: true }).dataTransfer.dropEffect, "none");
  drag("drop", ["sample.wav"], { types: ["text/plain"] });
  assert.equal(sent.length, 0);
  for (const type of ["dragleave", "dragend"]) {
    drag("dragover", ["sample.wav"]);
    listeners[type]({ type, dataTransfer: { types: [] } });
    assert.equal(messages.at(-1).phase, "leave");
  }
});
