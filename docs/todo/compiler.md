# Compiler TODO

## Processor lowering and compile-time scalability

The convolution project is the current stress case: it contains two convolver instances, four FFT
sizes, large flattened state, and more than two hundred specialized MIR functions. Some of that work
is inherent, but the compiler currently constructs uniform flattened helper ABIs and then removes
most of their parameters. Keep the existing MIR unused-parameter pass as a correctness backstop,
while moving avoidable work earlier in the pipeline.

- Add call-transitive, leaf-level state-use analysis during processor ABI planning.
  - Emit only the scalar/array leaves each generated helper transitively accesses.
  - Preserve source argument evaluation order and all potentially failing or stateful expressions.
  - Run this before MIR construction so lowering, range propagation, validation, and every backend
    avoid dead parameters and arguments.
  - Keep the MIR pruning pass afterward for ordinary user functions and defensive cleanup.
  - Replace its fixed-point full scans with a caller worklist if profiling finds pathological deep
    forwarding chains.

- Make processor specializations independent of instance paths where possible.
  - Compile structurally identical instances, such as left/right convolvers or repeated voices,
    against one shared implementation.
  - Pass an explicit instance-state view instead of baking physical state names into each helper.
  - Preserve specialization by processor type, compile context, and genuinely distinct constants.

- Make MIR unused-parameter pruning see through scalar parameter materialization.
  - Lowering currently emits a `local = load @parameter` assignment for every scalar and tuple
    component, so the load itself makes an otherwise unused parameter appear live.
  - Remove the canonical materialization together with the parameter when its destination local is
    otherwise dead, including after forwarding arguments disappear in a later pruning round.
  - Continue preserving source argument evaluation whenever preparing an argument can fail or has
    observable effects.

- Evaluate compact typed state-region references in MIR.
  - Represent a processor or aggregate state region with one reference plus validated field access,
    rather than hundreds of independent scalar/array reference parameters.
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

## Parameter smoothing

- Generate smoothing state and block-boundary ramps only for parameters whose values are read.
  - Reuse and generalize the parameter-array read analysis to follow helpers, slice aliases,
    events, and dynamic parameter indexing. Length-only access does not require smoothing.
  - Analyze authored code before inserting smoothing's own reads and writes, so generated ramps
    do not make otherwise unused parameters appear live.
  - Keep the check per declaration: reading any array element retains the whole array's ramps.
    Preserve host parameter descriptors even when no smoothing state or code is generated.
  - Verify unused and length-only declarations generate no ramp state or processing code, while
    indirect reads preserve smoothing, in native and Wasm compilation.

## DSP scheduling and processor-array performance

A 1,000-instance `Sine` bank exposes avoidable state traffic and a serial floating-point
reduction in the sample loop. Calls and table lookups are already inlined; enabling fast math lets
LLVM vectorize the bank. Use this workload to guide general processor and MIR optimizations.

- Reduce processor state traffic using field-level liveness and effect analysis.
  - Eliminate unobservable processor-output stores and reuse invariant parameters, increments, and
    offsets across samples when writes and aliases permit it.
  - Keep evolving state in locals where possible, committing it at observable boundaries.
  - Preserve event, exported-function, state-snapshot, and segmented-processing behavior, including
    state and effect ordering on failure.

- Evaluate block scheduling or bounded tiling of independent processor computations.
  - Reuse oscillator/filter state across several samples and expose independent work to backend
    vectorizers when dependency and effect analysis proves the transformation valid.
  - Respect sample feedback, shared mutable state, aliasing, observable effects, and sample-accurate
    event boundaries; retain sample scheduling wherever those constraints require it.
  - Measure register pressure, scratch storage, cache traffic, and code size before adopting a
    scheduling strategy. Allocate any required scratch storage during instance setup.

- Expose independent processor-array work and reductions to backend vectorizers.
  - Preserve source floating-point evaluation order under default settings; automatically splitting
    an ordered sum into parallel accumulators or reassociating it requires opt-in fast math.
  - Optimize explicit independent accumulators without requiring global fast math when their
    authored operation order can be preserved.
  - Keep dependency and liveness analyses shared in MIR, with target-specific SIMD decisions in
    backends and no processor-name special cases.

## DSP loop proofs and diagnostics

Deferred follow-ups from the stdlib DSP performance analysis. Keep
Onda DSP source scalar; shared analyses should expose facts to backends without FFT-specific
compiler rules or user-authored SIMD operations.

- Preserve proven alignment, stride, storage identity, and disjoint memory ranges through array
  views, slices, aliases, and function calls. Slice loads/stores currently lower with `align 1`.
  Emit stronger alignment and alias facts only when storage layout and access rules guarantee them.
- Extend relational loop and bounds proofs to derived butterfly indices and mirrored ranges.
  Prove the independence of real-spectrum bin pairs and eliminate index normalization when valid,
  using the same machinery for other DSP kernels. See the existing
  [range-analysis follow-ups](language.md#language-follow-ups).
- Expose optimization diagnostics tied to source locations: successful/missed vectorization,
  dependence or alignment limitations, selected vectorization factors, and relevant backend
  remarks. Use emitted assembly and isolated kernel measurements to check shuffle and spill costs.
- Track static coefficient storage and compile/JIT cost alongside processing speed. Generic
  compile-time tables should share immutable data per specialization and materialize only used
  declarations through the existing const machinery.

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
  `viewN: f32[] = viewN-1[:]`). After indexed source locations removed the parser bottleneck, a
  6,000-link debug compile still takes about 21 seconds while parse/constant inspection takes about
  1.8 seconds. Find and remove repeated whole-chain work without changing captured bounds,
  permissions, or lifetime checks; keep this generated program as a scaling benchmark.
- Add phase-level timing for parse/load, semantic analysis, processor rewriting, MIR construction,
  parameter pruning, range propagation, validation, MIR optimization, backend IR construction,
  backend optimization, and JIT linking.
- Track MIR shape alongside time: function count, state bytes, locals, parameters, statements, and
  serialized size.
- Keep regression workloads for a tiny program, a medium nested-processor program, the convolution
  project, and a large processor array. Optimize from these measurements rather than total CLI time
  alone.
- Add reproducible DSP benchmarks for processors, processor banks and reductions.
  Include distinct frequencies, modulation, feedback, and segmented events;
  compare default and fast-math compilation across block sizes and native/Wasm backends. Measure
  generated DSP separately from host/UI/audio transport, inspect state accesses and SIMD, and
  verify output and state behavior under the selected floating-point policy.
