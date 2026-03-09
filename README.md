# omni-llvm

`omni-llvm` is a Rust compiler/runtime for the Omni audio DSL, with LLVM ORC JIT codegen and a C ABI for host embedding.

## What this project is

- A language toolchain for sample/block DSP programs.
- A runtime for executing compiled Omni programs.
- A C API (`include/omni_llvm.h`) for integration in non-Rust hosts.
- A CLI for compile/render workflows.
- Language support for overloaded top-level `def` functions and struct methods only; proc-local `def` blocks are not overloadable (arity/type-based dispatch with ambiguity diagnostics).
- Processor-instance array dispatch with literal/runtime indices for calls, endpoint reads, statement calls, and proc-event forwarding (direct indexed instance access, with proc-slot buffer refs synced on `process_bound` for dynamic buffer-backed calls).
- Graph routing/composition via `graph` blocks, with implicit proc scheduling, sample-delay cycle breaking, strict array shape checks, and graph inspection via CLI `--dump-graph`.
- User-defined scalar compile-time constants via `const NAME = expr` / `const NAME: T = expr`, available at top-level, in namespaces, and in executable scopes.
- Python-style slice expressions and writable slice assignment for primitive arrays/buffers (for example `a[1:-1]`, `a[:] = 0.0`, `dst[:] = src[:]`).
  - Namespace consts are addressable from outside via qualified paths such as `std::convolution<8, 8>::HopSize` or `std::convolution::HopSize`.
  - For procs with a `block` section, dynamic indexed proc-array `()` calls trigger per-slot block hooks only for actually called slots:
    - `block pre` runs lazily on first `()` call for that slot in the current block.
    - `block post` runs once at block end for slots called in that block.
  - Hook triggering is tied to the proc `()` call (including expression/statement forms), not to alias/index retrieval alone.
  - Procs without `block` keep the fast path (no dynamic active-slot hook tracking).

Current backend target is ORC JIT only.

## Documentation

- Language syntax and semantics: [SYNTAX.md](SYNTAX.md)
- Project structure and implementation notes: [INFO.md](INFO.md)

## Quick start

1. Bootstrap LLVM from source (default static-link setup):
   - Windows (PowerShell):
     - `pwsh ./scripts/bootstrap-llvm-source.ps1`
     - `pwsh ./scripts/use-llvm-env.ps1`
   - macOS/Linux (bash):
     - `bash ./scripts/bootstrap-llvm-source.sh`
     - `source ./scripts/use-llvm-env.sh`
2. Build/check:
   - `cargo check --workspace`
3. Compile an Omni file:
   - `cargo run -p omni_cli -- compile examples/sine.omni`
   - `cargo run -p omni_cli -- compile examples/proc_gain_graph.omni --dump-graph`
   - `cargo run -p omni_cli -- compile examples/stdlib_f32_graph.omni --dump-graph`
   - `cargo run -p omni_cli -- compile examples/inspect_feedback_mix_graph.omni --dump-graph`
4. Render WAV:
   - `cargo run -p omni_cli -- render examples/sine.omni --output ./omni_out.wav --dur 5`

Notes:
- `scripts/bootstrap-llvm-source.ps1` defaults to `-Linkage Static`.
- `scripts/use-llvm-env.ps1` defaults to `-Flavor auto`, which prefers `source-static` when available.
- If you use `scripts/bootstrap-llvm.ps1` (prebuilt LLVM), linking is dynamic (`LLVM-C.dll`).
- Shell equivalents are available for macOS/Linux:
  - `scripts/bootstrap-llvm-source.sh`
  - `scripts/use-llvm-env.sh`
  - `scripts/bootstrap-llvm.sh`

## C API quick start

- Header: `include/omni_llvm.h`
- Compile entrypoint: `omni_compile(src_utf8, options, out_diag)` where `options` is `omni_compile_options_t { fast_math, sample_rate, block_size }`
- Main runtime flow: compile -> create instance -> bind IO/params/buffers -> process -> destroy
- Event flow (optional): query events (`omni_event_count` / `omni_event_name` / `omni_event_index` / `omni_event_payload_bytes`) then dispatch with `omni_trigger_event_by_index`
  - Unknown event index is ignored.
  - Known event with wrong payload size returns an error.
  - Fixed-shape payload bytes are packed in declaration order (native-endian per primitive type; fixed arrays are contiguous).
  - Slice params use dynamic payload layout: `i32 len` followed by contiguous element bytes.
  - `omni_event_payload_bytes` returns `-1` for dynamic event layouts such as slice params.
- Built-in stdlib modules include `std/math`, `std/lookup`, `std/data`, `std/complex`, `std/fft`, and `std/convolution`.
- Buffer threading hint (optional): query `omni_buffer_may_write` to detect buffers that may be written by program code.

Binding contract for optimized codegen:
- Bound input/output/buffer memory regions must not overlap (non-aliased).
- Bindings are zero-copy: runtime stores and uses host pointers directly (no internal copy).
- Bound memory must remain valid and not be freed/reallocated/moved while in use (until rebound/unbound or instance destroy).
- Input memory must be readable; output and buffer memory must be writable during processing.

## Repository layout

- `crates/omni_frontend`: parser, AST, diagnostics
- `crates/omni_semantics`: semantic analysis, typing, lowering rewrites
- `crates/omni_codegen_llvm`: LLVM ORC backend
- `crates/omni_runtime`: runtime instance/process APIs
- `crates/omni_api`: C ABI
- `crates/omni_cli`: CLI commands
- `examples/`: Omni source examples
