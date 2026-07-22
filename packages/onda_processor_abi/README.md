# Onda processor ABI helpers

`@onda-lang/processor-abi` is the small, compiler-free JavaScript contract for Onda processor
artifacts. It validates current-format descriptors and complete core-WebAssembly modules, creates and
loads integrity-associated `.wasm`/`.onda.json` pairs, and provides the shared TypeScript surface
used by `@onda-lang/wasm-compiler`, `@onda-lang/binaryen-web`, and `@onda-lang/webaudio`.

```js
import {
  loadProcessorArtifactFiles,
  validateProcessorArtifact,
} from "@onda-lang/processor-abi";
```

This package does not compile Onda source or host Web Audio. It exists so loaders and hosts can
validate artifacts without installing the compiler or duplicating the ABI schema.
The detailed TypeScript records mirror `onda_processor_abi::ProcessorDescriptor`; both packages
validate the same checked-in conformance fixture.
