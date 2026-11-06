import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PayloadPlan, writeEventInput } from "@onda-lang/processor-abi";
import { compileTrustedMir } from "../src/index.js";
import { resolveOndaCli } from "./onda-cli.mjs";
import { writeParameterDefaults } from "./wasm-memory.mjs";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = resolve(packageDir, "../..");
const cli = resolveOndaCli(repoDir);
const temporary = mkdtempSync(join(tmpdir(), "onda-enums-"));
try {
  const mirPath = join(temporary, "enums.msgpack");
  execFileSync(cli, ["compile", join(packageDir, "test/fixtures/enums.onda"),
    "--emit", "mir-messagepack", "--output", mirPath, "--block-size", "4"], { cwd: repoDir });
  for (const optimize of [false, true]) {
    const artifact = compileTrustedMir(readFileSync(mirPath), { optimize });
    const descriptor = artifact.metadata;
    const { params, events } = descriptor.metadata;
    assert.equal(params[0].scalar, "i32");
    assert.equal(params[0].enumeration.members[2].label, "Band-pass");
    assert.equal(params[1].param_control.step_count, 0);
    assert.equal(events[0].schema.params[0].ty.fields[0].ty.fields[0].ty.enumeration.name, "Mode");
    const { instance } = await WebAssembly.instantiate(artifact.wasm, {});
    const exports = instance.exports;
    const memory = exports.memory;
    let heap = Number(exports.__heap_base.value);
    const allocate = (bytes, alignment = 16) => {
      heap = Math.ceil(heap / alignment) * alignment;
      const pointer = heap;
      heap += bytes;
      return pointer;
    };
    const paramPtr = allocate(descriptor.runtime.param_size_bytes);
    const statePtr = allocate(descriptor.runtime.state_size_bytes);
    const table = allocate(9 * 4);
    const outputs = Array.from({ length: 9 }, () => allocate(4 * 4));
    const view = new DataView(memory.buffer);
    outputs.forEach((pointer, index) => view.setUint32(table + index * 4, pointer, true));
    writeParameterDefaults(memory, paramPtr, params);
    assert.equal(exports.onda_processor_init(paramPtr, statePtr, 1, 0, 0, 0, 0, 0), 0);
    for (const [index, values] of [[0, [2147483647, -2147483648, 99]], [1, [99]]]) {
      const plan = new PayloadPlan(events[index].schema);
      const payload = allocate(values.length * 4, 4);
      values.forEach((value, offset) => view.setInt32(payload + offset * 4, value, true));
      const workspace = allocate(plan.sizes([]).workspace, 8);
      const input = allocate(16, 4);
      writeEventInput(memory, input, payload, values.length * 4, workspace, plan.sizes([]).workspace);
      assert.equal(exports[`onda_event_${index}`](input, paramPtr, statePtr, 0, 0, 0, 0, 0), 0);
    }
    for (const [ordinal, expected] of [[-(1n << 63n), 0], [(1n << 63n) - 1n, 2]]) {
      params.slice(0, 2).forEach((param) => view.setInt32(paramPtr + param.byte_offset, 99, true));
      view.setBigInt64(paramPtr + params[2].byte_offset, ordinal, true);
      view.setInt32(paramPtr + params[3].byte_offset, 99, true);
      view.setInt32(paramPtr + params[3].byte_offset + 4, -99, true);
      assert.equal(exports.onda_process(statePtr, paramPtr, 0, table, 0, 4, 3, 0, 0, 0, 0, 0), 0);
      assert.deepEqual(outputs.map((pointer) => [...new Float32Array(memory.buffer, pointer, 4)]),
        [2, 2, 0, 2, 2, 0, expected, 2, 0].map((value) => Array(4).fill(value)));
    }
  }
  process.stdout.write("Enum metadata and raw boundary normalization pass in optimized and unoptimized Wasm.\n");
} finally {
  rmSync(temporary, { recursive: true, force: true });
}
