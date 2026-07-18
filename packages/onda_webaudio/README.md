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
const snapshot = await processor.snapshot();
```

The adapter registers `onda-wasm-processor`, derives Web Audio channel options from artifact
metadata, marshals declared scalar widths, schedules arbitrary render quanta across Onda compile
blocks, and provides request/response helpers for parameters, events, buffers, control outputs,
reset, and portable snapshots.

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
into Wasm with typed-array bulk operations during construction.

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
