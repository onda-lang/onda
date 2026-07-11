# Runtime TODO

## Optimization / runtime follow-ups

- Proc-array block-hook lowering
  - When a loop over proc-array indices is statically proven to visit a known set of slots, lower those calls like static slot access:
    emit direct `block_pre` / `block_post` calls for the proven slots and remove the runtime active-flag bookkeeping for that path.
  - Target cases include exhaustive constant-bound loops such as `for i in 0..N: voices[i]()` where the visited entries are known at compile time.

- SIMD strategy
  - Add explicit vector DSL design (or auto-vectorization-oriented lowering passes) beyond current LLVM loop optimizations.
  - Define stable semantics for vector math and scalar/vector interoperability.

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.

- C ABI diagnostics lifecycle
  - Tighten memory ownership model for diagnostic messages and document host-side lifecycle guarantees.

