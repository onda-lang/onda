const RING_MAGIC = 0x4f4e4441;
const RING_VERSION = 1;

const CONTROL_MAGIC = 0;
const CONTROL_VERSION = 1;
const CONTROL_SLOT_COUNT = 2;
const CONTROL_DELEGATE_CAPACITY = 3;
const CONTROL_PRINT_CAPACITY = 4;
export const EXECUTION_OUTPUT_RING_READ_INDEX = 5;
export const EXECUTION_OUTPUT_RING_WRITE_INDEX = 6;
export const EXECUTION_OUTPUT_RING_WAKE_INDEX = 7;
const CONTROL_WORDS = 8;

const SLOT_OPERATION = 0;
const SLOT_OPERATION_INDEX = 1;
const SLOT_DELEGATE_SUBSCRIPTION = 2;
const SLOT_DELEGATE_USED = 3;
const SLOT_DELEGATE_RECORDS = 4;
const SLOT_DELEGATE_OVERFLOW = 5;
const SLOT_DELEGATE_TRANSPORT_DROPS = 6;
const SLOT_PRINT_SUBSCRIPTION = 7;
const SLOT_PRINT_USED = 8;
const SLOT_PRINT_RECORDS = 9;
const SLOT_PRINT_OVERFLOW = 10;
const SLOT_PRINT_TRANSPORT_DROPS = 11;
const SLOT_WORDS = 12;

const CONTROL_BYTES = CONTROL_WORDS * 4;
const SLOT_HEADER_BYTES = SLOT_WORDS * 4;
const DEFAULT_RING_BUDGET_BYTES = 4 * 1024 * 1024;
const MAX_RING_SLOTS = 32;

export const EXECUTION_OPERATION_INIT = 1;
export const EXECUTION_OPERATION_PROCESS = 2;
export const EXECUTION_OPERATION_EVENT = 3;
export const EXECUTION_OPERATION_TRANSPORT = 4;

function align4(value) {
  return Math.ceil(value / 4) * 4;
}

function checkedCapacity(value, name) {
  if (!Number.isSafeInteger(value) || value < 0 || value > 0x7fff_ffff) {
    throw new Error(`${name} must be an integer from 0 through 2147483647 bytes`);
  }
  return value;
}

export function createExecutionOutputRing(
  delegateCapacity,
  printCapacity,
  budgetBytes = DEFAULT_RING_BUDGET_BYTES,
) {
  const delegateBytes = checkedCapacity(delegateCapacity, "delegate capacity");
  const printBytes = checkedCapacity(printCapacity, "print capacity");
  if (typeof SharedArrayBuffer !== "function") return null;
  if (!Number.isSafeInteger(budgetBytes) || budgetBytes <= 0) {
    throw new Error("execution-output ring budget must be a positive integer");
  }

  const payloadBytes = delegateBytes + printBytes;
  if (!Number.isSafeInteger(payloadBytes)) {
    throw new Error("combined execution-output capacity exceeds the JavaScript size limit");
  }
  const slotBytes = align4(SLOT_HEADER_BYTES + payloadBytes);
  const slotCount = Math.max(
    1,
    Math.min(MAX_RING_SLOTS, Math.floor(budgetBytes / slotBytes)),
  );
  const totalBytes = CONTROL_BYTES + slotCount * slotBytes;
  let buffer;
  try {
    buffer = new SharedArrayBuffer(totalBytes);
  } catch (error) {
    throw new Error(
      `cannot allocate ${totalBytes} bytes for the shared execution-output ring`,
      { cause: error },
    );
  }
  const control = new Int32Array(buffer, 0, CONTROL_WORDS);
  control[CONTROL_MAGIC] = RING_MAGIC;
  control[CONTROL_VERSION] = RING_VERSION;
  control[CONTROL_SLOT_COUNT] = slotCount;
  control[CONTROL_DELEGATE_CAPACITY] = delegateBytes;
  control[CONTROL_PRINT_CAPACITY] = printBytes;
  return buffer;
}

export function openExecutionOutputRing(buffer) {
  if (
    typeof SharedArrayBuffer !== "function"
    || !(buffer instanceof SharedArrayBuffer)
  ) {
    throw new Error("execution-output ring must be a SharedArrayBuffer");
  }
  if (buffer.byteLength < CONTROL_BYTES) {
    throw new Error("execution-output ring is shorter than its control header");
  }
  const control = new Int32Array(buffer, 0, CONTROL_WORDS);
  const slotCount = control[CONTROL_SLOT_COUNT] >>> 0;
  const delegateCapacity = control[CONTROL_DELEGATE_CAPACITY] >>> 0;
  const printCapacity = control[CONTROL_PRINT_CAPACITY] >>> 0;
  const slotBytes = align4(SLOT_HEADER_BYTES + delegateCapacity + printCapacity);
  if (
    (control[CONTROL_MAGIC] >>> 0) !== RING_MAGIC
    || (control[CONTROL_VERSION] >>> 0) !== RING_VERSION
    || slotCount === 0
    || CONTROL_BYTES + slotCount * slotBytes !== buffer.byteLength
  ) {
    throw new Error("execution-output ring has an invalid layout");
  }
  return {
    buffer,
    control,
    words: new Uint32Array(buffer),
    bytes: new Uint8Array(buffer),
    slotCount,
    slotBytes,
    delegateCapacity,
    printCapacity,
  };
}

function slotOffsets(ring, slot) {
  const byteOffset = CONTROL_BYTES + slot * ring.slotBytes;
  return {
    wordOffset: byteOffset / 4,
    delegateOffset: byteOffset + SLOT_HEADER_BYTES,
    printOffset: byteOffset + SLOT_HEADER_BYTES + ring.delegateCapacity,
  };
}

function copyBytes(destination, destinationOffset, source, length) {
  for (let index = 0; index < length; index += 1) {
    destination[destinationOffset + index] = source[index];
  }
}

/**
 * Single-producer write. `entry` and source views may be reused by the caller; this function
 * allocates nothing on either the success or saturation path.
 */
export function writeExecutionOutputRing(ring, entry, delegateSource, printSource) {
  const read = Atomics.load(ring.control, EXECUTION_OUTPUT_RING_READ_INDEX) >>> 0;
  const write = Atomics.load(ring.control, EXECUTION_OUTPUT_RING_WRITE_INDEX) >>> 0;
  if (((write - read) >>> 0) >= ring.slotCount) return false;
  if (
    entry.delegateUsed > ring.delegateCapacity
    || entry.printUsed > ring.printCapacity
  ) {
    throw new Error("execution-output entry exceeds its shared ring capacity");
  }
  if (
    entry.delegateUsed
    && (!delegateSource || delegateSource.length < entry.delegateUsed)
  ) {
    throw new Error("delegate source is shorter than its execution-output entry");
  }
  if (entry.printUsed && (!printSource || printSource.length < entry.printUsed)) {
    throw new Error("print source is shorter than its execution-output entry");
  }

  const slot = write % ring.slotCount;
  const byteOffset = CONTROL_BYTES + slot * ring.slotBytes;
  const wordOffset = byteOffset / 4;
  const delegateOffset = byteOffset + SLOT_HEADER_BYTES;
  const printOffset = delegateOffset + ring.delegateCapacity;
  const words = ring.words;
  words[wordOffset + SLOT_OPERATION] = entry.operation;
  words[wordOffset + SLOT_OPERATION_INDEX] = entry.operationIndex;
  words[wordOffset + SLOT_DELEGATE_SUBSCRIPTION] = entry.delegateSubscriptionId;
  words[wordOffset + SLOT_DELEGATE_USED] = entry.delegateUsed;
  words[wordOffset + SLOT_DELEGATE_RECORDS] = entry.delegateRecordCount;
  words[wordOffset + SLOT_DELEGATE_OVERFLOW] = entry.delegateOverflowCount;
  words[wordOffset + SLOT_DELEGATE_TRANSPORT_DROPS] = entry.delegateTransportDropCount;
  words[wordOffset + SLOT_PRINT_SUBSCRIPTION] = entry.printSubscriptionId;
  words[wordOffset + SLOT_PRINT_USED] = entry.printUsed;
  words[wordOffset + SLOT_PRINT_RECORDS] = entry.printRecordCount;
  words[wordOffset + SLOT_PRINT_OVERFLOW] = entry.printOverflowCount;
  words[wordOffset + SLOT_PRINT_TRANSPORT_DROPS] = entry.printTransportDropCount;
  copyBytes(ring.bytes, delegateOffset, delegateSource, entry.delegateUsed);
  copyBytes(ring.bytes, printOffset, printSource, entry.printUsed);

  Atomics.store(
    ring.control,
    EXECUTION_OUTPUT_RING_WRITE_INDEX,
    (write + 1) | 0,
  );
  Atomics.add(ring.control, EXECUTION_OUTPUT_RING_WAKE_INDEX, 1);
  Atomics.notify(ring.control, EXECUTION_OUTPUT_RING_WAKE_INDEX);
  return true;
}

/** Single-consumer drain. Allocations occur only on the main-thread consumer side. */
export function drainExecutionOutputRing(ring, consume) {
  let drained = 0;
  for (;;) {
    const read = Atomics.load(ring.control, EXECUTION_OUTPUT_RING_READ_INDEX) >>> 0;
    const write = Atomics.load(ring.control, EXECUTION_OUTPUT_RING_WRITE_INDEX) >>> 0;
    if (read === write) return drained;
    const slot = read % ring.slotCount;
    const { wordOffset, delegateOffset, printOffset } = slotOffsets(ring, slot);
    const words = ring.words;
    const delegateUsed = words[wordOffset + SLOT_DELEGATE_USED];
    const printUsed = words[wordOffset + SLOT_PRINT_USED];
    const entry = {
      operation: words[wordOffset + SLOT_OPERATION],
      operationIndex: words[wordOffset + SLOT_OPERATION_INDEX],
      delegateSubscriptionId: words[wordOffset + SLOT_DELEGATE_SUBSCRIPTION],
      delegateUsed,
      delegateRecordCount: words[wordOffset + SLOT_DELEGATE_RECORDS],
      delegateOverflowCount: words[wordOffset + SLOT_DELEGATE_OVERFLOW],
      delegateTransportDropCount:
        words[wordOffset + SLOT_DELEGATE_TRANSPORT_DROPS],
      printSubscriptionId: words[wordOffset + SLOT_PRINT_SUBSCRIPTION],
      printUsed,
      printRecordCount: words[wordOffset + SLOT_PRINT_RECORDS],
      printOverflowCount: words[wordOffset + SLOT_PRINT_OVERFLOW],
      printTransportDropCount: words[wordOffset + SLOT_PRINT_TRANSPORT_DROPS],
      delegateStorage: new Uint8Array(ring.buffer, delegateOffset, delegateUsed),
      printStorage: new Uint8Array(ring.buffer, printOffset, printUsed),
    };
    try {
      consume(entry);
    } finally {
      Atomics.store(
        ring.control,
        EXECUTION_OUTPUT_RING_READ_INDEX,
        (read + 1) | 0,
      );
      drained += 1;
    }
  }
}
