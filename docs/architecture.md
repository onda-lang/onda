---
title: Compiler architecture
description: A map of the Onda compiler, runtime, CLI, language server, and host crates.
permalink: /docs/architecture/
section: reference
eyebrow: Contributor guide
---

# Onda compiler architecture

This guide describes the architecture of `onda` and where each piece of the project lives.
For language syntax and semantics, see the [language guide](https://onda-lang.org/docs/language/).
For build, CLI usage, and editor integrations, see the [getting-started guide](https://onda-lang.org/docs/getting-started/).

## Workspace layout

`onda` is a Cargo workspace. The crates live under `crates/`:

| Crate | Role |
| --- | --- |
| `onda_frontend` | Parser, AST, diagnostics. PEG grammar (`grammar.pest`) driving an iterative parser. |
| `onda_project` | Host-neutral project manifests and immutable images, portable source relocation, and typed buffer assets. |
| `onda_semantics` | Semantic analysis and lowering rewrites: typing, overload resolution, generic specialization, proc/graph lowering, name resolution. |
| `onda_mir` | Backend-neutral typed executable IR: logical types, explicit storage/resources, structured control flow, proof-aware validation, optimization, and JSON/MessagePack transport. |
| `onda_codegen_llvm` | LLVM lowering and ORC JIT backend, plus AOT IR/object emission and metadata extraction. |
| `onda_processor_abi` | Compiler-free shared processor descriptor schema and ABI version constants. |
| `onda_compiler_web` | Filesystem-free browser compiler: in-memory source/projects plus embedded stdlib to validated current-schema MIR MessagePack or JSON. |
| `onda_realtime` | Backend-independent realtime thread policy, including one-time x86 FTZ/DAZ setup. |
| `onda_host_protocol` | Canonical host-owned MIDI and transport event catalog shared by native tooling and browser hosts. |
| `onda_runtime` | Runtime instance model and processing APIs (process / segment / reset). |
| `onda_api` | C ABI surface exposed through `include/onda.h`. |
| `onda_cpal` | Minimal CPAL/PipeWire backend: device discovery, RT callbacks, sample conversion, and SPSC transport. |
| `onda_daemon` | Stateful session engine: in-memory analysis overlays and live run sessions. |
| `onda_run` | Shared run controller / real-time playback transport used by the CLI and run hosts. |
| `onda_lsp` | JSON-RPC language server: diagnostics, completion, navigation, semantic tokens, and shared source formatting. |
| `onda_cli` | `onda` binary: argument parsing and `compile`/`run`/`daemon`/`lsp` command dispatch. |
| `onda_egui` | Native egui run host (default `onda run` UI). |
| `onda_webview` | Native webview run host (opt-in via `--webview`). |
| `onda_examples` | Example `.onda` programs and `.ondaproject` showcases surfaced through `examples/`. |

Non-crate directories of note:

- `stdlib/` — built-in `std/...` modules imported by Onda source.
- `include/` — public C header `onda.h`.
- `targets/` — checked-in AOT codegen presets for `onda compile --target-spec`.
- `packages/onda_binaryen_web/` — Binaryen.js MIR-to-Wasm backend, with its compiler split into
  validation/layout, general lowering, and slice/metadata layers under `src/compiler/`; also owns
  the reproducible embedded no-std math kernel and browser runtime helpers.
- `packages/onda_processor_abi/` — small compiler-free JavaScript contract for processor descriptor
  validation, Wasm export validation, integrity-associated artifact files, and shared TypeScript types.
- `packages/onda_wasm_compiler/` — product-facing browser/Node source-to-Wasm package, worker API,
  typed diagnostics, packed artifact helpers, and `onda-wasm` CLI. Release builds stage the Rust
  frontend Wasm and Binaryen backend inside this package.
- `packages/onda_webaudio/` — optional metadata-driven Web Audio adapter for complete wasm32
  processor artifacts; it is not part of the processor ABI or code generator.
- `deps/llvm-bootstrap` — git submodule used to bootstrap LLVM from source.
- `scripts/` — LLVM bootstrap and env-selection helpers.
- `docs/`, `examples/`, `assets/`, `ui/`, `sc/` — supporting material.

## Module map

### `onda_frontend` (`crates/onda_frontend/src`)
- `lib.rs` — public AST types and diagnostic context.
- `ast.rs` — AST node definitions.
- `diagnostics.rs` — diagnostic construction.
- `parser.rs`, `parser/` — parser entry plus submodules:
  - `parser/block_parsing.rs`, `parser/expr_stmt.rs` — block and expression/statement parsing.
  - `parser/preprocess.rs` — source preprocessing.
  - `parser/loading_support.rs`, `parser/module_loading.rs`, `parser/module_loading/namespaces.rs` — `import` / `include` / namespace resolution, authoritative non-stdlib source manifests with exact documents and resolved/unresolved dependency edges, symlink-free native filesystem loading, and filesystem-free replay of captured source graphs.
  - `parser/type_helpers.rs` — type-syntax helpers.
  - `parser/tests.rs` — parser tests.
- `grammar.pest` — the PEG grammar.

### `onda_project` (`crates/onda_project/src`)
- `manifest.rs` — editable `.ondaproject` parsing, validation, symlink-free contained filesystem
  resolution, recovery watch paths for missing entries/assets, inline typed buffers, and bounded
  buffer-shape inspection without decoding file-backed samples.
- `buffer.rs` — canonical `.ondabuffer` transport for every primitive buffer type plus the WAV
  adapter, including header-only metadata inspection; payload integrity is validated when an asset
  is loaded for use.
- `capture.rs` — portable source-path assignment and syntax-aware graph relocation from an exact frontend manifest.
- `image.rs` — immutable content-addressed source-and-buffer checkpoints with bounded stable serialization.
- `materialize.rs` — filesystem-free export and empty `onda project` materialization plans.

### `onda_semantics` (`crates/onda_semantics/src`)
- `lib.rs` — public types and orchestration wiring; re-exports `pipeline::analyze`.
- `pipeline.rs`, `pipeline/` — top-level analysis orchestration, with focused modules for
  compile-time evaluation, const rewriting, integer-range normalization, namespace flattening,
  and post-analysis validation.
- Analysis cores:
  - `expr_validation.rs`, `expr_typing.rs`, `expr_analysis/` — expression validation, typing, and environment construction.
  - `stmt_analysis/` — shared executable-flow statement analysis for defs, methods, events,
    block/sample bodies, and lowered task/proc bodies; init-specific state/resource construction;
    plus shared alias and indexed-binding helpers. Scope policies express the few legal-context
    differences without duplicating typing, assignment, or control-flow semantics.
  - `index_access.rs` — canonical source indexing access modes and unsafe receiver-call rewriting.
  - `port_coercion.rs` — port and parameter coercion.
  - `declaration_coercion.rs` — struct field, param, and buffer coercion.
  - `namespacing.rs` — namespace, `use`, and qualified-path resolution.
  - `io_state_helpers.rs` — input/output/state surface helpers.
  - `executable_owner_analysis.rs` — executable-scope owner scoping.
  - `builtins.rs` — builtin functions and compile-time constants.
  - `array_structs.rs` — array/struct helpers.
  - `decl_symbols.rs` — declaration symbol tables.
- `def`/generic machinery:
  - `def_semantics/` — thin `def` adapters over shared executable-flow analysis, `inference/`
    (call + return), `monomorphization`, and `overloads`.
  - `generic_specialization.rs`, `generic_specialization/proc_specialization.rs` — generic owner specialization.
- Processor lowering:
  - `task_lowering.rs` — owner-local task validation and lowering through a typed CFG, backwards
    live-across-yield analysis, fixed continuation-frame construction, resumable state-machine
    generation, and structured `await`/`reset` expansion. Proc tasks and top-level tasks share the
    same preparation path. Both use one generated resume helper per task; top-level helpers carry
    explicit runtime-context metadata so they can address owner state without cloning their body at
    each `await`. Reset invalidates only the program counter, leaving frame initialization to the
    next start.
  - `processor_lowering.rs`, `processor_lowering/` — proc desugaring, `nested_proc_lowering`, `nested_paths`, `proc_local_defs`, `shape_helpers`, `generated_blocks`, `generic_proc_rewrite`, `global_proc_rewrite`.
  - `processor_lowering/graph_lowering/` — graph inference/planning/emission/resolution/rewriting/surface/topology/validation/orchestration.
- `proc_call_rewrite.rs`, `proc_call_rewrite/call_arguments.rs`, `proc_call_support.rs`,
  `proc_resolution.rs`, `proc_state_rewrite.rs` — proc call lowering, named-argument and call-surface
  expansion, aliasing, and state symbol rewriting.
- `internal_names.rs` — internal symbol naming.
- `mir_lowering.rs` — transactional semantic-to-MIR lowering; owns executable storage/resources,
  events, functions, aggregate/reference shapes, recursive processor arrays, oversampling, and the
  canonical segmented process schedule.
  - `mir_lowering/{lowerer_core,scheduling,control_flow,expressions,calls,aggregates,slices,values}.rs`
    — focused construction domains kept behind one lowering transaction.
  - `mir_lowering/audio_outputs.rs` — per-sample audio-output cache initialization and single ordered
    commit, kept separate as the optimization-facing output transaction boundary.
  - `mir_lowering/tests.rs` — complete-program lowering regression coverage.

### `onda_mir` (`crates/onda_mir/src`)
- `lib.rs` — public MIR surface and schema version.
- `ids.rs` — deterministic typed IDs for program entities and resources.
- `types.rs` — target-independent scalar, aggregate, slice, and buffer types.
- `ir.rs` — program/interface/state/function model and executable operations.
- `analysis.rs` — backend-neutral call-transitive effects, reference access direction, and integer
  range facts.
- `format.rs` — deterministic human-readable dumps for diagnostics and golden tests.
- `validate.rs` — structural/type validation and explicit trusted-producer provenance for unchecked bounds.
- `passes.rs`, `passes/{bounds_proofs,cse,state_promotion}.rs` — fixed-point backend-neutral
  canonicalization, integer-range-based bounds proofs, pure-expression value numbering, bounded
  alias-safe scalar-state promotion, and cleanup.
- `json.rs`, `messagepack.rs` — inspectable and compact transports over the same versioned schema.
- The production MIR contract is documented in `docs/mir.md`.

### `onda_codegen_llvm` (`crates/onda_codegen_llvm/src`)
- `lib.rs` — public JIT/AOT API. The sole `TypedProgram` JIT path lowers to validated MIR before
  native codegen; direct MIR entry points accept `onda_mir::Program` without a frontend side channel.
- `mir_metadata.rs` — runtime descriptors derived from MIR plus the exact physical offsets selected
  by native codegen.
- `runtime_metadata.rs` — shared runtime descriptor types and accessors populated exclusively from
  MIR-native metadata lowering.
- `runtime_validation.rs` — runtime binding validation.
- `target_config.rs` — target triple / CPU / features / reloc / code-model / opt-level config.
- `primitives.rs` — LLVM primitive helpers.
- `aot_artifact.rs` — AOT object metadata/sidecar model, populated from MIR-native layout and
  interface descriptors.
- `orc_backend.rs`, `orc_backend/` — ORC backend assembly and lowering:
  - `mir_native.rs`, `mir_native/function_emitter.rs` — production validated-MIR-to-LLVM lowering,
    function-body emission, ORC JIT, targeted LLVM IR/object emission, ABI layout, and native
    process/event handles.
  - `jit_utils.rs`, `llvm_helpers.rs` — target-machine, pass-pipeline, ORC, and LLVM initialization
    support shared by MIR JIT and AOT emission.

### `onda_compiler_web` (`crates/onda_compiler_web/src`)
- `lib.rs` — browser-safe source-to-MIR front half. The Wasm exports compile one source or a virtual
  multi-file workspace or immutable project image entirely in memory, resolve `std/...` from the
  embedded standard library, return structured JSON diagnostics and exact source graphs, and expose
  host-neutral project image, materialization, typed-buffer, format-version, and MIR-schema APIs.
  The native test API uses the same path.

### `onda_realtime` (`crates/onda_realtime/src`)
- `lib.rs` — allocation-free, once-per-thread audio floating-point policy shared by runtime hosts.

### `onda_host_protocol` (`crates/onda_host_protocol`)

- `events.json` — single canonical MIDI and host-context event catalog consumed by browser hosts and
  generated into Rust constants for runtime validation and language-server completion.
- `src/lib.rs` — catalog types plus exact name, type, and parameter-order matching.

### `onda_processor_abi`

- `onda_processor_abi/src/lib.rs` — serializable/deserializable processor descriptor owned
  outside every compiler/backend crate.

### `onda_runtime` (`crates/onda_runtime/src`)
- `lib.rs` — runtime instance model, `process_checked` / `process_unchecked` / segment variants,
  reset, event dispatch, and public validated parameter-domain discovery/conversion through
  `Instance::param_domain`.

### `onda_api` (`crates/onda_api/src`)
- `lib.rs`, `metadata.rs` — C ABI surface (single-source, direct filesystem source/project input,
  exact in-memory source-graph, and project-image compilation; complete filesystem watch
  projections; program-owned filesystem project defaults without an intermediate portable image;
  host-neutral project capture/load/serialization/materialization and typed buffer assets; source
  snapshot metadata and syntax-aware reference rewriting; create/process/destroy; bind/set;
  metadata queries; event trigger; state snapshot/restore).

### `onda_cpal` (`crates/onda_cpal/src`)
- `lib.rs` — CPAL 0.18/PipeWire device discovery and stream setup, allocation-free input/output callbacks, FP-mode setup, and lock-free SPSC sample transport.

### `onda_daemon` (`crates/onda_daemon/src`)
- `lib.rs` — session manager and transport entry points.
- `analysis_session.rs` — in-memory document overlays and `analyze_document` snapshots.
- `run_session.rs` — live JIT instance lifecycle, validated initial and replacement buffer binding,
  param updates, and `render_block`.

### `onda_run` (`crates/onda_run/src`)
- `lib.rs` — run controller wiring real-time audio to a daemon run session, with one revisioned raw
  filesystem watcher for the entry, transitive non-stdlib sources, project manifest/entry/assets,
  unresolved recovery paths, path-targeted snapshot validation, and disk fallback for partial
  watcher coverage.
- `playback.rs` — preallocated render producer and optional `--control-json` TCP control server; delegates the device callbacks and SPSC transport to `onda_cpal`.

### `onda_lsp` (`crates/onda_lsp/src`)
- `lib.rs` — public LSP entry point used by `onda lsp`.
- `server.rs` — JSON-RPC transport, document and watched-file notification state, dependency-aware
  cache invalidation, snapshot-replayed diagnostic workers, request dispatch, and integration tests;
  the server does not own an OS filesystem watcher.
- `server/diagnostics.rs`, `server/completion.rs`, `server/navigation.rs` — diagnostics, contextual
  completion, hover, signature help, and definition handling.
- `server/param_domain.rs` — parameter-domain and integer-binding-range completion/token contexts.
- `server/language_intrinsics.rs`, `server/unsafe_index.rs` — shared compiler-known statement and
  unchecked-intrinsic signatures/documentation used across completion and navigation.
- `server/namespace_resolution.rs`, `server/position.rs`, `server/path_utils.rs` — namespace, source-position, and path support.
- `server/semantic_tokens/{mod,ast_index,source_fallback,tests}.rs` — semantic-token indexing, incomplete-source fallback, and tests.
- `formatting.rs` — source formatting shared with the CLI.

### `onda_cli` (`crates/onda_cli/src`)
- `main.rs`, `main_tests.rs` — binary entry and integration tests.
- `args.rs` — CLI argument parsing.
- `compile_cmd.rs` — `onda compile` (check / IR / obj / `--dump-graph` / cross-target).
- `run_cmd.rs` — `onda run` / `run play` / `run render` dispatch.
- `daemon_stdio.rs` — `onda daemon stdio` JSON transport.
- `diag_print.rs` — terminal diagnostic rendering.

### `onda_egui` (`crates/onda_egui/src`)
- `lib.rs` — native egui run host (param/buffer panels, device selectors, transport controls).

### `onda_webview` (`crates/onda_webview/src`)
- `lib.rs` — webview run host.
- `ipc.rs` — JSON IPC with the run control socket.
- `process.rs` — `onda run play` child process management.

## Practical navigation entrypoints

- Language/front-end behavior: `onda_frontend/src/parser/*`, `onda_semantics/src/lib.rs`.
- Host-selected compile constants: the loader retains root `config const` declarations, immutable
  `onda_semantics::CompileInputs` are applied before namespace/shape preprocessing, and all native,
  C, CLI, virtual-project, project-image, and browser entry points converge on that semantic path.
- Proc lowering path: `processor_lowering.rs` → `processor_lowering/*` → `proc_call_rewrite.rs`.
- Graph lowering path: `processor_lowering/graph_lowering.rs` → `graph_lowering/*` (inspect via `onda compile --dump-graph`).
- Production native lowering: `onda_semantics::lower_program_to_optimized_mir` →
  `orc_backend/mir_native.rs` → `mir_metadata.rs` / `aot_artifact.rs`.
- Browser path: `onda_compiler_web` → MIR MessagePack →
  `packages/onda_binaryen_web` → DSP Wasm + host metadata →
  `packages/onda_processor_abi` validation → shared `ui/playground` runtime →
  `examples/web/onda_wasm_playground` or website `/playground/` host.
- Packaged browser/Node compiler: `packages/onda_wasm_compiler` composes those first two boundaries,
  verifies their schema handshake, and exposes source/project-to-artifact APIs without exposing the
  trusted MIR transition to ordinary consumers.
- AOT browser deployment: native compiler-only MIR helper →
  `packages/onda_binaryen_web` at build time → complete Wasm artifact →
  `examples/web/onda_wasm_aot_sample_player`.
- Runtime API usage: `onda_runtime/src/lib.rs`.
- C ABI surface: `onda_api/src/lib.rs`, `include/onda.h`.
- Daemon analysis/run sessions: `onda_daemon/src/{analysis_session,run_session}.rs`.
- Real-time playback: `onda_run/src/{lib,playback}.rs`.
- LSP: `onda_cli/src/main.rs` dispatches to `onda_lsp/src/lib.rs` → `server.rs` / `server/*`.

## Runtime and codegen architecture

- ORC JIT is the native execution backend. Public `TypedProgram` JIT entry points unconditionally
  route through semantic-to-MIR lowering and the production MIR-native ORC implementation; `onda
  compile` emits target-aware LLVM IR or objects through the same MIR lowering. There is no direct
  `TypedProgram`/frontend-AST LLVM backend.
- Native JIT metadata and AOT sidecar metadata come from validated MIR plus codegen's selected byte
  offsets. Parameter, state, audio/control I/O, buffer, input-event, delegate, print-site, source,
  export, and target information
  therefore cannot drift from the executable layout through a separate `TypedProgram` walk. The
  processor descriptor also maps each packed snapshot segment to its physical state offset,
  records the little-endian scalar encoding and post-init restore base, and declares the
  resolved target pointer model plus artifact integration profile.
- `onda compile <file> --emit mir` exposes deterministic MIR for inspection, while `--emit mir-json`
  emits inspectable current-schema interchange and `--emit mir-messagepack` emits the compact production
  transport. In a browser, `onda_compiler_web` produces the same MessagePack directly from in-memory
  source and embedded `std/...` modules.
  `packages/onda_binaryen_web` consumes the current schema, including explicit control mirrors, checked slice
  construction, reference windows, and function attributes, and returns DSP Wasm plus physical
  state, snapshot, interface, input-event, delegate, print-site, source, buffer, and import metadata.
- [`processor-abi.md`](processor-abi.md) defines the shared logical processor contract. LLVM emits
  relocatable objects for native and WebAssembly targets and leaves linking to the application;
  Binaryen emits a complete core-Wasm module because browsers expose no linker. Target triples
  select LLVM's platform ABI and object representation without changing the logical Onda ABI.
- Standard optimized LLVM pass pipeline (`default<O3>`-style) with host-target settings; MIR does not
  override LLVM's loop or SLP vectorization heuristics.
- Native checked and prepared-unchecked processing install the shared audio-thread denormal policy;
  on x86 this enables FTZ/DAZ once per thread before executing DSP.
- Compile-time block size per program/instance; no callback-time allocations for compiler-managed DSP state (all setup happens during instance creation/init).
- Runtime init, events, and processing use prepared buffer descriptor tables; omitted slots are
  prepared as neutral descriptors rather than blocking execution. Rebinding invalidates the tables,
  and the next checked entry-point call observes the replacement without implicitly rerunning init.
  Buffer-write metadata conservatively joins writes reachable from init, process, and exported
  events, including generated proc-init paths. Segment variants exist for hosts that split a logical
  block around sample-accurate events:
  - `process_checked_segment(instance, start_frame, frames, flags)`
  - `prepare_unchecked_process(instance)` / `process_unchecked_segment(...)`
  - unchecked preparation validates the current bindings; buffer references resolve directly
    through those tables, so rebinding cannot leave pointer-bearing derived state stale.
  - native codegen snapshots each used direct buffer descriptor field once at process/event entry,
    exposes the validated primitive alignment on accesses, and marks external-buffer memory as
    disjoint from audio outputs. Dynamic buffer-collection selection remains table-driven. This
    keeps rebinding between calls while allowing loop-invariant descriptor reuse and SIMD.
  - segment variants keep full-block base pointers and JIT-loop local frames `[0, frames)`, addressing bound I/O at `start_frame + local_frame`.
  - flags `ONDA_PROCESS_BEGIN_BLOCK` / `ONDA_PROCESS_END_BLOCK` drive block hooks only; they do not imply an implicit runtime cursor.
- The MIR schema defines the `(start_frame, frames, flags)` process signature and checked
  `process_frame` audio addressing. The native wrapper validates segment bounds
  and the flag mask; zero-frame segments are legal. The Binaryen wrapper and reference
  AudioWorklet implement the same scheduling contract. The worklet maintains the host-side
  compile-block cursor needed when Web Audio callback sizes differ from the compiled block size.
- Entry-point behavior: omitted buffers use neutral prepared descriptors; each top-level ranged
  parameter used by an entry point is hoisted and clamped once at the start of init, each event, or
  each logical process block; top-level ranged inputs are clamped once per sample; ranged proc
  parameters are clamped once when stored and are not reclamped when read. Floating NaN maps to the
  range minimum at these generated clamp boundaries. Host-triggered events run synchronously via
  index dispatch; slice events use a dynamic payload layout (`i32 len` followed by contiguous
  element bytes). Source delegates lower to direct synchronous subscription calls. Top-level
  delegate publication and authored printing remain explicit observable MIR effects as
  `PublishDelegate` and `PublishLog`. Init, process, and input-event entries accept one optional
  `ExecutionOutput` containing independent caller-owned delegate and print batches, reset supplied
  counters and one shared output sequence per call, and append complete packed records without
  allocation. Hosts merge the two batches by sequence before delivery. Generated failure
  clears incomplete delegates while retaining diagnostic print records. Native and Binaryen
  backends share the same logical layouts. Web Audio transports raw print records out of the audio
  callback and formats on the main side; daemon, CLI, and run hosts likewise decode bounded batches
  outside generated execution. Run UIs deliver prints and subscribed delegates in call-local source
  order.
- Ordinary source indexing clamps each coordinate independently for every nonempty indexable
  surface. Integer storage ranges preserve `i32`/`i64` interval facts through MIR, and the shared
  whole-program analysis carries them through statically resolved read-only call boundaries and
  scalar returns. The shared bounds-proof pass removes clamping or checks when the complete
  coordinate interval is known to fit. Explicit `read_unsafe` / `write_unsafe` calls instead
  establish a programmer-owned unchecked boundary and are memory-unsafe when any coordinate is
  invalid.
- MIR optimization removes unused internal parameters and forwarding arguments to a fixed point.
  It retains a parameter when any call site uses fallible argument preparation, preserving checked
  fixed-range and dynamic-slice addressing even when the callee does not read the reference.

## Browser build and verification

- `npm run build --workspace @onda-lang/wasm-compiler` builds the Rust frontend and JavaScript glue
  with `wasm-pack --release`, then runs the npm-pinned Binaryen `wasm-opt -O4` over the frontend
  module. The internal `--no-opt` flag prevents wasm-pack from downloading and running a different
  optimizer release. The build also uses `cargo-about` 0.9.2 to generate complete Rust dependency
  license text from the locked cross-platform dependency graph. `dist/build.json` records the
  optimizer policy and size reduction, and package tests verify that the optimized bytes are the
  ones shipped. Generated DSP modules use their
  independent Binaryen O4 compilation policy and are not redundantly post-optimized.
- `npm run test:web` tests the ABI, Binaryen, Web Audio, and compiler workspaces.
  `npm run test:pack --workspace @onda-lang/wasm-compiler` packs and installs all four public
  tarballs in an empty project, then compiles a smoke source through the published package layout.
- The top-level `[workspace.package].version` is the single authored product version.
  `scripts/sync-package-versions.mjs` updates workspace-owned `Cargo.lock` entries, discovers all
  `@onda-lang/*` packages, and updates their npm manifests plus the root workspace lockfile.
  Compiler builds, CI, and release packaging invoke it automatically; the JavaScript runtime
  version module is generated.
- Tag releases publish the four npm packages in dependency order with OIDC trusted publishing and
  provenance; no registry token is stored in GitHub. Each package's npm settings must authorize
  `onda-lang/onda` and `.github/workflows/release.yml` as its trusted publisher before relying on
  the tag job; newly reserved package names may need a one-time registry-owner bootstrap first.
- `npm run build:website` builds the browser compiler, bundles its worker, and emits versioned,
  content-addressed website assets. It also regenerates `docs/stdlib.md` from the standard-library
  modules embedded in `onda_frontend`, so the repository and website share the same API reference.
  The homepage opens its displayed example in `/playground/`
  without loading the compiler itself; `/playground/` provides the full LSP-backed editor and
  AudioWorklet host. The same build discovers every checked-in Onda example automatically and emits
  it with its local source dependencies, so cookbook links open complete projects directly.
  `website/stage.sh` refuses to stage stale or missing browser assets and writes the product version
  into Jekyll data.
- `bash ./examples/web/onda_wasm_playground/build-demo.sh --serve` builds/stages the compiler and pinned
  Binaryen assets, then serves the editable playground. The PowerShell equivalent is
  `.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve`.
- `bash ./examples/web/onda_wasm_aot_sample_player/build-demo.sh --serve` compiles the shared sample
  player to MIR and Binaryen Wasm before serving a compiler-free page on port 8788. The PowerShell
  equivalent is `.\examples\web\onda_wasm_aot_sample_player\build-demo.ps1 -Serve`.
- From `packages/onda_binaryen_web`, `npm test` runs backend/worklet fixtures, `npm run test:onda` runs
  real Onda source plus LLVM/Binaryen parity and the internal-Wasm FMA oracle, and `npm run test:parity`
  selects the differential renderer. `npm run test:corpus` continuously compiles the positive
  backend fixtures through source -> MIR MessagePack -> Binaryen -> valid Wasm. After building the
  browser compiler, `npm run check:examples` compiles every generated example-catalog project and
  every materialized `.ondaproject` showcase through the browser frontend and Binaryen backend.
  The backend-fixture command requires a working native Rust/LLVM Onda build; the example verifier
  and browser asset build do not.
- `npm run bench` runs the reproducible native LLVM versus Binaryen/Wasm comparison documented in
  [`docs/backend-benchmarks.md`](backend-benchmarks.md). It is a development benchmark, not a
  universal browser-performance claim.
- The browser build is static after staging and requires no CLI, LLVM, or server-side compiler.
  Compiler and Binaryen work run in a module worker; `packages/onda_webaudio` registers and hosts the
  processor worklet. The Web Audio adapter precompiles processor modules outside the rendering
  thread, caches typed linear-memory views, bulk-copies full-block f32 audio, and locks host
  linear-memory allocation after construction; dynamic event storage is bounded and preallocated.
  The playground now uses the compiler's multi-file project API and provides a native-style output
  scope plus PCM/IEEE-float WAV loading for f32 external buffers. Top-level audio inputs lazily
  connect one reusable microphone stream; projects without them never request media permission.
  Current product limitations include restart/reset rather than seamless state-preserving hot swap
  and no mutable buffer/control-output inspection. Software math helpers
  are internal to generated Wasm, so the render path has no JavaScript math boundary, though native
  LLVM may still execute them faster. The page recompiles against the actual `AudioContext` sample
  rate before constructing a processor, while the adapter rejects mismatched AOT artifacts. The
  current smoke endpoint proves compilation, but automated browser
  AudioWorklet playback coverage remains future work. Non-local hosting needs a secure context
  (normally HTTPS; localhost is exempt).

## LLVM dependency strategy

- `deps/llvm-bootstrap` is a git submodule; local developer builds bootstrap LLVM from source on all platforms.
- Source-bootstrap wrappers download `llvm-project` source and install into:
  - static install: `.deps/llvm-src/21.1.2-static`
  - shared install: `.deps/llvm-src/21.1.2-shared`
- Default source-bootstrap target set is `X86;AArch64;WebAssembly`.
- CI-oriented prebuilt bootstrap: `scripts/bootstrap-llvm.ps1` / `scripts/bootstrap-llvm.sh` (when `CI` is set) downloads release assets from `onda-lang/llvm-bootstrap` into `.deps/llvm/21.1.2`.
- LLVM env-selection scripts: `scripts/use-llvm-env.ps1` / `scripts/use-llvm-env.sh` (source the bash one). Flavors: `auto`, `prebuilt`, `source-static`, `source-shared`, `source`.
- `llvm-sys` line is `211.x` (compatible with LLVM 21.1.x C API). The ORC path is implemented through `llvm-sys`.
