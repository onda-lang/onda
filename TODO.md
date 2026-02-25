# TODO

## Next Features

- Graph composition syntax
  - Locked MVP syntax:
    - `graph` is an alternative to `sample` (mutually exclusive), while `init` stays available.
    - Proc instances/nodes are created in `init`; `graph` only declares connections.
    - Edge forms:
      - `src >> dst`
      - `@block expr >> dst` (`@sample` optional; default rate is sample)
      - `src >>[N] dst` (static sample delay, `N` compile-time integer `>= 0`)
    - Edge sources can be endpoint outputs/inputs/params, literals, expressions, and graph-safe `def` calls.
    - Endpoint sugar should work the same as outside graph (`p(...).endpoint`, `.outN`, named endpoint access).
  - Illustrative syntax examples:
    ```omni
    proc Main
      ins 2
      outs 2
      params
        mix = 0.25

      init
        rev = Reverb(mix: mix)
        lfo = Sine(freq: 0.2)

      graph
        in1 >> rev.inL
        in2 >> rev.inR
        @block (mix * 0.5 + 0.2) >> rev.mix
        lfo.out1 >> rev.mix
        rev.outL >> out1
        rev.outR >> out2
        rev.outL >>[1] rev.inL
    ```
  - Locked MVP semantics:
    - Constructor args in `init` are initial/default values; incoming graph edges provide runtime drive.
    - `mix >> rev.mix` style param routing is supported for block-time host params and sample-time modulation sources.
    - `@block` edges evaluate once per block and are reused for all samples in the block.
    - Unannotated edges are `@sample` by default.
    - `@block` edges must be block-safe (no sample-rate dependencies).
    - Single-writer rule per destination endpoint in MVP (duplicate drivers are semantic errors).
    - Fan-out is allowed.
    - Graph cycles are rejected unless total cycle delay is positive via `>>[N]`.
    - `>>[0]` does not break cycles; delayed feedback requires `N > 0` on at least one cycle edge.
    - Delayed edge state is per-edge and persistent across blocks.
    - Graph expression/def usage must not mutate proc state (directly or indirectly).
  - Locked MVP graph-safe def constraints:
    - Must not write proc fields, endpoints, buffers, `Data`, or other non-local state.
    - Local temporary mutation is allowed.
    - Primitive arrays in graph expressions are treated as value inputs (local copy semantics for mutation).
    - Array-transform defs and shape-generic transforms (`.len()`) are allowed as long as they stay graph-safe.
  - Def usage in graph edges (examples to preserve in tests/docs):
    ```omni
    // allowed: graph-safe scalar transform
    def shape(x, k)
      return tanh(x * k)

    // allowed: graph-safe array transform with local mutation
    def scale_arr(v, k)
      for i in 0..v.len()
        v[i] = v[i] * k
      return v

    // rejected: writes non-local state
    def poke_state(s, x)
      s.acc = x
      return s.acc

    graph
      shape(rev.outL, mix) >> out1
      scale_arr(procA.outVec, 4.0) >> out_vec
      // poke_state(...) >> out1    // semantic error (non-graph-safe def)
    ```
  - Delivery plan:
    - Frontend: parser + AST for `graph` block, edge annotations (`@block/@sample`), and delayed edges `>>[N]`.
    - Semantics: node/endpoint resolution from `init`, rate checking, graph-safe def validation, cycle detection with delay accounting, single-writer enforcement.
    - Lowering/codegen: deterministic topological scheduling, per-edge delay state lowering, block-vs-sample edge scheduling, param-edge runtime drive.
    - Diagnostics: unknown node/endpoint, type/shape mismatch, rate mismatch for `@block`, duplicate drivers, invalid cycles with cycle path reporting.
    - Tests:
      - parser coverage for all edge forms and invalid variants;
      - semantic tests for graph/sample exclusivity, node declaration requirements, and delay-cycle legality;
      - runtime/codegen tests for multi-out routing, param modulation, delayed feedback behavior, and array/def edge transforms.

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

- Generics follow-ups
  - Extend/clarify generic typed local declarations beyond `init` if we decide to support them in additional scopes.
  - Add focused conformance tests for explicit vs inferred generic specialization across `struct`/`proc` and stdlib usage.
  - Document generic ownership/error rules in a dedicated language-spec section (`T` must belong to the current generic owner).

- Range declarations follow-ups
  - Evaluate whether range syntax should be extended to array `ins`/`params` declarations.
  - Decide whether generated `min/max` clamp lowering should gain explicit NaN/Inf sanitization semantics.

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
