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
