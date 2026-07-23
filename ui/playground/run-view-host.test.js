import assert from "node:assert/strict";
import test from "node:test";

import { BrowserRunViewHost, mergeEvents, mergeParams } from "./run-view-host.js";
import { UNBOUND_BUFFERS_MESSAGE } from "./browser-buffers.js";

function scalarParam(overrides = {}) {
  return {
    name: "freq",
    type_repr: "f32",
    scalar: "f32",
    array_len: 1,
    default_reprs: ["440"],
    range_min_repr: "20",
    range_max_repr: "1000",
    ...overrides,
  };
}

test("maps scalar artifact defaults and ranges to run-view params", () => {
  const params = mergeParams([
    scalarParam(),
    scalarParam({
      name: "enabled",
      type_repr: "bool",
      scalar: "bool",
      default_reprs: ["true"],
      range_min_repr: null,
      range_max_repr: null,
    }),
    scalarParam({
      name: "partials",
      type_repr: "f32[2]",
      array_len: 2,
      default_reprs: ["0.5", "0.25"],
    }),
  ], []);

  assert.deepEqual(params, [
    {
      index: 0,
      name: "freq",
      type: "f32",
      default: 440,
      rangeMin: 20,
      rangeMax: 1000,
      scalar: true,
      value: 440,
    },
    {
      index: 1,
      name: "enabled",
      type: "bool",
      default: true,
      rangeMin: null,
      rangeMax: null,
      scalar: true,
      value: true,
    },
  ]);
});

test("preserves an edited value only while the artifact param shape matches", () => {
  const [initial] = mergeParams([scalarParam()], []);
  const [preserved] = mergeParams([scalarParam()], [{ ...initial, value: 880 }]);
  const [reset] = mergeParams([
    scalarParam({ default_reprs: ["220"] }),
  ], [{ ...initial, value: 880 }]);

  assert.equal(preserved.value, 880);
  assert.equal(reset.value, 220);
});

test("decodes floating-point bit-pattern representations", () => {
  const [param] = mergeParams([
    scalarParam({
      scalar: "f64",
      type_repr: "f64",
      default_reprs: ["0x3ff8000000000000"],
      range_min_repr: "0x0000000000000000",
      range_max_repr: "0x4000000000000000",
    }),
  ], []);

  assert.equal(param.value, 1.5);
  assert.equal(param.rangeMin, 0);
  assert.equal(param.rangeMax, 2);
});

test("preserves event array shapes instead of presenting them as scalars", () => {
  const events = mergeEvents([{
    name: "load",
    params: [
      {
        name: "fixed",
        type_repr: "f32[2]",
        scalar: "f32",
        array_len: 2,
        is_slice: false,
        default_reprs: ["0.5", "0.25"],
      },
      {
        name: "samples",
        type_repr: "f32[]",
        scalar: "f32",
        array_len: 0,
        is_slice: true,
        default_reprs: [],
      },
    ],
  }], []);

  assert.deepEqual(events[0].args, [
    {
      index: 0,
      name: "fixed",
      type: "f32[2]",
      scalar: "f32",
      arrayLength: 2,
      isSlice: false,
      default: [0.5, 0.25],
      value: [0.5, 0.25],
    },
    {
      index: 1,
      name: "samples",
      type: "f32[]",
      scalar: "f32",
      arrayLength: null,
      isSlice: true,
      default: [],
      value: [],
    },
  ]);
});

test("preserves event values only while the argument shape matches", () => {
  const scalarEvent = {
    name: "load",
    params: [{
      name: "samples",
      type_repr: "f32",
      scalar: "f32",
      array_len: 1,
      is_slice: false,
      default_reprs: ["1"],
    }],
  };
  const [initial] = mergeEvents([scalarEvent], []);
  const [preserved] = mergeEvents([scalarEvent], [{
    ...initial,
    args: [{ ...initial.args[0], value: 0.5 }],
  }]);
  const [resetForArray] = mergeEvents([{
    ...scalarEvent,
    params: [{
      ...scalarEvent.params[0],
      type_repr: "f32[2]",
      array_len: 2,
      default_reprs: ["1", "2"],
    }],
  }], [{
    ...initial,
    args: [{ ...initial.args[0], value: 0.5 }],
  }]);
  const [resetForDefault] = mergeEvents([{
    ...scalarEvent,
    params: [{
      ...scalarEvent.params[0],
      default_reprs: ["2"],
    }],
  }], [{
    ...initial,
    args: [{ ...initial.args[0], value: 0.5 }],
  }]);

  assert.equal(preserved.args[0].value, 0.5);
  assert.deepEqual(resetForArray.args[0].value, [1, 2]);
  assert.equal(resetForDefault.args[0].value, 2);
});

test("blocks browser playback and shows guidance until every buffer is bound", async () => {
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const previousMutationObserver = globalThis.MutationObserver;
  let starts = 0;
  globalThis.window = {
    location: { href: "https://onda.test/play/" },
    addEventListener() {},
    removeEventListener() {},
  };
  globalThis.document = { documentElement: { dataset: {} } };
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
  };
  const iframe = {
    src: "https://onda.test/play/run.html",
    contentWindow: { postMessage() {} },
    addEventListener() {},
  };

  try {
    const host = new BrowserRunViewHost(iframe, {
      start: async () => { starts += 1; },
    });
    host.setArtifact({
      metadata: {
        compile: {
          sample_rate: 44_100,
          block_size: 256,
        },
        metadata: {
          params: [],
          events: [],
          outputs: [],
          buffers: [{
            name: "clip",
            type_repr: "buffer[f32]",
            channels: "mono",
            static_channels: null,
          }],
        },
      },
    }, new Map());

    assert.equal(host.state.status, UNBOUND_BUFFERS_MESSAGE);
    assert.equal(host.state.running, false);
    assert.equal(host.state.sampleRateHz, 44_100);
    assert.equal(host.state.blockFrames, 256);
    await host.handleMessage({ type: "start" });
    assert.equal(starts, 0);

    host.updateBufferFile("clip", { name: "clip.wav" }, {
      frames: 96_000,
      channels: 2,
      sampleRate: 48_000,
    });
    assert.deepEqual(
      {
        loadedFrames: host.state.buffers[0].loadedFrames,
        loadedChannels: host.state.buffers[0].loadedChannels,
        loadedSampleRate: host.state.buffers[0].loadedSampleRate,
      },
      { loadedFrames: 96_000, loadedChannels: 2, loadedSampleRate: 48_000 },
    );
    await host.handleMessage({ type: "start" });
    assert.equal(starts, 1);
    host.dispose();
  } finally {
    globalThis.window = previousWindow;
    globalThis.document = previousDocument;
    globalThis.MutationObserver = previousMutationObserver;
  }
});
