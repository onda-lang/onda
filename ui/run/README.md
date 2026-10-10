# Shared run view

`run.html` is embedded by the native run host, editor integrations, and the
VST3 plugin. It also runs in the browser playground iframe.

All single-number inputs use the shared `ui/number-input.js` widget, including
parameters, event arguments, event array elements, MIDI octave, and MIDI velocity. The entire
`Smooth … ms` control supports horizontal dragging.
Hold Shift for fine adjustment, or click (or focus with Tab) to type a value.
Leaving the field restores dragging. Parameter drags send live targets through
the host's smoothing path and commit on release; ranged controls stay in sync.
Ranged parameter numbers and MIDI velocity use 375 horizontal pixels for their
full range. Parameter-number drags follow the declared scale or curve in egui,
webview, and the web playground, while displaying and accepting typed values in
plain units. MIDI octave uses a fixed sensitivity of 10 horizontal pixels per octave.
Knobs use 250 vertical pixels for their full normalized range in both hosts.
Shift makes knob and number drags ten times finer, and can be toggled during a drag.
Floating parameter and event editors start with up to five decimal places, using
scientific notation for tiny or very large values. Typed values can include more
precision; opening and closing an untouched editor preserves the exact underlying value.
Escape cancels parameter typing without rounding the underlying value. Keyboard
edits commit on Enter or blur, even when they match the rounded idle display.
Removing a parameter, changing its control metadata, or selecting another program
discards its drag and pending updates. Changing layout finishes retained drags.
Reversing direction at a range boundary changes the value immediately.
Display rounding does not change the drag's
accumulated value or its final commit.
Range drags accumulate pointer movement before scaling to plain values, so even
subnormal floating-point ranges remain draggable and reach their exact endpoints.
Double-click a number, knob, or slider to restore its parameter default.
Smoothing uses whole milliseconds and keeps a fixed width while dragging or typing.
Double-clicking anywhere on its control restores the host's initial smoothing
option (`--param-smoothing-ms` in the native hosts, which defaults to 30 ms).
Hosts send the current duration in `paramSmoothingMs` and the reset target in
`paramSmoothingDefaultMs`; edits and program reloads preserve the reset target.
Fractional reset targets remain exact even while the field displays whole milliseconds.
Event numbers reset to the argument's declared default; array elements reset
individually, with zero for newly added slice elements. MIDI octave resets to four
and MIDI velocity to 1.0, which is also its initial value.
Floating event fields display up to four decimal places, matching egui. Dragging
and triggering retain the exact value. Focusing starts a shortened draft in all
hosts; unchanged drafts and Escape preserve the exact underlying value. Leaving
the field restores the compact display.
Event `i64` values stay exact while typing and dragging in both hosts, including
beyond float precision.

Browser hosts serve `number-input.js` beside `run.html`. The native host injects
the same widget into its embedded page.

Run the browser interaction regression tests from the repository root:

```sh
npm run test:run-view
```

The tests require Node.js 22+ and Firefox. Set `FIREFOX_BIN` to select an
executable outside `PATH`. They use an isolated temporary browser profile and
local HTTP server, exercise the actual shared page, and cover event editing,
parameter drafts and drags, resets, connection changes, and host refreshes.
Trusted keyboard cases use Firefox's built-in WebDriver BiDi endpoint to verify
native input, change, and blur behavior without a separate browser driver.

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
