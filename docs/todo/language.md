# Language TODO

## Language follow-ups

- Polymorph defs follow-ups
  - Improve overload diagnostics to show per-candidate ranking details.
  - Evaluate extending overloads from top-level `def` and struct methods to proc-local defs.
  - Document remaining overload edge cases for complex untyped array/buffer inference-heavy call sites.
  - Evaluate banning recursion and mutual recursion for ordinary top-level `def` and struct methods, then marking lowered user defs/methods `alwaysinline` once the user-def call graph is guaranteed acyclic.
  - If we take the no-recursion route, add an explicit semantic cycle check for ordinary `def`/method call graphs rather than relying on LLVM `alwaysinline` failures for diagnostics.

- `const` follow-ups
  - Implement const arrays + const defs MVP as one compile-time execution pass:
    - widen `const` declarations from scalar-only to `primitive | fixed array`
    - keep scope to top-level and namespace-level declarations only
    - continue to reject unsupported const types such as structs, tuples, buffers, and proc values
    - syntax examples:
      ```onda
      const Window: f32[4] = [0.0, 0.5, 1.0, 0.5]
      const Zeros = [0.0, 0.0, 0.0, 0.0]

      namespace Window<N = 4>:
        const Base: f32[N] = [0.0, 0.5, 1.0, 0.5]
      ```
  - Add a real compile-time const value model in semantics:
    - scalar const values
    - fixed array const values
    - preserve current lexical-order rules; forward references and cycles remain out of scope
  - Add a dedicated const-eval pass in semantics:
    - evaluate top-level and namespace const declarations
    - evaluate `const def` calls from const initializers and other compile-time expression sites
    - hard-error on non-const arguments, calls to non-const defs, recursion, and compile-time out-of-bounds indexing
  - Implement `const def` MVP:
    - surface syntax: `const def ...`
    - supported body subset: locals, local fixed arrays, indexed reads/writes, `if`, `for`, `loop N`, `return`, and pure builtin math
    - keep `while`, proc calls, buffers, events, runtime symbol access, proc-local const defs, and mutual recursion out of scope
    - allow only primitive-scalar or fixed-array returns
    - syntax examples:
      ```onda
      const def sqr(x: f32) -> f32:
        return x * x

      namespace Window<N = 512>:
        const def hann() -> f32[N]:
          w: f32[N]
          for i in 0..N:
            phase = TWO_PI * f32(i) / 3.0
            w[i] = 0.5 - 0.5 * cos(phase)
          return w

      const HannWindow = Window::hann()
      ```
  - Runtime semantics for const arrays:
    - allow read-only indexing, slicing, and `.len()` from runtime code
    - reject all writes, including mutation through `unsafe_write`
    - decide and enforce whether passing const arrays to ordinary mutable array params is rejected or requires explicit read-only analysis
    - runtime usage example:
      ```onda
      const Window: f32[4] = [0.0, 0.5, 1.0, 0.5]

      sample:
        out1 = Window[i32(phase) % Window.len()]
      ```
  - Lower const arrays as immutable program data rather than instance state:
    - materialize private immutable LLVM array globals for top-level and namespace const arrays
    - add a dedicated lowering path for const-array reads/slices/len
    - keep the runtime model read-only regardless of internal lowering details
  - Acceptance tests for the MVP:
    - top-level const arrays
    - namespace const arrays
    - runtime indexed reads from const arrays
    - const defs that fill and return fixed arrays for windows/filter kernels/general storage
    - diagnostics for unsupported const types and illegal non-const usage
  - Evaluate extending `const` beyond scalar primitives to arrays/structural compile-time values where justified.
  - Decide whether forward references and cycle diagnostics should be supported instead of the current strict lexical-order rule.
  - Fix float-literal precision/typing for typed constants and typed declarations:
    today decimal literals are parsed as `f32`, so `const X: f64 = 0.33333333333` widens an `f32` value instead of preserving `f64` precision.

- Graph composition follow-ups
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

- Events follow-ups
  - Add deeper conformance tests for complex proc-event forwarding chains and nested dispatch edge cases.
  - Add deeper conformance tests for proc-event slice forwarding edge cases (aliases, nested field arrays, and diagnostic coverage).
  - Add deeper conformance tests for host slice-event payload layouts, truncation diagnostics, and mixed fixed/slice event signatures.

- Oversampling follow-ups
  - Consider user-exposed quality/performance modes.
  - Consider selective/local oversampling syntax in addition to full-block `sample N:`.

- Standard library follow-ups
  - Keep the built-in module inventory in sync as docs evolve across `README.md`, `docs/INFO.md`, and `docs/SYNTAX.md`:
    `std/prelude`, `std/math`, `std/random`, `std/export_math`, `std/complex`, `std/osc`, `std/filter`, `std/env`,
    `std/delay`, `std/data`, `std/lookup`, `std/fft`, `std/convolution`.
  - Decide which stdlib modules are considered stable MVP surface versus still-evolving API.
  - Plan the next expansion/versioning pass beyond the current shipped module set.

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
