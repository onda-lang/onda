# Onda benchmark and inspection harness

Use this harness to measure stdlib changes, compare compiler settings, replay saved MIR, and
inspect assumptions about operators and automatic SIMD. Workloads use ordinary scalar Onda.

Follow the [runtime performance policy](../../docs/architecture.md#optimization-boundaries): accept
generated-code growth for measured runtime gains, prioritizing LLVM. Object size is diagnostic
context and has no acceptance limit. Measure throughput and callback latency to assess runtime
effects such as instruction-cache pressure and register spills.

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

## UI comparison

The shared GPU screen has measurement harnesses separate from DSP benchmarks:

```sh
# Linux/EGL: unchanged production egui panels versus the shared wgpu screen.
cargo run --release -p onda_ui --features native-bench --example compare_native \
  > target/bench-ui-native.json

# Build ui/gpu/pkg first with npm run build:gpu-ui. Requires Chromium.
CHROME_BIN=/path/to/chromium node scripts/bench/ui-browser.mjs \
  target/bench-ui-browser.json

# Real standalone playback/control transport, without a window.
cargo run --release -p onda_run --example scope_cadence \
  > target/bench-ui-cadence.jsonl

# Require smooth fresh-data delivery and exercise the compact display budget.
ONDA_SCOPE_INTERVAL_MS=16 ONDA_SCOPE_COLUMNS=348 ONDA_SCOPE_MIN_HZ=45 \
  cargo run --release -p onda_run --example scope_cadence
```

The completed-panel workloads use the shared fixture in ui/gpu/fixtures/panels.json:

```sh
cargo run --release -p onda_ui --example panels -- target/ui-panels
ONDA_UI_PANELS=1 CHROME_BIN=/path/to/chromium node scripts/bench/ui-browser.mjs \
  target/bench-ui-panels-browser.json
```

They cover each of the seven tabs with a live scope, then with panel-specific updates
(values, waveforms, typed events, logs, MIDI activity, or smoothing settings).
The native harness writes one screenshot per tab and measures 240 warmed frames.
The browser comparison uses one warmed round per panel/scenario/implementation;
legacy sections are collapsed except scope and the selected section. Setup uses
the legacy device/settings controls. Layouts and widget implementations differ.
Build and validate first, stop other test applications, then run measurements
serially; do not overlap builds with benchmark collection.

The render fixtures use 768×840 pixels, 64 scalar knobs, and 1,024 stereo samples.
The native harness covers scope-only updates and scope plus changes to one or all
64 control values. The browser comparison covers scope-only and all 64 values.
Other editor sections are empty/hidden; layouts and features are not identical.
Native runs alternate implementations across three rounds, warming for at least
60 frames and 250 ms before collecting 300 measured frames each. They report CPU submission time and wall time through
GPU completion, without window presentation or audio/control IPC. The old path
uses the actual egui layout, widgets, scope, and OpenGL painter, exposed only by
the `onda_egui/ui-bench` feature. Both paths must use the same GPU; inspect the
reported adapter names. `ONDA_BENCH_EGL_DEVICE` selects the EGL device index.
The GPU rows also split value-message cloning/application, scope reduction, and
render submission. The comparison calls the shared UI directly; producer capture,
controller wakeups, and host schema publication are outside this measurement.
For before/after comparisons on hybrid CPUs, build first, then use the same core
for both runs, for example `taskset -c 0 target/release/examples/compare_native`.
Elapsed warmup and CPU affinity reduce noise; isolated slow rounds still need
reproduction before attributing them to a code change.

Browser runs exercise the actual DOM/Canvas2D page and Wasm/WebGPU canvas,
including the production JavaScript-to-Wasm message bridge. The DOM fixture uses
raw floats; the GPU fixture uses the compact display-sized envelope returned by
the current transport. Packet preparation is outside the timed browser loop;
payload sizes are recorded alongside timings. Each of three
alternating rounds has 60 warmup updates and 180 measured updates paced by the
browser's animation clock. Browser main-thread CPU measurements use DevTools'
`threadTicks` time domain; callback/handler samples use `performance.now()` wall
time. These metrics exclude compositor/GPU-process CPU and are not directly
comparable to the native GPU-completion metric. GPU/driver metadata is retained
so software fallback cannot be mistaken for a hardware measurement.
`ONDA_UI_CHROME_ARGS` accepts a JSON array of graphics flags for another setup.

The cadence probe starts the real playback child with two nearly inaudible
outputs and counts fresh scope responses over five seconds at each host polling
interval. It measures control-data delivery, not display presentation. A 16 ms
polling setting does not establish a 60 Hz scope: request/reply latency and the
controller's in-flight scheduling also constrain delivery. Build `onda_cli` in
release mode first so the child includes transport changes. `ONDA_SCOPE_COLUMNS`
selects compact packets; unset it for raw floats. `ONDA_SCOPE_INTERVAL_MS` selects
one interval, `ONDA_SCOPE_SECONDS` controls duration, and `ONDA_SCOPE_MIN_HZ` makes
the probe fail below the required cadence. All these results
are scoped benchmarks, not full VS Code/DAW startup or interaction-latency claims.

The [2026-10-26 UI optimization measurements](results/2026-10-26-ui-optimization.json)
retain before/after samples, hardware metadata, payload sizes, real transport
cadence, and source hashes. On that fixture, compact envelopes reduce browser
scope-update CPU from 2.02 to 0.91 ms and serialized scope messages from 40,692
to 3,756 bytes. Fresh standalone capture delivery rises from 15 to 61 Hz.
Native rendering remains approximately 0.06 ms through GPU completion.

The [completed-panel measurements](results/2026-10-26-ui-panels.json) retain all
raw results, source hashes, GPU metadata, and fresh scope cadence. On the
i9-14900HX / RTX 4080 Laptop GPU at 768×840:

| Workload | Legacy | Shared GPU |
| --- | ---: | ---: |
| Native scope, CPU + GPU completion | 0.691 ms | 0.057 ms |
| Native scope + 64 changing values | 0.673 ms | 0.180 ms |
| Browser scope, main-thread CPU | 4.584 ms | 0.912 ms |
| Browser scope + 64 changing values | 8.283 ms | 1.942 ms |

These comparisons take the median of three warmed round summaries. Completing
the panels leaves the earlier GPU scope/control costs effectively unchanged.

| Active panel, scope + panel changes | Native completion median | Browser CPU per update |
| --- | ---: | ---: |
| Parameters | 0.185 ms | 1.471 ms |
| Input controls | 0.142 ms | 1.320 ms |
| Buffers / waveforms | 0.170 ms | 1.570 ms |
| Typed events | 0.079 ms | 1.371 ms |
| Logs | 0.113 ms | 1.609 ms |
| MIDI | 0.068 ms | 1.183 ms |
| Setup | 0.076 ms | 1.304 ms |

Each panel uses one warmed round and the complete fixture, rather than the
64-control comparison fixture. Native scope-only medians span 0.052–0.204 ms;
browser scope-only CPU spans 0.872–0.951 ms. Fresh real scope delivery is 61.5 Hz.
These are rendering/transport measurements, excluding VS Code/DAW presentation,
audio-processing CPU, and end-to-end input latency.

The [2026-10-27 parameter-update investigation](results/2026-10-27-ui-parameter-updates.json)
compares the same native harness with and without recent-run text shaping reuse,
with all benchmark threads pinned to CPU 0. The 64-changing-value workload drops
from 0.096 to 0.079 ms CPU submission and from 0.176 to 0.157 ms through GPU
completion. The single-changing-value workload has no material improvement.
Temporary stage timing found shaping at 24.1 µs before and 8.8 µs after; scene
rebuilding and glyph preparation remain. Advanced shaping and font fallback are
preserved; cached runs expire after recent text batches and reset on font changes.

The earlier isolated 0.767 ms round did not recur in the first unchanged rerun
(0.1686–0.1687 ms); its exact cause was not captured. The imported capture
subscription, ring-copy, and host-wakeup changes are outside this rendering
benchmark. Parameter publications use the existing value-only path and do not
rebuild control schemas. These results explain the remaining rendering work,
without establishing a regression from those host changes.

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

`slice-fixed` and `slice-dynamic` pass contiguous views to the same elementwise helper,
with full-array or runtime lengths. They exercise MIR bounds proofs, descriptor lowering,
and backend vectorization; both report whole-block cost per host frame.

`voice-bank-16`, `voice-bank-256`, and `voice-bank-4096` update independent sine voices
in contiguous arrays, then mix them in a separate ordered loop. They exercise SIMD across
voices under strict floating-point rules and increase the active data set from 192 bytes to
48 KiB (two state arrays and one constant table). Each output frame updates every voice.
The compiler counters include eliminated common expressions and bounds checks.

`state-window-array-f32/f64` and `state-window-slice-f32/f64` read and write separate offset regions
of one state allocation, using fixed references or forwarded slice descriptors. They exercise
view shape propagation, bounds proofs, alignment and dependence analysis without changing storage layout.
The `state-window-shared-array-*` and `state-window-shared-slice-*` variants call the same helper on
two pairs of disjoint regions, testing joined view facts. `state-window-bounded-f32/f64` uses a
runtime count capped at 256, testing the dependence proof over a bounded iteration range. Both
variants preserve strict floating-point semantics and use exactly representable initial data.

`array-guarded-helper-f32/f64` and `array-guarded-state-f32/f64` traverse a runtime-sized prefix
of fixed arrays in forward and reverse order. They test branch-sensitive bounds elimination,
state/reference access and contiguous SIMD loads. Initialization uses exactly representable
fractions, allowing bitwise native/Wasm comparisons without a transcendental-math difference.
Like the operator probes, they report whole-block cost per host frame. Inspect gathers, clamps
and vector loads together with runtime measurements; fewer instructions alone do not establish
better cache locality.

## Complete FFTs and cache pressure

With FFTW development headers and both precision libraries installed:

```sh
python3 scripts/bench/fft.py --sizes 1024 4096 --precisions f32 f64 \
  --instances 1 32 512 --rounds 7 --round-ms 50 --cpu 6 \
  --output target/bench/fft-complete
```

Select an allowed CPU on your machine. This Linux-oriented harness emits native objects using
`--runner`, then compares the raw processor ABI with FFTW's measured split real-transform plan.
Both consume the same immutable N-sample input and publish all N/2+1 positive-frequency complex
bins. Onda's output-publication loop is included. Planning, allocation, initialization and
validation are excluded from timing. Rounds alternate implementations; repeat with alternating
old/new compiler runs before accepting a small difference. This is a throughput comparison of
complete operations, not a comparison of identical FFT algorithms or rounding sequences.

Validation checks every bin for impulses, DC, tones, mixed scales and seeded noise, then checks
each instance and reports an output fingerprint for old/new comparisons. The fingerprint is a
regression aid, not a cryptographic integrity check. New-array execution preserves shape, stride,
alignment and split-plane separation as required by [FFTW's contract](https://www.fftw.org/fftw3_doc/New_002darray-Execute-Functions.html).
Multiple instances increase the working set without adding allocation to the measured loop.
Optional Linux hardware counters report cycles, instructions, L1 data read misses and last-level
read misses per transform, with availability and multiplexing fractions. They run in separate
untimed passes. Inspect these measurements before attributing a speedup to cache locality.

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
and disassembly with relocations using `llvm-objdump` or `objdump`. Assembly inspection reports
external relocations, vector register instructions and stack address operands. Stack references
include local storage and ABI saves as well as spills; they are descriptive counts.

Add `--remarks` with LLVM 21 `opt` on PATH to save `.preopt.ll` and `.remarks.yaml` plus the
success/missed/analysis diagnostic stream for loop and SLP vectorization. Line tables map remarks
to Onda source files and lines. This runs the selected optimization pipeline separately from an O0
module, outside JIT timing; it is diagnostic replay, not a trace of the live JIT invocation.

Rows include parse/load, semantic analysis, MIR lowering and MIR optimization timings, cleanup
counts, and final effect/range function-visit counts. `--allocations` adds Rust allocation calls,
requested allocation bytes, and Linux process peak RSS for frontend and JIT compilation. Allocation
counts include realloc requests and exclude LLVM's C++ allocations; peak RSS covers the whole
process lifetime. Counter overhead affects instrumented compilation timings. Processing timings
run with the counters disabled.

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
`--fp-reassociate`, `--fp-contract`, and `--fp-no-signed-zeros` independently control native
arithmetic permissions. General reductions commonly need both reassociation and ignoring signed
zero to vectorize. Contraction permits one fused rounding instead of two. Vector math automatically
uses embedded SLEEF 3.9.0 on Linux/macOS x64 and ARM64 and Windows x64 MSVC; other targets keep
scalar math. JIT and AOT code include the helpers without an extra library dependency, including
cross-compilation. SLEEF documents a 3.5 ULP bound for `u35` (sin, cos, tan, tanh, atan, atan2, log),
and a 1 ULP bound for `u10` (exp, exp2, pow), within their supported input domains. Results need not be
correctly rounded or bit-identical to scalar libm. See `deps/sleef/docs/` for function-specific
qualifications. Vector math does not permit assuming finite inputs or discarding signed zeros.
`--fast-math` enables all arithmetic relaxations. These fine-grained settings apply to native
compilation; Wasm retains its own strict operation semantics.
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

## Compare backend compilation and runtime

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
Its JSON also includes cold-block maximum and steady-state block median, p99,
and maximum latency, so hop-based DSP bursts can be inspected independently of
average throughput. Scalar f32 inputs are supplied as deterministic silence.

### Methodology

The benchmark is a development diagnostic, with these safeguards:

1. The CLI emits compact MIR MessagePack once per scenario. The reported source-to-MIR duration is
   a single subprocess measurement that includes CLI startup and file I/O; it
   is displayed separately and is not treated as browser compiler latency. The
   MIR size column is the production MessagePack payload, not pretty JSON.
2. Binaryen compilation and WebAssembly instantiation each discard one
   in-process warmup, then retain
   `ONDA_BENCH_COMPILE_REPETITIONS` samples. The native helper parses and
   analyzes the source outside the JIT timer, lowers it to optimized MIR once,
   discards one LLVM JIT warmup, then measures the same number of LLVM O3 and
   ORC JIT compilations from that MIR. This matches Binaryen's side of the backend boundary.
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

### Settings

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

The [2026-07-17 results](results/2026-07-17.md) retain the original measurements, denormal
investigation, and rejected global optimizer experiments. The [2026-10-23 results](results/2026-10-23.md)
cover subsequent MIR, vector-math, and control-rate improvements. Historical results describe their
recorded compiler and machine; use repeated paired measurements for current performance decisions.

## Saved artifacts and tests

Dated reports and their raw JSON evidence live in `results/`; compiler design belongs in
[architecture](../../docs/architecture.md#optimization-boundaries) and the
[MIR contract](../../docs/mir.md#backend-neutral-passes). Keep experiment proposals and machine-specific
setup out of the public documentation. The [2026-10-23 report](results/2026-10-23.md) combines the
runtime audit, implementation results, and follow-up codegen review, including known regressions.

The [2026-10-22 performance cleanup results](results/2026-10-22.json) record the retained
improvements, removed experiments, worker identities, paired samples, validation and reproduction
steps. The final 12-case native check produces byte-identical objects to the compiler before the
removed experiments; it establishes preservation of code generation, not an additional speedup.
Historical compile-scaling, Wasm and FFT results are labeled separately from that cleanup check.

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
