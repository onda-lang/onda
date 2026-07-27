# Onda WebAssembly compiler

`@onda-lang/wasm-compiler` compiles Onda source or an in-memory multi-file project to a complete,
self-contained WebAssembly processor artifact. It runs in modern browsers and Node.js and does not
require LLVM, a Wasm linker, Rust, or `wasm-pack` after installation.

```js
import { createCompiler } from "@onda-lang/wasm-compiler";

const compiler = await createCompiler();
const { artifact, sourceFiles } = await compiler.compileSource(source, {
  sampleRate: 48_000,
  blockSize: 128,
});

console.log(artifact.wasm, artifact.metadata, sourceFiles);
```

The package composes Onda's embedded Rust frontend with its Binaryen backend. The frontend emits
validated versioned MIR in memory; the backend lowers that trusted producer output to the generic
Onda WebAssembly processor ABI. The package verifies the MIR schema handshake during startup.

Release builds post-optimize the Rust frontend Wasm with the package's pinned Binaryen
`wasm-opt -O4`. The build records input/output sizes and the optimizer version in
`dist/build.json`, and fails if the pass does not reduce the shipped module. Generated DSP modules
are already optimized independently by the runtime Binaryen O4 pipeline.

## Projects

Project compilation resolves imports and includes entirely from the supplied source map and the
embedded standard library:

```js
const { artifact, sourceFiles } = await compiler.compileProject({
  entry: "main.onda",
  sources: {
    "main.onda": mainSource,
    "dsp/filter.onda": filterSource,
  },
}, {
  sampleRate: 48_000,
  blockSize: 128,
});

// Entry first, then transitive imports/includes. Embedded stdlib is excluded.
console.log(sourceFiles);
```

Compilation failures throw `OndaCompileError`. Its `diagnostics` property contains structured
parse, semantic, MIR, configuration, or code-generation diagnostics. Its `sourceFiles` property
contains every project source resolved before compilation stopped, allowing hosts to retain useful
watch registrations while the project is temporarily invalid. `unresolvedSourceFiles` contains
referenced non-standard-library candidates which were not present, allowing hosts to watch for their
creation without treating them as contributing compilation inputs.

## Browser workers

Compilation is CPU-intensive. Use the built-in worker client in interactive browser applications:

```js
const compiler = await createCompiler({ worker: true });
const { artifact, sourceFiles } = await compiler.compileSource(source, options);
await compiler.dispose();
```

Static hosts and bundlers may provide explicit `workerUrl` and `frontendWasm` URLs. The worker
receives the frontend URL during initialization, so versioned or content-hashed compiler assets do
not depend on the package's development directory layout.

In worker mode the page-side entry point stays lightweight; the Rust frontend Wasm, Binaryen, and
the MIR backend are loaded only inside the worker.

The package also exports `@onda-lang/wasm-compiler/worker` for hosts that want to own the worker
protocol directly.

## Browser LSP

The frontend Wasm contains the same transport-neutral language server used by `onda lsp`. Worker
clients send ordinary JSON-RPC/LSP messages and receive every response or notification emitted for
that message:

```js
await compiler.setLspAnalysisOptions({ sampleRate: 48_000, blockSize: 256 });

const [initialized] = await compiler.sendLspMessage({
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: { processId: null, capabilities: {} },
});

await compiler.sendLspMessage({
  jsonrpc: "2.0",
  method: "textDocument/didOpen",
  params: {
    textDocument: {
      uri: "file:///onda-project/main.onda",
      languageId: "onda",
      version: 1,
      text: source,
    },
  },
});
```

Open every virtual project file with `didOpen`; imports and includes resolve from those overlays and
the embedded standard library. Diagnostics, semantic tokens, completion, hover, definitions, and
document symbols use the native server implementation. The browser transport does not run MIR or a
backend until the host explicitly calls `compileSource` or `compileProject`.

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
version. Run `scripts/sync-versions.sh` or `scripts/sync-versions.ps1` after changing it.
The underlying `scripts/sync-package-versions.mjs` updates the workspace-owned `Cargo.lock` entries,
discovers every `@onda-lang/*` package, and synchronizes its manifest and lockfile. Compiler builds
and release jobs run it automatically; `ONDA_VERSION` is generated into the packaged distribution.
