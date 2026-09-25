# Shared run view

`run.html` is embedded by the native run host, editor integrations, and the
VST3 plugin. It also runs in the browser playground iframe.

Run the DOM interaction regression tests from the repository root:

```sh
npm run test:run-view
```

The tests require Node.js 22+ and Firefox. Set `FIREFOX_BIN` to select an
executable outside `PATH`. They use an isolated temporary browser profile and
local HTTP server, exercise the actual shared page, and cover event editing,
parameter drafts, resets, connection changes, and host refreshes.

Hosts can send `resetEventArguments: true` in a state update alongside explicit
event argument values when selecting a new program or resetting arguments.
This also discards incomplete drafts whose parsed values have not changed.
Ordinary metadata refreshes preserve the existing controls and local edits.

Hosts that send a `viewState` in a state update receive `runViewReady` after
the host state and saved controls are rendered and the restored scroll position
has painted. Readiness waits for event controls until their schema and the
connection are ready, whether the schema arrives with the saved view state or
in a later update; a new file or a stopped/error view supersedes that wait.
Embedded hosts can keep their loading overlay visible until then;
`webviewReady` only means that the page can receive state.
The run view also hides its own controls during restoration, while keeping
layout and animation frames active. Hosts may include a `readyId` in `viewState`;
`runViewReady` echoes it so they can ignore readiness from an older reset.
The native and browser hosts send a reset when opening the view or changing files.
The browser host also resets when replacing a project that keeps the same entry path.
