# Onda Web Audio adapter

This optional package hosts a complete wasm32 Onda processor artifact in an `AudioWorklet`. It is a
reference adapter for the generic Onda processor ABI; Web Audio is not required by the compiler or
by native and relocatable-WebAssembly object consumers.

```js
import { createOndaAudioProcessor } from "@onda-lang/webaudio";

const processor = await createOndaAudioProcessor(audioContext, artifact, {
  params: { gain: 0.5 },
  buffers: {},
});
processor.node.connect(audioContext.destination);
await processor.setParam("gain", 0.75);
await processor.setParamNormalized("cutoff", 0.5);
const snapshot = await processor.snapshot();
```

The adapter registers `onda-wasm-processor`, derives Web Audio channel options from artifact
metadata, marshals declared scalar widths, schedules arbitrary render quanta across Onda compile
blocks, and provides request/response helpers for parameters, events, buffers, control outputs,
reset, and portable snapshots.

## Parameters

`params` passed during construction and values passed to `setParam()` are plain Onda values. The
adapter clamps ranged scalar values and snaps stepped domains before posting them; the real-time
worklet only writes the resulting canonical value in the declared scalar representation. Unknown
initial parameter names or indices are rejected before the worklet node is constructed.

`setParamNormalized()` accepts a host value in `[0, 1]`. The adapter uses the artifact descriptor's
linear or logarithmic scale and step metadata to convert it to a plain value before posting the
write to the worklet. Boolean normalized values use the `0.5` threshold. For example:

```js
await processor.setParam("cutoff", 440);          // exactly 440 Hz
await processor.setParamNormalized("cutoff", 0.5); // midpoint in its declared control scale
```

The same synchronous conversion helpers are re-exported for UI display and typed entry:

```js
import {
  paramNormalizedToPlain,
  paramPlainToNormalized,
} from "@onda-lang/webaudio";

const cutoff = processor.metadata.metadata.params.find(
  (param) => param.name === "cutoff",
);
const displayValue = paramNormalizedToPlain(cutoff, sliderValue);
const sliderValueFor440Hz = paramPlainToNormalized(cutoff, 440);
```

The helpers preserve exact endpoints, clamp and snap consistently with adapter writes, and reject
arrays or numeric parameters without a host-control domain. `scale`, `curve`, `unit`, `step_repr`,
and `step_count` remain available on each parameter's descriptor metadata for control construction
and formatting. For repeated UI conversion, `createParamControl(param)` prepares and validates the
descriptor once and returns bound conversion methods.

Controlled `i64` domains use the descriptor's exact binary64 integer range. Full-width unranged
`i64` values continue to use `bigint` when written directly.

The artifact must be compiled for exactly `audioContext.sampleRate`; the adapter rejects a mismatch
before registering the node so sample-rate-derived language semantics cannot silently drift. A Web
Audio processor must expose at least one audio input or output, because an empty callback surface
does not carry a render-quantum frame count. Control-only artifacts remain usable through the generic
processor ABI in a non-Web-Audio host.

## Real-time behavior

`createOndaAudioProcessor` compiles the processor's `WebAssembly.Module` concurrently with worklet
registration, before constructing the `AudioWorkletNode`. The module is structured-cloned into the
worklet, so processor construction only instantiates it. Applications creating several instances can
compile once and reuse the module:

```js
import {
  compileOndaProcessorModule,
  createOndaAudioProcessor,
} from "@onda-lang/webaudio";

const compiledModule = await compileOndaProcessorModule(artifact);
const left = await createOndaAudioProcessor(context, artifact, { compiledModule });
const right = await createOndaAudioProcessor(context, artifact, { compiledModule });
```

After construction, the normal f32 render callback reuses cached Wasm-memory views and performs no
host-side allocation or memory growth. Full-block f32 inputs and outputs use typed-array bulk copies;
segmented callbacks and other ABI scalar widths use preallocated typed views with conversion loops
(i64 input conversion necessarily creates JavaScript `BigInt` values). External buffers are copied
into Wasm with typed-array bulk operations during construction. Every declared external buffer must
be supplied with nonempty data; the adapter rejects missing or empty bindings before creating the
rendering node.

Artifact descriptors and module exports are validated by the shared, compiler-free
`@onda-lang/processor-abi` package before anything reaches the rendering thread.

Dynamic event storage is also allocated before rendering. Its default capacity is 64 KiB per
processor with dynamic events and can be changed explicitly:

```js
const processor = await createOndaAudioProcessor(context, artifact, {
  eventPayloadCapacityBytes: 256 * 1024,
});
```

An event exceeding the configured capacity is rejected instead of growing memory on the rendering
thread. Parameters and ordinary events are lightweight control operations. Snapshot creation,
snapshot restore, control-output reads, and especially complete external-buffer reads necessarily
copy data; suspend or disconnect real-time playback before requesting large transfers.
