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

`createParamControl()` validates and decodes one parameter descriptor into a reusable control with
`constrainPlain()`, `normalizedToPlain()`, and `plainToNormalized()` methods. It preserves exact
endpoints, clamps out-of-range values, applies linear, logarithmic, or SuperCollider-style curved
mapping, and snaps stepped domains. The package also exports one-shot functions with the same
behavior. Numeric parameters without a control domain and parameter arrays are rejected. These
APIs are synchronous and do not require a compiler, WebAssembly instance, or `AudioWorklet`, so
editors can prepare controls once and use them responsively.

Controlled `i64` domains are limited by the descriptor contract to the exact binary64 integer range;
unranged `i64` parameters retain their full width through typed storage.
