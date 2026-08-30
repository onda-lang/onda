import assert from "node:assert/strict";
import test from "node:test";
import {
  EXECUTION_OPERATION_EVENT,
  createExecutionOutputRing,
  drainExecutionOutputRing,
  openExecutionOutputRing,
  writeExecutionOutputRing,
} from "../src/execution-output-ring.js";

function entry(overrides = {}) {
  return {
    operation: EXECUTION_OPERATION_EVENT,
    operationIndex: 3,
    delegateSubscriptionId: 5,
    delegateUsed: 0,
    delegateRecordCount: 0,
    delegateOverflowCount: 0,
    delegateTransportDropCount: 0,
    printSubscriptionId: 7,
    printUsed: 0,
    printRecordCount: 0,
    printOverflowCount: 0,
    printTransportDropCount: 0,
    ...overrides,
  };
}

test("shared execution-output ring preserves metadata and payloads across wraparound", () => {
  // A 60-byte slot and a 128-byte budget produce exactly two slots.
  const ring = openExecutionOutputRing(createExecutionOutputRing(5, 7, 128));
  assert.equal(ring.slotCount, 2);

  const first = entry({
    delegateUsed: 3,
    delegateRecordCount: 1,
    delegateOverflowCount: 2,
    delegateTransportDropCount: 4,
    printUsed: 4,
    printRecordCount: 1,
    printOverflowCount: 6,
    printTransportDropCount: 8,
  });
  assert.equal(
    writeExecutionOutputRing(
      ring,
      first,
      new Uint8Array([1, 2, 3, 99]),
      new Uint8Array([4, 5, 6, 7, 99]),
    ),
    true,
  );
  assert.equal(
    writeExecutionOutputRing(
      ring,
      entry({ operationIndex: 10 }),
      new Uint8Array(),
      new Uint8Array(),
    ),
    true,
  );
  assert.equal(
    writeExecutionOutputRing(
      ring,
      entry({ operationIndex: 11 }),
      new Uint8Array(),
      new Uint8Array(),
    ),
    false,
  );

  const drained = [];
  assert.equal(drainExecutionOutputRing(ring, (output) => {
    drained.push({
      ...output,
      delegateStorage: [...output.delegateStorage],
      printStorage: [...output.printStorage],
    });
  }), 2);
  assert.deepEqual(drained[0], {
    ...first,
    delegateStorage: [1, 2, 3],
    printStorage: [4, 5, 6, 7],
  });
  assert.equal(drained[1].operationIndex, 10);

  assert.equal(
    writeExecutionOutputRing(
      ring,
      entry({ operationIndex: 12, delegateUsed: 2 }),
      new Uint8Array([8, 9]),
      new Uint8Array(),
    ),
    true,
  );
  let wrapped;
  assert.equal(drainExecutionOutputRing(ring, (output) => {
    wrapped = {
      operationIndex: output.operationIndex,
      delegateStorage: [...output.delegateStorage],
    };
  }), 1);
  assert.deepEqual(wrapped, { operationIndex: 12, delegateStorage: [8, 9] });
});

test("shared execution-output ring validates capacities and entry bounds", () => {
  const empty = openExecutionOutputRing(createExecutionOutputRing(0, 0, 64));
  assert.equal(empty.delegateCapacity, 0);
  assert.equal(empty.printCapacity, 0);
  assert.throws(
    () => createExecutionOutputRing(-1, 0),
    /delegate capacity must be an integer/,
  );
  const ring = openExecutionOutputRing(createExecutionOutputRing(1, 1, 64));
  assert.throws(
    () => writeExecutionOutputRing(
      ring,
      entry({ delegateUsed: 2 }),
      new Uint8Array(2),
      new Uint8Array(),
    ),
    /exceeds its shared ring capacity/,
  );
  assert.throws(
    () => writeExecutionOutputRing(
      ring,
      entry({ printUsed: 1 }),
      new Uint8Array(),
      new Uint8Array(),
    ),
    /print source is shorter/,
  );
});

test("shared execution-output ring reports unavailable shared memory", () => {
  const SharedBuffer = globalThis.SharedArrayBuffer;
  try {
    globalThis.SharedArrayBuffer = undefined;
    assert.equal(createExecutionOutputRing(1, 1), null);
  } finally {
    globalThis.SharedArrayBuffer = SharedBuffer;
  }
});
