# Target Specs

This folder contains checked-in example target-spec TOML files for `onda compile --target-spec`.

These are low-level codegen presets only. They do not describe sysroots, SDKs, linkers, or full platform toolchains.

Examples:

```powershell
cargo run -p onda_cli -- compile examples/sine.onda --emit obj --target-spec .\targets\arm64.toml
cargo run -p onda_cli -- compile examples/sine.onda --emit obj --target-spec .\targets\aarch64-none-elf.toml
cargo run -p onda_cli -- compile examples/sine_wasm.onda --emit obj --target-spec .\targets\wasm32-unknown-unknown.toml
```

Suggested presets:

- Windows x64 generic: `targets/windows-x64-generic.toml`
- Windows x64 AVX2-class baseline: `targets/windows-x64-v3.toml`
- Linux x64 generic: `targets/linux-x64-generic.toml`
- Linux x64 AVX2-class baseline: `targets/linux-x64-v3.toml`
- macOS Apple Silicon generic: `targets/macos-arm64-generic.toml`
- macOS Apple Silicon M1-tuned: `targets/macos-arm64-apple-m1.toml`
- AArch64 ELF bare metal: `targets/aarch64-none-elf.toml`
- WebAssembly object emission: `targets/wasm32-unknown-unknown.toml`

Notes:

- `x86-64-v3` is a useful standardized x64 baseline when you want "at least AVX2" without hand-writing feature strings.
- These presets are examples, not canonical toolchain definitions. Adjust `cpu`, `features`, `abi_name`, and relocation model to match your real deployment target.
