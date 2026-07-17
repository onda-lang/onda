import assert from "node:assert/strict";
import test from "node:test";

import {
  ONDA_AUDIO_WORKLET_PROCESSOR_NAME,
  OndaAudioProcessor,
  createOndaAudioProcessor,
  ondaAudioWorkletNodeOptions,
} from "../src/index.js";

function artifact() {
  return {
    wasm: new Uint8Array([0, 97, 115, 109]),
    metadata: {
      format: "onda-processor",
      format_version: 3,
      abi_version: 1,
      artifact_kind: "webassembly_module",
      integration: { profile: { kind: "core_webassembly_module" } },
      target: {
        pointer_model: "linear_memory_offset",
        pointer_width_bits: 32,
      },
      metadata: {
        inputs: [{ channel_count: 2 }],
        outputs: [{ channel_count: 1 }],
      },
    },
  };
}

class FakePort {
  constructor() {
    this.listeners = new Set();
    this.messages = [];
  }

  addEventListener(_type, listener) {
    this.listeners.add(listener);
  }

  removeEventListener(_type, listener) {
    this.listeners.delete(listener);
  }

  postMessage(message) {
    this.messages.push(message);
  }

  start() {}

  reply(message) {
    for (const listener of this.listeners) listener({ data: message });
  }
}

class FakeNode {
  constructor(context, name, options) {
    this.context = context;
    this.name = name;
    this.options = options;
    this.port = new FakePort();
  }
}

test("derives explicit Web Audio channel options from processor metadata", () => {
  const options = ondaAudioWorkletNodeOptions(artifact(), { params: { gain: 1 } });
  assert.equal(options.numberOfInputs, 1);
  assert.equal(options.numberOfOutputs, 1);
  assert.equal(options.channelCount, 2);
  assert.deepEqual(options.outputChannelCount, [1]);
  assert.deepEqual(options.processorOptions.params, { gain: 1 });
});

test("registers the worklet before constructing the public processor node", async () => {
  const modules = [];
  const context = {
    audioWorklet: {
      addModule: async (url) => modules.push(String(url)),
    },
  };
  const processor = await createOndaAudioProcessor(context, artifact(), {
    workletUrl: "/onda-worklet.js",
    AudioWorkletNode: FakeNode,
  });
  assert.equal(modules.length, 1);
  assert.equal(processor.node.name, ONDA_AUDIO_WORKLET_PROCESSOR_NAME);
});

test("correlates control responses and preserves caller snapshot storage", async () => {
  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node);
  const pending = processor.setParam("gain", 0.5);
  const request = node.port.messages.at(-1);
  node.port.reply({ type: "onda-ok", requestId: request.requestId });
  await pending;

  const snapshot = new Uint8Array([1, 2, 3]);
  const restore = processor.restoreSnapshot(snapshot);
  const restoreRequest = node.port.messages.at(-1);
  assert.notEqual(restoreRequest.snapshot.buffer, snapshot.buffer);
  assert.deepEqual([...snapshot], [1, 2, 3]);
  node.port.reply({ type: "onda-ok", requestId: restoreRequest.requestId });
  await restore;
  processor.close();
});
