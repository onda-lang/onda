# Onda processor ABI helpers

`@onda-lang/processor-abi` is the small, compiler-free JavaScript contract for Onda processor
artifacts. It validates current-format descriptors and complete core-WebAssembly modules, creates and
loads integrity-associated `.wasm`/`.onda.json` pairs, and provides the shared TypeScript surface
used by `@onda-lang/wasm-compiler`, `@onda-lang/binaryen-web`, and `@onda-lang/webaudio`.

```js
import {
  createParamControl,
  loadProcessorArtifactFiles,
  validateProcessorArtifact,
} from "@onda-lang/processor-abi";

const cutoff = artifact.metadata.metadata.params.find(
  (param) => param.name === "cutoff",
);
const cutoffControl = createParamControl(cutoff);
const plain = cutoffControl.normalizedToPlain(0.5);
const normalized440 = cutoffControl.plainToNormalized(440);
```

This package does not compile Onda source or host Web Audio. It exists so loaders and hosts can
validate artifacts without installing the compiler or duplicating the ABI schema.
The detailed TypeScript records mirror `onda_processor_abi::ProcessorDescriptor`; both packages
validate the same checked-in conformance fixture.

## Delegate batches

The package exports `DELEGATE_RECORD_HEADER_SIZE_BYTES`, `DELEGATE_BATCH_SIZE_BYTES`,
`writeDelegateBatch()`, `readDelegateBatch()`, and `decodeDelegateRecords()` for allocation-free
call-scoped delegate collection. Descriptor entries expose exact fixed payload sizes or dynamic
minimum sizes. A complete fixed record occupies the eight-byte header plus its payload; no exact
whole-call capacity exists because occurrence counts and slice lengths may be runtime-dependent.

```js
import {
  DELEGATE_RECORD_HEADER_SIZE_BYTES,
  writeDelegateBatch,
  readDelegateBatch,
  decodeDelegateRecords,
} from "@onda-lang/processor-abi";

const delegates = artifact.metadata.metadata.delegates;
const fixedRecordBytes = delegates.map((delegate) =>
  delegate.payload_size_bytes == null
    ? null
    : DELEGATE_RECORD_HEADER_SIZE_BYTES + delegate.payload_size_bytes
);

writeDelegateBatch(memory, batchAddress, storageAddress, capacityBytes);
// Pass batchAddress as the final onda_process or onda_event_N argument.
const batch = readDelegateBatch(memory, batchAddress);
const storage = new Uint8Array(memory.buffer, storageAddress, batch.usedBytes);
const occurrences = decodeDelegateRecords(
  storage,
  batch.usedBytes,
  delegates,
  artifact.metadata.target.byte_order,
);
if (batch.overflowCount) reportOverflow(batch.overflowCount);
```

Allocate the descriptor and storage before realtime execution and consume records before the next
call reuses them. See [Hosting Onda delegates](../../docs/delegates.md) for the complete lifecycle,
capacity guidance, and APIs for other hosts.

## Print batches

Print delivery uses an independent caller-owned batch with the same physical descriptor shape.
`writePrintBatch()`, `readPrintBatch()`, and `decodePrintRecords()` preserve each site's concrete
scalar types; `formatPrintBatch()` and `formatPrintRecords()` produce canonical newline-terminated
text with exact `i64` and width-correct floating-point formatting. Use `writeExecutionOutput()` to
pass independently nullable delegate and print batch addresses to init, process, or event exports.
See [Hosting Onda print output](../../docs/printing.md) for the complete lifecycle and metadata
contract.

The current descriptor represents every bindable buffer-array slot as a physical
`metadata.buffers` entry and records logical contiguous groups in `metadata.buffer_arrays`. At
runtime all four descriptor tables are present when any buffer exists, but an individual sample
pointer may be null. Null entries select the processor's neutral one-frame zero/discard storage;
they are valid bindings, not an incomplete artifact.

`createParamControl()` validates and decodes one parameter descriptor into a reusable control with
`constrainPlain()`, `normalizedToPlain()`, and `plainToNormalized()` methods. It preserves exact
endpoints, clamps out-of-range values, applies linear, logarithmic, or SuperCollider-style curved
mapping, and snaps stepped domains. The package also exports one-shot functions with the same
behavior. Numeric parameters without a control domain and parameter arrays are rejected. These
APIs are synchronous and do not require a compiler, WebAssembly instance, or `AudioWorklet`, so
editors can prepare controls once and use them responsively.
Boolean plain and normalized numeric inputs use the same `value >= 0.5` threshold as the native
runtime and generated processor-object helpers.

`createParamDomain()` prepares the same reusable control from an already-decoded domain. It is
intended for products such as the native run view whose transport already provides numeric
`minimum`, `maximum`, `step`, and `stepCount` values.

Controlled `i64` domains are limited by the descriptor contract to the exact binary64 integer range;
unranged `i64` parameters retain their full width through typed storage.
