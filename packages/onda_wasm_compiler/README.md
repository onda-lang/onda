# Onda WebAssembly compiler

`@onda-lang/wasm-compiler` compiles Onda source or an in-memory multi-file project to a complete,
self-contained WebAssembly processor artifact. It runs in modern browsers and Node.js and does not
require LLVM, a Wasm linker, Rust, or `wasm-pack` after installation.

```js
import { createCompiler } from "@onda-lang/wasm-compiler";

const compiler = await createCompiler();
const artifact = await compiler.compileSource(source, {
  sampleRate: 48_000,
  blockSize: 128,
});

console.log(artifact.wasm, artifact.metadata);
```

The package composes Onda's embedded Rust frontend with its Binaryen backend. The frontend emits
validated versioned MIR in memory; the backend lowers that trusted producer output to the generic
Onda WebAssembly processor ABI. The package verifies the MIR schema handshake during startup.

## Projects

Project compilation resolves imports and includes entirely from the supplied source map and the
embedded standard library:

```js
const artifact = await compiler.compileProject({
  entry: "main.onda",
  sources: {
    "main.onda": mainSource,
    "dsp/filter.onda": filterSource,
  },
}, {
  sampleRate: 48_000,
  blockSize: 128,
});
```

Compilation failures throw `OndaCompileError`. Its `diagnostics` property contains structured
parse, semantic, MIR, configuration, or code-generation diagnostics.

## Browser workers

Compilation is CPU-intensive. Use the built-in worker client in interactive browser applications:

```js
const compiler = await createCompiler({ worker: true });
const artifact = await compiler.compileSource(source, options);
await compiler.dispose();
```

In worker mode the page-side entry point stays lightweight; the Rust frontend Wasm, Binaryen, and
the MIR backend are loaded only inside the worker.

The package also exports `@onda-lang/wasm-compiler/worker` for hosts that want to own the worker
protocol directly.

## CLI

The npm package installs `onda-wasm`:

```sh
onda-wasm compile ./main.onda --root . --output ./dist/main.wasm
```

The command recursively collects `.onda` files below `--root`, compiles the requested entry, and
writes an integrity-associated `main.wasm` plus `main.onda.json` descriptor. Use `--help` for code
generation and output options.

## Artifact hosting

The compiler returns the generic processor artifact. Web Audio is optional; use
`@onda-lang/webaudio` when an `AudioWorklet` host is desired. Build-time users should publish only
the generated `.wasm` and `.onda.json`, not this compiler package or Binaryen.

The low-level `@onda-lang/binaryen-web` package remains available for advanced consumers that
already produce compatible Onda MIR.

## Versioning

`[workspace.package].version` in the repository's top-level `Cargo.toml` is the only authored Onda
version. `scripts/sync-package-versions.mjs` updates the workspace-owned `Cargo.lock` entries,
discovers every `@onda-lang/*` package, and synchronizes its manifest and lockfile. Compiler builds
and release jobs run it automatically; `ONDA_VERSION` is generated into the packaged distribution.
