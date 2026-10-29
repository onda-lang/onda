# Shared GPU run screen

The shared Rust/wgpu run screen serves standalone Onda, the VS Code panel,
the JUCE plugin, and both browser editors. It provides
a compact source and transport header, collapsible scope, log, buffers, events,
input controls and parameters, plus a docked MIDI keyboard
only for patches with `note_on` support. Parameters and input controls default to
knobs; the segmented Sliders/Knobs selector highlights selection and pointer hover.
Device settings open from the header. Empty sections are hidden and the document scrolls above the keyboard. All hosts use this renderer.

Scalar event fields stay compact, float labels omit binary rounding artifacts,
and buffer previews use individual sample stems and dots with a separate scale gutter.
Numeric labels use the declared or adaptive step to limit displayed decimals.
Dragging and committing text preserve the complete stored and processor values;
small nonzero values use scientific notation when fixed decimals would hide them.

Buffers provide bounded waveform previews, binding/replacement, clearing, and native
file drops. Events support scalar, struct, tuple, array, and slice arguments,
inline fields, whole-value JSON editing, defaults, and generation-aware dispatch.
Logs render visible lines with file, line and lexical-owner context, follow the tail, report transport losses, and pan
horizontally with Shift + scroll. Copy exports the complete log; hovering clipped source
context reveals the full file, line and lexical owner. Header icons have action descriptions
and hovering the filename reveals its full path. The MIDI keyboard supports mouse and multi-touch glissando,
physical computer-key positions, and Z/X octave shortcuts. It monitors host notes and releases owned notes on hide, focus loss,
transport stop, or source changes. Setup exposes the host's source/project actions,
devices, run settings, fast math, and smoothing controls.
The plugin hides computer-key labels with `showComputerKeyLabels: false` and
does not send `computerKey` events, since the DAW owns computer-keyboard MIDI
routing. It forwards unused keyboard input to the DAW; mouse and touch piano
input remain available.
MIDI velocity defaults and resets to 1 in every host; explicit saved values are retained.
The keyboard adds octaves on wider screens and bounds key sizes. Compact
headers place the source and transport below the logo and actions; bank toolbars
wrap before their controls overlap the heading.

Section folds, control layouts, and the document scroll position are retained; parameter arrays and
structured arguments can fold. The log has independent scrolling and tail following. Plugin sessions and VS Code panel state retain presentation and typed
event values. Browser editors retain these locally across page reloads.
Number drags save once when released or cancelled. Scroll persistence waits for
100 ms of inactivity before building a full snapshot;
committed edits, focus loss, and source switches flush pending changes immediately.
Standalone retains presentation per source during the window session.
Browser fields use a native text editor for mobile keyboards, selection, clipboard,
and IME composition. Standalone composition uses winit's IME events; the plugin
exposes the same draft, selection and shaped caret geometry through JUCE's text input interface.

## Standalone

```sh
cargo run --release -p onda_cli --bin onda -- run examples/basic/sine.onda
```

`onda run` without a path opens the source picker, with sample-rate and
block-size dropdowns and Fast Math before loading a source. Dropdowns have bounded
scrolling, selected-item highlighting,
and arrow-key, Enter, Escape, and type-to-search input.
Source/project watching and audio processing use `onda_run::RunController`.
The standalone window prefers native Wayland on Wayland desktops, including niri,
and supports X11 directly. Winit is pinned to 0.31.0-beta.4 for its native file-offer
API; 0.30 lacks Wayland file drops. Drops use event coordinates and highlight one
supported buffer asset or the unloaded source picker for a source/project file.
Asynchronous file delivery retains the target at the drop.
Source/project drops open the source regardless of the card below them; multiple distinct files are ignored.
GPU scope capture targets 30 Hz (about 33.3 ms) in each host, independently
of UI redraws. The time window remains 1,024 audio frames (about 21.3 ms at 48 kHz);
the pixel budget changes only the min/max reduction. The GPU adapter redraws when
input, state, scope contents, or the window changes. Source watching continues
while playback is stopped.
Zero-size and occluded native windows suspend drawing and scope requests;
temporarily unavailable surfaces retry on a deadline without a redraw loop.

## Browser and VS Code

```sh
npm run build:gpu-ui
CHROME_BIN=/path/to/chromium npm run test:gpu-ui
```

Browser tests exercise both hardware WebGPU and software WebGL2 by default.
Set `ONDA_UI_TEST_WEBGPU=0` on machines without a hardware adapter; the WebGPU
cases are reported as skipped. Hosted CI runs this software path. Build and
stage both browser hosts before running editor integration tests:

```sh
npm run build --workspace @onda-lang/wasm-compiler
npm run build:gpu-ui
bash examples/web/onda_wasm_playground/build-demo.sh --skip-build
node scripts/build-website-playground.mjs
CHROME_BIN=/path/to/chromium npm run test:gpu-web-editor
```

The build requires `npm ci`, `wasm-pack`, and the Rust `wasm32-unknown-unknown` target.
It uses the workspace-pinned Binaryen optimizer, matching the browser compiler build.
It generates `ui/gpu/pkg`, which is ignored by Git. After a successful build,
`pkg/build.json` records the Onda version and SHA-256 checksums of the browser
host, Wasm, glue, fonts, and notices. Rebuild after changing the version or any
runtime asset; consumers verify this manifest before staging and packaging.
Serve `ui/gpu` over localhost
or HTTPS and publish existing run-view messages to `window._onHostMessage`.
Commands use the existing VS Code/IPC bridge or an `ondaCommand` CustomEvent.
WebGPU is selected when a hardware adapter is available; otherwise the canvas uses
wgpu's WebGL2 backend. `?backend=webgl` selects that backend explicitly.

The [browser editor](../playground/README.md) uses a same-origin iframe bridge in
`browser.js`. It validates the parent window and origin, forwards browser `File`
objects for buffer binding, and preserves trusted audio-enabling gestures.
Its scope producer sends reusable `Uint8Array` OSQ1 packets without base64 or
JSON float arrays. The website build stages the same renderer under its hashed
asset directory.
Renderer failures send `runViewError { message }` through the readiness bridge,
revealing the error even when the parent is waiting for the first frame.
Readiness requests replay failures as well as successful initialization; reloading
the iframe retries initialization.

To build and try the complete browser editor:

```sh
bash examples/web/onda_wasm_playground/build-demo.sh --serve
```

Open `http://127.0.0.1:8787/`. After changing only JavaScript, add `--skip-build`;
after changing Rust UI code, run `npm run build:gpu-ui` before staging again.
`CHROME_BIN=/path/to/chromium npm run test:gpu-web-editor` checks both editor hosts
against real compilation and muted AudioWorklet processing.

In the VS Code integration worktree:

```sh
npm run stage:gpu -- /path/to/onda/ui/gpu
npm run compile
```

The extension uses the shared GPU canvas with the existing playback
subprocess and control socket. Use a CLI built from the same source for compact
scope requests. Generated GPU assets are staged in `ui/gpu` and
copied into `out/gpu` during ordinary extension builds. The panel's content
security policy permits loading the bundled Wasm without granting general eval.
`npm run package:gpu -- /path/to/onda/ui/gpu [output.vsix]` produces an installable
extension including those assets. `npm run test:gpu-host -- /path/to/onda [package.vsix]`
tests the real VS Code extension host, playback, numeric changes, hidden-panel
suspension, and source reload in a temporary profile.

## Native plugin

```sh
cargo build --release -p onda_ui --features ffi
```

The C interface is in `include/onda_ui.h`. It borrows a native view and display;
the host must destroy the UI before those handles. All calls run on the host's
message thread. Rust owns no native event loop or audio resources. Pointer
coordinates are logical pixels; resize dimensions are physical pixels.

Configure the plugin integration worktree with:

```sh
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DONDA_DEVELOPMENT_SOURCE_DIR=/path/to/onda \
  -DONDA_GPU_UI_LIBRARY=/path/to/onda/target/release/libonda_ui.so
cmake --build build
```

Use the corresponding `.dylib` or `.dll` on macOS or Windows.
`ONDA_GPU_UI_LIBRARY` can override the bundled library path at runtime.
All builds put the library and font
licenses inside each VST3 bundle at `Contents/Resources/onda_ui`. The native loader
finds it relative to its own plugin module, allowing the bundle to be relocated.
Release builds obtain the library and header from the Onda SDK. The native
adapter has Xlib, Win32, and AppKit handle paths.

Scope snapshots cross the C interface as interleaved floats, without JSON.
Controls consume the plugin's normalized host-domain metadata and send existing
automation commands, including gesture termination on cancellation. The host
continues to own parameter mappings and audio-thread isolation.

The dedicated `onda_gpu_ui_tests` CTest target checks native surface creation,
numeric editing, scope uploads, resizing, hide/show, and repeated destruction.
It requires a desktop display on Linux.
The plugin's `scripts/test-gpu-reaper.py build` checks relocated synth/effect
bundles in the actual REAPER host, including native editing, automation, resize,
and reopening. It uses a temporary configuration and disables test tracks'
master sends. It requires X11/XTest, REAPER, and ImageMagick; run it with
`xvfb-run -a` for an isolated display.

The [Linux host-validation record](host-validation.json) captures checks against
an installed VSIX in the real VS Code extension host and relocated synth/effect
bundles in REAPER, with artifact hashes and the remaining limitations. These are
functional checks; the [render/transport benchmarks](../../scripts/bench/README.md#gpu-ui-measurements)
measure performance separately.

## Rendering and verification

`crates/onda_ui` owns the UI state, interaction, cached scene, WGSL primitives,
scope drawing, and text atlas. It reuses `onda_mir::ParamDomain` for numeric
conversion and glyphon for shaping/rasterization. Inter supplies compact UI labels;
JetBrains Mono supplies parameter names, values, logs and diagnostics, including
real regular and semibold weights. Both are embedded under their SIL Open Font
License notices in `assets/fonts/`.
Smoothing fields and layout selectors use compact 12 px proportional text;
browser text editors follow the canvas field's font and size.
Noto fallback fonts cover CJK, Arabic, Hebrew and emoji. The build derives a compact
coverage index from their actual glyph maps;
native hosts load needed fonts from embedded bytes and browsers fetch only the
needed self-hosted fonts from `pkg/fonts/`, outside the initial Wasm download.
Initialization does not scan system fonts. One palette owns the shared dark/light colors. Primitives,
text, and clear colors use the target's color space, giving native sRGB surfaces
and browser surfaces the same palette. Vector icons, knob arcs, checkmarks and
scope channels share antialiased WGSL primitives. Geometry and text are clipped
to the document, log, or popup bounds before the dock and settings overlay are painted.
The Onda logo follows the original SVG curves, with one antialiased contour for
the wave so overlapping segment edges cannot thicken its stroke. Browser canvases
follow display density changes as well as window resizing.

Scene geometry and shaped text are retained across scope updates. Control-bank and event
structural layout and keyboard targets are cached across value, hover and scroll changes. Visible rows
are clipped and culled. GPU instance buffers retain capacity. Scope vertices are
expanded in WGSL from signed 16-bit min/max envelopes. Each bucket covers two
physical pixels, capped at 512 columns, and retains peaks across the whole capture.
Adjacent buckets form connected, antialiased bands with geometry bounded to each
channel's row. Identical envelopes skip both uploads and redraws.
Frames use one render pass; the host controls
when to request another frame. Native adapters drain commands again at
`RunUi.view_state_deadline()` while idle; the C adapter's host already drains
commands on each UI timer tick. Browser frames are coalesced with
`requestAnimationFrame`: only the latest scope and each parameter value are applied.
Hidden pages suspend drawing. Hidden or collapsed scopes suspend capture in all three hosts.

GPU hosts request `getScopeData` with a `columns` display budget and `maxFrames`.
The producer reduces its ring directly, without allocating a full float snapshot.
It returns `{ packet: "<base64>" }`: OSQ1 magic, little-endian u16 channel and
column counts, then channel-major pairs of little-endian i16 extrema. A 348-column
stereo frame uses 2,792 bytes before base64. The Wasm boundary accepts decoded bytes
directly; raw `scopeData { channels, samples }` is also reduced through a typed
float buffer. Neither path serializes scope samples into Rust JSON.

```sh
cargo test -p onda_ui --lib
cargo test -p onda_gpu --lib
cargo run --release -p onda_ui --example benchmark -- /tmp/onda-gpu-ui.png
cargo run --release -p onda_ui --example panels -- target/ui-panels
```

The benchmark renders the production screen into a texture, checks that pixels
were drawn, and writes a PNG. It reports warm median/p95 CPU-plus-GPU completion
time for 64 controls and 1,024 stereo samples at 768×840. It does not measure
window presentation latency, audio underruns, or a speedup over another editor.
The native adapter suite checks asynchronous file delivery, frozen buffer targets,
cancellation, stale responses, and file URI decoding. The ignored
`standalone_prefers_native_wayland_over_the_xwayland_bridge` test requires a
Wayland display; `xdnd_file_drop_retains_the_drag_coordinates_through_winit`
requires an X11 display and exercises the real URI-list protocol without moving
the pointer. Run either by name with `-- --ignored`.
The complete-panel harness writes a PNG for each section and measures scope-only and
changing-panel workloads separately. The browser tests run real Wasm, shaders,
text, pointer/keyboard editing, each section, MIDI release, scope collapse, resizing,
hardware WebGPU, and the WebGL2 software fallback in Chromium. On Linux the
hardware test selects Vulkan; override graphics flags with ONDA_UI_CHROME_ARGS.
Chrome's experimental SwiftShader WebGPU path currently fails when glyphon grows
a mapped text buffer; the supported software fallback is WebGL2.
On this Linux headless setup the Vulkan compositor can omit iframe layers in
page screenshots. The editor suite also reads the canvas pixels directly and
uses SwiftShader WebGL for the complete-page fallback preview.

The [browser-editor validation record](web-editor-validation.json) records
real compiler/AudioWorklet checks for both browser hosts, backends, autoplay
blocking, and hosting without cross-origin isolation.

## Input and state interface

State messages retain the existing `state`, `parameterValues`, `scopeData`,
`midiActivity`, `bufferFileDrag`, and `sourceFileDrag` shapes. Unknown fields and unsupported controls are ignored. Full state changes
replace supplied control banks; value publications only change known controls.
Focused drafts survive value updates; source/schema replacement cancels them.
Hosts validate supported buffer file formats before forwarding drops; the shared
screen checks the target and requires one distinct, nonempty path.
Saved event values include ordered argument names, shapes, and defaults and restore
only against a matching signature. Changed event defaults cancel active drafts
and discard preserved argument values.
Hosts publish `schemaReady: false` while loading control metadata, then `true`
after all banks arrive (or loading fails). Saved presentation waits for that
publication, so early empty states cannot discard event values or clamp saved scrolling.
Synchronous state producers may omit the field.
An authoritative presentation restore carries a fresh `viewState.readyId`, even
when the source path is unchanged. Repeated IDs and ordinary saved-state echoes
do not overwrite local edits.
Resetting presentation retains event schemas and restores their argument defaults.

Input is a tagged JSON object: `pointerDown { x, y, clicks }`,
`pointerMove { x, y, fine }`, `pointerUp`, `pointerLeave`, `scroll { delta }`,
`horizontalScroll { delta }`, `text { text }`, `key { key, shift }`,
`computerKey { key, pressed }`, `releaseComputerKeys`, or `focusLost`.
Touch adapters send `touchStart { id, x, y }`, `touchMove { id, x, y }`,
`touchEnd { id }` and `touchCancel { id }`. Piano fingers own notes independently;
other gestures retain one controlling finger.
Vertical swipes pan the original document, log, settings, or menu; horizontal number
drags and direct slider, knob, and MIDI gestures keep their control ownership.
Unconsumed vertical pan distance emits `scrollParent { deltaY }` in logical pixels.
The browser adapter applies it to scrollable ancestors across same-origin frames,
respecting their overscroll policy. Hosts without an enclosing scroller ignore it.
Browser `touchMove` events include optional `deltaY` (positive to scroll down),
measured independently of local coordinates so scrolling the page under a finger
does not alter its movement.
Releasing computer keys preserves pointer-owned MIDI notes and active field drafts.
Keys include Enter, Escape, Backspace, Delete, ArrowLeft, ArrowRight, Home, End,
Tab, SelectAll, Copy, and Cut. Shift extends text selections; `copyText { text }`
commands are consumed by the host clipboard adapter. `CanvasUi.text_edit()` describes
the active field and its logical rectangle; browser editors synchronize native text
through `editText { text, anchor, cursor }`, using UTF-16 selection offsets.
The browser/native adapters translate platform input into
these events. Hosts forward pointer movement without a held button for hover and
scroll targeting, and use `CanvasUi.cursor()` or `onda_ui_cursor()` for the cursor.
`RunUi::input()` and `Renderer::input()` report whether the event was consumed.
The C function `onda_ui_input()` returns 1 for consumed input, 0 for unused
input, and -1 for an error. Embedded hosts forward unused keys to their parent.
`CanvasUi.scroll(delta, horizontal)` returns whether the targeted region moved.
Browser hosts cancel wheel events only when it returns true, allowing normal
page scrolling through the iframe when the UI has no overflow or reaches an edge.
Numbers drag horizontally and enter text editing on a click. Sliders follow the
track position; knobs drag vertically. Double-click resets a control, Shift-drag
adjusts finely, Tab traverses the header, sections, fields, and MIDI settings while
skipping disabled controls. Enter activates a focused action or commits a draft;
Escape cancels. Fields accept pasted text and scroll to keep the caret visible.
Section headers use 40px rows and wrap their tools when needed. Names stay on one line,
and disclose their full text on hover. Slider headings share the value field's vertical center, and centered
control headings use compact 12px font at widths up to 400px. Card titles
and actions share their content inset. The source picker centers the controls
actually displayed; settings selectors and buttons share a 32px height.
Keyboard navigation shows focus; pointer and touch activation retain logical
focus without leaving action buttons highlighted after the pointer leaves.
Captured drags keep hover feedback on their owner. Smooth uses the
1 ms per pixel sensitivity, whole-millisecond steps, and a zero lower bound;
reset preserves the host's exact default. Sliders and knobs use the
track, thumb, arc, and spacing rules.
Hovering a parameter or input-control heading reveals its full name and type in
a wrapped tooltip after 500 ms, including when the compact card clips the heading.
Moving within the same heading retains the delay; leaving or interacting dismisses it.
Idle native and browser hosts wake at the tooltip deadline without continuous redraws.
Arrow keys use the control's declared or adaptive numeric step. The single
Start/Stop button and transport shortcuts remain available during invalid edits.
