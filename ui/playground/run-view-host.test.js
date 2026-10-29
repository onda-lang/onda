import assert from "node:assert/strict";
import test from "node:test";
import { OndaCompileError } from "@onda-lang/wasm-compiler";

import {
  BrowserRunViewHost,
  mergeEvents as mergeSchemaEvents,
  mergeParams,
} from "./run-view-host.js";

test("renderer errors reveal the frame through source changes and reloads retry readiness", async () => {
  const loading = [];
  const host = Object.assign(Object.create(BrowserRunViewHost.prototype), {
    iframe: { style:{} },
    handlers: { loadingChanged:value => loading.push(value) },
    state: { path:"main.onda", schemaReady:true },
    ready:false,
    pendingViewReset:true,
    viewResetId:0,
    post() {},
  });
  host.setLoading(true);
  assert.equal(host.iframe.style.opacity, "0");
  await host.handleMessage({type:"runViewError",message:"Wasm fetch failed"});
  assert.equal(host.ready, false);
  assert.equal(host.iframe.style.opacity, "");
  host.setPath("other.onda");
  assert.equal(host.iframe.style.opacity, "", "a source change must not conceal the renderer error");
  await host.handleMessage({type:"webviewReady"});
  assert.equal(host.viewFailed, false);
  host.requestViewReset();
  assert.equal(host.iframe.style.opacity, "0");
  host.postState();
  await host.handleMessage({type:"runViewReady",readyId:host.viewResetId});
  assert.equal(host.iframe.style.opacity, "");
  assert.deepEqual(loading, [true,false,false,true,false]);
});

test("compiler errors retain source locations in shared run-view messages", () => {
  for (const method of ["setError", "showError"]) {
    for (const [location, suffix] of [
      [{ file: "voices/lead.onda", line: 24, column: 7 }, " (voices/lead.onda:24:7)"],
      [{ file: null, line: 24, column: 7 }, " (line 24:7)"],
      [{ file: "main.onda", line: 24, column: 0 }, " (main.onda:24)"],
      [{ file: "main.onda", line: 0, column: 0 }, ""],
    ]) {
      const messages = [];
      const host = Object.assign(Object.create(BrowserRunViewHost.prototype), {
        ready: true,
        state: { path: "main.onda", running: true, connected: true },
        post: message => messages.push(message),
      });
      const error = new OndaCompileError([
        { stage: "semantic", message: "Unknown identifier 'missing'", ...location },
        { stage: "semantic", message: "Another error", file: "other.onda", line: 9, column: 2 },
      ]);
      host[method](error);
      assert.equal(messages.at(-1).state.error, error.message + suffix);
      assert.equal(host.state.running, method === "showError");
      host[method](new Error("Audio unavailable"));
      assert.equal(messages.at(-1).state.error, "Audio unavailable");
      host[method]("");
      assert.equal(messages.at(-1).state.error, "");
    }
  }
});

// Keep primitive fixture shorthand local to tests; production consumes recursive schemas.
function mergeEvents(events, existing) {
  return mergeSchemaEvents(
    events.map((event) => ({
      ...event,
      schema: event.schema ?? {
        params: event.params.map((param) => {
          const scalar = { kind: "scalar", encoding: param.scalar };
          const array = /\[/.test(param.type_repr);
          return {
            name: param.name,
            ty: param.is_slice
              ? { kind: "slice", element: scalar }
              : array
                ? { kind: "array", element: scalar, len: param.array_len }
                : scalar,
            ...(!param.is_slice && param.default_reprs?.length
              ? {
                  default: array ? param.default_reprs : param.default_reprs[0],
                }
              : {}),
          };
        }),
      },
    })),
    existing,
  );
}

function scalarParam(overrides = {}) {
  return {
    name: "freq",
    type_repr: "f32",
    scalar: "f32",
    array_len: 1,
    default_reprs: ["440"],
    range_min_repr: "20",
    range_max_repr: "1000",
    param_control: {
      scale: "log",
      curve: null,
      unit: "Hz",
      step_repr: null,
      step_count: null,
    },
    ...overrides,
  };
}

test("maps scalar artifact defaults and ranges to run-view params", () => {
  const params = mergeParams(
    [
      scalarParam(),
      scalarParam({
        name: "enabled",
        type_repr: "bool",
        scalar: "bool",
        default_reprs: ["true"],
        range_min_repr: null,
        range_max_repr: null,
        param_control: null,
      }),
      scalarParam({
        name: "live_input",
        type_repr: "bool",
        scalar: "bool",
        default_reprs: ["false"],
        range_min_repr: null,
        range_max_repr: null,
        param_control: null,
      }),
      scalarParam({
        name: "partials",
        type_repr: "f32[2]",
        array_len: 2,
        default_reprs: ["0.5", "0.25"],
      }),
    ],
    [],
  );

  assert.deepEqual(params.slice(0, 3), [
    {
      index: 0,
      name: "freq",
      type: "f32",
      default: 440,
      rangeMin: 20,
      rangeMax: 1000,
      scale: "log",
      curve: null,
      unit: "Hz",
      step: null,
      stepCount: null,
      scalar: true,
      array: null,
      value: 440,
    },
    {
      index: 1,
      name: "enabled",
      type: "bool",
      default: true,
      rangeMin: null,
      rangeMax: null,
      scale: null,
      curve: null,
      unit: null,
      step: null,
      stepCount: null,
      scalar: true,
      array: null,
      value: true,
    },
    {
      index: 2,
      name: "live_input",
      type: "bool",
      default: false,
      rangeMin: null,
      rangeMax: null,
      scale: null,
      curve: null,
      unit: null,
      step: null,
      stepCount: null,
      scalar: true,
      array: null,
      value: false,
    },
  ]);
  assert.deepEqual(
    params.slice(3).map((param) => [param.name, param.default, param.array]),
    [
      ["partials[0]", 0.5, { name: "partials", length: 2, index: 0 }],
      ["partials[1]", 0.25, { name: "partials", length: 2, index: 1 }],
    ],
  );
});

test("preserves an edited value only while the artifact param shape matches", () => {
  const [initial] = mergeParams([scalarParam()], []);
  const [preserved] = mergeParams(
    [scalarParam()],
    [{ ...initial, value: 880 }],
  );
  const [reset] = mergeParams(
    [scalarParam({ default_reprs: ["220"] })],
    [{ ...initial, value: 880 }],
  );

  assert.equal(preserved.value, 880);
  assert.equal(reset.value, 220);
});

test("resets an edited value when parameter curvature changes", () => {
  const [initial] = mergeParams(
    [
      scalarParam({
        param_control: {
          scale: "linear",
          curve: -4,
          unit: "Hz",
          step_repr: null,
          step_count: null,
        },
      }),
    ],
    [],
  );
  const [reset] = mergeParams(
    [
      scalarParam({
        param_control: {
          scale: "linear",
          curve: 4,
          unit: "Hz",
          step_repr: null,
          step_count: null,
        },
      }),
    ],
    [{ ...initial, value: 880 }],
  );

  assert.equal(reset.value, 440);
});

test("decodes floating-point bit-pattern representations", () => {
  const [param] = mergeParams(
    [
      scalarParam({
        scalar: "f64",
        type_repr: "f64",
        default_reprs: ["0x3ff8000000000000"],
        range_min_repr: "0x0000000000000000",
        range_max_repr: "0x4000000000000000",
      }),
    ],
    [],
  );

  assert.equal(param.value, 1.5);
  assert.equal(param.rangeMin, 0);
  assert.equal(param.rangeMax, 2);
});

test("decodes finite f32 representations at their declared precision", () => {
  const [param, stepped] = mergeParams(
    [
      scalarParam({
        default_reprs: ["0.72"],
        range_min_repr: "0",
        range_max_repr: "0.98",
        param_control: {
          scale: "linear",
          curve: null,
          unit: null,
          step_repr: null,
          step_count: null,
        },
      }),
      scalarParam({
        name: "stepped",
        default_reprs: ["0.2"],
        range_min_repr: "0",
        range_max_repr: "0.3",
        param_control: {
          scale: "linear",
          curve: null,
          unit: null,
          step_repr: "0.1",
          step_count: 3,
        },
      }),
    ],
    [],
  );

  assert.equal(param.default, 0.72);
  assert.equal(param.value, 0.72);
  assert.equal(param.rangeMax, 0.98);
  assert.equal(stepped.step, 0.1);
});

test("preserves event array shapes instead of presenting them as scalars", () => {
  const events = mergeEvents(
    [
      {
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
      },
    ],
    [],
  );

  assert.deepEqual(events[0].args, [
    {
      index: 0,
      name: "fixed",
      type: "f32[2]",
      scalar: "f32",
      arrayLength: 2,
      isSlice: false,
      shape: {
        kind: "array",
        element: { kind: "scalar", encoding: "f32" },
        len: 2,
      },
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
      shape: { kind: "slice", element: { kind: "scalar", encoding: "f32" } },
      default: [],
      value: [],
    },
  ]);
});

test("preserves full-range i64 event values as decimal strings", () => {
  const [event] = mergeEvents(
    [
      {
        name: "select",
        params: [
          {
            name: "scalar",
            type_repr: "i64",
            scalar: "i64",
            array_len: 1,
            is_slice: false,
            default_reprs: ["9007199254740993"],
          },
          {
            name: "fixed",
            type_repr: "i64[2]",
            scalar: "i64",
            array_len: 2,
            is_slice: false,
            default_reprs: ["-9223372036854775808", "9223372036854775807"],
          },
        ],
      },
    ],
    [],
  );

  assert.equal(event.args[0].default, "9007199254740993");
  assert.equal(event.args[0].value, "9007199254740993");
  assert.deepEqual(event.args[1].default, [
    "-9223372036854775808",
    "9223372036854775807",
  ]);
  assert.deepEqual(event.args[1].value, event.args[1].default);
});

test("preserves event values only while the argument shape matches", () => {
  const scalarEvent = {
    name: "load",
    params: [
      {
        name: "samples",
        type_repr: "f32",
        scalar: "f32",
        array_len: 1,
        is_slice: false,
        default_reprs: ["1"],
      },
    ],
  };
  const [initial] = mergeEvents([scalarEvent], []);
  const [preserved] = mergeEvents(
    [scalarEvent],
    [
      {
        ...initial,
        args: [{ ...initial.args[0], value: 0.5 }],
      },
    ],
  );
  const [resetForArray] = mergeEvents(
    [
      {
        ...scalarEvent,
        params: [
          {
            ...scalarEvent.params[0],
            type_repr: "f32[2]",
            array_len: 2,
            default_reprs: ["1", "2"],
          },
        ],
      },
    ],
    [
      {
        ...initial,
        args: [{ ...initial.args[0], value: 0.5 }],
      },
    ],
  );
  const [resetForDefault] = mergeEvents(
    [
      {
        ...scalarEvent,
        params: [
          {
            ...scalarEvent.params[0],
            default_reprs: ["2"],
          },
        ],
      },
    ],
    [
      {
        ...initial,
        args: [{ ...initial.args[0], value: 0.5 }],
      },
    ],
  );

  assert.equal(preserved.args[0].value, 0.5);
  assert.deepEqual(resetForArray.args[0].value, [1, 2]);
  assert.equal(resetForDefault.args[0].value, 2);
});

test("distinguishes IEEE-special event defaults when preserving values", () => {
  const scalarEvent = (defaultValue) => ({
    name: "scalar",
    schema: {
      params: [
        {
          name: "value",
          ty: { kind: "scalar", encoding: "f64" },
          default: defaultValue,
        },
      ],
    },
  });
  const [positiveZero] = mergeSchemaEvents([scalarEvent("0")], []);
  positiveZero.args[0].value = 1;
  const [negativeZero] = mergeSchemaEvents(
    [scalarEvent("0x8000000000000000")],
    [positiveZero],
  );

  const aggregateEvent = (defaultValue) => ({
    name: "aggregate",
    schema: {
      params: [
        {
          name: "values",
          ty: {
            kind: "array",
            element: { kind: "scalar", encoding: "f64" },
            len: 1,
          },
          default: [defaultValue],
        },
      ],
    },
  });
  const [notANumber] = mergeSchemaEvents(
    [aggregateEvent("0x7ff8000000000000")],
    [],
  );
  notANumber.args[0].value = [1];
  const [infinity] = mergeSchemaEvents(
    [aggregateEvent("0x7ff0000000000000")],
    [notANumber],
  );

  assert.ok(Object.is(negativeZero.args[0].value, -0));
  assert.equal(infinity.args[0].value[0], Infinity);
});

test("publishes buffer previews together and preserves them through recompiles", () => {
  const host = Object.create(BrowserRunViewHost.prototype);
  host.state = { params: [], events: [], buffers: [] };
  host.handlers = {};
  let publications = 0;
  host.postState = () => publications++;
  const artifact = {
    metadata: {
      metadata: {
        params: [],
        events: [],
        outputs: [],
        buffers: ["clip", "other"].map((name) => ({
          name,
          type_repr: "buffer<f32>",
        })),
      },
    },
  };
  const files = new Map([
    ["clip", { name: "clip.wav" }],
    ["other", { name: "other.wav" }],
  ]);
  const waveform = {
    minValue: -0.5,
    maxValue: 1,
    minimums: [-0.5],
    maximums: [1],
  };
  host.setArtifact(artifact, files);
  const before = publications;
  host.updateBufferFiles(
    [...files].map(([name, file]) => ({
      name,
      file,
      metadata: { frames: 2, channels: 1, sampleRate: 48_000, waveform },
    })),
  );
  assert.equal(publications, before + 1);
  assert.deepEqual(
    host.state.buffers.map((buffer) => buffer.waveform),
    [waveform, waveform],
  );

  host.setArtifact(artifact, files);
  assert.deepEqual(
    host.state.buffers.map((buffer) => buffer.waveform),
    [waveform, waveform],
  );
  const replacement = {
    minValue: 0,
    maxValue: 0,
    minimums: [0],
    maximums: [0],
  };
  host.updateBufferFile(
    "clip",
    { name: "replacement.wav" },
    {
      frames: 1,
      channels: 1,
      sampleRate: 48_000,
      waveform: replacement,
    },
  );
  assert.deepEqual(host.state.buffers[0].waveform, replacement);
  host.updateBufferFile("clip", null);
  assert.equal(host.state.buffers[0].waveform, null);
  assert.equal(host.state.buffers[0].loadedFrames, null);

  host.setArtifact(artifact, new Map([["other", { name: "different.wav" }]]));
  assert.equal(host.state.buffers[1].waveform, null);
});

test("parameter edits and resets publish values after the view receives its schema", async () => {
  const messages = [];
  const edits = [];
  const host = Object.assign(Object.create(BrowserRunViewHost.prototype), {
    ready: false,
    pendingViewReset: true,
    viewResetId: 1,
    state: {
      path: "sampler.onda",
      params: [
        { name: "gain", type: "f32", default: 1, value: 0.5 },
        { name: "gate", type: "bool", default: false, value: true },
        {
          name: "count",
          type: "i64",
          default: "9223372036854775807",
          value: "0",
        },
      ],
      buffers: [{ name: "clip", loadedPath: "clip.wav" }],
      events: [{ name: "bang", args: [{ name: "amount", value: 0.25 }] }],
    },
    handlers: { setParam: async (...edit) => edits.push(edit) },
    post: (message) => messages.push(structuredClone(message)),
  });
  await host.handleMessage({ type: "setParam", name: "gain", value: 0.75 });
  assert.equal(messages.length, 0);
  await host.handleMessage({ type: "webviewReady" });
  assert.equal(messages.at(-1).type, "state");
  assert.equal(messages.at(-1).state.params[0].value, 0.75);
  assert.deepEqual(messages.at(-1).state.viewState, {
    reset: true,
    readyId: 1,
  });

  for (const [name, value] of [
    ["gain", 0.25],
    ["gate", false],
    ["count", "9007199254740993"],
  ]) {
    await host.handleMessage({ type: "setParam", name, value });
    assert.deepEqual(messages.at(-1), {
      type: "parameterValues",
      state: { params: [{ name, value }] },
    });
    assert.equal(
      host.state.params.find((param) => param.name === name).value,
      value,
    );
    assert.deepEqual(edits.at(-1), [name, value]);
  }
  await host.handleMessage({ type: "resetParams" });
  assert.deepEqual(messages.at(-1), {
    type: "parameterValues",
    state: {
      params: [
        { name: "gain", value: 1 },
        { name: "gate", value: false },
        { name: "count", value: "9223372036854775807" },
      ],
    },
  });
  assert.equal(host.state.events[0].args[0].value, 0.25);

  host.setState({ params: [{ name: "level", type: "f32", value: 0.5 }] });
  assert.equal(messages.at(-1).type, "state");
  host.pendingViewReset = true;
  await host.handleMessage({ type: "setParam", name: "level", value: 0.75 });
  assert.equal(messages.at(-1).type, "state");
  assert.equal(messages.at(-1).state.params[0].value, 0.75);
  assert.equal(messages.at(-1).state.viewState.reset, true);
});

test("allows browser playback while buffers are unbound", async () => {
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const previousMutationObserver = globalThis.MutationObserver;
  let starts = 0;
  let paramResets = 0;
  const smoothingChanges = [];
  const runSettings = [];
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
    removeEventListener() {},
    style: {},
  };

  try {
    const host = new BrowserRunViewHost(iframe, {
      start: async () => {
        starts += 1;
      },
      resetParams: async () => {
        paramResets += 1;
      },
      setParamSmoothing: async (milliseconds) => {
        if (milliseconds === 999) throw new Error("duration rejected");
        smoothingChanges.push(milliseconds);
      },
      setRunSettings: async settings => runSettings.push(settings),
    });
    assert.deepEqual(
      {
        sourceSelection: host.state.supportsSourceSelection,
        transport: host.state.supportsTransport,
        deviceSelection: host.state.supportsDeviceSelection,
        scope: host.state.supportsScope,
      },
      {
        sourceSelection: false,
        transport: true,
        deviceSelection: false,
        scope: true,
      },
    );
    assert.equal(host.state.supportsParamSmoothing, true);
    assert.equal(host.state.paramSmoothingMs, null);
    host.setState({ paramSmoothingMs: 50, paramSmoothingDefaultMs: 50 });
    for (const milliseconds of [15.5, 0]) {
      await host.handleMessage({ type: "setParamSmoothing", milliseconds });
      assert.equal(host.state.paramSmoothingMs, milliseconds);
      assert.equal(host.state.paramSmoothingDefaultMs, 50);
    }
    host.setRunning(44_100);
    assert.equal(host.state.supportsRunSettings, true);
    assert.equal(host.state.canSetSampleRate, true);
    assert.equal(host.state.canSetBlockSize, true);
    await host.handleMessage({ type: "setRunSettings", sampleRateHz: 48_000, blockSamples: 256 });
    assert.deepEqual(runSettings, [{ sampleRateHz: 48_000, blockSamples: 256 }]);
    for (const milliseconds of [-1, NaN, Infinity, "5", 999]) {
      await host.handleMessage({ type: "setParamSmoothing", milliseconds });
      assert.equal(host.state.paramSmoothingMs, 0);
      assert.equal(host.state.running, true);
    }
    assert.deepEqual(smoothingChanges, [15.5, 0]);
    host.clearArtifact();
    assert.equal(host.state.fastMath, false);
    assert.equal(host.state.paramSmoothingMs, 0);
    assert.equal(host.state.paramSmoothingDefaultMs, 50);
    const artifact = {
      metadata: {
        compile: {
          sample_rate: 44_100,
          block_size: 256,
          fast_math: true,
        },
        metadata: {
          params: [],
          events: [],
          outputs: [],
          buffers: [
            {
              name: "clip",
              type_repr: "buffer<f32>",
              channels: "mono",
              static_channels: null,
            },
          ],
        },
      },
    };
    host.setArtifact(artifact, new Map());
    assert.equal(host.state.fastMath, true);
    artifact.metadata.compile.fast_math = false;
    host.setArtifact(artifact, new Map());
    assert.equal(host.state.fastMath, false);

    assert.equal(host.state.status, "Stopped");
    assert.equal(host.state.running, false);
    assert.equal(host.state.sampleRateHz, 44_100);
    assert.equal(host.state.blockSamples, 256);
    await host.handleMessage({ type: "start" });
    assert.equal(starts, 1);

    host.updateBufferFile(
      "clip",
      { name: "clip.wav" },
      {
        frames: 96_000,
        channels: 2,
        sampleRate: 48_000,
      },
    );
    assert.deepEqual(
      {
        loadedFrames: host.state.buffers[0].loadedFrames,
        loadedChannels: host.state.buffers[0].loadedChannels,
        loadedSampleRate: host.state.buffers[0].loadedSampleRate,
      },
      { loadedFrames: 96_000, loadedChannels: 2, loadedSampleRate: 48_000 },
    );
    await host.handleMessage({ type: "start" });
    assert.equal(starts, 2);

    host.state.params = [
      { name: "gain", type: "f32", default: 1, value: 0.5 },
      { name: "live_input", type: "bool", default: false, value: true },
    ];
    host.state.events = [
      {
        name: "note",
        args: [{ name: "velocity", default: 1, value: 0.25 }],
      },
    ];
    await host.handleMessage({ type: "resetParams" });
    assert.equal(paramResets, 1);
    assert.equal(host.state.params[0].value, 1);
    assert.equal(host.state.params[1].value, false);
    assert.equal(host.state.events[0].args[0].value, 0.25);

    await host.handleMessage({ type: "resetEventArguments" });
    assert.equal(host.state.params[0].value, 1);
    assert.equal(host.state.params[1].value, false);
    assert.equal(host.state.events[0].args[0].value, 1);
    host.dispose();
  } finally {
    globalThis.window = previousWindow;
    globalThis.document = previousDocument;
    globalThis.MutationObserver = previousMutationObserver;
  }
});

test("browser run view handles readiness, MIDI activity, and audio gesture binding", async (t) => {
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const previousMutationObserver = globalThis.MutationObserver;
  const messages = [];
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
    contentWindow: { postMessage: (message) => messages.push(message) },
    addEventListener: t.mock.fn(),
    removeEventListener: t.mock.fn(),
    style: {},
  };

  try {
    const resume = t.mock.fn();
    const stop = t.mock.fn();
    const loading = [];
    const host = new BrowserRunViewHost(iframe, {
      resume, stop, loadingChanged: value => loading.push(value),
    });
    assert.equal(iframe.style.opacity, "0");
    host.setPath("main.onda");
    assert.equal(messages.length, 0);
    await host.handleMessage({ type: "webviewReady" });
    assert.equal(messages.length, 0, "compiler initialization must not publish an unloaded view");
    await host.handleMessage({ type: "runViewReady", readyId: 1 });
    assert.equal(iframe.style.opacity, "0");
    assert.deepEqual(loading, [true, true], "early readiness must keep loading visible");
    host.setState({ schemaReady: true });
    assert.deepEqual(messages.at(-1).message.state.viewState, {
      reset: true,
      readyId: 1,
    });
    await host.handleMessage({ type: "runViewReady", readyId: 1 });
    assert.equal(iframe.style.opacity, "");
    host.setPath("next.onda");
    assert.equal(iframe.style.opacity, "0");
    assert.deepEqual(messages.at(-1).message.state.viewState, {
      reset: true,
      readyId: 2,
    });
    await host.handleMessage({ type: "runViewReady", readyId: 1 });
    assert.equal(iframe.style.opacity, "0");
    host.setState({ status: "Running" });
    assert.deepEqual(messages.at(-1).message.state, { status: "Running" });
    await host.handleMessage({ type: "runViewReady", readyId: 2 });
    assert.equal(iframe.style.opacity, "");
    host.clearArtifact("next.onda");
    assert.equal(iframe.style.opacity, "0");
    assert.deepEqual(
      messages.findLast(({ message }) => message.type === "state").message.state
        .viewState,
      { reset: true, readyId: 3 },
    );
    await host.handleMessage({ type: "runViewReady", readyId: 2 });
    assert.equal(iframe.style.opacity, "0");
    await host.handleMessage({ type: "runViewReady", readyId: 3 });
    assert.equal(iframe.style.opacity, "");
    assert.deepEqual(loading, [true, true, false, true, false, true, false],
      "only the matching rendered frame may dismiss loading after a reset");
    host.setMidiActivity([60, 64, 127]);
    assert.deepEqual(messages.at(-1), {
      __ondaRunHost: true,
      message: { type: "midiActivity", activeNotes: [60, 64, 127] },
    });

    const onLoad = iframe.addEventListener.mock.calls.find(
      (call) => call.arguments[0] === "load",
    ).arguments[1];
    const frameDocuments = Array.from({ length: 2 }, () => ({
      addEventListener: t.mock.fn(),
      removeEventListener: t.mock.fn(),
    }));
    for (const ownerDocument of frameDocuments) {
      iframe.contentDocument = ownerDocument;
      onLoad();
      assert.equal(host.ready, false);
      assert.equal(iframe.style.opacity, "0");
      const onPointer = ownerDocument.addEventListener.mock.calls.find(
        (call) => call.arguments[0] === "pointerdown",
      ).arguments[1];
      onPointer({ type: "pointerdown", pointerType: "mouse", isTrusted: true });
      onPointer({
        type: "pointerdown",
        pointerType: "mouse",
        isTrusted: true,
        target: { closest: () => ({ disabled: false }) },
      });
    }
    assert.equal(resume.mock.callCount(), 2);
    assert.equal(stop.mock.callCount(), 2);
    assert.equal(frameDocuments[1].removeEventListener.mock.callCount(), 0);
    host.dispose();
    for (const ownerDocument of frameDocuments) {
      assert.deepEqual(
        ownerDocument.removeEventListener.mock.calls.map(
          (call) => call.arguments,
        ),
        ownerDocument.addEventListener.mock.calls.map((call) => call.arguments),
      );
    }
    assert.deepEqual(
      iframe.removeEventListener.mock.calls.map((call) => call.arguments),
      [["load", onLoad]],
    );
  } finally {
    globalThis.window = previousWindow;
    globalThis.document = previousDocument;
    globalThis.MutationObserver = previousMutationObserver;
  }
});

test("browser run view preserves logical structured message shapes and independent defaults", () => {
  const event = {
    name: "configure",
    schema: {
      params: [
        {
          name: "patch",
          ty: {
            kind: "struct",
            name: "Patch",
            fields: [
              {
                name: "id",
                ty: { kind: "scalar", encoding: "i64" },
                default: "9223372036854775807",
              },
              {
                name: "gains",
                ty: {
                  kind: "array",
                  element: { kind: "scalar", encoding: "f64" },
                  len: 2,
                },
              },
            ],
          },
        },
      ],
    },
  };
  const [first] = mergeSchemaEvents([event], []);
  assert.equal(first.args.length, 1);
  assert.equal(first.args[0].type, "Patch");
  assert.deepEqual(first.args[0].value, {
    id: "9223372036854775807",
    gains: [0, 0],
  });
  first.args[0].value.gains[0] = 0.5;
  assert.equal(first.args[0].default.gains[0], 0);
  const [next] = mergeSchemaEvents([event], [first]);
  assert.equal(next.args[0].value.gains[0], 0.5);
  event.schema.params[0].ty.fields[1].ty.element.encoding = "f32";
  assert.equal(mergeSchemaEvents([event], [next])[0].args[0].value.gains[0], 0);
});

test("GPU event dispatch converts nested float wire values without narrowing wide integers", async (t) => {
  const trigger = t.mock.fn();
  const host = Object.create(BrowserRunViewHost.prototype);
  const shape = {
    kind: "struct",
    fields: [
      { name: "id", ty: { kind: "scalar", encoding: "i64" } },
      {
        name: "samples",
        ty: { kind: "slice", element: { kind: "scalar", encoding: "f64" } },
      },
    ],
  };
  host.state = {
    events: [{ name: "configure", args: [{ name: "patch", shape }] }],
  };
  host.handlers = {
    triggerEvent: trigger,
    error: (error) => {
      throw error;
    },
  };
  await host.handleMessage({
    type: "triggerEvent",
    name: "configure",
    values: [
      {
        id: "9223372036854775807",
        samples: ["NaN", "Infinity", "-Infinity", "-0"],
      },
    ],
  });
  assert.deepEqual(trigger.mock.calls[0].arguments, [
    "configure",
    [{ id: "9223372036854775807", samples: [NaN, Infinity, -Infinity, -0] }],
  ]);
});

test("saved GPU event values survive recompilation and explicit project resets discard presentation", () => {
  const host = Object.create(BrowserRunViewHost.prototype);
  host.handlers = {};
  const arg = {
    name: "id",
    shape: { kind: "scalar", encoding: "i64" },
    default: "0",
    value: "0",
  };
  host.state = { path: "main.onda", events: [{ name: "select", args: [arg] }] };
  host.ready = false;
  host.viewResetId = 0;
  host.iframe = { style: {} };
  const saved = {
    path: "main.onda",
    activePanel: "events",
    eventValues: [{ name: "select", args: [arg], values: ["9223372036854775807"] }],
  };
  host.saveViewState(saved);
  assert.equal(arg.value, "9223372036854775807");
  assert.equal(host.viewState.activePanel, "events");
  host.setState({ path: "main.onda" }, { resetViewState: true });
  assert.equal(host.viewState, null);
  assert.equal(host.pendingViewReset, true);
});

test("saved GPU event values require the same ordered names, shapes, and defaults", () => {
  const args = [
    { name: "pitch", shape: { kind: "scalar", encoding: "f64" }, default: 69, value: 69 },
    { name: "velocity", shape: { kind: "scalar", encoding: "f64" }, default: 0.5, value: 0.5 },
  ];
  const view = { path: "main.onda", eventValues: [{ name: "note", args, values: [60, 0.8] }] };
  for (const change of ["none", "reorder", "rename", "shape", "default"]) {
    const host = Object.create(BrowserRunViewHost.prototype);
    const current = structuredClone(args);
    if (change === "reorder") current.reverse();
    if (change === "rename") current[0].name = "frequency";
    if (change === "shape") current[0].shape.encoding = "f32";
    if (change === "default") current[0].default = 72;
    const initial = current.map(arg => arg.value);
    host.state = { path: "main.onda", events: [{ name: "note", args: current }] };
    host.restoreEventValues(view);
    assert.deepEqual(current.map(arg => arg.value), change === "none" ? [60, 0.8] : initial, change);
  }
});

test("saved GPU defaults match through JSON transport for special floats and structured values", async () => {
  const { wireJson } = await import("../gpu/protocol.js");
  const host = Object.create(BrowserRunViewHost.prototype);
  const arg = {
    name: "voice",
    shape: { kind: "struct", fields: [
      { name: "z", ty: { kind: "scalar", encoding: "f64" } },
      { name: "a", ty: { kind: "scalar", encoding: "f64" } },
    ] },
    default: { z: NaN, a: -0 },
    value: { z: 0, a: 0 },
  };
  host.state = { path: "main.onda", events: [{ name: "note", args: [arg] }] };
  const saved = JSON.parse(wireJson({ ...arg, default: { a: -0, z: NaN } }));
  host.restoreEventValues({path:"main.onda",eventValues:[{name:"note",args:[saved],values:[{z:1,a:2}]}]});
  assert.deepEqual(arg.value, { z: 1, a: 2 });
});

test("saved typed arguments restore when the compiler publishes a schema after frame readiness", () => {
  const host = Object.create(BrowserRunViewHost.prototype);
  host.state = { path: "main.onda", events: [], params: [], buffers: [] };
  host.handlers = {};
  host.postState = () => {};
  host.viewState = {
    path: "main.onda",
    eventValues: [{ name: "select", args: [{name:"id",shape:{kind:"scalar",encoding:"i64"},default:"0"}], values: ["9223372036854775807"] }],
  };
  host.setCompiling("main.onda");
  assert.equal(host.state.schemaReady, false);
  host.setArtifact(
    {
      metadata: {
        compile: {},
        metadata: {
          outputs: [],
          params: [],
          buffers: [],
          events: [
            {
              name: "select",
              schema: {
                params: [
                  {
                    name: "id",
                    ty: { kind: "scalar", encoding: "i64" },
                    default: "0",
                  },
                ],
              },
            },
          ],
        },
      },
    },
    new Map(),
  );
  assert.equal(host.state.events[0].args[0].value, "9223372036854775807");
  assert.equal(host.state.schemaReady, true);
});

function printHost(t) {
  const original = {
    requestAnimationFrame: globalThis.requestAnimationFrame,
    cancelAnimationFrame: globalThis.cancelAnimationFrame,
  };
  t.after(() => Object.assign(globalThis, original));
  let scheduled;
  globalThis.requestAnimationFrame = (callback) => {
    scheduled = callback;
    return 1;
  };
  globalThis.cancelAnimationFrame = () => {
    scheduled = null;
  };
  const host = Object.create(BrowserRunViewHost.prototype);
  host.state = {
    logText: "",
    logEntries: [],
    printOverflowCount: 0,
    printTransportDropCount: 0,
  };
  host.logFrame = 0;
  host.sourceFiles = [{ path: "main.onda" }];
  host.ready = false;
  host.handlers = {};
  host.iframe = { style: {} };
  const sent = [];
  host.post = (message) => sent.push(message);
  return {
    host, sent,
    flush() { const callback = scheduled; scheduled = null; callback?.(); },
    pending: () => scheduled,
  };
}

test("print batches are bounded and coalesced to one partial publication per frame", (t) => {
  const { host, sent, flush, pending } = printHost(t);
  for (let i = 0; i < 100; i++)
    host.appendPrint({
      text: "a".repeat(1000),
      overflowCount: 1,
      transportDropCount: 2,
    });
  assert.equal(host.state.logText.length, 65536);
  assert.equal(sent.length, 0);
  flush();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].state.printOverflowCount, 100);
  assert.equal(sent[0].state.printTransportDropCount, 200);
  assert.equal(sent[0].state.events, undefined);
  host.appendPrint({ text: "later" });
  host.handleMessage({ type: "clearLog" });
  assert.equal(pending(), null);
  assert.equal(host.state.logText, "");
  assert.deepEqual(host.state.logEntries, []);
});

test("clearing an artifact discards pending print publications and accepts new prints", (t) => {
  const { host, sent, flush } = printHost(t);
  host.appendPrint({ text: "old output\n" });
  host.clearArtifact("next.onda");
  const publications = sent.length;
  flush();
  assert.equal(sent.length, publications, "cleared output must not reveal an empty log");
  assert.equal(host.state.logRevealed, false);

  host.appendPrint({ text: "new output\n" });
  flush();
  assert.equal(sent.at(-1).type, "state");
  assert.equal(sent.at(-1).state.logText, "new output\n");
  assert.equal(sent.at(-1).state.logRevealed, true);
});

test("print source context survives coalescing, missing entries, and partial lines", (t) => {
  const { host, sent, flush } = printHost(t);
  const context = (line) => ({
    source: { file: "main.onda", line }, lexicalOwner: "Voice.sample",
  });
  host.appendPrint({ text: "hello", entries: [{
    ...context(12), source: {file:0, line:12}, values: [42n],
  }] });
  host.appendPrint({ text: " world\nnext\nlast", entries: [context(20), context(21), context(22)] });
  host.appendPrint({ text: "\nplain\n" });
  flush();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].state.logText, "hello world\nnext\nlast\nplain\n");
  assert.deepEqual(sent[0].state.logEntries, [context(12), context(21), context(22), null]);
  assert.doesNotThrow(() => JSON.stringify(sent[0]), "decoded BigInt values must not enter the JSON log state");
  host.clearArtifact();
  assert.equal(host.state.logText, "");
  assert.deepEqual(host.state.logEntries, []);
  assert.deepEqual(host.sourceFiles, []);
});

test("print history trims source entries with their lines at the character budget", (t) => {
  const { host, sent, flush } = printHost(t);
  const lines = Array.from({ length:6000 }, (_, i) => `message ${String(i).padStart(6,"0")}\n`);
  const entries = lines.map((_, line) => ({ source:{ file:"main.onda", line }, lexicalOwner:"sample" }));
  const text = lines.join("");
  host.appendPrint({ text, entries });
  const removedLines = text.slice(0, -65536).split("\n").length - 1;
  assert.equal(host.state.logText, text.slice(-65536));
  assert.deepEqual(host.state.logEntries, entries.slice(removedLines));
  host.appendPrint({ text:"final\n", entries:[{ source:{file:"final.onda", line:3} }] });
  flush();
  const state = sent[0].state;
  assert.equal(state.logText.length, 65536);
  assert.equal(state.logEntries.length, state.logText.split("\n").length - 1);
  assert.equal(state.logEntries.at(-1).source.file, "final.onda");
});

test("print history preserves Unicode pairs and source context at the truncation boundary", (t) => {
  const { host, sent, flush } = printHost(t);
  const entries = [1, 2].map(line => ({ source: { file: "main.onda", line }, lexicalOwner: "sample" }));
  host.appendPrint({ text: "discarded\n" + "😀".repeat(32768) + "x", entries });
  flush();
  assert.equal(sent[0].state.logText.isWellFormed(), true);
  assert.equal(sent[0].state.logText, "😀".repeat(32767) + "x");
  assert.ok(sent[0].state.logText.length <= 65536);
  assert.deepEqual(sent[0].state.logEntries, entries.slice(1));
});

test("GPU wire JSON preserves non-finite floats, signed zero, and exact integer strings", async () => {
  const { wireJson } = await import("../gpu/protocol.js");
  assert.deepEqual(
    JSON.parse(
      wireJson({
        values: [NaN, Infinity, -Infinity, -0, "9223372036854775807"],
      }),
    ),
    { values: ["NaN", "Infinity", "-Infinity", "-0", "9223372036854775807"] },
  );
});
