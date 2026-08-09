# MIR backend benchmarks

The reproducible comparison lives in
`packages/onda_binaryen_web/scripts/benchmark-backends.mjs`:

```bash
cd packages/onda_binaryen_web
npm run bench
```

It validates the first rendered block across backends, then reports median ±
MAD for:

- native LLVM O3 lowering/JIT compilation from the same optimized MIR
- Binaryen O4 MIR-to-Wasm compilation
- WebAssembly instantiation
- direct native `onda_process` throughput
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
   lowers the typed program to optimized MIR once, discards one LLVM warmup,
   then measures the same number of LLVM O3 and ORC JIT compilations from that
   MIR. This matches Binaryen's side of the backend boundary.
3. Before throughput timing, a fresh WebAssembly instance renders one block.
   A separate native preflight JIT renders the same block through the checked
   raw processor entry and compares every output channel and frame. Non-finite values,
   shape differences, or errors beyond absolute `1e-6` plus relative `1e-6`
   abort the benchmark. The f32 reference block is exchanged as raw
   little-endian samples, so aggregate checksums cannot hide offsetting errors.
4. Native storage and pointer tables are validated once, then the helper and
   WebAssembly both call their raw `onda_process` backend entries and reject a
   nonzero execution status after every block. Both sides process complete
   blocks with strict arithmetic, the runtime audio-thread denormal policy, and
   no daemon, JSON, audio-device, worklet-adapter, or control-transport work in
   the timed loop.
5. Each side receives 200 warmup blocks. A preflight calibration chooses one
   shared native/Wasm block count per scenario, never below
   `ONDA_BENCH_ITERATIONS`, targeting at least
   `ONDA_BENCH_MIN_ROUND_MS` per timing round. The report shows median ± median
   absolute deviation (MAD).
6. On Linux the script re-executes itself under `taskset` on the first allowed CPU, so Node/V8 and
   every native child inherit the same affinity; `ONDA_BENCH_AFFINITY_CPU` selects another allowed
   CPU. The report includes the effective affinity. By default the
   command exits unsuccessfully unless every scenario has a Wasm/LLVM runtime
   ratio of at least `1.00`, making a Binaryen win an explicit investigation
   rather than a surprising table cell. Workloads may set a larger margin, but
   the default does not misclassify a narrow LLVM win as a failure.

The checked-in scenarios have no audio inputs and expose scalar f32 outputs. Buffer scenarios bind
identically shaped, zero-initialized storage on both backends: mono and statically shaped buffers
use their declared channel count, while dynamic-channel buffers use two channels. The buffer
collection scenarios separately exercise constant, block-invariant, and sample-varying selectors;
each includes mono, fixed-channel, and dynamic-channel collections. A separate forwarded-invariant
scenario passes a collection window into a processor and measures constant plus block-invariant
selection after processor-helper lowering. These measurements exercise generated buffer access
without claiming to cover asset decoding, browser scheduling, AudioWorklet copying, or host-buffer
traffic. The Web Audio adapter has a separate cached-view/bulk-copy host path; backend numbers must
not be presented as end-to-end browser render timings.

## Illustrative run

Measured 2026-07-17 on an Intel Core i9-14900HX (32 logical CPUs), Linux x86-64,
Node 26.4.0, rustc 1.96.1, LLVM 21.1.2, and Binaryen 130, pinned to logical CPU 0.
The run used strict arithmetic with the realtime denormal policy, 128-frame
blocks, a 2,000-block minimum, a 100 ms target per round, nine timing rounds, and
five retained compile/instantiate samples. Binaryen used the production O4
profile with StackIR disabled. Every parity comparison remained within the
strict backend tolerance and LLVM was faster in every scenario. This historical snapshot predates
the sequential and interpolation buffer rows; the current command reports those in addition to the
four scenarios below.

| Scenario | Blocks/round | MIR MessagePack KiB | Wasm KiB | Binaryen ms | Instantiate ms | LLVM JIT ms | LLVM ns/frame | Wasm ns/frame | Wasm/LLVM |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Language/state/events | 2,048,000 | 9.1 | 0.4 | 34.44 ± 22.27 | 0.02 ± 0.00 | 5.11 ± 0.03 | 0.75 ± 0.00 | 0.77 ± 0.01 | 1.02× |
| Processor oversampling | 256,000 | 41.0 | 1.1 | 38.97 ± 2.10 | 0.03 ± 0.00 | 7.09 ± 0.14 | 5.87 ± 0.01 | 12.15 ± 0.05 | 2.07× |
| Saw/filter/saturator | 32,000 | 130.5 | 8.1 | 174.77 ± 2.28 | 0.05 ± 0.01 | 15.80 ± 0.24 | 32.77 ± 0.27 | 62.75 ± 0.44 | 1.92× |
| Complete f32/f64 math | 32,000 | 23.4 | 18.5 | 254.34 ± 1.03 | 0.08 ± 0.01 | 5.34 ± 0.14 | 45.93 ± 0.26 | 69.43 ± 0.52 | 1.51× |

These numbers are an illustrative snapshot, not a universal performance
baseline. The trivial language case is especially sensitive to CPU frequency;
compile time and the larger kernels also move with thermals and code-cache
state. MAD describes within-process samples, not drift between whole
invocations. Shared scalar-state promotion makes the trivial dependent-add
loop equally register-resident in both backends, so its 2% LLVM lead is
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
ONDA_BENCH_AFFINITY_CPU
ONDA_BENCH_DISABLE_AFFINITY
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

## Optimization findings

Binaryen snapshots descriptor fields for direct and constant-selected buffers into function-entry
locals because WebAssembly linear-memory aliasing otherwise prevents reliable loop-invariant load
motion. Runtime collection selectors are evaluated once per buffer operation and reused for pointer,
frame, and dynamic-channel lookup, including forwarded buffer spans and buffer-derived slices. Mono
and fixed-channel shapes use their declared channel counts directly; sample-varying selections retain
the descriptor loads that genuinely depend on the selected slot.

Portable inlining and unconstrained scalar-state promotion were both rejected by measurement.
Pre-inlining structured MIR regressed the larger reverb by roughly 30%, and promoting all 33
oversampling state scalars more than doubled the object size by creating excessive SSA/PHI
pressure. MIR therefore retains target inlining intent for the backend and caps portable state
promotion at a small alias-safe working set. Binaryen's global
`allowInliningFunctionsWithLoops` lever is exposed but defaults off: an A/B improved oversampling
while worsening the language and saturator workloads. These are target cost-model decisions, not
portable semantic transforms.
