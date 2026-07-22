# Target Specs

This folder contains checked-in example target-spec TOML files for `onda compile --target-spec`.

These are low-level codegen presets only. They do not describe sysroots, SDKs, linkers, or full platform toolchains.
Every target emits a relocatable object plus the generic Onda processor descriptor documented in
[`docs/processor-abi.md`](../docs/processor-abi.md); the consuming application owns final linking.

Examples:

```powershell
cargo run -p onda_cli -- compile examples/foundations/sine.onda --emit obj --target-spec .\targets\arm64.toml
cargo run -p onda_cli -- compile examples/foundations/sine.onda --emit obj --target-spec .\targets\aarch64-none-elf.toml
cargo run -p onda_cli -- compile examples/web/onda_wasm_playground/default.onda --emit obj --target-spec .\targets\wasm32-unknown-unknown.toml
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
- Native applications include [`onda_processor_abi.h`](../include/onda_processor_abi.h), construct
  storage and pointer tables from the descriptor, and call the linked entrypoints directly. See the
  [raw object example](../examples/native/raw_processor_object/README.md). Onda still does not
  select or invoke the final platform linker.
