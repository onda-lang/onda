# TODO

## Next Features

- Graph composition syntax
  - Add a functional/graph-oriented syntax to connect processor outputs to processor inputs.
  - Support composing multiple processors into reusable signal chains without manual sample-level wiring.
  - Define deterministic evaluation order and clear error messages for invalid/cyclic graph wiring.

- Generics follow-ups
  - Extend/clarify generic typed local declarations beyond `init` if we decide to support them in additional scopes.
  - Add focused conformance tests for explicit vs inferred generic specialization across `struct`/`proc` and stdlib usage.
  - Document generic ownership/error rules in a dedicated language-spec section (`T` must belong to the current generic owner).

- Range declarations follow-ups
  - Evaluate whether range syntax should be extended to array `ins`/`params` declarations.
  - Decide whether generated `min/max` clamp lowering should gain explicit NaN/Inf sanitization semantics.

- Oversampling / downsampling blocks (`sample` scope)
  - Locked MVP syntax:
    - `up N:` block inside `sample:` (both top-level `sample` and `proc sample`).
    - `N` must be one of `{2, 4, 8, 16}`.
    - `up` can appear inside conditionals/branches in `sample`.
    - No explicit `down` block in MVP; returning to base rate is automatic after `up`.
  - Locked MVP semantics:
    - Nested `up` blocks are rejected.
    - Invalid factors (`up 3`, non-literal factors, etc.) are semantic errors with explicit allowed set `{2,4,8,16}`.
    - Proc calls are allowed in `up` and run at substep rate (`N` calls per base sample).
    - Same proc instance can be called both inside and outside `up` in the same sample tick; execution is deterministic by source order.
    - Writing `out*` directly inside `up` is rejected; outputs must be assigned after returning to base rate.
    - Symbols written in `up` and consumed after `up` cross the boundary via lowpass + decimate (not last-sample/average shortcuts).
    - Proc params/state assignments inside `up` are allowed and execute per substep.
  - Locked MVP rate-conversion behavior:
    - `ins` read in `up` are interpolated to substeps (not ZOH hold).
    - `params` read in `up` are held constant across substeps within the base sample.
    - Up/down conversion filters are compiler/runtime-managed with fixed high-quality settings in MVP.
    - Chosen filter family for MVP: IIR polyphase.
  - Delivery plan:
    - Frontend: parser + AST for `up N:` statements in `sample` bodies.
    - Semantics: placement/factor/nesting validation, `out*`-write restrictions inside `up`, and boundary type/rate checks.
    - Codegen: substep loop lowering, interpolating input reads, held params, proc-call substep execution, and decimating boundary exports.
    - Tests:
      - parser/semantic conformance for valid/invalid `up` usage;
      - deterministic mixed inside/outside proc-call ordering;
      - audio quality regression tests for alias reduction on nonlinear patches;
      - performance benchmark target at `N=4`: provisional budget `<= 2.5x` baseline cost on representative patches.
  - Follow-up (post-MVP):
    - Consider explicit `down:` block and user-exposed quality/performance modes.

- Standard library modules
  - Add more DSP coverage to the built-in stdlib (beyond current `std/math`, `std/osc`, `std/filter`, `std/env`, `std/delay` MVP set).

## Backends

- AOT backend
  - Add an ahead-of-time compiler path (object/static library) alongside ORC JIT.
  - Reuse the same semantic/lowering pipeline to keep runtime behavior consistent.
  - Define exported symbols/ABI for host integration without JIT startup.

- C++ backend (`.hpp` export)
  - Add a backend that exports Omni programs to a single-file, self-contained C++ header class.
  - Generate deterministic `init`/`process` methods with no dynamic allocation in the audio callback.
  - Keep generated API compatible with current channel/state/data model for easy host embedding.

## Optimization / Runtime follow-ups

- SIMD strategy
  - Add explicit vector DSL design (or auto-vectorization-oriented lowering passes) beyond current LLVM loop optimizations.
  - Define stable semantics for vector math and scalar/vector interoperability.

- RT-safety verification suite
  - Add automated checks/assertions for callback-time allocation/lock regressions.
  - Add repeatable stress tests around bind/rebind/process paths.

- C ABI diagnostics lifecycle
  - Tighten memory ownership model for diagnostic messages and document host-side lifecycle guarantees.
