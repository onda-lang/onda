# Omni VSCode Extension

This extension provides:

- `.omni` language registration
- syntax highlighting
- `omni lsp` client wiring over stdio

## Development

1. Install dependencies:
   - `npm install`
2. Compile the extension:
   - `npm run compile`
3. Open `editors/vscode/` in VSCode and launch the extension host.

By default the extension starts `omni lsp`. Override the executable with the `omni.server.path` setting if needed.
