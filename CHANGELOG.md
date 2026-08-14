# Changelog

All notable changes to Onda are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Onda follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). This changelog starts with the 0.5.0
release; earlier releases are available on the
[GitHub releases page](https://github.com/onda-lang/onda/releases).

## [0.7.3]

### Added

- Runtime `def` parameters without type annotations can now specialize to each call site's scalar
  type, including `bool`. Unsized processor-array parameters likewise specialize to the supplied
  array capacity.
- Locals created in every continuing branch of an `if` are now available afterward. Compatible
  numeric types are widened automatically, while arrays and other aggregates must retain one
  compatible shape.
- Added `std::osc::KSine<T>`, a block-rate sine oscillator with frequency, amplitude, phase offset,
  and phase-reset controls.
- Added `set_offset` events to `std::convolution` block and zero-latency convolvers for
  deterministic scheduling when required.

### Changed

- Reworked `std::convolution` to spread long-impulse FFT work across each hop, use progressively
  larger zero-latency stages, reduce spectrum storage, and make the direct convolution loop use
  contiguous history reads.
- Refreshed the bundled patches to use boolean controls, block-rate modulation where appropriate,
  and the improved type inference, and clarified how browser users open complete `.ondaproject`
  workspaces from ZIP files.

### Fixed

- Fixed generic calls and overloads losing argument types through reassignments, control-flow
  joins, aliases, tuple and struct fields, array and buffer views, omitted defaults, and nested
  calls. Fixed-size array contracts now retain and validate their element type and length.
- Fixed negative literals and unary negation to preserve `i32`, `i64`, `f32`, and `f64` types and
  precision, including inside generic code.
- Fixed long zero-latency convolution impulses becoming misaligned across stage boundaries or after
  reset.
- Fixed browser builds attempting native filesystem safety checks for in-memory module paths.
- Fixed parameter-knob indicators rendering and rotating inconsistently in the shared run view.
- Fixed generated standard-library documentation exposing internal namespaces.

## [0.7.2]

### Added

- Added separate `plugin_midi_*` and `plugin_host_*` LSP completion families for the canonical
  plugin MIDI and DAW host-context event declarations.
- Added an implementation-status and product-contract guide for the separate live-linked OndaSynth
  and OndaFX VST3 plugins.
- Made `onda_compile_file` the generic native filesystem entry point for `.onda`, `.on`, and
  `.ondaproject`; project inputs resolve their entries internally and retain inline and file-backed
  buffers as program defaults, so C hosts no longer need to materialize project bytes themselves.
  Added `onda_source_manifest_watch_count` and `onda_source_manifest_watch_path` for a deduplicated
  watch projection returned on success and failure, covering the selected input, resolved and
  unresolved sources, project manifest, entry, and file-backed assets.
- Added `std/reverb` with a stereo Schroeder reverb and `std/pitch_shift` with a dual-window pitch
  shifter, plus a resonator filter, reusable decay-envelope coefficient and trigger event,
  oscillator phase reset events, and a sine phase-offset parameter.
- Added direct real, imaginary, power, magnitude, phase, and packed-spectrum access to
  `std::fft::RealFFT`, including bulk packed-spectrum output.

### Changed

- Reworked the checked-in examples into outcome-oriented `basic`, `buffers`, `effects`, `feedback`,
  `instruments`, `soundscapes`, and `spectral` collections, with self-playing musical patches and
  reusable processors. Added self-contained Wavetable Garden, Score-driven Resonator, and Embedded
  Room `.ondaproject` showcases, and refreshed the website, playground, and AOT sample player for
  the new corpus.
- Native run hosts now share one controller-owned filesystem watcher, perform path-targeted source
  validation, reuse parser-captured source bytes, and avoid rereading unchanged sources or hashing
  unrelated project assets. The egui and webview frontends poll at 20 Hz only while loaded and sleep
  while unloaded; egui also reapplies its theme only when it changes.
- Filesystem project compilation now analyzes and generates code before decoding external assets,
  directly owns decoded program defaults, and avoids constructing or hashing a temporary portable
  project image.
- Native filesystem-backed entries, imports, includes, project manifests, project entries, assets,
  run inputs, and LSP file URIs now reject symbolic-link traversal. Virtual sources and immutable
  project images remain unaffected.
- LSP invalidation is now dependency-aware and preserves unrelated parse, completion, and semantic
  token caches while remaining driven by editor and watched-file notifications.
- Real FFT analysis and synthesis now use a half-size complex transform, avoid redundant packed
  spectrum storage, and suppress incomplete initial overlap-add output.
- Native benchmark JSON now reports cold-block maximum latency and steady-state block median, p99,
  and maximum latency, and supplies deterministic silence to scalar `f32` inputs.

### Fixed

- Fixed native live reload missing changes after watcher creation or partial-coverage failures,
  during compilation or watcher recreation, after temporarily invalid parses, while project entries
  or assets are missing, and after atomic file replacement on macOS.
- Fixed LSP dependency edits leaving affected diagnostics stale, and replayed diagnostic-worker
  results from their exact source snapshots so thread-local source-location identities cannot cross
  threads.
- Fixed generic-call monomorphization inside index expressions, slice coordinates, and array
  constructor sizes and initializers.
- Fixed nested processor lowering for generated helper functions, nested method paths, proc-array
  state lengths, buffer methods, and events that update their own pinned parameters without exposing
  those parameters to external user code.
- Fixed identifiers beginning with `const`, such as `constructed`, being parsed as constant
  declarations.

### Migration notes

- Replace symbolic links in native filesystem-backed source and project paths with regular files
  and directories, or use virtual sources or an immutable project image when indirection is needed.
- Replace direct `RealFFT.packed` field access with `packed_value`, the direct spectrum accessors, or
  `store_real_packed`; FFT sizes must now be powers of two greater than one.
- Update scripts and links that reference the previous checked-in example directories to the new
  `basic`, `buffers`, `effects`, `feedback`, `instruments`, `projects`, `soundscapes`, and `spectral`
  paths.

## [0.7.1]

### Added

- Added `onda_diag_dispose`; every non-null string returned in an `onda_diag_t` is now owned by
  Onda and remains valid until the diagnostic is disposed.

### Fixed

- Corrected LLVM ORC ownership transfers during native JIT compilation, including cleanup on
  optimization, verification, builder, and module-addition error paths.
- Released diagnostic strings when callers dispose them or omit the diagnostic output, preventing
  repeated compilation and project-operation failures from accumulating memory.

## [0.7.0]

### Added

- Added host-neutral editable `.ondaproject` files and immutable project images with exact source graphs,
  content-addressed typed buffer assets, version-1 project-image and `.ondabuffer` serialization,
  integrity fingerprints, and filesystem-free materialization plans. The CLI, native run GUIs,
  browser playground, C API, and WebAssembly compiler now share the same project model.
- Added `onda project` for creating an empty project or packaging an existing source with optional
  `--buffer name=path` bindings, plus project open/save workflows in the egui and webview hosts and
  ZIP import/export in the browser playground.
- Added fixed buffer resource arrays such as `f32 {88}`, with clamped constant-time
  selection, contiguous host descriptors, logical group metadata, nullable project bindings,
  first-class selected slots, and exact compile-time subspans for forwarding collections to procs.

### Changed

- Renamed exact in-memory source compilation to `onda_compile_source_graph` in C and
  `compileWorkspace` in `@onda-lang/wasm-compiler`, reserving “project” for portable project images.
- Buffer types now use `buffer<T>` (for example `buffer<f32[2]>`) instead of `buffer[T]`, and
  multichannel sample/slice access now uses one `[channel, frame-or-range]` coordinate pair instead
  of chained channel indexing. Buffer parameters with dynamic channels accept mono and exact-channel
  declarations, while exact-channel parameters reject dynamic-channel declarations.
- Unbound buffers now process through neutral one-frame descriptors: reads return zero, writes are
  discarded, exact channel declarations are retained, and dynamic channel declarations report one
  channel. Source-level unsafe buffer operations were removed in favor of uniformly clamped
  indexing; unchecked access remains compiler-internal MIR only.
- Advanced the MIR schema to version 4 and the processor artifact and ABI formats to version 3 for
  buffer-array references and nullable buffer pointer entries with processor-owned neutral storage.

### Fixed

- Restored `onda_buffer_may_write` and processor-descriptor `may_write` metadata to report
  call-transitive writes reachable from init, process, or event entry points. Declared buffer access
  capability remains available separately as `access`.

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

[0.7.3]: https://github.com/onda-lang/onda/compare/0.7.2...0.7.3
[0.7.2]: https://github.com/onda-lang/onda/compare/0.7.1...0.7.2
[0.7.1]: https://github.com/onda-lang/onda/compare/0.7.0...0.7.1
[0.7.0]: https://github.com/onda-lang/onda/compare/0.6.0...0.7.0
[0.6.0]: https://github.com/onda-lang/onda/compare/0.5.4...0.6.0
[0.5.4]: https://github.com/onda-lang/onda/compare/0.5.3...0.5.4
[0.5.3]: https://github.com/onda-lang/onda/compare/0.5.2...0.5.3
[0.5.2]: https://github.com/onda-lang/onda/compare/0.5.1...0.5.2
[0.5.1]: https://github.com/onda-lang/onda/compare/0.5.0...0.5.1
[0.5.0]: https://github.com/onda-lang/onda/compare/0.4.4...0.5.0
