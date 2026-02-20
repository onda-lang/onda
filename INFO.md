# omni-llvm Project Info

## Overview
`omni-llvm` is a Rust compiler/runtime for an Omni-syntax audio DSL, targeting LLVM ORC JIT for host embedding (C ABI first-class).

## Current implementation snapshot (2026-02)

### Language and parser
- Top-level and proc blocks: `ins`, `outs`, `params`, `buffers`, `init`, `block`, `sample`, `def`, `struct`, `proc`/`processor`, `namespace`.
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
- In `ins`/`outs`/`params`/`buffers`, missing per-entry types can be filled by a section default type (`section[T]`); explicit entry types always win.
- Scalar `ins` and scalar `params` support optional declaration ranges and defaults:
  - `in1 = 440 {0.01, 22000}` (min+max)
  - `in1 = 440 {22000}` (max-only)
  - `freq: i32 = 500 {20, 8000}` (min+max)
  - `freq: i32 = 500 {8000}` (max-only)
  - Integer-typed declarations require integer defaults/min/max constant expressions.
  - Ranges on array declarations are rejected.
- `Data[...]` is supported for stateful storage, including typed forms and compile-time capacity expressions.
- Scalar assignment typing follows first-assignment inference by default; explicit declaration typing (`x: i64 = ...`) pins the symbol type.
- Constants available in compile-time expressions and runtime code paths: `PI`, `TWO_PI`/`TWOPI`, `SAMPLE_RATE`/`SR`, `BLOCK_SIZE`.
- `std/math` is auto-imported during semantic analysis; local symbols with the same name take precedence, while qualified calls remain available via `std::math::...`.
- Control flow and calls: `if`, `for`, `loop N`, `return`, call statements.
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
- generic typed local declarations (`x: T = ...`) are currently supported in `init` only.
- Processor call forms:
  - `p(...)`
  - indexed multi-out: `p(...)[k]` (compile-time constant index)
  - statement call + field reads
- Nested processor state/composition is supported, including deep nesting.
- Processor constructor arguments for params/buffers are enforced as named-only.

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

## C API and CLI
- C ABI exposes compile/create/process/destroy and bind/set calls:
  - params: byte-typed `set_param_by_index`
  - inputs/outputs: pointer + byte-size binding
  - buffers: pointer + frames + channels + element type binding
  - outputs: `bind_output` and `copy_output`
- Metadata queries exposed for names, indices, types, and byte sizes (where applicable).
- CLI (`omni`) supports:
  - `compile <file> [--ir] [--meta]`
  - `render <file> [--output] [--dur] [--sr|--sample-rate] [--block] [--ir]`

## LLVM dependency strategy
- Prebuilt LLVM is vendored under `.deps/llvm/21.1.2`.
- `llvm-sys` line is `211.x` (compatible with LLVM 21.1.x C API).
- ORC path is implemented through `llvm-sys`.

## Major remaining work
- Graph composition syntax.
- AOT backend.
- C++ single-header backend.
- Standard library expansion/versioning beyond MVP module set.
- RT-safety instrumentation/audit suite and stricter host-facing diagnostics lifecycle.



