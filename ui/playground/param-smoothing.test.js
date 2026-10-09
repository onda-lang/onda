import assert from "node:assert/strict";
import test from "node:test";
import { OndaAudioProcessor } from "@onda-lang/webaudio";
import { BrowserParamSmoothing } from "./param-smoothing.js";

function fixture() {
  const messages = [];
  const port = new EventTarget();
  port.postMessage = message => messages.push(message);
  let processor = new OndaAudioProcessor({ port }, {
    compile: { sample_rate: 48_000, block_size: 512 },
  });
  const storage = new Map();
  const errors = [];
  const preference = new BrowserParamSmoothing(48_000, () => processor,
    error => errors.push(error), {
      getItem: key => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
    });
  return { preference, messages, errors, storage, port,
    close() { const previous = processor; processor = null; previous.close(); },
  };
}

test("smoothing preference survives closing a processor before acknowledgement", async () => {
  const host = fixture();
  host.preference.set(0);
  host.close();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.preference.milliseconds, 0);
  assert.equal([...host.storage.values()][0], "0");
  assert.equal(host.messages[0].seconds, 0);
  assert.deepEqual(host.errors, []);
});

test("smoothing edits are retained while stopped and invalid edits leave the preference intact", () => {
  const host = fixture();
  host.close();
  host.preference.set(15.5);
  assert.equal(host.preference.milliseconds, 15.5);
  for (const value of [-1, NaN, Infinity, 1e20, "5"]) {
    assert.throws(() => host.preference.set(value), /Smoothing duration/);
    assert.equal(host.preference.milliseconds, 15.5);
  }
});

test("smoothing failures on the current processor are reported without losing the preference", async () => {
  const host = fixture();
  host.preference.set(50);
  host.port.dispatchEvent(new MessageEvent("message", { data: {
    type: "onda-error", requestId: host.messages[0].requestId, error: "update failed",
  } }));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.preference.milliseconds, 50);
  assert.equal(host.errors[0].message, "update failed");
  host.close();
});

test("processor replies cannot overwrite a newer smoothing edit", async () => {
  const host = fixture();
  host.preference.set(15);
  host.preference.set(0);
  for (const request of host.messages.toReversed()) {
    host.port.dispatchEvent(new MessageEvent("message", { data: {
      type: "onda-ok", requestId: request.requestId,
    } }));
  }
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(host.preference.milliseconds, 0);
  assert.equal([...host.storage.values()][0], "0");
  assert.deepEqual(host.errors, []);
  host.close();
});

test("smoothing restores valid stored durations and falls back when storage is invalid or unavailable", () => {
  for (const [saved, expected] of [["0", 0], ["15.5", 15.5], [null, 30], ["", 30],
    [" ", 30], ["NaN", 30], ["-1", 30], ["1e20", 30]]) {
    const preference = new BrowserParamSmoothing(48_000, () => null, () => {}, {
      getItem: () => saved,
    });
    assert.equal(preference.milliseconds, expected);
  }
  const preference = new BrowserParamSmoothing(48_000, () => null, () => {}, {
    getItem() { throw new Error("storage unavailable"); },
    setItem() { throw new Error("storage unavailable"); },
  });
  preference.set(0);
  assert.equal(preference.milliseconds, 0);
});
