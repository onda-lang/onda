---
title: CLI and editor tools
description: Compile, run, render, diagnose, and edit Onda programs.
permalink: /docs/tooling/
section: learn
eyebrow: Tools
---

# CLI and editor tools

The `onda` executable is the center of the toolchain. It checks and compiles source, runs real-time audio, renders offline, exposes daemon services, and starts the language server.

## Compile and inspect

Check a file:

```bash
onda compile examples/foundations/sine.onda
```

Inspect graph lowering or generated LLVM IR:

```bash
onda compile examples/processors-and-graphs/proc_gain_graph.onda --dump-graph
onda compile examples/foundations/sine.onda --emit llvm-ir
```

Emit a native object for ahead-of-time integration:

```bash
onda compile examples/foundations/sine.onda --emit obj
```

Target presets can describe cross-target code generation:

```bash
onda compile examples/foundations/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

## Compile a complete WebAssembly module

The separately packaged WebAssembly compiler produces a self-contained core-Wasm processor and its
descriptor without LLVM or a linker:

```bash
npx onda-wasm compile examples/foundations/sine.onda \
  --root . \
  --output ./sine.wasm
```

The same `@onda-lang/wasm-compiler` package exposes asynchronous single-source and virtual-project
APIs for Node.js and browsers, including a worker-backed browser mode. The lower-level
`@onda-lang/binaryen-web` package remains available to tools that already produce compatible MIR.

## Real-time playback

The standalone UI watches the source program and provides controls for its exposed surface:

```bash
onda run examples/foundations/sine.onda
```

The native egui host is the default. Pass `--webview` to select the webview host. Common options select the sample rate, block size, audio devices, and color theme.

For playback without the standalone UI:

```bash
onda run play examples/foundations/sine.onda --dur 2
onda run play examples/foundations/sine.onda --forever --set freq=220
```

## Offline rendering

Render through the run pipeline to a WAV file:

```bash
onda run render examples/foundations/sine.onda \
  --output ./onda_out.wav \
  --dur 5 \
  --set freq=220
```

Offline rendering is a good default for automated comparisons and patches that do not need live input.

## Diagnostics and services

Run daemon-backed analysis once:

```bash
onda daemon diagnose examples/foundations/sine.onda
```

`onda daemon stdio` starts the JSON control transport for editor and tool integrations. `onda lsp` starts the language server over stdio.

Current language-server features include document synchronization, immediate and debounced diagnostics, and semantic tokens for important program symbols.

## Editor support

### VS Code

The [Onda VS Code extension](https://github.com/onda-lang/onda-vscode) registers `.onda` and `.on` files, connects to `onda lsp`, and provides **Onda: Run File** for launching the webview run interface inside the editor.

### Neovim

The [Onda Neovim plugin](https://github.com/onda-lang/onda-nvim) provides filetype detection, built-in LSP setup through `onda lsp`, and `:OndaRunFile` for launching the standalone run window.

## Embedding Onda

The public C interface lives in [`include/onda.h`](https://github.com/onda-lang/onda/blob/main/include/onda.h). It exposes compiler, instance, process, parameter, buffer, event, metadata, and state operations for non-Rust hosts.

Use the pre-built shared and static libraries or build them from source with:

```bash
cargo build -p onda_api --release
```

For implementation details and crate entry points, continue to the [architecture guide]({{ '/docs/architecture/' | relative_url }}).
