---
title: Compiler architecture
description: A map of the Onda compiler, runtime, CLI, language server, and host crates.
permalink: /docs/architecture/
section: reference
eyebrow: Contributor guide
---

# Onda compiler architecture

This document describes the architecture of `onda` and where each piece of the project lives.
For language syntax and semantics, see the [language guide](https://onda-lang.github.io/onda/docs/language/).
For build, CLI usage, and editor integrations, see the [getting-started guide](https://onda-lang.github.io/onda/docs/getting-started/).

## Workspace layout

`onda` is a Cargo workspace. The crates live under `crates/`:

| Crate | Role |
| --- | --- |
| `onda_frontend` | Parser, AST, diagnostics. PEG grammar (`grammar.pest`) driving an iterative parser. |
| `onda_semantics` | Semantic analysis and lowering rewrites: typing, overload resolution, generic specialization, proc/graph lowering, name resolution. |
| `onda_mir` | Backend-neutral typed executable IR: logical types, explicit storage/resources, structured control flow, proof-aware validation, optimization, and JSON/MessagePack transport. |
| `onda_codegen_llvm` | LLVM lowering and ORC JIT backend, plus AOT IR/object emission and metadata extraction. |
| `onda_processor_abi` | Compiler-free shared processor descriptor schema and ABI version constants. |
| `onda_compiler_web` | Filesystem-free browser compiler: in-memory source/projects plus embedded stdlib to validated schema-5 MIR MessagePack or JSON. |
| `onda_realtime` | Backend-independent realtime thread policy, including one-time x86 FTZ/DAZ setup. |
| `onda_runtime` | Runtime instance model and processing APIs (process / segment / reset). |
| `onda_api` | C ABI surface exposed through `include/onda.h`. |
| `onda_cpal` | Minimal CPAL/PipeWire backend: device discovery, RT callbacks, sample conversion, and SPSC transport. |
| `onda_daemon` | Stateful session engine: in-memory analysis overlays and live run sessions. |
| `onda_run` | Shared run controller / real-time playback transport used by the CLI and run hosts. |
| `onda_cli` | `onda` binary: argument parsing, `compile`/`run`/`daemon`/`lsp` commands, and the LSP adapter. |
| `onda_egui` | Native egui run host (default `onda run` UI). |
| `onda_webview` | Native webview run host (opt-in via `--webview`). |
| `onda_examples` | Example `.onda` programs surfaced through `examples/`. |

Non-crate directories of note:

- `stdlib/` — built-in `std/...` modules imported by Onda source.
- `include/` — public C header `onda.h`.
- `targets/` — checked-in AOT codegen presets for `onda compile --target-spec`.
- `packages/onda_binaryen_web/` — Binaryen.js MIR-to-Wasm backend, reproducible embedded no-std
  math kernel, and browser runtime helpers.
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
  - `parser/loading_support.rs`, `parser/module_loading.rs`, `parser/module_loading/namespaces.rs` — `import` / `include` / namespace resolution.
  - `parser/type_helpers.rs` — type-syntax helpers.
  - `parser/tests.rs` — parser tests.
- `grammar.pest` — the PEG grammar.

### `onda_semantics` (`crates/onda_semantics/src`)
- `lib.rs` — public types and orchestration wiring; re-exports `pipeline::analyze`.
- `pipeline.rs`, `pipeline/` — top-level analysis pipeline and `namespace_flattening`.
- Analysis cores:
  - `expr_validation.rs`, `expr_typing.rs`, `expr_analysis/` — expression validation, typing, and environment construction.
  - `stmt_analysis/` — init / runtime / alias / indexed-binding statement analysis.
  - `port_coercion.rs` — port and parameter coercion.
  - `declaration_coercion.rs` — struct field, param, and buffer coercion.
  - `namespacing.rs` — namespace, `use`, and qualified-path resolution.
  - `io_state_helpers.rs` — input/output/state surface helpers.
  - `executable_owner_analysis.rs` — executable-scope owner scoping.
  - `builtins.rs` — builtin functions and compile-time constants.
  - `array_structs.rs` — array/struct helpers.
  - `decl_symbols.rs` — declaration symbol tables.
- `def`/generic machinery:
  - `def_semantics/` — `def` body analysis, `inference/` (call + return), `monomorphization`, `overloads`.
  - `generic_specialization.rs`, `generic_specialization/proc_specialization.rs` — generic owner specialization.
- Processor lowering:
  - `processor_lowering.rs`, `processor_lowering/` — proc desugaring, `nested_proc_lowering`, `nested_paths`, `proc_local_defs`, `shape_helpers`, `generated_blocks`, `generic_proc_rewrite`, `global_proc_rewrite`.
  - `processor_lowering/graph_lowering/` — graph inference/planning/emission/resolution/rewriting/surface/topology/validation/orchestration.
  - `proc_call_rewrite.rs`, `proc_call_support.rs`, `proc_resolution.rs`, `proc_state_rewrite.rs` — proc call lowering, aliasing, and state symbol rewriting.
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
- `passes.rs`, `passes/{cse,state_promotion}.rs` — fixed-point backend-neutral canonicalization,
  pure-expression value numbering, bounded alias-safe scalar-state promotion, and cleanup.
- `json.rs`, `messagepack.rs` — inspectable and compact transports over the same versioned schema.
- The production MIR contract is documented in `docs/MIR.md`.

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
  - `mir_native.rs` — production validated-MIR-to-LLVM lowering, ORC JIT, targeted LLVM IR/object
    emission, ABI layout, and native process/event handles.
  - `jit_utils.rs`, `llvm_helpers.rs` — target-machine, pass-pipeline, ORC, and LLVM initialization
    support shared by MIR JIT and AOT emission.

### `onda_compiler_web` (`crates/onda_compiler_web/src`)
- `lib.rs` — browser-safe source-to-MIR front half. The Wasm exports compile one source or a virtual
  multi-file project entirely in memory, resolve `std/...` from the embedded standard library,
  return structured JSON diagnostics, and expose the MIR schema version. The native test API uses
  the same path.

### `onda_realtime` (`crates/onda_realtime/src`)
- `lib.rs` — allocation-free, once-per-thread audio floating-point policy shared by runtime hosts.

### `onda_processor_abi`

- `onda_processor_abi/src/lib.rs` — serializable/deserializable format-3 processor descriptor owned
  outside every compiler/backend crate.

### `onda_runtime` (`crates/onda_runtime/src`)
- `lib.rs` — runtime instance model, `process_checked` / `process_unchecked` / segment variants, reset, param hoisting/clamping, event dispatch.

### `onda_api` (`crates/onda_api/src`)
- `lib.rs` — C ABI surface (compile/create/process/destroy, bind/set, metadata queries, event trigger, state snapshot/restore).

### `onda_cpal` (`crates/onda_cpal/src`)
- `lib.rs` — CPAL 0.18/PipeWire device discovery and stream setup, allocation-free input/output callbacks, FP-mode setup, and lock-free SPSC sample transport.

### `onda_daemon` (`crates/onda_daemon/src`)
- `lib.rs` — session manager and transport entry points.
- `analysis_session.rs` — in-memory document overlays and `analyze_document` snapshots.
- `run_session.rs` — live JIT instance lifecycle, param/buffer binding, `render_block`.

### `onda_run` (`crates/onda_run/src`)
- `lib.rs` — run controller wiring real-time audio to a daemon run session.
- `playback.rs` — preallocated render producer and optional `--control-json` TCP control server; delegates the device callbacks and SPSC transport to `onda_cpal`.

### `onda_cli` (`crates/onda_cli/src`)
- `main.rs`, `main_tests.rs` — binary entry and integration tests.
- `args.rs` — CLI argument parsing.
- `compile_cmd.rs` — `onda compile` (check / IR / obj / `--dump-graph` / cross-target).
- `run_cmd.rs` — `onda run` / `run play` / `run render` dispatch.
- `daemon_stdio.rs` — `onda daemon stdio` JSON transport.
- `lsp_stdio.rs`, `lsp_stdio/` — hand-rolled JSON-RPC LSP server:
  - `diagnostics.rs`, `completion.rs`, `navigation.rs`, `namespace_resolution.rs`, `position.rs`, `path_utils.rs`.
  - `semantic_tokens/{mod,ast_index,source_fallback,tests}.rs`.
- `formatting.rs`, `diag_print.rs` — diagnostic formatting/printing.

### `onda_egui` (`crates/onda_egui/src`)
- `lib.rs` — native egui run host (param/buffer panels, device selectors, transport controls).

### `onda_webview` (`crates/onda_webview/src`)
- `lib.rs` — webview run host.
- `ipc.rs` — JSON IPC with the run control socket.
- `process.rs` — `onda run play` child process management.
- `watcher.rs` — auto-restart on `.onda` save.

## Practical navigation entrypoints

- Language/front-end behavior: `onda_frontend/src/parser/*`, `onda_semantics/src/lib.rs`.
- Proc lowering path: `processor_lowering.rs` → `processor_lowering/*` → `proc_call_rewrite.rs`.
- Graph lowering path: `processor_lowering/graph_lowering.rs` → `graph_lowering/*` (inspect via `onda compile --dump-graph`).
- Production native lowering: `onda_semantics::lower_program_to_optimized_mir` →
  `orc_backend/mir_native.rs` → `mir_metadata.rs` / `aot_artifact.rs`.
- Browser path: `onda_compiler_web` → schema-5 MIR MessagePack →
  `packages/onda_binaryen_web` → DSP Wasm + host metadata →
  `examples/web/onda_wasm_playground`.
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
- LSP: `onda_cli/src/lsp_stdio.rs` → `lsp_stdio/*`.

## Runtime and codegen architecture

- ORC JIT is the native execution backend. Public `TypedProgram` JIT entry points unconditionally
  route through semantic-to-MIR lowering and the production MIR-native ORC implementation; `onda
  compile` emits target-aware LLVM IR or objects through the same MIR lowering. There is no direct
  `TypedProgram`/frontend-AST LLVM backend.
- Native JIT metadata and AOT sidecar metadata come from validated MIR plus codegen's selected byte
  offsets. Parameter, state, audio/control I/O, buffer, event, export, and target information
  therefore cannot drift from the executable layout through a separate `TypedProgram` walk. The
  format-3 processor descriptor also maps each packed snapshot segment to its physical state offset,
  records the little-endian format-1 scalar encoding and post-init restore base, and declares the
  resolved target pointer model plus artifact integration profile.
- `onda compile <file> --emit mir` exposes deterministic MIR for inspection, while `--emit mir-json`
  emits inspectable schema-5 interchange and `--emit mir-messagepack` emits the compact production
  transport. In a browser, `onda_compiler_web` produces the same MessagePack directly from in-memory
  source and embedded `std/...` modules.
  `packages/onda_binaryen_web` consumes schema 5, including explicit control mirrors, checked slice
  construction, reference windows, and function attributes, and returns DSP Wasm plus physical
  state, snapshot, interface, event, buffer, and import metadata.
- [`PROCESSOR_ABI.md`](PROCESSOR_ABI.md) defines the shared logical processor contract. LLVM emits
  relocatable objects for native and WebAssembly targets and leaves linking to the application;
  Binaryen emits a complete core-Wasm module because browsers expose no linker. Target triples
  select LLVM's platform ABI and object representation without changing the logical Onda ABI.
- Standard optimized LLVM pass pipeline (`default<O3>`-style) with host-target settings; MIR does not
  override LLVM's loop or SLP vectorization heuristics.
- Native checked and prepared-unchecked processing install the shared audio-thread denormal policy;
  on x86 this enables FTZ/DAZ once per thread before executing DSP.
- Compile-time block size per program/instance; no callback-time allocations for compiler-managed DSP state (all setup happens during instance creation/init).
- Runtime processing is bound-buffer based (`process_checked`). Segment variants exist for hosts that split a logical block around sample-accurate events:
  - `process_checked_segment(instance, start_frame, frames, flags)`
  - `prepare_unchecked_process(instance)` / `process_unchecked_segment(...)`
  - unchecked preparation validates current bindings and completes backend setup; it does not
    intentionally preserve stale buffer bindings after a rebind.
  - segment variants keep full-block base pointers and JIT-loop local frames `[0, frames)`, addressing bound I/O at `start_frame + local_frame`.
  - flags `ONDA_PROCESS_BEGIN_BLOCK` / `ONDA_PROCESS_END_BLOCK` drive block hooks only; they do not imply an implicit runtime cursor.
- MIR schema 4 introduced the `(start_frame, frames, flags)` process signature and checked
  `process_frame` audio addressing retained by schema 5. The native wrapper validates segment bounds
  and the flag mask; zero-frame segments are legal. The schema-5 Binaryen wrapper and reference
  AudioWorklet implement the same scheduling contract. The worklet maintains the host-side
  compile-block cursor needed when Web Audio callback sizes differ from the compiled block size.
- Per-block behavior: declared buffers must be bound before processing; top-level ranged params are hoisted/clamped once per block; top-level ranged inputs once per sample; host-triggered events run synchronously via index dispatch; slice events use a dynamic payload layout (`i32 len` followed by contiguous element bytes).

## Browser build and verification

- `wasm-pack build crates/onda_compiler_web --target web --release --no-opt` builds the JavaScript
  glue and compiler Wasm. `--no-opt` disables wasm-pack's optional post-link `wasm-opt` pass; the
  Rust crate remains a release build. `wasm-pack` is required; ordinary native crate tests are not.
- `npm test --prefix packages/onda_wasm_compiler` builds the packaged compiler and tests its API and
  CLI. `npm run test:pack --prefix packages/onda_wasm_compiler` installs the actual tarball in an
  empty project and compiles a smoke source through the published package layout.
- The top-level `[workspace.package].version` is the single authored product version.
  `scripts/sync-package-versions.mjs` updates workspace-owned `Cargo.lock` entries, discovers all
  `@onda-lang/*` packages, and updates their npm manifests/lockfiles. Compiler builds, CI, and
  release packaging invoke it automatically; the JavaScript runtime version module is generated.
- `bash ./examples/web/onda_wasm_playground/build-demo.sh --serve` builds/stages the compiler and pinned
  Binaryen assets, then serves the editable playground. The PowerShell equivalent is
  `.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve`.
- `bash ./examples/web/onda_wasm_aot_sample_player/build-demo.sh --serve` compiles the shared sample
  player to MIR and Binaryen Wasm before serving a compiler-free page on port 8788. The PowerShell
  equivalent is `.\examples\web\onda_wasm_aot_sample_player\build-demo.ps1 -Serve`.
- From `packages/onda_binaryen_web`, `npm test` runs backend/worklet fixtures, `npm run test:onda` runs
  real Onda source plus LLVM/Binaryen parity and the internal-Wasm FMA oracle, and `npm run test:parity`
  selects the differential renderer. `npm run test:corpus` continuously compiles all 47 checked-in
  examples and positive backend fixtures through source -> schema-5 MIR MessagePack -> Binaryen -> valid
  Wasm. These source-driven commands require a working native Rust/LLVM Onda build; `npm test` and
  the browser asset build do not.
- `npm run bench` runs the reproducible native LLVM versus Binaryen/Wasm comparison documented in
  [`docs/BACKEND_BENCHMARKS.md`](BACKEND_BENCHMARKS.md). It is a development benchmark, not a
  universal browser-performance claim.
- The browser build is static after staging and requires no CLI, LLVM, or server-side compiler.
  Compiler and Binaryen work run in a module worker; `packages/onda_webaudio` registers and hosts the
  processor worklet. The Web Audio adapter precompiles processor modules outside the rendering
  thread, caches typed linear-memory views, bulk-copies full-block f32 audio, and locks host
  linear-memory allocation after construction; dynamic event storage is bounded and preallocated.
  Current product limitations include a single-file playground UI despite the
  compiler's multi-file API, restart/reset rather than seamless state-preserving hot swap, no
  control-output or external-buffer UI, and no microphone/input-source routing. Software math helpers
  are internal to generated Wasm, so the render path has no JavaScript math boundary, though native
  LLVM may still execute them faster. The page also assumes the requested `AudioContext` sample rate
  is honored. The current smoke endpoint proves compilation, but automated browser
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
