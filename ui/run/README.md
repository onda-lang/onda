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
