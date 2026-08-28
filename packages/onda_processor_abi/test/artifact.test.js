import assert from "node:assert/strict";
import { webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  PROCESSOR_ABI_VERSION,
  PROCESSOR_ARTIFACT_FORMAT_VERSION,
  PROCESSOR_SNAPSHOT_FORMAT_VERSION,
  createParamDomain,
  createParamControl,
  constrainParamPlain,
  createProcessorArtifactFiles,
  decodeDelegateRecords,
  formatPrintBatch,
  formatPrintRecords,
  loadProcessorArtifactFiles,
  paramNormalizedToPlain,
  paramPlainToNormalized,
  readDelegateBatch,
  validateProcessorArtifact,
  validateProcessorModule,
  validateProcessorMetadata,
  writeDelegateBatch,
  writeExecutionOutput,
  writePrintBatch,
} from "../src/index.js";

if (!globalThis.crypto) globalThis.crypto = webcrypto;

test("validates the descriptor fixture shared with the Rust schema", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/processor-descriptor-v9.json", import.meta.url),
    "utf8",
  ));
  assert.equal(
    validateProcessorMetadata(fixture).format_version,
    PROCESSOR_ARTIFACT_FORMAT_VERSION,
  );
  assert.equal(fixture.metadata.states[0].integer_range.mode, "wrap");

  const missingCanonicalField = structuredClone(fixture);
  delete missingCanonicalField.metadata.inputs[0].default_reprs;
  assert.throws(
    () => validateProcessorMetadata(missingCanonicalField),
    /default_reprs must be present/,
  );

  const inconsistentLayout = structuredClone(fixture);
  inconsistentLayout.metadata.inputs[0].byte_size = 8;
  assert.throws(
    () => validateProcessorMetadata(inconsistentLayout),
    /byte_size does not match/,
  );

  const inconsistentLogPayload = structuredClone(fixture);
  inconsistentLogPayload.metadata.log_sites = [{
    index: 0,
    label: null,
    source: { file: null, line: 1, column: 1, end_line: 1, end_column: 2 },
    lexical_owner: "program",
    declaration: "sample",
    argument_types: ["f32"],
    payload_size_bytes: 5,
  }];
  assert.throws(
    () => validateProcessorMetadata(inconsistentLogPayload),
    /payload_size_bytes must be/,
  );

  const readOnlyUse = structuredClone(fixture);
  readOnlyUse.metadata.buffers[0].may_write = false;
  assert.equal(
    validateProcessorMetadata(readOnlyUse).metadata.buffers[0].access,
    "read_write",
  );

  const grouped = structuredClone(fixture);
  grouped.metadata.buffer_arrays = [{ name: "bank", first_buffer: 0, len: 1 }];
  assert.equal(validateProcessorMetadata(grouped).metadata.buffer_arrays[0].name, "bank");
  grouped.metadata.buffer_arrays.push({ name: "other", first_buffer: 0, len: 1 });
  assert.throws(
    () => validateProcessorMetadata(grouped),
    /overlaps another buffer array/,
  );

  const rangedState = structuredClone(fixture);
  rangedState.metadata.states[0] = {
    ...rangedState.metadata.states[0],
    type_repr: "i64",
    scalar: "i64",
    element_size_bytes: 8,
    byte_size: 8,
    integer_range: {
      min: { type: "i64", value: "-9223372036854775808" },
      max: { type: "i64", value: "9223372036854775807" },
      mode: "wrap",
    },
  };
  rangedState.runtime.snapshot_size_bytes = 8;
  assert.equal(
    validateProcessorMetadata(rangedState).metadata.states[0].integer_range.mode,
    "wrap",
  );
  for (const mutate of [
    (range) => { range.min.type = "i32"; },
    (range) => { range.max.value = "9223372036854775808"; },
    (range) => { range.min.value = "01"; },
    (range) => { range.mode = "saturate"; },
  ]) {
    const invalid = structuredClone(rangedState);
    mutate(invalid.metadata.states[0].integer_range);
    assert.throws(() => validateProcessorMetadata(invalid), /integer_range/);
  }

  const invalidReadOnlyWrite = structuredClone(fixture);
  invalidReadOnlyWrite.metadata.buffers[0].access = "read_only";
  assert.throws(
    () => validateProcessorMetadata(invalidReadOnlyWrite),
    /may_write requires read_write access/,
  );
});

test("formats packed print records with width-aware canonical scalars", () => {
  const storage = new Uint8Array(8 + 4 + 8 + 8 + 1);
  const view = new DataView(storage.buffer);
  view.setUint32(0, 0, true);
  view.setUint32(4, 21, true);
  view.setFloat32(8, 1.234567, true);
  view.setFloat64(12, -0, true);
  view.setBigInt64(20, 9_007_199_254_740_993n, true);
  view.setUint8(28, 1);
  const metadata = {
    target: { byte_order: "little_endian" },
    metadata: {
      log_sites: [{
        index: 0,
        label: "value\0\n",
        source: { file: null, line: 1, column: 1, end_line: 1, end_column: 1 },
        lexical_owner: "program",
        declaration: "sample",
        argument_types: ["f32", "f64", "i64", "bool"],
        payload_size_bytes: 21,
      }],
    },
  };
  const result = formatPrintRecords(storage, storage.byteLength, metadata, 3);
  assert.equal(result.text, "value\\0\\n: 1.234567 -0.0 9007199254740993 true\n");
  assert.equal(result.entries[0].values[0].value, Math.fround(1.234567));
  assert.equal(result.overflowCount, 3);

  const memory = new WebAssembly.Memory({ initial: 1 });
  writePrintBatch(memory, 0, 32, storage.byteLength);
  new Uint8Array(memory.buffer, 32, storage.byteLength).set(storage);
  const batch = new DataView(memory.buffer);
  batch.setUint32(8, storage.byteLength, true);
  batch.setUint32(4, 0, true);
  assert.throws(
    () => formatPrintBatch(memory, 0, metadata),
    /usedBytes exceeds capacityBytes/,
  );
  batch.setUint32(4, storage.byteLength, true);
  batch.setUint32(12, 0, true);
  assert.throws(
    () => formatPrintBatch(memory, 0, metadata),
    /recordCount does not match packed storage/,
  );
});

test("escapes every print-label record separator", () => {
  const storage = new Uint8Array(8);
  const metadata = {
    target: { byte_order: "little_endian" },
    metadata: {
      log_sites: [{
        index: 0,
        label: "\0\\\n\r\t\u0007\u000b\u000c\u007f\u0085\u2028\u2029sound",
        source: { file: null, line: 1, column: 1, end_line: 1, end_column: 1 },
        lexical_owner: "program",
        declaration: "sample",
        argument_types: [],
        payload_size_bytes: 0,
      }],
    },
  };

  assert.equal(
    formatPrintRecords(storage, storage.byteLength, metadata).text,
    "\\0\\\\\\n\\r\\t\\u{7}\\u{b}\\u{c}\\u{7f}\\u{85}\\u{2028}\\u{2029}sound\n",
  );
});

test("matches native canonical formatting for deterministic randomized float bits", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/print-float-parity.json", import.meta.url),
    "utf8",
  ));
  const metadata = (scalar, payloadSize) => ({
    target: { byte_order: "little_endian" },
    metadata: {
      log_sites: [{
        index: 0,
        label: null,
        source: { file: null, line: 0, column: 0, end_line: 0, end_column: 0 },
        lexical_owner: "program",
        declaration: null,
        argument_types: [scalar],
        payload_size_bytes: payloadSize,
      }],
    },
  });

  for (const entry of fixture.f32) {
    const storage = new Uint8Array(12);
    const view = new DataView(storage.buffer);
    view.setUint32(4, 4, true);
    view.setUint32(8, Number.parseInt(entry.bits, 16), true);
    assert.equal(formatPrintRecords(storage, storage.length, metadata("f32", 4)).text, `${entry.text}\n`);
  }
  for (const entry of fixture.f64) {
    const storage = new Uint8Array(16);
    const view = new DataView(storage.buffer);
    view.setUint32(4, 8, true);
    view.setBigUint64(8, BigInt(`0x${entry.bits}`), true);
    assert.equal(formatPrintRecords(storage, storage.length, metadata("f64", 8)).text, `${entry.text}\n`);
  }
});

function controlledParam({
  name = "cutoff",
  scalar = "f64",
  minimum = "20",
  maximum = "20000",
  scale = "log",
  curve = null,
  step = null,
  stepCount = null,
  arrayLen = 1,
} = {}) {
  return {
    name,
    type_repr: arrayLen === 1 ? scalar : `${scalar}[${arrayLen}]`,
    scalar,
    array_len: arrayLen,
    element_size_bytes: scalar === "f64" || scalar === "i64" ? 8 : 4,
    slot_offset: 0,
    byte_offset: 0,
    state_byte_offset: null,
    byte_size: (scalar === "f64" || scalar === "i64" ? 8 : 4) * arrayLen,
    default_reprs: null,
    range_min_repr: minimum,
    range_max_repr: maximum,
    param_control: {
      scale,
      curve,
      unit: null,
      step_repr: step,
      step_count: stepCount,
    },
  };
}

test("converts linear and logarithmic parameter domains in both directions", () => {
  const linear = controlledParam({ scale: "linear" });
  assert.equal(paramNormalizedToPlain(linear, 0.5), 10_010);
  assert.equal(paramNormalizedToPlain(linear, Number.NaN), 20);
  assert.equal(paramNormalizedToPlain(linear, Number.POSITIVE_INFINITY), 20_000);
  assert.equal(paramPlainToNormalized(linear, 10_010), 0.5);

  const logarithmic = controlledParam();
  const midpoint = Math.sqrt(20 * 20_000);
  assert.ok(Math.abs(paramNormalizedToPlain(logarithmic, 0.5) - midpoint) < 1e-12);
  assert.ok(Math.abs(paramPlainToNormalized(logarithmic, midpoint) - 0.5) < 1e-12);

  const normalized440 = paramPlainToNormalized(logarithmic, 440);
  assert.ok(Math.abs(paramNormalizedToPlain(logarithmic, normalized440) - 440) < 1e-12);
  assert.equal(paramNormalizedToPlain(logarithmic, 0), 20);
  assert.equal(paramNormalizedToPlain(logarithmic, 1), 20_000);

  const wideLinear = controlledParam({
    name: "wide-linear",
    minimum: "-1e308",
    maximum: "1e308",
    scale: "linear",
  });
  assert.equal(paramNormalizedToPlain(wideLinear, 0.5), 0);
  assert.equal(paramPlainToNormalized(wideLinear, 0), 0.5);

  const wideCurve = controlledParam({
    name: "wide-curve",
    minimum: "-1e308",
    maximum: "1e308",
    scale: "linear",
    curve: -4,
  });
  const wideCurveMidpoint = paramNormalizedToPlain(wideCurve, 0.5);
  assert.equal(Number.isFinite(wideCurveMidpoint), true);
  assert.ok(Math.abs(paramPlainToNormalized(wideCurve, wideCurveMidpoint) - 0.5) < 1e-12);
});

test("prepares a reusable decoded parameter control", () => {
  const param = controlledParam({
    minimum: "0",
    maximum: "1",
    scale: "linear",
    curve: -4,
  });
  const control = createParamControl(param);
  const midpoint = control.normalizedToPlain(0.5);

  assert.equal(control.minimum, 0);
  assert.equal(control.maximum, 1);
  assert.equal(control.curve, -4);
  assert.equal(control.step, null);
  assert.ok(Math.abs(control.plainToNormalized(midpoint) - 0.5) < 1e-12);
  assert.equal(control.constrainPlain(2), 1);
  assert.equal(Object.isFrozen(control), true);
});

test("prepares an already-decoded parameter domain", () => {
  const control = createParamDomain({
    name: "gain",
    scalar: "f64",
    minimum: 0,
    maximum: 1,
    scale: "linear",
    curve: null,
    unit: "dB",
    step: 0.25,
    stepCount: 4,
  });

  assert.equal(control.name, "gain");
  assert.equal(control.unit, "dB");
  assert.equal(control.normalizedToPlain(0.5), 0.5);
  assert.equal(control.plainToNormalized(0.5), 0.5);
  assert.equal(control.constrainPlain(0.7), 0.75);

  const f32Control = createParamDomain({
    name: "frequency",
    scalar: "f32",
    minimum: 0,
    maximum: 100_000,
    scale: "linear",
    step: 0.1,
    stepCount: 1_000_000,
  });
  assert.equal(f32Control.stepCount, 1_000_000);
  assert.equal(f32Control.step, Math.fround(0.1));
});

test("constrains stepped and boolean host-control values", () => {
  const stepped = controlledParam({
    name: "mode",
    scalar: "i32",
    minimum: "0",
    maximum: "10",
    scale: "linear",
    step: "2",
    stepCount: 5,
  });
  assert.equal(constrainParamPlain(stepped, 3.2), 4);
  assert.equal(paramNormalizedToPlain(stepped, 0.3), 4);
  assert.equal(paramPlainToNormalized(stepped, 3.2), 0.4);
  assert.equal(constrainParamPlain(stepped, 100), 10);

  const fine = controlledParam({
    name: "fine",
    scalar: "f64",
    minimum: "0",
    maximum: "0.000001",
    scale: "linear",
    step: "0.0000001",
    stepCount: 10,
  });
  assert.equal(constrainParamPlain(fine, 0.0000003), 0.0000003);

  const wideLog = controlledParam({
    name: "wide-log",
    scalar: "f64",
    minimum: "1e-300",
    maximum: "1e300",
    scale: "log",
  });
  assert.ok(Math.abs(paramNormalizedToPlain(wideLog, 0.5) - 1) < 1e-12);
  assert.ok(Math.abs(paramPlainToNormalized(wideLog, 1) - 0.5) < 1e-12);

  const inverseCurve = controlledParam({
    name: "inverse-curve",
    minimum: "0",
    maximum: "1",
    scale: "linear",
    curve: -4,
  });
  const curvedMidpoint = paramNormalizedToPlain(inverseCurve, 0.5);
  const expectedCurveMidpoint = Math.expm1(-2) / Math.expm1(-4);
  assert.ok(Math.abs(curvedMidpoint - expectedCurveMidpoint) < 1e-12);
  assert.ok(Math.abs(paramPlainToNormalized(inverseCurve, curvedMidpoint) - 0.5) < 1e-12);

  const forwardCurve = controlledParam({
    name: "forward-curve",
    minimum: "0",
    maximum: "1",
    scale: "linear",
    curve: 4,
  });
  assert.ok(
    Math.abs(paramNormalizedToPlain(forwardCurve, 0.5) + curvedMidpoint - 1) < 1e-12,
  );

  const boolean = controlledParam({
    name: "enabled",
    scalar: "bool",
    minimum: null,
    maximum: null,
    scale: null,
  });
  boolean.param_control = null;
  assert.equal(constrainParamPlain(boolean, -1), false);
  assert.equal(constrainParamPlain(boolean, 0.49), false);
  assert.equal(constrainParamPlain(boolean, 0.5), true);
  assert.equal(paramNormalizedToPlain(boolean, 0.49), false);
  assert.equal(paramNormalizedToPlain(boolean, 0.5), true);
  assert.equal(paramPlainToNormalized(boolean, 0.49), 0);
  assert.equal(paramPlainToNormalized(boolean, 0.5), 1);
  assert.equal(paramPlainToNormalized(boolean, false), 0);
  assert.equal(paramPlainToNormalized(boolean, true), 1);
});

test("rejects parameters without a scalar host-control domain", () => {
  const unranged = controlledParam();
  unranged.param_control = null;
  unranged.range_min_repr = null;
  unranged.range_max_repr = null;
  assert.throws(
    () => paramNormalizedToPlain(unranged, 0.5),
    /no numeric host-control domain/,
  );
  assert.throws(
    () => paramPlainToNormalized(controlledParam({ arrayLen: 2 }), 440),
    /scalar host-control domain/,
  );
  const booleanArray = controlledParam({
    name: "flags",
    scalar: "bool",
    arrayLen: 1,
  });
  booleanArray.type_repr = "bool[1]";
  booleanArray.param_control = null;
  assert.throws(
    () => paramNormalizedToPlain(booleanArray, 1),
    /scalar host-control domain/,
  );
});

test("rejects i64 control domains that are not exact through host numbers", () => {
  const unsafe = controlledParam({
    name: "wide",
    scalar: "i64",
    minimum: "9223372036854771711",
    maximum: "9223372036854775807",
    scale: "linear",
    step: "1024",
    stepCount: 4,
  });
  assert.throws(
    () => paramNormalizedToPlain(unsafe, 1),
    /outside the exact host-control integer range/,
  );
});

test("validates parameter-control semantics before accepting a descriptor", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/processor-descriptor-v9.json", import.meta.url),
    "utf8",
  ));

  const integerLog = structuredClone(fixture);
  Object.assign(integerLog.metadata.params[0], {
    type_repr: "i32",
    scalar: "i32",
    range_min_repr: "-20",
    range_max_repr: "20000",
    default_reprs: ["440"],
  });
  assert.throws(
    () => validateProcessorMetadata(integerLog),
    /logarithmic scale with a non-floating scalar/,
  );

  const wrongStepCount = structuredClone(fixture);
  Object.assign(wrongStepCount.metadata.params[0].param_control, {
    scale: "linear",
    step_repr: "10",
    step_count: 1997,
  });
  assert.throws(
    () => validateProcessorMetadata(wrongStepCount),
    /step_count inconsistent/,
  );

  const offGridDefault = structuredClone(fixture);
  offGridDefault.metadata.params[0].default_reprs = ["445"];
  Object.assign(offGridDefault.metadata.params[0].param_control, {
    scale: "linear",
    step_repr: "10",
    step_count: 1998,
  });
  assert.throws(
    () => validateProcessorMetadata(offGridDefault),
    /default outside its host-control step grid/,
  );

  const largeOffGridDefault = structuredClone(fixture);
  Object.assign(largeOffGridDefault.metadata.params[0], {
    type_repr: "f32",
    scalar: "f32",
    element_size_bytes: 4,
    byte_size: 4,
    range_min_repr: "0",
    range_max_repr: "100000",
    default_reprs: ["50000.5"],
  });
  Object.assign(largeOffGridDefault.metadata.params[0].param_control, {
    scale: "linear",
    step_repr: "1",
    step_count: 100000,
  });
  assert.throws(
    () => validateProcessorMetadata(largeOffGridDefault),
    /default outside its host-control step grid/,
  );

  const nonDividingLargeRange = structuredClone(largeOffGridDefault);
  Object.assign(nonDividingLargeRange.metadata.params[0], {
    range_max_repr: "100000.5",
    default_reprs: ["0"],
  });
  Object.assign(nonDividingLargeRange.metadata.params[0].param_control, {
    step_count: 100001,
  });
  assert.throws(
    () => validateProcessorMetadata(nonDividingLargeRange),
    /step_count inconsistent/,
  );

  const mixedLogCurve = structuredClone(fixture);
  mixedLogCurve.metadata.params[0].param_control.curve = -4;
  assert.throws(
    () => validateProcessorMetadata(mixedLogCurve),
    /cannot combine logarithmic scale with curve/,
  );

  const controlOnInput = structuredClone(fixture);
  controlOnInput.metadata.inputs[0].range_min_repr = "0";
  controlOnInput.metadata.inputs[0].range_max_repr = "1";
  controlOnInput.metadata.inputs[0].param_control = {
    scale: "linear",
    curve: null,
    unit: null,
    step_repr: null,
    step_count: null,
  };
  assert.throws(
    () => validateProcessorMetadata(controlOnInput),
    /only valid for parameters/,
  );
});

test("rejects an unsupported snapshot format", () => {
  const fixture = metadata();
  fixture.runtime.snapshot_format_version = PROCESSOR_SNAPSHOT_FORMAT_VERSION + 1;
  assert.throws(
    () => validateProcessorMetadata(fixture),
    /unsupported processor snapshot version/,
  );
});

test("rejects runtime semantics not implemented by the current processor ABI", () => {
  for (const [field, value, expected] of [
    ["state_initialization", "host_initialized", "zeroed"],
    ["snapshot_byte_order", "big_endian", "little_endian"],
    ["snapshot_restore_base", "zeroed_state", "post_init_physical_state_image"],
  ]) {
    const fixture = metadata();
    fixture.runtime[field] = value;
    assert.throws(
      () => validateProcessorMetadata(fixture),
      new RegExp(`runtime\\.${field} must be '${expected}'`),
    );
  }

  const bigEndianWasm = metadata();
  bigEndianWasm.target.byte_order = "big_endian";
  assert.throws(
    () => validateProcessorMetadata(bigEndianWasm),
    /target\.byte_order must be 'little_endian'/,
  );
});

test("rejects metadata layouts outside or overlapping their runtime regions", () => {
  const fixture = JSON.parse(readFileSync(
    new URL("./fixtures/processor-descriptor-v9.json", import.meta.url),
    "utf8",
  ));

  const stateOutOfBounds = structuredClone(fixture);
  stateOutOfBounds.metadata.states[0].physical_state_byte_offset = 16;
  assert.throws(
    () => validateProcessorMetadata(stateOutOfBounds),
    /exceeds runtime physical-state storage size 16/,
  );

  const snapshotGap = structuredClone(fixture);
  snapshotGap.metadata.states[0].packed_snapshot_byte_offset = 1;
  assert.throws(
    () => validateProcessorMetadata(snapshotGap),
    /packed_snapshot_byte_offset must be 0/,
  );

  const overlappingControlOutput = structuredClone(fixture);
  overlappingControlOutput.metadata.control_outputs = [{
    ...structuredClone(fixture.metadata.inputs[0]),
    name: "meter",
    byte_offset: null,
    state_byte_offset: 0,
  }];
  assert.throws(
    () => validateProcessorMetadata(overlappingControlOutput),
    /overlaps metadata\.states\[0\] in runtime physical-state storage/,
  );

  const paramOutOfBounds = structuredClone(fixture);
  paramOutOfBounds.metadata.params = [{
    ...structuredClone(fixture.metadata.inputs[0]),
    name: "gain",
    byte_offset: 16,
    default_reprs: ["0x00000000"],
  }];
  assert.throws(
    () => validateProcessorMetadata(paramOutOfBounds),
    /exceeds runtime parameter storage size 16/,
  );

  const slotGap = structuredClone(fixture);
  slotGap.metadata.inputs[0].slot_offset = 1;
  assert.throws(
    () => validateProcessorMetadata(slotGap),
    /metadata\.inputs\[0\]\.slot_offset must be 0/,
  );
});

const FIXTURE_MIR_SCHEMA_VERSION = 8;

const wasm = new Uint8Array([
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
]);

function metadata() {
  return {
    format: "onda-processor",
    format_version: PROCESSOR_ARTIFACT_FORMAT_VERSION,
    abi_version: PROCESSOR_ABI_VERSION,
    artifact_kind: "webassembly_module",
    backend: "test",
    mir_schema_version: FIXTURE_MIR_SCHEMA_VERSION,
    target: {
      triple: "wasm32-unknown-unknown",
      cpu: "generic",
      features: "",
      reloc_model: "static",
      code_model: "default",
      opt_level: "4",
      abi_name: null,
      data_layout: "e-m:e-p:32:32-i64:64-n32:64-S128",
      pointer_width_bits: 32,
      byte_order: "little_endian",
      pointer_model: "linear_memory_offset",
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
      delegate_record_header_size_bytes: 8,
      print_record_header_size_bytes: 8,
    },
    exports: {
      memory: "memory",
      heap_base: "__heap_base",
      init: "onda_processor_init",
      process: "onda_process",
      events: [],
    },
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
    metadata: {
      states: [],
      inputs: [],
      outputs: [],
      control_outputs: [],
      params: [],
      buffers: [],
      events: [],
      delegates: [],
      source_files: [],
      log_sites: [],
    },
  };
}

test("validates descriptor and WebAssembly export kinds together", () => {
  const artifact = { wasm, metadata: metadata() };
  assert.equal(validateProcessorArtifact(artifact).wasm.byteLength, wasm.byteLength);
  const module = new WebAssembly.Module(wasm);
  assert.equal(validateProcessorModule(module, artifact.metadata).module, module);
});

test("rejects descriptor exports omitted from required_symbols", () => {
  const value = metadata();
  value.integration.required_symbols.pop();
  assert.throws(
    () => validateProcessorArtifact({ wasm, metadata: value }),
    /missing executable export 'onda_process'/,
  );
});

test("round-trips integrity-associated artifact files", async () => {
  const files = await createProcessorArtifactFiles({ wasm, metadata: metadata() }, {
    baseName: "test-processor",
  });
  const loaded = await loadProcessorArtifactFiles(files.wasm.bytes, files.metadata.text);
  assert.deepEqual(loaded.wasm, wasm);
});

test("prepares and decodes call-scoped delegate batches", () => {
  const memory = new ArrayBuffer(80);
  writeDelegateBatch(memory, 0, 20, 24);
  assert.deepEqual(readDelegateBatch(memory, 0), {
    storageAddress: 20,
    capacityBytes: 24,
    usedBytes: 0,
    recordCount: 0,
    overflowCount: 0,
  });

  const view = new DataView(memory);
  view.setUint32(20, 0, true);
  view.setUint32(24, 16, true);
  view.setInt32(28, 7, true);
  view.setInt32(32, 2, true);
  view.setFloat32(36, 1.25, true);
  view.setFloat32(40, -2.5, true);
  view.setUint32(8, 24, true);
  view.setUint32(12, 1, true);
  const delegates = [{
    name: "report",
    params: [
      { name: "code", scalar: "i32", array_len: 1, is_slice: false, element_size_bytes: 4 },
      { name: "values", scalar: "f32", array_len: 0, is_slice: true, element_size_bytes: 4 },
    ],
  }];
  const batch = readDelegateBatch(memory, 0);
  const records = decodeDelegateRecords(
    new Uint8Array(memory, batch.storageAddress, batch.capacityBytes),
    batch.usedBytes,
    delegates,
  );
  assert.deepEqual(records.map(({ name, values }) => ({ name, values })), [{
    name: "report",
    values: { code: 7, values: [1.25, -2.5] },
  }]);
});

test("rejects execution-output addresses outside wasm32", () => {
  const memory = new ArrayBuffer(16);
  assert.throws(
    () => writeExecutionOutput(memory, 0, 0x1_0000_0000, 0),
    /must fit u32/,
  );
  assert.throws(
    () => writeExecutionOutput(memory, 0, 0, 0x1_0000_0000),
    /must fit u32/,
  );
});
