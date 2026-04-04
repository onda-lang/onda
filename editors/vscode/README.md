# Onda VSCode Extension

This extension adds VSCode support for Onda:

- `.onda` language registration
- syntax highlighting
- semantic tokens from `onda lsp`
- `Onda: Run Patch`
- `Onda: Stop Patch`
- `Onda: Restart Language Server`

## Requirements

- VSCode 1.90 or newer
- an `onda` executable available on `PATH`, or an explicit configured path

If you need to build the CLI locally:

```bash
cargo build -p onda_cli --release
```

That produces the binary at:
- Windows: `target/release/onda.exe`
- macOS/Linux: `target/release/onda`

## Install

### Option 1: install a `.vsix`

If you already have a packaged `.vsix`, install it with one of these:

- VSCode Command Palette: `Extensions: Install from VSIX...`
- CLI:

```bash
code --install-extension onda-vscode-0.0.1.vsix
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
code --install-extension ./onda-vscode-0.0.1.vsix
```

If you prefer the UI, use `Extensions: Install from VSIX...` and select the generated file.

## Configuration

By default the extension starts:

```text
onda lsp
```

You can override the executable and prepend extra args in VSCode settings:

- `onda.server.path`
- `onda.server.args`

Example settings:

```json
{
  "onda.server.path": "C:/path/to/onda.exe",
  "onda.server.args": []
}
```

Or on macOS/Linux:

```json
{
  "onda.server.path": "/path/to/onda",
  "onda.server.args": []
}
```

## Using the extension

Open an `.onda` file and the extension will activate automatically.

Available commands:
- `Onda: Run Patch`
- `Onda: Stop Patch`
- `Onda: Restart Language Server`

`Onda: Run Patch` starts the preview transport and opens the patch UI.

## Development

If you want to work on the extension itself:

```bash
npm install
npm run compile
```

Then open `editors/vscode/` in VSCode and launch an Extension Development Host.
