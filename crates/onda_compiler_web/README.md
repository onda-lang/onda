# Onda browser compiler

`onda_compiler_web` is the browser-safe `source -> semantic analysis -> optimized validated MIR`
front half of Onda. The standard library is embedded in the Rust frontend, so compiling an
in-memory program and its `std/...` imports does not require filesystem access.

Build the production JavaScript glue and compiler Wasm from the repository root with:

```sh
npm run build --workspace @onda-lang/wasm-compiler
```

The package build runs `wasm-pack --release` and then the workspace-pinned Binaryen
`wasm-opt -O4`. Its internal `--no-opt` argument only disables wasm-pack's separately bundled
optimizer so every local, CI, website, and npm build uses the same Binaryen release.

The production exports `compile_to_mir_messagepack(source, sampleRate, blockSize)` and
`compile_project_to_mir_messagepack(entryPath, sourcesJson, sampleRate, blockSize)` return compact
schema-versioned bytes. JSON variants with the same names ending in `_json` remain available for
inspection and tooling. Project compilation accepts a JSON object of project-relative paths to
source strings, resolving imports and includes entirely in memory. Paths cannot escape the virtual
project root. `mir_schema_version()` exposes the producer version for integration checks.
Compilation failures reject with a JSON-encoded array of structured diagnostics.

The current producer and `packages/onda_binaryen_web` both use the current MIR schema. The browser playground
under `examples/web/onda_wasm_playground` passes the generated MessagePack directly to the explicitly
trusted Onda-producer entry point in the Binaryen.js backend and runs the resulting DSP Wasm in an
AudioWorklet.

Native MessagePack and JSON APIs exist for tests and non-JavaScript embedders. They use the identical
in-memory lowering and optimization path as the Wasm exports.

Run native tests with:

```sh
cargo test -p onda_compiler_web
```

Build and serve the complete browser path from the repository root with:

```sh
bash ./examples/web/onda_wasm_playground/build-demo.sh --serve
```

That demo build requires Node/npm and `wasm-pack`; it does not require LLVM or the native Onda CLI.
This crate deliberately stops at optimized validated MIR transport. DSP Wasm emission, internal
Wasm math legalization, runtime layout metadata, and AudioWorklet hosting live in the Binaryen
package and browser example. Backend
compile/render measurements are documented in
[`docs/BACKEND_BENCHMARKS.md`](../../docs/BACKEND_BENCHMARKS.md).

Application consumers should normally install
[`@onda-lang/wasm-compiler`](../../packages/onda_wasm_compiler/README.md), which packages this
frontend Wasm with the compatible Binaryen backend, verifies their MIR schema handshake, and
provides typed source/project APIs plus the `onda-wasm` CLI.
