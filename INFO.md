# omni-llvm Project Info

## Overview
`omni-llvm` is a Rust compiler/runtime for an Omni-syntax audio DSL, targeting LLVM ORC JIT for host embedding (C ABI first-class).

### Module map: semantics (`crates/omni_semantics/src`)
- Entry:
  - `lib.rs` (public types + orchestration wiring)
- Key analysis/lowering modules:
  - `processor_lowering.rs` + `processor_lowering/*` (proc desugaring and lowering pipeline)
  - `proc_call_rewrite.rs` (proc call lowering/rewrite)
  - `stmt_analysis/*` (init/sample/def statement analysis)
  - `expr_validation.rs`, `expr_typing.rs`, `port_coercion.rs`, `namespacing.rs`
- Recently split helpers:
  - `proc_state_rewrite.rs` (proc state discovery + symbol rewrite helpers + proc metadata structs/constants)
  - `declaration_coercion.rs` (`coerce_struct_fields`, `coerce_params`, `coerce_buffers`, `split_field_path`)
  - `io_state_helpers.rs`
  - `def_inference/call_inference.rs`, `def_inference/return_inference.rs`
  - `generic_specialization/proc_specialization.rs`
  - `processor_lowering/generic_proc_rewrite.rs`
  - `processor_lowering/global_proc_rewrite.rs`
  - `processor_lowering/generated_blocks.rs`

### Module map: codegen (`crates/omni_codegen_llvm/src`)
- Entry:
  - `lib.rs` (public JIT API + metadata extraction)
  - `orc_backend.rs` (backend assembly/wiring)
- ORC backend modules:
  - `proc_ir.rs`, `user_fn_ir.rs`, `specialization.rs`, `data_access.rs`, `call_helpers.rs`
  - `builtin_intrinsics.rs`, `layout.rs`, `jit_utils.rs`, `llvm_helpers.rs`, `pointer_helpers.rs`, `orc_locals.rs`
- Recently split helpers:
  - `orc_backend/orc_expr_stmt.rs` as thin wrapper +:
    - `orc_backend/orc_expr_stmt/expr_lowering.rs`
    - `orc_backend/orc_expr_stmt/stmt_lowering.rs`
  - `orc_backend/def_lowering.rs` as orchestrator +:
    - `orc_backend/def_lowering/expr_lowering.rs`
    - `orc_backend/def_lowering/stmt_lowering.rs`
    - `orc_backend/def_lowering/struct_helpers.rs`

### Practical navigation entrypoints
- Language/front-end behavior: `omni_frontend/src/parser/*`, `omni_semantics/src/lib.rs`
- Proc lowering path: `processor_lowering.rs` -> `processor_lowering/*` -> `proc_call_rewrite.rs`
- ORC lowering path: `orc_backend.rs` -> `{proc_ir,user_fn_ir}` -> `{orc_expr_stmt,def_lowering}` -> `{data_access,call_helpers}`
- Runtime API usage: `omni_runtime/src/lib.rs`
- C ABI surface: `omni_api/src/lib.rs`

## Current implementation snapshot (2026-02)

### Language and parser
- Top-level and proc blocks: `ins`, `outs`, `params`, `events`, `buffers`, `init`, `block`, `sample`, `def`, `struct`, `proc`/`processor`, `namespace`.
- Both brace and indentation syntaxes are supported.
- Statement separators support both newline and `;`.
- Import system is implemented:
  - `include "path.omni"` (quoted, `.omni` suffix required).
  - `import module/path` (resolved as `module/path.omni`, imported once, declaration-only files).
  - Built-in std modules via `import std/...` are supported from both file and in-memory source compilation paths.
- Namespaces with `::` are supported.

### Declaration shorthand
- Count shorthand is supported for all IO/param/buffer sections:
  - `ins N` -> `in1..inN`
  - `outs N` -> `out1..outN`
  - `params N` -> `param1..paramN`
  - `buffers N` -> `buf1..bufN`
- For `ins` / `outs` / `params`, count prefix + explicit list is supported (`ins 2: ...`) and must match the explicit declaration count.
- For `buffers`, explicit declarations and count shorthand still cannot be mixed in the same block.
- Section default type shorthand is supported for IO/param/buffer sections:
  - `ins[T]: ...`, `outs[f64]: ...`, `params[i32]: ...`, `buffers[T]: ...`
  - Also works with count shorthand (`ins[f64] 2`, `buffers[T] 4`).
  - Per-entry explicit types override the section default.

### Types and semantics
- Primitive types: `f32`, `f64`, `i32`, `i64`, `bool`.
- Generic primitive specialization is supported for `struct` and `proc` via type parameters (for example `Name[T]`).
- Specialization is monomorphized at use sites with explicit type args (`Name[f64](...)`) or inferred type args from constructor arguments/defaults where possible.
- Array type syntax: `T[N]` across declarations (with current scope rules enforced in semantics).
- Typed primitive array declarations with inline array-literal initializers are supported in `init` / `sample` / `def` (for example `a: f32[2] = [0.5, 0.8]`).
- Untyped array literal declarations are supported for local/state array declarations in executable blocks (`init` top-level + proc init, `sample`/`block`, `def`, and event handlers), with first-element type inference (for example `a = [0.5, 0.8]`, `a = [f64(0.0), 1.0]`, `a = [0, 1]`, `a = [i64(0), 1]`).
- Scalar `ins` and scalar `params` support optional declaration ranges and defaults:
  - `in1 = 440 {0.01, 22000}` (min+max)
  - `in1 = 440 {22000}` (max-only)
  - `freq: i32 = 500 {20, 8000}` (min+max)
  - `freq: i32 = 500 {8000}` (max-only)
  - Integer-typed declarations require integer defaults/min/max constant expressions.
  - Ranges on array declarations are rejected.
- `Data[...]` is supported for stateful storage, including typed forms and compile-time capacity expressions.
- Scalar assignment typing follows first-assignment inference by default; explicit declaration typing (`x: i64 = ...`) pins the symbol type.
- Constants available in compile-time expressions and runtime code paths: `PI`/`pi`, `TWO_PI`/`TWOPI`/`two_pi`/`twopi`, `SAMPLE_RATE`/`SAMPLERATE`/`SR`/`sample_rate`/`samplerate`, `BLOCK_SIZE`/`BLOCKSIZE`/`BS`/`block_size`/`blocksize`.
- `std/math` is auto-imported during semantic analysis; local symbols with the same name take precedence, while qualified calls remain available via `std::math::...`.
- Control flow and calls:
  - `if`, `for`, `loop N`, `while`, `break`, `continue`, `return`, call statements.
  - `for` syntax supports:
    - `for i in A..B` (exclusive end)
    - `for i in A..=B` (inclusive end)
    - `for i @ STEP in A..B` (`@ STEP` optional, defaults to `1`; use negative step for descending)
- Events:
  - Top-level and proc-level `events` blocks are supported.
  - Event params support primitive scalars and fixed-size primitive arrays.
  - Event array params are passed as read-only references.
  - Event params without explicit type default to `f32`.
  - Event handlers can declare local fixed-size primitive arrays via untyped literals (for example `b = [1, 2, 3]`).
  - Event handlers can write init-root state only (plus local symbols); output/input/event-param writes are rejected.
  - Top-level handlers are host-triggered and run immediately on the audio thread.
  - Unknown host event indices are ignored; payload-size mismatches are runtime errors.
  - Proc events are reached through explicit calls/forwarding (for example `voice.note_on(...)`).
- Functions (`def`):
  - positional + named args, default values, early return.
  - generic type parameters are intentionally unsupported on `def`; polymorphism is through typed/untyped parameters and call-site monomorphization.
  - explicit struct-typed params are nominal.
  - typed and duck-typed buffer params are supported; duck-typed buffer calls specialize by caller shape/type.
- Structs:
  - field defaults and methods supported.
  - generic structs are supported; methods can use owner generic parameters and are specialized with the struct.
  - method `self` rules enforced.
  - constructors restricted to valid semantic contexts (init-time state construction rules enforced).

### Processors (`proc`)
- `proc`/`processor` declarations lower to internal struct + helper defs.
- generic processors are supported and specialized/monomorphized on constructor use.
- `sample` is required; `init` is optional (top-level `init` is also optional).
- `events` is optional inside `proc`.
- generic typed local declarations (`x: T = ...`) are currently supported in `init` only.
- Processor call forms:
  - `p(...)` (scalar return for single-out procs; sugar for `p.out1` / endpoint name)
  - direct endpoint call read: `p(...).<endpointName>` (also supports `.outN` alias)
  - endpoint reads: `p.<endpointName>`
  - ordinal reads: `p.outN` (1-based alias to the Nth declared endpoint)
  - statement call + field reads is supported for stateful updates + explicit output access
- Nested processor state/composition is supported, including deep nesting.
- Processor constructor arguments for params/buffers are enforced as named-only.
- Processor instance arrays are supported in `init` (top-level and proc-level) via typed declarations such as:
  - `voices: Voice[N_EXPR] = [Voice(...), ...]`
  - `voices: Voice[N_EXPR] = Voice(...)` (broadcast constructor sugar)
  - `N_EXPR` can be any compile-time constant expression (not only integer literals).
  - These declarations currently desugar to per-slot instances (`voices[idx]`) during processor/top-level desugaring.
  - For broadcast constructor sugar, constructor args can be mixed:
    - scalar expression: broadcast to every slot (`gain = 0.5`)
    - array literal: per-slot value (`gain = [0.5, 0.8]`, `buf = [buf1, buf2]`)
    - array symbol for non-buffer args: per-slot indexed read (`g: f32[2] = [...]`, then `gain = g`)
  - Indexed proc-array calls are supported with literal indices:
    - `voices[1](...)`
    - `voices[1](...).outN` / named output endpoint
  - Non-literal proc-array call indices are currently rejected.
- Sample oversampling is implemented for both top-level and proc sample blocks:
  - syntax: `sample N:` where `N` is one of `{1,2,4,8,16,32,64}`.
  - oversampling path is compiler-managed (input interpolation, held params, filtered decimation).
  - proc-level oversampling uses the same codegen-rate specialization model as top-level oversampling (unified behavior model, no source-level `SR` rewrite hack).

### External buffers
- Implemented in language, semantics, runtime, and C API.
- Buffer types:
  - `buffer[T]`, `buffer[T[2]]`, `buffer[T[]]` where `T` is `f32`, `f64`, `i32`, `i64`, or `bool`
  - In `buffers { ... }`, shorthand forms like `buf: f32` and `buf: f64[2]` are accepted.
- Access:
  - mono: `buf[i]`
  - multi-channel: `buf[ch][i]`
  - `.len()` and `.chans()`
  - `unsafe_read` / `unsafe_write` for unchecked access (UB on OOB).
- Runtime binding validates element type and channel constraints.

## Runtime and codegen
- ORC JIT backend only (`Auto` routes to ORC).
- Optimized LLVM pipeline is used (`default<O3>` style pass pipeline + host-target settings).
- Fixed compile-time block size per program/instance.
- No callback-time allocations for compiler-managed DSP state; allocations happen during setup/init.
- Runtime processing API is bound-buffer based (`process_bound`).
- Current runtime behavior:
  - all declared buffers must be bound before processing.
  - top-level ranged params are hoisted and clamped once per block in JITed code.
  - top-level ranged inputs are hoisted and clamped once per sample in JITed code.
  - host-triggered events execute synchronously via index-based dispatch.

## C API and CLI
- C ABI exposes compile/create/process/destroy and bind/set calls:
  - params: byte-typed `set_param_by_index`
  - events: `trigger_event_by_index`
  - instance state lifecycle: `reset_instance_state`
  - inputs/outputs: pointer + byte-size binding
  - buffers: pointer + frames + channels + element type binding
  - outputs: `bind_output` and `copy_output`
- Metadata queries exposed for names, indices, types, and byte sizes (including events/payload size).
- CLI (`omni`) supports:
  - `compile <file> [--ir] [--meta]`
  - `render <file> [--output] [--dur] [--sr|--sample-rate] [--block] [--ir]`

## LLVM dependency strategy
- Prebuilt LLVM is vendored under `.deps/llvm/21.1.2`.
- Source bootstrap supports linkage modes:
  - static install: `.deps/llvm-src/21.1.2-static` (`scripts/bootstrap-llvm-source.ps1 -Linkage Static`)
  - shared install: `.deps/llvm-src/21.1.2-shared` (`scripts/bootstrap-llvm-source.ps1 -Linkage Shared`)
- `scripts/use-llvm-env.ps1` can select a flavor (`auto`, `prebuilt`, `source-static`, `source-shared`, `source`).
- `llvm-sys` line is `211.x` (compatible with LLVM 21.1.x C API).
- ORC path is implemented through `llvm-sys`.

## Major remaining work
- Graph composition syntax.
- AOT backend.
- C++ single-header backend.
- Standard library expansion/versioning beyond MVP module set.
- RT-safety instrumentation/audit suite and stricter host-facing diagnostics lifecycle.
