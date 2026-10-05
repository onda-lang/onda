# Onda benchmark and inspection harness

Use this harness to measure stdlib changes, compare compiler settings, replay saved MIR, and
inspect assumptions about operators and automatic SIMD. Workloads use ordinary scalar Onda.

The harness currently runs on Linux. Native workers use address-space, CPU and wall-time limits;
Node/V8 workers use systemd user-service memory cgroups with swap disabled, a JavaScript heap limit,
and CPU/wall-time limits. Reference compilation, C/FFTW execution and disassembly use the same
bounded launcher. Cases run sequentially. Every child command, stdout and stderr is retained.

## Build and select a workload

Build the worker using the repository's LLVM setup:

```sh
cargo build --release -j1 -p onda_examples --features llvm-orc --example benchmark_dsp
python3 scripts/bench/run.py --list
python3 scripts/bench/run.py --filter operator-arithmetic-f32 \
  --backend both --ir --assembly --output target/bench/arithmetic-o3
```

Select a CPU with `--cpu`; the default is the first CPU in the current allowed affinity. Default
limits are 2 GiB and 30 seconds per child, with a 256 MiB Node heap. Set `--memory-mib`,
`--timeout-seconds`, and `--node-heap-mib` for workloads that need different limits. Worker builds
are a separate step; `run.py` requires an existing binary, selected with `--runner` if needed.

Each run needs a new or empty output directory. Select one or more builtin names using `--filter`
(`|` separates substring alternatives), pass repeatable `--source`, or explicitly request `--all`.
Heavy convolution and FFT cases are included in `--all`; select narrow cases during development.

## Custom sources and operator experiments

```sh
python3 scripts/bench/run.py --source path/to/probe.onda \
  --backend both --sample-rate 44100 --block-size 64 \
  --ir --output target/bench/probe
```

Custom files compile at their original paths, so relative imports/includes resolve correctly.
An exact snapshot of loaded documents and the worker's embedded stdlib is saved. Rebuild the
worker after changing compiler or embedded stdlib code. Custom files need unique filename stems.

Scalar f32, f64, i32, i64 and bool ports are supported. Outputs retain their original precision;
i64 parity is exact, including values outside the exact range of JavaScript numbers. External
buffers, array ports, host events and parameter automation are outside this runner's current scope.
Workloads can implement parameter changes and independent checks inside their Onda source.

`--input-pattern noise|ramp|zero|impulse` and `--input-seed` select deterministic inputs; the same
block repeats throughout execution. Defaults are seeded noise, 48 kHz and 128 frames per block.
Use `--warmup-blocks`, `--validation-blocks` and `--latency-blocks` to set observation coverage.
Convolution defaults to a longer warmup; other cases use 512 blocks.

Builtin `operator-*` probes cover f32/f64 array arithmetic, division, reciprocal multiplication,
and ordered reductions. They process 512 array elements per block. Their `ns_per_frame` is the
whole block cost divided by the host block size, rather than the cost of one operator. Inspect
and measure both variants before concluding an operator or source rewrite is faster.

## Inspect and replay IR

```sh
python3 scripts/bench/run.py --filter operator-arithmetic-f32 \
  --inspect-only --backend both --assembly --output target/bench/inspect
python3 scripts/bench/run.py --source target/bench/probe/probe.mir.msgpack \
  --trust-mir --backend both --sample-rate 44100 --block-size 64 \
  --ir --output target/bench/replay
```

`--inspect-only` emits IR/artifacts without initializing or processing the workload. It enables
`--ir`, which saves readable MIR, MIR JSON, optimized host LLVM IR, Wasm WAT, and descriptive
LLVM instruction counts by function. `--assembly` also saves a host object, target/ABI metadata,
and disassembly using `llvm-objdump` or `objdump`.

Vector types, shuffles and alignment counts describe emitted IR. Check the saved assembly for
actual instructions, register use and spills. These counts do not establish a throughput benefit.
The disassembly uses the same host optimization/fast-math policy as the JIT, with the object
emitter's relocation and linking layout; it is not a dump of the JIT's loaded machine code.

Saved `.mir.json` and `.mir.msgpack` inputs must match the selected sample rate and block size.
Ordinary validation applies by default. Use `--trust-mir` for compiler-produced artifacts to retain
unchecked-index and integer-range proofs; editing those operations requires maintaining the
producer's proof contract. Both backends receive the same validated, optimized MIR.

## Compare experiments

```sh
python3 scripts/bench/run.py --filter operator-arithmetic-f32 --backend both --ir \
  --opt-level 0 --output target/bench/arithmetic-o0
python3 scripts/bench/run.py --filter operator-arithmetic-f32 --backend both --ir \
  --opt-level 3 --compare-to target/bench/arithmetic-o0 --output target/bench/arithmetic-o3-repeat
python3 scripts/bench/compare.py target/bench/arithmetic-o0 target/bench/arithmetic-o3-repeat
```

`--opt-level 0..3` controls LLVM, `--wasm-opt-level 0..4` controls Binaryen, and `--fast-math`
explicitly enables relaxed floating-point optimization. Fast math can change results, reduction
order and exceptional-value behavior. Keep the intended numerical contract explicit when comparing.
`--label` records the assumption being investigated.

To compare two differently named variants, select their pair explicitly. Both directories can
be the same run when it contains both variants:

```sh
python3 scripts/bench/compare.py target/bench/operators target/bench/operators \
  --case-pair operator-division-f32:operator-reciprocal-f32
```

Timing reports retain each round, median, median absolute deviation, min/max and block latency
percentiles. Compilation and initialization have separate timings. Native IR/object emission is
outside JIT timing; Wasm compilation reports whether WAT emission is included. Small kernels include
host call and timer overhead. Use the passthrough case as context and repeat noisy measurements.
Defaults use three 10 ms rounds; select `--repetitions` and `--round-ms` to change them.

Comparisons report candidate/baseline timing ratios, changed settings/compiler/artifacts,
changed compiled-source fingerprints, and startup/warmed output differences. They compare common case names/backends;
changing output types or input/observation settings prevents numerical comparison. Numerical
mismatches are reported without hiding performance results. The compare command exits successfully
when a report is produced, so inspect its `equivalent` fields before accepting a transformation.

Every processing run checks finite outputs at startup, after warmup and at the end of measurement.
`--backend both` additionally checks native/Wasm startup and warmed parity using `--atol` and
`--rtol` (defaults 1e-6 each); integer/bool ports require exact equality. `--bitwise` requires
identical floating-point bits, including signed zero. Backend parity is agreement, not an independent
correctness proof. Encode an independent oracle in a workload, emit its error,
and set `--max-output-abs` to make that contract executable. Bounded-error diagnostics are checked
independently on each backend instead of requiring identical scaled errors.
To investigate NaNs or infinities, expose finite classification/error outputs from the workload.
Native execution uses Onda's runtime floating-point policy, including FTZ/DAZ on x86. The Wasm
worker follows WebAssembly floating-point semantics; take this difference into account for
subnormal-sensitive probes.

Builtin `fft-accuracy-*` cases compare all complex/real spectrum bins and inverse samples with an
independent f64 DFT. These are diagnostic workloads; their timings do not measure FFT throughput.
For selected scalar filters and real FFT performance cases, add `--references` to build/run C and
FFTW references (requires GCC and FFTW development libraries). Filter references check startup and
warmed samples. The FFTW performance reference checks the real part of bin 7; use the accuracy
cases for full-spectrum correctness. References currently require strict math and default inputs.

## Saved artifacts and tests

`metadata.json` records settings, compiler binary hash, harness hashes, CPU information, platform,
Git revision and whether the working tree is dirty. Wasm runs also record backend source and
package-lock hashes, Node and V8 versions. Result rows include MIR/LLVM/Wasm artifact hashes.
Per-case configuration, compiled-source hashes, source snapshots, MIR and output snapshots accompany
`native.json`, `wasm.json` and optional `references.json`/`comparison.json`. Timing results and
`status.json` are written incrementally with atomic, synced replacements. An interruption preserves
the last complete summary; interrupted/failed runs keep their completed artifacts.

```sh
python3 -m unittest discover -s scripts/bench -p 'test_*.py' -v
ONDA_BENCH_INTEGRATION=1 python3 -m unittest discover -s scripts/bench -p 'test_*.py' -v
```

Integration checks use short bounded native/Wasm runs to cover imports, typed ports, large integers,
MIR replay, incompatible configurations and inspection without executing initialization.
