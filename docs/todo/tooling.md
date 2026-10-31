# Tooling TODO

## Editor / daemon follow-ups

- Deepen language-server symbol information where the language has not defined enough source
  metadata yet: namespace/template specialization details, doc comments once their syntax and
  attachment rules are defined, references, and source mapping for generated proc-local symbols.
- Add cancellation-aware analysis scheduling.
- Clarify daemon/editor transport ownership and versioning beyond the documented
  [UI host protocol](../ui-api.md).
- Keep VSCode syntax highlighting and semantic tokens aligned as the language grows.
- Extend extension-host smoke coverage to language-service diagnostics, completion,
  and semantic tokens.
- Add bulk JSON editing for structured event payloads alongside the existing typed
  controls, with schema validation and precise diagnostics.
- Ship a static `onda_ui` library alongside the shared library in native release SDKs,
  with CMake targets carrying platform link dependencies and tests linking it with static `libonda`.
- Support native Wayland in the `onda_ui` C API alongside Xlib: accept borrowed
  `wl_display*`/`wl_surface*` handles, document host-owned surface/event-loop lifetimes,
  and test rendering, input, scaling, resize, and teardown without XWayland.
  Keep the JUCE plugin on X11/XWayland until JUCE and DAW embedding support permit
  native Wayland; standalone `onda run` already supports both backends through winit.
- Expand declared-event-aware on-screen MIDI controls beyond piano notes:
  - show pitch bend, modulation (CC 1), sustain (CC 64), channel pressure, and configurable CC
    controls only when the corresponding canonical event is declared
  - share control behavior and reset/lifecycle semantics across native and browser GPU hosts
  - validate the existing native timestamp-to-frame scheduling with hardware-backed
    Linux/macOS/Windows coverage, and cover browser MIDI timing separately
- Improve print/delegate Log UX:
  - report UI-history eviction separately from generated batch overflow and transport loss
  - decide and consistently apply whether explicit processor restarts retain or clear visible history
  - evaluate source navigation, per-site filtering, and a sample-scope print lint without silently
    throttling, sampling, or changing authored execution
  - introduce stable cross-recompile log-site identity only if editor navigation or retained-history
    workflows need it; processor artifact site indices intentionally remain artifact-local
- Broaden run buffer ingestion beyond WAV and typed `.ondabuffer` assets if warranted, using the
  shared `onda_project` decoders across native and browser hosts.

## Visual graph editor

- Build the visual graph editor as an Onda source editor, not as a separate runtime:
  - proc declarations become node types
  - node instances are emitted into `init`
  - connections are emitted into `graph`
  - user-authored Onda remains compilable without the visual editor
  - unsupported source shapes degrade to read-only or text-only views rather than lossy rewrites
- Initial scope:
  - single-file graph editing first
  - instantiate existing procs and stdlib procs
  - connect scalar and fixed-array endpoints using current graph rules
  - edit scalar params with existing range/default metadata
  - JIT/recompile through the daemon on graph changes
  - show compiler diagnostics inline on nodes and edges
- Round-trip strategy:
  - preserve non-graph sections as text
  - own a clearly delimited generated `init` / `graph` region for MVP
  - later support richer parsing of hand-written graph code into editable nodes
  - avoid formatting churn in unrelated source
- Graph metadata needed from compiler/daemon:
  - node/proc declarations and endpoint schemas
  - param ranges/defaults/types
  - buffer requirements
  - resolved edge list after graph lowering
  - stable diagnostic anchors for endpoints and edges
- UX follow-ups:
  - searchable node palette grouped by stdlib category
  - connection validation while dragging
  - keyboard-driven node creation
  - compact inspector for params, buffers, events, and generated code
  - graph minimap only if real patch sizes justify it
- Integration options:
  - VSCode GPU panel first, sharing the existing run panel transport
  - standalone `onda run --graph` or `onda graph <file>` later
  - optional browser-hosted version reusing the existing playground compiler worker and AudioWorklet
    adapter
