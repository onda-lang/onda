# SLEEF vector math

`deps/sleef` is a Git submodule of [SLEEF](https://github.com/shibatch/sleef), pinned to
3.9.0, commit `906ca7512ee483296780a81a21b9ca715d40dfe1` (Boost Software License 1.0).
It is the unmodified upstream source for Onda's sole native vector math provider.

Onda ships precompiled LLVM bitcode, not a separate vector math library. Normal builds
embed the checked-in kernels without building SLEEF or fetching its source. JIT and AOT
code contain the helpers they use, including when cross-compiling. Initialization of this
submodule is needed only for regeneration:

```sh
git submodule update --init deps/sleef
```

See [the kernel README](../crates/onda_codegen_llvm/math-kernel/README.md) for the
numerical contract, supported targets, regeneration and validation instructions.
To update SLEEF, check out the desired upstream commit in the submodule, update the pin
in the generator, regenerate the kernels and license, and stage the new submodule gitlink.
Run the math-policy and standalone-object tests on Linux, macOS and Windows, and compare
both DSP speed and compilation time with the previous kernels before accepting the update.
