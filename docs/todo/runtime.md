# Runtime TODO

## Optimization / runtime follow-ups

- Remove or rename the existing `prepare_unchecked_process` /
  `onda_prepare_unchecked_process` binding-validation APIs because they perform validation, not
  initialization. Prefer the existing `validate_bindings` APIs; if a distinct convenience entry
  remains, name it `validate_bindings_for_unchecked_process`.

- Proc-array block-hook lowering
  - When a loop over proc-array indices is statically proven to visit a known set of slots, lower those calls like static slot access:
    emit direct `block_pre` / `block_post` calls for the proven slots and remove the runtime active-flag bookkeeping for that path.
  - Target cases include exhaustive constant-bound loops such as `for i in 0..N: voices[i]()` where the visited entries are known at compile time.

- SIMD strategy
  - `std/convolution`'s time-domain convolver uses a mirrored history ring so its hot loop reads
    two contiguous forward ranges without a per-tap wrap recurrence. Keep that layout covered by
    wraparound tests; it is intentionally a standard-library fix and does not promise that every
    backend will vectorize the loop. Native LLVM currently recognizes the cleaned-up reduction with
    `--fast-math`, but retains vector clamps and masked gathers; strict arithmetic remains scalar
    because reassociation would change its floating-point semantics.
  - Eliminate fixed-array clamps only when MIR proves the index range at the individual access.
    This requires branch refinement, canonical loop-induction facts, and a proof-consuming rewrite
    from `Clamp` to trusted `Unchecked`; the current integer-range summary alone is insufficient.
    The range-refined integer binding design tracked in `language.md` should provide explicit facts
    for persistent state where whole-program inference would otherwise be required.
  - Design a backend-neutral dot-product or multiply-reduction operation if cleaned-up scalar loops
    still do not vectorize reliably in both LLVM and Binaryen. Define its floating-point contract
    first: scalar-order preservation, a fixed reduction tree, or explicitly reassociated arithmetic.
  - Add explicit vector DSL design only if portable reductions and auto-vectorization-oriented MIR
    lowering do not cover real DSP workloads. Define stable semantics for vector math and
    scalar/vector interoperability before exposing vector types in the language.

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.

- C ABI diagnostics lifecycle
  - Tighten memory ownership model for diagnostic messages and document host-side lifecycle guarantees.
