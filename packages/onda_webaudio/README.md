# Onda Web Audio adapter

See [api.md](api.md) for the complete public Web API shared with the released package.

This optional package hosts a complete wasm32 Onda processor artifact in an `AudioWorklet`. It is a
reference adapter for the generic Onda processor ABI; Web Audio is not required by the compiler or
by native and relocatable-WebAssembly object consumers.

```js
import {
  createOndaAudioProcessorInitialized,
  ONDA_INIT_FULL,
  ONDA_INIT_PRESERVE_PINNED,
} from "@onda-lang/webaudio";

const processor = await createOndaAudioProcessorInitialized(audioContext, artifact, {
  params: { gain: 0.5 },
  buffers: {},
});
processor.node.connect(audioContext.destination);
await processor.setParam("gain", 0.75);
await processor.setParamNormalized("cutoff", 0.5);
const snapshot = await processor.snapshot();
await processor.init(ONDA_INIT_PRESERVE_PINNED);
await processor.init(ONDA_INIT_FULL);
```

The adapter registers `onda-wasm-processor`, derives Web Audio channel options from artifact
metadata, marshals declared scalar widths, schedules arbitrary render quanta across Onda compile
blocks, and provides request/response helpers for parameters, events, buffers, control outputs,
initialization, and portable snapshots.

`createOndaAudioProcessor()` is allocation-only. This lets a host configure parameters before the
first initializer run:

```js
const processor = await createOndaAudioProcessor(audioContext, artifact);
await processor.setParam("gain", 0.75);
await processor.init(ONDA_INIT_FULL);
```

Until full initialization succeeds, the worklet emits silence and rejects stateful control
operations. Successful initialization switches it to the initialized process callback, so the
steady-state audio callback does not retain a lifecycle branch.

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

Call `processor.close()` when the adapter is no longer used. Closing is idempotent and terminal: it
rejects pending requests, removes listeners, and makes subsequent operations fail immediately. The
wrapped `AudioWorkletNode` and `AudioContext` remain caller-owned.

## Prints

Authored `print(...)` occurrences leave generated execution as bounded typed records. The worklet
copies those records without formatting or allocating strings during audio rendering; the main-side
adapter turns them into canonical, newline-terminated text:

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  printCapacityBytes: 128 * 1024,
});

const unsubscribe = processor.onPrint(({
  text,
  entries,
  overflowCount,
  transportDropCount,
}) => {
  logView.append(text);
  if (overflowCount) console.warn(`${overflowCount} generated print records were dropped`);
  if (transportDropCount) console.warn(`${transportDropCount} print records missed UI transport`);
});
```

`text` is ready to write or display and contains one newline-terminated line per occurrence.
`entries` retains typed values and source/log-site metadata for source-aware consumers. Generated
batch overflow and bounded worklet-to-main transport loss are reported separately; loss-only
notifications are delivered even when no later authored print arrives. Capacity defaults to 64 KiB
and can be set to zero to suppress host delivery without suppressing argument evaluation.
See the internal [print host integration](../../docs/printing.md) reference for scalar formatting,
source metadata, and the equivalent Rust, C, and raw processor APIs.

## Delegates

Top-level delegates are delivered after generated execution through `onDelegates()`. The worklet
uses reusable storage allocated during construction; listeners run from the main-side message
handler, not as callbacks inside generated DSP code.

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  delegateCapacityBytes: 128 * 1024,
});

const unsubscribe = processor.onDelegates(({
  occurrences,
  overflowCount,
  transportDropCount,
}) => {
  for (const occurrence of occurrences) {
    console.log(occurrence.name, occurrence.values);
  }
  if (overflowCount) console.warn(`${overflowCount} delegate records were dropped`);
  if (transportDropCount) {
    console.warn(`${transportDropCount} delegate records were dropped in transport`);
  }
});
```

Capacity defaults to 64 KiB and can be set to zero to disable host collection. It is a host policy,
not a compiler-computable exact whole-call size: occurrence counts and slice payload lengths may be
runtime-dependent. The worklet supplies delegate storage to generated code only while at least one
listener is registered, and decoding happens on the main side. `overflowCount` reports insufficient
configured capacity; `transportDropCount` separately reports records discarded by the bounded
worklet-to-main queue. Internal Onda `when` handlers still run in either case. See the internal
[delegate host integration](../../docs/delegates.md) reference for sizing and lifecycle details.

## Real-time behavior

`createOndaAudioProcessor` compiles the processor's `WebAssembly.Module` concurrently with worklet
registration, before constructing the `AudioWorkletNode`. The module is structured-cloned into the
worklet, so processor construction only instantiates it. Applications creating several instances can
compile once and reuse the module:

```js
import {
  compileOndaProcessorModule,
  createOndaAudioProcessorInitialized,
} from "@onda-lang/webaudio";

const compiledModule = await compileOndaProcessorModule(artifact);
const left = await createOndaAudioProcessorInitialized(context, artifact, { compiledModule });
const right = await createOndaAudioProcessorInitialized(context, artifact, { compiledModule });
```

After construction, the normal f32 render callback reuses cached Wasm-memory views and performs no
host-side allocation or memory growth. Full-block f32 inputs and outputs use typed-array bulk copies;
segmented callbacks and other ABI scalar widths use preallocated typed views with conversion loops
(i64 input conversion necessarily creates JavaScript `BigInt` values). External buffers are copied
into Wasm with typed-array bulk operations during construction. Missing or `null` bindings install
neutral one-frame descriptors: reads return zero, writes are discarded, exact channel counts are
retained, and dynamic-channel buffers report one channel. Supplied bindings must still contain
nonempty, correctly shaped data.

Fixed buffer arrays may be supplied under their logical group name. Each slot is independent:

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  buffers: {
    impulse: { data: impulseSamples, channels: 1, sampleRate: 48_000 },
    bank: [
      null,
      { data: secondSamples, channels: 1, sampleRate: 48_000 },
      // Remaining slots are neutral.
    ],
  },
});
```

Hosts may instead pass a flat array in physical descriptor order or key individual physical names
such as `"bank[1]"`. Logical group metadata determines the contiguous slot range; no sample data is
copied when Onda selects a slot while processing. Initial descriptors are installed before the
initialized constructor runs full initialization, so authored top-level and proc init code can
preprocess the supplied samples once before rendering begins.

Artifact descriptors and module exports are validated by the shared, compiler-free
`@onda-lang/processor-abi` package before anything reaches the rendering thread.
If generated init or event code returns a nonzero execution status, the adapter reports the error
to the caller. A failing process call reports an `onda-error` and emits silence. Any generated-code
failure invalidates the live state, so later callbacks remain silent and stateful operations are
rejected until full initialization or snapshot restoration succeeds.
`init(ONDA_INIT_PRESERVE_PINNED)` reruns generated initialization
while preserving pinned roots and task continuations; `init(ONDA_INIT_FULL)` initializes the
complete physical state and is required before processing an instance returned by
`createOndaAudioProcessor`. The initialized convenience constructor performs full initialization
during worklet construction. Neither mode allocates on the successful path, and a
failure returns the processor to its silent pending state until full initialization or snapshot
restore succeeds. Suspend or disconnect playback before initialization that performs substantial
work.

Dynamic event storage is also allocated before rendering. Its default capacity is 64 KiB per
processor with dynamic events and can be changed explicitly:

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  eventPayloadCapacityBytes: 256 * 1024,
});
```

An event exceeding the configured capacity is rejected instead of growing memory on the rendering
thread. Parameters and ordinary events are lightweight control operations. Snapshot creation,
snapshot restore, control-output reads, and especially complete external-buffer reads necessarily
copy data; suspend or disconnect real-time playback before requesting large transfers.
