# Onda Browser Playground

This example is the end-to-end browser path:

```text
editable Onda source
  -> onda_compiler_web Wasm + embedded stdlib
  -> validated schema-5 MIR MessagePack
  -> onda_binaryen_web + Binaryen.js
  -> DSP Wasm + metadata
  -> AudioWorklet
```

The page contains a source editor, structured compiler diagnostics, sample-rate and compile-block
settings, metadata-generated parameter and event controls, DSP reset, master gain, and live audio.
The source is compiled in the browser; there is no compiler service, native Onda CLI, LLVM, or
`wasm-ld` in the runtime path.

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
.\examples\web\sine_wasm_worklet\build-demo.ps1 -Serve
```

macOS/Linux:

```bash
bash ./examples/web/sine_wasm_worklet/build-demo.sh --serve
```

Open `http://127.0.0.1:8787/`. Edit the source and select **Compile** (or press
Ctrl/Cmd+Enter), then select **Start audio**. Browser autoplay rules require that playback begin
from a user gesture.

Without `-Serve`/`--serve`, the scripts only prepare the static assets:

```powershell
.\examples\web\sine_wasm_worklet\build-demo.ps1
```

```bash
bash ./examples/web/sine_wasm_worklet/build-demo.sh
```

The scripts:

- run `wasm-pack build` for `crates/onda_compiler_web` with the `web` target
- install the pinned `binaryen` npm dependency when it is missing
- stage the compiler package, Binaryen ESM file, Onda schema-5 backend, MessagePack decoder, and exact-math support
- optionally start `server.mjs` on `127.0.0.1:8787`

Sample rate and compile block size are editor controls, not build-script flags.

If PowerShell script execution is blocked:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\examples\web\sine_wasm_worklet\build-demo.ps1 -Serve
```

## Verification

The backend and host tests live in `packages/onda_binaryen_web`:

```bash
cd packages/onda_binaryen_web
npm install
npm test
npm run test:onda
```

`npm test` covers schema-5 lowering, exact math, and AudioWorklet behavior. `npm run test:onda`
compiles real Onda source, compares Binaryen renders with the native LLVM/MIR path, and runs the FMA
oracle. `npm run test:parity` runs only the LLVM/Binaryen differential render suite. The
source-driven/parity commands require a working native Rust/LLVM Onda build; the demo asset build
and `npm test` do not.

`npm run bench` runs the reproducible development comparison documented in
[`docs/BACKEND_BENCHMARKS.md`](../../../docs/BACKEND_BENCHMARKS.md) and requires the native
Rust/LLVM Onda build.

For a manual browser smoke test, edit the default program, compile it, start audio, move a generated
parameter control, trigger a declared event if the edited source has one, and reset the DSP.

## Host behavior

The worklet's persistent compile-block cursor decouples Web Audio render-quantum size from the MIR
compile-time block size. Each callback is split into legal `(start_frame, frames, flags)` segments;
a compile block may span callbacks, and one callback may cross several compile-block boundaries
without resetting DSP state.

Parameters are initialized from generated metadata and updated with
`{ type: "set-param", param: "name", value }`. Scalar and fixed-array parameter types are written
according to their metadata rather than by a name-specific convention. `{ type: "reset" }` clears
physical state, resets the compile-block cursor, and runs `onda_init` again.

The generic event/control-output ABI accepts
`{ type: "event", event: "event_name", values: { param: value } }`; scalar, fixed-array, and slice
payloads are packed from compiler metadata and dispatched through the generated `onda_event_N`
export. `{ type: "read-control-outputs" }` produces a
`{ type: "control-outputs", values }` reply. ABI errors are returned as `onda-error` messages rather
than terminating the processor.

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
- Rust compilation and Binaryen optimization run synchronously on the page's main thread, so larger
  programs can temporarily block editor interaction.
- The page requests the selected sample rate from `AudioContext`, but it does not currently
  recompile if a browser chooses a different actual rate.
- Exact f32/f64 FMA uses the `onda_exact_math_v1` BigInt import. It preserves one-rounding IEEE
  behavior but is substantially slower than native hardware FMA in dense sample loops.
- Binaryen and the compiler Wasm are staged as development assets; production hosting should add
  compression, caching, versioned URLs, and broader Chromium/Firefox/Safari audio coverage.
- The current browser smoke endpoint verifies source-to-Wasm page compilation, not automated audible
  AudioWorklet output; browser-audio automation remains a roadmap item.
