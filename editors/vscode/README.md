# Omni VSCode Extension

This extension adds VSCode support for Omni:

- `.omni` language registration
- syntax highlighting
- semantic tokens from `omni lsp`
- `Omni: Run Patch`
- `Omni: Stop Patch`
- `Omni: Restart Language Server`

## Requirements

- VSCode 1.90 or newer
- an `omni` executable available on `PATH`, or an explicit configured path

If you need to build the CLI locally:

```bash
cargo build -p omni_cli --release
```

That produces the binary at:
- Windows: `target/release/omni.exe`
- macOS/Linux: `target/release/omni`

## Install

### Option 1: install a `.vsix`

If you already have a packaged `.vsix`, install it with one of these:

- VSCode Command Palette: `Extensions: Install from VSIX...`
- CLI:

```bash
code --install-extension omni-vscode-0.0.1.vsix
```

### Option 2: build a `.vsix` locally from this repo

From `editors/vscode/`:

```bash
npm install
npm run compile
npx @vscode/vsce package --skip-license
```

That produces a `.vsix` file in `editors/vscode/`, which you can then install with:

```bash
code --install-extension ./omni-vscode-0.0.1.vsix
```

If you prefer the UI, use `Extensions: Install from VSIX...` and select the generated file.

## Configuration

By default the extension starts:

```text
omni lsp
```

You can override the executable and prepend extra args in VSCode settings:

- `omni.server.path`
- `omni.server.args`

Example settings:

```json
{
  "omni.server.path": "C:/path/to/omni.exe",
  "omni.server.args": []
}
```

Or on macOS/Linux:

```json
{
  "omni.server.path": "/path/to/omni",
  "omni.server.args": []
}
```

## Using the extension

Open an `.omni` file and the extension will activate automatically.

Available commands:
- `Omni: Run Patch`
- `Omni: Stop Patch`
- `Omni: Restart Language Server`

`Omni: Run Patch` starts the preview transport and opens the patch UI.

## Development

If you want to work on the extension itself:

```bash
npm install
npm run compile
```

Then open `editors/vscode/` in VSCode and launch an Extension Development Host.
