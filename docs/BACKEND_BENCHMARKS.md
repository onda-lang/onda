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
- Binaryen O4 MIR-to-Wasm compilation
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
   ratio of at least `1.00`, making a Binaryen win an explicit investigation
   rather than a surprising table cell. Workloads may set a larger margin, but
   the default does not misclassify a narrow LLVM win as a failure.

The checked-in scenarios have no audio inputs or external buffers and expose
scalar f32 outputs. This keeps the measured ABI identical without claiming to
cover browser scheduling, AudioWorklet copying, or host-buffer traffic.

## Illustrative run

Measured 2026-07-17 on an Intel Core i9-14900HX (32 logical CPUs), Linux x86-64,
Node 26.4.0, rustc 1.96.1, LLVM 21.1.2, and Binaryen 130, pinned to logical CPU 8.
The run used strict arithmetic with the realtime denormal policy, 128-frame
blocks, a 2,000-block minimum, a 100 ms target per round, nine timing rounds, and
five retained compile/instantiate samples. Binaryen used the production O4
profile with StackIR disabled. Every parity comparison remained within the
strict backend tolerance and LLVM was faster in every scenario.

| Scenario | Blocks/round | MIR MessagePack KiB | Wasm KiB | Binaryen ms | Instantiate ms | LLVM JIT ms | LLVM ns/frame | Wasm ns/frame | Wasm/LLVM |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Language/state/events | 2,048,000 | 9.1 | 0.4 | 30.40 ± 17.10 | 0.02 ± 0.00 | 5.52 ± 0.34 | 0.74 ± 0.00 | 0.75 ± 0.00 | 1.01× |
| Processor oversampling | 256,000 | 41.0 | 1.1 | 37.28 ± 0.97 | 0.02 ± 0.00 | 7.66 ± 0.30 | 5.89 ± 0.08 | 12.31 ± 0.20 | 2.09× |
| Saw/filter/saturator | 32,000 | 130.5 | 8.1 | 173.92 ± 2.27 | 0.04 ± 0.01 | 16.96 ± 0.38 | 33.45 ± 0.27 | 64.21 ± 0.44 | 1.92× |
| Complete f32/f64 math | 32,000 | 23.4 | 18.5 | 269.42 ± 4.37 | 0.07 ± 0.01 | 5.62 ± 0.22 | 49.65 ± 0.83 | 74.13 ± 1.06 | 1.49× |

These numbers are an illustrative snapshot, not a universal performance
baseline. The trivial language case is especially sensitive to CPU frequency;
compile time and the larger kernels also move with thermals and code-cache
state. MAD describes within-process samples, not drift between whole
invocations. Shared scalar-state promotion makes the trivial dependent-add
loop equally register-resident in both backends, so its 1% LLVM lead is
expected to be much narrower than the DSP kernels. That is a cross-backend MIR
improvement, not evidence that LLVM lost an optimization.

The math row exercises both widths of every MIR math intrinsic, including strict FMA. It verifies
that LLVM retains a substantial target advantage while the generated Wasm remains self-contained.
The saturator now links its f32 `tanh` closure into the module instead of crossing the old
JavaScript `onda_math` boundary on every call. Relative to the previous checked-in pinned snapshot this
change increased compile time and artifact size, as expected, but reduced Wasm render time from
112.45 to 64.21 ns/frame (about 43%). The embedded-kernel design therefore buys realtime throughput
and host simplicity at an explicit browser compilation/transfer cost; Binaryen dead-code
elimination limits that cost to the used helper closure.

The immediately adjacent affinity-pinned O3 control measured Wasm throughput of
0.76, 12.70, 64.18, and 75.13 ns/frame in table order. O4 therefore improved
three workloads by roughly 1–3% and left saturator effectively unchanged, while
raising one-time Binaryen latency by roughly 13–30% for the nontrivial modules.
A separate O4-plus-StackIR run improved oversampling and saturator slightly but
regressed language and math, so StackIR is not part of the production profile.

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
ONDA_BENCH_BINARYEN_OPT_LEVEL
ONDA_BENCH_BINARYEN_STACK_IR
ONDA_BENCH_REQUIRE_LLVM_WIN
ONDA_BENCH_MIN_WASM_TO_LLVM_RATIO
```

The LLVM gate defaults to enabled with a minimum ratio of `1.00`. Set
`ONDA_BENCH_REQUIRE_LLVM_WIN=0` only when collecting diagnostic data for a known
regression; do not use it for a release comparison.

`ONDA_BENCH_BINARYEN_OPT_LEVEL` defaults to the production O4 policy and accepts
Binaryen levels 0 through 4. It exists for optimizer A/B measurements; changing
it does not change the browser backend's production default.

`ONDA_BENCH_BINARYEN_STACK_IR=1` enables Binaryen's StackIR generation and
StackIR optimizer for an A/B run. It is disabled by default unless measurement
justifies changing the production policy.

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

On the same host on 2026-07-17, one representative oracle run after the shared-pass work produced:

| Scenario | MIR IR change | MIR object change | Prepared runtime change |
| --- | ---: | ---: | ---: |
| Scalar expression | -44.6% | -33.9% | -18.3% |
| Deep user-call chain | -52.2% | -78.5% | -90.3% |
| Stateful oscillator | -15.6% | -12.7% | -42.3% |
| Oversampled processor | -51.9% | -50.7% | -26.7% |
| Larger reverb | -18.3% | -31.5% | -13.3% |
| Neural synth | -37.7% | -47.4% | -22.7% |

Negative runtime values mean the MIR-generated code was faster. The oracle now
fails if any production prepared-runtime case is slower than its paired legacy
measurement. In this run all six generated-code cases improved and every
artifact shrank. End-to-end
JIT latency was mixed because it includes semantic-to-MIR construction:
small/stateful programs can spend more time at the new compiler boundary even
when the resulting machine code is better, while larger call-heavy programs
usually recover that cost during LLVM optimization or execution. Treat these
figures as a migration diagnostic, not a permanent release threshold.

Portable inlining and unconstrained scalar-state promotion were both rejected by measurement.
Pre-inlining structured MIR regressed the larger reverb by roughly 30%, and promoting all 33
oversampling state scalars more than doubled the object size by creating excessive SSA/PHI
pressure. MIR therefore retains target inlining intent for the backend and caps portable state
promotion at a small alias-safe working set. Binaryen's global
`allowInliningFunctionsWithLoops` lever is exposed but defaults off: an A/B improved oversampling
while worsening the language and saturator workloads. These are target cost-model decisions, not
portable semantic transforms.
