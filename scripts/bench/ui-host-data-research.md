# Host and data ideas from the earlier UI attempts

Inspected on 2026-10-27: the working trees at `../onda`, `../onda-shader`, and
this checkout, `onda-shader2`. This is source research, with focused copy and
publication experiments; it does not establish an end-to-end host speedup. The current UI's
layout, text, numeric editing, and scope quality should remain the product target.

## Findings in priority order

| Priority | Idea | Where it helps | Evidence status |
| --- | --- | --- | --- |
| 1 | Subscribe to scope capture at the producer | Native standalone and headless playback; hidden/collapsed viewers | Clear missing-work elimination; application impact still to measure |
| 2 | Copy ring input in at most two contiguous spans | Native playback producer | Equivalent logical windows checked; substantial kernel reduction, small stereo absolute cost |
| 3 | Wake the host on controller events and schedule actual deadlines | Native standalone and native webview | Earlier implementation demonstrates the mechanism; wake and latency measurements needed |
| 4 | Publish only changed fields; retain cold schemas | Native UI adapters, especially large event/log schemas | Current partial-message path is measured; typed rewrite remains optional |
| 5 | Cache static UI pixels between scope frames | GPU rendering, especially integrated/mobile hardware | Conditional experiment, not a demonstrated win for the current renderer |

### 1. Make scope capture a viewer subscription

In `../onda`, the ring starts disabled and refuses writes without a viewer.
The control command changes `ring.enabled` and resets capture history. The
controller's visibility operation also sends the subscription change to the
child, rather than only stopping requests. See [old ring writes](/home/francesco/Sources/onda/crates/onda_run/src/playback.rs:269),
[subscription handling](/home/francesco/Sources/onda/crates/onda_run/src/playback.rs:2548),
and [visibility propagation](/home/francesco/Sources/onda/crates/onda_run/src/control_client.rs:97).

Here `set_scope_visible` only stores a controller flag, and `poll` gates the next
request with it. The producer ring still accepts samples independently of that
flag. See [current visibility](/home/francesco/Sources/onda-shader2/crates/onda_run/src/lib.rs:448),
[request gating](/home/francesco/Sources/onda-shader2/crates/onda_run/src/lib.rs:583),
and [current ring writes](/home/francesco/Sources/onda-shader2/crates/onda_run/src/playback.rs:278).

Port the subscription behavior for standalone playback. Do not introduce a
duplicate plugin mechanism: [the current plugin already has capture enablement](/home/francesco/Sources/onda-plugin-gpu/src/ScopeCapture.h:36).
Keep one-shot raw scope inspection possible, define connection ownership, stop
capture on disconnect, and reject old replies after visibility or source changes.
The benefit is avoided producer work when no scope is viewed; it is not a faster
shader. Measure hidden, collapsed, stopped, and headless playback separately.

### 2. Replace sample-by-sample modulo with contiguous copies

The old ring retains the newest capacity-sized suffix, calculates the first
contiguous span, then uses two `copy_from_slice` calls. The current writer computes
a runtime modulo for every sample. The logical chronological window can stay
identical even if the physical ring offset after an oversized write differs.
See [old implementation](/home/francesco/Sources/onda/crates/onda_run/src/playback.rs:280)
and [current implementation](/home/francesco/Sources/onda-shader2/crates/onda_run/src/playback.rs:278).

A focused optimized-Rust experiment extracted both method bodies, alternated
seven rounds on CPU 0, kept capacity runtime-visible, and checked logical window equality
for 1/2/8/64 channels, wraparound, and oversized writes. Representative 512-frame
block medians were:

| Channels | Current modulo writer | Two spans | Kernel ratio |
| --- | ---: | ---: | ---: |
| 2 | 1,104 ns | 27.1 ns | 40.8× |
| 8 | 4,412 ns | 186 ns | 23.7× |
| 64 | 35,260 ns | 1,786 ns | 19.7× |

At 48 kHz/512 frames the stereo difference is only about 0.01% of one CPU core.
Do not call this a forty-fold application speedup. Preserved evidence:
[results](results/2026-10-27-ui-host-data/copy.json),
[extracted Rust harness](results/2026-10-27-ui-host-data/copy.rs),
[provenance and reproduction](results/2026-10-27-ui-host-data/metadata.json).
The isolated methods are deliberately not inlined; this measures the writer
kernel, not its final inlined cost within the producer or DSP throughput.

### 3. Replace periodic controller draining with event wakeups

`../onda` wraps event sending so a channel publication wakes the host.
The winit adapter routes that wake to `EventLoopProxy` and drains the controller
in `user_event`. Its background maintenance deadline is one second. See
[event sender](/home/francesco/Sources/onda/crates/onda_run/src/publication.rs:31),
[controller wake interface](/home/francesco/Sources/onda/crates/onda_run/src/lib.rs:430),
[proxy registration](/home/francesco/Sources/onda/crates/onda_egui/src/native.rs:228),
and [wake handling](/home/francesco/Sources/onda/crates/onda_egui/src/native.rs:285).

The current GPU host drains only when `next_poll` is due: approximately every
33 ms while running and visible, or every 100 ms otherwise. Its reader thread
publishes into a plain channel without waking the window. A ready reply can
therefore wait for the next polling deadline. See [host polling](/home/francesco/Sources/onda-shader2/crates/onda_gpu/src/lib.rs:524)
and [reader publication](/home/francesco/Sources/onda-shader2/crates/onda_run/src/control_client.rs:47).

Reuse the event-driven principle. A new worker is optional: the existing host can
sleep until the earliest capture, watcher-maintenance, surface-retry, drag, or
presentation-save deadline, and wake immediately for asynchronous events. This
can reduce idle wakeups and reply latency without increasing the scope rate.
Coalesce wake notifications under dense log traffic, wake for events queued
before callback registration, and avoid lost-wakeup races.

The old dedicated scope worker also demonstrates useful backpressure: it keeps
one request outstanding until the UI consumes the decoded frame, and parks while
disabled. Its session/subscription epochs are the right invariant to preserve.
However, do not copy its exact separate atomic `pending_id`/`pending_epoch`
association without auditing races; store the in-flight request and epoch as a
coherent state. See [old reply decoding](/home/francesco/Sources/onda/crates/onda_run/src/control_client.rs:54),
[epoch changes](/home/francesco/Sources/onda/crates/onda_run/src/control_client.rs:97),
and [worker/backpressure](/home/francesco/Sources/onda/crates/onda_run/src/control_client.rs:217).

### 4. Publish only changed fields before considering a typed rewrite

`../onda` exposes `Arc<RunState>` snapshots and uses copy-on-write for mutation.
Its egui renderer borrows that snapshot while painting. This is useful ownership
design, but it does not eliminate mutation-time clones if another owner still
retains the snapshot. See [shared publication](/home/francesco/Sources/onda/crates/onda_run/src/publication.rs:6)
and [frame borrow](/home/francesco/Sources/onda/crates/onda_egui/src/lib.rs:1273).

`../onda-shader` moves an outer `RunState` type into the compiler-free host-protocol
crate, but its control/event banks remain `Vec<serde_json::Value>`. More
importantly, its native adapter serializes the controller to a `Value`, and the
UI serializes its existing state, merges the update, then deserializes a new
`RunState`. This is a partial design precedent, not a zero-copy implementation to
transplant. See [outer schema](/home/francesco/Sources/onda-shader/crates/onda_host_protocol/src/run_ui.rs:7),
[native projection](/home/francesco/Sources/onda-shader/crates/onda_egui/src/lib.rs:96),
and [UI merge](/home/francesco/Sources/onda-shader/crates/onda_ui/src/messages.rs:24).

The current retained UI already avoids a complete state clone per render. Its
remaining same-process publication cost is the native JSON-shaped projection,
followed by metadata cloning/reconstruction in `RunUi::message`. Log-only
publications avoid resending control banks but still copy accumulated log text
and entries. See [current host projection](/home/francesco/Sources/onda-shader2/crates/onda_gpu/src/lib.rs:105),
[log projection](/home/francesco/Sources/onda-shader2/crates/onda_run/src/view_state.rs:118),
and [control reconstruction](/home/francesco/Sources/onda-shader2/crates/onda_ui/src/model.rs:334).

A focused current-public-interface experiment used seven rounds of 500
updates per case, pinned to CPU 0; cases ran in a fixed order. It includes cloning the incoming `Value` and applying
`RunUi::message`, but excludes host projection, JSON parsing, scene preparation,
GPU work, and presentation:

| Existing controls | Full state to change status | Partial `{status}` | One `parameterValues` update |
| --- | ---: | ---: | ---: |
| 64 | 76.8 µs | 0.226 µs | 0.254 µs |
| 512 | 571 µs | 0.229 µs | 0.259 µs |
| 4,096 | 5,186 µs | 0.233 µs | 0.250 µs |

The first implementation step is therefore to use the already-supported partial
messages for status/log-only changes. That avoids resending and rebuilding
unchanged control banks without introducing a replacement protocol.
In particular, the current plugin falls through to its full-state builder when
only `logRevision` changes; standalone already publishes a separate log patch.
See [plugin full-state decision](/home/francesco/Sources/onda-plugin-gpu/src/Editor.cpp:663)
and [standalone log patch](/home/francesco/Sources/onda-shader2/crates/onda_gpu/src/lib.rs:127).
Preserved evidence: [publication results](results/2026-10-27-ui-host-data/publication.json),
[Rust harness](results/2026-10-27-ui-host-data/publication.rs),
[provenance and reproduction](results/2026-10-27-ui-host-data/metadata.json).
Both tables report medians of per-round averages, not per-call latency
percentiles. They are local diagnostic measurements with normal turbo/governor
behavior, not release performance guarantees.

If further profiling confirms substantial remaining projection cost, keep
schemas immutable by source generation and use narrow typed updates for
values/status/logs in Rust/C hosts. Keep JSON at JavaScript and external-process
transport interfaces where it is needed. Do not replace the whole state with
`Arc` merely to reproduce an old optimization the retained renderer already
achieves. A log-delta design also needs generation, bounded history, loss
accounting, and recovery from missed messages.

### 5. Experiment with static UI pixel retention only if GPU cost matters

The older shader attempt can cache rasterized static UI pixels and composite
them while updating the GPU scope separately. It falls back to ordinary drawing
when later content overlaps the scope, preserving popup paint order. See
[retained scene](/home/francesco/Sources/onda-shader/crates/onda_ui/src/gpu/scene.rs:1),
[cache invalidation](/home/francesco/Sources/onda-shader/crates/onda_ui/src/gpu/mod.rs:48),
and [overlap test](/home/francesco/Sources/onda-shader/crates/onda_ui/src/gpu/scene.rs:153).

Here geometry, glyph shaping, and GPU buffers are retained, but unchanged static
shapes and text are still drawn into each scope frame. An offscreen static layer
could reduce repeated primitive/glyph work on some GPUs. See [preparation cache](/home/francesco/Sources/onda-shader2/crates/onda_ui/src/renderer.rs:326)
and [per-frame drawing](/home/francesco/Sources/onda-shader2/crates/onda_ui/src/renderer.rs:405).

This adds a texture (about 2.5 MiB at 768×840 RGBA8, or 9.8 MiB at 2× density),
extra rendering when the UI changes, and more cache invalidation/paint-order
complexity. Measure GPU time and whole-process CPU on integrated/mobile hardware
before adopting it. Keep the current GPU envelope scope and antialiasing; static
UI pixel retention does not require rasterizing the waveform on the CPU.

## Ideas already present here, or not worth transplanting

- Current browser frames already coalesce to `requestAnimationFrame`, retain only
  the newest pending scope and value per control, and suspend while hidden.
  [Browser scheduling](/home/francesco/Sources/onda-shader2/ui/gpu/host.js:81),
  [pending data](/home/francesco/Sources/onda-shader2/ui/gpu/host.js:25).
- Scope updates already avoid scene/text rebuilding and unchanged envelopes avoid
  uploads. [Renderer revisions](/home/francesco/Sources/onda-shader2/crates/onda_ui/src/renderer.rs:350),
  [scope revision comparison](/home/francesco/Sources/onda-shader2/crates/onda_ui/src/model.rs:566).
- Current OSQ1 packets already remove raw-float JSON transport and retain min/max
  extrema. Do not copy the old fixed 160-point, two-channel, signed-byte product
  limits to obtain smaller payloads. [Current codec budget](/home/francesco/Sources/onda-shader2/crates/onda_scope/src/lib.rs:1),
  [old requested budget](/home/francesco/Sources/onda/crates/onda_run/src/control_client.rs:245).
- Current code already disables Nagle for native control sockets. It is not a
  remaining idea from the old bridge. [Current socket setup](/home/francesco/Sources/onda-shader2/crates/onda_run/src/control_client.rs:34).
- Do not bring back egui/eframe or their vendor cache hooks. The existing renderer
  already owns dirty state and host scheduling directly.

## Measurements needed before broader changes

The old shader report measured whole browser/Electron process CPU, including GPU
processes, while the current browser renderer harness reports main-thread CPU;
the workloads, scope sizes, and control counts differ. Those numbers cannot rank
these implementations directly. See [old measurement scope](/home/francesco/Sources/onda-shader/scripts/bench/ui/results/2026-10-26/README.md:31)
and [current measurement scope](/home/francesco/Sources/onda-shader2/scripts/bench/README.md:75).

For each candidate, use the same current renderer, source, scope quality, capture
cadence, viewport/DPI, audio configuration, and host. Collect idle/stopped/hidden,
scope-only, dense automation, and bounded-log cases. Measure CPU across host/UI,
playback, renderer and GPU processes separately, plus wakeups, allocations, wire
bytes, and publication-to-presentation latency. Keep the small copy-kernel
experiment separate from application results.


## Imported implementation and validation (2026-10-27)

The four recommended changes are now implemented:

- `ScopeCapture` owns producer subscriptions, the atomic disabled fast path,
  disconnect cleanup, ring storage, and bulk reads/writes. Reconfiguration retains
  the subscription. Standalone and VS Code hosts subscribe only while a scope is
  visible; the plugin already had producer capture enablement.
- Controller event senders coalesce wakes, including work queued before callback
  registration. The native GPU window waits for these events, scope deadlines,
  presentation deadlines, or infrequent maintenance. The pending-wake flag resets
  before draining events to avoid losing a wake.
- Scope request IDs and VS Code subscription epochs reject stale responses after
  visibility, resolution, transport, or connection changes. Old replies cannot
  release a newer request. Capture remains bounded to one current request.
- Native status, smoothing, parameter and log updates use partial publications.
  Browser state patches stay partial; VS Code filters unchanged fields before
  serialization and sends parameter values separately. Plugin log revisions use
  a log-only projection. Source changes and readiness still send full snapshots.

The [production-path capture benchmark](results/2026-10-27-ui-host-data/capture.rs)
includes the new atomic gate and uncontended ring mutex. Its comparison baseline
is the previous writer kernel alone, without the previous surrounding mutex.
Local CPU 0 medians for 512-frame blocks:

| Channels | Previous writer kernel | Enabled production capture | Disabled capture |
| --- | ---: | ---: | ---: |
| 2 | 1,106 ns | 32.2 ns | 0.54 ns |
| 8 | 4,410 ns | 194.7 ns | 0.54 ns |
| 64 | 35,238 ns | 1,771.5 ns | 0.54 ns |

These are isolated CPU costs, not an application speedup measurement. Exact
results and provenance are in [capture.json](results/2026-10-27-ui-host-data/capture.json)
and [implementation.json](results/2026-10-27-ui-host-data/implementation.json).

The real playback probe delivered 91 fresh envelopes over three seconds (30.3 Hz)
with a 33 ms target and a 33.28 ms median gap. After hiding the scope, only two
controller polls occurred over 1.2 seconds, including the immediate transition.
See [cadence.json](results/2026-10-27-ui-host-data/cadence.json). The probe now uses
actual wake callbacks and controller deadlines rather than sleeping at the scope
interval.

Validation passed across Rust library suites, strict controller/GPU Clippy,
browser unit tests, GPU canvas and both web editor browser tests, VS Code unit
and real extension-host tests, and JUCE GPU surface/editor and log adapter tests.
The extension-host probe explicitly requests the initial readiness snapshot after
attaching its observer; subsequent status changes correctly omit capabilities and
unchanged control banks. Static pixel caching remains a separate experiment.
