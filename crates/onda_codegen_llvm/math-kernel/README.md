# Embedded native vector math

These generated LLVM 21 bitcode modules contain SLEEF 3.9.0 helpers for f32 and f64:

- `sin`, `cos`, `tan`, `tanh`, `atan`, `atan2`, `log`: upstream `u35` implementations.
- `exp`, `exp2`, `pow`: upstream `u10` implementations.
- x86-64 SSE2, AVX, AVX2 at 128/256 bits, AVX-512, and AArch64 AdvSIMD.

SLEEF documents nominal bounds of 3.5 ULP (`u35`) and 1 ULP (`u10`), with function-specific
input and underflow qualifications in its documentation. Results can differ from scalar
libm. Vector math does not enable finite-only assumptions, reassociation, contraction or
ignoring signed zero in application arithmetic.

Normal Cargo builds embed these files directly. There is no SLEEF build, download, separate
library, DLL, or vector-math flag. The compiler selects legal variants from the destination
CPU's resolved LLVM features and lets LLVM choose profitable vector widths. After application
optimization it resolves fixed-width `llvm.exp2` calls introduced by LLVM (including
power-of-two `pow` expressions), links only used ISA/precision modules, makes the helpers internal, and
removes unused functions. Both JIT code and native objects contain their implementation.
Scalar tails still use platform libm (`-lm` when linking Unix objects).

Cross-compilation works on Linux/macOS x64 and ARM64, and Windows x64 MSVC. These freestanding
kernels contain fixed-width vector operations, constants and LLVM intrinsics; they contain
no host libc imports, assembly or CPU attributes. The destination supplies the data layout
and ABI. Internal helper calls use LLVM's register calling convention on every platform, so
no external SIMD C ABI or Windows vectorcall library is involved. Unsupported targets retain
scalar lowering; Binaryen is unchanged.

Kernels are compiled with FP contraction off and without fast math. The compiler does not
rerun application optimization over their bodies, even when application fast math is enabled.
Only global dead-code elimination runs after linking. This keeps SLEEF's numerical algorithm
and its intentional FMA intrinsics intact.

## Regeneration

The source is the pinned `deps/sleef` Git submodule. Regenerate with Python 3, Clang 21.1.8,
and LLVM 21.1.2 (the same pinned tools used on `origin/perf-work`):

```sh
git submodule update --init deps/sleef
python3 scripts/build-native-math-kernel.py \
  --clang /path/to/clang \
  --llvm-bin .deps/llvm-src/21.1.2-static/bin
python3 scripts/build-native-math-kernel.py --check
```

The generator requires clean sources at the pinned commit. It compiles unmodified upstream
sources using minimal freestanding header declarations, rejects non-LLVM imports and inline
assembly, and removes generating CPU attributes and module flags. It records tool versions
and SHA-256 hashes in `manifest.json`, and generates the operation mapping used by the backend.
`--check` needs neither Clang nor initialized SLEEF sources and runs in CI.

Run `cargo test -p onda_codegen_llvm --test math_policy --test slice_optimization` on each
release platform. Tests cover numerical edge cases, scalar tails, CPU feature overrides,
loop and SLP vectorization, cross-target objects and native C hosts linked without SLEEF
or Onda libraries. The Boost license ships in `licenses/SLEEF-LICENSE.txt`.
