# Language TODO

## Language follow-ups

- Input control metadata (deferred)
  - Keep `ins` defaults and numeric min/max ranges as the current input contract. Hosts can expose
    controls using those values; authors needing different scaling or quantization can express it
    explicitly in the `sample` block. Existing input range clamping remains unchanged.
  - Revisit input attributes such as `scale` (`linear` or `log`), `curve`, `step`, and `unit` after
    deciding how they fit ordinary Onda sample-rate inputs. This is an open design question, not
    a committed language extension.
  - Decide whether these attributes would be presentation hints or a shared contract for
    normalized host-control values, and who would perform conversions. Keep one uniform input
    model; host-selected controls must not introduce an audio/control classification in Onda.
  - Distinguish host-generated control samples from connected input buffers. Any future
    control metadata must not implicitly scale or quantize externally supplied signal samples.
  - If adopted, reuse parameter-domain metadata, validation, and conversion helpers across native
    and WebAssembly interfaces and both run views, while retaining host-owned control selection,
    smoothing, and automation policy.
  - Evaluate the cost of sample-accurate nonlinear automation before choosing an implementation.
    Include flat and changing spans, sparse and dense queues, numerical accuracy, and identical
    results across processing-segment splits. Prepared mappings and exponential recurrences are
    possible optimizations to assess, not requirements for the current input implementation.

- `match` statements and patterns
  - Add a `match` statement for nominal enums, evaluating the subject once and executing one arm
    without fallthrough. Require exhaustive coverage or an explicit wildcard arm; diagnose missing,
    duplicate, and unreachable cases. Start with member patterns and alternatives. Proposed syntax:

    ```onda
    match mode:
      case FilterMode::LowPass:
        out1 = low
      case FilterMode::HighPass | FilterMode::BandPass:
        out1 = high
    ```

    The spelling is tentative; the existing [enum model](../syntax.md#enums) supplies the
    finite set of cases.
  - Reuse ordinary branch scopes, definite-assignment and return-flow analysis, and effect/rate
    checking in init, event, block, and sample code. Support the same semantics in `const def`
    evaluation. Lower through shared MIR control flow so LLVM and Wasm can select efficient
    comparisons or switch dispatch without introducing a runtime matching engine.
  - Evaluate guards and tuple/struct destructuring after exhaustive enum matching. Guarded arms
    alone must not establish exhaustive coverage; define binding scopes and arm ordering before
    admitting overlapping patterns. Consider integer and boolean subjects using the same model.
  - Consider a `match` expression once statement semantics are settled, with exhaustive arms and
    ordinary result type/shape unification, including aggregate values.

- Enum follow-ups
  - Add compile-time enum reflection for member count and ordered values, and evaluate constant
    iteration over members for table generation. Keep names and labels as metadata rather than
    implicitly adding runtime strings.
  - Evaluate contextual member shorthand in typed assignments, call arguments, and match arms.
    Require one unambiguous expected enum type and preserve qualified member lookup as the explicit
    form; avoid global member-name injection.
  - Allow CLI and host tools to select enum members by source name, using the exported metadata
    to resolve names to ordinals. Evaluate optional member descriptions and grouping alongside
    existing display labels, while keeping binary parameter and event values as integers.
  - Evaluate enums with associated data as a separate extension: define a bounded tag/payload
    layout, variant construction, exhaustive destructuring, aggregate copy semantics, and recursive
    event/delegate schemas before implementation. Such values need more than the current `i32`
    representation; host controls must account for variant fields as well as the selected tag.
  - Evaluate explicit discriminants and flag sets separately if concrete use cases justify them.
    Their domains and host metadata cannot assume the current dense `0..member_count - 1` model;
    ordinary selection enums should retain their simple contiguous representation.

- Structured-data follow-ups
  - Evaluate implicit struct-method receivers as a replacement for the current explicit `self`
    parameter. The proposed model makes fields and sibling methods available by bare name like
    proc-local state, and keeps `self.member` only as an explicit escape hatch when a lexical
    binding shadows an owner member. Calls remain receiver-based (`voice.tick(...)`); the receiver is compiler-owned
    and should be introduced only when lowering the method, not inserted into the source-level AST.
    Implement this through shared owner-aware name resolution and receiver capture for struct
    methods and proc-local defs. Avoid a separate textual qualification pass or synthetic shadow
    bindings that leak into typed events, MIR, or tooling.
  - Measure and document default per-instance event, delegate, and print capacity costs before tuning
    them; keep all rendering-thread storage bounded and provisioned before execution.
  - Design structured host params and proc constructor controls as one extension of the existing
    bounded aggregate model. Reuse the canonical data layout, defaults, validation, descriptor
    metadata, and graph endpoint types instead of introducing a separate control-value
    representation.
  - Evaluate borrowed aggregate returns only with explicit, statically checkable lifetime and
    escape rules. Ordinary aggregate returns should remain independent values.
  - Evaluate reference fields and exclusive references only as part of a coherent lifetime and
    aliasing model. Struct fields currently own fixed data, and ordinary aggregate aliases remain
    writable and non-exclusive.

- Public proc getters
  - Add opt-in `pub def` proc getters, callable as expressions such as
    `voice.get_phase()`. Reuse ordinary def arguments, returns, specialization, and proc-local
    owner capture; unmarked proc-local defs remain private. For example:
    ```onda
    proc Voice:
      init:
        phase = 0.0

      pub def get_phase() -> f32:
        return phase
    ```
  - Require read-only behavior transitively through helper calls: getters must not mutate owner
    state or caller storage, step processors, dispatch events/delegates, print, or advance tasks.
    Keep events as the external mutation interface. Define legal call contexts using the existing
    execution-rate and resource-access rules; reading a getter must not trigger processing hooks.
  - Support getters on selected proc-array elements (`voices[i].get_phase()`) using the existing
    selector normalization and single-evaluation rules. Allow getters to query child procs and
    child proc arrays, for example `return voices[index].get_phase()`, without exposing those
    internal instances. Direct nested calls such as `voice.osc.get_phase()` must respect the
    visibility of the child instance; `pub def` alone does not make it public.
  - Return aggregates as independent values under ordinary def return rules. Returning proc
    instances, proc arrays, or borrowed internal storage is outside this extension and requires
    a separate resource-reference/lifetime design. Defer property-style getter syntax.

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.

- `const` future follow-ups
  - Evaluate const-def overloads. Start with unique names per lexical scope unless reusing ordinary overload machinery is straightforward.
  - Evaluate inferred array return types for const defs, such as `-> f32[]` and `-> []`, where each call site validates the returned compile-time array element type and inferred length.
  - Support const arrays and const defs in generic scopes, including proc-local declarations that
    inherit enclosing type parameters. See [Constants in generic scopes](compiler.md#constants-in-generic-scopes)
    for lazy evaluation, specialization caching, and shared immutable data requirements.
  - Support explicit type parameters on `const def` so coefficient/window generation can return
    `T[N]` directly, sharing ordinary numeric typing and specialization rules.
  - Evaluate bounded `while`, `break`, and `continue` in const defs with precise diagnostics and
    evaluation limits. These currently work in runtime code but are rejected in const defs.
  - Add diagnostics for unintended precision loss in compile-time calculations. The FFT coefficient
    generator's context-free `value = 0.0` inferred f32 before assignment into an f64 array;
    preserve first-assignment typing while making such narrowing easier to discover.
  - Support scientific-notation numeric literals for small DSP coefficients and tolerances.
  - Consider const structs or structural compile-time values, including structured `const def`
    parameters and results, if stdlib/table generation starts needing them.
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
    - aggregate audio and control endpoints with explicit rate and shape compatibility
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
    Keep numeric coercion limited to legal graph destinations; top-level params remain host-owned.
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

- Task follow-ups
  - Evaluate task `reset()` inside defs and task bodies. Ordinary top-level defs would need explicit
    owner runtime context, and task-control effects would need to propagate transitively through
    callable and synchronous event/delegate paths. Define active self-reset semantics before
    exposing this: either reject every direct and indirect path by which a running task can reset
    itself, or make reset terminate the current activation and restart it deterministically on the
    next advance. Resetting another owner-local task may be admitted independently if it does not
    weaken the rule that tasks cannot invoke one another.
  - Reconsider broadened, explicitly advanced tasks only when a concrete use case is substantially
    clearer than an ordinary stateful struct. A possible `task.step() -> bool` would advance until
    the next `yield` or completion without aborting the owner or producing neutral outputs; repeated
    calls could deliberately cross multiple yield boundaries. Specify legal init/event/block/sample
    call sites, call-rate effects, interaction with `await` and `reset()`, runtime failures, and
    partial-state publication. In particular, sample-rate stepping cannot allow a task to smuggle
    its currently legal block-rate processor calls into sample code. Prefer fixed-budget struct
    state machines for incremental FFT and other recurring realtime DSP unless coroutine syntax
    demonstrates a compelling broader benefit.

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
  - Defer the measured DSP optimization and validation work recorded in
    [Standard library TODO](stdlib.md), using scalar Onda source and backend-selected SIMD.
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

- Array and slice follow-ups
  - Evaluate arrays of arrays, arrays of tuples, and aggregate tuple elements as one coherent
    nesting extension, with fixed layout and ordinary aggregate copy/alias semantics.
  - Preserve the statically provable length of constant-bound slices so an exact-length slice can
    satisfy a fixed-array parameter, such as `stereo_sum(gains[0:2])` for a parameter of type
    `f32[2]`. Keep rejecting slices whose required length cannot be proved at compile time.

- Generics follow-ups
  - Evaluate compile-time integer value parameters on defs and controlled static loop expansion.
    Replace manually enumerated FFT stage namespace specializations with concise generic code;
    specify specialization, checking of selected bodies, and code-size limits without exposing
    target vector widths or SIMD operations in Onda.

- Range-analysis follow-ups
  - Add source syntax for refined integer function parameters, reusing the existing storage-domain
    normalization and call-boundary range analysis.
  - Keep dynamic-view, external-buffer, and derived-index proof work in
    [compiler.md](compiler.md#dsp-loop-proofs-and-diagnostics).
