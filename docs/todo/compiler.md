# Compiler TODO

## Processor lowering and compile-time scalability

Processor arrays now use columnar field tensors and shared implementations specialized by
processor declaration and context shape. Runtime views carry storage roots and coordinates;
helper signatures scale with live fields rather than array capacity. MIR dead-value cleanup and
unused-parameter pruning already remove dead scalar parameter materializations and converge
across forwarding chains while retaining failing argument preparation.

The convolution project remains a compile-time stress case with large state and many specialized
functions. Further work should reduce unnecessary construction before the existing MIR cleanup,
using fresh measurements rather than treating the former per-slot expansion as a current cost.

- Add call-transitive, leaf-level state-use analysis during processor ABI planning.
  - Emit only the scalar/array leaves each generated helper transitively accesses.
  - Preserve source argument evaluation order and all potentially failing or stateful expressions.
  - Run this before MIR construction so lowering, range propagation, validation, and every backend
    avoid dead parameters and arguments.
  - Keep the MIR pruning pass afterward for ordinary user functions and defensive cleanup.
  - Replace its fixed-point full scans with a caller worklist if profiling finds pathological deep
    forwarding chains.

- Evaluate compact typed state-region references in MIR.
  - Compare a region reference with the current one-reference-per-live-field ABI for declarations
    with many distinct fields. Array capacity no longer expands that ABI.
  - Define field-level aliasing, mutability, range facts, serialization, and backend lowering before
    changing the MIR contract.
  - Measure this against leaf-pruned flat ABIs first; do not add aggregate machinery unless it
    provides a material compile-time or generated-code benefit.

- Intern flattened symbols and paths.
  - Replace repeated owned strings for generated state paths, parameters, and bindings with stable
    symbol/path IDs during semantic analysis and MIR construction.
  - Materialize readable names only for diagnostics, dumps, and serialized metadata.
  - Preserve deterministic output independent of hash-map iteration or thread scheduling.

- Parallelize independent function lowering.
  - Lower contextual function specializations concurrently into deterministic per-function results.
  - Avoid shared mutable type/source interners in workers; merge local tables deterministically or
    precompute the shared IDs.
  - Benchmark total latency and peak memory on convolution, processor arrays, and small programs so
    parallel setup does not regress ordinary compilation.

- Add cross-invocation compiler caching.
  - Cache parsed and typed standard-library modules, then evaluate caching contextual
    specializations or optimized MIR where dependency boundaries are stable.
  - Key entries by compiler/schema version, standard-library digest, target-independent analysis
    options such as sample rate and block size, source dependency content, and relevant flags.
  - Use content-addressed, integrity-checked entries with bounded storage and deterministic
    invalidation.
  - Keep cold compilation correct and fully supported; caching must only be an acceleration layer.

## Generic type reasoning and arguments

- Add compile-time introspection of generic type parameters.
  - Support type comparisons such as `T == f64` so generic code can reason about its concrete type.
  - Resolve type conditions during specialization, with clear rules for checking type-specific
    branches and pruning unselected code without runtime overhead.

- Accept all supported struct types as generic type arguments alongside primitives.
  - Include nominal structs, concrete generic struct specializations, and structs with nested
    aggregate fields across generic functions, processors, and structs.
  - Preserve concrete type identity, layout, and ordinary member and assignment checks through
    the shared specialization machinery.

## Constants in generic scopes

- Support const arrays and const defs in processor and other generic declaration scopes.
  - Let declarations inherit enclosing type parameters, specializing element types, signatures,
    and shapes through the existing generic machinery.
  - Keep name, type, and shape metadata available without evaluating contents. Evaluate only on
    surviving uses, using the existing lazy const resolver and diagnostics.
  - Cache values and failures once per concrete declaration and specialization in each compilation.
    Include type arguments, compile-time sizes, and relevant compile context in declaration identity;
    all instances of the same specialization share immutable data without per-instance generation
    or storage.
  - Keep compile-time declarations independent of runtime processor state, parameters, and resources.
    Array initializer caching must not imply automatic memoization of every const-def invocation.
  - Define access to a common typed const declaration so `Sine<T>` and `KSine<T>` can share one
    wavetable per used type. Separate declarations remain separate cache entries; avoid duplicating
    table-generation code or introducing oscillator-specific compiler behavior.
  - Verify inherited types, distinct type/size specializations, unused declarations, failure caching,
    and sharing across large processor arrays in native and Wasm compilation.

## DSP scheduling and processor-array performance

A 1,000-instance `Sine` bank exposes avoidable state traffic and a serial floating-point
reduction in the sample loop. Calls and table lookups are already inlined; enabling fast math lets
LLVM vectorize the bank. Use this workload to guide general processor and MIR optimizations.

The [final processor-storage report](../../scripts/bench/results/2026-11-02-processor-final.md#mir-alignment-and-larger-capacity-stress)
adds measured follow-ups from banks up to 16,384 slots. Its ordinary-array numerical references
carry less state and omit callback dispatch; use them to identify costs while accounting for that
difference in work.

- Prune unused compiler-generated delegate routing state per instance.
  - In the 16,384-slot nested callback bank, `__onda_delegate_index` and
    `__onda_delegate_route` are initialized but never read by processing/helpers, which use captured
    coordinates directly. Removing these two tensors would save 131,072 bytes of state/snapshot
    storage and their initialization writes.
  - Prove routing fields unused across reachable helpers, events, aliases, and replacement paths.
    Keep the routing generator for top-level subscriptions that still read these fields.
  - Derive state/snapshot metadata from the retained layout and verify initialization, cloning,
    snapshot/restore, and delegate identity/order. Measure storage, initialization and snapshot
    costs separately from DSP runtime.

- Reduce processor state traffic using field-level liveness and effect analysis.
  - Eliminate unobservable processor-output stores and reuse invariant parameters, increments, and
    offsets across samples when writes and aliases permit it.
  - Use the reported 16/128/1,024/16,384-slot banks to prove when cached outputs are used only by
    their producing call. Forward immediate uses while retaining stores observed through aliases,
    helpers, events or later samples; check removed stores in MIR/LLVM and measure state bytes and
    runtime against the current processor baseline.
  - Keep evolving state in locals where possible, committing it at observable boundaries.
  - Preserve event, exported-function, state-snapshot, and segmented-processing behavior, including
    state and effect ordering on failure.

- Expose independent work in processor callback loops through shared MIR proofs.
  - Native scheduling now peels a distinguished first iteration using generic MIR predicate
    summaries through helpers. This exposes vectorization in the indexed callback banks without
    reordering their effects. Extend only with proofs and measurements for predicates selecting
    other iterations, dynamic bounds, and more general callback dependencies.
  - Proven first-index comparisons in the remainder now fold using the counted-loop legality proof
    and conservative local-copy tracking. This also addresses the direct array recurrence that
    stayed scalar in the [initial peeling measurements](../../scripts/bench/results/2026-11-06-loop-peeling.json);
    [paired folding measurements](../../scripts/bench/results/2026-11-06-predicate-folding.json)
    cover that case and unchanged processor-bank controls.
    Further propagation through helper boundaries needs its own proof; profitability summaries
    cannot substitute for value identity or memory independence.
  - Preserve synchronous callback order, overlapping views, checked failures and the ordered
    floating-point reduction. Keep uncertain effects on the original execution path.
  - Share reusable effect/dependence facts in MIR; evaluate vectorization and target costs in the
    [backends](backends.md#optimization-follow-ups), with native/Wasm differential coverage.

- Evaluate block scheduling or bounded tiling of independent processor computations.
  - Reuse oscillator/filter state across several samples and expose independent work to backend
    vectorizers when dependency and effect analysis proves the transformation valid.
  - Respect sample feedback, shared mutable state, aliasing, observable effects, and sample-accurate
    event boundaries; retain sample scheduling wherever those constraints require it.
  - Measure register pressure, scratch storage, cache traffic, and code size before adopting a
    scheduling strategy. Allocate any required scratch storage during instance setup.

- Optimize processor-array block-hook lowering for statically proven slot sets.
  - Emit direct `block_pre` / `block_post` calls and omit runtime active flags when a loop is proved
    to visit known slots, including exhaustive constant-bound `for i in 0..N: voices[i]()` loops.
  - Preserve activation and hook ordering for conditional, repeated, or unknown slot visits.

- Evaluate a backend-neutral dot-product or reduction operation only if shared scalar-loop proofs
  remain insufficient in measured LLVM/Binaryen kernels. Define scalar-order preservation, a fixed
  reduction tree, or explicitly permitted reassociation before introducing such an operation.

- Expose independent processor-array work and reductions to backend vectorizers.
  - Native lowering already distributes independent expensive rendering from an ordered sum when
    results are materialized in an existing array; portable MIR and Binaryen keep the fused loop.
    Extend coverage only where further dependence and effect proofs justify it.
  - Preserve source floating-point evaluation order under default settings; automatically splitting
    an ordered sum into parallel accumulators or reassociating it requires opt-in fast math.
  - Optimize explicit independent accumulators without requiring global fast math when their
    authored operation order can be preserved.
  - Keep dependency and liveness analyses shared in MIR, with target-specific SIMD decisions in
    backends and no processor-name special cases.

## DSP loop proofs and diagnostics

The implemented baseline is documented in the [MIR pass contract](../mir.md#backend-neutral-passes).
Keep Onda DSP source scalar; shared analyses should expose facts to backends without FFT-specific
compiler rules or user-authored SIMD operations. Measurements and known regressions live in the
[benchmark results folder](../../scripts/bench/results/).

- Extend offset-aware storage origins, reference alignment, contiguous-window and stride-residue
  dependence proofs to runtime offsets/strides and dynamic ring-wrap patterns where provable.
  Keep unknown views conservative.
- Extend nonwrapping affine proofs with remaining relational induction and divisibility facts for
  FFT butterflies and unsupported loop shapes. Eliminate index normalization only when every
  intermediate in expressions such as `start + k + stage / 2` is proved safe.
- Extend relational bounds proofs to external-buffer lengths and remaining dynamic-view patterns.
  Preserve conservative handling of mutable descriptor references and unknown external lengths.
- Surface the harness's source-linked LLVM remarks in user tooling: successful/missed vectorization,
  dependence or alignment limitations, and selected vectorization factors. Use emitted assembly
  and isolated kernel measurements to check shuffle and spill costs.
- Track static coefficient storage and compile/JIT cost alongside processing speed when extending
  [constants in generic scopes](#constants-in-generic-scopes).

Backend vectorization work is tracked in [backends.md](backends.md#optimization-follow-ups), and
algorithm tuning in [stdlib.md](stdlib.md).

## Lazy module imports

- Refactor module loading to resolve imported declarations on demand.
  - Expose names, signatures, and types without evaluating declarations or loading their contents
    until requested.
  - Apply the same mechanism to every declaration kind: constants, functions, processors, structs,
    and namespaces, including transitive imports and concrete specializations.
  - Defer unused namespace arguments, body metadata, layouts, and scalar declarations through
    this shared declaration mechanism rather than adding separate const reachability passes.
  - Keep this separate from lazy const-array payloads: current declaration preprocessing and
    import parsing remain eager.

## Measurement

- Profile semantic analysis of long typed slice-view chains (`view0: f32[] = values[:]`, then
  `viewN: f32[] = viewN-1[:]`). Measure scaling and remove repeated whole-chain work without changing
  captured bounds, permissions, or lifetime checks; keep this generated program as a benchmark.
- Extend the harness's existing parse/load, semantic analysis, MIR construction/optimization, and
  aggregate JIT timings with processor rewriting, parameter pruning, range propagation, validation,
  backend IR construction/optimization, and linking breakdowns.
- Extend existing function/state-entry counts, serialized size, allocation and analysis-visit
  measurements with locals, parameters, statements, and state bytes in inspect-only runs.
- Keep regression workloads for a tiny program, a medium nested-processor program, the convolution
  project, and a large processor array. Optimize from these measurements rather than total CLI time
  alone.
- Extend the [benchmark harness](../../scripts/bench/README.md) with representative external assets
  and live event/parameter automation. Include distinct frequencies, modulation, feedback, and segmented events;
  compare default and fast-math compilation across block sizes and native/Wasm backends. Measure
  generated DSP separately from host/UI/audio transport, inspect state accesses and SIMD, and
  verify output and state behavior under the selected floating-point policy.
