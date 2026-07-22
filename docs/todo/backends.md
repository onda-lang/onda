# Backend TODO

## Current contract

Onda owns one backend-neutral processor ABI, documented in
[`docs/processor-abi.md`](../processor-abi.md). MIR and the logical processor interface do not change
with the target:

- LLVM emits optimized relocatable objects for native and WebAssembly target triples. Onda does not
  bundle a linker; the application uses its normal platform toolchain and the processor descriptor.
- The browser-safe Binaryen backend emits a complete, self-contained core-Wasm module because the
  browser has no object-linking interface.
- `packages/onda_webaudio` is an optional host adapter, not part of the ABI or either backend.

Implemented browser path:

```text
source editor -> compiler worker -> MIR -> Binaryen O4 -> Wasm + descriptor
                                                        -> optional AudioWorklet adapter
```

The playground can export the reusable `.wasm` and integrity-checked `.onda.json` descriptor. It
does not need LLVM, a server-side compiler, `wasm-ld`, or JavaScript math callbacks.

## AOT confidence

- Add CI smoke coverage for representative ELF, COFF, Mach-O, big-endian, wasm32, and eventually
  wasm64 targets supported by the configured LLVM build. Current tests cover the host object and an
  actual relocatable wasm32 object with its `linking` section.
- Add consumer examples for linking an Onda object into a small C/C++ host on each major object
  format. These examples should invoke the user's linker; they must not turn the Onda compiler into
  a platform toolchain driver.
- Decide whether descriptor embedding is useful in addition to the sidecar. Sidecars remain the
  portable baseline because embedding differs by object format and final container.
- Document target runtime/library dependencies for LLVM objects when a program's intrinsic surface
  requires them.

## Processor ABI evolution

- Keep the ABI stable while the metadata format evolves through additive optional fields.
- Consider a future namespaced-symbol profile for linking several Onda processors into one artifact.
- Add a portable descriptor parser/validator crate so native C/Rust hosts need not duplicate JSON
  validation. The generated C API remains a separate higher-level embedding surface.
- Preserve the packed little-endian snapshot format across targets and add big-endian physical-layout
  round trips when such an LLVM runner is available in CI.

## Browser product

- Expose the compiler's existing virtual multi-file project API in the editor.
- Add buffer loading/inspection, control-output display, and microphone/host-input routing.
- Implement seamless or crossfaded hot swap with an explicit snapshot migration policy.
- Add production asset splitting, compression, cache/version policy, and measured first-load budgets;
  the pinned Binaryen distribution is large when served uncompressed.
- Add automated Chromium, Firefox, and Safari coverage for worklet registration, audible processing,
  snapshot/restore, autoplay behavior, and browsers that choose a different `AudioContext` sample
  rate.
- Consider a shareable artifact/source bundle. Keep the exported `.wasm` plus `.onda.json` pair as
  the reusable low-level product rather than inventing a browser-only compiler ABI.

## Optimization follow-ups

- Continue the affinity-pinned LLVM/Binaryen matrix as MIR passes and backend versions change.
- Evaluate targeted MIR analyses only when they expose facts that both LLVM and Binaryen cannot
  reliably recover; avoid encoding target-specific vector widths or loop policies in MIR.
- Re-evaluate WebAssembly relaxed SIMD and future native core-Wasm math/FMA proposals when broadly
  deployable. Current strict transcendental and FMA helpers are internal and correct but naturally
  slower than native hardware/library paths in dense math workloads.
- Keep backend pass policies evidence-driven. LLVM uses its standard O3 pipeline and Binaryen uses
  O4; global loop-inlining and StackIR experiments remain off because the complete workload matrix
  showed regressions.

## Other backends

- A single-file C++ header backend remains a possible portability/export feature. It should consume
  validated MIR, preserve the processor ABI semantics, allocate no memory in the audio callback,
  and participate in the same differential render suite.
