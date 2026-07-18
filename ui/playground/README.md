# Browser playground UI

This directory owns the shared browser IDE implementation used by both the website `/playground/` route
and the standalone `examples/web/onda_wasm_playground` host.

- `live.js` coordinates the compiler worker, LSP client, editor, shared run view, and Web Audio host.
- `editor.js` owns the CodeMirror project editor and LSP presentation.
- `completions.js` maps Onda LSP kinds to font-independent editor icons.
- `examples.js` loads versioned checked-in example projects selected by website URLs.
- `lsp-client.js` speaks JSON-RPC to the Wasm-hosted `onda lsp` session.
- `run-view-host.js` adapts `ui/run/run.html` to the browser runtime.
- `browser-buffers.js` validates and decodes browser-provided WAV buffers.
- `microphone.js` requests and reuses browser microphone input only for top-level audio ports.
- `default.onda` is embedded into the generated playground bundle.

Hosts provide the required DOM elements and `globalThis.__ONDA_PLAYGROUND_ASSETS__`; they do not own
copies of the IDE runtime. `scripts/bundle-web-playground.mjs` is the single bundling entry point.

The editor uses compact project tabs, calls the compiler entry point the **Main file**, runs with
Cmd/Ctrl+Enter, stops with Ctrl+Period, and follows definitions with Cmd/Ctrl+click. Run and stop
shortcuts work anywhere in the playground, including inside the run view. Embedded standard
library targets open as read-only virtual tabs supplied by the Wasm LSP. Closing a project tab deletes
that file from the browser project; closing a standard-library tab only dismisses the virtual document.
Definition navigation remains available inside those virtual standard-library tabs.
Completion-kind icons are local SVG masks, so their availability does not depend on the selected
editor font. Programs with top-level audio inputs request microphone permission once and reuse that
stream across recompiles; projects without those inputs never request media-device access.

The **Share** action writes a versioned, compressed snapshot to the URL fragment. The snapshot
contains every project source, the main and active files, sample rate, and block size. URL fragments
remain client-side; browser-selected WAV data is intentionally not embedded in the link.
