# Language TODO

## Language follow-ups

- Structured-data implementation cleanups
  - Revisit implicit struct-method receivers. The intended source model omits `self` from method
    declarations, makes fields and sibling methods available by bare name like proc-local state,
    and keeps `self.member` only as an explicit escape hatch when a lexical binding shadows an
    owner member. Calls remain receiver-based (`voice.tick(...)`); the receiver is compiler-owned
    and should be introduced only when lowering the method, not inserted into the source-level AST.
    Implement this through shared owner-aware name resolution and receiver capture for struct
    methods and proc-local defs. Avoid a separate textual qualification pass or synthetic shadow
    bindings that leak into typed events, MIR, or tooling.
  - Consolidate hosted event payload validation so one diagnostic host pass precedes the mandatory
    raw-entry preflight without changing rejection or instance-lifecycle behavior.
  - Measure and document default per-instance event, delegate, and print capacity costs before tuning
    them; keep all rendering-thread storage bounded and provisioned before execution.

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
  - Expand `graph` into a coherent declarative composition surface for more than sample-rate signal
    edges. Design one concise routing vocabulary that can express:
    - sample-rate inputs, outputs, proc endpoints, expressions, and delayed feedback
    - block-rate params, proc params, proc `kouts`, and top-level `kouts`
    - forwarding events into procs and proc arrays
    - forwarding or subscribing to child delegates, including whole proc arrays and their indices
    - pure `def` transforms at compatible sample or block rates
    - fanout, explicit fan-in/reduction, endpoint families, and destination bundles
    - structured event/delegate payloads without flattening their nominal schemas
    Keep authored rate crossings visible, preserve deterministic event/delegate order, and reject
    ambiguous or stateful routes. Lower the expanded syntax onto the existing proc scheduling,
    event, delegate/`when`, expression, and block/sample machinery so `graph` remains composition
    syntax rather than a second execution model. Ordinary `sample`, `block`, `events`, and `when`
    code should remain the imperative escape hatch for routing that needs state or control flow.
  - Explore compact syntax for the expanded surface without committing each endpoint family to a
    separate mini-language. Representative relationships that should become easy to spell include:
    ```onda
    graph:
      @sample input * gain >> filter.in1
      @block analyzer.kout1 >> meter.level
      note_on >> voices.note_on
      voices.finished >> voice_finished
      @sample shape(filter.out1) >> out1
    ```
    The final syntax must distinguish event handlers, delegates, values, and pure functions from
    their resolved declarations; the sketch only captures the desired readability.
  - Widen graph source expressions:
    support array-constructor sources and any other remaining non-call source forms where semantics stay unambiguous.
  - Support calls to proven-pure runtime defs in graph source expressions. Reuse ordinary overload,
    specialization, type, shape, and effect analysis; reject state, resource, event, delegate, and
    other side-effecting call graphs rather than creating a second graph-only function model.
  - Evaluate compile-time conditional graph topology driven by `config const`, ordinary const, and
    namespace arguments. Only the selected edge topology should reach scheduling and cycle analysis;
    this is static specialization, not runtime graph rewiring. Prune unreachable proc state where
    doing so preserves initialization and metadata semantics.
  - Evaluate explicit graph fan-in/reduction rather than silently weakening the one-writer rule:
    - sum compatible scalar audio sources with deterministic typing
    - reduce proc-array output families without hand-writing each slot
    - keep non-summable values and mismatched shapes as errors
    - preserve one canonical writer after graph expansion so scheduling and diagnostics stay simple
  - Add block-rate graph routing for `kouts` and block-rate proc outputs. Make every sample/block rate
    crossing explicit and deterministic instead of treating control values as audio streams.
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
    - deterministic multiplexing when several event sources target one handler
    - structured payload compatibility using the ordinary event/delegate message model
    - clear rejection of ambiguous sample-accurate versus immediate event behavior
    - compatibility with ordinary explicit `events` blocks
    - lower routing sugar onto the existing event/delegate machinery rather than introducing a
      second callback model; explicit handlers and `when` remain the stateful routing surface
  - Add static processor latency metadata and automatic graph delay compensation:
    - declare latency in host-sample units and expose the resolved value to hosts and graph tools
    - accumulate latency through proc composition, proc arrays, explicit delayed edges, and rate changes
    - align converging audio paths without changing intentional feedback delays
    - diagnose dynamic or otherwise non-provable latency declarations
    - show inserted compensation edges in graph inspection output
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

- Print value follow-ups
  - Evaluate structured print values only after defining a bounded, host-independent representation
    for arrays, slices, tuples, and structs. Preserve scalar-leaf types and avoid reflective dumping
    of processors, buffers, or other runtime-owned objects.
  - Revisit dynamic print text only as part of a general runtime-string design with explicit
    ownership and realtime constraints. Static labels should remain allocation-free metadata.
  - Consider optional processor-instance or proc-array-slot context for log occurrences if explicit
    authored indices prove insufficient. Any design must keep lexical source ownership stable and
    avoid adding hidden per-instance strings or callbacks to generated execution.

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
  - Evaluate per-instance graph rate specialization so one proc implementation can be instantiated at
    different fixed rates without wrapper duplication, while retaining `sample N:` as the proc-local
    spelling when the rate is intrinsic to the implementation.
  - Add fixed-factor undersampled nodes for control and analysis work that should run every `N` host
    samples. Define held-output behavior, event timing, block hooks, task scheduling, and startup state
    explicitly rather than encoding undersampling as an implicit counter convention.
  - Expose deliberate rate-crossing policies such as hold, linear, and sinc where their input type and
    direction make sense. Account for filter latency and avoid inserting duplicate converters across
    already-compatible rate domains.

- Standard library follow-ups
  - Keep the built-in module inventory in sync as docs evolve across `README.md`,
    `docs/architecture.md`, and `docs/syntax.md`:
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

- Assignment follow-ups
  - Support indexed compound-assignment targets such as `values[i] += amount` while evaluating each
    selector exactly once.

- Array and slice follow-ups
  - Preserve the statically provable length of constant-bound slices so an exact-length slice can
    satisfy a fixed-array parameter, such as `stereo_sum(gains[0:2])` for a parameter of type
    `f32[2]`. Keep rejecting slices whose required length cannot be proved at compile time.

- Generics follow-ups
  - Add focused conformance tests for explicit vs inferred generic specialization across `struct`/`proc` and stdlib usage.

- Range-analysis follow-ups
  - Add source syntax for refined integer function parameters. Ranged `i32`/`i64` locals and state,
    normalization on stores, erased physical representation, conservative MIR range propagation,
    loop induction facts, call-boundary propagation, and fixed-array bounds-check elimination are
    implemented.
  - Extend bounds proofs from fixed storage to relational dynamic slice and external-buffer facts,
    such as an index derived from the same descriptor's `.len()`. Explicit `read_unsafe` and
    `write_unsafe` are available when the programmer can establish such a proof today.
