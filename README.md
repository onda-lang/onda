# omni-llvm

`omni-llvm` is the toolchain for the Omni audio DSL.
It provides the compiler, runtime, CLI, language server, preview tooling, editor integrations, and a C API for embedding.

If you want language syntax and semantics, start with [SYNTAX.md](SYNTAX.md).
This README is the overview of the tools around the language.

## What this repository provides

- `omni` CLI for compile, render, preview, diagnostics, and language-server workflows
- LLVM-backed compiler and JIT runtime for Omni programs
- stdio LSP server for diagnostics and semantic tokens
- preview tooling for offline render and real-time playback
- editor integrations for VSCode and Neovim
- C API in `include/omni_llvm.h` for non-Rust hosts

Current execution is LLVM ORC JIT.
The CLI can also emit LLVM IR and native object files for AOT-style workflows.

## Documentation

- [SYNTAX.md](SYNTAX.md): language syntax and semantics
- [INFO.md](INFO.md): project structure and implementation notes

## Quick start

1. Bootstrap LLVM.

Windows PowerShell:

```powershell
pwsh ./scripts/bootstrap-llvm-source.ps1
pwsh ./scripts/use-llvm-env.ps1
```

macOS/Linux:

```bash
bash ./scripts/bootstrap-llvm-source.sh
source ./scripts/use-llvm-env.sh
```

2. Check the workspace.

```bash
cargo check --workspace
```

3. Compile an Omni file.

```bash
cargo run -p omni_cli -- compile examples/sine.omni
```

4. Render audio offline.

```bash
cargo run -p omni_cli -- render examples/sine.omni --output ./omni_out.wav --dur 5
```

5. Start the language server.

```bash
cargo run -p omni_cli -- lsp
```

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
cargo run -p omni_cli -- compile examples/sine.omni
cargo run -p omni_cli -- compile examples/proc_gain_graph.omni --dump-graph
cargo run -p omni_cli -- compile examples/sine.omni --emit llvm-ir
cargo run -p omni_cli -- compile examples/sine.omni --emit obj
```

Cross-target IR and object emission is also supported:

```bash
cargo run -p omni_cli -- compile examples/sine.omni --target-spec ./targets/arm64.toml --emit obj
```

### `omni render`

Renders an Omni program to a WAV file offline.

```bash
cargo run -p omni_cli -- render examples/sine.omni --output ./omni_out.wav --dur 5
```

Useful flags:
- `--output`
- `--dur`
- `--sample-rate`
- `--block`
- `--dump-graph`
- `--ir`
- `--fast-math`

### `omni preview`

Opens the standalone patch preview window.
This is the interactive path for listening to a patch, tweaking params, and inspecting buffers/devices.

```bash
cargo run -p omni_cli -- preview examples/sine.omni
```

Useful flags:
- `--sample-rate`
- `--block`
- `--fast-math`
- `--input-device`
- `--output-device`

### `omni preview play`

Runs the real-time playback/control transport without opening the standalone UI.
This is what editor integrations use under the hood.

```bash
cargo run -p omni_cli -- preview play examples/sine.omni --dur 2
cargo run -p omni_cli -- preview play examples/sine.omni --forever
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
cargo run -p omni_cli -- preview render examples/sine.omni --output ./omni_out.wav --dur 5 --set freq=220
```

### `omni lsp`

Starts the Omni language server over stdio.

```bash
cargo run -p omni_cli -- lsp
```

Current LSP support includes:
- open, change, save, and close document tracking
- diagnostics on open and save
- semantic tokens for constants, params, ports, and init-scoped variables

### `omni daemon diagnose`

Runs daemon-backed analysis for a file and reports diagnostics.

```bash
cargo run -p omni_cli -- daemon diagnose examples/sine.omni
```

### `omni daemon stdio`

Starts the daemon control transport over stdio.
This is intended for tool/editor integration rather than everyday manual use.

```bash
cargo run -p omni_cli -- daemon stdio
```

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

## Main components

- `crates/omni_frontend`: parser, AST, diagnostics
- `crates/omni_semantics`: semantic analysis and lowering rewrites
- `crates/omni_codegen_llvm`: LLVM lowering and ORC JIT backend
- `crates/omni_runtime`: runtime instance and processing APIs
- `crates/omni_api`: C ABI
- `crates/omni_daemon`: analysis and preview session engine
- `crates/omni_cli`: CLI, LSP adapter, and preview control transport
- `editors/vscode`: VSCode extension
- `editors/nvim`: Neovim runtime support
- `examples/`: Omni example programs

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

## Short Omni example

For the actual language reference, see [SYNTAX.md](SYNTAX.md).
As a minimal example:

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
