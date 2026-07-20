import assert from "node:assert/strict";
import test from "node:test";

import {
  ONDA_AUDIO_WORKLET_PROCESSOR_NAME,
  OndaAudioProcessor,
  compileOndaProcessorModule,
  createOndaAudioProcessor,
  ondaAudioWorkletNodeOptions,
} from "../src/index.js";

const FIXTURE_MIR_SCHEMA_VERSION = 1;

function artifact() {
  return {
    wasm: new Uint8Array([
      0, 97, 115, 109, 1, 0, 0, 0,
      1, 4, 1, 96, 0, 0,
      3, 3, 2, 0, 0,
      5, 3, 1, 0, 1,
      6, 7, 1, 127, 0, 65, 128, 8, 11,
      7, 51, 4,
      6, 109, 101, 109, 111, 114, 121, 2, 0,
      11, 95, 95, 104, 101, 97, 112, 95, 98, 97, 115, 101, 3, 0,
      9, 111, 110, 100, 97, 95, 105, 110, 105, 116, 0, 0,
      12, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 0, 1,
      10, 7, 2, 2, 0, 11, 2, 0, 11,
    ]),
    metadata: {
      format: "onda-processor",
      format_version: 3,
      abi_version: 1,
      artifact_kind: "webassembly_module",
      backend: "test",
      mir_schema_version: FIXTURE_MIR_SCHEMA_VERSION,
      integration: {
        required_symbols: ["memory", "__heap_base", "onda_init", "onda_process"],
        one_processor_per_artifact: true,
        profile: {
          kind: "core_webassembly_module",
          memory_export: "memory",
          heap_base_export: "__heap_base",
        },
      },
      target: {
        triple: "wasm32-unknown-unknown",
        byte_order: "little_endian",
        pointer_model: "linear_memory_offset",
        pointer_width_bits: 32,
        calling_convention: "core-wasm",
      },
      compile: { sample_rate: 48_000, block_size: 128 },
      runtime: {
        state_size_bytes: 0,
        state_align_bytes: 1,
        param_size_bytes: 0,
        param_align_bytes: 1,
        snapshot_size_bytes: 0,
      },
      exports: {
        memory: "memory",
        heap_base: "__heap_base",
        init: "onda_init",
        process: "onda_process",
        events: [],
      },
      metadata: {
        states: [],
        inputs: [{ channel_count: 2 }],
        outputs: [{ channel_count: 1 }],
        control_outputs: [],
        params: [],
        buffers: [],
        events: [],
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
  const options = ondaAudioWorkletNodeOptions(artifact(), {
    params: { gain: 1 },
    nodeOptions: {
      numberOfInputs: 0,
      outputChannelCount: [8],
    },
  });
  assert.equal(options.numberOfInputs, 1);
  assert.equal(options.numberOfOutputs, 1);
  assert.equal(options.channelCount, 2);
  assert.equal(options.channelInterpretation, "discrete");
  assert.deepEqual(options.outputChannelCount, [1]);
  assert.deepEqual(options.processorOptions.params, { gain: 1 });
});

test("rejects invalid processor channel metadata", () => {
  const source = artifact();
  source.metadata.metadata.outputs[0].channel_count = -1;
  assert.throws(
    () => ondaAudioWorkletNodeOptions(source),
    /invalid channel_count/,
  );
});

test("registers the worklet before constructing the public processor node", async () => {
  const modules = [];
  const context = {
    sampleRate: 48_000,
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
  assert.equal(
    processor.node.options.processorOptions.wasmModule instanceof WebAssembly.Module,
    true,
  );
  assert.equal("wasmBytes" in processor.node.options.processorOptions, false);
});

test("rejects a processor compiled for a different AudioContext sample rate", async () => {
  const context = {
    sampleRate: 44_100,
    audioWorklet: { addModule: async () => {} },
  };
  await assert.rejects(
    createOndaAudioProcessor(context, artifact(), {
      AudioWorkletNode: FakeNode,
    }),
    /compiled for 48000 Hz.*runs at 44100 Hz/,
  );
});

test("rejects Web Audio processors without a render-quantum audio surface", () => {
  const source = artifact();
  source.metadata.metadata.inputs = [];
  source.metadata.metadata.outputs = [];
  assert.throws(
    () => ondaAudioWorkletNodeOptions(source),
    /at least one audio input or output/,
  );
});

test("can reuse a processor module compiled off the audio rendering thread", async () => {
  const source = artifact();
  const compiledModule = await compileOndaProcessorModule(source);
  const options = ondaAudioWorkletNodeOptions(source, {
    compiledModule,
    eventPayloadCapacityBytes: 8192,
  });

  assert.equal(options.processorOptions.wasmModule, compiledModule);
  assert.equal(options.processorOptions.eventPayloadCapacityBytes, 8192);
  assert.equal("wasmBytes" in options.processorOptions, false);
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
