# Browser playground UI

Shared browser IDE for the website `/playground/` route and the standalone
[Wasm playground](../../examples/web/onda_wasm_playground/README.md).

## Implementation

- `live.js` coordinates compilation, the editor, run view, and Web Audio.
- `editor.js` and `lsp-client.js` provide the CodeMirror editor and Wasm LSP client.
- `editor-font-size.js` manages editor font preferences.
- `layout.css` owns the shared layout; hosts supply colors and branding.
- `run-view-host.js` adapts the [shared run view](../run/README.md) to the browser.
- `browser-buffers.js`, `microphone.js`, and `midi.js` handle browser audio and MIDI input.
- `param-smoothing.js` preserves smoothing preferences across recompiles.
- `examples.js`, `project-archive.js`, and `share.js` load and share projects.
- `default.onda` supplies the initial program.

Hosts provide the DOM and `globalThis.__ONDA_PLAYGROUND_ASSETS__`.
[bundle-web-playground.mjs](../../scripts/bundle-web-playground.mjs) bundles
`live.js` and emits `playground.css`; hosts load both outputs.

## Behavior

Ctrl/Command+S or Ctrl/Command+Enter plays; Ctrl/Command+. stops. Ctrl/Command+click
follows definitions, including into read-only standard-library tabs. Closing a
project tab deletes its file; closing a standard-library tab dismisses the view.

The project persists in local storage. Play creates a fresh processor and reuses
the compiled artifact when sources and compile settings are unchanged.
**New project** stops playback and replaces the project with an empty `main.onda`.
Programs with audio inputs request microphone permission and reuse the stream
across recompiles.

**Share** puts project sources and the main and active files in the URL fragment.
Compile settings and selected buffer data stay local.

## Tests

From the repository root:

```sh
npm run test:playground
npm run test:run-view
```

The run-view suite also checks both playground hosts' layouts and requires
Node.js 22+ and Firefox.
