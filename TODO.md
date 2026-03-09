# TODO

## Next Features

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Clarify current exclusions in docs: proc-local defs are still not overloadable.
  - Clarify/document overload behavior for complex untyped array/buffer inference-heavy call sites.

- `const` follow-ups
  - Evaluate extending `const` beyond scalar primitives to arrays/structural compile-time values where justified.
  - Decide whether forward references and cycle diagnostics should be supported instead of the current strict lexical-order rule.

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
    - `graph` is for continuous value routing only; it does not define event propagation syntax in MVP.
    - Top-level `events` remain imperative and may call proc events on graph nodes instantiated in `init`.
    - Graph-instantiated proc nodes stay addressable by name outside `graph`, so event handlers can target them directly (for example `voice.note_on(...)`).
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
    - Illustrative event/control-plane pattern:
      ```omni
      proc Main
        outs 1

        init
          voice = Voice()
          env = Env()

        graph
          env.out1 >> voice.amp
          voice.out1 >> out1

        events
          note_on(note: i32, vel: i32)
            voice.note_on(note, vel)
            env.gate_on()

          note_off()
            voice.note_off()
            env.gate_off()
      ```
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
  - Add deeper conformance tests for complex proc-event forwarding chains and nested dispatch edge cases.
  - Add deeper conformance tests for proc-event slice forwarding edge cases (aliases, nested field arrays, and diagnostic coverage).
  - Add deeper conformance tests for host slice-event payload layouts, truncation diagnostics, and mixed fixed/slice event signatures.

- Oversampling follow-ups
  - Consider selective/local oversampling syntax in addition to full-block `sample N:`.
  - Consider user-exposed quality/performance modes.
  - Add explicit performance budget tracking for higher factors (`N=32`, `N=64`) on representative patches.

- Standard library follow-ups
  - Keep the built-in module inventory synced across `README.md`, `INFO.md`, and `SYNTAX.md`:
    `std/prelude`, `std/math`, `std/complex`, `std/osc`, `std/filter`, `std/env`,
    `std/delay`, `std/data`, `std/lookup`, `std/fft`, `std/convolution`.
  - Decide which stdlib modules are considered stable MVP surface versus still-evolving API.
  - Plan the next expansion/versioning pass beyond the current shipped module set.

- Generics follow-ups
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
