# Onda embedded-compiler playground

This example demonstrates embedding the Onda compiler in a browser application:

```text
editable Onda source
  -> @onda-lang/wasm-compiler module worker + embedded stdlib
  -> validated schema-5 MIR MessagePack
  -> packaged Binaryen backend + Binaryen.js
  -> DSP Wasm + metadata
  -> @onda-lang/webaudio AudioWorklet
```

The page consumes the public `@onda-lang/wasm-compiler` and `@onda-lang/webaudio` APIs. It contains
a source editor, structured compiler diagnostics, sample-rate and compile-block settings,
metadata-generated parameter and event controls, reusable artifact export, DSP reset, master gain,
and live audio. Rust semantic compilation and Binaryen O4 optimization run through the compiler
package's module worker, so compilation does not block editor interaction.
The source is compiled in the browser; there is no compiler service, native Onda CLI, LLVM, or
`wasm-ld` in the runtime path.

For an application that ships only an ahead-of-time compiled processor, see the separate
[`onda_wasm_aot_sample_player`](../onda_wasm_aot_sample_player/README.md) example. That page contains
neither the Onda compiler nor Binaryen.

## Prerequisites

- Node.js and npm
- `wasm-pack`
- a browser with WebAssembly, JavaScript modules, and AudioWorklet support

`wasm-pack` is a build-time requirement because the Rust browser compiler must be packaged as Wasm.
After the build, the demo consists of static files. Local development can use HTTP on localhost;
non-local AudioWorklet hosting normally requires HTTPS because it must run in a secure context.

## Build and run

Windows PowerShell:

```powershell
.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve
```

macOS/Linux:

```bash
bash ./examples/web/onda_wasm_playground/build-demo.sh --serve
```

Open `http://127.0.0.1:8787/`. Edit the source and select **Compile** (or press
Ctrl/Cmd+Enter), then select **Start audio**. Browser autoplay rules require that playback begin
from a user gesture.

Without `-Serve`/`--serve`, the scripts only prepare the static assets:

```powershell
.\examples\web\onda_wasm_playground\build-demo.ps1
```

```bash
bash ./examples/web/onda_wasm_playground/build-demo.sh
```

The scripts:

- install the compiler package's pinned `binaryen` dependency when it is missing
- run the `@onda-lang/wasm-compiler` package build, including its release `wasm-pack build --no-opt`
- stage the compiler and Web Audio packages behind the import map used by this static example
- optionally start `server.mjs` on `127.0.0.1:8787`

Sample rate and compile block size are editor controls, not build-script flags.

If PowerShell script execution is blocked:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\examples\web\onda_wasm_playground\build-demo.ps1 -Serve
```

## Verification

The product compiler tests live in `packages/onda_wasm_compiler`; lower-level backend tests live in
`packages/onda_binaryen_web`; adapter API tests live in `packages/onda_webaudio`:

```bash
cd packages/onda_wasm_compiler
npm install
npm test
cd ../onda_binaryen_web
npm install
npm test
npm run test:onda
cd ../onda_webaudio
npm test
```

The packaged-layout smoke test is also available separately:

```bash
cd packages/onda_wasm_compiler
npm run test:pack
```

`npm test` in the compiler package builds the source-to-Wasm product API and tests source, project,
diagnostic, and worker behavior. The backend's `npm test` covers schema-5 lowering, the internal Wasm
math kernel, and AudioWorklet behavior. Its `npm run test:onda` compiles real Onda source, compares
Binaryen renders with the native LLVM/MIR path, and runs the FMA oracle. `npm run test:parity` runs
only the LLVM/Binaryen differential render suite. The source-driven/parity commands require a
working native Rust/LLVM Onda build; the demo asset build and ordinary JavaScript tests do not.

```bash
cd packages/onda_binaryen_web
npm run bench
```

This runs the reproducible development comparison documented in
[`docs/BACKEND_BENCHMARKS.md`](../../../docs/BACKEND_BENCHMARKS.md) and requires the native
Rust/LLVM Onda build.

For a manual browser smoke test, edit the default program, compile it, start audio, move a generated
parameter control, trigger a declared event if the edited source has one, and reset the DSP.

## Host behavior

The worklet's persistent compile-block cursor decouples Web Audio render-quantum size from the MIR
compile-time block size. Each callback is split into legal `(start_frame, frames, flags)` segments;
a compile block may span callbacks, and one callback may cross several compile-block boundaries
without resetting DSP state.

The `@onda-lang/webaudio` adapter registers the worklet module before constructing the node, derives
channel options from metadata, and provides request-correlated helpers. Parameters are
initialized from generated metadata and updated with
`{ type: "set-param", param: "name", value }`. Scalar and fixed-array parameter types are written
according to their metadata rather than by a name-specific convention. `{ type: "reset" }` clears
physical state, resets the compile-block cursor, and runs `onda_init` again.

The generic event/control-output ABI accepts
`{ type: "event", event: "event_name", values: { param: value } }`; scalar, fixed-array, and slice
payloads are packed from compiler metadata and dispatched through the generated `onda_event_N`
export. `{ type: "read-control-outputs" }` produces a
`{ type: "control-outputs", values }` reply. ABI errors are returned as `onda-error` messages rather
than terminating the processor. Portable packed state can be requested and restored through
`snapshot` and `restore-snapshot`; restore starts from a fresh post-init physical image.

Declared external buffers are supplied in `processorOptions.buffers`, keyed by Onda name. Each
binding has `{ data, frames, channels, sampleRate }`; `data` uses the native interleaved frame
layout. Mono/static channel constraints are checked against compiler metadata.
`{ type: "read-buffer", buffer: "name" }` returns current contents, including writes from Onda.

## Current limitations

- The playground UI edits one source file. `onda_compiler_web` supports virtual multi-file projects,
  but that API is not exposed in this page yet.
- Recompiling while audio is active stops and recreates the AudioContext/processor, so DSP state is
  initialized rather than migrated or crossfaded.
- Declared external buffers receive zero-filled defaults; the page does not yet provide file loading
  or buffer inspection controls.
- The worklet can return control-output values, but the page does not yet render them.
- The page does not connect a microphone or other source to its AudioWorklet input, so programs with
  audio inputs currently receive silence.
- The page rejects more than 32 flattened input or output channels because of Web Audio node limits.
- The page requests the selected sample rate from `AudioContext`, but it does not currently
  recompile if a browser chooses a different actual rate.
- Exact f32/f64 FMA is linked into the DSP module from the pure-Wasm math kernel. It preserves
  one-rounding behavior without JavaScript work on the audio thread, but can remain slower than
  native hardware FMA in dense sample loops.
- The compiler and Web Audio packages are staged as development assets; production hosting should add
  compression, caching, versioned URLs, and broader Chromium/Firefox/Safari audio coverage.
- The current browser smoke endpoint verifies source-to-Wasm page compilation, not automated audible
  AudioWorklet output; browser-audio automation remains a roadmap item.
