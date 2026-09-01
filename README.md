<h1>
  <img src="assets/svg/onda-logo-dark.svg" alt="onda logo" width="40" align="absmiddle" /> Onda 
</h1>

Onda is an expressive and performant JIT-compiled audio programming language.

Visit the project's [website](https://onda-lang.org) to try the web compiler and get an introduction to the language.

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


## Precompiled releases

[GitHub Releases](https://github.com/onda-lang/onda/releases/latest) provides precompiled packages
for Linux x64, macOS arm64, and Windows x64. Each package includes the CLI, an Onda Run application
entry, static and shared C libraries, public headers, hosted and raw processor API references,
language guide, standard-library reference, and examples. The Linux archive includes `install.sh`
and `uninstall.sh` for a per-user CLI and desktop installation.

Tagged releases attach portable tarballs and publish the four public npm packages:
`@onda-lang/processor-abi`, `@onda-lang/binaryen-web`, `@onda-lang/webaudio`, and `@onda-lang/wasm-compiler`.

## Documentation

- [CHANGELOG.md](CHANGELOG.md): release history and migration notes
- [docs/syntax.md](docs/syntax.md): language syntax and semantics
- [docs/stdlib.md](docs/stdlib.md): generated standard-library API reference
- [docs/architecture.md](docs/architecture.md): compiler architecture and codebase navigation
- [docs/projects.md](docs/projects.md): project manifests, typed buffer assets, and project images
- [docs/mir.md](docs/mir.md): backend-neutral MIR and backend boundary
- [docs/api.md](docs/api.md): complete hosted C API and library integration reference
- [docs/processor-abi.md](docs/processor-abi.md): raw processor-object API, ABI, and target profiles
- [docs/web-api.md](docs/web-api.md): complete JavaScript and WebAssembly package API
- [packages/onda_wasm_compiler](packages/onda_wasm_compiler/README.md): packaged source-to-WebAssembly compiler and `onda-wasm` CLI
- [packages/onda_binaryen_web](packages/onda_binaryen_web/README.md): current MIR-to-Wasm backend
- [packages/onda_processor_abi](packages/onda_processor_abi/README.md): compiler-free artifact schema, validation, and integrity helpers
- [packages/onda_webaudio](packages/onda_webaudio/README.md): optional reusable Web Audio adapter
- [examples/web/onda_wasm_playground](examples/web/onda_wasm_playground/README.md): editable embedded-compiler playground
- [examples/native/raw_processor_object](examples/native/raw_processor_object/README.md): link and call a native relocatable processor object directly

## The `onda` CLI

The CLI surface is:

```text
onda
onda compile <input>
onda run [input] [--theme <auto|dark|light>]
onda run play <input>
onda run render <input>
onda project <directory> [--from <input.onda>] [--buffer <name=path>]
onda daemon diagnose <input>
onda daemon stdio
onda lsp
```

`<input>` may be an `.onda` source file or an `.ondaproject` project file.

For a full list of all commands and their flags, run the help file via `onda --help`.

### `onda compile`

Compiles an Onda file and optionally emits IR or an object file.

Typical uses:
- syntax and semantic checking
- inspect graph lowering with `--dump-graph`
- select explicitly declared compile-time variants with repeatable `--const Name=value`, or inspect
  their resolved types and defaults with `--list-consts`
- emit backend-neutral MIR for inspection with `--emit mir`, versioned JSON with `--emit mir-json`, or compact production transport with `--emit mir-messagepack`
- emit LLVM IR with `--emit llvm-ir` or `--ir`
- emit a native object file with `--emit obj`

Examples:

```bash
onda compile examples/basic/sine.onda
onda compile examples/basic/sine.onda --emit mir
onda compile examples/basic/sine.onda --emit mir-json --output sine.mir.json
onda compile examples/basic/sine.onda --emit mir-messagepack --output sine.mir.msgpack
onda compile examples/feedback/cybernetic_feedback_graph.onda --dump-graph
onda compile program.onda --const Channels=8 --const 'Window=[0.0, 0.5, 1.0]' --emit mir
onda compile program.onda --list-consts
onda compile examples/basic/sine.onda --emit llvm-ir
onda compile examples/basic/sine.onda --emit obj
onda compile examples/basic/sine.onda --target-triple aarch64-unknown-linux-gnu --emit obj
```

Cross-target IR and object emission is also supported:

```bash
onda compile examples/basic/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

### `onda run`

Opens the standalone UI window.
This can be used to interactively play with the live-running code.
Running `onda` without a command opens the same window with its file picker.
The empty view lets you choose the sample rate and compile block size before loading a file. Use
**Unload** to return there and recompile with different settings.
Loaded filesystem sources and projects reload when their entry, transitive source dependencies,
manifest, or file-backed assets change; unresolved and temporarily missing paths remain watchable.

```bash
onda
onda run examples/basic/sine.onda
```

Run host selection:
- egui is the default run host
- `--webview` selects the webview run host explicitly

Useful flags:
- `--sample-rate`
- `--block-size`
- `--input-device`
- `--output-device`
- `--midi-input-device`
- `--theme`

### `onda run play`

Runs the real-time playback/control transport without opening the standalone UI.
Parameters can be set via the `--set` argument.

```bash
onda run play examples/basic/sine.onda --dur 2
onda run play examples/basic/sine.onda --forever --set freq=220
```

Useful flags:
- `--dur` or `--forever`
- `--sample-rate`
- `--block-size`
- `--input-device`
- `--output-device`
- `--midi-input-device`
- `--set name=value`
- `--buffer name=path`

With `--control-json`, `onda run play` prints a control handshake on stdout and serves a localhost control socket for run clients.

### `onda run render`

Offline render through the run pipeline.
This is useful when you want to render out to a wav file without running real-time playback.

```bash
onda run render examples/basic/sine.onda --output ./onda_out.wav --dur 5 --set freq=220
```

Useful flags:
- `--output`
- `--dur`
- `--sample-rate`
- `--block-size`
- `--set name=value`
- `--buffer name=path`

### `onda project`

Creates an empty, editable Onda project folder with a project file named after the target,
`code/main.onda`, and an `assets` directory. With `--from`, it instead captures an existing source
graph and packages supplied buffers into canonical project assets.

```bash
onda project my-project
onda run my-project/my-project.ondaproject

onda project portable-sampler \
  --from sampler.onda \
  --buffer sample=recording.wav
```

### `onda daemon diagnose`

Runs daemon-backed analysis for a file and reports diagnostics.

```bash
onda daemon diagnose examples/basic/sine.onda
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
- syntax highlighting, hover, navigation and context-aware completion

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

Already-compiled native processor objects do not require `libonda`: include
`include/onda_processor_abi.h`, link the emitted object with the application, and call its raw
entrypoints directly. The paired JSON descriptor supplies storage sizes, alignments, defaults, and
interface layouts. See the native raw-object example for a complete link command.

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
