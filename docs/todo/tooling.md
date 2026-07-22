# Tooling TODO

## Editor / daemon follow-ups

- Expand `onda lsp` beyond diagnostics + semantic tokens:
  add hover, go-to-definition, document symbols, completion, and cancellation-aware analysis scheduling.
  - Hover:
    - resolved symbol kind and type
    - proc endpoint and param metadata
    - namespace/template specialization info
    - doc comments once the language has a comment convention
  - Completion:
    - top-level and namespace symbols
    - stdlib modules and members
    - proc endpoints after `instance.`
    - event names after proc receivers
    - fields/methods for structs
  - Go-to-definition/references:
    - local symbols
    - imported and included files
    - namespace-specialized declarations
    - generated proc-local def symbols mapped back to source locations
- Improve diagnostic cadence:
  evaluate publish-on-change/debounced diagnostics in addition to the current open/save flow.
- Stabilize daemon/editor transport boundaries:
  decide which run-control pieces stay private versus becoming a documented protocol.
- Keep VSCode syntax highlighting and semantic tokens aligned as the language grows.
- Add an extension smoke-test path or automation for:
  `onda lsp`, `Onda: Run File`, semantic tokens, and run webview controls.
- Improve run panel UX:
  better knob/slider affordances, richer status/errors, and explicit device/runtime state display.
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
