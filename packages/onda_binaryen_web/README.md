# Onda Binaryen Web Backend

This package consumes Onda's versioned MIR as compact MessagePack, JSON, or an already-decoded
object and emits an executable WebAssembly DSP module with Binaryen.js. It runs in browsers and Node
without LLVM, `wasm-ld`, or an Onda toolchain in the code-generation environment.

Compatibility status: this package implements MIR schema 5 and consumes current output from
`onda compile --emit mir-messagepack`, `--emit mir-json`, and `crates/onda_compiler_web`. It validates explicit control-mirror
IDs/persistence, checked `make_slice`, fixed-array and slice reference windows, and schema-5
function attributes before code generation.

```js
import {
  compileTrustedMir,
  createDefaultImports,
} from "@onda-lang/binaryen-web";

const mir = await fetch("patch.mir.msgpack").then((response) => response.arrayBuffer());
const { wasm, metadata } = compileTrustedMir(mir);
const instance = await WebAssembly.instantiate(wasm, createDefaultImports());
```

`compileMir` is the safe boundary for arbitrary MIR and rejects every operation marked with
unchecked bounds. `compileTrustedMir` is reserved for output from Onda's semantic MIR producer,
which owns the corresponding range proofs. Naming that trust transition prevents a downloaded or
hand-authored MIR document from asserting its own memory-safety proof. Both functions accept a JSON
string, a MessagePack `ArrayBuffer`/typed-array view, or a decoded object. JSON remains useful for
inspection; the browser compiler and test corpus use MessagePack as the production transport.

The generated module exports `memory`, `__heap_base`, `onda_init(params_ptr, state_ptr)`, the native-compatible 11-argument `onda_process`, and one native-compatible `onda_event_N` function per declared event. The host owns allocation in linear memory. Metadata contains state/parameter layouts, state-backed control-output offsets, flattened audio-port channels, packed event payload layouts, and all exported ABI names.

The executable backend supports:

- scalar locals, value parameters, state, and runtime parameters
- scalar and fixed scalar-array state/parameter/audio-port addressing
- constants, casts, arithmetic, comparisons, structured `if`/loop control, calls, and scalar or multi-value returns
- input loads, output stores, and immutable constant-data loads
- scalar/fixed-array event payload loads and event ABI wrappers
- scalar/fixed-array control-output stores in the shared state blob
- interleaved mono/static/dynamic external buffers, including reads, writes, bounds modes, and runtime metadata queries
- checked slice construction plus fixed-array/slice reference windows
- schema-5 function attributes as validated, portable optimization hints
- native WebAssembly numeric intrinsics plus optional `onda_math` imports for transcendental operations
- Binaryen validation and optimization before emission

The backend also supports primitive slice locals and reference arguments, event slices, flattened data structs, structure-of-arrays processor state, recursive processor arrays, and canonical top-level/processor oversampling schedules. Oversampling interpolation, substeps, sinc-filter state updates, and output decimation are ordinary MIR operations; Binaryen has no Onda-specific scheduling logic. Recursive call graphs are rejected as unbounded realtime work before fixed-array local storage can become re-entrant. Aggregate shapes that cannot be represented by portable MIR are rejected above the backend boundary.

MIR schema 5 retains the three ordered `i32` process parameters introduced in schema 4:
`(start_frame, frames, flags)`. `process_frame(offset)` is the checked source of audio-I/O
addresses. The public 11-argument `onda_process` export keeps full-block base pointers and accepts
any segment contained in the configured block. BEGIN and END flags independently gate block hooks;
they do not assert a position or maintain a hidden ABI cursor, and zero-frame calls are valid. The
reference AudioWorklet maintains its own compile-block cursor so Web Audio render quanta may be
smaller or larger than the configured MIR block while DSP state and block-boundary hooks remain
continuous.

Emitted metadata distinguishes physical Wasm state storage from the packed persistent snapshot:
`runtime.state_size_bytes` sizes linear-memory state, while `runtime.snapshot_size_bytes` and
`metadata.states` describe the deterministic scratch-free snapshot layout shared with native MIR.

Core WebAssembly has no scalar fused multiply-add instruction. MIR `fma` therefore uses the explicitly versioned `onda_exact_math_v1` support ABI instead of being silently expanded to a rounded multiply followed by an add. Wasm passes the three operands and result as `i32`/`i64` IEEE bit patterns through `fma_f32_bits` or `fma_f64_bits`. The bundled support reconstructs the exact integer product and sum with `BigInt`, then performs one round-to-nearest, ties-to-even step. It handles normals, subnormals, underflow, overflow, infinities, deterministic canonical quiet NaNs, exact cancellation, and signed zero. `createDefaultImports()` includes this support; a custom host can merge `createExactMathImports()` and should use `artifact.metadata.imports` to discover when it is required.

The exact support prioritizes semantics over throughput: each dynamic `fma` crosses a Wasm import boundary and performs allocating `BigInt` arithmetic, so it is materially slower and less realtime-friendly than a native hardware FMA. Avoid placing it in a dense per-sample hot path when latency is critical. The versioned bit ABI allows a host to substitute a faster implementation only if it returns identical IEEE bits. `npm run test:fma-oracle` compares targeted edge cases and deterministic random bit patterns against Rust's native `mul_add` oracle.

MIR schema 5 retains the lossless scalar encoding introduced by schema 3: `i64` values are decimal strings and non-finite floats are exact hexadecimal IEEE bit patterns. The compiler and reference AudioWorklet decode both forms before constructing Wasm constants or initializing host storage.

Install the pinned Binaryen dependency and run the test layers from this directory:

```sh
npm install
npm test
npm run test:onda
npm run test:corpus
```

`npm test` runs schema-5 backend, exact-math, and AudioWorklet fixtures. `npm run test:onda`
compiles real Onda sources, runs LLVM/MIR-Binaryen render parity through MessagePack, and verifies
exact FMA against Rust's `mul_add`; `npm run test:parity` selects only the differential renderer.
That renderer covers full/segmented/zero-frame scheduling, events, snapshots/restores, numeric edge
semantics, buffers, slices, processor arrays, and oversampling. The source-driven
and parity commands require the native Rust/LLVM Onda build. `npm run test:corpus` continuously
discovers every `.onda` program under `examples/` and this package's positive fixtures, compiles each
through the CLI to schema-5 MIR, lowers it with Binaryen, and validates the generated Wasm. It also
requires the native build. `npm test` does not.

Run `npm run bench` for the reproducible development comparison documented in
[`docs/BACKEND_BENCHMARKS.md`](../../docs/BACKEND_BENCHMARKS.md). Those measurements validate output
and compare native LLVM JIT/processing with Binaryen compilation, Wasm instantiation, and Wasm
processing; they require the native Rust/LLVM Onda build and are not universal browser-performance
claims. The benchmark fails by default unless LLVM is at least 5% faster in every checked scenario;
the report includes the current CPU affinity so heterogeneous-core placement is visible.

The browser playground under `examples/web/sine_wasm_worklet` builds the Rust compiler with
`wasm-pack`, loads this backend as an ESM module, and compiles edited source to executable DSP Wasm
inside the page.

Current limitations are explicit:

- exact FMA is correct but expensive because it crosses an import boundary and allocates BigInts
- transcendental operations use `onda_math` imports because core WebAssembly lacks those instructions
- recursive call graphs are rejected as unbounded realtime work
- `inline: always` and `inline: never` are validated but remain advisory because Binaryen.js does
  not expose per-function inline/no-inline annotations; its module optimizer chooses whether to
  inline a call
- this package is a JavaScript backend API, not yet a first-class `onda compile --emit wasm` CLI mode
- production consumers still need to package/cache Binaryen and provide an AudioWorklet or other host
