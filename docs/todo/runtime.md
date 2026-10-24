# Runtime TODO

## Optimization / runtime follow-ups

- Remove or rename the existing `prepare_unchecked_process` /
  `onda_prepare_unchecked_process` binding-validation APIs because they perform validation, not
  initialization. Prefer the existing `validate_bindings` APIs; if a distinct convenience entry
  remains, name it `validate_bindings_for_unchecked_process`.

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.

Processor-array hook lowering and loop proofs belong in [compiler.md](compiler.md), target SIMD
selection in [backends.md](backends.md#optimization-follow-ups), and DSP algorithm work in
[stdlib.md](stdlib.md).
