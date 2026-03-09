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

- Graph composition follow-ups
  - `graph` MVP is implemented:
    - top-level and proc-local `graph` blocks
    - `@block`, `@sample`, `>>[N]`, and receiver sugar `<<`
    - proc-array slot references with static indices
    - strict shape checking for scalar/array edges
    - whole-array routing and element-wise array expressions
    - cycle rejection unless broken by positive sample delay
    - single-writer enforcement, fan-out, and implicit proc scheduling
    - runtime/event integration coverage for graph-instantiated proc nodes
    - CLI graph lowering inspection via `--dump-graph`
  - Remaining graph work:
    - Widen graph source expressions beyond the current MVP:
      support array-constructor sources and any other remaining non-call source forms where semantics stay unambiguous.
    - Evaluate opt-in graph-edge coercions/broadcasting:
      scalar-to-array broadcast, endpoint-family expansion for proc arrays, and broader numeric coercion rules.
      Example scalar-to-array broadcast:
      ```omni
      outs:
        out_st: f32[2]

      graph:
        0.25 >> out_st
      ```
      which would expand to:
      ```omni
      graph:
        0.25 >> out_st[0]
        0.25 >> out_st[1]
      ```
      Example endpoint-family expansion:
      ```omni
      init:
        voices: Voice[4] = Voice()

      graph:
        env.out1 >> voices.gain
      ```
      which would expand to:
      ```omni
      graph:
        env.out1 >> voices[0].gain
        env.out1 >> voices[1].gain
        env.out1 >> voices[2].gain
        env.out1 >> voices[3].gain
      ```
      Example scalar-to-array proc input broadcast:
      ```omni
      init:
        gain = StereoGain()

      graph:
        in1 >> gain.in_st
      ```
      which would expand to:
      ```omni
      graph:
        in1 >> gain.in_st[0]
        in1 >> gain.in_st[1]
      ```
      Example broader numeric coercion:
      ```omni
      params:
        mode: i32 = 0

      graph:
        gate >> mode
      ```
      where today an explicit cast would still be preferred:
      ```omni
      graph:
        i32(gate) >> mode
      ```
    - Improve graph diagnostics further where useful:
      especially more explicit hints on inferred-`@block` failures and richer cycle path reporting.

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
