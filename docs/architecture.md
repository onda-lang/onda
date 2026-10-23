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
For build, CLI usage, and editor integrations, see [Getting started](https://onda-lang.org/docs/getting-started/).
The [MIR reference](https://onda-lang.org/docs/mir/) specifies the executable compiler boundary;
the [benchmark guide](https://github.com/onda-lang/onda/blob/main/scripts/bench/README.md) owns
measurement procedures and dated results.

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
- `deps/sleef` — pinned upstream vector math sources; see `deps/SLEEF.md` for provenance and ABI.
- `scripts/` — LLVM bootstrap and env-selection helpers; `scripts/bench/` provides bounded DSP
  benchmarks, MIR replay, and IR/SIMD inspection.
- `docs/`, `examples/`, `assets/`, `ui/`, `sc/` — supporting material.

## Compilation pipeline

Both backends consume the same validated, optimized MIR. This keeps language semantics, processor
scheduling, and reusable proofs in one place while allowing target-specific code generation.

| Layer | Responsibility |
| --- | --- |
| Frontend and semantics | Parse and type source, resolve generics and overloads, lower processors/graphs/tasks, and make execution order and storage explicit. |
| MIR | Validate the executable contract, simplify portable operations, and derive effects, ranges, storage origins, and loop legality. |
| Native LLVM | Select physical layout and target features, emit proven attributes, run target-aware O3, select SIMD, and link used vector-math helpers. JIT and AOT share this path. |
| Binaryen | Legalize structured MIR into complete Wasm modules, lower eligible sample loops to SIMD, and optimize with its own target policy. |
| Runtime and hosts | Prepare storage and bindings, schedule segments/events, and collect output outside generated DSP. |

## Module map

### `onda_frontend` (`crates/onda_frontend/src`)
- `lib.rs` — public AST types and diagnostic context.
- `ast.rs` — AST node definitions.
- `source_locations.rs` — transferable source locations sharing immutable file/trace contexts.
  Locations own their context through `Arc`; they remain valid after their program is dropped or
  moved to another thread. No thread-local source registry or bounded context IDs are used.
- `expr_traversal.rs` — shared iterative expression walking, pruned rewrites, postorder
  rewrites, and result folding. Semantic passes use heap-backed traversal for long
  operator chains instead of growing the thread stack; MIR folds eager operators
  in evaluation order while retaining dedicated control-flow lowering for short-circuit expressions.
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
  `const_artifacts` checks and records scalar values, array descriptors, and const-def
  signatures in declaration order. Scalar const initializers evaluate when registered;
  named const arrays retain their initializer until a value is needed. This is array
  payload laziness, independent of future module or declaration loading on demand.
  Configuration arrays follow the same rule; configuration inspection explicitly
  demands their values when constructing host descriptors.
  `declaration_metadata` normalizes dimensions, storage domains, layouts, and required
  defaults using ordinary declaration options. A required scalar initializer, dimension,
  namespace argument, or shape proof can therefore demand an array. Length queries use
  descriptors without demanding elements. Tuple selectors retain their checked index,
  and closed runtime slice bounds retain the options used for their shape proofs.
  Runtime values and direct const-def calls otherwise use their executable owner's
  effective sample rate and block size; host-rate aliases remain fixed.
  `const_validation` and `const_declaration_types` use the ordinary expression, binding,
  and array checkers to report name, call, type, and known shape errors without running
  array initializers or const-def bodies. Generic and slice-dependent checks complete
  with concrete argument metadata. `const_call_validation` caches checked local types,
  shapes, and failures by argument metadata; evaluation consumes those checked types.
  Const declarations are restricted to root and namespace scope. Const defs bypass
  runtime monomorphization; ordinary functions specialize by argument types and shapes.
  Scalar const metadata separates evaluation width from contextual typing. Untyped numeric
  consts evaluate in `i64`/`f64` and materialize as untyped literals; annotations, casts, and
  concrete array elements preserve their types. Overloads and monomorphization share
  `numeric_constraints`, which considers concrete operands before contextual defaults.
  After contextual adaptation, arithmetic, comparisons, numeric builtins, generic
  inference, and branch joins share the numeric promotion rule in `expr_typing`.
  A concrete `i64` combined with a floating operand promotes to `f64`.
  Binary operands, builtin arguments, and scalar generic constraints also share
  one contextual operand adapter; a floating literal retains its family beside
  an integer peer. Pure builtin results retain full literal precision until use,
  independently of the builtin's default concrete result type.
  Assignment checks recognize closed compile-time expressions from lexical metadata without
  demanding array payloads. These expressions may convert at typed destinations; scalar
  lowering evaluates their concrete arithmetic before checking and converting the result.
  Overload resolution and monomorphization share one lexical statement walker and
  specialization fixed point. Defaults use that same solver in their declaration's
  lexical environment; dependent call metadata defers specialization rather than
  recursively traversing default dependencies.
  `const_resolver` owns shared declaration nodes and caches each array payload or failure
  once per concrete declaration and namespace specialization. Concrete array defaults
  in const and runtime callables use the same catalog, including literal initializers,
  with a separate cache for each used compile context. Cached initializer values retain
  their owned-copy semantics through ordinary array constructors; references retain
  their access permissions. Repeated metadata normalization reuses these handles.
  Each declaration captures only direct lexical references; callees own their environments.
  Registration never copies transitive dependency closures, and metadata never depends on
  whether a value is cached. Namespace flattening shares the same nodes and caches while
  keeping declaration catalogs separate from lexical visibility. Later declarations cannot
  enter a captured initializer, function body, or default scope.
  `const_interpreter` uses one heap continuation stack for expressions, declarations,
  calls, and loops. A read suspends its consumer, evaluates the initializer, and resumes
  without a dependency pre-walk or replay. Array bindings retain a declaration and slice
  range through forwarding, arguments, aliases, copies, and length queries. Element reads
  and writes demand the payload. Local aliases share a storage handle; explicit copies and
  fixed-array returns capture independent values through copy-on-write payloads. Const
  checking reuses runtime source resolution and transitive parameter permission analysis. Long
  dependency and metadata chains also release their storage without recursive destruction.
  `array_semantics` owns partial shape matching and initializer classification for static
  checks, const execution, and MIR lowering. Unknown source metadata defers a required
  check; array replacement checks the destination's concrete shape before mutation.
  Defaults use ordinary argument permission checks after transitive writes are known,
  and processor event arguments use the ordinary call checker.
  Const-def branches and logical operands evaluate only executed dependencies. Ordinary
  runtime statements retain the compiler's existing control-flow and checking behavior;
  this feature adds no early branch pruning or deferred body admission.
  `const_scalar` shares concrete scalar operations with MIR constant folding, including
  integer wrapping, exact wide comparisons, float rounding, NaNs, and signed zero.
  Scalar evaluation and MIR lowering defer literal operands until a destination or concrete
  peer selects their arithmetic width. A bottom-up metadata pass records purity and natural
  types; deferred operands borrow the AST and emit no wider intermediates. Explicit casts keep
  their argument's own context, and integer-only literal subtrees use full `i64` precision in
  floating contexts.
  Materialized calls, array elements, and length queries retain their checked numeric types.
  Static slice lengths use the same i32 bound conversion and normalization as runtime views.
  Closed slice bounds are materialized in the checked layout context; dynamic bounds
  retain runtime evaluation.
  Runtime array and slice length queries stay in MIR; const evaluation consults only
  its own array bindings and declaration catalog. Checked function assignments retain
  scalar storage types and tuple component types on the statement before materialization,
  so specialization and cloning require no metadata keyed by diagnostic locations.
  `runtime_materialization` selects the ordinary syntactic call graph before substituting
  constants in roots and selected functions. Its late worklist preserves runtime sample-rate
  and block-size variants through helpers and defaults without changing their checked types
  or layouts. Unused function bodies do not demand array values. Additional variants clone
  a body only when needed; the final variant moves it, and metadata and payloads stay shared.
  `runtime_defaults` expands omitted arguments before call selection. Nested runtime defaults
  use shared parameterless helpers to keep the call graph linear in source size; execution is
  never memoized. Processor hooks retain ordinary ownership, ordering, and activation guards.
  `compile_context` centralizes runtime call-context inheritance; MIR consumes the selected
  variants without rediscovering contexts. Exported message defaults are required metadata.
  MIR registers immutable array data only at its first actual load or view. Folded element
  reads and metadata queries emit no data. Aliases share a data ID by payload identity without
  hashing or copying contents. Shapes are checked per declaration; payload element types are
  checked once per shared payload and expected type. Typed programs retain only payloads
  referenced by materialized runtime code; aliases and clones share their immutable storage.
  Read-only arguments backed by cached immutable values borrow their data directly;
  mutable arguments copy into independent storage. Copies from mutable sources retain
  their snapshot semantics before later arguments execute.
  Const scopes borrow signatures from declaration nodes, and signature copies share default
  expression trees until specialization requires a rewrite.
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
    (call + return), `monomorphization`, and `overloads`. `call_types/scopes` owns their shared
    lexical traversal, branch joins, loop bindings, and termination rules.
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
  Checked tuple destinations use scalar assignment semantics after capturing their RHS once;
  numeric context, range normalization and processor parameter hooks are preserved.
  - `mir_lowering/{lowerer_core,scheduling,control_flow,expressions,calls,aggregates,slices,values}.rs`
    — focused construction domains kept behind one lowering transaction.
  - `mir_lowering/{data,data_slices,references,reference_joins,storage,retained_storage}.rs` — canonical data
    construction/copying, nominal slices, scalar/tensor reference arguments, branch-selected
    references, prepared fixed scratch, and pointer-free block selection reconstruction. Fixed
    scalar arrays of at most 256 bytes preferentially remain local within a conservative 64 KiB
    maximum across each active call path; overflow uses lifetime-reused instance scratch.
    Struct helper tensors use strided slice descriptors;
    fixed-data returns use caller-owned result storage.
  - `mir_lowering/param_arrays.rs` — clamped snapshots of ranged parameter arrays: process
    snapshots persist across segments, init/event snapshots are invocation-local, and helpers
    receive read-only references to the calling boundary’s snapshot. Logical host array
    metadata is preserved. `param_arrays/reads.rs` follows slice origins and transitive helper
    reads so unused arrays and length-only views do not allocate snapshots.
  - `mir_lowering/audio_outputs.rs` — per-sample audio-output cache initialization and single ordered
    commit, kept separate as the optimization-facing output transaction boundary.
  - `mir_lowering/tests.rs` — complete-program lowering regression coverage.

### `onda_mir` (`crates/onda_mir/src`)
- `lib.rs` — public MIR surface and schema version.
- `ids.rs` — deterministic typed IDs for program entities and resources.
- `types.rs` — target-independent scalar, aggregate, slice, and buffer types.
- `ir.rs` — program/interface/state/function model and executable operations.
- `call_graph.rs` — iterative call-graph traversal and dependency worklists.
- `analysis.rs` — backend-neutral call-transitive effects, reference access direction, and integer
  range facts.
- `memory.rs` — storage origins across slice definitions, reference arguments, and returns.
- `loops.rs`, `loops/facts.rs` — counted-loop recognition and reusable per-function facts for
  pointwise audio, memory independence and native loop-cost queries.
- `format.rs` — deterministic human-readable dumps for diagnostics and golden tests.
- `validate.rs` — structural/type validation and explicit trusted-producer provenance for unchecked
  bounds and proven-safe slice-copy overlap.
- `passes.rs`, `passes/{bounds_proofs,cse,state_promotion,loop_distribution}.rs` — backend-neutral
  canonicalization, integer-range-based bounds proofs, conservative scalar CSE, bounded
  alias-safe scalar-state promotion, cleanup, and selective ordered-reduction distribution.
- `json.rs`, `messagepack.rs` — inspectable and compact transports over the same versioned schema.
- The production contract is documented in the [MIR reference](https://onda-lang.org/docs/mir/).

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

- `crates/onda_processor_abi/src/lib.rs` — serializable/deserializable processor descriptor owned
  outside every compiler/backend crate.
- `crates/onda_processor_abi/src/payload.rs`, `payload/` — compiler-free recursive message schemas,
  canonical tensor planning, checked wire preparation, and reusable host encoders/decoders shared
  by native and WebAssembly integrations.

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
- Analysis sessions and document overlays are re-exported from `onda_semantics::AnalysisSession`.
- `run_session.rs` — live JIT instance lifecycle, validated initial and replacement buffer binding,
  param updates, and `render_block`.
  `PreparedRunBuffer` owns validated samples and precomputed waveform data; replacement
  returns retired instance/storage ownership for reclamation outside the render producer.
- `run_session/param_smoothing.rs` — host-owned linear ramps for continuous float controls.
  Prepared indexed entries and a preallocated active list avoid descriptor searches and allocation
  during block updates. Targets remain available to the UI and restart separately from rendered
  values. Hosts can change the duration live; zero settles active targets and disables ramps,
  while positive changes restart active ramps from their published values. Ramp storage is
  prepared even when initially disabled so enabling it needs no render-time allocation.
  The compiler and processor snapshots contain no parameter smoothing state.

### `onda_run` (`crates/onda_run/src`)
- `child_session.rs` — playback child lifetime, readiness messages, and bounded UTF-8 diagnostics.
- `control_client.rs` — controller-side JSON control connection and requests.
- `playback/control_reader.rs` — byte framing that retains partial requests across socket timeouts.
- `param_value_json.rs` — lossless scalar control transport, including decimal strings for `i64`.
- `lib.rs` — run controller wiring real-time audio to a daemon run session, with one revisioned raw
  filesystem watcher for the entry, transitive non-stdlib sources, project manifest/entry/assets,
  unresolved recovery paths, path-targeted snapshot validation, and disk fallback for partial
  watcher coverage. Recovery uses shared non-recursive ancestor subscriptions, including paths
  whose parent directories do not exist yet. Degraded paths are checked once per second;
  metadata validated against a content check avoids rereading unchanged files during fallback.
  Native invalidations always validate contents, regardless of metadata. Reattached and newly
  reachable watch roots receive a content check after registration, so edits during the handoff
  invalidate the compiled snapshot before the controller acknowledges the change.
- `source_graph.rs` — frontend dependency discovery for editor reloads, sharing native Run’s
  source manifest and project recovery paths, including incomplete graphs. Graphs carry SHA-256
  hashes of captured documents and project files so editors can validate the discovery-to-watch
  gap; playback and metadata replies, including failed builds, report the exact
  compiled source manifest and hashes of the manifest/asset bytes actually loaded. The frontend
  retains rejected UTF-8 bytes and failed reads so failure fingerprints never reread restored files.
  Playback reports these
  graphs only with `--report-sources`, which editor hosts opt into alongside `--control-json`;
  native Run owns its watcher and skips this work.
  Project recovery paths and manifest fingerprints come from one read, including invalid manifests.
  Metadata errors carry their graph in `result.sources`; playback failures emit a `sources` event
  before exiting, including failures while resolving project manifests, constants, and assets.
  Resolution retains fingerprints before parsing or decoding can fail; unavailable paths have
  null hashes, while paths rejected before reading remain watchable without a content hash.
  Failed graphs retain previous dependencies until a complete graph replaces them.
- `playback.rs` — preallocated render producer and optional `--control-json` TCP control server; delegates the device callbacks and SPSC transport to `onda_cpal`.
  Events use bounded reusable storage, inline MIDI payloads, and borrowed iterator dispatch.
  `playback/output_worker.rs` transfers a fixed pool of packed output buffers to a consumer that
  decodes delegates and formats prints. Saturation drops output with explicit occurrence counters;
  the producer neither blocks waiting for a packet nor decodes discarded output.
- `playback/buffer_worker.rs` — bounded background file loading, decoding, validation,
  waveform preparation, and retired-storage reclamation. Per-buffer revisions reject
  loads superseded by a later bind or clear. Control clients remain responsive during
  loading; the render producer commits prepared storage between blocks. Replacement
  still rebuilds and initializes the instance at commit, since init may access other
  writable buffers. File contents are owned snapshots after loading; later path changes
  cannot invalidate their memory, and project asset watching retains its reload policy.

### `onda_lsp` (`crates/onda_lsp/src`)
- `lib.rs` — public LSP entry point used by `onda lsp`.
- `server.rs` — JSON-RPC transport, document and watched-file notification state, dependency-aware
  cache invalidation, snapshot-based diagnostic workers, request dispatch, and integration tests;
  parsed worker results transfer directly into the main-thread cache;
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
- `lib.rs` — native egui run host, controller integration, parameter/buffer editors, and theme.
- `buffer_drop.rs`, `buffer_drop/x11.rs` — buffer-card drop targeting and native drag coordinates.
  Linux observes XDND coordinates through Xlib's event converter while preserving winit's
  original events; core pointer queries can remain stale during Wayland-to-XWayland drags.
- `run_view.rs` — loaded run panels and docked MIDI keyboard, compact source and transport header, device selectors, and section orchestration.
- `number_input.rs` — shared native number widget with owned editing drafts, parameter-domain-scaled horizontal drags, and exact integer arithmetic.
- `input_controls.rs` — optional host-owned Input Controls section and scalar editor rendering
  shared with Params. `InputControlSection::show` returns value, layout, smoothing, and reset
  actions for the host to apply; an empty list renders no section. Stock Onda leaves it unused.

### `onda_webview` (`crates/onda_webview/src`)
- `lib.rs` — webview run host, using `onda_run::RunController` for child processes and control transport.

### Host-provided input controls

The shared `ui/run/run.html` accepts an optional `inputControls` collection separately
from `params` and displays it above Params when populated. Hosts decide which inputs belong
in this collection; the compiler and runtime retain one uniform input model. Stock native
and browser hosts supply no controls, so the section remains hidden.

Each entry uses the scalar parameter presentation fields (`name`, optional `label`,
`type`, `scalar`, `value`, `default`, `rangeMin`, `rangeMax`, and `scale`). An entry with
`enabled: false` displays **External input** and disables its editors. Equal finite range
endpoints render a read-only constant. Values describe the stored host control, not a
live measurement of an external signal. Names identify entries within each bank.

The section emits `setInputControl` with `name`, plain `value`, and `commit`, plus
`resetInputControls`, `setInputControlLayout` with `layout` (`sliders` or `knobs`), and
`setInputControlSmoothing` with `milliseconds`. Hosts apply these independently of Params.
Layout updates use `inputControlLayout`; hosts enable the smoothing editor with
`supportsInputControlSmoothing`, `inputControlSmoothingMs`, and
`inputControlSmoothingDefaultMs`. Section folds follow the existing `viewState` protocol;
values and processing settings remain host-owned. Both banks share their scalar editors
and preserve numeric drafts through independent value updates.

Hosts can update existing bindings with `{ type: "parameterValues", state: {
params: [{ name, value }], inputControls: [{ name, value }] } }`. Values are in
plain units; either bank may be omitted. These messages preserve focused drafts
and active gestures and do not render other sections. Names must belong to the
most recently published schema; unknown names are ignored. Schema, layout, and
editability changes use ordinary `state` messages.
The browser host publishes parameter edits and resets through `parameterValues`.
The native webview host uses the same message when a publication changes only
parameter values; any schema or other host-state change publishes full state.

`ui/run/buffers.js` retains cards while their descriptions and editability are
unchanged. When those inputs change, it builds the complete replacement off-DOM
and commits it before measuring waveform canvases, keeping partial layouts from
clamping the document's scroll position. Theme, resize, and section visibility
changes redraw existing waveform canvases independently.

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
- Analysis sessions: `onda_semantics/src/analysis_session.rs`; run sessions: `onda_daemon/src/run_session.rs`.
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
- The [processor ABI](https://onda-lang.org/docs/processor-api/) defines the shared logical processor contract. LLVM emits
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
  index dispatch. Events and delegates use one recursive message schema: wire data is packed
  little-endian in parameter and depth-first field order, with one contiguous tensor per primitive
  leaf; a runtime slice contributes one `i32` logical length followed by its leaf tensors. Event
  entry points preflight the complete wire payload into aligned workspace before handler execution.
  Source delegates lower to direct synchronous subscription calls. Top-level
  delegate publication and authored printing remain explicit observable MIR effects as
  `PublishDelegate` and `PublishLog`. Init, process, and input-event entries accept one optional
  `ExecutionOutput` containing independent caller-owned delegate and print batches. Init and process
  entries reset supplied counters and one shared output sequence on entry; an event entry does the
  same before input preflight, so rejected input returns empty batches. Entries
  append complete packed records without allocation. Hosts merge the two batches by sequence before
  delivery. Generated failure
  clears incomplete delegates while retaining diagnostic print records. Native and Binaryen
  backends share the same logical layouts. Web Audio transports raw print records out of the audio
  callback and formats on the main side; daemon, CLI, and run hosts likewise decode bounded batches
  outside generated execution. Run UIs deliver prints and subscribed delegates in call-local source
  order. See the [host output guide](https://onda-lang.org/docs/host-output/) for collection and
  decoding examples.

### Optimization boundaries

LLVM is the primary performance target. Prefer shared MIR proofs and simplifications; retain a
clear native gain when its Wasm cost is modest and measured. Generated code size has no independent
budget: accept growth from inlining, unrolling, specialization or vectorization whenever it improves
measured runtime performance. Assess instruction-cache effects, register spills and audio callback
latency through runtime measurements. Vector widths, inlining, instruction selection, and
register-pressure decisions belong to backend cost models. Keep numerical permissions explicit
and validate results as well as elapsed time.

The [MIR pass contract](https://onda-lang.org/docs/mir/#backend-neutral-passes) describes legality
and invalidation in detail. The main opportunities exposed to backends are:

- Ordinary source indexing clamps each coordinate independently for every nonempty indexable
  surface. Integer storage ranges preserve `i32`/`i64` interval facts through MIR, and the shared
  whole-program analysis carries them through statically resolved read-only call boundaries and
  scalar returns. The shared bounds-proof pass removes clamping or checks when the complete
  coordinate interval is known to fit. Path-sensitive integer comparisons also prove fixed-array,
  state-array and dynamic slice accesses, including loops bounded by the minimum of several slice
  lengths. Compound boolean guards retain their branch facts, and guarded unit-step loops retain
  a proven lower bound without assuming wrapping arithmetic is monotonic. These proofs
  track value identities through copies, writes, branches, and loops; escaped local storage
  remains conservative. Affine derived indices are expanded only when every intermediate is
  proved not to wrap. Affine queries visit their definitions and current branch constraints,
  without scanning unrelated expression history. The resulting unchecked MIR operations benefit every backend.
  Slice descriptor length caps propagate through definitions, subslices, value arguments and
  returns; payload writes do not invalidate a descriptor's length. Unknown external lengths and
  mutable descriptor references remain conservative.
  Explicit `read_unsafe` / `write_unsafe` calls instead
  establish a programmer-owned unchecked boundary and are memory-unsafe when any coordinate is
  invalid.
- MIR common-subexpression elimination reuses pure scalar expressions whose operands and saved
  results remain stable. Memory and descriptor operations stay outside this pass; backend
  optimizers handle load/store forwarding using their own alias analysis.
- MIR optimization propagates proven integer argument constants and removes unreachable functions,
  unused internal parameters, and forwarding arguments to a fixed point after semantic validation.
  Call-graph worklists propagate effects and integer ranges to affected functions. `OptimizedProgram`
  retains final analysis facts for backend consumption; mutations require a new optimization boundary.
  It retains a parameter when any call site uses fallible argument preparation, preserving checked
  fixed-range and dynamic-slice addressing even when the callee does not read the reference.
- MIR storage origins preserve owned-object identities and possible contiguous element ranges through
  views and calls, including different views passed to the same helper. MIR folds common descriptor
  lengths and removes proven slice construction and window checks. Native lowering computes
  reference alignment from the recorded element offsets; overlapping views keep the same identity.
  LLVM alias scopes share proven disjoint groups of accessed origins, avoiding a full complement
  list per state object. Overlapping unions refine those groups without discarding disjointness
  facts; mixed origins receive only negative alias facts. Pointwise audio proofs allow a native
  guarded vector path when every pair involving a write is disjoint or exactly equal, with
  partial overlap using the original loop.
  Transitive MIR effects preserve that proof through memory-free scalar value-only helpers that
  cannot fail or loop indefinitely; LLVM retains responsibility for inlining and vectorization.
- Bounded counted array and known-shape slice loops have a shared MIR dependence proof, including affine forward and
  mirrored indices and straight-line helper calls. Every pair involving a write must use separate
  owned objects, disjoint index ranges or stride residue classes, or the same injective index
  expression. Array references may join several owned state regions across call sites; comparisons
  include each region's offset in its backing object. Every possible view undergoes the same
  dependence checks. Runtime loop limits use their proven upper bound to cover all possible iterations;
  negative or zero counts remain empty. Unknown references,
  wrapping address arithmetic, unsupported control flow and hidden dependencies reject the proof.
  LLVM access groups carry it through inlining without relaxing floating-point order. A separate
  native cost policy keeps short floating-point copies scalar on AVX2 targets without AVX-512;
  larger loops and arithmetic kernels retain LLVM's ordinary vectorization decisions.
  Loop queries share one immutable per-function summary and lazily collected descriptor origins;
  each dependence proof tracks only the locals it uses, including inside helpers.
- Large fixed loops with independent expensive math and an already-materialized result may split
  rendering from an ordered sum. The MIR transform preserves addition order and introduces no
  scratch arrays. Calls, failures, unresolved aliases, and coupled reductions prevent distribution.
- The standard library keeps selected coefficient derivations at their update rate. `OnePole`,
  `DecayEnv`, and `Pan3db` cache results only while their source parameters remain equal, with
  explicit validity state for initialization and recomputation at each changed assignment.
  NaNs recompute because they do not compare equal. These are processor-owned caches, not a
  compiler-wide promise to skip user parameter hooks; events, segments, and snapshots retain
  their ordinary semantics.
- Native floating-point permissions independently control reassociation, contraction and ignoring
  signed zero. They default off. Vector math is automatic: Linux/macOS x64 and ARM64, and Windows
  x64 MSVC, use SLEEF VFABI mappings selected from resolved LLVM CPU features. After application
  optimization, the backend resolves newly introduced fixed-width vector `exp2` intrinsics using
  the same legal target variants, links only the used ISA/precision bitcode, internalizes helpers with
  a private register calling convention, and discards unused functions. SLEEF keeps its own strict
  arithmetic policy even under application fast math. Both JIT code and AOT objects contain the
  implementations, including cross-target objects; there is no separate SLEEF library or flag.
  Scalar tails use platform libm, whose addresses are registered explicitly for ORC.
  IR inspection includes source line tables for LLVM remarks; JIT and object emission omit them.
- Native state arrays of at least 256 bytes start at 64-byte boundaries, avoiding split SIMD
  accesses after irregular array lengths. Owned state allocations and clones preserve that
  alignment, including with host allocators; LLVM entry-point attributes expose the guarantee
  through derived pointers and slices. Portable snapshots contain only logical elements.
- Binaryen vectorizes proven independent straight-line f32/f64 sample loops with scalar tails.
  Runtime guards admit disjoint or exactly in-place audio buffers; partial overlap uses scalar code.
  Invariant reads include state and interface controls, whose MIR place base is `param`.
  Carried state, calls, mixed precision and unsupported control flow retain scalar lowering.

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
  the [benchmark guide](https://github.com/onda-lang/onda/blob/main/scripts/bench/README.md#compare-backend-compilation-and-runtime). It is a development benchmark, not a
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

- `deps/sleef` is a git submodule pinned to SLEEF 3.9.0. Generated LLVM 21 bitcode in
  `crates/onda_codegen_llvm/math-kernel` ships with the compiler; regular Cargo builds need neither
  initialized SLEEF sources nor a SLEEF build. `scripts/build-native-math-kernel.py` regenerates
  the kernels from unmodified upstream sources and records tool versions and SHA-256 hashes.

- `deps/llvm-bootstrap` is a git submodule; local developer builds bootstrap LLVM from source on all platforms.
- Source-bootstrap wrappers download `llvm-project` source and install into:
  - static install: `.deps/llvm-src/21.1.2-static`
  - shared install: `.deps/llvm-src/21.1.2-shared`
- Default source-bootstrap target set is `X86;AArch64;WebAssembly`.
- CI-oriented prebuilt bootstrap: `scripts/bootstrap-llvm.ps1` / `scripts/bootstrap-llvm.sh` (when `CI` is set) downloads release assets from `onda-lang/llvm-bootstrap` into `.deps/llvm/21.1.2`.
- LLVM env-selection scripts: `scripts/use-llvm-env.ps1` / `scripts/use-llvm-env.sh` (source the bash one). Flavors: `auto`, `prebuilt`, `source-static`, `source-shared`, `source`.
- `llvm-sys` line is `211.x` (compatible with LLVM 21.1.x C API). The ORC path is implemented through `llvm-sys`.

Structured host messages share `onda_processor_abi::payload`, a compiler-free recursive schema and
SoA tensor planner. Semantic aggregate layouts, MIR validation, native input preparation, and host
value codecs use its canonical leaf order. The browser ABI package mirrors that contract, and
backend parity tests exercise the same structured messages. Generated event entry points preflight
all input before writing aligned workspace; synchronous internal forwarding borrows existing data.
Logical host encoding/decoding and workspace provisioning happen outside realtime execution.
Typed nominal structs retain only their directly declared fields; semantic passes recursively walk
those references when they need leaf paths rather than storing duplicated dotted descendants.
