# Backend TODO

## Backends

- AOT follow-ups
  - Add a first-class `--emit wasm` convenience path that wraps the current object-emission + `wasm-ld` flow for single-module exports.
    Current state:
    wasm object emission already works through `onda compile --emit obj --target wasm32-unknown-unknown`
    or `--target-spec targets/wasm32-unknown-unknown.toml` when LLVM was built with the `WebAssembly`
    target enabled. That native-hosted path is now separate from the browser Binaryen demo under
    `examples/web/sine_wasm_worklet/`.
  - Decide whether `onda link` is worth adding as a low-level multi-object/native-link orchestration command, or whether that should stay external.
  - Decide whether metadata should remain sidecar-only or gain an optional embedded/exported form for host loaders.
  - Add a few more cross-target artifact smoke tests if we want broader confidence across ELF/COFF/Mach-O/WASM object formats.

- WASM productization / first-class `.wasm` export
  - Current state:
    - cross-target WebAssembly object emission is already available through the LLVM backend
    - checked-in target spec: `targets/wasm32-unknown-unknown.toml`
    - checked-in LLVM object/link support remains available for native-hosted optimized builds
    - `onda compile --emit mir-messagepack` plus `packages/onda_binaryen_web` now provides an executable browser-side MIR-to-Wasm slice; JSON remains available for inspection
    - `examples/web/sine_wasm_worklet/` compiles edited source to MIR and then DSP Wasm in the page
      before starting the AudioWorklet
  - Target: `wasm32-unknown-unknown` (pure compute module, no WASI dependency).
  - Two codegen strategies (not mutually exclusive):
    - **LLVM WebAssembly target** (native-hosted, reuses existing codegen):
      - This path already exists at the object-emission level when LLVM is built with the `WebAssembly` target.
      - Remaining work is to turn that existing capability into a first-class `.wasm` output path and polished host workflow.
      - Reuses the production schema-5 MIR-native lowering in `orc_backend/mir_native.rs`, selecting a
        WebAssembly target and object emission instead of host ORC execution. The old frontend
        `build_*_ir` path is only a differential test oracle.
      - Benefits from the full LLVM O3 pipeline (loop vectorization, inlining, etc.).
      - Only runs where LLVM is available (native host). Cannot run inside WASM itself.
    - **Binaryen.js MIR backend** (browser-native schema-5 implementation):
      - Onda owns a versioned serialized MIR boundary; Binaryen is not statically linked into the Rust compiler Wasm.
      - The Rust frontend/semantics stage and `packages/onda_binaryen_web` use schema 5 with the
        official Binaryen.js browser library.
      - This separation keeps Onda language semantics in Rust/MIR and WebAssembly construction in the environment where Binaryen already has a supported API.
      - The executable slice covers scalar state/params/audio; fixed arrays in addressable state and
        local storage; primitive slices; scalarized tuple parameters and multi-value results;
        buffer-reference parameters; flattened data-struct state/method references;
        structure-of-slices data-struct arrays, constructor lists/broadcasts, and retained element
        aliases; structured control flow; native numeric intrinsics; constant data; packed
        scalar/fixed-array/dynamic-slice events; explicit control mirrors; checked `make_slice`;
        array/slice reference windows; function attributes; and interleaved mono/static/dynamic
        external buffers.
      - Generated process/event wrappers, payload contracts, and packed snapshot layout match the
        native MIR ABI. Physical state offsets remain backend-selected and are described by emitted
        metadata. Source-to-Wasm integration tests consume current compiler output.
      - Recursive processor arrays are supported. The native/Wasm parity suite covers
        parameters/tuples/state, packed snapshots and restore, events, segmented and zero-frame
        scheduling, numeric edge semantics, buffers, processor and top-level oversampling, a dual
        oscillator, the saw/filter/saturator example, and processor-array initialization/indexed
        dispatch/block updates.
      - Binaryen is strong on Wasm-specific transforms but does not replace LLVM's high-level optimization pipeline. LLVM remains the optional native-hosted optimized route.
  - Shared WASM concerns (apply to both strategies):
    - Memory model:
      - All state, IO buffers, param blocks, and external buffer bindings live in WASM linear memory.
      - The host (JS/Rust/etc.) allocates regions and passes i32 byte offsets to exported functions.
      - The existing pointer+length ABI translates directly: wasm32 pointers are i32 offsets into linear memory.
      - Logical MIR storage types and the packed snapshot contract are portable. Physical state
        layout is backend-selected; emitted metadata supplies the offsets and sizes required by the
        host. Pointer-width fields, if introduced, must use explicit portable widths.
    - Exported WASM functions:
      - `onda_init(params_ptr, state_ptr)` - bind parameter/state blobs, clear physical state, and
        run the MIR init entry point.
      - `onda_process(ins_ptr, outs_ptr, start_frame, frames, flags, params_ptr, state_ptr, bufs_ptr, buf_frames_ptr, buf_channels_ptr, buf_samplerates_ptr)`
        - native-compatible 11-argument segmented ABI, with pointers as i32 linear-memory offsets.
      - `onda_event_N(payload_ptr, params_ptr, state_ptr, bufs_ptr, buf_frames_ptr, buf_channels_ptr, buf_samplerates_ptr)` - per-event dispatch.
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
      - No dynamic linking. Most generated modules are self-contained; direct transcendental MIR intrinsics may import deterministic `onda_math` host functions because core WebAssembly has no corresponding instructions. Scalar `fma` uses the versioned bit-level `onda_exact_math_v1` import: the bundled BigInt implementation rounds the exact product-plus-add once for f32/f64 and preserves IEEE exceptional-value and signed-zero semantics, at a documented import/allocation cost.
      - No file I/O or OS calls - pure deterministic compute kernel.
      - `std/fft` and other stdlib modules that use only arithmetic should work unchanged;
        verify no stdlib path accidentally depends on host intrinsics.
      - Oversampling sinc/filter tables: bake as constant data in the WASM data section
        (same as current approach, just verify emission works for wasm target).
  - AudioWorklet glue (JS/TS runtime):
    - Productize the reference JavaScript worklet as a reusable JS/TS helper that loads the `.wasm`,
      allocates linear-memory regions for state/IO/params/buffers, and bridges
      `AudioWorkletProcessor.process()` to `onda_process`.
    - The reference worklet keeps a persistent compile-block cursor and splits arbitrary Web Audio
      render quanta into legal segments under the scheduling contract introduced by schema 4 and
      retained by schema 5, preserving full-block storage and BEGIN/END hooks.
    - The glue layer handles interleaved<->planar conversion if needed (or document that Onda
      uses planar layout matching `AudioWorkletProcessor` conventions).
    - Param changes, reset, event dispatch, control-output reads, and buffer reads go through the
      glue layer via `MessagePort`; the playground UI currently surfaces params/events/reset but not
      control-output or external-buffer views.
  - Host-side hot-swap (daemon-served live run):
    - Compilation stays on the host: the daemon runs LLVM or Binaryen natively and emits `.wasm` bytes -
      no JIT inside the WASM sandbox.
    - Reuse the existing daemon recompile-on-save loop; the only change is the output artifact
      (`.wasm` bytes instead of ORC function pointers).
    - Transport: daemon serves `.wasm` bytes to the browser client via WebSocket or HTTP endpoint.
      On source change, daemon recompiles and pushes/notifies the new `.wasm`.
    - Client-side swap protocol:
      - Browser receives new `.wasm` bytes -> `WebAssembly.compile()` -> `WebAssembly.instantiate()`.
      - New AudioWorklet processor is wired up; old processor is drained/crossfaded or hard-swapped
        (accept a brief glitch, same as current native run does on recompile).
      - State is reset on swap (matches current native run behavior on recompile).
      - Param values and buffer bindings are re-applied from the client-side shadow state
        (same pattern as the current daemon run session rebuild).
    - Decide whether to extend `onda run play --target wasm32` to spawn a local HTTP server +
      WebSocket bridge, or keep WASM run as a separate `onda run web` subcommand.
    - Evaluate whether the VSCode extension's Run panel can reuse this path
      (webview already runs in a browser-like context; could load the AudioWorklet directly).
  - In-browser compiler (zero-install web playground):
    - Compile the Onda frontend (`onda_frontend`) + semantics (`onda_semantics`) + MIR serialization to `wasm32-unknown-unknown`.
    - `onda_compiler_web` now provides that filesystem-free Wasm front half and embeds the standard
      library; its native tests also compile the checked-in example corpus through the in-memory
      project path.
    - Keep Binaryen.js as a separate browser stage: source -> Rust compiler Wasm -> versioned MIR -> Binaryen.js -> DSP Wasm.
    - `onda_compiler_web` exposes single-source and virtual multi-file source-to-MIR APIs through
      `wasm-bindgen`. The checked-in playground demonstrates source editing, structured diagnostics,
      MIR-to-Binaryen compilation, generated controls, reset, and AudioWorklet execution end to end.
    - Measure compile latency and asset transfer size in the real playground before setting budgets; the pinned Binaryen ESM distribution is large uncompressed and must be cached/compressed by a production static host.
    - Extend the current single-file playground UI to the already-supported virtual multi-file API,
      buffer loading/inspection, control-output display, microphone/input routing, export/share
      flows, worker-based compilation, and state-aware hot swap.
  - CLI integration (proposed, not implemented):
    - `onda compile foo.onda --target wasm32` emits `foo.wasm`.
    - `onda compile foo.onda --target wasm32 --emit js` also emits the AudioWorklet glue module.
    - `onda compile foo.onda --target wasm32 --meta` emits a JSON descriptor alongside the `.wasm`.
    - `--wasm-backend binaryen|llvm` selects the codegen strategy (default: `binaryen`).
  - Testing:
    - Keep `npm test` as the schema-5 backend/AudioWorklet fixture gate.
    - Keep `npm run test:onda` as the source-driven integration, LLVM/MIR-Binaryen parity, and exact
      FMA oracle gate; it requires the native Rust/LLVM Onda build. Expand its real-program corpus
      as features land.
    - Keep `npm run bench` as the reproducible development comparison described in
      [`docs/BACKEND_BENCHMARKS.md`](../BACKEND_BENCHMARKS.md); add browser and architecture runs
      before setting product budgets.
    - Add a browser-based integration smoke test for source edit -> compiler Wasm -> Binaryen ->
      AudioWorklet playback and future hot swap (headless Chromium/Playwright where audio support is
      reliable).
    - Add cross-browser compatibility coverage for Chromium, Firefox, and Safari AudioWorklet
      behavior.

- Browser playground product
  - Current state: the checked-in zero-install runtime path is:
    source editor -> diagnostics -> compile -> AudioWorklet playback -> param/event controls.
  - The current page compiles entirely in-browser, uses metadata-generated params/events, persists
    edited source locally, and requires only a static HTTP host after `wasm-pack`/npm staging.
  - Remaining product requirements:
    - production asset compression/caching and measured compile feedback on typical stdlib/graph patches
    - virtual multi-file editing and shareable URLs
    - buffer loading/inspection and export/download of source, metadata, and compiled Wasm
    - control-output display driven by generated metadata
    - microphone/host-input routing; current AudioWorklet inputs are unconnected and therefore silent
    - seamless/crossfaded hot swap with a clear state migration policy
    - compiler/Binaryen workerization so larger programs do not block the editor main thread
    - secure-context deployment guidance and handling when a browser does not honor the requested
      `AudioContext` sample rate
    - Playwright-style source-edit/compile/AudioWorklet smoke coverage where browser automation can
      exercise audio reliably
    - browser compatibility and mobile/autoplay passes

- C++ backend (`.hpp` export)
  - Add a backend that exports Onda programs to a single-file, self-contained C++ header class.
  - Generate deterministic `init`/`process` methods with no dynamic allocation in the audio callback.
  - Keep generated API compatible with current channel/state/data model for easy host embedding.
