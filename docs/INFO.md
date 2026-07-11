# onda Project Info

This document describes the architecture of `onda` and where each piece of the project lives.
For language syntax and semantics, see [SYNTAX.md](SYNTAX.md).
For build, CLI usage, and editor integrations, see [README.md](../README.md).

## Workspace layout

`onda` is a Cargo workspace. The crates live under `crates/`:

| Crate | Role |
| --- | --- |
| `onda_frontend` | Parser, AST, diagnostics. PEG grammar (`grammar.pest`) driving an iterative parser. |
| `onda_semantics` | Semantic analysis and lowering rewrites: typing, overload resolution, generic specialization, proc/graph lowering, name resolution. |
| `onda_codegen_llvm` | LLVM lowering and ORC JIT backend, plus AOT IR/object emission and metadata extraction. |
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

### `onda_codegen_llvm` (`crates/onda_codegen_llvm/src`)
- `lib.rs` — public JIT/AOT API (`CompileOptions`, `ExecutionBackend`, metadata extraction).
- `metadata.rs` — runtime metadata extraction (params, buffers, events, state layout).
- `state_layout.rs` — instance state byte layout.
- `runtime_validation.rs` — runtime binding validation.
- `target_config.rs` — target triple / CPU / features / reloc / code-model / opt-level config.
- `primitives.rs` — LLVM primitive helpers.
- `aot_artifact.rs` — AOT object + sidecar emission.
- `orc_backend.rs`, `orc_backend/` — ORC backend assembly and lowering:
  - `pipeline.rs`, `contexts.rs`, `value_model.rs`, `process_handle.rs` — backend wiring and process handle.
  - `proc_ir.rs`, `proc_ir/{common,event_ir,init_ir,process_ir}.rs` — proc IR emission.
  - `user_fn_ir.rs`, `user_fn_ir/{lowering,registry}.rs` — top-level `def` IR emission and registry.
  - `specialization.rs` — generic specialization at IR level.
  - `def_lowering.rs`, `def_lowering/{expr_lowering,stmt_lowering,struct_helpers}.rs` — def-body lowering.
  - `orc_expr_stmt.rs`, `orc_expr_stmt/{expr_lowering,stmt_lowering}.rs` — owner-scope expr/stmt lowering.
  - `lowering_common/{mod,expr,stmt}.rs` — shared lowering helpers.
  - `expr_common.rs`, `stmt_common.rs` — shared expr/stmt primitives.
  - `call_helpers.rs`, `call_helpers/{common,data_views,struct_args}.rs` — call argument lowering.
  - `array_access.rs`, `data_access`-style helpers via `pointer_helpers.rs` — array, buffer, and pointer access.
  - `layout.rs` — LLVM type layout helpers.
  - `llvm_helpers.rs`, `jit_utils.rs`, `orc_locals.rs`, `oversampling.rs`, `proc_buffer_refs.rs`, `builtin_intrinsics.rs` — assorted backend support.

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
- ORC lowering path: `orc_backend.rs` → `{proc_ir,user_fn_ir}` → `{orc_expr_stmt,def_lowering}` → `{data_access,call_helpers}`.
- Runtime API usage: `onda_runtime/src/lib.rs`.
- C ABI surface: `onda_api/src/lib.rs`, `include/onda.h`.
- Daemon analysis/run sessions: `onda_daemon/src/{analysis_session,run_session}.rs`.
- Real-time playback: `onda_run/src/{lib,playback}.rs`.
- LSP: `onda_cli/src/lsp_stdio.rs` → `lsp_stdio/*`.

## Runtime and codegen architecture

- ORC JIT is the only execution backend (`Auto` routes to ORC). `onda compile` can additionally emit target-aware LLVM IR and native object files through the same lowering path.
- Optimized LLVM pass pipeline (`default<O3>`-style) with host-target settings.
- Compile-time block size per program/instance; no callback-time allocations for compiler-managed DSP state (all setup happens during instance creation/init).
- Runtime processing is bound-buffer based (`process_checked`). Segment variants exist for hosts that split a logical block around sample-accurate events:
  - `process_checked_segment(instance, start_frame, frames, flags)`
  - `prepare_unchecked_process(instance)` / `process_unchecked_segment(...)`
  - segment variants keep full-block base pointers and JIT-loop local frames `[0, frames)`, addressing bound I/O at `start_frame + local_frame`.
  - flags `ONDA_PROCESS_BEGIN_BLOCK` / `ONDA_PROCESS_END_BLOCK` drive block hooks only; they do not imply an implicit runtime cursor.
- Per-block behavior: declared buffers must be bound before processing; top-level ranged params are hoisted/clamped once per block; top-level ranged inputs once per sample; host-triggered events run synchronously via index dispatch; slice events use a dynamic payload layout (`i32 len` followed by contiguous element bytes).

## LLVM dependency strategy

- `deps/llvm-bootstrap` is a git submodule; local developer builds bootstrap LLVM from source on all platforms.
- Source-bootstrap wrappers download `llvm-project` source and install into:
  - static install: `.deps/llvm-src/21.1.2-static`
  - shared install: `.deps/llvm-src/21.1.2-shared`
- Default source-bootstrap target set is `X86;AArch64;WebAssembly`.
- CI-oriented prebuilt bootstrap: `scripts/bootstrap-llvm.ps1` / `scripts/bootstrap-llvm.sh` (when `CI` is set) downloads release assets from `onda-lang/llvm-bootstrap` into `.deps/llvm/21.1.2`.
- LLVM env-selection scripts: `scripts/use-llvm-env.ps1` / `scripts/use-llvm-env.sh` (source the bash one). Flavors: `auto`, `prebuilt`, `source-static`, `source-shared`, `source`.
- `llvm-sys` line is `211.x` (compatible with LLVM 21.1.x C API). The ORC path is implemented through `llvm-sys`.

## Major remaining work

Detailed roadmap notes live under [docs/todo/](todo/).

High-level themes:
- Graph follow-ups: broader source expressions, optional coercion/broadcasting rules, richer diagnostics.
- AOT convenience layers: `--emit wasm`, optional link helpers, host-integration polish beyond the current object + sidecar model.
- C++ single-header backend.
- Richer standard library.
- RT-safety instrumentation/audit suite and stricter host-facing diagnostics lifecycle.
