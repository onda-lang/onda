import assert from "node:assert/strict";
import test from "node:test";
import {
  createExecutionStatusRing, openExecutionStatusRing,
  writeExecutionStatus, drainExecutionStatusRing,
} from "../src/execution-status-ring.js";
import { ONDA_RUNTIME_ERROR as ERROR } from "../src/runtime-status.js";

test("execution statuses retain signed failures and u32 request IDs across counter wraparound", () => {
  const ring = openExecutionStatusRing(createExecutionStatusRing());
  // Header read/write counters are adjacent; start immediately before u32 rollover.
  ring[1] = -2;
  ring[2] = -2;
  for (let round = 0; round < 100; round += 1) {
    for (let index = 0; index < 64; index += 1) {
      assert.equal(writeExecutionStatus(ring, 0xffff_ffff - index, 3, -index), true);
    }
    const entries = [];
    assert.equal(drainExecutionStatusRing(ring, (...entry) => entries.push(entry)), 64);
    assert.deepEqual(entries, Array.from({ length: 64 }, (_, index) => [0xffff_ffff - index, 3, -index || 0]));
  }
});

test("status saturation retains unread results and permanently signals transport failure", () => {
  const ring = openExecutionStatusRing(createExecutionStatusRing());
  for (let id = 1; id <= 64; id += 1) assert.equal(writeExecutionStatus(ring, id, 2, 0), true);
  assert.equal(writeExecutionStatus(ring, 65, 2, 1), false);
  const entries = [];
  drainExecutionStatusRing(ring, (...entry) => entries.push(entry));
  assert.equal(entries.length, 65);
  assert.deepEqual(entries[0], [1, 2, 0]);
  assert.deepEqual(entries[63], [64, 2, 0]);
  assert.deepEqual(entries[64], [0, 0, ERROR.TRANSPORT_OVERFLOW]);
  assert.equal(writeExecutionStatus(ring, 66, 2, 0), false);
});

test("status transport rejects invalid storage and supports unavailable shared memory", () => {
  assert.throws(() => openExecutionStatusRing(new ArrayBuffer(788)), /invalid/);
  assert.throws(() => openExecutionStatusRing(new SharedArrayBuffer(788)), /header/);
  const original = globalThis.SharedArrayBuffer;
  try {
    globalThis.SharedArrayBuffer = undefined;
    assert.equal(createExecutionStatusRing(), null);
    assert.throws(() => openExecutionStatusRing(null), /invalid/);
  } finally {
    globalThis.SharedArrayBuffer = original;
  }
});
