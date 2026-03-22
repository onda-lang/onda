# TODO

## Next Features

- Editor / daemon follow-ups
  - Expand `omni lsp` beyond diagnostics + semantic tokens:
    add hover, go-to-definition, document symbols, completion, and cancellation-aware analysis scheduling.
  - Improve diagnostic cadence:
    evaluate publish-on-change/debounced diagnostics in addition to the current open/save flow.
  - Stabilize daemon/editor transport boundaries:
    decide which preview-control pieces stay private versus becoming a documented protocol.
  - Keep VSCode syntax highlighting and semantic tokens aligned as the language grows.
  - Add an extension smoke-test path or automation for:
    `omni lsp`, `Omni: Run Patch`, semantic tokens, and preview webview controls.
  - Improve preview panel UX:
    better knob/slider affordances, richer status/errors, and explicit device/runtime state display.
  - Broaden preview buffer ingestion beyond current WAV-only `hound` path if warranted.

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Clarify current exclusions in docs: proc-local defs are still not overloadable.
  - Clarify/document overload behavior for complex untyped array/buffer inference-heavy call sites.

- `const` follow-ups
  - Evaluate extending `const` beyond scalar primitives to arrays/structural compile-time values where justified.
  - Decide whether forward references and cycle diagnostics should be supported instead of the current strict lexical-order rule.
  - Fix float-literal precision/typing for typed constants and typed declarations:
    today decimal literals are parsed as `f32`, so `const X: f64 = 0.33333333333` widens an `f32` value instead of preserving `f64` precision.

- Graph composition follow-ups
  - Widen graph source expressions:
    support array-constructor sources and any other remaining non-call source forms where semantics stay unambiguous.
  - Evaluate opt-in graph-edge coercions/broadcasting:
    endpoint-family expansion for proc arrays and broader numeric coercion rules.
    Example endpoint-family expansion:
    ```omni
    init:
      voices: Voice[4] = Voice()

    graph:
      env.out1 >> voices.gain
    ```
    which would expand to:
    ```omni
    graph:
      env.out1 >> voices[0].gain
      env.out1 >> voices[1].gain
      env.out1 >> voices[2].gain
      env.out1 >> voices[3].gain
    ```
    Example broader numeric coercion:
    ```omni
    params:
      mode: i32 = 0

    graph:
      gate >> mode
    ```
    where today an explicit cast would still be preferred:
    ```omni
    graph:
      i32(gate) >> mode
    ```
  - Improve graph diagnostics further where useful:
    especially more explicit hints on inferred-`@block` failures and richer cycle path reporting.

- Events follow-ups
  - Add deeper conformance tests for complex proc-event forwarding chains and nested dispatch edge cases.
  - Add deeper conformance tests for proc-event slice forwarding edge cases (aliases, nested field arrays, and diagnostic coverage).
  - Add deeper conformance tests for host slice-event payload layouts, truncation diagnostics, and mixed fixed/slice event signatures.

- Oversampling follow-ups
  - Consider user-exposed quality/performance modes.
  - Consider selective/local oversampling syntax in addition to full-block `sample N:`.

- Standard library follow-ups
  - Keep the built-in module inventory synced across `README.md`, `INFO.md`, and `SYNTAX.md`:
    `std/prelude`, `std/math`, `std/export_math`, `std/complex`, `std/osc`, `std/filter`, `std/env`,
    `std/delay`, `std/data`, `std/lookup`, `std/fft`, `std/convolution`.
  - Decide which stdlib modules are considered stable MVP surface versus still-evolving API.
  - Plan the next expansion/versioning pass beyond the current shipped module set.

- Tuple follow-ups
  - Nested tuples (`((f32, f32), i32)`).
  - Expression-level indexing (`calcIdx(pos)[0]` without an intermediate variable).
  - Tuple equality/comparison.
  - Tuple in proc port types.

- Generics follow-ups
  - Add focused conformance tests for explicit vs inferred generic specialization across `struct`/`proc` and stdlib usage.
  - Document generic ownership/error rules in a dedicated language-spec section (`T` must belong to the current generic owner).

- Range declarations follow-ups
  - Evaluate whether range syntax should be extended to array `ins`/`params` declarations.
  - Decide whether generated `min/max` clamp lowering should gain explicit NaN/Inf sanitization semantics.

## Backends

- AOT follow-ups
  - Add a first-class `--emit wasm` convenience path that wraps the current object-emission + `wasm-ld` flow for single-module exports.
  - Decide whether `omni link` is worth adding as a low-level multi-object/native-link orchestration command, or whether that should stay external.
  - Decide whether metadata should remain sidecar-only or gain an optional embedded/exported form for host loaders.
  - Add a few more cross-target artifact smoke tests if we want broader confidence across ELF/COFF/Mach-O/WASM object formats.

- WASM backend (`.wasm` export)
  - Target: `wasm32-unknown-unknown` (pure compute module, no WASI dependency).
  - Two codegen strategies (not mutually exclusive):
    - **LLVM WebAssembly target** (native-hosted, reuses existing codegen):
      - LLVM target initialization: `LLVMInitializeWebAssemblyTarget`, `LLVMInitializeWebAssemblyTargetInfo`,
        `LLVMInitializeWebAssemblyTargetMC`, `LLVMInitializeWebAssemblyAsmPrinter`.
      - Build system: add `webassembly` to the LLVM component link list alongside `native`
        (conditional on a cargo feature flag, e.g. `wasm-backend`).
      - Reuses the existing LLVM IR generation from `orc_backend/` — same `build_*_ir` functions,
        just targeting a different triple and emitting an object instead of JIT-executing.
      - Benefits from the full LLVM O3 pipeline (loop vectorization, inlining, etc.).
      - Only runs where LLVM is available (native host). Cannot run inside WASM itself.
    - **Binaryen codegen backend** (lightweight, WASM-native, embeddable):
      - New codegen crate (e.g. `omni_codegen_binaryen`) that emits Binaryen IR directly
        from `TypedProgram`, bypassing LLVM entirely.
      - Uses the Binaryen C API (`binaryen-c.h`) to construct modules, functions, and expressions.
      - Binaryen's own optimizer handles WASM-specific passes (dead code, coalescing, reordering, etc.).
      - Much smaller dependency than LLVM (~3-5MB as a WASM binary vs 20-30MB+).
      - Key advantage: Binaryen itself compiles to WASM — this enables the in-browser compiler path
        (see below). The same codegen backend works natively and inside the browser.
      - Tradeoff: loses LLVM's general-purpose optimization power; Binaryen is strong on WASM-specific
        transforms but does not do high-level loop vectorization, aggressive inlining heuristics, etc.
      - The Binaryen backend can start as the simpler path (no LLVM cross-target plumbing) and
        serve as the reference WASM codegen, with the LLVM WebAssembly target as an optional
        "optimized" alternative for offline/CLI builds.
  - Shared WASM concerns (apply to both strategies):
    - Memory model:
      - All state, IO buffers, param blocks, and external buffer bindings live in WASM linear memory.
      - The host (JS/Rust/etc.) allocates regions and passes i32 byte offsets to exported functions.
      - The existing pointer+length ABI translates directly: wasm32 pointers are i32 offsets into linear memory.
      - State blob layout (`layout.rs`) is portable — primitive sizes are identical, pointer-width fields
        (if any are introduced) need to use explicit i32 widths rather than `isize`/`usize`.
    - Exported WASM functions:
      - `omni_init(state_ptr)` — initialize state blob at the given offset.
      - `omni_process(state_ptr, frames, ins_ptr, outs_ptr, params_ptr, bufs_ptr, buf_frames_ptr, buf_channels_ptr, buf_samplerates_ptr)`
        — same signature shape as the current ORC `omni_process`, with pointers as i32 memory offsets.
      - `omni_event_N(state_ptr, payload_ptr, payload_len)` — per-event dispatch.
      - `omni_alloc(bytes) -> ptr` / `omni_free(ptr)` — optional bump/arena allocator exported from the
        module so the host can request linear memory regions without linking a full allocator.
      - `omni_state_size() -> i32` — return required state blob size for host-side allocation.
      - `omni_metadata()` — optional: return a pointer to a static descriptor blob with
        input/output/param/buffer/event counts, names, types, and byte sizes.
    - Optimization:
      - LLVM path: `default<O3>` pipeline + optional `wasm-opt` post-pass.
      - Binaryen path: Binaryen's own optimization passes (equivalent to `wasm-opt -O3`).
      - Evaluate WASM SIMD (`simd128`) for vectorizable inner loops (both paths).
    - Constraints and exclusions:
      - No dynamic linking / imported functions — the module is fully self-contained.
      - No file I/O or OS calls — pure deterministic compute kernel.
      - `std/fft` and other stdlib modules that use only arithmetic should work unchanged;
        verify no stdlib path accidentally depends on host intrinsics.
      - Oversampling sinc/filter tables: bake as constant data in the WASM data section
        (same as current approach, just verify emission works for wasm target).
  - AudioWorklet glue (JS/TS runtime):
    - Ship a JS/TS helper module that loads the `.wasm`, allocates linear memory regions for
      state/IO/params/buffers, and bridges `AudioWorkletProcessor.process()` to `omni_process`.
    - The glue layer handles interleaved↔planar conversion if needed (or document that Omni
      uses planar layout matching `AudioWorkletProcessor` conventions).
    - Param changes and event dispatch go through the glue layer via `MessagePort`.
  - Host-side hot-swap (daemon-served live preview):
    - Compilation stays on the host: the daemon runs LLVM or Binaryen natively and emits `.wasm` bytes —
      no JIT inside the WASM sandbox.
    - Reuse the existing daemon recompile-on-save loop; the only change is the output artifact
      (`.wasm` bytes instead of ORC function pointers).
    - Transport: daemon serves `.wasm` bytes to the browser client via WebSocket or HTTP endpoint.
      On source change, daemon recompiles and pushes/notifies the new `.wasm`.
    - Client-side swap protocol:
      - Browser receives new `.wasm` bytes → `WebAssembly.compile()` → `WebAssembly.instantiate()`.
      - New AudioWorklet processor is wired up; old processor is drained/crossfaded or hard-swapped
        (accept a brief glitch, same as current native preview does on recompile).
      - State is reset on swap (matches current native preview behavior on recompile).
      - Param values and buffer bindings are re-applied from the client-side shadow state
        (same pattern as the current daemon preview session rebuild).
    - Decide whether to extend `omni preview play --target wasm32` to spawn a local HTTP server +
      WebSocket bridge, or keep WASM preview as a separate `omni preview web` subcommand.
    - Evaluate whether the VSCode extension's Patch panel can reuse this path
      (webview already runs in a browser-like context; could load the AudioWorklet directly).
  - In-browser compiler (zero-install web playground):
    - Compile the Omni frontend (`omni_frontend`) + semantics (`omni_semantics`) +
      Binaryen codegen backend (`omni_codegen_binaryen`) to `wasm32-unknown-unknown` via
      `cargo build --target wasm32-unknown-unknown`.
    - The frontend and semantics crates are pure Rust with no OS dependencies — should
      cross-compile cleanly. Verify no transitive dependency pulls in `std::fs`/`std::net`/etc.
    - Binaryen itself is compiled to WASM (it supports this) and linked into the codegen crate.
    - Result: a single WASM module (~3-8MB estimated) that takes Omni source text as input
      and returns `.wasm` bytes as output — the full compiler running in the browser.
    - The web playground then: user edits Omni source → compiler WASM produces DSP WASM →
      DSP WASM is loaded into AudioWorklet → audio plays. All client-side, no server.
    - Latency budget: Binaryen codegen is fast enough for interactive use (~10-50ms for typical
      Omni programs); LLVM would be too slow inside WASM.
    - The playground UI can reuse the AudioWorklet glue layer and param/buffer control surface
      from the daemon-served path.
  - CLI integration:
    - `omni compile foo.omni --target wasm32` emits `foo.wasm`.
    - `omni compile foo.omni --target wasm32 --emit js` also emits the AudioWorklet glue module.
    - `omni compile foo.omni --target wasm32 --meta` emits a JSON descriptor alongside the `.wasm`.
    - `--wasm-backend binaryen|llvm` selects the codegen strategy (default: `binaryen`).
  - Testing:
    - Run existing integration-test suite cross-compiled to WASM via a lightweight WASM runtime
      (e.g. `wasmtime` or `wasmer` invoked from Rust tests) to verify numerical equivalence
      with the native ORC JIT path.
    - Add a browser-based integration smoke test for the AudioWorklet hot-swap path
      (headless Chromium or Playwright).
    - Test the in-browser compiler path end-to-end: source → compile-in-WASM → instantiate →
      verify output samples match the native reference.

- C++ backend (`.hpp` export)
  - Add a backend that exports Omni programs to a single-file, self-contained C++ header class.
  - Generate deterministic `init`/`process` methods with no dynamic allocation in the audio callback.
  - Keep generated API compatible with current channel/state/data model for easy host embedding.

## Optimization / Runtime follow-ups

- SIMD strategy
  - Add explicit vector DSL design (or auto-vectorization-oriented lowering passes) beyond current LLVM loop optimizations.
  - Define stable semantics for vector math and scalar/vector interoperability.

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.

- C ABI diagnostics lifecycle
  - Tighten memory ownership model for diagnostic messages and document host-side lifecycle guarantees.
