<h1>
  onda <img src="assets/svg/onda-logo-dark.svg" alt="onda logo" width="40" align="absmiddle" />
</h1>

`onda` is a DSL for low-level audio programming.
This repository provides the compiler, runtime, CLI, language server, preview tooling and a C API for embedding the JIT compiler.

## Short example

For the actual language reference, see [SYNTAX.md](SYNTAX.md).
As a minimal example for a sine wave:

```onda
params:
  freq = 440.0 {20, 1000}

init:
  phase = 0.0

block:
  incr = freq * TWO_PI / SR

  sample:
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
    out1 = sin(phase)
```

Take a look at the `examples/` folder for more usage examples.

## What this repository provides

- `onda` CLI for compile, render, preview, diagnostics, and language-server workflows
- LLVM-backed compiler and JIT runtime for Onda programs
- stdio LSP server for diagnostics and semantic tokens
- preview tooling for offline render and real-time playback
- editor integrations for VSCode and Neovim
- C API in `include/onda.h` for non-Rust hosts

Current execution is LLVM ORC JIT.
The CLI can also emit LLVM IR and native object files for AOT-style workflows.

## Main components

- `crates/onda_frontend`: parser, AST, diagnostics
- `crates/onda_semantics`: semantic analysis and lowering rewrites
- `crates/onda_codegen_llvm`: LLVM lowering and ORC JIT backend
- `crates/onda_runtime`: runtime instance and processing APIs
- `crates/onda_api`: C ABI
- `crates/onda_daemon`: analysis and preview session engine
- `crates/onda_preview`: shared preview controller/runtime
- `crates/onda_cli`: CLI, LSP adapter, and preview control transport
- `crates/onda_egui`: native egui preview host
- `crates/onda_webview`: native webview preview host
- `examples/`: Onda example programs

## Documentation

- [SYNTAX.md](SYNTAX.md): language syntax and semantics
- [INFO.md](INFO.md): project structure and implementation notes

## Building `onda`

1. Initialize submodules.

```bash
git submodule update --init --recursive
```

2. Bootstrap LLVM.

Windows PowerShell:

```powershell
./scripts/bootstrap-llvm.ps1
```

macOS/Linux:

```bash
./scripts/bootstrap-llvm.sh
```

Local builds use `deps/llvm-bootstrap` to build LLVM from source on all platforms.
When `CI` is set, the bootstrap scripts may download a matching prebuilt LLVM package instead.

3. Build the CLI:

```bash
cargo build -p onda_cli --release
```

This produces the `onda` executable in `target/release/`.

4. Build the static and shared libraries if needed:

To build the C API in release mode:

```bash
cargo build -p onda_api --release
```

This produces the `onda` C API library artifacts in `target/release/` along with the public header in `include/onda.h`.
Depending on platform/toolchain, that includes the static library and the shared library import/runtime pair.
On Windows, the shipped static `onda.lib` is built with the static MSVC CRT (`/MT`), so hosts linking that library should use a compatible runtime choice.

## The `onda` CLI

The CLI surface is:

```text
onda compile <input.onda>
onda render <input.onda>
onda lsp
onda preview <input.onda> [--theme <auto|dark|light>]
onda preview play <input.onda>
onda preview render <input.onda>
onda daemon diagnose <input.onda>
onda daemon stdio
```

### `onda compile`

Compiles an Onda file and optionally emits IR or an object file.

Typical uses:
- syntax and semantic checking
- inspect graph lowering with `--dump-graph`
- emit LLVM IR with `--emit llvm-ir` or `--ir`
- emit a native object file with `--emit obj`

Examples:

```bash
onda compile examples/sine.onda
onda compile examples/proc_gain_graph.onda --dump-graph
onda compile examples/sine.onda --emit llvm-ir
onda compile examples/sine.onda --emit obj
```

Cross-target IR and object emission is also supported:

```bash
onda compile examples/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

### `onda preview`

Opens the standalone patch preview window.
This is the interactive path for listening to a patch, tweaking params, and inspecting buffers/devices.

```bash
onda preview examples/sine.onda
```

Preview host selection:
- egui is the default preview host
- `--webview` selects the webview preview host explicitly

Useful flags:
- `--sample-rate`
- `--block`
- `--fast-math`
- `--input-device`
- `--output-device`
- `--theme`

### `onda preview play`

Runs the real-time playback/control transport without opening the standalone UI.

```bash
onda preview play examples/sine.onda --dur 2
onda preview play examples/sine.onda --forever
```

Useful flags:
- `--dur` or `--forever`
- `--set name=value`
- `--meta`
- `--control-json`
- `--input-device`
- `--output-device`

With `--control-json`, `onda preview play` prints a control handshake on stdout and serves a localhost control socket for preview clients.

### `onda preview render`

Offline render through the preview pipeline.
This is useful when you want the preview-oriented path, including `--set` parameter overrides, without running real-time playback.

```bash
onda preview render examples/sine.onda --output ./onda_out.wav --dur 5 --set freq=220
```

### `onda render`

Renders an Onda program to a WAV file offline.

```bash
onda render examples/sine.onda --output ./onda_out.wav --dur 5
```

Useful flags:
- `--output`
- `--dur`
- `--sample-rate`
- `--block`
- `--dump-graph`
- `--ir`
- `--fast-math`

### `onda lsp`

Starts the Onda language server over stdio.

```bash
onda lsp
```

Current LSP support includes:
- open, change, save, and close document tracking
- diagnostics on open and save
- semantic tokens for constants, params, ports, and init-scoped variables

### `onda daemon diagnose`

Runs daemon-backed analysis for a file and reports diagnostics.

```bash
onda daemon diagnose examples/sine.onda
```

### `onda daemon stdio`

Starts the daemon control transport over stdio.
This is intended for tool/editor integration rather than everyday manual use.

```bash
onda daemon stdio
```

## C API

The C API is exposed through `include/onda.h`.
At a high level the flow is:

1. compile source
2. create an instance
3. bind inputs, outputs, params, and buffers
4. process audio
5. optionally trigger events
6. destroy the instance

This is the embedding surface for non-Rust hosts.

## Editor support

### VSCode

The VSCode extension lives in the standalone [`onda-lang/onda-vscode`](https://github.com/onda-lang/onda-vscode) repository.
It provides:
- `.onda` and `.on` language registration
- syntax highlighting plus LSP semantic tokens
- `Onda: Run Patch`
- `Onda: Stop Patch`
- `Onda: Restart Language Server`

`Onda: Run Patch` launches the preview transport and opens a patch UI with:
- start, stop, and reset controls
- scalar param controls
- input and output device selectors
- buffer binding cards for declared `f32` buffers

### Neovim

The Neovim plugin lives in the standalone [`onda-lang/onda-nvim`](https://github.com/onda-lang/onda-nvim) repository.
It provides:
- `.onda` and `.on` filetype detection
- regex syntax highlighting
- builtin LSP startup through `onda lsp`
- `:OndaRunPatch` for launching the standalone preview window
