# omni-llvm

`omni-llvm` is the toolchain for the Omni audio DSL.
It provides the compiler, runtime, CLI, language server, preview tooling, editor integrations, and a C API for embedding.

This README is the overview of the tools around the language.

## Short Omni example

For the actual language reference, see [SYNTAX.md](SYNTAX.md).
As a minimal example for a sine wave:

```omni
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

- `omni` CLI for compile, render, preview, diagnostics, and language-server workflows
- LLVM-backed compiler and JIT runtime for Omni programs
- stdio LSP server for diagnostics and semantic tokens
- preview tooling for offline render and real-time playback
- editor integrations for VSCode and Neovim
- C API in `include/omni_llvm.h` for non-Rust hosts

Current execution is LLVM ORC JIT.
The CLI can also emit LLVM IR and native object files for AOT-style workflows.

## Main components

- `crates/omni_frontend`: parser, AST, diagnostics
- `crates/omni_semantics`: semantic analysis and lowering rewrites
- `crates/omni_codegen_llvm`: LLVM lowering and ORC JIT backend
- `crates/omni_runtime`: runtime instance and processing APIs
- `crates/omni_api`: C ABI
- `crates/omni_daemon`: analysis and preview session engine
- `crates/omni_preview`: shared preview controller/runtime
- `crates/omni_cli`: CLI, LSP adapter, and preview control transport
- `crates/omni_egui`: native egui preview host
- `crates/omni_webview`: native webview preview host
- `editors/vscode`: VSCode extension
- `editors/nvim`: Neovim runtime support
- `examples/`: Omni example programs

## Documentation

- [SYNTAX.md](SYNTAX.md): language syntax and semantics
- [INFO.md](INFO.md): project structure and implementation notes

## Building `omni`

1. Bootstrap LLVM.

Windows PowerShell:

```powershell
./scripts/bootstrap-llvm.ps1
```

macOS/Linux:

```bash
./scripts/bootstrap-llvm.sh
```

2. Build the CLI:

```bash
cargo build -p omni_cli --release
```

This produces the `omni` executable in `target/release/`.

3. Build the static and shared libraries if needed:

To build the C API in release mode:

```bash
cargo build -p omni_api --release
```

This produces the `omni_api` artifacts in `target/release/` along with the public header in `include/omni_llvm.h`.
Depending on platform/toolchain, that includes the static library and the shared library import/runtime pair.

## The `omni` CLI

The CLI surface is:

```text
omni compile <input.omni>
omni render <input.omni>
omni lsp
omni preview <input.omni>
omni preview play <input.omni>
omni preview render <input.omni>
omni daemon diagnose <input.omni>
omni daemon stdio
```

### `omni compile`

Compiles an Omni file and optionally emits IR or an object file.

Typical uses:
- syntax and semantic checking
- inspect graph lowering with `--dump-graph`
- emit LLVM IR with `--emit llvm-ir` or `--ir`
- emit a native object file with `--emit obj`

Examples:

```bash
omni compile examples/sine.omni
omni compile examples/proc_gain_graph.omni --dump-graph
omni compile examples/sine.omni --emit llvm-ir
omni compile examples/sine.omni --emit obj
```

Cross-target IR and object emission is also supported:

```bash
omni compile examples/sine.omni --target-spec ./targets/arm64.toml --emit obj
```

### `omni preview`

Opens the standalone patch preview window.
This is the interactive path for listening to a patch, tweaking params, and inspecting buffers/devices.

```bash
omni preview examples/sine.omni
```

Preview host selection:
- `--egui` forces the egui preview host
- `--no-egui` forces the webview preview host
- Note: Linux is currently egui-only. Windows and macOS default to the webview preview host instead.

Useful flags:
- `--sample-rate`
- `--block`
- `--fast-math`
- `--input-device`
- `--output-device`

### `omni preview play`

Runs the real-time playback/control transport without opening the standalone UI.

```bash
omni preview play examples/sine.omni --dur 2
omni preview play examples/sine.omni --forever
```

Useful flags:
- `--dur` or `--forever`
- `--set name=value`
- `--meta`
- `--control-json`
- `--input-device`
- `--output-device`

With `--control-json`, `omni preview play` prints a control handshake on stdout and serves a localhost control socket for preview clients.

### `omni preview render`

Offline render through the preview pipeline.
This is useful when you want the preview-oriented path, including `--set` parameter overrides, without running real-time playback.

```bash
omni preview render examples/sine.omni --output ./omni_out.wav --dur 5 --set freq=220
```

### `omni render`

Renders an Omni program to a WAV file offline.

```bash
omni render examples/sine.omni --output ./omni_out.wav --dur 5
```

Useful flags:
- `--output`
- `--dur`
- `--sample-rate`
- `--block`
- `--dump-graph`
- `--ir`
- `--fast-math`

### `omni lsp`

Starts the Omni language server over stdio.

```bash
omni lsp
```

Current LSP support includes:
- open, change, save, and close document tracking
- diagnostics on open and save
- semantic tokens for constants, params, ports, and init-scoped variables

### `omni daemon diagnose`

Runs daemon-backed analysis for a file and reports diagnostics.

```bash
omni daemon diagnose examples/sine.omni
```

### `omni daemon stdio`

Starts the daemon control transport over stdio.
This is intended for tool/editor integration rather than everyday manual use.

```bash
omni daemon stdio
```

## C API

The C API is exposed through `include/omni_llvm.h`.
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

The extension lives in `editors/vscode`.
It provides:
- `.omni` language registration
- syntax highlighting plus LSP semantic tokens
- `Omni: Run Patch`
- `Omni: Stop Patch`
- `Omni: Restart Language Server`

`Omni: Run Patch` launches the preview transport and opens a patch UI with:
- start, stop, and reset controls
- scalar param controls
- input and output device selectors
- buffer binding cards for declared `f32` buffers

### Neovim

The runtime lives in `editors/nvim`.
It provides:
- `.omni` filetype detection
- regex syntax highlighting
- builtin LSP startup through `omni lsp`
- `:OmniRunPatch` for launching the standalone preview window
