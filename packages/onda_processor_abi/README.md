# Onda processor ABI helpers

`@onda-lang/processor-abi` is the small, compiler-free JavaScript contract for Onda processor
artifacts. It validates current-format descriptors and complete core-WebAssembly modules, creates and
loads integrity-associated `.wasm`/`.onda.json` pairs, and provides the shared TypeScript surface
used by `@onda-lang/wasm-compiler`, `@onda-lang/binaryen-web`, and `@onda-lang/webaudio`.

```js
import {
  loadProcessorArtifactFiles,
  paramNormalizedToPlain,
  paramPlainToNormalized,
  validateProcessorArtifact,
} from "@onda-lang/processor-abi";

const cutoff = artifact.metadata.metadata.params.find(
  (param) => param.name === "cutoff",
);
const plain = paramNormalizedToPlain(cutoff, 0.5);
const normalized440 = paramPlainToNormalized(cutoff, 440);
```

This package does not compile Onda source or host Web Audio. It exists so loaders and hosts can
validate artifacts without installing the compiler or duplicating the ABI schema.
The detailed TypeScript records mirror `onda_processor_abi::ProcessorDescriptor`; both packages
validate the same checked-in conformance fixture.

The pure `constrainParamPlain()`, `paramNormalizedToPlain()`, and
`paramPlainToNormalized()` helpers implement the descriptor's canonical parameter-control rules.
They support scalar numeric and Boolean host controls, preserve exact endpoints, clamp out-of-range
values, apply linear or logarithmic mapping, and snap stepped domains. Numeric parameters without a
control domain and parameter arrays are rejected. These functions are synchronous and do not
require a compiler, WebAssembly instance, or `AudioWorklet`, so editors can use them for responsive
display and exact plain-value entry.
