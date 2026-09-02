# Third-party notices

Onda's native distributions include LLVM and statically linked Rust crates.
Browser distributions additionally include Binaryen, the Onda math kernel, and
bundled JavaScript packages.

The complete applicable license texts accompany each distribution:

- `licenses/LLVM-LICENSE.txt` — LLVM, used by native compiler and runtime builds.
- `licenses/LLVM-BLAKE3-LICENSE.txt` — BLAKE3 code included in LLVM Support.
- `licenses/LLVM-XXHASH-LICENSE.txt` — xxHash code included in LLVM Support.
- `licenses/LLVM-MD5-LICENSE.txt` — MD5 code included in LLVM Support.
- `licenses/LLVM-REGEX-LICENSE.txt` — regex code included in LLVM Support.
- `licenses/LLVM-UNICODE-LICENSE.txt` — Unicode code and data included in LLVM Support.
- `licenses/LLVM-MSVCSETUPAPI-LICENSE.txt` — Microsoft setup API code included in LLVM's Windows
  support.
- `licenses/RUST-DEPENDENCIES.txt` — Rust crates used by native and WebAssembly builds.
- `licenses/MPL-2.0-SOURCES/` — exact source packages for MPL-2.0 Rust dependencies in native builds.
- `licenses/BINARYEN-LICENSE.txt` — Binaryen, used by browser compiler builds.
- `licenses/LIBM-LICENSE.txt` — `libm`, used by generated WebAssembly processors.
- `licenses/BUNDLED-JAVASCRIPT-LICENSES.txt` — JavaScript packages bundled into website assets.

Not every distribution contains every component. A distribution may therefore
omit license files for components it does not contain.
