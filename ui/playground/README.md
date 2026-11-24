# Browser playground development

Shared browser IDE for the website `/playground/` route and the standalone
[Wasm playground](../../examples/web/onda_wasm_playground/README.md). Use the
example guide for setup and end-user workflows; this file describes integration.

## Module ownership

| Modules | Responsibility |
| --- | --- |
| `live.js` | Compilation, project lifecycle, editor/run-view coordination, and Web Audio. |
| `editor.js`, `lsp-client.js`, `editor-selection.js`, `editor-font-size.js` | CodeMirror, Wasm LSP, selection rendering, and font preferences. |
| `new-file.js`, `project-menu.js`, `examples.js`, `project-archive.js`, `share.js` | Project files, menus, loading, export, and sharing. |
| `run-view-host.js` | State/command adapter for the [GPU canvas](../gpu/README.md), readiness, logs, and saved presentation. |
| `browser-scope.js`, `meter-worklet.js` | Visibility-driven waveform capture and 30 Hz peak/visualization publication. |
| `audio-activation.js`, `browser-buffers.js`, `microphone.js`, `midi.js`, `param-smoothing.js` | Trusted audio activation, external buffers, input devices, and smoothing preferences. |
| `layout.css` | Shared layout. |

Hosts supply the DOM and `globalThis.__ONDA_PLAYGROUND_ASSETS__`.
[bundle-web-playground.mjs](../../scripts/bundle-web-playground.mjs) bundles
`live.js` and emits `playground.css`; both hosts load those outputs.
The initial source is imported directly from
[`examples/basic/saw_filter_saturator.onda`](../../examples/basic/saw_filter_saturator.onda),
which also supplies the homepage example.

## Integration invariants

- Compilation and LSP work run in a module worker. Play creates a fresh processor
  and reuses the artifact only when source and compile settings are unchanged.
- Settings changes sample rate, block size, or Fast Math through recompilation,
  retaining playback intent. All three compile choices persist locally. Theme
  belongs to the enclosing page; the run view follows it and hides its theme button.
- The run view stays behind loading feedback until its current state is rendered.
  Initialization failures remain visible and iframe reload retries the renderer.
- Autoplay recovery uses trusted gestures from both documents. Stop cancels pending
  playback; later ordinary interaction must not restart it.
- Project sources, settings, presentation, and schema-matched event arguments
  persist locally. Selected buffer files are session-only; Share includes sources
  and main/active files, while project ZIP export includes bound buffers.
- Meter peaks drive one visualization stream; waveform analysers are connected
  only while needed. OSQ1 packets reuse storage and contain at most 512 min/max
  columns per channel. Hidden pages disable both captures.
- Authored print collection requires cross-origin isolation. Audio, controls, and
  scope work without it. Microphone access is requested only for programs with
  audio inputs, and one stream is reused across recompiles.

The [host API](../../docs/ui-api.md) owns state, command, and iframe-envelope
contracts. The [Web API](../../docs/web-api.md) owns compiler and AudioWorklet APIs.

## Tests

From the repository root:

```sh
npm run test:playground
```

The Node.js 22+ suite does not open an audio device. For real Chrome canvas and
compiler/AudioWorklet tests, follow the build/staging steps in
[GPU UI verification](../gpu/README.md#verification). Both browser hosts have
WebGPU and WebGL2 cases; hardware-only cases can be skipped explicitly.
