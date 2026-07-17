<h1>
  <img src="assets/svg/onda-logo-dark.svg" alt="onda logo" width="40" align="absmiddle" /> Onda 
</h1>

Onda is an expressive and performant JIT-compiled audio programming language.

This repository provides the compiler, runtime, CLI, LSP and a C API for embedding the JIT compiler.
The production compiler lowers fully resolved semantics to one validated, backend-neutral MIR:
native hosts consume schema-5 MIR through LLVM/ORC or AOT object emission, while the browser-safe
compiler produces the same schema in memory for the Binaryen.js WebAssembly backend. The checked-in
browser playground provides a source editor, structured diagnostics, generated parameter/event
controls, and AudioWorklet playback without a compiler service.

Visit the project's [website](https://onda-lang.github.io/onda) for an introduction to the language.

## Code example

Here is a code example for a saw oscillator processed by a resonant filter and an oversampled saturator:

```onda
import std/osc
import std/filter

params:
  freq = 110.0 {20.0, 880.0}
  cutoff = 1200.0 {40.0, 12000.0}
  resonance = 1.0 {0.1, 8.0}
  drive = 1.0 {1.0, 10.0}

def soft_clip(x):
  return tanh(x)

proc Saturator:
  params:
    amount = 1.0

  sample 4:
    out1 = soft_clip(in1 * amount)

init:
  osc = std::osc::Saw()
  filter = std::filter::Svf(cutoff = cutoff, q = resonance)
  saturator = Saturator()

block:
  filter.update_coeffs(cutoff, resonance)

  sample:
    tone = osc(freq = freq)
    out1 = saturator(filter(tone), amount = drive)
```

Take a look at the `examples/` folder for more usage examples.

## Documentation

- [docs/SYNTAX.md](docs/SYNTAX.md): language syntax and semantics
- [docs/INFO.md](docs/INFO.md): project structure and implementation notes
- [docs/MIR.md](docs/MIR.md): backend-neutral MIR and browser-backend boundary
- [docs/BACKEND_BENCHMARKS.md](docs/BACKEND_BENCHMARKS.md): reproducible LLVM/Binaryen compile and render comparison
- [crates/onda_compiler_web](crates/onda_compiler_web/README.md): in-browser Onda source-to-MIR compiler API
- [packages/onda_binaryen_web](packages/onda_binaryen_web/README.md): schema-5 MIR-to-Wasm backend
- [examples/web/onda_wasm_playground](examples/web/onda_wasm_playground/README.md): editable browser playground and AudioWorklet host

## Browser playground

The browser demo compiles edited Onda source entirely on the client:

```text
Onda source -> compiler Wasm -> schema-5 MIR MessagePack -> Binaryen.js -> DSP Wasm -> AudioWorklet
```

Preparing its static assets requires Node/npm and `wasm-pack`:

```bash
bash ./examples/web/onda_wasm_playground/build-demo.sh --serve
```

PowerShell:

```powershell
.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve
```

Then open `http://127.0.0.1:8787/`. The build does not require the native `onda` CLI or LLVM, but
`wasm-pack` is required to build the browser compiler. See the demo README for test commands and
current limitations.

## Precompiled releases

[GitHub Releases](https://github.com/onda-lang/onda/releases/latest) provides precompiled packages for Linux x64, macOS arm64, and Windows x64. Each package includes the CLI, static and shared C libraries, the public header, language guide and examples.

## The `onda` CLI

The CLI surface is:

```text
onda compile <input.onda>
onda lsp
onda run <input.onda> [--theme <auto|dark|light>]
onda run play <input.onda>
onda run render <input.onda>
onda daemon diagnose <input.onda>
onda daemon stdio
```

For a full list of all commands and their flags, run the help file via `onda --help`.

### `onda compile`

Compiles an Onda file and optionally emits IR or an object file.

Typical uses:
- syntax and semantic checking
- inspect graph lowering with `--dump-graph`
- emit backend-neutral MIR for inspection with `--emit mir`, versioned JSON with `--emit mir-json`, or compact production transport with `--emit mir-messagepack`
- emit LLVM IR with `--emit llvm-ir` or `--ir`
- emit a native object file with `--emit obj`

Examples:

```bash
onda compile examples/foundations/sine.onda
onda compile examples/foundations/sine.onda --emit mir
onda compile examples/foundations/sine.onda --emit mir-json --output sine.mir.json
onda compile examples/foundations/sine.onda --emit mir-messagepack --output sine.mir.msgpack
onda compile examples/processors-and-graphs/proc_gain_graph.onda --dump-graph
onda compile examples/foundations/sine.onda --emit llvm-ir
onda compile examples/foundations/sine.onda --emit obj
onda compile examples/foundations/sine.onda --target-triple aarch64-unknown-linux-gnu --emit obj
```

Cross-target IR and object emission is also supported:

```bash
onda compile examples/foundations/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

### `onda run`

Opens the standalone UI window.
This can be used to interactively play with the live-running code.

```bash
onda run examples/foundations/sine.onda
```

Run host selection:
- egui is the default run host
- `--webview` selects the webview run host explicitly

Useful flags:
- `--sample-rate`
- `--block-size`
- `--input-device`
- `--output-device`
- `--theme`

### `onda run play`

Runs the real-time playback/control transport without opening the standalone UI.
Parameters can be set via the `--set` argument.

```bash
onda run play examples/foundations/sine.onda --dur 2
onda run play examples/foundations/sine.onda --forever --set freq=220
```

Useful flags:
- `--dur` or `--forever`
- `--sample-rate`
- `--block-size`
- `--input-device`
- `--output-device`
- `--set name=value`

With `--control-json`, `onda run play` prints a control handshake on stdout and serves a localhost control socket for run clients.

### `onda run render`

Offline render through the run pipeline.
This is useful when you want to render out to a wav file without running real-time playback.

```bash
onda run render examples/foundations/sine.onda --output ./onda_out.wav --dur 5 --set freq=220
```

Useful flags:
- `--output`
- `--dur`
- `--sample-rate`
- `--block-size`
- `--set name=value`

### `onda daemon diagnose`

Runs daemon-backed analysis for a file and reports diagnostics.

```bash
onda daemon diagnose examples/foundations/sine.onda
```

### `onda daemon stdio`

Starts the daemon control transport over stdio.
This is intended for tool/editor integration rather than everyday manual use.

```bash
onda daemon stdio
```

### `onda lsp`

Starts the Onda language server over stdio.

```bash
onda lsp
```

Current LSP support includes:
- open, change, save, and close document tracking
- immediate diagnostics on open/save and debounced diagnostics while editing
- semantic tokens for constants, params, ports, and init-scoped variables

## Building `onda` from source

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

## Editor support

### VSCode

The VSCode extension lives in the standalone [`onda-lang/onda-vscode`](https://github.com/onda-lang/onda-vscode) repository.
It provides:
- `.onda` and `.on` language registration
- builtin LSP through `onda lsp`
- `Onda: Run File` for launching the `onda run --webview` UI directly in-editor

### Neovim

The Neovim plugin lives in the standalone [`onda-lang/onda-nvim`](https://github.com/onda-lang/onda-nvim) repository.
It provides:
- `.onda` and `.on` filetype detection
- builtin LSP through `onda lsp`
- `:OndaRunFile` for launching the standalone run window
