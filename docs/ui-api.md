# Onda UI host API

`onda_ui` is the shared Onda run screen. A host supplies state and input, applies
commands, and schedules rendering. The host owns audio, compilation, files,
clipboard access, persistence, and its event loop. The UI owns layout, gestures,
numeric drafts, and presentation. Its internal widgets are private.

This reference defines the host protocol for Rust `RunUi`, the C interface in
`include/onda_ui.h`, and Wasm `CanvasUi`. JSON names and value encodings are the
same across these interfaces. The browser adapter adds the envelope and lifecycle
messages described below. Use matching Onda library, header, and browser assets.

## Rust embedding

`RunUi::default()` creates an empty model. Apply JSON objects with `message()`;
drain the ordered `Vec<serde_json::Value>` from `take_commands()`. Input has public
Rust types, `Input` and `Key`; the JSON input forms below deserialize to `Input`.

For an application that already uses wgpu, construct
`Renderer::from_device(device.clone(), queue.clone(), format)`. The queue must
belong to that device. The device must support standard wgpu downlevel WebGL2
limits. The host retains its original handles and can use the UI output in its
other rendering work. `Renderer::new(adapter, format).await` is a convenience
constructor for hosts that want a dedicated device.

`Renderer::render(ui, view, width, height, scale)` clears the entire target,
records a render pass, and submits it. It returns a `wgpu::SubmissionIndex`.
The target must belong to the renderer's device, have the constructor's format,
one sample, `RENDER_ATTACHMENT` usage, and the supplied physical dimensions.
Render into a separate texture and composite it in the host when necessary.
It does not present a window or wait for GPU completion.

`SurfaceRenderer` owns the supplied surface, creates an adapter/device, and
handles configuration and presentation. Its `render()` returns `Ok(false)` for a
temporarily unavailable or zero-sized surface; retry on a later display tick.
An error requires the host to report the failure and recreate the renderer.

The source checkout's `crates/onda_ui/examples/host.rs` example owns its device
and texture, edits a parameter, applies the command, and renders without an Onda
compiler, audio device, or window. Run it from the source workspace:

```sh
cargo run -p onda_ui --example host
```

## Host lifecycle

1. Create the renderer and model. Set the logical size and positive finite display
   scale with `RunUi::resize()`. Renderer and C/canvas dimensions are physical
   pixels; input positions, text rectangles, and scroll distances are logical pixels.
2. Publish a `state` message before accepting input. Explicitly disable capabilities
   the host does not implement. For asynchronous metadata, publish `schemaReady:false`
   while loading and `true` after all banks arrive or loading fails.
3. Route native pointer/text input through `Renderer::input()` so selection uses
   the same shaping as rendering. Forward unused keys to the containing host.
   Wasm hosts overlay a native textarea, as `ui/gpu/text-input.js` demonstrates.
4. Drain commands after input, resize, state updates, and rendering. Measured
   layout can move a draft out of view, committing valid text or cancelling an
   invalid or unfinished composition. Refresh native text/IME state afterward.
   Apply commands on the host side and acknowledge accepted values/settings.
   Report rejected operations in `state.error` and republish the accepted value.
   Parse a C command array before invoking host callbacks, since callbacks can
   recursively publish state.
5. Call `RunUi::update()` to advance timed state and resolve layout without drawing.
   Drain commands, then compare `revisions()` with the last presented pair. Schedule
   a coalesced redraw when changed. Recompute `wake_deadline()` after updates,
   command draining, or state/input changes; it is absent when there is no timed
   work. At that deadline, update and drain commands again. A wakeup can save view
   state without changing pixels. Native exposure still requires presentation even
   with unchanged state. The host owns display pacing, visibility, data polling,
   and bounded presentation retries; the UI never starts a timer or event loop.
   Layout, text, and capture queries do not advance elapsed feedback or consume
   timed repaint deadlines; only an explicit update does so.
6. Query `meter_enabled()` and `scope_enabled()` after layout/input changes.
   Capture peaks only while a meter or waveform is visible, and waveform history
   only while `scope_enabled()` is true. `scope_columns()` is the physical display
   budget. Hidden views stop capture and drawing. Stock hosts publish meters and
   the optional waveform together on one 30 Hz visualization stream, independently
   of input and other redraws. Retain the last meter reading when no new audio
   block has arrived; an actual silent block, stop, or reset clears it.
7. Before hiding, switching away, or destroying the view, send `focusLost` and
   drain its commands while host handlers still exist. This commits valid drafts,
   cancels unfinished composition, ends gestures, and releases owned MIDI notes.
   Release GPU resources before destroying borrowed native handles.

Parameter gestures emit intermediate `commit:false` updates and a final
`commit:true` update. A cancellation can instead emit `endParamGesture` or
`endInputControlGesture`. Hosts with automation open a gesture on the first
intermediate update and close it on either terminal command. An isolated committed
edit is a complete gesture. These commands never execute on an audio callback.

## Values and state updates

All wire text is UTF-8 JSON. Names identify controls within their bank, buffers,
and events; use unique names in each collection. Ordering determines presentation.
An omitted state property retains its previous value. A supplied collection
replaces that entire collection; use `[]` to clear it. Unknown properties and
message kinds are ignored. Unsupported scalar control types are omitted.

Numbers and numeric metadata use plain units. `bool` values are JSON booleans;
`i32` values are JSON integers; `i64` values are decimal strings to preserve all
bits through JavaScript. Finite floats are JSON numbers. Event values also accept
`"NaN"`, `"Infinity"`, `"-Infinity"`, and `"-0"`. JavaScript hosts can use
`wireJson()` from `ui/gpu/protocol.js` to preserve special float values. Parameter
editors accept finite values only.
Value updates, numeric edits, and schema/default comparisons distinguish positive
and negative zero, including inside structured event defaults.

### Host-to-UI messages

| `type` | Other properties | Effect |
| --- | --- | --- |
| `state` | `state: StatePatch` | Merge the supplied state properties. |
| `parameterValues` | `state: {params?: ValueUpdate[], inputControls?: ValueUpdate[]}` | Update known values without replacing metadata, drafts, or active gestures. |
| `meterData` | `channels: integer`, `peaks: number[]` | Per-channel nonnegative maxima since the previous capture; up to 64 channels. |
| `scopeData` | `channels: integer`, optional `peaks: number[]`, optional `samples: number[]` | One visualization update. Peaks are per-channel maxima; waveform samples are interleaved, bounded to 16,384 frames and 64 displayed channels. Omit waveform data for meters-only updates. |
| `midiActivity` | `activeNotes: integer[]` | Replace monitored MIDI notes, each 0–127. This does not emit MIDI commands. |
| `bufferFileDrag` | `phase: string`, `position?: {x,y}`, `paths?: string[]` | Highlight a visible buffer; `phase:"drop"` emits `bindBufferFile`. |
| `sourceFileDrag` | Same drag properties | Highlight the unloaded source picker. The host opens dropped sources itself. |

`ValueUpdate` is `{name, value}`. As an alternative, `{name, normalizedValue}`
supplies a normalized 0–1 value interpreted through the published control domain.
Unknown names are ignored. During a drag the UI retains its local target; focused
text drafts survive host value updates. Schema or source replacement cancels them.

Drag phases `enter` and `over` update the target; `leave` clears it. The host
validates supported files first. `paths`, when supplied, must contain one distinct,
nonempty path; duplicate entries for that same path are accepted. Positions are
relative to the UI. Hosts retaining an asynchronous drop must also retain its
target and discard delivery after a source switch or cancellation.

For scope data, Rust also accepts `set_scope_interleaved()`, `set_scope_envelope()`,
and `set_scope_packet()`; C accepts copied float snapshots through `onda_ui_scope()`.
`CanvasUi` accepts `scope_samples(channels, Float32Array)` and
`scope_packet(Uint8Array)`. The packaged browser adapter additionally accepts
`{type:"scopeData", packet:Uint8Array}` or a base64 packet string, decoding it before
calling Wasm. Packet-bearing objects are a browser-adapter feature; do not send
them to `RunUi::message()` or `onda_ui_message()`.

OSQ1 scope packets contain four ASCII bytes `OSQ1`, little-endian `u16` channel
and column counts, then channel-major pairs of little-endian signed `i16` extrema.
Each pair is minimum then maximum; decode amplitude by dividing an extremum by 32,767.
Limits are 64 channels and 512 columns; an empty packet has both counts zero.
Invalid packets leave the previous frame intact. Identical frames skip uploads.
The browser adapter rejects malformed packets and base64 without interrupting
rendering or later host updates.

### StatePatch: status and capabilities

| Property | Type and meaning |
| --- | --- |
| `path` | Source identity string; `""` unloads the source. On a source change, also replace or clear the control/event/buffer banks. |
| `status`, `error` | Display strings; use `null` or `""` to clear an error. |
| `running` | Boolean playback state, initially false. A transition to false releases owned MIDI notes. |
| `connected` | Boolean event-dispatch availability. Explicit false prevents Trigger commands. |
| `schemaReady` | Boolean metadata completion barrier; omitted means synchronous/ready. |
| `engineGeneration` | Host-defined string identifying the current event schema/engine; copied verbatim into `triggerEvent` commands, or `null` when absent. The host must reject stale generations if it uses this facility. |
| `outputChannels` | Integer output count, 1–64; otherwise inferred from visualization data. |
| `ondaVersion` | Optional version label. |
| `themeMode` | `"dark"` or `"light"`. Omitted/null lets a browser host detect its appearance at startup. Native hosts resolve an unspecified choice when opening the window. |
| `hostTheme` | Optional host appearance hint: `"dark"` or `"light"`. Browser adapters can resolve `"auto"` against VS Code or system appearance. |
| `supportsThemeSelection` | Show the ◐ Dark / Light button unless false. Browser editors disable it and keep following the page's `hostTheme`. |
| `supportsTransport` | Start/Stop actions; enabled unless explicitly false. |
| `supportsScope` | Meter/waveform availability for a loaded source; enabled unless explicitly false. |
| `supportsSourceSelection` | Source picker and Unload; enabled only by true. |
| `supportsProjectExport`, `canExportProject` | Save-as-project action; both must be true. |
| `supportsDeviceSelection` | Audio input/output selectors and device refresh; enabled only by true. |
| `supportsMidiInputSelection` | MIDI selector and refresh; enabled only by true. |
| `supportsRunSettings` | Sample-rate/block-size editing; enabled only by true. |
| `canSetSampleRate`, `canSetBlockSize` | Permit the corresponding setting while a source is loaded; enabled only by true. |
| `supportsFastMath`, `fastMath` | Availability and current boolean value of the Fast Math option. |
| `supportsParamSmoothing`, `supportsInputControlSmoothing` | Enable each bank's smoothing editor; enabled only by true. |
| `supportsViewState` | Enable `viewState` persistence commands; enabled only by true. |

### StatePatch: controls and settings

| Property | Type and meaning |
| --- | --- |
| `params`, `inputControls` | Arrays of scalar controls described below. The banks are independent. |
| `paramLayout`, `inputControlLayout` | `"knobs"` (default) or `"sliders"`. |
| `paramSmoothingMs`, `inputControlSmoothingMs` | Finite nonnegative duration, in milliseconds. |
| `paramSmoothingDefaultMs`, `inputControlSmoothingDefaultMs` | The corresponding exact reset targets. |
| `sampleRateHz`, `blockSamples` | Positive integer sample rate and compiled block size. Presence supplies the displayed setting. |
| `sampleRateOptions`, `blockSizeOptions` | Optional arrays of positive integers replacing the default selector choices. |
| `hostBlockSamples` | Positive host block capacity. Presence selects plugin-style block settings and hides sample-rate editing. |
| `preferredBlockSamples` | Plugin block preference; zero means follow the host. A larger-than-current-capacity saved preference is displayed but not offered as a new choice. |
| `inputDevices`, `outputDevices`, `midiInputDevices` | Arrays of device-name strings. |
| `currentInputDevice`, `currentOutputDevice` | Device-name string or `null` for the default device. |
| `currentMidiInputDevice` | MIDI device-name string. With MIDI selection enabled, publish `"Computer Keyboard"` explicitly to enable computer-key input. |
| `buffers`, `events` | Arrays described below. Empty collections hide their sections. |
| `midi` | `{noteOn:boolean, noteOff?:boolean}`; `noteOn:true` enables the MIDI keyboard section. |
| `midiKeyboardInteractive` | False disables new UI note input; otherwise interactive when `midi.noteOn` is true. |
| `showComputerKeyLabels` | Hide computer-keyboard labels with false. Plugin hosts omit `computerKey` input and let their DAW own computer MIDI routing. |
| `viewState` | Saved presentation object described below. |
| `resetEventArguments` | Transient true on a message supplying `events` makes its argument values authoritative. Without it, matching live event signatures retain edited values. |

Default sample-rate choices are 44,100, 48,000, 88,200, 96,000 and 192,000 Hz;
default block choices are 64, 128, 256, 512, 1,024 and 2,048 frames. With
`hostBlockSamples`, block choices instead include Follow host and the host capacity
divided by 1, 2, 4 and 8. Settings remain host-validated; publish their accepted values.

### Scalar controls

Each control requires `name`, `type` (`"bool"`, `"i32"`, `"i64"`, `"f32"`, or
`"f64"`), and its current `value` or `normalizedValue`. Optional properties are:

| Property | Meaning |
| --- | --- |
| `label` | Display name; defaults to `name`. |
| `default` | Plain reset value in the control's scalar encoding. |
| `rangeMin`, `rangeMax` | Finite numeric bounds, with minimum less than maximum. Equal endpoints make the field read-only. |
| `scale` | `"linear"` (default) or `"log"`; log requires a positive floating range. |
| `curve` | Finite curvature for a linear range; cannot be combined with log. |
| `unit` | Optional display-unit string. |
| `step`, `stepCount` | Positive grid spacing and matching interval count; supply both or neither. Integer domains require both. A one-member enum uses `step:1, stepCount:0`. |
| `enumeration` | Optional `{name, members:[{name,label},...]}`. Member indices are `i32` values; render a dropdown independently of the knob/slider layout. |
| `enabled` | False disables editing and labels numeric input controls as External input. |
| `array` | `{name:string, length:integer, index:integer}` groups flattened array controls under one foldable heading. |
| `normalizedValue` | Alternative initial value when `value` is omitted; converted through the numeric domain or the boolean 0.5 threshold. |
| `hostValueDomain` | `"normalized"` makes set commands carry normalized values. Otherwise commands carry plain values. State and defaults still use plain values. |

`ParamDomain` validates ranges and grids. Integer bounds must fit their scalar
type; `i64` bounds and range width must also fit within ±(2^53−1) and 2^53−1,
respectively. The same `enumeration` object on a scalar leaf of an event argument `shape` selects a dropdown, including nested struct fields and array elements.
For example, an integer range 0–127 uses `step:1, stepCount:127`.
Without a valid domain, numeric text editing remains available but the knob/slider
has no range gesture. Unranged `i64` text editing preserves the complete signed
64-bit value. Step snapping and clamping use the declared domain.

Example bank:

```json
{"type":"state","state":{"path":"host.onda","params":[
  {"name":"gain","type":"f64","value":0.5,"default":0.25,"rangeMin":0,"rangeMax":1},
  {"name":"channel","type":"i32","value":1,"default":1,"rangeMin":1,"rangeMax":16,"step":1,"stepCount":15},
  {"name":"enabled","type":"bool","value":true,"default":true}
]}}
```

### Events and recursive values

An event is `{name, args: Argument[]}`. Each argument is
`{name, shape, default?, value?}`. `shape` is a recursive object:

| `kind` | Other shape properties | Value representation |
| --- | --- | --- |
| `scalar` | `encoding`: a scalar control type | Scalar encoding above. |
| `struct` | `fields: [{name, ty: Shape, default?}]`, optional display `name` | Object keyed by field name. |
| `tuple` | `elements: Shape[]` | Array in element order. |
| `array` | `element: Shape`, `len: nonnegative integer` | Array with exactly `len` elements. |
| `slice` | `element: Shape` | Array of variable length; UI Add stops at 4,096 elements. |

A missing/invalid argument `default` is built recursively: scalars use false or
zero, tuples/arrays use element defaults, slices are empty, and structs use their
field defaults. Struct field default descriptors follow processor metadata: scalar
defaults are decimal/float-bit strings, and aggregate defaults use ordered arrays.
A valid argument `value` overrides its default. Top-level argument defaults and
values use the logical representations in the table, including struct objects.
Always publish `shape`; primitive `type` spellings are accepted as shorthand but
cannot describe nested structured fields.

```json
{"type":"state","state":{"events":[{"name":"note","args":[
  {"name":"pitch","shape":{"kind":"scalar","encoding":"f64"},"default":69},
  {"name":"id","shape":{"kind":"scalar","encoding":"i64"},"default":"0"}
]}]}}
```

Preserve edited or saved values only if the complete event signature matches:
event name, argument count, and each ordered argument's name, shape, and default.
A change to any member resets all arguments and cancels active drafts. Values
must validate against the complete shape before dispatch. `triggerEvent.values`
follows argument order. Hosts perform their own schema and engine checks before
executing commands received asynchronously.

### Buffers and logs

A buffer entry requires `name`; optional `type` is a display string. `loadedPath`
is a path string. `loadedFrames`, `loadedChannels`, and `loadedSampleRate` describe
a bound asset; a positive frame count without a path displays Embedded project
asset. `waveform` is `{minimums:number[], maximums:number[], minValue?:number,
maxValue?:number}`. Corresponding array entries are bucket extrema. Publish
bounded previews; drawing reduces them to at most 512 columns. Omit `waveform`
when no preview exists.

Log state uses `logText:string`, optional `logEntries:Entry[]`, and
`logRevealed:boolean`. Entries align with `logText` lines and may be null;
`Entry` is `{source?:{file?:string,line?:integer}, lexicalOwner?:string}`.
Copy exports all `logText`. Loss counters are nonnegative integers:
`printOverflowCount`, `printTransportDropCount`, `delegateOverflowCount`, and
`delegateTransportDropCount`. Log-only state patches retain control/event schemas.
Hosts bound their history and trim matching entries when dropping old lines.

### Presentation persistence

With `supportsViewState:true`, save emitted `{type:"viewState", state:object}`
snapshots under their `state.path`. Treat snapshots as opaque host-owned data and
return them as `state.viewState`. They include section folds, scope pinning,
document/log scrolling, bank layouts, octave/velocity, and typed event signatures
and values; they contain no DSP parameter snapshot.

For initial or authoritative restoration, add a fresh `viewState.readyId` string
or integer. The UI applies each ID once; repeated IDs and ordinary saved-state
echoes cannot overwrite newer local edits. Restoration waits for `schemaReady`.
`viewState:{reset:true}` resets presentation and event arguments to defaults while
retaining their schemas. Flush persistence commands before source replacement;
commands generated by replacement can still belong to the previous `path`.

New sources start with the scope pinned and expanded. Saved pinning and fold
choices take precedence; hidden and collapsed scopes still stop waveform capture.
Hosts constructing a presentation explicitly may supply `scopePinned:boolean`,
`sections:{scope?,log?,buffers?,events?,inputControls?,params?,midi?}` booleans,
`scrollTop:number`, `logFollow:boolean`, `logScrollTop:number`, `logScrollX:number`,
`paramLayout`/`inputControlLayout`, `octave:integer` (−1–7), and
`velocity:number` (0–1; default 1). `folded:string[]` contains opaque UI fold IDs.
`eventValues` contains `{name,args:[{name,shape,default}],values:Value[]}` entries.

## UI-to-host commands

Every command is an object with the following `type` and payload. Ignore unknown
types. The host validates names, values, capabilities, and engine identity and
acknowledges operations through state/value messages.

| `type` | Other properties | Host action |
| --- | --- | --- |
| `start`, `stop` | None | Change playback state. |
| `chooseOndaFile`, `unload`, `saveProjectAs` | None | Run the corresponding source/project operation. |
| `setParam`, `setInputControl` | `name`, `value`, `commit:boolean` | Set the bank's value in its declared host value domain. |
| `endParamGesture`, `endInputControlGesture` | `name` | End a canceled gesture without another value write. |
| `resetParams`, `resetInputControls` | None | Restore the corresponding host-owned defaults. |
| `setParamLayout`, `setInputControlLayout` | `layout:"knobs"|"sliders"` | Retain presentation; DSP is unaffected. |
| `setParamSmoothing`, `setInputControlSmoothing` | `milliseconds:number` | Apply a nonnegative host smoothing duration. |
| `setRunSettings` | `sampleRateHz:integer`, `blockSamples:integer` | Validate and recompile with the selected settings, preserving controls and playback intent; plugin block preference zero follows the host. |
| `setFastMath` | `enabled:boolean` | Recompile with the selected floating-point optimization policy, preserving playback intent. |
| `setTheme` | `mode:"dark"\|"light"` | Save the theme choice and publish it in state; requires no recompilation. |
| `refreshDevices` | None | Refresh supported device lists. |
| `setInputDevice`, `setOutputDevice`, `setMidiInputDevice` | `name:string|null` | Select a device; null selects the default audio device. Computer MIDI uses the explicit string `"Computer Keyboard"`. |
| `chooseBufferFile`, `clearBuffer` | `name` | Pick/replace or clear a binding. |
| `bindBufferFile` | `name`, `filePath:string` | Bind an accepted native file drop. The browser bridge supplies `file:File` instead. |
| `triggerEvent` | `name`, `values:Value[]`, `engineGeneration:string|null` | Encode ordered typed arguments and dispatch to the matching engine. |
| `resetEventArguments` | None | UI values are already reset; acknowledge if the host mirrors event edits. |
| `clearLog` | None | Clear the host log and counters; UI clears immediately too. |
| `copyText` | `text:string` | Write the host clipboard. |
| `midiNote` | `key:integer`, `pressed:boolean`, `velocity:number` | Emit owned MIDI; key is 0–127 and velocity is 0–1, zero on release. |
| `viewState` | `state:object` | Persist the presentation snapshot. |
| `scrollParent` | `deltaY:number` | Pass unconsumed logical vertical pan distance to an enclosing scroller, if any. |

## Input and native text

All input objects have `type`. Coordinates are logical and relative to the UI;
positive scroll deltas move down/right. Forward hover movement without a held
button. Native hosts should use physical computer-key positions for MIDI.

| `type` | Other properties |
| --- | --- |
| `pointerDown` | `x:number`, `y:number`, optional `clicks:integer` (default 0; 2 resets). |
| `pointerMove` | `x`, `y`, optional `fine:boolean` (default false). |
| `pointerUp`, `pointerLeave` | None. |
| `touchStart` | `id:integer`, `x`, `y`. |
| `touchMove` | `id`, `x`, `y`, optional `deltaY:number` independent of local position when the host scrolls under the finger. |
| `touchEnd`, `touchCancel` | `id`; keep each finger's ID stable. |
| `scroll`, `horizontalScroll` | `delta:number`. |
| `key` | `key:string`, optional `shift:boolean` (default false). |
| `computerKey` | `key:string` physical letter, `pressed:boolean`. |
| `releaseComputerKeys` | None; release only computer-owned MIDI. |
| `focusLost` | None; resolve drafts and release all owned input. |
| `text` | `text:string` to replace the current selection. |
| `preedit` | `text`, `anchor:integer`, `cursor:integer`; reversible composition separate from the committed draft. |
| `textMark` | `start:integer`, `end:integer`; mark a native composition range, or an empty range to finish composition. |
| `editText` | `text`, `anchor`, `cursor`; replace the whole draft from a host editor and finish reversible preedit, even if text and selection are unchanged. |
| `selectText` | `index:integer`, optional `extend:boolean`. |

`key` accepts Enter, Escape, Backspace, Delete, ArrowLeft, ArrowRight, ArrowUp,
ArrowDown, Home, End, Tab, SelectAll, Copy, Cut, Play, and Stop. Shift extends
selection or reverses Tab. Hosts map platform modifiers to SelectAll/Copy/Cut;
paste supplies `text`. Map transport shortcuts to Play/Stop without key repeat.
Text is limited to 65,536 UTF-8 bytes and rejects control characters. Selection
and composition offsets use UTF-16 code units, including on native platforms.
Browser hosts forward provisional `compositionupdate` text as `preedit`, then
publish the final native editor snapshot with `editText` on `compositionend`.
This lets shared cancellation policy handle scroll, resize, and focus loss.

`RunUi::input()` and `Renderer::input()` return whether input was consumed.
`RunUi::scroll(delta, horizontal)` returns whether the targeted viewport moved,
allowing hosts to propagate wheel events at its edges. `cursor()` returns
`default`, `pointer`, `text`, `ew-resize`, or `ns-resize`.

`text_edit()` returns null without a visible draft, otherwise
`{text,anchor,cursor,rect:[x,y,width,height],centered,invalid,fontSize,monospace,resettable}`.
`Renderer::text_bounds(start,end)` and `text_hit(x,y)` supply shaped native text
geometry. A native adapter must clear its composition state when this draft
disappears, even if its input method does not send an explicit end callback.

## C surface adapter

Enable the Rust `ffi` feature and include `onda_ui.h`. All calls occur on the
host UI thread. `OndaUi*` owns its GPU/model resources and borrows the native
window/display, which must outlive it. Xlib uses platform 1 with a window XID and
`Display*`; Win32 uses platform 2 and HWND; AppKit uses platform 3 and NSView*.
Pass zero for unused display handles. This C adapter does not accept Wayland
handles; the standalone winit adapter supports native Wayland separately.

Give the renderer exclusive ownership of pixels in that native view. When
embedding in another toolkit, use a dedicated child window/view so its painter
cannot erase or cover GPU presentation. Keep input/focus/event-loop ownership
in the host. On resize, update native bounds, call `onda_ui_resize`, and render
within the same UI callback; use the host event loop to retry temporary surface
unavailability. Destroy the renderer before releasing its native child/display.

Native release archives contain `include/onda_ui.h` and a shared library in `lib/`:
`onda_ui.dll`, `libonda_ui.so`, or `libonda_ui.dylib`. Embedded hosts can load
that library with their platform loader and resolve the declared functions;
the plugin's `GpuView` demonstrates this approach in the source repositories.

<!-- BEGIN UI C API FUNCTION INDEX -->
```text
onda_ui_create
onda_ui_destroy
onda_ui_resize
onda_ui_message
onda_ui_input
onda_ui_scope
onda_ui_update
onda_ui_wake_delay_ms
onda_ui_render
onda_ui_scope_visible
onda_ui_meter
onda_ui_meter_visible
onda_ui_commands
onda_ui_error
onda_ui_text_edit
onda_ui_text_bounds
onda_ui_text_hit
onda_ui_cursor
```
<!-- END UI C API FUNCTION INDEX -->

| Function | Result and ownership |
| --- | --- |
| `onda_ui_create` | Native platform/handles, physical dimensions and scale to a new handle; null on failure, with a NUL-terminated message in the caller's error buffer when provided. |
| `onda_ui_destroy` | Release a handle; null is accepted. |
| `onda_ui_resize` | Physical width/height and positive finite scale; zero dimensions suspend presentation. Also call on exposure with current dimensions to force presentation. |
| `onda_ui_message` | NUL-terminated host-message JSON; 1 on successful parsing, 0 on error. |
| `onda_ui_input` | NUL-terminated input JSON; 1 consumed, 0 unused, −1 error. |
| `onda_ui_scope` | Copy `count` interleaved floats, at most 64×16,384 samples; zero count clears the waveform. |
| `onda_ui_meter` | Copy `count` peak floats, at most 64, with a channel count. |
| `onda_ui_scope_visible`, `onda_ui_meter_visible` | Resolve layout and return 1 when that clipped viewport needs capture, otherwise 0. |
| `onda_ui_update` | Advance timed state/layout without drawing; 1 redraw needed, 0 clean, −1 error. Drain commands afterward even when clean. |
| `onda_ui_wake_delay_ms` | Milliseconds until the next timed update; −1 when absent. Relative to the library's monotonic clock; no clock epoch crosses the C interface. Recompute after updates, command draining, or state/input changes. |
| `onda_ui_render` | Update and render if changed; 1 presented/clean, 0 retry later, −1 error. |
| `onda_ui_commands` | Drain to a borrowed JSON command-array string, valid until the next commands call or destroy. |
| `onda_ui_error` | Borrow the last error string until another error or destroy. |
| `onda_ui_text_edit` | Borrow text-edit JSON until the next text-edit call or destroy; JSON null without an editor. |
| `onda_ui_text_bounds` | UTF-16 start/end offsets to four logical floats in caller storage; 1 on success, 0 without geometry. |
| `onda_ui_text_hit` | Logical point to UTF-16 offset; `SIZE_MAX` without a text hit. |
| `onda_ui_cursor` | CSS cursor identifier with static lifetime. |

After input/state changes, call update and drain commands. Coalesce redraws through
the host's display/event loop. Arm a one-shot timer for `onda_ui_wake_delay_ms`,
rounding up to the host timer's resolution, then update and drain commands when it
fires. Recompute the delay after draining; overdue work returns zero. Timed work
does not necessarily need rendering. Stop scheduled drawing/timers while hidden,
then update and force presentation when exposed again. If render returns zero,
retry on a later display tick or bounded host timer, even without further input.
Data polling for automation, meters, and scope is independent of UI wakeups and
must not gate input redraws. Pointer moves, state updates, and text edits can
change capture visibility. Send focus loss and drain before hiding/destroying.
Nonzero sample counts require a valid pointer for that many floats. Message
strings are borrowed only during the call.

## Browser adapter

`ui/gpu/index.html` loads the packaged adapter and Wasm. It queues initialization
messages, coalesces value/scope updates into animation frames, resolves automatic
theme selection, handles native textarea editing/clipboard, and suspends drawing
when hidden. Use the certified staged assets described in `ui/gpu/README.md`.

For a same-origin iframe, the parent posts
`{__ondaRunHost:true, message:HostMessage}` to the iframe's explicit origin.
The UI replies `{__ondaRunUi:true, message:Command}`. Validate both event source
and origin. Browser buffer picking/dropping carries a `File`, requiring structured
clone rather than JSON serialization. A top-level custom host can instead call
`window._onHostMessage(message)` and listen for the `ondaCommand` CustomEvent.
VS Code uses `acquireVsCodeApi().postMessage()` and its saved webview state.

GPU device/context loss displays a reload error, releases owned input, stops
capture and rendering, and rejects further state updates until the view reloads.

The browser adapter accepts `requestRunViewReady` to replay `uiReady` or
`runViewError {message}`. It emits `runViewReady {readyId?}` after rendering a
state; match that ID before dismissing loading feedback. It also emits
`scopeConfig {columns,visible,meter}` whenever capture needs change. Additional
host messages are `computerKey {code?,key?,pressed}` and
`releaseVirtualMidiNotes` (computer-owned notes only). Audio hosts handle trusted
activation gestures through the same-origin bridge in `ui/gpu/browser.js`.

Direct Wasm hosts use `CanvasUi.create(canvas, webgpu)` after module initialization.
It exposes `message(json)`, `input(json)`, `resize(width,height,scale)`, `render()`,
`commands()`, `scroll(delta,horizontal)`, `scope_samples()`, `scope_packet()`,
`meter_enabled()`, `scope_enabled()`, `scope_columns()`, `cursor()`, `editing()`,
`text_edit()`, `buffer_target(x,y)`, and `pointer_stops(x,y)` for transport gestures.
`message()`/`input()` return no consumption result; `scroll()` returns movement.
`render()` returns true for presented/clean and false for retry. Both deadlines
are exposed together through `wake_delay_ms()`, or undefined when absent.
`update()` advances timed state/layout without drawing and returns whether a
redraw is needed. Drain commands even when it returns false. The browser host
uses one wakeup timer and coalesces drawing through `requestAnimationFrame`.
`fallback_fonts()` returns a JSON
array of missing bundled font names; fetch those assets and call
`load_font(name, Uint8Array)`, then redraw. `free()` releases the canvas resources.
Register `on_device_lost(callback)` to receive an asynchronous device-loss message,
including while rendering is idle. Custom WebGL hosts also handle the canvas's
`webglcontextlost` event. Release input and capture before disposing a failed view.
Coalesce layout/density changes into the next animation callback. Change the
canvas backing dimensions, call `resize`, and render in that callback: changing
`canvas.width`/`height` discards the displayed image, so doing it earlier exposes
a blank canvas while waiting for the replacement frame. CSS may stretch the last
frame until the callback runs. The stock browser/VS Code host follows this rule.
