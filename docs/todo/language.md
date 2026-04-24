# Language TODO

## Language follow-ups

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.
  - Evaluate banning recursion and mutual recursion for ordinary top-level `def` and struct methods, then marking lowered user defs/methods `alwaysinline` once the user-def call graph is guaranteed acyclic.
  - If we take the no-recursion route, add an explicit semantic cycle check for ordinary `def`/method call graphs rather than relying on LLVM `alwaysinline` failures for diagnostics.

- `const` arrays + `const def` MVP
  - Goal:
    - Make `const` a real compile-time value system instead of only frontend scalar substitution.
    - Support immutable scalar constants and immutable fixed-size primitive array constants.
    - Let runtime DSP code read tables, windows, kernels, scales, and waveshapers without allocating per-instance state.
    - Let `const def` generate those values at compile time from small deterministic Onda programs.
  - Non-goals for the MVP:
    - No const structs, tuples, buffers, proc values, events, or runtime-resource access.
    - No local/proc-local const arrays yet. Existing local scalar const behavior can remain, but array constants should start at top-level and namespace scope.
    - No forward references or cycles. Keep the current lexical-order model and improve diagnostics only where cheap.
    - No general-purpose compile-time side effects, I/O, random streams, host calls, or allocation visible to the language.
    - No mutable borrowing of const array data. Read-only array params/slices can be a later feature.

  - Surface syntax:
    ```onda
    const Window: f32[4] = [0.0, 0.5, 1.0, 0.5]
    const Ratios = [1.0, 1.5, 2.0, 3.0]
    const Flags: bool[3] = [true, false, true]

    namespace Windows<N = 512>:
      const def hann() -> f32[N]:
        w: f32[N]
        for i in 0..N:
          phase = TWO_PI * f32(i) / f32(N - 1)
          w[i] = 0.5 - 0.5 * cos(phase)
        return w

      const Hann: f32[N] = hann()
    ```
  - Const declaration rules:
    - `const NAME = scalar_expr` keeps the existing scalar behavior.
    - `const NAME: T = scalar_expr` keeps the existing typed scalar behavior.
    - `const NAME: T[N] = array_expr` declares an immutable fixed-size primitive array.
    - `const NAME = [ ... ]` infers a fixed-size primitive array from the first element, matching ordinary untyped array-literal inference.
    - Empty const array literals are rejected.
    - Typed const array initializers must produce exactly `N` elements.
    - Element values use the same scalar compatibility/coercion rules as fixed-size `ins` / `params` array defaults.
    - `N` must be a compile-time integer expression greater than zero. Namespace integer template params are valid in namespace-specialized declarations.
    - Unsupported const types are hard errors: structs, tuples, buffers, slices, proc values, and nested arrays.

  - Runtime read semantics:
    ```onda
    const Table: f32[4] = [0.0, 0.25, 0.5, 1.0]

    init:
      phase = 0

    sample:
      phase = phase + 1
      out1 = Table[phase % Table.len()]
    ```
    - `Table[i]` reads one immutable element.
    - `Table.len()` returns an `i32` fixed length and should be usable anywhere ordinary array `.len()` is usable.
    - `Table[:]`, `Table[start:]`, `Table[:end]`, and `Table[start:end]` produce read-only primitive slice views.
    - Runtime indexing should follow existing fixed-array behavior: dynamic indices are clamped; constant out-of-range indices should be diagnosed before codegen where possible.
    - Const arrays are valid read sources in slice copy, for example `dst[:] = Table[:]`.
    - Const arrays and const slices are invalid write targets.
    - `unsafe_write(Table, i, x)` and `Table.unsafe_write(i, x)` are rejected.
    - Passing a const array/slice to an ordinary mutable array parameter is rejected in the MVP. Add explicit read-only parameter syntax later if the need is real.

  - Compile-time expression semantics:
    - Add a semantic `ConstValue` model:
      - `Scalar(TypedConstValue)`
      - `Array { elem_ty: PrimitiveType, len: usize, values: Vec<TypedConstValue> }`
    - Keep source-level const visibility lexical and namespace-aware.
    - Const scalar values remain usable in all current compile-time sites: counts, array sizes, namespace args, graph delays, asserts, oversample factors, and range/default expressions.
    - Const array `.len()` should be compile-time evaluable.
    - Const array indexing with a compile-time integer index should be compile-time evaluable.
    - Compile-time array indexing out of bounds is a semantic error, not a clamped read.
    - Const array values should be allowed as fixed-array defaults where type and length match:
      ```onda
      const Spread: f32[2] = [0.2, 0.8]

      params:
        pan: f32[2] = Spread
      ```

  - `const def` MVP:
    - Surface syntax: `const def name(...) -> T:` or `const def name(...) -> T[N]:`.
    - Valid locations: top-level and namespace scope.
    - Valid parameter types for the first pass: primitive scalars only. Fixed-array params can come after read-only array ABI rules are settled.
    - Valid return types: primitive scalar or fixed-size primitive array.
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
    - Add an implementation iteration cap for compile-time loops so bad template values cannot hang analysis.
    - Const-def overloads can be deferred. Start with unique names per lexical scope unless reusing ordinary overload machinery is straightforward.

  - Frontend / semantics implementation direction:
    - Widen `ConstDecl.ty` from `Option<PrimitiveType>` to a const type spec, for example:
      - `Primitive(PrimitiveType)`
      - `Array { elem: PrimitiveType, size: Expr }`
    - Widen grammar from `const NAME (: type_name)? = expr` to `const NAME (: (type_name | array_type))? = expr`.
    - Add `const def` to the AST, either as `FunctionDef { is_const: true, ... }` or a separate `ConstFunctionDef` if that keeps validation cleaner.
    - Move array-const evaluation into semantics. The current frontend scalar-const substitution cannot model `Table[i]` because indexed expressions keep the base as a symbol.
    - Prefer eventually moving all const evaluation to semantics, but an incremental path is acceptable:
      - keep existing scalar const substitution for compatibility
      - retain const array declarations as named declarations for semantics/codegen
      - teach namespace rewriting to qualify const array symbols without expanding them into literals
    - Run const evaluation after import/namespace rewriting and auto-std injection, before asserts, graph lowering, proc desugaring, and type analysis.
    - Add typed const arrays to `TypedProgram`, for example:
      ```rust
      pub struct TypedConstArray {
          pub name: String,
          pub elem_ty: PrimitiveType,
          pub len: usize,
          pub values: Vec<TypedConstValue>,
      }
      ```
    - Add const arrays to expression environments so typing, validation, `.len()`, indexing, slicing, and unsafe data builtins can distinguish immutable data from state arrays.
    - Reuse existing immutable alias machinery where possible (`LocalArrayAliasInfo { writable: false, ... }`) for slices and read-only views.

  - LLVM lowering:
    - Lower const arrays as immutable module data, not instance state.
    - Emit one private constant LLVM global per specialized const array.
    - Do not include const array bytes in the runtime state layout.
    - Add const-array maps to lowering contexts:
      - symbol -> global pointer
      - symbol -> element type
      - symbol -> length
    - Reuse existing fixed-array index/slice/len lowering where possible, but load from the immutable global base pointer instead of the state blob.
    - Keep writes rejected in semantics, but add defensive codegen errors for const-array write/unsafe-write paths.
    - AOT object/IR emission should include the same private immutable globals; no extra host binding should be required.

  - Acceptance tests:
    - Parser:
      - typed primitive const arrays
      - untyped array-literal consts
      - namespace const arrays using namespace template integer params
      - rejection of unsupported const types
      - duplicate const diagnostics in one scope
    - Semantics:
      - type inference and typed element coercion
      - length mismatch diagnostics
      - empty array diagnostic
      - lexical-order visibility and no forward references
      - `Table.len()` in runtime and compile-time contexts
      - constant-index reads in compile-time contexts
      - out-of-bounds compile-time index diagnostic
      - rejected writes, slice writes, and `unsafe_write`
      - rejected passing to mutable array params
    - Const defs:
      - scalar-returning pure helper
      - fixed-array-returning table/window generator
      - namespace-specialized const def using `N`
      - calls to earlier const defs
      - recursion/mutual recursion rejection
      - runtime symbol access rejection
      - loop iteration cap diagnostic
    - Codegen/runtime:
      - JIT read from const array in `sample`
      - JIT read from namespace const array
      - slice-copy source from const array
      - state size does not grow for const arrays
      - emitted LLVM IR/object contains immutable globals for const arrays

  - Follow-ups after the MVP:
    - Read-only array/slice parameter syntax so const arrays can be passed to ordinary defs safely.
    - Local/proc-local const arrays if they prove useful.
    - Const structs or structural compile-time values if stdlib/table generation starts needing them.
    - Better forward-reference and cycle diagnostics if the strict lexical model becomes annoying.
    - Preserve the float-literal precision invariant during the const-eval migration:
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
