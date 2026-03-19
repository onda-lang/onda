# Omni Neovim Support

This directory contains a small Neovim runtime plugin for Omni:

- `.omni` filetype detection
- regex-based syntax highlighting
- builtin LSP client startup via `omni lsp`
- `:OmniRunPatch`, which launches the standalone preview window with `omni preview <file>`

## Requirements

- Neovim 0.10+
- an `omni` binary on `PATH`, or an explicit configured path

## Install

With `lazy.nvim`:

```lua
{
  dir = "C:/Users/franc/Sources/omni-llvm/editors/nvim",
  name = "omni.nvim",
  config = function()
    require("omni").setup({
      server_path = "C:/Users/franc/Sources/omni-llvm/target/debug/omni.exe",
    })
  end,
}
```

Without a plugin manager, add `editors/nvim` to `runtimepath`.

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
- `:OmniRunPatch` saves the current buffer before launching preview

## Commands

- `:OmniRunPatch` opens the standalone preview window for the current `.omni` file
