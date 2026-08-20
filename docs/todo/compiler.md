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

## Measurement

- Add phase-level timing for parse/load, semantic analysis, processor rewriting, MIR construction,
  parameter pruning, range propagation, validation, MIR optimization, backend IR construction,
  backend optimization, and JIT linking.
- Track MIR shape alongside time: function count, state bytes, locals, parameters, statements, and
  serialized size.
- Keep regression workloads for a tiny program, a medium nested-processor program, the convolution
  project, and a large processor array. Optimize from these measurements rather than total CLI time
  alone.
