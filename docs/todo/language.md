# Language TODO

## Language follow-ups

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.
  - Evaluate banning recursion and mutual recursion for ordinary top-level `def` and struct methods, then marking lowered user defs/methods `alwaysinline` once the user-def call graph is guaranteed acyclic.
  - If we take the no-recursion route, add an explicit semantic cycle check for ordinary `def`/method call graphs rather than relying on LLVM `alwaysinline` failures for diagnostics.

- `const` future follow-ups
  - Evaluate const-def overloads. Start with unique names per lexical scope unless reusing ordinary overload machinery is straightforward.
  - Evaluate inferred array return types for const defs, such as `-> f32[]` and `-> []`, where each call site validates the returned compile-time array element type and inferred length.
  - Consider local/proc-local const arrays if they prove useful.
  - Consider const structs or structural compile-time values if stdlib/table generation starts needing them.
  - Improve forward-reference and cycle diagnostics if the strict lexical model becomes annoying.
  - Preserve the float-literal precision invariant as this code evolves:
    decimal literals parse into `f64` AST values, untyped assignment inference still defaults them to `f32`,
    and typed `f64` constants must not round through `f32`.

- Graph composition follow-ups
  - Keep the textual `graph` block as the source-of-truth model for any future visual graph editor.
    Visual tooling should generate and round-trip ordinary Onda `init` + `graph` code rather than
    introducing a separate patcher runtime.
  - Widen graph source expressions:
    support array-constructor sources and any other remaining non-call source forms where semantics stay unambiguous.
  - Evaluate opt-in graph-edge coercions/broadcasting:
    endpoint-family expansion for proc arrays and broader numeric coercion rules.
    Example endpoint-family expansion:
    ```onda
    init:
      voices: Voice[4] = Voice()

    graph:
      env.out1 >> voices.gain
    ```
    which would expand to:
    ```onda
    graph:
      env.out1 >> voices[0].gain
      env.out1 >> voices[1].gain
      env.out1 >> voices[2].gain
      env.out1 >> voices[3].gain
    ```
    Example broader numeric coercion:
    ```onda
    params:
      mode: i32 = 0

    graph:
      gate >> mode
    ```
    where today an explicit cast would still be preferred:
    ```onda
    graph:
      i32(gate) >> mode
    ```
  - Improve graph diagnostics further where useful:
    especially more explicit hints on inferred-`@block` failures and richer cycle path reporting.
  - Evaluate event routing syntax for graph-heavy programs:
    - forwarding top-level events to proc instances and proc arrays
    - fanout to destination sets
    - clear rejection of ambiguous sample-accurate versus immediate event behavior
    - compatibility with ordinary explicit `events` blocks
  - Add graph introspection metadata for tools:
    - resolved node list
    - endpoint names, types, array shapes, and rates
    - edge list after fanout/broadcast expansion
    - cycle-path diagnostics with stable node/edge identifiers
  - Consider a graph formatter/dumper mode aimed at editor round-tripping, separate from the existing
    human-oriented `--dump-graph` inspection output.

- Events follow-ups
  - Add deeper conformance tests for complex proc-event forwarding chains and nested dispatch edge cases.
  - Add deeper conformance tests for proc-event slice forwarding edge cases (aliases, nested field arrays, and diagnostic coverage).
  - Add deeper conformance tests for host slice-event payload layouts, truncation diagnostics, and mixed fixed/slice event signatures.

- Musical scheduling / pattern follow-ups
  - Evaluate a small sample-accurate scheduling layer on top of events:
    - host-triggered events can carry target sample offsets inside the next block
    - scheduled events execute at deterministic sample positions rather than only immediately on the audio thread
    - payload layout and C API/daemon transport remain explicit and RT-safe
  - Consider a standard-library pattern/clock helper instead of new syntax first:
    - phasor/clock utilities
    - trigger division and swing helpers
    - note/gate sequencing helpers
    - envelope trigger helpers
  - Decide whether musical timing belongs in core language syntax, stdlib procs, or host-side event scheduling.
  - Add examples that demonstrate polyphonic voice arrays driven by events and graph routing.

- Oversampling follow-ups
  - Consider user-exposed quality/performance modes.
  - Consider selective/local oversampling syntax in addition to full-block `sample N:`.

- Standard library follow-ups
  - Keep the built-in module inventory in sync as docs evolve across `README.md`, `docs/INFO.md`, and `docs/SYNTAX.md`:
    `std/prelude`, `std/math`, `std/random`, `std/export_math`, `std/complex`, `std/osc`, `std/filter`, `std/env`,
    `std/delay`, `std/data`, `std/lookup`, `std/fft`, `std/convolution`.
  - Decide which stdlib modules are considered stable MVP surface versus still-evolving API.
  - Plan the next expansion/versioning pass beyond the current shipped module set.
  - Prioritize graph-friendly proc modules:
    - oscillators with consistent `freq` / `phase` / `reset` surfaces
    - filters with stable coefficient/range behavior
    - envelopes and gates with event-driven and signal-driven variants
    - delay/reverb building blocks with explicit buffer requirements
    - waveshaping and lookup helpers that benefit from const arrays
  - Add metadata conventions for stdlib procs that tools can use:
    - short label
    - category
    - default display ranges
    - preferred knob/slider/control style
    - endpoint grouping for stereo/multichannel nodes
  - Add small, focused stdlib examples that double as visual-graph node smoke tests.

- Tuple follow-ups
  - Nested tuples (`((f32, f32), i32)`).
  - Expression-level indexing (`calcIdx(pos)[0]` without an intermediate variable).
  - Tuple equality/comparison.
  - Tuple in proc port types.

- Generics follow-ups
  - Add focused conformance tests for explicit vs inferred generic specialization across `struct`/`proc` and stdlib usage.

- Range declarations follow-ups
  - Evaluate whether range syntax should be extended to array `ins`/`params` declarations.
  - Decide whether generated `min/max` clamp lowering should gain explicit NaN/Inf sanitization semantics.
