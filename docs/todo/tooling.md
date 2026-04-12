# Tooling TODO

## Editor / daemon follow-ups

- Expand `onda lsp` beyond diagnostics + semantic tokens:
  add hover, go-to-definition, document symbols, completion, and cancellation-aware analysis scheduling.
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
