import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const [adapterPath, workletPath] = process.argv.slice(2);
assert.ok(adapterPath && workletPath, "expected staged adapter and worklet paths");

// Import the staged modules so missing transitive dependencies fail the build.
const adapter = await import(pathToFileURL(adapterPath));
assert.equal(typeof adapter.createOndaAudioProcessor, "function");
globalThis.AudioWorkletProcessor = class {};
let registered = false;
globalThis.registerProcessor = (name, processor) => {
  assert.equal(typeof name, "string");
  assert.equal(typeof processor, "function");
  registered = true;
};
await import(pathToFileURL(workletPath));
assert.ok(registered, "staged worklet did not register its processor");
console.log("Staged Web Audio adapter and worklet imports passed");
