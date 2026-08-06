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
`compile_source_workspace_to_mir_messagepack(entryPath, sourcesJson, sampleRate, blockSize)` return a
`FrontendMessagePackCompilation` containing compact schema-versioned bytes plus the ordered
contributing project paths. JSON variants return the equivalent `FrontendJsonCompilation` for
inspection and tooling. Both result types expose the source list and exact portable source image.
Workspace
compilation accepts a JSON object of project-relative paths to source strings, resolving imports and
includes entirely in memory. Paths cannot escape the virtual project root. Embedded
standard-library modules are omitted from the source manifest. `mir_schema_version()` exposes the
producer version for integration checks. Compilation failures reject with a JSON-encoded object
containing structured diagnostics, the partially resolved source list, and unresolved
non-standard-library candidates which a host may watch for creation.

The Wasm surface also builds, loads, inspects, compiles, and materializes canonical `ProjectImage`
values, encodes/decodes every `.ondabuffer` primitive type, and decodes WAV files through the same
canonical buffer path as native hosts. These operations directly use `onda_project`, matching the
C API's bytes, digests, validation, and format capability values. Materialization publishes
`code/main.onda`, preserves meaningful source paths below `code/`, and writes typed assets below
`assets/`. Input file sets may contain `.ondaproject` manifests in any directory when the host
explicitly selects the one to load.

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
[`docs/backend-benchmarks.md`](../../docs/backend-benchmarks.md).

Application consumers should normally install
[`@onda-lang/wasm-compiler`](../../packages/onda_wasm_compiler/README.md), which packages this
frontend Wasm with the compatible Binaryen backend, verifies their MIR schema handshake, and
provides typed source-workspace and project-image APIs plus the `onda-wasm` CLI.
