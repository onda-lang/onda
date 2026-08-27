# Onda Binaryen Web Backend

This package consumes Onda's versioned MIR as compact MessagePack, JSON, or an already-decoded
object and emits an executable WebAssembly DSP module with Binaryen.js. It runs in browsers and Node
without LLVM, `wasm-ld`, or an Onda toolchain in the code-generation environment.

This is the supported low-level MIR backend. Applications starting from Onda source should normally
use [`@onda-lang/wasm-compiler`](../onda_wasm_compiler/README.md), which packages the browser
frontend and this backend behind one typed source/project API and the `onda-wasm` CLI.

Compatibility status: this package implements the current MIR schema and consumes output from
`onda compile --emit mir-messagepack`, `--emit mir-json`, and `crates/onda_compiler_web`. It validates explicit control-mirror
IDs/persistence, checked `make_slice`, fixed-array and slice reference windows, and current-schema
function attributes before code generation.

```js
import {
  compileTrustedMir,
} from "@onda-lang/binaryen-web";

const mir = await fetch("patch.mir.msgpack").then((response) => response.arrayBuffer());
const { wasm, metadata } = compileTrustedMir(mir);
const instance = await WebAssembly.instantiate(wasm);
```

`compileTrustedMir` accepts only output from Onda's semantic MIR producer, which owns the complete
validator proof including bounds, types, structured definite assignment, and resource legality.
The package intentionally does not expose a partial validator for downloaded or hand-authored MIR:
such a boundary would need to reproduce every invariant enforced by `onda_mir`. The function accepts
a JSON string, a MessagePack `ArrayBuffer`/typed-array view, or a decoded object. JSON remains useful
for inspection; the browser compiler and test corpus use MessagePack as the production transport.

The generated module exports `memory`, `__heap_base`,
`onda_processor_init(params_ptr, state_ptr, mode, output_ptr)`, the 12-argument processor
`onda_process`, and one `onda_event_N` function per declared event. Init, process, and event entries
take an optional call-scoped execution-output pointer carrying independent delegate and print
batches. Each function returns zero on success or a positive generated execution-failure code. These are
the complete wasm32-module profile of the generic
[`Onda processor ABI`](../../docs/processor-abi.md), not a Web Audio-specific interface. The host
owns allocation in linear memory. Metadata contains resolved target/integration facts,
state/parameter layouts, state-backed control-output offsets, flattened audio-port channels, packed
event/delegate payload layouts, print log sites, source tables, and all exported ABI names. See
[Hosting Onda delegates](../../docs/delegates.md) and
[Hosting Onda print output](../../docs/printing.md) for batch sizing, decoding, formatting, and
overflow handling.

`createProcessorArtifactFiles()` validates the final module, computes a SHA-256 digest, and returns
a reusable `.wasm` plus `.onda.json` descriptor pair. `validateProcessorArtifact`,
`parseProcessorMetadata`, and `serializeProcessorMetadata` are also exported for loaders. The
`loadProcessorArtifactFiles()` validates both files and verifies their SHA-256 association before
returning an artifact. The package includes TypeScript declarations and is publishable as
`@onda-lang/binaryen-web`.

The executable backend supports:

- scalar locals, value parameters, state, and runtime parameters
- scalar and fixed scalar-array state/parameter/audio-port addressing
- constants, casts, arithmetic, comparisons, structured `if`/loop control, calls, and scalar or multi-value returns
- input loads, output stores, and immutable constant-data loads
- scalar/fixed-array event payload loads and event ABI wrappers
- scalar/fixed-array control-output stores in the shared state blob
- interleaved mono/static/dynamic external buffers, uniformly clamped source access, fixed
  constant-time buffer collections, nullable host pointers with neutral zero/discard storage, and
  runtime metadata queries
- checked slice construction plus fixed-array/slice reference windows
- address-taken scalar locals legalized through per-function reference scratch storage
- SIMD contiguous slice fill with a scalar tail and bulk-memory contiguous slice copy
- current-schema function attributes as validated, portable optimization hints
- native WebAssembly numeric intrinsics plus on-demand internal transcendental and strict-FMA helpers
- Binaryen validation and optimization before emission

The backend also supports primitive slice locals and reference arguments, event slices, flattened data structs, structure-of-arrays processor state, recursive processor arrays, and canonical top-level/processor oversampling schedules. Oversampling interpolation, substeps, sinc-filter state updates, and output decimation are ordinary MIR operations; Binaryen has no Onda-specific scheduling logic. Recursive call graphs are rejected as unbounded realtime work before fixed-array local storage can become re-entrant. Aggregate shapes that cannot be represented by portable MIR are rejected above the backend boundary.

The MIR schema defines three ordered `i32` process parameters:
`(start_frame, frames, flags)`. `process_frame(offset)` is the checked source of audio-I/O
addresses. The public 12-argument `onda_process` export keeps full-block base pointers and accepts
any segment contained in the configured block. BEGIN and END flags independently gate block hooks;
they do not assert a position or maintain a hidden ABI cursor, and zero-frame calls are valid. The
reference AudioWorklet maintains its own compile-block cursor so Web Audio render quanta may be
smaller or larger than the configured MIR block while DSP state and block-boundary hooks remain
continuous.

Emitted metadata distinguishes physical Wasm state storage from the packed persistent snapshot:
`runtime.state_size_bytes` sizes linear-memory state, while `runtime.snapshot_size_bytes` and
`metadata.states` describe the deterministic scratch-free snapshot layout shared with native MIR.
Snapshot entries carry an `authored` flag so compiler-owned task frames remain serializable without
being presented as user-authored state reflection.

Core WebAssembly has no scalar fused multiply-add or transcendental instructions. The backend
therefore links only the required closure from an embedded, pure-Wasm math kernel into each DSP
module before Binaryen optimization. Calls remain inside one Wasm instance: generated artifacts
have no `onda_math` or exact-FMA host imports, and the AudioWorklet performs no JavaScript math on
the render path. The kernel provides the same f32/f64 `sin`, `cos`, `tan`, `tanh`, `atan`, `atan2`,
`exp`, `log`, `pow`, and strict `fma` surface as native LLVM lowering. Core Wasm instructions still
handle `sqrt`, `abs`, `floor`, `ceil`, `trunc`, `min`, and `max`; a small internal helper implements
LLVM-compatible half-away-from-zero `round`.

The kernel is a reproducible `no_std` Rust build of MIT-licensed `libm` 0.2.16. Its strict software
FMA performs one correctly rounded operation without an allocating JavaScript `BigInt` boundary.
`npm run build:math-kernel` regenerates the embedded module, while `npm run test:fma-oracle`
compares targeted edge cases and deterministic random bit patterns against Rust's native
`mul_add` oracle. Binaryen removes unused helper functions, so a program pays code size only for
the math closure it calls. `createDefaultImports()` remains as a compatibility convenience and now
returns an empty object.

Compilation defaults to Binaryen O4 for speed, strict floating-point optimization, WebAssembly
SIMD enabled, and Binaryen's ordinary inlining policy. Options are explicit and recorded in
`artifact.metadata.optimization`:

```js
compileTrustedMir(mir, {
  optimize: true,
  optimizeLevel: 4,       // 0..4
  shrinkLevel: 0,         // 0..2
  fastMath: false,        // opt in to relaxed floating-point rewrites
  simd: true,             // set false for pre-SIMD Wasm hosts
  allowInliningFunctionsWithLoops: false,
  emitText: false,
});
```

Loop-containing inlining remains opt-in because controlled measurements were workload-dependent:
it helped the oversampling fixture but regressed the language and saturator fixtures. The compiler
saves and restores Binaryen's process-global optimization settings around every module, so one
compile cannot silently change another compile's numerical or inlining policy.

O4 was selected by an affinity-pinned O3/O4 A/B over language, oversampling, saturator, and complete
math workloads. It improved three workloads and left saturator effectively unchanged, at the cost
of higher one-time compilation latency. Binaryen StackIR generation remains off because it improved
some workloads but regressed others in the same matrix.

The MIR schema uses lossless scalar encoding: `i64` values are decimal strings and non-finite floats are exact hexadecimal IEEE bit patterns. The compiler and reference AudioWorklet decode both forms before constructing Wasm constants or initializing host storage.

Install the pinned Binaryen dependency and run the test layers from this directory:

```sh
npm install
npm test
npm run test:onda
npm run test:corpus
```

`npm test` runs current-schema backend, embedded-math-kernel, artifact, and reference-worklet fixtures. `npm run test:onda`
compiles real Onda sources, runs LLVM/MIR-Binaryen render parity through MessagePack, and verifies
exact FMA against Rust's `mul_add`; `npm run test:parity` selects only the differential renderer.
That renderer covers full/segmented/zero-frame scheduling, events, snapshots/restores, numeric edge
semantics, buffers, slices, processor arrays, and oversampling. Strict arithmetic scenarios compare
raw f32 output bits and f64 snapshot storage, with NaN payloads treated as unspecified; only scenarios
that exercise approximate transcendental kernels use numeric tolerances. The source-driven
and parity commands require the native Rust/LLVM Onda build. `npm run test:corpus` continuously
discovers this package's positive fixtures, compiles each through the CLI to current-schema MIR,
lowers it with Binaryen, and validates the generated Wasm. It also
requires the native build. `npm test` does not.

Run `npm run bench` for the reproducible development comparison documented in
[`docs/backend-benchmarks.md`](../../docs/backend-benchmarks.md). Those measurements validate output
and compare native LLVM JIT/processing with Binaryen compilation, Wasm instantiation, and Wasm
processing; they require the native Rust/LLVM Onda build and are not universal browser-performance
claims. The benchmark fails by default if Binaryen/Wasm beats LLVM in any checked scenario; the
report includes the current CPU affinity so heterogeneous-core placement is visible.

The embedded-compiler playground under `examples/web/onda_wasm_playground` builds the Rust compiler
with `wasm-pack`, loads this backend as an ESM module, and compiles edited source to executable DSP
Wasm inside the page. The separate `examples/web/onda_wasm_aot_sample_player` example invokes this
backend at build time and ships only the resulting module and descriptor to the browser.

Current limitations are explicit:

- scalar FMA and transcendental operations are software helpers on core Wasm targets, so LLVM may
  still use faster target instructions or platform libm implementations
- recursive call graphs are rejected as unbounded realtime work
- `inline: always` and `inline: never` are validated but remain advisory because Binaryen.js does
  not expose per-function inline/no-inline annotations; its module optimizer chooses whether to
  inline a call
- this low-level package accepts MIR rather than source; the product-facing `onda-wasm` CLI lives in
  `@onda-lang/wasm-compiler`, while the native `onda compile` command does not yet emit linked Wasm
- production consumers must package/cache Binaryen; Web Audio users can use the separate optional
  `@onda-lang/webaudio` adapter, while other hosts consume the generic artifact directly
