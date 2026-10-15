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
    backend will vectorize the loop. Current MIR already proves the time-domain convolver's
    fixed-array accesses in bounds. Native LLVM recognizes the reduction with `--fast-math`;
    strict arithmetic retains the ordered sum because reassociation would change its floating-point
    semantics. Recheck generated code when tuning the remaining reduction and memory costs.
  - Extend the existing MIR bounds proofs to relational and derived loop indices needed by FIR
    and similar kernels. Range-refined integer bindings, constant-loop induction intervals,
    branch-sensitive integer comparisons, widened loop invariants, derived interval arithmetic,
    and proof-consuming rewrites from `Clamp`/`Checked` to trusted `Unchecked` are implemented.
    Calls and indirect writes still discard path facts; the analysis does not represent relational
    general constraints between variables. Direct guards against the same dynamic slice's
    length now work; derived offsets and external-buffer length relationships remain open.
    Remaining work is tracked in the [range-analysis follow-ups](language.md#language-follow-ups).
  - Design a backend-neutral dot-product or multiply-reduction operation if cleaned-up scalar loops
    still do not vectorize reliably in both LLVM and Binaryen. Define its floating-point contract
    first: scalar-order preservation, a fixed reduction tree, or explicitly reassociated arithmetic.
  - Keep SIMD internal to compiler backends and ordinary Onda DSP source scalar. Improve shared
    proofs and automatic vectorization rather than adding a source-level vector DSL. Follow-ups
    are tracked in [compiler.md](compiler.md#dsp-loop-proofs-and-diagnostics) and
    [backends.md](backends.md#optimization-follow-ups).

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.
