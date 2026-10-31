# Onda embedded-compiler playground

This example hosts the shared [browser IDE](../../../ui/playground/README.md):
a CodeMirror source editor, the Onda compiler/LSP in a worker, GPU run controls,
and an AudioWorklet processor. It runs entirely in the browser after staging;
there is no native CLI or compiler service at runtime.

For a page that ships only a precompiled processor, use the
[AOT sample-player example](../onda_wasm_aot_sample_player/README.md).
For embedding APIs, see the [Web API](../../../docs/web-api.md) and
[UI host API](../../../docs/ui-api.md).

## Prerequisites

- Rust, `wasm-pack`, and the `wasm32-unknown-unknown` target.
- Node.js/npm and `cargo-about` 0.9.2 with the `cli` feature for license generation.
- A browser with WebAssembly, JavaScript modules, AudioWorklet, and WebGPU or WebGL2.

The result is a static site. Serve it over localhost or HTTPS; non-local audio
requires a secure context. The included server supplies COOP/COEP headers for
printed output. Without cross-origin isolation, audio, controls, and scope still
work, but authored prints are unavailable.

## Build and run

Run from the repository root on macOS/Linux:

```sh
bash examples/web/onda_wasm_playground/build-demo.sh --serve
```

Or in Windows PowerShell:

```powershell
.\examples\web\onda_wasm_playground\build-demo.ps1 -Serve
```

Open `http://127.0.0.1:8787/`. The scripts install missing npm dependencies, build
the compiler and GPU Wasm, and stage the shared editor, worker, and audio modules.
Omit `--serve`/`-Serve` to stage without starting the server.

For JavaScript/CSS changes after the initial build, the Bash script can restage
without rebuilding Wasm:

```sh
bash examples/web/onda_wasm_playground/build-demo.sh --skip-build --serve
```

Rebuild after changes to Rust, the product version, or packaged backend assets.
For Rust UI changes alone, run `npm run build:gpu-ui` before restaging with
`--skip-build`. If the server is already running, omit `--serve` and reload.

## Use the playground

The loaded project starts when compilation finishes. If autoplay is blocked,
click, tap, or press a key in the playground to enable audio. **Stop** cancels
pending playback and keeps it stopped until the next **Play** action.

- **Play:** Ctrl/Command+S or Ctrl/Command+Enter. Changed sources compile first;
  unchanged sources reuse their artifact. Each play starts fresh DSP state.
- **Stop:** Ctrl/Command+Period. Transport shortcuts also work inside the run view.
- **Navigate:** Ctrl/Command+click a definition. Standard-library definitions open
  in read-only tabs. Closing a project tab deletes that file; closing a library
  tab only dismisses it. The **+** beside the tabs creates a file.
- **Settings:** Sample Rate and Block Size recompile and restart the project.
  Preferences persist locally; the browser's actual sample rate is used for DSP.
- **Parameters and events:** The run view provides typed editors. Params Reset
  restores parameter defaults; Events Reset restores argument values. Neither
  reruns processor initialization. Play recreates the processor.
- **Buffers:** Bind/Replace or drop a WAV/`.ondabuffer` onto its buffer card. Changing
  a binding recreates the processor. Clearing a binding leaves neutral storage.

The GPU controls select WebGPU when a non-fallback adapter is available and
WebGL2 otherwise. Append `?backend=webgl` to force WebGL2. Presentation and typed
event arguments persist across reloads when their schemas still match.

## Projects and sharing

**Share** copies a URL-fragment snapshot of the project sources and main/active
files. Compile settings and selected buffer data stay local.

**Open project** accepts `.onda`, `.on`, or a project ZIP. A bare `.ondaproject`
manifest does not grant browser access to sibling sources/assets, so ZIP the
manifest with its dependencies. Archives with multiple manifests offer a choice.
**Download project** exports the current sources and bound buffers to the same
portable ZIP format. After extraction, pass its `.ondaproject` to native Onda.

Sources and preferences persist in local storage. Selected buffer files do not;
export a project ZIP to retain them. Omitted buffers do not prevent playback.

## Limits

- Recompilation and buffer changes recreate the processor; DSP state is not migrated.
- WAV input supports little-endian PCM/IEEE-float samples for `f32` buffers;
  `.ondabuffer` preserves all supported primitive element types.
- Programs with audio inputs request microphone permission and reuse one stream.
  The playground supports at most 32 flattened input or output channels.
- Control-output and mutable-buffer inspection are available in the underlying
  APIs but are not exposed by the playground.

## Verification

After building/staging, run from the repository root:

```sh
npm run test:playground
CHROME_BIN=/path/to/chromium npm run test:gpu-web-editor
```

The Chrome suite checks both browser hosts with real compilation and muted
AudioWorklet processing. It covers controls, files, MIDI, saved presentation,
autoplay, and non-isolated hosting. See [GPU UI verification](../../../ui/gpu/README.md#verification)
for website staging and hardware-WebGPU requirements. Other package tests are
listed in the [Web API development guide](../../../docs/architecture.md#browser-build-and-verification).
