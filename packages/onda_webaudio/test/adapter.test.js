import assert from "node:assert/strict";
import test from "node:test";

import {
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
} from "@onda-lang/processor-abi";

import {
  ONDA_AUDIO_WORKLET_PROCESSOR_NAME,
  ONDA_INIT_FULL,
  ONDA_INIT_PRESERVE_PINNED,
  OndaAudioProcessor,
  compileOndaProcessorModule,
  createOndaAudioProcessor,
  createOndaAudioProcessorInitialized,
  ondaAudioWorkletNodeOptions,
} from "../src/index.js";

const FIXTURE_MIR_SCHEMA_VERSION = 8;

function artifact() {
  const port = (name, arrayLen) => ({
    name,
    type_repr: arrayLen === 1 ? "f32" : `f32[${arrayLen}]`,
    scalar: "f32",
    array_len: arrayLen,
    element_size_bytes: 4,
    slot_offset: 0,
    byte_offset: null,
    state_byte_offset: null,
    byte_size: arrayLen * 4,
    default_reprs: null,
    range_min_repr: null,
    range_max_repr: null,
    param_control: null,
  });
  return {
    wasm: new Uint8Array([
      0, 97, 115, 109, 1, 0, 0, 0,
      1, 4, 1, 96, 0, 0,
      3, 3, 2, 0, 0,
      5, 3, 1, 0, 1,
      6, 7, 1, 127, 0, 65, 128, 8, 11,
      7, 61, 4,
      6, 109, 101, 109, 111, 114, 121, 2, 0,
      11, 95, 95, 104, 101, 97, 112, 95, 98, 97, 115, 101, 3, 0,
      19, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 111, 114, 95, 105, 110, 105, 116, 0, 0,
      12, 111, 110, 100, 97, 95, 112, 114, 111, 99, 101, 115, 115, 0, 1,
      10, 7, 2, 2, 0, 11, 2, 0, 11,
    ]),
    metadata: {
      format: "onda-processor",
      format_version: PROCESSOR_ARTIFACT_FORMAT_VERSION,
      abi_version: PROCESSOR_ABI_VERSION,
      artifact_kind: "webassembly_module",
      backend: "test",
      mir_schema_version: FIXTURE_MIR_SCHEMA_VERSION,
      integration: {
        required_symbols: ["memory", "__heap_base", "onda_processor_init", "onda_process"],
        one_processor_per_artifact: true,
        profile: {
          kind: "core_webassembly_module",
          imports: [],
          memory_export: "memory",
          heap_base_export: "__heap_base",
        },
      },
      target: {
        triple: "wasm32-unknown-unknown",
        cpu: "generic",
        features: "",
        reloc_model: "static",
        code_model: "default",
        opt_level: "4",
        abi_name: null,
        data_layout: "e-m:e-p:32:32-i64:64-n32:64-S128",
        byte_order: "little_endian",
        pointer_model: "linear_memory_offset",
        pointer_width_bits: 32,
        calling_convention: "core-wasm",
      },
      compile: { sample_rate: 48_000, block_size: 128, fast_math: false },
      runtime: {
        state_size_bytes: 0,
        state_align_bytes: 1,
        param_size_bytes: 0,
        param_align_bytes: 1,
        snapshot_size_bytes: 0,
        state_initialization: "zeroed",
        snapshot_format_version: PROCESSOR_SNAPSHOT_FORMAT_VERSION,
        snapshot_byte_order: "little_endian",
        snapshot_restore_base: "post_init_physical_state_image",
        requires_full_blocks: false,
        delegate_record_header_size_bytes: 12,
        print_record_header_size_bytes: 12,
      },
      exports: {
        memory: "memory",
        heap_base: "__heap_base",
        init: "onda_processor_init",
        process: "onda_process",
        events: [],
      },
      metadata: {
        states: [],
        inputs: [port("in1", 2)],
        outputs: [port("out1", 1)],
        control_outputs: [],
        params: [],
        buffers: [],
        events: [],
        delegates: [],
        source_files: [],
        log_sites: [],
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

test("closed processors reject new work and settle pending requests", async () => {
  const node = new FakeNode({}, ONDA_AUDIO_WORKLET_PROCESSOR_NAME, {});
  const processor = new OndaAudioProcessor(node, artifact().metadata);
  const pending = processor.trigger("note");
  const reason = new Error("closed by host");

  processor.close(reason);
  processor.close();

  await assert.rejects(pending, reason);
  await assert.rejects(processor.trigger("note"), reason);
  assert.throws(() => processor.onPrint(() => {}), reason);
  assert.equal(node.port.messages.length, 1);
});

test("derives explicit Web Audio channel options from processor metadata", () => {
  const options = ondaAudioWorkletNodeOptions(artifact(), {
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
  assert.deepEqual(options.processorOptions.params, {});
});

test("rejects unknown initial parameters before worklet construction", () => {
  assert.throws(
    () => ondaAudioWorkletNodeOptions(artifact(), { params: { gain: 1 } }),
    /unknown Onda parameter 'gain'/,
  );
  assert.throws(
    () => ondaAudioWorkletNodeOptions(artifact(), { params: [1] }),
    /unknown Onda parameter '0'/,
  );
  assert.throws(
    () => ondaAudioWorkletNodeOptions(artifact(), { onPrint: true }),
    /initial print listener must be a function/,
  );
});

test("rejects invalid processor channel metadata", () => {
  const source = artifact();
  source.metadata.metadata.outputs[0].array_len = -1;
  assert.throws(
    () => ondaAudioWorkletNodeOptions(source),
    /array_len/,
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
  assert.equal(processor.node.options.processorOptions.initialize, false);
});

test("initialized creation requests full initialization in the worklet constructor", async () => {
  const context = {
    sampleRate: 48_000,
    audioWorklet: { addModule: async () => {} },
  };
  const processor = await createOndaAudioProcessorInitialized(context, artifact(), {
    AudioWorkletNode: FakeNode,
  });
  assert.equal(processor.node.options.processorOptions.initialize, true);
  assert.equal(processor.node.options.processorOptions.printCollectionEnabled, false);
});

test("initialized creation installs its print listener before worklet output arrives", async () => {
  const source = artifact();
  source.metadata.metadata.log_sites = [{
    index: 0,
    label: "init",
    source: { file: null, line: 1, column: 1, end_line: 1, end_column: 10 },
    lexical_owner: "program",
    declaration: "init",
    argument_types: ["i32"],
    payload_size_bytes: 4,
  }];
  const context = {
    sampleRate: 48_000,
    audioWorklet: { addModule: async () => {} },
  };
  const batches = [];
  const processor = await createOndaAudioProcessorInitialized(context, source, {
    AudioWorkletNode: FakeNode,
    onPrint: (batch) => batches.push(batch),
  });
  assert.equal(processor.node.options.processorOptions.printCollectionEnabled, true);
  assert.equal(processor.node.options.processorOptions.printSubscriptionId, 1);

  const storage = new Uint8Array(16);
  const view = new DataView(storage.buffer);
  view.setUint32(0, 0, true);
  view.setUint32(4, 4, true);
  view.setUint32(8, 0, true);
  view.setInt32(12, 42, true);
  processor.node.port.reply({
    type: "onda-print-records",
    operation: "processor init",
    storage,
    usedBytes: storage.byteLength,
    recordCount: 1,
    overflowCount: 0,
    transportDropCount: 0,
    subscriptionId: 1,
  });
  assert.equal(batches[0].text, "init: 42\n");
  processor.close();
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

  const init = processor.init(ONDA_INIT_PRESERVE_PINNED);
  const initRequest = node.port.messages.at(-1);
  assert.equal(initRequest.type, "init");
  assert.equal(initRequest.mode, ONDA_INIT_PRESERVE_PINNED);
  node.port.reply({ type: "onda-ok", requestId: initRequest.requestId });
  await init;

  const fullInit = processor.init(ONDA_INIT_FULL);
  const fullInitRequest = node.port.messages.at(-1);
  assert.equal(fullInitRequest.type, "init");
  assert.equal(fullInitRequest.mode, ONDA_INIT_FULL);
  node.port.reply({ type: "onda-ok", requestId: fullInitRequest.requestId });
  await fullInit;
  processor.close();
});

test("formats worklet print records on the main side", () => {
  const source = artifact();
  source.metadata.metadata.log_sites = [{
    index: 0,
    label: "value",
    source: { file: null, line: 1, column: 1, end_line: 1, end_column: 10 },
    lexical_owner: "program",
    declaration: "sample",
    argument_types: ["i32"],
    payload_size_bytes: 4,
  }];
  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node, source.metadata);
  const batches = [];
  const unsubscribe = processor.onPrint((batch) => batches.push(batch));
  const subscription = node.port.messages.at(-1);
  assert.equal(subscription.type, "print-subscription");
  assert.equal(subscription.enabled, true);
  const storage = new Uint8Array(16);
  const view = new DataView(storage.buffer);
  view.setUint32(0, 0, true);
  view.setUint32(4, 4, true);
  view.setUint32(8, 0, true);
  view.setInt32(12, 42, true);
  node.port.reply({
    type: "onda-print-records",
    operation: "process",
    storage,
    usedBytes: storage.byteLength,
    recordCount: 1,
    overflowCount: 2,
    transportDropCount: 3,
    subscriptionId: subscription.subscriptionId,
  });
  assert.equal(batches[0].text, "value: 42\n");
  assert.equal(batches[0].entries[0].values[0].value, 42);
  assert.equal(batches[0].overflowCount, 2);
  assert.equal(batches[0].transportDropCount, 3);
  assert.deepEqual(node.port.messages.at(-1), { type: "print-ack", storage });
  assert.equal(unsubscribe(), true);
  assert.deepEqual(node.port.messages.at(-1), {
    type: "print-subscription",
    enabled: false,
    subscriptionId: subscription.subscriptionId,
  });
  node.port.reply({
    type: "onda-print-records",
    operation: "stale process",
    storage,
    usedBytes: storage.byteLength,
    recordCount: 1,
    overflowCount: 0,
    transportDropCount: 0,
    subscriptionId: subscription.subscriptionId,
  });
  assert.equal(batches.length, 1);
  assert.deepEqual(node.port.messages.at(-1), { type: "print-ack", storage });
  processor.close();
});

test("subscribes lazily and decodes delegate records on the main side", () => {
  const source = artifact();
  source.metadata.metadata.delegates = [{
    name: "report",
    params: [
      {
        name: "code",
        scalar: "i32",
        array_len: 1,
        is_slice: false,
        element_size_bytes: 4,
      },
      {
        name: "values",
        scalar: "f32",
        array_len: 0,
        is_slice: true,
        element_size_bytes: 4,
      },
    ],
  }];
  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node, source.metadata);
  const batches = [];
  const unsubscribe = processor.onDelegates((batch) => batches.push(batch));
  const subscription = node.port.messages.at(-1);
  assert.equal(subscription.type, "delegate-subscription");
  assert.equal(subscription.enabled, true);

  const storage = new Uint8Array(28);
  const view = new DataView(storage.buffer);
  view.setUint32(0, 0, true);
  view.setUint32(4, 16, true);
  view.setUint32(8, 0, true);
  view.setInt32(12, 7, true);
  view.setInt32(16, 2, true);
  view.setFloat32(20, 1.25, true);
  view.setFloat32(24, -2.5, true);
  node.port.reply({
    type: "onda-delegate-records",
    operation: "process",
    storage,
    usedBytes: storage.byteLength,
    recordCount: 1,
    overflowCount: 2,
    transportDropCount: 3,
    subscriptionId: subscription.subscriptionId,
  });

  assert.deepEqual(batches, [{
    type: "onda-delegates",
    operation: "process",
    occurrences: [{
      sequence: 0,
      index: 0,
      name: "report",
      values: { code: 7, values: [1.25, -2.5] },
    }],
    overflowCount: 2,
    transportDropCount: 3,
  }]);
  assert.deepEqual(node.port.messages.at(-1), { type: "delegate-ack", storage });

  assert.equal(unsubscribe(), true);
  assert.deepEqual(node.port.messages.at(-1), {
    type: "delegate-subscription",
    enabled: false,
    subscriptionId: subscription.subscriptionId,
  });
  processor.close();
});

test("delivers print and delegate callbacks in call-local source order", () => {
  const source = artifact();
  source.metadata.metadata.log_sites = [{
    index: 0,
    label: null,
    source: { file: null, line: 1, column: 1, end_line: 1, end_column: 1 },
    lexical_owner: "program",
    declaration: "sample",
    argument_types: ["i32"],
    payload_size_bytes: 4,
  }];
  source.metadata.metadata.delegates = [{
    name: "tick",
    params: [{
      name: "value",
      scalar: "i32",
      array_len: 1,
      is_slice: false,
      element_size_bytes: 4,
    }],
  }];
  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node, source.metadata);
  const delivered = [];
  processor.onPrint((batch) => delivered.push(`print:${batch.entries[0].values[0].value}`));
  const printSubscription = node.port.messages.at(-1).subscriptionId;
  processor.onDelegates((batch) => delivered.push(`delegate:${batch.occurrences[0].values.value}`));
  const delegateSubscription = node.port.messages.at(-1).subscriptionId;

  const printStorage = new Uint8Array(32);
  const printView = new DataView(printStorage.buffer);
  for (const [offset, sequence, value] of [[0, 0, 11], [16, 2, 33]]) {
    printView.setUint32(offset, 0, true);
    printView.setUint32(offset + 4, 4, true);
    printView.setUint32(offset + 8, sequence, true);
    printView.setInt32(offset + 12, value, true);
  }
  node.port.reply({
    type: "onda-print-records",
    operation: "process",
    executionOutputId: 7,
    storage: printStorage,
    usedBytes: printStorage.byteLength,
    recordCount: 2,
    overflowCount: 0,
    transportDropCount: 0,
    subscriptionId: printSubscription,
  });

  const delegateStorage = new Uint8Array(16);
  const delegateView = new DataView(delegateStorage.buffer);
  delegateView.setUint32(0, 0, true);
  delegateView.setUint32(4, 4, true);
  delegateView.setUint32(8, 1, true);
  delegateView.setInt32(12, 22, true);
  node.port.reply({
    type: "onda-delegate-records",
    operation: "process",
    executionOutputId: 7,
    storage: delegateStorage,
    usedBytes: delegateStorage.byteLength,
    recordCount: 1,
    overflowCount: 0,
    transportDropCount: 0,
    subscriptionId: delegateSubscription,
  });
  assert.deepEqual(delivered, []);
  node.port.reply({
    type: "onda-execution-output-end",
    operation: "process",
    executionOutputId: 7,
  });
  assert.deepEqual(delivered, ["print:11", "delegate:22", "print:33"]);
  processor.close();
});

test("converts normalized parameters before posting a plain worklet write", async () => {
  const source = artifact();
  source.metadata.metadata.params = [{
    name: "cutoff",
    type_repr: "f32",
    scalar: "f32",
    array_len: 1,
    element_size_bytes: 4,
    slot_offset: 0,
    byte_offset: 0,
    state_byte_offset: null,
    byte_size: 4,
    default_reprs: ["440"],
    range_min_repr: "20",
    range_max_repr: "20000",
    param_control: {
      scale: "log",
      curve: null,
      unit: "Hz",
      step_repr: null,
      step_count: null,
    },
  }];
  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node, source.metadata);

  const pending = processor.setParamNormalized("cutoff", 0.5);
  const request = node.port.messages.at(-1);
  assert.equal(request.type, "set-param");
  assert.ok(Math.abs(request.value - Math.sqrt(20 * 20_000)) < 1e-12);
  node.port.reply({ type: "onda-ok", requestId: request.requestId });
  await pending;
  processor.close();
});

test("constrains plain parameters before posting to the worklet", async () => {
  const source = artifact();
  source.metadata.runtime.param_size_bytes = 4;
  source.metadata.runtime.param_align_bytes = 4;
  source.metadata.metadata.params = [{
    name: "mode",
    type_repr: "i32",
    scalar: "i32",
    array_len: 1,
    element_size_bytes: 4,
    slot_offset: 0,
    byte_offset: 0,
    state_byte_offset: null,
    byte_size: 4,
    default_reprs: ["0"],
    range_min_repr: "0",
    range_max_repr: "10",
    param_control: {
      scale: "linear",
      curve: null,
      unit: null,
      step_repr: "2",
      step_count: 5,
    },
  }];
  const options = ondaAudioWorkletNodeOptions(source, {
    params: { mode: 100 },
  });
  assert.deepEqual(options.processorOptions.params, { mode: 10 });

  const node = { port: new FakePort() };
  const processor = new OndaAudioProcessor(node, source.metadata);

  const pending = processor.setParam("mode", 3.2);
  const request = node.port.messages.at(-1);
  assert.equal(request.type, "set-param");
  assert.equal(request.value, 4);
  node.port.reply({ type: "onda-ok", requestId: request.requestId });
  await pending;
  processor.close();
});
