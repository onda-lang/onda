# MIR backend benchmarks

The reproducible comparison lives in
`packages/onda_binaryen_web/scripts/benchmark-backends.mjs`:

```bash
cd packages/onda_binaryen_web
npm run bench
```

It validates the first rendered block across backends, then reports median ±
MAD for:

- native LLVM O3 lowering/JIT compilation
- Binaryen O3 MIR-to-Wasm compilation
- WebAssembly instantiation
- prepared native `process_unchecked` throughput
- direct WebAssembly `onda_process` throughput

The native timing helper is
`crates/onda_examples/examples/benchmark_mir_native.rs`. It runs inside the
process and does not include daemon, JSON, audio-device, or file-I/O overhead.

## Methodology

The benchmark is a development diagnostic, with these safeguards:

1. The CLI emits compact MIR MessagePack once per scenario. The reported source-to-MIR duration is
   a single subprocess measurement that includes CLI startup and file I/O; it
   is displayed separately and is not treated as browser compiler latency. The
   MIR size column is the production MessagePack payload, not pretty JSON.
2. Binaryen compilation and WebAssembly instantiation each discard one
   in-process warmup, then retain
   `ONDA_BENCH_COMPILE_REPETITIONS` samples. The native helper parses and
   analyzes the source outside the JIT timer, discards one LLVM JIT warmup, and
   then measures the same number of full MIR-lowering, LLVM O3, and ORC JIT
   compilations.
3. Before throughput timing, a fresh WebAssembly instance renders one block.
   A separate native preflight JIT renders the same block through the checked
   runtime path and compares every output channel and frame. Non-finite values,
   shape differences, or errors beyond absolute `1e-6` plus relative `1e-6`
   abort the benchmark. The f32 reference block is exchanged as raw
   little-endian samples, so aggregate checksums cannot hide offsetting errors.
4. Native outputs are bound and validated once, then the helper calls
   `prepare_unchecked_process` and times only `process_unchecked`. WebAssembly
   calls the raw `onda_process` export. Both sides process complete blocks with
   strict arithmetic, the runtime audio-thread denormal policy, and no daemon,
   JSON, audio-device, or control-transport work in the timed loop.
5. Each side receives 200 warmup blocks. A preflight calibration chooses one
   shared native/Wasm block count per scenario, never below
   `ONDA_BENCH_ITERATIONS`, targeting at least
   `ONDA_BENCH_MIN_ROUND_MS` per timing round. The report shows median ± median
   absolute deviation (MAD).
6. The report includes the host CPU model and inherited affinity. By default the
   command exits unsuccessfully unless every scenario has a Wasm/LLVM runtime
   ratio of at least `1.05`, making a Binaryen win an explicit investigation
   rather than a surprising table cell.

The checked-in scenarios have no audio inputs or external buffers and expose
scalar f32 outputs. This keeps the measured ABI identical without claiming to
cover browser scheduling, AudioWorklet copying, or host-buffer traffic.

## Illustrative run

Measured 2026-07-17 on an Intel Core i9-14900HX (32 logical CPUs), Linux x86-64,
Node 26.4.0, rustc 1.96.1, LLVM 21.1.2, and Binaryen 130, pinned to logical CPU 8.
The run used strict arithmetic with the realtime denormal policy, 128-frame
blocks, a 2,000-block minimum, a 50 ms target per round, five timing rounds, and
five retained compile/instantiate samples. Every parity comparison had zero
observed sample error and the LLVM win gate passed.

| Scenario | Blocks/round | MIR MessagePack KiB | Wasm KiB | Binaryen ms | Instantiate ms | LLVM JIT ms | LLVM ns/frame | Wasm ns/frame | Wasm/LLVM |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Language/state/events | 1,024,000 | 8.7 | 0.4 | 34.43 ± 16.62 | 0.03 ± 0.00 | 5.38 ± 0.06 | 0.73 ± 0.01 | 1.79 ± 0.01 | 2.47× |
| Processor oversampling | 128,000 | 41.0 | 1.1 | 29.15 ± 2.56 | 0.03 ± 0.01 | 7.15 ± 0.22 | 5.61 ± 0.02 | 12.11 ± 0.05 | 2.16× |
| Saw/filter/saturator | 16,000 | 132.9 | 3.1 | 66.62 ± 3.54 | 0.03 ± 0.01 | 17.76 ± 1.16 | 31.68 ± 0.08 | 106.33 ± 0.97 | 3.36× |

These numbers are an illustrative snapshot, not a universal performance
baseline. The trivial language case is especially sensitive to CPU frequency;
compile time and the larger kernels also move with thermals and code-cache
state. MAD describes within-process samples, not drift between whole
invocations.

### Resolved denormal anomaly

An earlier version of this benchmark showed Binaryen beating LLVM only for the
decaying oversampling kernel, with the native result varying dramatically by
core type. That was not an MIR or LLVM optimization win for Wasm: the offline
native runtime had failed to install the FTZ/DAZ floating-point mode already
used by the CPAL callback, so feedback state eventually spent the timed loop in
slow subnormal arithmetic. `onda_realtime` now centralizes the policy and every
checked or prepared-unchecked native processing entry installs it once per
thread. Repeating the benchmark on both performance and efficiency cores made
LLVM consistently faster; with the standard unmodified LLVM O3 pipeline,
oversampling measured 5.61 versus 12.11 ns/frame on logical CPU 8 and 11.93
versus 23.59 ns/frame on logical CPU 16. The default ratio gate protects this
failure mode.

For consequential comparisons, repeat the complete command, keep the machine
idle, use a fixed performance governor or pinned cores where available, and
compare distributions rather than one table. Re-run in target browsers and on
target architectures before setting release budgets.

Environment variables adjust the run:

```text
ONDA_BENCH_BLOCK_SIZE
ONDA_BENCH_ITERATIONS
ONDA_BENCH_REPETITIONS
ONDA_BENCH_COMPILE_REPETITIONS
ONDA_BENCH_MIN_ROUND_MS
ONDA_BENCH_REQUIRE_LLVM_WIN
ONDA_BENCH_MIN_WASM_TO_LLVM_RATIO
```

The LLVM gate defaults to enabled with a minimum ratio of `1.05`. Set
`ONDA_BENCH_REQUIRE_LLVM_WIN=0` only when collecting diagnostic data for a known
regression; do not use it for a release comparison.

## Legacy-to-MIR migration oracle

The native codegen crate retains the former direct `TypedProgram`-to-LLVM path
only under `cfg(test)`. This makes it possible to compare the retired and
production MIR pipelines in the same process without keeping two selectable
production backends:

```bash
cargo test -p onda_codegen_llvm \
  legacy_vs_mir_o3_performance_oracle \
  -- --ignored --nocapture
```

The oracle builds both paths at LLVM O3, rejects surviving compiler-generated
helper calls and runtime-sized allocas in MIR output, requires MIR LLVM IR and
object files not to exceed the legacy artifacts, and measures paired checked
and prepared-unchecked processing medians. A separate non-ignored regression
renders representative user-call, stateful, and oversampled programs through
both paths and enforces the strict-math backend tolerance sample by sample.

On the same host, pinned to logical CPU 8 on 2026-07-17, one oracle run produced:

| Scenario | MIR IR change | MIR object change | Prepared runtime change |
| --- | ---: | ---: | ---: |
| Scalar expression | -11.5% | -22.6% | -18.2% |
| Deep user-call chain | -36.2% | -76.0% | -90.5% |
| Stateful oscillator | -9.0% | -5.9% | -42.5% |
| Oversampled processor | -52.1% | -49.7% | -26.6% |
| Larger reverb | -18.3% | -31.5% | -10.7% |
| Neural synth | -37.7% | -46.6% | -23.2% |

Negative runtime values mean the MIR-generated code was faster. The oracle now
fails if any production prepared-runtime case is slower than its paired legacy
measurement. In this run all six generated-code cases improved and every
artifact shrank. End-to-end
JIT latency was mixed because it includes semantic-to-MIR construction:
small/stateful programs can spend more time at the new compiler boundary even
when the resulting machine code is better, while larger call-heavy programs
usually recover that cost during LLVM optimization or execution. Treat these
figures as a migration diagnostic, not a permanent release threshold.
