# Changelog

All notable changes to Onda are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Onda follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog starts with the 0.5.0
release; earlier releases are available on the
[GitHub releases page](https://github.com/onda-lang/onda/releases).

## [0.6.0]

### Added

- Extended top-level parameter ranges into host-control domains with linear and logarithmic scales,
  SuperCollider-style curves, display units, and discrete steps. Positional and named domain fields
  are supported, and the LSP provides validation, completion, hover, and semantic-token coverage for
  the new syntax.
- Added shared plain/normalized parameter conversion, clamping, and snapping across the Rust
  runtime, C API and processor ABI header, processor descriptors, JavaScript packages, Web Audio
  adapter, and native and browser run views.
- Added source manifests to filesystem and in-memory compilation. Successful and failed compilations
  report their resolved non-stdlib sources and unresolved candidates, allowing hosts to maintain
  complete project-wide watches while source trees are temporarily invalid.
- Added `Onda::Static` and `Onda::Shared` CMake package targets for source checkouts and extracted
  release SDKs, including their platform-specific link requirements and static-library symbol
  hiding.
- Added support for safely sharing compiled JIT programs between threads and moving exclusively
  owned runtime instances between threads, with explicit custom-allocator lifetime and concurrency
  contracts.
- Added an LSP snippet that declares the complete VST3 MIDI event surface.

### Changed

- The standalone run hosts now watch the entry file and every transitive user import and include,
  including unresolved paths that may subsequently be created. Reloads use complete source
  snapshots and ignore stale subprocess messages.
- Ranged top-level parameters are clamped once at each `init`, event, and logical process-block
  boundary. Ranged processor parameters are clamped on every write. Floating NaNs map to the range
  minimum and infinities clamp to an endpoint.
- Native processor `init`, event, and process entry points now return an execution status. Generated
  bounds, division, and conversion failures return `RUNTIME_SAFETY_FAILURE` instead of invoking
  `llvm.trap`; native and web hosts stop processing a failed instance until it is reset.
- Reset now restores parameter defaults as well as reinitializing processor state and clearing
  transient run-view state.
- `@onda-lang/wasm-compiler` source and project compilation now return the artifact together with
  the ordered contributing source paths. Compilation errors also expose resolved and unresolved
  source paths.
- Advanced the MIR schema from version 1 to 3 and the processor artifact and ABI formats from
  version 1 to 2 for parameter-control metadata and fallible execution entry points.
- Renamed the packaged Windows application launcher from `Onda Run.exe` to `Onda.exe`.
- Reserved identifiers beginning with `__onda_` for compiler-generated symbols.

### Fixed

- Fixed dynamically indexed ranged input and parameter reads bypassing their entry-point clamps.
- Fixed range lowering when user declarations shadowed compiler-generated clamp names.
- Fixed host-control validation for named bounds in arbitrary order, exact integer and floating-point
  step grids, wide logarithmic ranges, and curved controls.
- Fixed LSP completion parsing around incomplete parameter domains and internal completion sentinels.
- Fixed project reloads after dependency errors, unresolved dependency creation, rapid edits during
  compilation, and stale child-process responses.

### Migration notes

- Update `@onda-lang/wasm-compiler` callers to read `artifact` and `sourceFiles` from the compilation
  result instead of treating the result itself as the artifact.
- Pass an `onda_source_manifest_t**` before the diagnostic argument to `onda_compile_file`, or pass
  null when the source manifest is not needed. Destroy returned manifests with
  `onda_source_manifest_destroy`.
- Rebuild serialized MIR and processor artifacts. Consumers must accept MIR schema version 3 and
  processor artifact/ABI version 2; version-1 artifacts are not compatible with this release.
- Update raw native processor hosts for `onda_init`, `onda_process`, and `onda_event_N` returning
  `i32`. Zero is success; after a nonzero execution result, discard the instance or reset its state
  and call `onda_init` again.
- Rename user identifiers beginning with `__onda_`.
- Hosts that implement reset themselves should restore parameter defaults in addition to
  reinitializing processor state.

## [0.5.4]

### Added

- Added native file pickers and support for launching `onda run` without a
  source path.
- Added application icons and desktop launchers for Linux, macOS, and Windows.
- Added settings controls for audio block size and persisted view preferences.
- Added structured array values to runtime events and their native and web
  visualizations.
- Added playground editor tests covering indentation and selection behavior.

### Changed

- Reworked the egui and webview applications with improved file lifecycle,
  playback controls, parameter editing, buffer views, scopes, and event
  displays.
- Unified browser and native run-view behavior, including settings and shared
  event rendering.
- Improved the web playground's editor, live-run lifecycle, sharing flow, and
  responsive layout.
- Updated release packaging with platform-specific launchers, installers,
  metadata, and asset validation.
- Refreshed the website, documentation, and sample-player example for the new
  run workflow.

### Fixed

- Cleaned up playback restarts and buffer metadata refreshes when recompiling
  or switching source files.
- Improved application shutdown and unload handling across desktop and browser
  views.

## [0.5.3]

### Added

- Added `--buffer name=path` to `onda run play` and `onda run render` for binding declared buffers
  to WAV files from the command line.

### Fixed

- Fixed LLVM/Binaryen parity verification after native run hosts stopped creating implicit buffer
  bindings; the verifier now explicitly binds identical zero-filled buffers for both backends.

## [0.5.2]

### Added

- Added LSP member completion for built-in buffer methods and `std::lookup` buffer extensions.

### Changed

- Native and browser run hosts now require every declared buffer to be bound to a valid WAV file
  before processing starts. Buffer loads and clears update processing state only after the host
  accepts them, and processing resumes automatically once all requested bindings are ready.

### Fixed

- Fixed browser file-picker cleanup after selecting or cancelling a buffer file.
- Fixed the webview scope panel remaining visible while processing is stopped.
- Improved native run-host status and error reporting for rejected control commands and unexpected
  audio-process exits.

## [0.5.1]

### Fixed

- Made zero-sample-rate runtime buffer bindings explicitly unbind their slots, and required every
  bound buffer to have a non-null pointer, positive dimensions, and a finite positive sample rate.
  This also removes redundant empty-range checks from generated external-buffer accesses.

## [0.5.0]

### Added

- Added `onda_mir`, a typed, structured, backend-neutral intermediate representation for complete
  executable Onda programs. MIR includes explicit storage and resource tables, source locations,
  structured control flow, definite-assignment and bounds validation, effect and integer-range
  analyses, canonical formatting, and backend-neutral optimization passes.
- Added versioned MIR JSON and MessagePack transports. `onda compile` can emit readable MIR with
  `--emit mir`, versioned JSON with `--emit mir-json`, and compact MessagePack with
  `--emit mir-messagepack`.
- Added a Binaryen-based MIR backend that emits complete WebAssembly processor modules in browsers
  and Node.js without LLVM or a Wasm linker. It supports Onda's current language surface, events,
  external buffers, slices, processor arrays, oversampling, segmented processing, snapshots, SIMD
  slice operations, and strict floating-point behavior.
- Added an embedded, reproducible WebAssembly math kernel for transcendental functions and correctly
  rounded fused multiply-add. Generated modules link only the helpers they use and have no host math
  imports.
- Added `onda_compiler_web`, a filesystem-free Rust/Wasm frontend that compiles source strings and
  in-memory projects, with the standard library embedded in the compiler.
- Added four publishable npm packages:
  - `@onda-lang/wasm-compiler` provides the source-to-WebAssembly product API, browser-worker mode,
    structured diagnostics, browser LSP support, artifact helpers, and the `onda-wasm` CLI.
  - `@onda-lang/binaryen-web` provides the low-level MIR-to-WebAssembly backend.
  - `@onda-lang/processor-abi` validates processor descriptors, module exports, snapshots, and
    integrity-associated artifact pairs without loading a compiler.
  - `@onda-lang/webaudio` hosts compiled processor artifacts in an `AudioWorklet` and exposes typed
    parameter, event, buffer, control-output, reset, and snapshot operations.
- Added the versioned generic Onda processor ABI and descriptor schemas for native objects and
  WebAssembly modules. The ABI defines portable interface metadata, packed event payloads, segmented
  processing flags, persistent snapshots, and host-owned storage.
- Added `include/onda_processor_abi.h` and a complete native example that links and calls an emitted
  processor object without linking `libonda` or embedding the compiler.
- Added a browser playground with compilation in a worker, language-server features, example
  projects, microphone and buffer support, shareable source links, and reusable run views.
- Added a compiler-free, precompiled WebAssembly sample-player example using the generic artifact
  format and Web Audio adapter.
- Added differential LLVM/Binaryen rendering tests, strict numeric and FMA oracles, a full source
  corpus compiler test, reusable package-consumer tests, and backend benchmarks.
- Added a dedicated `onda_lsp` crate so the native CLI and browser compiler use the same
  transport-neutral language server.
- Added a project website, getting-started guide, example cookbook, compiler architecture guide,
  MIR reference, processor ABI reference, and automated website/playground publishing.
- Added synchronized Cargo/npm/wire-format version tooling and release automation for native
  archives, checksums, portable npm tarballs, and npm trusted publishing.

### Changed

- Rebuilt the LLVM JIT and AOT backend as a consumer of validated, optimized MIR. LLVM and Binaryen
  now share one semantic lowering pipeline rather than independently recovering language behavior
  from the typed frontend program.
- Made backend entry points consume `ValidatedProgram` or `OptimizedProgram` capabilities. Ordinary
  MIR deserialization rejects unchecked accesses; the trusted producer boundary is explicit and
  retains its proof through optimization and serialization.
- Added backend-neutral constant propagation, algebraic simplification, branch and dead-code
  cleanup, common-subexpression elimination, scalar state promotion, and redundant-zero-store
  elimination before target lowering.
- Standardized strict numerical semantics across LLVM and WebAssembly, including wrapping integer
  arithmetic, masked shifts, saturating float-to-int conversion, ordered comparisons, signed-zero
  `min`/`max`, division overflow, non-finite constants, and fused multiply-add. Relaxed floating-point
  rewrites remain opt-in through `fast_math`.
- Changed numeric literals and pure compile-time numeric expressions to remain contextual until a
  concrete `f32`, `f64`, `i32`, or `i64` boundary is known. Runtime operations execute directly at
  the selected width instead of silently using wider intermediates.
- Runtime value-returning functions must now return on every reachable path, and runtime call graphs
  must be acyclic so realtime work remains statically bounded.
- Expanded the reserved-keyword set to cover control-flow, module, declaration, modifier, and boolean
  syntax. In particular, `in` is reserved for `for` ranges and can no longer be used as an identifier.
- Reworked processor snapshots into a deterministic, packed persistent-state format shared by native
  and WebAssembly targets. Compiler scratch storage and control-output mirrors are omitted and reset
  after restoration.
- Reworked process entry points around explicit `(start_frame, frames, flags)` segments. Hosts can
  submit arbitrary render-quantum sizes, including zero-frame boundary calls, without changing Onda's
  compile-block scheduling semantics.
- Tightened host binding validation for element alignment, non-overlap, required buffers,
  sample-rate compatibility, event payload capacity, and processor descriptor/module consistency.
- Moved reusable formatting and language-server behavior out of the CLI and into `onda_lsp`.
- Reorganized examples into foundations, processors and graphs, standard library, buffers, FFT and
  convolution, larger patches, native embedding, and web integration groups.
- Updated `std::math::clamp` and `std::math::lerp` to use explicit generic scalar signatures.
- Enabled one-time x86 FTZ/DAZ configuration through the backend-independent realtime support crate.

### Removed

- Removed the legacy direct semantics-to-LLVM lowering pipeline and its duplicated backend-specific
  language lowering, storage modeling, scheduling, and specialization code.
- Removed the raw complete-program MIR compatibility API. Production frontend consumers now receive
  an `OptimizedProgram` that preserves validation and optimization guarantees.
- Removed `std/export_math`; WebAssembly targets now implement the ordinary Onda math intrinsics with
  the embedded strict math kernel.
- Removed the original sine-only WebAssembly worklet demo in favor of the reusable compiler,
  processor ABI, Web Audio adapter, playground, and precompiled sample-player examples.

### Fixed

- Fixed cross-backend differences around signed zero, NaNs, integer overflow and division, casts,
  comparisons, strict FMA, transcendental functions, snapshots, events, slices, external buffers,
  processor arrays, and oversampling.
- Fixed block-boundary scheduling when Web Audio render quanta are smaller or larger than the Onda
  compile block.
- Fixed browser playground run controls, ownership cleanup, compile caching, tab ordering, shared
  links, microphone teardown, and native webview integration.
- Fixed in-memory project path confinement so browser imports and includes cannot escape the virtual
  project root.

### Migration notes

- Rust compiler integrations should analyze source, call
  `onda_semantics::lower_program_to_optimized_mir`, and pass the resulting `OptimizedProgram` to a
  MIR backend. Direct typed-program-to-LLVM APIs no longer exist.
- Native object hosts should use `include/onda_processor_abi.h` and the emitted processor descriptor.
  Hosts embedding the compiler continue to use `include/onda.h`, but snapshot sizes and state offsets
  now describe the packed persistent snapshot rather than the complete runtime allocation.
- Browser and Node.js applications starting from Onda source should use
  `@onda-lang/wasm-compiler`. Use `@onda-lang/binaryen-web` only when the application already owns
  trusted, current-schema Onda MIR.
- Replace imports of `std/export_math` with ordinary math intrinsics or `std/math` helpers.
- Rename identifiers that now collide with reserved keywords, especially `in`.
- Update scripts and documentation that refer to the old flat `examples/` paths.

[0.6.0]: https://github.com/onda-lang/onda/compare/0.5.4...HEAD
[0.5.4]: https://github.com/onda-lang/onda/compare/0.5.3...0.5.4
[0.5.3]: https://github.com/onda-lang/onda/compare/0.5.2...0.5.3
[0.5.2]: https://github.com/onda-lang/onda/compare/0.5.1...0.5.2
[0.5.1]: https://github.com/onda-lang/onda/compare/0.5.0...0.5.1
[0.5.0]: https://github.com/onda-lang/onda/compare/0.4.4...0.5.0
