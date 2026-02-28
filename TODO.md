# TODO

## Next Features

- Array procs (partial)
  - Status: core typed proc-array declarations/calls and broadcast ctor sugar are implemented.
  - Remaining:
    - Decide/implement policy for non-literal indexed proc-array dispatch (currently rejected).
    - Add indexed proc-array event-call syntax (currently not expanded; existing event calls still use named proc-instance targets).

- Graph composition syntax
  - Locked MVP syntax:
    - `graph` is an alternative to `sample` (mutually exclusive), while `init` stays available.
    - Proc instances/nodes are created in `init`; `graph` only declares connections.
    - Edge forms:
      - `src >> dst`
      - `@block expr >> dst`
      - `@sample src >> dst` (explicit sample-rate override)
      - `src >>[N] dst` (static sample delay, `N` compile-time integer `>= 0`)
    - Edge sources can be endpoint outputs/inputs/params, literals, and expressions.
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

      graph
        in1 >> rev.inL
        in2 >> rev.inR
        mix >> rev.mix
        rev.outL >> out1
        rev.outR >> out2
        rev.outL >>[1] rev.inL # single-sample delay
    ```
  - Rate-behavior examples:
    ```omni
    # 1) Sample-rate processing through proc nodes
    proc Main
      ins 1
      outs 1

      init
        sat = SoftClip()

      graph
        in1 >> sat.in
        sat.out1 >> out1
    ```
    ```omni
    # 2) Param destination infers @block (no explicit annotation needed)
    proc Main
      ins 1
      outs 1
      params
        mix = 0.25

      init
        lp = OnePole()

      graph
        in1 >> lp.in
        mix >> lp.cutoff         # inferred @block because dst is a param endpoint
        lp.out1 >> out1
    ```
    ```omni
    # 3) Explicit @sample override for sample-rate modulation into a param
    proc Main
      ins 1
      outs 1

      init
        lp = OnePole()
        lfo = Sine(freq: 0.2)

      graph
        in1 >> lp.in
        @sample lfo.out1 >> lp.cutoff
        lp.out1 >> out1
    ```
    ```omni
    # 4) Heavier block-rate control logic in `block`, then routed to graph
    proc Main
      ins 1
      outs 1
      params
        target = 0.4

      init
        lp = OnePole()
        cutoff_ctrl = 1000.0

      block
        cutoff_ctrl = cutoff_ctrl + (target * 9000.0 - cutoff_ctrl) * 0.03

      graph
        in1 >> lp.in
        cutoff_ctrl >> lp.cutoff # inferred @block
        lp.out1 >> out1
    ```
  - Locked MVP semantics:
    - Constructor args in `init` are initial/default values; incoming graph edges provide runtime drive.
    - Unannotated edges targeting proc `param` endpoints are inferred as `@block`.
    - Unannotated edges targeting non-param destinations are `@sample` by default.
    - `@sample` can be used to override inferred `@block` on param destinations.
    - `@block` edges (explicit or inferred) evaluate once per block and are reused for all samples in the block.
    - `@block` edges must be block-safe (no sample-rate dependencies).
    - If a param edge is inferred `@block` but source is sample-rate, emit an error that suggests explicit `@sample`.
    - Single-writer rule per destination endpoint in MVP (duplicate drivers are semantic errors).
    - Fan-out is allowed.
    - Graph cycles are rejected unless total cycle delay is positive via `>>[N]`.
    - `>>[0]` does not break cycles; delayed feedback requires `N > 0` on at least one cycle edge.
    - Delayed edge state is per-edge and persistent across blocks.
    - Function-call processing in graph edges is not part of graph MVP; use proc nodes for transforms.
    - Complex block-rate control logic should run in `block`, then feed graph param edges.
  - Delivery plan:
    - Frontend: parser + AST for `graph` block, edge annotations (`@block/@sample`), and delayed edges `>>[N]`.
    - Semantics: node/endpoint resolution from `init`, rate inference (`dst` param => `@block`), rate checking, cycle detection with delay accounting, single-writer enforcement.
    - Lowering/codegen: deterministic topological scheduling, per-edge delay state lowering, block-vs-sample edge scheduling, param-edge runtime drive.
    - Diagnostics: unknown node/endpoint, type/shape mismatch, inferred-`@block` source-rate errors (with `@sample` hint), duplicate drivers, invalid cycles with cycle path reporting.
    - Tests:
      - parser coverage for all edge forms and invalid variants;
      - semantic tests for graph/sample exclusivity, node declaration requirements, param-destination rate inference/override, and delay-cycle legality;
      - runtime/codegen tests for multi-out routing, param modulation, and delayed feedback behavior.

- Events follow-ups
  - Status: MVP behavior is implemented (top-level/proc events, typed payloads, forwarding, runtime dispatch checks).
  - Remaining:
    - Clarify/lock policy for triggering proc events from general Omni code paths outside event handlers (currently intentionally not expanded further).
    - Add deeper conformance tests for complex proc-event forwarding chains and nested dispatch edge cases.

- Oversampling follow-ups (core implementation is done)
  - Status: current oversampling behavior is implemented and stable.
  - Remaining follow-ups:
    - Consider selective/local oversampling syntax in addition to full-block `sample N:`.
    - Consider user-exposed quality/performance modes.
    - Add explicit performance budget tracking for higher factors (`N=32`, `N=64`) on representative patches.

- Standard library modules
  - Add more DSP coverage to the built-in stdlib (beyond current `std/math`, `std/osc`, `std/filter`, `std/env`, `std/delay`, `std/data` MVP set).

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
