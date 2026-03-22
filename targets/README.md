# Target Specs

This folder contains checked-in example target-spec TOML files for `omni compile --target-spec`.

These are low-level codegen presets only. They do not describe sysroots, SDKs, linkers, or full platform toolchains.

Examples:

```powershell
cargo run -p omni_cli -- compile examples/sine.omni --emit obj --target-spec .\targets\arm64.toml
cargo run -p omni_cli -- compile examples/sine.omni --emit obj --target-spec .\targets\aarch64-none-elf.toml
cargo run -p omni_cli -- compile examples/sine_wasm.omni --emit obj --target-spec .\targets\wasm32-unknown-unknown.toml
```
