# Embedded native vector math

These six generated bitcode modules contain SLEEF 3.9.0 sin/cos/exp/log kernels at
f32/f64 precision. Trigonometric and log functions use the upstream `u10` (named
`_u1` in the implementation) variants; exp uses its upstream default implementation.
The exact source commit, archive checksum, tools, and output hashes are recorded in
`manifest.json`. The Boost license is in `../../../licenses/SLEEF-LICENSE.txt`.

Normal Cargo builds embed these files directly. No SLEEF installation, C compiler,
network access, additional DLL, or extra object-linking flag is required. The compiler
links only the needed ISA/precision modules, internalizes the helpers, and discards
unused functions. Both JIT code and exported native objects contain their implementation.

The late LLVM pass uses SSE2 for x86-64, AVX2 only with affirmative AVX/AVX2/FMA
features, and AdvSIMD for little-endian AArch64. It respects explicit feature disables.
Smaller vectors are padded with 1.0; larger vectors are split into supported widths.
The actual compilation target supplies the final ABI and data layout. Helper functions
have internal linkage, so their vector signatures never form an external C ABI.
Other architectures retain their existing math lowering. The Binaryen backend is unchanged.

The application first runs its ordinary O2/O3 pipeline. With fast math enabled, the pass
then replaces vector sin/cos/exp/log intrinsics already selected by LLVM. It does not
change scalar tails or force vectorization of sequential loops. SLEEF bodies are compiled
with FP contraction off and without fast math; the pass never copies Onda's relaxed flags
onto helper instructions or reruns the application optimizer after linking them.

Regenerate with Python 3.12+, Clang 21.1.8, and LLVM 21.1.2:

```sh
python3 scripts/build-native-math-kernel.py \
  --clang /path/to/clang \
  --llvm-bin .deps/llvm-src/21.1.2-static/bin
```

`--source /path/to/sleef.tar.gz` uses an existing archive instead of downloading it.
The checksum must match the pinned commit in either case. The generator compiles upstream
sources without patches, using minimal freestanding declarations, and rejects remaining
non-LLVM imports. It removes the generating target's CPU attributes and module flags before
emitting LLVM 21 bitcode. ISA-specific intrinsics remain intentional and require the matching
target features. Cross-emission tests cover Linux x64 ELF, Windows x64 COFF, and macOS arm64
Mach-O; execution and timing on each OS require that platform's hardware/CI.
