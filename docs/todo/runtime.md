# Runtime TODO

## Optimization / runtime follow-ups

- RT-safety verification suite
  - Extend allocation/deallocation regression checks to lock detection.
  - Add repeatable stress tests around bind/rebind/process paths.

Processor-array hook lowering and loop proofs belong in [compiler.md](compiler.md), target SIMD
selection in [backends.md](backends.md#optimization-follow-ups), and DSP algorithm work in
[stdlib.md](stdlib.md).
