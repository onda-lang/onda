# Tooling TODO

## Editor / daemon follow-ups

- Deepen language-server symbol information where the language has not defined enough source
  metadata yet: namespace/template specialization details, doc comments once Onda has a comment
  convention, references, and source mapping for generated proc-local symbols.
- Add cancellation-aware analysis scheduling.
- Improve diagnostic cadence:
  evaluate publish-on-change/debounced diagnostics in addition to the current open/save flow.
- Stabilize daemon/editor transport boundaries:
  decide which run-control pieces stay private versus becoming a documented protocol.
- Keep VSCode syntax highlighting and semantic tokens aligned as the language grows.
- Add an extension smoke-test path or automation for:
  `onda lsp`, `Onda: Run File`, semantic tokens, and run webview controls.
- Improve run panel UX:
  better knob/slider affordances, richer status/errors, and explicit device/runtime state display.
- Improve print/delegate Log UX:
  - report UI-history eviction separately from generated batch overflow and transport loss
  - decide and consistently apply whether explicit processor restarts retain or clear visible history
  - evaluate source navigation, per-site filtering, and a sample-scope print lint without silently
    throttling, sampling, or changing authored execution
  - introduce stable cross-recompile log-site identity only if editor navigation or retained-history
    workflows need it; processor artifact site indices intentionally remain artifact-local
- Broaden run buffer ingestion beyond current WAV-only `hound` path if warranted.

## Visual graph editor

- Build the visual graph editor as an Onda source editor, not as a separate runtime:
  - proc declarations become node types
  - node instances are emitted into `init`
  - connections are emitted into `graph`
  - user-authored Onda remains compilable without the visual editor
  - unsupported source shapes degrade to read-only or text-only views rather than lossy rewrites
- MVP scope:
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
  - VSCode webview panel first, sharing the existing run panel transport
  - standalone `onda run --graph` or `onda graph <file>` later
  - optional browser-hosted version once WASM/AudioWorklet export is first-class
