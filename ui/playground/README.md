# Browser playground UI

Shared browser IDE for the website `/playground/` route and the standalone
[Wasm playground](../../examples/web/onda_wasm_playground/README.md).

## Implementation

- `live.js` coordinates compilation, the editor, run view, and Web Audio.
- `editor.js` and `lsp-client.js` provide the CodeMirror editor and Wasm LSP client.
- `editor-font-size.js` manages editor font preferences.
- `layout.css` owns the shared layout; hosts supply colors and branding.
- `run-view-host.js` adapts the [shared run view](../run/README.md) to the browser.
- `audio-activation.js` handles trusted audio-enabling gestures in both documents.
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

The loaded project starts automatically when the compiler is ready. If the browser
blocks autoplay, a click, tap, or key press anywhere in the playground, including
the embedded controls, enables audio. The prompt clears when audio starts. Stop
cancels pending playback; ordinary interactions leave a stopped project stopped.

The project persists in local storage. Play creates a fresh processor and reuses
the compiled artifact when sources and compile settings are unchanged.
Sample Rate and Block Size are local compile preferences. Changing a compile
setting recompiles and restarts the project; the settings persist across page reloads.
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

The playground suite runs in Node.js 22+ and checks audio gesture handling without
opening an audio device. The run-view suite uses Firefox and also checks both
playground hosts' layouts.

Browser autoplay policy is checked manually: with autoplay blocked, click, tap,
or press a key in the editor or embedded controls to enable audio. Stop must
cancel pending playback without enabling audio. With autoplay allowed, audio
should start when the compiler is ready.
