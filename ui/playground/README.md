# Browser playground UI

Shared browser IDE for the website `/playground/` route and the standalone
[Wasm playground](../../examples/web/onda_wasm_playground/README.md).

## Implementation

- `live.js` coordinates compilation, the editor, run view, and Web Audio.
- `editor.js` and `lsp-client.js` provide the CodeMirror editor and Wasm LSP client.
- `editor-selection.js` aligns selection backgrounds with the editor's row bounds.
- `new-file.js` owns inline file creation from the tab strip.
- `editor-font-size.js` manages editor font preferences.
- `layout.css` owns the shared layout; hosts supply colors and branding.
- `run-view-host.js` adapts the [shared GPU panels](../gpu/README.md) to the browser.
- `browser-scope.js` reduces analyser data into reusable, display-sized OSQ1 packets.
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
The + opens a filename field beside the tabs. Enter creates and selects the file;
Escape cancels. Invalid or duplicate paths show an error beside the field.
Selection backgrounds fill the row height and extend to the content edges on
complete rows. The active-row highlight hides while text is selected.

The run pane shows loading and compilation progress until its first rendered frame.
The loaded project starts automatically when the compiler is ready. If the browser
blocks autoplay, a click, tap, or key press anywhere in the playground, including
the embedded controls, enables audio. The prompt clears when audio starts. Stop
cancels pending playback; ordinary interactions leave a stopped project stopped.

The project persists in local storage. Play creates a fresh processor and reuses
the compiled artifact when sources and compile settings are unchanged.
Sample Rate and Block Size live in the run panel's Settings menu. Changing a compile
setting recompiles and restarts the project; the settings persist across page reloads.
Smooth retains the chosen duration across reloads; double-click restores its 30 ms default.
The Project menu sits beside the file tabs and includes the project actions and
font-size control, with Play/Stop shortcuts below. Status appears in the run view;
the editor host keeps a live region for screen-reader announcements. The MIDI
keyboard appears only when the patch declares the canonical `note_on` event.
Compiler errors include the source file, line, and column when available.
**New project** stops playback and replaces the project with an empty `main.onda`.
Programs with audio inputs request microphone permission and reuse the stream
across recompiles.

**Share** puts project sources and the main and active files in the URL fragment.
Compile settings and selected buffer data stay local.
Bind a WAV or `.ondabuffer` by dropping it onto the corresponding buffer card
or using its Bind/Replace button.

The run panels use WebGPU when a hardware adapter is available and WebGL2 otherwise.
Append `?backend=webgl` to force WebGL2. Panel selection, folds, scroll positions,
and typed event arguments persist locally across recompiles and page reloads.
The source editor continues to use CodeMirror.

Scope capture follows animation frames and stops when the scope is collapsed or
the page is hidden. Packets carry at most 512 min/max columns per channel;
scope samples do not pass through JSON. Printed output appears in the Log panel
when hosting enables cross-origin isolation. The example server supplies the
required COOP/COEP headers; audio and other panels also work without these headers.

## Tests

From the repository root:

```sh
npm run test:playground
CHROME_BIN=/path/to/chromium npm run test:gpu-ui
CHROME_BIN=/path/to/chromium npm run test:gpu-web-editor
```

The playground suite runs in Node.js 22+ without opening an audio device.
Build the compiler and GPU assets and stage both browser hosts before running
the Chrome suites. The editor suite compiles real source and checks muted
AudioWorklet playback, parameter/event dispatch, file binding, MIDI, saved state,
autoplay Stop behavior, and operation without cross-origin isolation. It tests
the example with WebGPU and WebGL2 and the website with WebGPU.
