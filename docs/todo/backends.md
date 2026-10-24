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

The [raw-object C host](../../examples/native/raw_processor_object/README.md) already documents
linking and execution on Linux, macOS, and Windows, including the native math-library dependency.

- Extend cross-target object-emission checks into consumer link/run smoke coverage for ELF, COFF,
  and Mach-O, plus big-endian and eventually wasm64 targets supported by the configured LLVM build.
  Current tests already emit native math objects for Linux, macOS, and Windows targets and verify
  an actual relocatable wasm32 object with its `linking` section; foreign execution remains separate.
- Decide whether descriptor embedding is useful in addition to the sidecar. Sidecars remain the
  portable baseline because embedding differs by object format and final container.

## Processor ABI evolution

- Consider a future namespaced-symbol profile for linking several Onda processors into one artifact.
- Extend the existing compiler-free `onda_processor_abi` Rust schema with portable descriptor and
  artifact validation, and expose it to native C hosts where useful. Reuse the JavaScript package's
  validation contract rather than adding another schema or compiler dependency. The generated C
  API remains a separate higher-level embedding surface.
- Preserve the packed little-endian snapshot format across targets and add big-endian physical-layout
  round trips when such an LLVM runner is available in CI.

## Browser product

- Add mutable-buffer inspection and control-output display.
- Implement seamless or crossfaded hot swap with an explicit snapshot migration policy.
- Measure production first-load/transfer budgets and compression/cache behavior. Playground and
  compiler-worker bundles already use versioned, content-addressed asset directories; evaluate
  further splitting from measurements, including the large pinned Binaryen distribution.
- Add automated Chromium, Firefox, and Safari coverage for worklet registration, audible processing,
  snapshot/restore, autoplay behavior, and browsers that choose a different `AudioContext` sample
  rate.

## Optimization follow-ups

- Keep Onda source scalar and select SIMD inside backends. Consume shared alignment, range, and
  dependence facts from [compiler analyses](compiler.md#dsp-loop-proofs-and-diagnostics).
- Improve native vectorization decisions for small strided kernels. The current f32 FFT's fused
  8-point base stage lowers to extensive shuffles and register spills; evaluate vectorization
  direction, width, interleaving, and register pressure with measured code quality rather than
  assuming wider vectors are faster. Keep target-specific cost decisions in the LLVM backend.
- Broaden Binaryen/Wasm SIMD beyond slice fills and independent straight-line sample loops with
  invariant controls. General array/voice loops and helper calls remain candidates. Start with
  independent contiguous and mirrored-range kernels,
  preserving numerical and memory semantics with correct scalar handling where required.
- Continue isolated real-spectrum reconstruction measurements now that shared MIR proofs enable
  vectorization of mirrored bin pairs. Check shuffle/spill costs and wider target coverage against
  complete-transform comparisons in the [benchmark results folder](../../scripts/bench/results/).
- Continue the affinity-pinned LLVM/Binaryen matrix as MIR passes and backend versions change.
- Measure guard overhead on short partially overlapping audio calls. Use the existing
  short-copy comparisons in the [benchmark results folder](../../scripts/bench/results/)
  as target-cost regression coverage.
- Extend [native vector math](#native-vector-math) only with isolated generated-code and runtime
  evidence. Evaluate failure-path layout separately with the same measurement discipline.
- Evaluate targeted MIR analyses only when they expose reusable facts that a backend cannot
  reliably recover; avoid encoding target-specific vector widths or loop policies in MIR.
- Re-evaluate WebAssembly relaxed SIMD and future native core-Wasm math/FMA proposals when broadly
  deployable. Current strict transcendental and FMA helpers are internal and correct but naturally
  slower than native hardware/library paths in dense math workloads.
- Keep backend pass policies evidence-driven. LLVM uses its standard O3 pipeline and Binaryen uses
  O4; global loop-inlining and StackIR experiments remain off because the complete workload matrix
  showed regressions.

## Native vector math

The [bundled SLEEF kernels](../../crates/onda_codegen_llvm/math-kernel/README.md) already cover
Onda's current transcendental builtins. Fixed-width vector `exp2` introduced by LLVM is also
resolved after application optimization. Further candidates, in priority order:

- Investigate optimizer-created `exp10` first: LLVM 21 can rewrite `pow(10, x)` to `llvm.exp10`
  when the destination advertises the corresponding library function. Check generated IR across
  supported targets, then extend late resolution if these calls lose vectorization or become
  scalar library calls. SLEEF already provides vector `exp10`; this does not require a new Onda
  builtin. See [LLVM's rewrite](https://github.com/llvm/llvm-project/blob/llvmorg-21.1.2/llvm/lib/Transforms/Utils/SimplifyLibCalls.cpp#L2179-L2192)
  and [SLEEF 3.9.0's vector functions](https://github.com/shibatch/sleef/blob/906ca7512ee483296780a81a21b9ca715d40dfe1/docs/x86.xhtml#pow).
- Benchmark fusion of `sin(x)` and `cos(x)` for the same vector argument, using SLEEF's
  [two-result `sincos_u35` family](https://github.com/shibatch/sleef/blob/906ca7512ee483296780a81a21b9ca715d40dfe1/docs/x86.xhtml#trig). Oscillator and
  complex/FFT kernels are useful fixtures. This needs a backend-local multi-result helper path;
  the current [generated mapping](../../crates/onda_codegen_llvm/src/vector_math_spec.rs) describes
  only single-result unary/binary functions. Verify numerical differences, register pressure,
  and scalar tails before choosing fusion.
- Consider explicit `exp2`, `log2`, `log10`, `expm1`, `log1p`, `hypot`, and `cbrt` operations when
  DSP workloads justify them; inverse trig/hyperbolic functions and `sinpi`/`cospi` are additional
  options. These exist in [SLEEF 3.9.0](https://github.com/shibatch/sleef/blob/906ca7512ee483296780a81a21b9ca715d40dfe1/docs/purec.xhtml) but are absent from the current
  [MIR intrinsic surface](../../crates/onda_mir/src/ir.rs). Adding them requires a language/MIR
  decision and scalar/Binaryen behavior as well as native vector mappings. Do not silently replace
  existing compound expressions when rounding, overflow, underflow, or domain behavior differs.
- Preserve direct LLVM lowering for `sqrt`, `abs`, rounding, min/max, and FMA unless a measured
  target-specific gap warrants a library helper; these already use
  [LLVM intrinsics](../../crates/onda_codegen_llvm/src/orc_backend/mir_native/function_emitter.rs).

For each extension, select the accuracy family explicitly, retain the helpers' protected numerical
policy and freestanding linkage, and cover f32/f64, supported ISA widths, exceptional inputs,
scalar tails, and standalone objects. Measure runtime and code size before expanding the bundle.

## Other backends

- A single-file C++ header backend remains a possible portability/export feature. It should consume
  validated MIR, preserve the processor ABI semantics, allocate no memory in the audio callback,
  and participate in the same differential render suite.
