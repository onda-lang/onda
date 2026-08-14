# Language TODO

## Language follow-ups

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.
  - Runtime recursion and mutual recursion are now rejected explicitly as unbounded realtime work;
    keep cycle diagnostics precise as overloads and new callable forms are added.

- `const` future follow-ups
  - Evaluate const-def overloads. Start with unique names per lexical scope unless reusing ordinary overload machinery is straightforward.
  - Evaluate inferred array return types for const defs, such as `-> f32[]` and `-> []`, where each call site validates the returned compile-time array element type and inferred length.
  - Consider local/proc-local const arrays if they prove useful.
  - Consider const structs or structural compile-time values if stdlib/table generation starts needing them.
  - Improve forward-reference and cycle diagnostics if the strict lexical model becomes annoying.
  - Preserve the numeric-literal specialization invariant as this code evolves:
    the AST may use `f64`/`i64` as its widest supported internal literal representation, but an
    untyped literal is not yet a source-language `f64`/`i64`. Semantic context selects its concrete
    type once; context-free assignment defaults to `f32`/`i32`, typed `f64` constants must not round
    through `f32`, and concretely typed runtime expressions must not acquire implicit wider
    intermediates.

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
  - Keep the built-in module inventory in sync as docs evolve across `README.md`, `docs/info.md`, and `docs/syntax.md`:
    `std/prelude`, `std/math`, `std/random`, `std/complex`, `std/osc`, `std/filter`, `std/env`,
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
  - Extend the existing inclusive range braces to integer locals and state. Do not extend this
    feature to top-level or proc `ins`/`params`; their external control and signal domains have
    separate boundary semantics. Ranges are binding refinements rather than dedicated wrapping or
    saturating integer types:
    ```onda
    init:
      active_taps: i32 = 0 {0, MaxImpulseLen}
      write: i32 = 0 {0, MaxImpulseLen - 1, wrap}
    ```
    Bounds are compile-time integer expressions. `clamp` is the default mode and `wrap` selects
    modular normalization across the declared inclusive range. Bare `wrap` and `clamp` are the
    concise spellings; `mode = wrap` and `mode = clamp` are their equivalent explicit named forms.
  - Apply the binding's range mode automatically on initialization, direct assignment, compound
    assignment, and writes through references:
    ```onda
    active_taps = values.len() # clamped to 0..MaxImpulseLen
    write -= 1                # wrapped to 0..MaxImpulseLen - 1
    ```
    Normalize once when storing into the ranged binding, never again merely because the value is
    read or used as an array index.
  - Keep composition storage-based rather than propagating a range mode through expressions. To
    advance a wrapped position, mutate the ranged binding directly; to preserve the original,
    initialize another ranged binding and mutate that:
    ```onda
    write += 1
    sample = ring[write]

    next: i32 = write {0, MaxImpulseLen - 1, wrap}
    next += 1
    sample = ring[next]
    ```
    An expression such as `write + 1` is still an ordinary integer expression and does not itself
    wrap. This keeps the behavior attached to the storage whose invariant must be maintained.
  - Carry conservative range facts from binding reads through integer arithmetic, branches, loop
    induction, and calls. Arithmetic itself remains ordinary: if both `write` and `i` are known to
    be between `0` and `N - 1` inclusive, `write + i` is between `0` and `2 * N - 2` inclusive and
    does not inherit the binding's `wrap` mode.
  - Permit refined integer function parameters where useful, and infer the induction range of
    `for i in 0..bounded_end` without requiring an annotation on `i`.
  - Erase refinements to their underlying `i32` or `i64` physical representation. The range mode is
    enforced at stores and relevant call boundaries without changing state layout or the processor
    ABI.
  - Use the retained proof at fixed-array accesses to select trusted unchecked MIR bounds when the
    complete index range fits the array. This should let persistent DSP state carry invariants such
    as convolution tap counts and ring positions without rediscovering them through whole-program
    state analysis.
