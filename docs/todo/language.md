# Language TODO

## Language follow-ups

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.
  - Evaluate banning recursion and mutual recursion for ordinary top-level `def` and struct methods, then marking lowered user defs/methods `alwaysinline` once the user-def call graph is guaranteed acyclic.
  - If we take the no-recursion route, add an explicit semantic cycle check for ordinary `def`/method call graphs rather than relying on LLVM `alwaysinline` failures for diagnostics.

- `const` follow-ups after const-array MVP
  - Landed baseline:
    - Top-level and namespace-scope primitive const arrays.
    - Typed const array syntax, for example `const Table: f32[4] = [0.0, 0.25, 0.5, 1.0]`.
    - Untyped array-literal const inference from the first element.
    - Empty literal and typed length mismatch diagnostics.
    - Namespace qualification for const array symbols, including namespace templates.
    - Const arrays in `TypedProgram`.
    - Runtime reads from const arrays in init, block, sample, events, and defs.
    - Immutable alias treatment for ordinary indexed/slice assignment targets.
    - LLVM lowering as immutable private globals, not instance state.
    - CLI formatting for typed const declarations.
    - Semantics rejects `unsafe_write(Table, i, x)` and `Table.unsafe_write(i, x)`.
    - Const arrays/slices are rejected when passed to ordinary mutable array params.
    - Direct coverage for const-array slice copy sources, for example `dst[:] = Table[:]`.
    - Direct coverage that runtime `.len()` works for const arrays.
    - State-layout regression proving const array bytes do not increase per-instance state size.
    - AOT/object coverage proving const array bytes do not increase exported state size metadata.
    - Semantic const-value model for const arrays.
    - Semantic const-array folding after import/namespace rewriting and auto-std injection, before asserts, graph lowering, proc desugaring, and type analysis.
    - Const array `.len()` is compile-time evaluable in semantic compile-time contexts.
    - Const array indexing with compile-time integer indices is compile-time evaluable.
    - Compile-time const-array index out-of-bounds is reported in semantics.
    - Const array values are accepted as fixed-array defaults where the declaration can consume an array literal.
    - Fixed-array defaults that reference const arrays validate exact whole-array element type and length before literal inlining.
    - First `const def` slice:
      - Parser accepts `const def name(...) -> T:` at top-level and namespace scope.
      - Scalar-returning const defs with primitive scalar params can be called from later const-array initializers.
      - Const defs can call earlier visible const defs.
      - Namespace-qualified const defs work through existing namespace flattening.
      - Const defs are consumed during semantic const evaluation and are not emitted as runtime defs.
    - Fixed-array-returning const defs:
      - Parser accepts primitive fixed-array returns such as `const def name(...) -> f32[N]:`.
      - Const array initializers can consume array-returning const def calls.
      - Const def bodies support local mutable primitive arrays and indexed local-array writes.
      - Compile-time `for` / `loop` evaluation is supported with a fixed iteration cap.
    - Fixed-array const-def params:
      - Primitive fixed-array params such as `xs: f32[N]` are accepted.
      - Const arrays, local const-def arrays, array literals, and array-returning const-def calls can be passed to fixed-array params.
      - Element type and length are validated before evaluating the call body.
      - Namespace template constants are supported in fixed-array param sizes.
    - Const-def lexical and rejection coverage:
      - Const defs can only call earlier visible const defs, including from parameter defaults.
      - Forward references, recursion, and mutual recursion are rejected.
      - Runtime symbol access and ordinary non-const def calls are rejected.
      - Compile-time loops have direct iteration-cap diagnostics.
    - Read-only const array/slice params for ordinary runtime defs:
      - Ordinary array params are inferred read-only when the body never writes through the param, aliases of the param, `unsafe_write`, or a mutable callee.
      - Const arrays and const slices can be passed to inferred read-only array params.
      - Const arrays and const slices are still rejected for mutable array params.
      - Forwarding through other read-only defs is accepted; forwarding to mutating defs keeps the caller param mutable.
    - Semantic scalar const declarations where frontend substitution is not enough:
      - Top-level and namespace scalar `const` declarations can call scalar-returning `const def`s.
      - Later scalar const declarations can depend on earlier semantic scalar const declarations.
      - Semantic scalar consts fold into later runtime and compile-time uses.
      - Untyped semantic scalar const declarations preserve full `f64` / `i64` value precision until the eventual use site applies its normal type rules.

  - Remaining compile-time const-array work:
    - Next step: move all const-dependent language preprocessing to semantic preprocessing so every const use is backed by one semantic const evaluator.
      - Motivation: avoid split behavior where `const A = 10` works in syntax-shaping sites but `const A = some_const_def()` does not.
      - Frontend should parse/load/import source and preserve enough namespace/template/count-shorthand AST for semantics; it should not evaluate user const declarations for language semantics.
      - Semantic preprocessing should own scalar const evaluation, const-array evaluation, `const def` evaluation, count shorthand expansion, namespace template argument evaluation/instantiation, namespace const qualification/flattening where const values are involved, asserts, graph delays, array sizes, and defaults.
      - Count shorthand expansion (`ins N`, `outs N`, `params N`, `buffers N`, and proc variants) is the smaller first slice.
      - Namespace template instantiation is the larger architectural slice because current frontend module loading flattens namespaces and generated symbols before semantic analysis.
      - Keep import/include resolution in the frontend, but avoid extending the old frontend const evaluator for new semantic const features.
    - Fully retire legacy frontend scalar-const substitution after the semantic preprocessing path owns the remaining syntax-shaping const sites.
    - Preserve the float-literal precision invariant during the const-eval migration:
      decimal literals parse into `f64` AST values, untyped assignment inference still defaults them to `f32`,
      and typed `f64` constants must not round through `f32`.

  - `const def` follow-ups after MVP:
    - Valid locations: top-level and namespace scope.
    - Valid parameter types: primitive scalars and fixed-size primitive arrays.
    - Valid return types: primitive scalar or fixed-size primitive array.
    - Current remaining gap: const-def overloads.
    - Example target:
      ```onda
      namespace Windows<N = 512>:
        const def hann() -> f32[N]:
          w: f32[N]
          for i in 0..N:
            phase = TWO_PI * f32(i) / f32(N - 1)
            w[i] = 0.5 - 0.5 * cos(phase)
          return w

        const Hann: f32[N] = hann()
      ```
    - Supported body subset:
      - local scalar declarations and assignments
      - local fixed primitive arrays
      - indexed array reads/writes for local mutable arrays
      - reads from visible const arrays
      - `if` / `elif` / `else`
      - `for i in A..B`, `for i in A..=B`, and `loop N` with compile-time integer bounds
      - `return`
      - pure builtin math and calls to earlier visible `const def`s
    - Rejected in const defs:
      - `while`
      - proc construction/calls/events
      - buffers and host-bound data
      - runtime `ins` / `outs` / `params` / mutable state access
      - `unsafe_write`
      - ordinary non-const defs
      - recursion and mutual recursion
    - Const-def overloads can be deferred. Start with unique names per lexical scope unless reusing ordinary overload machinery is straightforward.
  - Later follow-ups:
    - Local/proc-local const arrays if they prove useful.
    - Const structs or structural compile-time values if stdlib/table generation starts needing them.
    - Even richer forward-reference and cycle diagnostics if the strict lexical model becomes annoying.

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
