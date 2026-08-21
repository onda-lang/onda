# Onda embedded-compiler playground

This example is a thin standalone host for the shared browser IDE in [`ui/playground`](../../../ui/playground/README.md).
It demonstrates embedding the Onda compiler in a browser application:

```text
editable Onda source
  -> onda lsp in @onda-lang/wasm-compiler Wasm + embedded stdlib
  -> live diagnostics, completion, hover, definitions, symbols, semantic tokens
  -> validated MIR MessagePack
  -> packaged Binaryen backend + Binaryen.js
  -> DSP Wasm + metadata
  -> @onda-lang/webaudio AudioWorklet
```

The shared UI consumes the public `@onda-lang/wasm-compiler`, `@onda-lang/processor-abi`, and
`@onda-lang/webaudio` APIs. It contains a CodeMirror project editor with line numbers and multiple
in-memory files, backed by the same Onda LSP implementation as `onda-vscode`. The existing shared
`ui/run/run.html` view provides scope, parameter, event, typed-buffer, reset, stop, and play controls.
Rust analysis, semantic compilation, and Binaryen O4 optimization run through the compiler package's
module worker, so neither LSP requests nor compilation block editor interaction.
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

Open `http://127.0.0.1:8787/`. Edit the source and select **Play** in the shared run view (or press
Cmd/Ctrl+Enter). This compiles changed project files and starts a fresh AudioWorklet; unchanged
projects reuse their compiled artifact. Ctrl+Period stops
execution. Both shortcuts work anywhere on the page, including inside the run view. Cmd/Ctrl+click
navigates to project definitions and opens standard-library definitions
in read-only tabs. Each compact tab has a close control: project tabs delete the browser-project file,
while standard-library tabs are merely dismissed. Definition navigation continues to work inside open
standard-library tabs. Browser autoplay rules may require another click on the page before output becomes
audible.

Use **Share** to copy a URL-fragment snapshot containing every project source file, the main and active
files, sample rate, and block size. The fragment stays in the browser and is compressed when supported.
Selected buffer files remain local and are not embedded in the URL.

Use **Open project** to replace the editor contents with a single `.onda` or `.on` file, or a project
ZIP. The browser picker intentionally excludes bare `.ondaproject` files because selecting a manifest
does not also give the page access to its referenced source and asset files. Package the manifest,
sources, and assets together in the ZIP instead. An archive may contain more than one
`.ondaproject`; the playground asks which one to open. Use **Download project** to package the current
in-memory sources and bound buffers into the same portable ZIP. After extraction, its `.ondaproject`
file can be passed directly to `onda compile`, `onda run`, or either native run GUI.

Without `-Serve`/`--serve`, the scripts only prepare the static assets:

```powershell
.\examples\web\onda_wasm_playground\build-demo.ps1
```

```bash
bash ./examples/web/onda_wasm_playground/build-demo.sh
```

The scripts:

- install the root npm workspace and its pinned `binaryen` dependency when it is missing
- build the Rust frontend with `wasm-pack --release`, then optimize it with the workspace-pinned
  Binaryen `wasm-opt -O4`
- stage the compiler, shared LSP/run view, ABI, and Web Audio modules
- bundle the canonical `ui/playground` runtime into this example's static host
- optionally start `server.mjs` on `127.0.0.1:8787`

The editor defaults to 48000 Hz and 512 frames, with 44100/48000 Hz sample rates and
128/256/512/1024/2048-frame compile blocks available; these are editor controls, not build-script flags.

If PowerShell script execution is blocked:

```powershell
powershell.exe -ExecutionPolicy Bypass -File .\examples\web\onda_wasm_playground\build-demo.ps1 -Serve
```

## Verification

All browser packages can be tested from the root workspace:

```bash
npm ci
npm run test:web
npm run test:onda --workspace @onda-lang/binaryen-web
```

The packaged-layout smoke test is also available separately:

```bash
npm run test:pack --workspace @onda-lang/wasm-compiler
```

`npm test` in the compiler package builds the source-to-Wasm product API and tests source, project,
diagnostic, and worker behavior. The backend's `npm test` covers current-schema lowering, the internal Wasm
math kernel, and AudioWorklet behavior. Its `npm run test:onda` compiles real Onda source, compares
Binaryen renders with the native LLVM/MIR path, and runs the FMA oracle. `npm run test:parity` runs
only the LLVM/Binaryen differential render suite. The source-driven/parity commands require a
working native Rust/LLVM Onda build; the demo asset build and ordinary JavaScript tests do not.

```bash
cd packages/onda_binaryen_web
npm run bench
```

This runs the reproducible development comparison documented in
[`docs/backend-benchmarks.md`](../../../docs/backend-benchmarks.md) and requires the native
Rust/LLVM Onda build.

For a manual browser smoke test, edit the default program and confirm LSP diagnostics/completion,
add an included project file, run it, inspect the shared output scope, move a generated parameter
control, load a PCM/float WAV or `.ondabuffer` for a declared buffer, trigger an event, export and
reopen the project ZIP, and reset the DSP.

## Host behavior

The worklet's persistent compile-block cursor decouples Web Audio render-quantum size from the MIR
compile-time block size. Each callback is split into legal `(start_frame, frames, flags)` segments;
a compile block may span callbacks, and one callback may cross several compile-block boundaries
without resetting DSP state.

The `@onda-lang/webaudio` adapter registers the worklet module before constructing the node, derives
channel options from metadata, and provides request-correlated helpers. Parameters are
initialized from generated metadata and updated with
`{ type: "set-param", param: "name", value }`. Scalar and fixed-array parameter types are written
according to their metadata rather than by a name-specific convention. The Params reset restores
parameter defaults through the same update path, while the Events reset restores only the event
argument editors. Neither action clears physical processor state or reruns processor initialization.

The generic event/control-output ABI accepts
`{ type: "event", event: "event_name", values: { param: value } }`; scalar, fixed-array, and slice
payloads are packed from compiler metadata and dispatched through the generated `onda_event_N`
export. `{ type: "read-control-outputs" }` produces a
`{ type: "control-outputs", values }` reply. ABI errors are returned as `onda-error` messages rather
than terminating the processor. Portable packed state can be requested and restored through
`snapshot` and `restore-snapshot`; restore starts from a fresh post-init physical image.

Declared external buffers are supplied in `processorOptions.buffers`, keyed by Onda name. Each
binding has `{ data, frames, channels, sampleRate }`; `data` uses the native interleaved frame
layout. Mono/static channel constraints are checked against compiler metadata. A fixed array can be
supplied under its logical name as an array of bindings, with `null` or omitted entries leaving
individual slots unbound. Missing bindings use the neutral one-frame descriptor and do not prevent
the processor from starting.
`{ type: "read-buffer", buffer: "name" }` returns current contents, including writes from Onda.

## Current limitations

- Recompiling while audio is active stops and recreates the AudioContext/processor, so DSP state is
  initialized rather than migrated or crossfaded.
- Browser projects persist in local storage, but selected buffer files do not. Playback may start
  with any scalar buffer or fixed-array slot unbound; the run view labels those slots as unbound and
  the processor uses neutral storage. Loading or clearing a file while audio is active recreates the
  processor with the new binding set.
- The playground loads little-endian PCM or IEEE-float WAV files into `f32` buffers and canonical
  `.ondabuffer` files into `bool`, `i32`, `i64`, `f32`, or `f64` buffers.
- The worklet can return control-output values, but the page does not yet render them.
- Top-level audio inputs connect to one shared microphone stream. The page requests permission only
  when the compiled processor exposes those inputs and reuses the stream across recompiles.
- The page rejects more than 32 flattened input or output channels because of Web Audio node limits.
- The page requests the selected sample rate from `AudioContext` and recompiles against the actual
  browser-selected rate before constructing the processor when they differ.
- Exact f32/f64 FMA is linked into the DSP module from the pure-Wasm math kernel. It preserves
  one-rounding behavior without JavaScript work on the audio thread, but can remain slower than
  native hardware FMA in dense sample loops.
- The standalone example stages development assets without compression or immutable URLs. The
  website build emits versioned assets; broader Chromium/Firefox/Safari audio coverage remains.
- The current browser smoke endpoint verifies source-to-Wasm page compilation, not automated audible
  AudioWorklet output; browser-audio automation remains a roadmap item.
