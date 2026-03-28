# Omni Neovim Support

This directory contains the Neovim plugin for Omni.

It provides:

- `.omni` filetype detection
- regex syntax highlighting
- builtin LSP startup through `omni lsp`
- `:OmniRunPatch`, which launches the standalone preview window with `omni preview <file>`

## Requirements

- Neovim 0.10 or newer
- an `omni` executable available on `PATH`, or an explicit configured path

If you need to build the CLI locally:

```bash
cargo build -p omni_cli --release
```

That produces the binary at:
- Windows: `target/release/omni.exe`
- macOS/Linux: `target/release/omni`

## Install

Because this plugin lives inside the `omni-llvm` repo, the normal installation pattern is:
1. keep a local checkout of the repo
2. add `editors/nvim` to your Neovim plugin/runtime path
3. point the plugin at your `omni` binary if it is not already on `PATH`

### `lazy.nvim`

```lua
{
  dir = "C:/path/to/omni-llvm/editors/nvim",
  name = "omni.nvim",
  config = function()
    require("omni").setup({
      server_path = "C:/path/to/omni.exe",
    })
  end,
}
```

macOS/Linux example:

```lua
{
  dir = "/path/to/omni-llvm/editors/nvim",
  name = "omni.nvim",
  config = function()
    require("omni").setup({
      server_path = "/path/to/omni",
    })
  end,
}
```

### Manual install without a plugin manager

Copy or symlink `editors/nvim` into a standard `pack` location.

You can also copy the contents of `editors/nvim/` directly into your Neovim config/runtime path.
That works because this folder already has the normal Neovim runtime layout:
- `ftdetect/`
- `ftplugin/`
- `lua/`
- `plugin/`
- `syntax/`

Typical targets:
- Windows: `%LOCALAPPDATA%\\nvim\\`
- macOS/Linux: `~/.config/nvim/`

For example, copying the contents of `editors/nvim/` into `~/.config/nvim/` will install the plugin without a plugin manager.

macOS/Linux example:

```bash
mkdir -p ~/.local/share/nvim/site/pack/omni/start
ln -s /path/to/omni-llvm/editors/nvim ~/.local/share/nvim/site/pack/omni/start/omni.nvim
```

Windows PowerShell example:

```powershell
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\nvim-data\site\pack\omni\start" | Out-Null
New-Item -ItemType SymbolicLink `
  -Path "$env:LOCALAPPDATA\nvim-data\site\pack\omni\start\omni.nvim" `
  -Target "C:\path\to\omni-llvm\editors\nvim"
```

Then add your configuration in `init.lua`:

```lua
require("omni").setup({
  server_path = "omni",
})
```

## Configuration

```lua
require("omni").setup({
  server_path = "omni",
  server_args = {},
  preview_path = nil,
  preview_args = {},
  root_markers = { "Cargo.toml", ".git" },
})
```

Notes:

- `server_path` is used for `omni lsp`
- `preview_path` defaults to `server_path`
- `preview_args` are appended to `omni preview <file>`
- `root_markers` controls project root detection for the builtin LSP startup

If the plugin is on your runtimepath, it auto-calls `require("omni").setup()` with defaults.
Providing your own `setup(...)` call overrides those defaults.

## Commands

- `:OmniRunPatch` saves the current `.omni` buffer and opens the standalone preview window

## What happens automatically

Once installed, the plugin:

- detects `.omni` files
- starts `omni lsp` when you open an Omni buffer
- applies Omni syntax highlighting

## Troubleshooting

If the LSP does not start:
- check that `omni` runs in a terminal
- set `server_path` explicitly to the built binary

If `:OmniRunPatch` does not launch:
- check that `preview_path` or `server_path` points to a working `omni` binary
- make sure the current buffer is saved to disk
