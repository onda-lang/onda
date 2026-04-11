# Backend TODO

## Backends

- AOT follow-ups
  - Add a first-class `--emit wasm` convenience path that wraps the current object-emission + `wasm-ld` flow for single-module exports.
    Current state:
    wasm object emission already works through `onda compile --emit obj --target wasm32-unknown-unknown`
    or `--target-spec targets/wasm32-unknown-unknown.toml` when LLVM was built with the `WebAssembly`
    target enabled. The repo also includes a working example flow under
    `examples/web/sine_wasm_worklet/` that links the emitted object with `wasm-ld`.
  - Decide whether `onda link` is worth adding as a low-level multi-object/native-link orchestration command, or whether that should stay external.
  - Decide whether metadata should remain sidecar-only or gain an optional embedded/exported form for host loaders.
  - Add a few more cross-target artifact smoke tests if we want broader confidence across ELF/COFF/Mach-O/WASM object formats.

- WASM productization / first-class `.wasm` export
  - Current state:
    - cross-target WebAssembly object emission is already available through the LLVM backend
    - checked-in target spec: `targets/wasm32-unknown-unknown.toml`
    - checked-in example/demo flow: `examples/web/sine_wasm_worklet/`
    - this is currently an object-emission plus external-link step, not a first-class `--emit wasm` CLI product
  - Target: `wasm32-unknown-unknown` (pure compute module, no WASI dependency).
  - Two codegen strategies (not mutually exclusive):
    - **LLVM WebAssembly target** (native-hosted, reuses existing codegen):
      - This path already exists at the object-emission level when LLVM is built with the `WebAssembly` target.
      - Remaining work is to turn that existing capability into a first-class `.wasm` output path and polished host workflow.
      - Reuses the existing LLVM IR generation from `orc_backend/` - same `build_*_ir` functions,
        just targeting a different triple and emitting an object instead of JIT-executing.
      - Benefits from the full LLVM O3 pipeline (loop vectorization, inlining, etc.).
      - Only runs where LLVM is available (native host). Cannot run inside WASM itself.
    - **Binaryen codegen backend** (lightweight, WASM-native, embeddable):
      - New codegen crate (e.g. `onda_codegen_binaryen`) that emits Binaryen IR directly
        from `TypedProgram`, bypassing LLVM entirely.
      - Uses the Binaryen C API (`binaryen-c.h`) to construct modules, functions, and expressions.
      - Binaryen's own optimizer handles WASM-specific passes (dead code, coalescing, reordering, etc.).
      - Much smaller dependency than LLVM (~3-5MB as a WASM binary vs 20-30MB+).
      - Key advantage: Binaryen itself compiles to WASM - this enables the in-browser compiler path
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
      - State blob layout (`layout.rs`) is portable - primitive sizes are identical, pointer-width fields
        (if any are introduced) need to use explicit i32 widths rather than `isize`/`usize`.
    - Exported WASM functions:
      - `onda_init(state_ptr)` - initialize state blob at the given offset.
      - `onda_process(state_ptr, frames, ins_ptr, outs_ptr, params_ptr, bufs_ptr, buf_frames_ptr, buf_channels_ptr, buf_samplerates_ptr)`
        - same signature shape as the current ORC `onda_process`, with pointers as i32 memory offsets.
      - `onda_event_N(state_ptr, payload_ptr, payload_len)` - per-event dispatch.
      - `onda_alloc(bytes) -> ptr` / `onda_free(ptr)` - optional bump/arena allocator exported from the
        module so the host can request linear memory regions without linking a full allocator.
      - `onda_state_size() -> i32` - return required state blob size for host-side allocation.
      - `onda_metadata()` - optional: return a pointer to a static descriptor blob with
        input/output/param/buffer/event counts, names, types, and byte sizes.
    - Optimization:
      - LLVM path: `default<O3>` pipeline + optional `wasm-opt` post-pass.
      - Binaryen path: Binaryen's own optimization passes (equivalent to `wasm-opt -O3`).
      - Evaluate WASM SIMD (`simd128`) for vectorizable inner loops (both paths).
    - Constraints and exclusions:
      - No dynamic linking / imported functions - the module is fully self-contained.
      - No file I/O or OS calls - pure deterministic compute kernel.
      - `std/fft` and other stdlib modules that use only arithmetic should work unchanged;
        verify no stdlib path accidentally depends on host intrinsics.
      - Oversampling sinc/filter tables: bake as constant data in the WASM data section
        (same as current approach, just verify emission works for wasm target).
  - AudioWorklet glue (JS/TS runtime):
    - Ship a JS/TS helper module that loads the `.wasm`, allocates linear memory regions for
      state/IO/params/buffers, and bridges `AudioWorkletProcessor.process()` to `onda_process`.
    - The glue layer handles interleaved<->planar conversion if needed (or document that Onda
      uses planar layout matching `AudioWorkletProcessor` conventions).
    - Param changes and event dispatch go through the glue layer via `MessagePort`.
  - Host-side hot-swap (daemon-served live preview):
    - Compilation stays on the host: the daemon runs LLVM or Binaryen natively and emits `.wasm` bytes -
      no JIT inside the WASM sandbox.
    - Reuse the existing daemon recompile-on-save loop; the only change is the output artifact
      (`.wasm` bytes instead of ORC function pointers).
    - Transport: daemon serves `.wasm` bytes to the browser client via WebSocket or HTTP endpoint.
      On source change, daemon recompiles and pushes/notifies the new `.wasm`.
    - Client-side swap protocol:
      - Browser receives new `.wasm` bytes -> `WebAssembly.compile()` -> `WebAssembly.instantiate()`.
      - New AudioWorklet processor is wired up; old processor is drained/crossfaded or hard-swapped
        (accept a brief glitch, same as current native preview does on recompile).
      - State is reset on swap (matches current native preview behavior on recompile).
      - Param values and buffer bindings are re-applied from the client-side shadow state
        (same pattern as the current daemon preview session rebuild).
    - Decide whether to extend `onda preview play --target wasm32` to spawn a local HTTP server +
      WebSocket bridge, or keep WASM preview as a separate `onda preview web` subcommand.
    - Evaluate whether the VSCode extension's Patch panel can reuse this path
      (webview already runs in a browser-like context; could load the AudioWorklet directly).
  - In-browser compiler (zero-install web playground):
    - Compile the Onda frontend (`onda_frontend`) + semantics (`onda_semantics`) +
      Binaryen codegen backend (`onda_codegen_binaryen`) to `wasm32-unknown-unknown` via
      `cargo build --target wasm32-unknown-unknown`.
    - The frontend and semantics crates are pure Rust with no OS dependencies - should
      cross-compile cleanly. Verify no transitive dependency pulls in `std::fs`/`std::net`/etc.
    - Binaryen itself is compiled to WASM (it supports this) and linked into the codegen crate.
    - Result: a single WASM module (~3-8MB estimated) that takes Onda source text as input
      and returns `.wasm` bytes as output - the full compiler running in the browser.
    - The web playground then: user edits Onda source -> compiler WASM produces DSP WASM ->
      DSP WASM is loaded into AudioWorklet -> audio plays. All client-side, no server.
    - Latency budget: Binaryen codegen is fast enough for interactive use (~10-50ms for typical
      Onda programs); LLVM would be too slow inside WASM.
    - The playground UI can reuse the AudioWorklet glue layer and param/buffer control surface
      from the daemon-served path.
  - CLI integration:
    - `onda compile foo.onda --target wasm32` emits `foo.wasm`.
    - `onda compile foo.onda --target wasm32 --emit js` also emits the AudioWorklet glue module.
    - `onda compile foo.onda --target wasm32 --meta` emits a JSON descriptor alongside the `.wasm`.
    - `--wasm-backend binaryen|llvm` selects the codegen strategy (default: `binaryen`).
  - Testing:
    - Run existing integration-test suite cross-compiled to WASM via a lightweight WASM runtime
      (e.g. `wasmtime` or `wasmer` invoked from Rust tests) to verify numerical equivalence
      with the native ORC JIT path.
    - Add a browser-based integration smoke test for the AudioWorklet hot-swap path
      (headless Chromium or Playwright).
    - Test the in-browser compiler path end-to-end: source -> compile-in-WASM -> instantiate ->
      verify output samples match the native reference.

- C++ backend (`.hpp` export)
  - Add a backend that exports Onda programs to a single-file, self-contained C++ header class.
  - Generate deterministic `init`/`process` methods with no dynamic allocation in the audio callback.
  - Keep generated API compatible with current channel/state/data model for easy host embedding.

