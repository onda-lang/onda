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
onda compile examples/basic/sine.onda
```

Inspect graph lowering or generated LLVM IR:

```bash
onda compile examples/feedback/cybernetic_feedback_graph.onda --dump-graph
onda compile examples/basic/sine.onda --emit llvm-ir
```

Emit a native object for ahead-of-time integration:

```bash
onda compile examples/basic/sine.onda --emit obj
```

Target presets can describe cross-target code generation:

```bash
onda compile examples/basic/sine.onda --target-spec ./targets/arm64.toml --emit obj
```

## Compile a complete WebAssembly module

The separately packaged WebAssembly compiler produces a self-contained core-Wasm processor and its
descriptor without LLVM or a linker:

```bash
npx onda-wasm compile examples/basic/sine.onda \
  --root . \
  --output ./sine.wasm
```

The same `@onda-lang/wasm-compiler` package exposes asynchronous single-source and virtual-project
APIs for Node.js and browsers, including a worker-backed browser mode. The lower-level
`@onda-lang/binaryen-web` package remains available to tools that already produce compatible MIR.
Use `inspectSourceConstants`, `inspectWorkspaceConstants`, or `inspectProjectImageConstants` to
resolve exposed `config const` declarations before compilation. Each accepts the same optional
context and partial constant overrides as its corresponding compile operation, and returns ordinary
JavaScript scalar values or typed arrays.

## Real-time playback

The standalone UI watches the entry plus every transitive non-standard-library import/include and
provides controls for the program's exposed surface. When opened from an `.ondaproject` file, it
also watches that manifest, its declared entry, and its file-backed buffer assets. Unresolved source
dependencies and missing project files remain watchable for recovery, and a failed parse preserves
the previous dependency watch set. Partial platform-watcher coverage falls back to targeted disk
validation.

Filesystem-backed entries, imports/includes, project manifests, and project assets must not
traverse symbolic links. Loading fails with an error identifying the offending path component;
use regular files and directories for editable, watched inputs.

```bash
onda run examples/basic/sine.onda
```

The native egui host is the default. Pass `--webview` to select the webview host. Common options select the sample rate, block size, audio devices, and color theme.

For playback without the standalone UI:

```bash
onda run play examples/basic/sine.onda --dur 2
onda run play examples/basic/sine.onda --forever --set freq=220
```

Buffer bindings are optional. Omitted scalar buffers and individual fixed-array slots use neutral
one-frame storage, so they read as zero and discard writes without preventing playback. Array slots
use their physical names at the CLI, for example `--buffer 'bank[3]=snare.ondabuffer'`.

## Offline rendering

Render through the run pipeline to a WAV file:

```bash
onda run render examples/basic/sine.onda \
  --output ./onda_out.wav \
  --dur 5 \
  --set freq=220
```

Offline rendering is a good default for automated comparisons and patches that do not need live input.

## Diagnostics and services

Run daemon-backed analysis once:

```bash
onda daemon diagnose examples/basic/sine.onda
```

`onda daemon stdio` starts the JSON control transport for editor and tool integrations. `onda lsp` starts the language server over stdio.

Current language-server features include document synchronization; immediate and debounced
diagnostics; context-aware declaration, call, member, parameter-domain, and integer-binding-range
completion; hover, go-to-definition, and signature help; and semantic tokens for important program
symbols and domain metadata. Unchecked `read_unsafe` / `write_unsafe` operations are offered only on
known compatible receivers where possible, with read/write direction reflected in member
completion; their free-call forms remain available as intrinsic completion. Hover and signature
help show the memory-safety contract.

Runtime diagnostics and dispatch use the same language-aware surfaces: `print` completion, hover,
and signature help describe its optional label and variadic printable scalar values; events and
delegates expose their typed call signatures; and `when` targets, inferred bindings, owner entries,
and nested locals participate in completion, hover, navigation, document symbols, and semantic
tokens. Incomplete `when` bodies retain their handler and owner scopes for semantic highlighting.

## Editor support

### VS Code

The [Onda VS Code extension](https://github.com/onda-lang/onda-vscode) registers `.onda` and `.on` files, connects to `onda lsp`, and provides **Onda: Run File** for launching the webview run interface inside the editor.

### Neovim

The [Onda Neovim plugin](https://github.com/onda-lang/onda-nvim) provides filetype detection, built-in LSP setup through `onda lsp`, and `:OndaRunFile` for launching the standalone run window.

## Embedding Onda

The public C interface lives in [`include/onda.h`](https://github.com/onda-lang/onda/blob/main/include/onda.h). It exposes compiler, instance, process, parameter, buffer, event, metadata, and state operations for non-Rust hosts.

Its lifetime verbs encode ownership. `*_destroy` consumes an opaque Onda handle, while `*_dispose`
releases Onda-owned members inside a caller-allocated value and resets that value. `*_reset` clears
reusable results without releasing their caller-owned storage. `alloc` and `free` appear only as raw
host allocator callbacks; memory returned by Onda must always go through its matching destroy or
dispose function rather than the C runtime `free`.

### Compile-time configuration from C

Every C compilation entry point accepts immutable `config const` selections through
`onda_compile_options_t.const_inputs`. The matching inspection entry point resolves the exposed
declarations under the same sample rate, block size, and optional partial selection:

```c
onda_compile_options_t options = {
  .sample_rate = 48000.0f,
  .block_size = 128,
};
onda_diag_t diag = {0};
onda_compile_constants_t* constants =
  onda_inspect_compile_constants_file("processor.onda", &options, NULL, &diag);
if (!constants) {
  /* diag.message describes the failure. Handle it and stop here. */
  return 1;
}

int count = onda_compile_constants_count(constants);
for (int i = 0; i < count; ++i) {
  const onda_compile_const_descriptor_t* descriptor =
    onda_compile_constants_at(constants, i);
  /* descriptor->input is already encoded as a valid compile input. */
}

options.const_inputs = onda_compile_constants_inputs(constants);
options.const_input_count = (size_t)count;
onda_program_t* program = onda_compile_file("processor.onda", &options, NULL, &diag);
onda_compile_constants_destroy(constants);
```

The returned contiguous input array reproduces the exact inspected selection and can be passed
directly to compilation while the descriptor handle remains alive. To change values, copy the
desired descriptor inputs into a partial selection, point `values` at the desired native C
values, and inspect again.

For example, this changes a scalar `f32` constant named `TEST` while leaving `YOYO` and every other
declaration at its authored value:

```c
float test = 0.5f;
onda_compile_const_input_t override = {
  .name_utf8 = "TEST",
  .element_type = ONDA_PRIMITIVE_F32,
  .is_array = 0,
  .element_count = 1,
  .values = &test,
};
options.const_inputs = &override;
options.const_input_count = 1;
```

The resulting descriptors contain defaults and fixed-array lengths resolved under those changes and
their contiguous input array can compile that exact variant. This second inspection matters when one
configuration constant affects another declaration's initializer or shape.

Values are ordinary native C values: `uint8_t` for booleans, `int32_t`, `int64_t`, `float`, or
`double`. Scalar entries point to one value, while arrays point to contiguous values and carry their
resolved element count.

Equivalent inspection functions are available for source strings, exact in-memory source graphs,
and project images.

Use the pre-built shared and static libraries or build them from source with:

```bash
cargo build -p onda_api --release
```

CMake hosts can consume either library without reproducing platform-specific
link requirements:

```cmake
find_package(Onda CONFIG REQUIRED)
target_link_libraries(my_host PRIVATE Onda::Static)
# Or:
target_link_libraries(my_host PRIVATE Onda::Shared)
```

Use `CMAKE_PREFIX_PATH` for an extracted release SDK, or set `Onda_DIR` to the
source checkout's `cmake` directory. `Onda::Static` carries the required system
libraries and, on Linux and macOS, hides the embedded Rust and LLVM implementation
symbols from the consumer's dynamic ABI. `Onda::Shared` links against the
shared-library import target and does not inherit those static-only options. On
Linux its SONAME is `libonda.so`, and on macOS its install name is
`@rpath/libonda.dylib`. The consuming application controls the runtime search
path and final shared-library placement.

When using `Onda::Shared`, deploy `onda.dll` where the Windows loader can find
it, deploy `libonda.so` in the application's configured ELF search path, or
place `libonda.dylib` at a location covered by the application's macOS
`LC_RPATH`. `Onda::Static` avoids this runtime deployment step.
