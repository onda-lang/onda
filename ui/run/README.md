# Shared run view

[run.html](run.html) provides the web UI used by the native webview host, editor
integrations, the VST3 plugin, and the browser playground. It displays transport,
scope, parameters, events, buffers, logs, and a virtual MIDI keyboard according
to host capabilities. The native egui host lives in
[crates/onda_egui](../../crates/onda_egui).

## Controls

- Ctrl/Command+S or Ctrl/Command+Enter plays; Ctrl/Command+. stops.
- Number fields support dragging, Shift for fine adjustment, and click or Tab to
  type. Enter or blur commits an edit; Escape cancels it.
- Decimal fields display and accept a dot on every platform, independently of
  regional settings. Arrow keys retain numeric stepping and range constraints.
- Double-click a parameter control to restore its default. Section reset buttons
  restore all parameters or event arguments.
- The smoothing control sets the duration of parameter changes. Double-click
  restores the host's initial setting.

## Host integration

Browser hosts serve [number-input.js](../number-input.js) and
[param-control.js](../../packages/onda_processor_abi/src/param-control.js) beside
`run.html`; the native webview host injects them into the page.
See [BrowserRunViewHost](../playground/run-view-host.js) for the browser adapter.

Host publications preserve authored values without applying the editing range
or step grid. Only user edits apply those constraints. Exact `i64` values and
defaults may use decimal strings to retain all 64 bits through display and focus.

Hosts opt in to the unloaded Fast Math checkbox with `supportsFastMath: true`
and publish its value as `fastMath`. The control appears beside the sample-rate
and block-size settings and can be enabled independently of `supportsRunSettings`.
Edits emit `setFastMath` with boolean `enabled`; hosts own the setting and apply
it to the next compilation. The control is hidden while a source is loaded.
The loaded header appends `Fast Math` when the host publishes `fastMath: true`;
browser hosts take this value from the compiled artifact.
Native egui and webview initialize it from `RunHostOptions.fast_math` (the CLI's
`--fast-math` option). Set `RunHostOptions.show_fast_math` to false to hide it in
either native host; its default is true. This compile option is separate from
the saved presentation `viewState`.

`webviewReady` means the page can receive state. When restoring `viewState`, wait
for `runViewReady` before dismissing a host loading overlay. A `readyId` in
`viewState` is echoed in that response to identify the restoration.
Use `viewState: { reset: true }` when changing programs, and
`resetEventArguments: true` to discard event drafts when restoring defaults.

## Tests

From the repository root:

```sh
npm run test:run-view
```

Requires Node.js 22+ and Firefox. Set `FIREFOX_BIN` if Firefox is outside `PATH`.
The suite exercises the shared page's controls, keyboard input, host updates,
and responsive layouts in a real browser.
