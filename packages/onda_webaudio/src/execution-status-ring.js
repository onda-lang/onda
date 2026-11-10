// Independent of the lossy print/delegate ring. Overflow is terminal and wakes
// the consumer so no control promise can remain pending after a lost reply.
import { ONDA_RUNTIME_ERROR } from "./runtime-status.js";

const MAGIC = 0x4f535431;
const READ = 1;
const WRITE = 2;
const FAILED = 3;
export const STATUS_WAKE = 4;
const HEADER_WORDS = 5;
const SLOT_WORDS = 3;
const CAPACITY = 64;
// Each command can recover the processor and cause one more render failure.
// Reserve two slots per command, plus one for an already-running processor.
export const MAX_IN_FLIGHT_REQUESTS = Math.floor((CAPACITY - 1) / 2);
const WORDS = HEADER_WORDS + CAPACITY * SLOT_WORDS;

export function createExecutionStatusRing() {
  if (typeof SharedArrayBuffer !== "function") return null;
  const buffer = new SharedArrayBuffer(WORDS * 4);
  new Int32Array(buffer)[0] = MAGIC;
  return buffer;
}

export function openExecutionStatusRing(buffer) {
  if (typeof SharedArrayBuffer !== "function" || !(buffer instanceof SharedArrayBuffer) || buffer.byteLength !== WORDS * 4) {
    throw new Error("invalid execution-status ring storage");
  }
  const words = new Int32Array(buffer);
  if (words[0] !== MAGIC) throw new Error("invalid execution-status ring header");
  return words;
}

export function wakeExecutionStatusRing(words) {
  Atomics.add(words, STATUS_WAKE, 1);
  Atomics.notify(words, STATUS_WAKE);
}

export function writeExecutionStatus(words, requestId, operation, status) {
  if (Atomics.load(words, FAILED)) return false;
  const read = Atomics.load(words, READ) >>> 0;
  const write = Atomics.load(words, WRITE) >>> 0;
  if (((write - read) >>> 0) >= CAPACITY) {
    Atomics.store(words, FAILED, ONDA_RUNTIME_ERROR.TRANSPORT_OVERFLOW);
    wakeExecutionStatusRing(words);
    return false;
  }
  const offset = HEADER_WORDS + (write % CAPACITY) * SLOT_WORDS;
  words[offset] = requestId;
  words[offset + 1] = operation;
  words[offset + 2] = status;
  Atomics.store(words, WRITE, (write + 1) | 0);
  wakeExecutionStatusRing(words);
  return true;
}

// Releases each slot before invoking user-side code. Never call this reentrantly.
export function drainExecutionStatusRing(words, consume) {
  let count = 0;
  for (;;) {
    const read = Atomics.load(words, READ) >>> 0;
    if (read === (Atomics.load(words, WRITE) >>> 0)) break;
    const offset = HEADER_WORDS + (read % CAPACITY) * SLOT_WORDS;
    const requestId = words[offset] >>> 0;
    const operation = words[offset + 1];
    const status = words[offset + 2];
    Atomics.store(words, READ, (read + 1) | 0);
    consume(requestId, operation, status);
    count += 1;
  }
  const failure = Atomics.load(words, FAILED);
  if (failure) consume(0, 0, failure);
  return count;
}
