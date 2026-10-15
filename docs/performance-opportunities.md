# Runtime performance opportunities from the example corpus

For the direct cumulative comparison with the original compiler, see
[cumulative original-to-current results](#cumulative-original-to-current-results):
5.0% lower native geometric-mean runtime across 39 examples at that checkpoint, with
one remaining 3.1% transient-sculptor regression. Subsequent incremental work is recorded
under [loop induction and native range facts](#loop-induction-and-native-range-facts).

All 54 checked-in `.onda` sources compiled to optimized MIR, LLVM IR, and native objects.
The audit includes the three project entry sources and their dependencies. The project
manifests contain no compile-constant overrides; their buffer assets were not loaded for
this source-compilation audit. Ten support modules have empty process entrypoints, and
five executable sources need external buffers. The other 39 sources were benchmarked
with their default parameters. Plugin instruments without MIDI events measure their
idle path, so these timings are not an active-voice benchmark.

The initial baseline inspection was in `target/performance-examples/baseline/`; native timing sweeps
are in `timings-baseline/`, `timings-inbounds/`, and `timings-promotion/` beside it.
These are local generated files. Instruction counts establish investigation targets,
not speedups; the repeated confirmation measurements below support the retained change.
Superseded binaries, duplicate inspection dumps, and old per-run MIR/source copies were
pruned to recover disk space. Historical timing results, output snapshots, source hashes,
and compact inspection metadata remain; old artifact paths below describe the experiments
and do not imply every intermediate is retained. The cleanup manifest is
`target/performance-archive/cleanup-2026-10-15.json`. The original baseline executable,
original-to-current comparison, and current round's workers and inspections were retained.

## Initial implementation: retain bounded scalar promotion in large processors

Previously, finding more than eight eligible scalar state slots disabled MIR promotion
entirely. The initial fix took the first eight eligible slots in deterministic state order,
keeping the existing budget and alias/lifecycle exclusions. The follow-up measurements
below increase the budget to 64 with runtime taking precedence over code size. Promotion
makes state values local across the process loop while preserving persistent storage
and the per-segment load/commit boundary. Both native and Binaryen consume the change.
Source: [state promotion](../crates/onda_mir/src/passes/state_promotion.rs).

The first implementation used the existing state order with no new profiling heuristic
or larger budget. The follow-up measured a larger budget and a loop-depth/access-count
ranking separately; the ranking was rejected after runtime regressions.

Measurements used base revision `215c6e0716d8efa91d425af5df605022e5071cfc`, an Intel
Core i9-14900HX pinned to logical CPU 0, LLVM 21.1.2 O3, strict math, 48 kHz and
128-frame blocks. Each confirmation used nine rounds targeting at least 50 ms, with
512 warmup blocks. The table is the median of three complete runs per variant,
alternating variant order. CPU frequency was not locked. Full records and individual
rounds are under `target/performance-examples/confirmation/`; `summary.json` summarizes
the nine cases. Early exploratory sweeps overlapped some other workers; the confirmation
runs were sequential and are the basis of the percentages below.

| Example | Baseline ns/frame | Bounded subset ns/frame | Runtime change |
| --- | ---: | ---: | ---: |
| Dual FM | 95.424 | 91.009 | −4.6% |
| Saw/filter/saturator | 64.176 | 64.030 | −0.2% |
| Stereo chorus | 23.463 | 21.370 | −8.9% |
| Tape echo | 36.836 | 34.105 | −7.4% |
| Benjolin | 51.575 | 50.215 | −2.6% |
| Neural synth | 535.886 | 531.903 | −0.7% |
| Additive synth | 34.909 | 35.153 | +0.7% |
| Drum machine | 23.895 | 24.288 | +1.6% |
| Spectral freeze | 36.197 | 36.872 | +1.9% |

All startup and warmed native output snapshots in those confirmation runs were
bit-identical to baseline. Changes around 1–2% should not be treated as established
speedups or regressions on this machine. This is a useful first improvement in several
examples, not a substantial speedup across the entire language.

Wasm checks used Node 26.10.0 and the pinned Binaryen backend, with the same input,
warmup, block and timing settings. Two complete runs reversed baseline/candidate order.
The observed candidate/baseline runtime changes were:

| Example | Wasm runtime change across the two runs |
| --- | ---: |
| Dual FM | −1.8% to −1.2% |
| Stereo chorus | −0.9% to +1.0% |
| Tape echo | +0.4% to +4.4% |
| Additive synth | +4.7% to +7.1% |
| Neural synth | −1.2% to +0.04% |

Every startup and warmed Wasm snapshot was bit-identical before and after the MIR
change. The additive-synth slowdown is a real tradeoff worth tracking; accepting it
here follows the native-first performance goal. These are five Node/V8 spot checks,
not a browser-wide performance guarantee. Artifacts: `wasm-only-{baseline,promotion}/`,
`wasm-repeat-{baseline,promotion}/`, and `wasm-repeat-comparison.json` under
`target/performance-examples/`.

Native/Wasm agreement is a separate issue: **the unchanged baseline** exceeds the
harness's default absolute-plus-relative tolerance in stereo chorus (block 2, channel
0, frame 35: 0.1068071499466896 vs 0.10680602490901947) and neural synth (block 2,
channel 0, frame 74: 0.08830095827579498 vs 0.08829984813928604). The logs are in
`wasm-baseline/` and `neural-parity-baseline/`. No tolerance was relaxed to report parity.
Wasm timing uses independent finite-output validation and exact before/after comparison
within each backend; it does not claim those two cross-backend checks passed.

The LLVM experiment changed fixed-array element GEPs to `inbounds`. It improved neural
synth from 535.886 to 510.630 ns/frame (4.7%) in the same confirmation, but most other
confirmed changes were small. That initial experiment was not retained; the follow-up
below implements the target-width proof and measures against the promoted-state baseline.

## Reproducing corpus inspection

```sh
cargo build --release -j2 -p onda_examples --features llvm-orc --example benchmark_dsp
python3 scripts/bench/run.py --examples --inspect-only --assembly \
  --timeout-seconds 120 --output target/bench/examples
python3 scripts/bench/run.py --source examples/effects/stereo_chorus.onda \
  --ir --assembly --repetitions 9 --round-ms 50 --output target/bench/chorus
```

Use distinct output directories and preserve a baseline worker with `--runner` before
changing the compiler. The inspection tool now counts `tail call` and `musttail call`
as calls; its previous opcode parser silently missed most optimized math calls.
The post-change all-source sweep is in `target/performance-examples/final-corpus/`.
`target/performance-examples/corpus-summary.json` indexes all 54 baseline sources with
MIR loop/bounds counts and process-function LLVM load/store/vector/call counts.

Validation completed with 126 MIR tests, 825 example integration tests (one ignored),
99 Binaryen backend tests, and all 12 harness tests including its native/Wasm integration
checks. The new state-promotion execution regression additionally exercises a zero-frame
BEGIN, split blocks, a second block, and an event changing both selected and unselected
state slots. Formatting and whitespace checks pass. An initial harness run hit the
existing immediate process-termination assertion under load; both subsequent ordinary
and integration runs passed without changes to that test or the process launcher.

The most promising sequence is to preserve existing bounds and memory facts in LLVM,
improve portable MIR memory reasoning, then investigate loop organization and math
kernels on workloads where profiling shows they dominate. Broadly enabling vectorization
is unnecessary: the compiler already runs LLVM's normal O3 pipeline, and several examples
already contain substantial SIMD. See [the pipeline](../crates/onda_codegen_llvm/src/orc_backend/jit_utils.rs)
and [benchmark methodology](../scripts/bench/README.md).

## What the corpus actually shows

The baseline contains 54 LLVM modules. Ten have effectively empty process entrypoints:
they define reusable processors or support code without invoking them. Keep these in the
compilation coverage denominator, but exclude them from runtime speedup aggregates.

| Example | Observation in optimized `onda_process` | Consequence for investigation |
| --- | --- | --- |
| [Voice bank](../examples/basic/voice_bank.onda) | The inspection report counts 194 vector instruction lines and 32 shuffles; the IR contains masked wavetable gathers. | SIMD exists already. Compare instruction and gather costs before forcing wider vectors. |
| [Additive synth](../examples/instruments/additive_synth.onda) | Its inner eight-voice loop reloads the fundamental and note ratio and multiplies them at each voice; array projections use ordinary GEPs in the baseline. | Investigate why disjoint array stores prevent invariant scalar loads from moving. |
| [FDN reverb](../examples/effects/processors/fdn_reverb.onda) | Many flattened delay-buffer accesses retain `smax`/`umin` index clamps, including each of eight FDN reads and writes. | Range information about persistent indices and conditional wrapping is missing or insufficient. |
| [Neural synth](../examples/feedback/neural_synth.onda) | Two four-float oversampling output temporaries are cleared inside the host-sample loop before a four-iteration producer loop. | Prove full overwrite and eliminate redundant initialization before lowering. |
| [Sine](../examples/basic/sine.onda) | The process loop is unrolled but retains scalar `llvm.sin.f32` calls and a sequential floating phase recurrence. | A vector math library alone does not remove the phase dependence. |
| [Embedded room](../examples/projects/embedded_room/code/main.onda) | The process function has 7,726 textual IR lines and several retained internal calls. | Inspect instruction-cache pressure and inlining as well as arithmetic throughput. |

Static searches find `tanhf` in 31 process functions, `llvm.exp.f32` in 35,
`llvm.sin.f32` in 18, and `tanf` in nine. These count functions containing a call,
not calls executed per frame: many coefficient calculations run only at block boundaries.
The baseline `.ll`, `.asm`, `.mir.txt`, and `.inspection.json` files preserve the evidence
under the corresponding source-relative directory.

## 1. Preserve bounded address calculations in LLVM

**First experiment; native-only implementation.** Fixed-array projections already pass
through `lower_fixed_index`: Clamp normalizes the index, Checked branches to failure,
and trusted Unchecked carries the producer's validity contract. The baseline emits an
ordinary GEP after that work. Conveying the established guarantee with `inbounds` may
help alias analysis and loop motion, especially when mutable arrays and invariant
scalars share the processor state allocation. The additive-synth loop is a concrete
before/after target. Sources: [array and window lowering](../crates/onda_codegen_llvm/src/orc_backend/mir_native/function_emitter.rs),
[fixed-array validation](../crates/onda_mir/src/validate.rs), and
[trusted bounds contract](mir.md#validation).

LLVM 21 `inbounds` also requires signed offset arithmetic not to wrap. Valid element
counts alone therefore do not justify it for arrays exceeding the signed pointer-index
byte range on 32-bit targets. Use the selected data layout to bound the array size;
retain ordinary GEP when that proof is unavailable. Empty slices and dynamically
strided buffer references need separate proofs. `inbounds` is not a declaration that
all references are mutually disjoint. [LLVM 21 GEP semantics](https://releases.llvm.org/21.1.0/docs/LangRef.html#getelementptr-instruction).

Check that invariant loads actually move and state traffic falls, then measure.
Changing a spelling in IR without a timing improvement is insufficient. Exercise
clamped negative/large indices, checked failures, valid trusted accesses, array windows,
and a 32-bit large-array emission case.

## 2. MIR load forwarding and conservative memory CSE

**Portable, bounded starting point.** Existing MIR CSE recognizes arithmetic,
comparisons, casts, and intrinsics with stable local operands. It intentionally excludes
memory and descriptor operations. Consequently the SVF benchmark's MIR repeatedly
loads the same coefficient references within a single straight-line expression group.
LLVM often cleans these up after inlining; portability and newly exposed optimization
chains are the reasons to implement this in MIR, and native gains must be demonstrated.
Sources: [CSE](../crates/onda_mir/src/passes/cse.rs),
[pass orchestration](../crates/onda_mir/src/passes.rs),
[SVF](../stdlib/std/filter.onda).

Start with exact scalar places and straight-line regions. Invalidate entries on writes
that may overlap, mutable reference calls, slice writes, and unknown indirect effects;
use existing transitive [effect analysis](../crates/onda_mir/src/analysis.rs) rather than
a second call-effects system. State IDs and local IDs identify distinct storage, while
two function reference parameters can alias. A store through one reference must not
leave cached loads through another reference valid without a disjointness proof.
Preserve checked-operation failures even when the resulting value is unused.

Only after this works should the same provenance support loop-invariant code motion.
Hoist non-failing pure expressions whose operands are invariant and whose execution
does not cross observable failures or nontermination. Avoid a general memoization
framework as the first implementation.

## 3. Recover ranges through control flow and reference ABIs

**Portable, potentially valuable for delay-heavy code.** Current bounds elimination
uses whole-function integer summaries. Branch analysis joins environments without
refining them from the condition, and loop analysis resets mutated locals to declared
ranges. Call-range observations accept value and explicitly readonly parameters;
generated proc functions commonly use readwrite references even when their inferred
parameter effects are readonly. Sources: [range analysis](../crates/onda_mir/src/analysis.rs),
[bounds proofs](../crates/onda_mir/src/passes/bounds_proofs.rs),
[uniform generated ABI](../crates/onda_semantics/src/mir_lowering.rs).

Two increments are more tractable than a new general optimizer: allow effect-proven
readonly references through the existing alias checks, and refine scalar intervals
inside integer comparison branches. Persistent array elements and induction invariants
are a further step. The FDN example uses unannotated index arrays and manual conditional
wraps, so the first two changes alone do not establish all its needed invariants.
Sample-rate-dependent delay lengths and externally restored state must remain valid;
the compiler cannot simply assume that every ring-buffer-looking access is safe.

Measure clamp removal and pointer arithmetic in FDN, Schroeder reverb, Karplus–Strong,
and the project buffer examples. Preserve clamped behavior for genuinely out-of-range
inputs rather than converting them to unsafe accesses.

## 4. Remove fully overwritten scratch initialization

**Portable and narrowly testable.** Oversampling in the neural-synth baseline clears
two 16-byte scratch arrays once per host frame. The four generated subframes write
those arrays before decimation reads them. A definite-full-overwrite proof could remove
these fills, and similar patterns occur in other oversampled examples. Source and
baseline evidence: [neural synth](../examples/feedback/neural_synth.onda) and
`baseline/feedback/neural_synth/neural_synth.ll`, process block `process_frame_ok`.

Recognize compiler-generated scratch with a complete constant-trip-count write loop,
without reads or escaping references before completion. Preserve initialization if a
path skips an element and subsequently reads it. Failure paths are acceptable only
when the uninitialized scratch is unobservable after failure. This is local dead-store
elimination, not permission to remove source-level array initialization generally.
Measure an isolated oversampling workload as well as complete examples: 32 removed
bytes per host frame may be a small fraction of a math-heavy synth's cost.

## 5. Vector math and loop organization

**Higher upside, higher implementation and numerical cost.** Sin/cos/exp/log already
lower to LLVM intrinsics; tan/tanh/atan/atan2 lower to scalar library symbols. Onda
currently selects a standard target optimization pipeline, with no configured vector
math library. [Math lowering](../crates/onda_codegen_llvm/src/orc_backend/mir_native/function_emitter.rs)
and [pipeline setup](../crates/onda_codegen_llvm/src/orc_backend/jit_utils.rs).

LLVM's vectorizer can use supported math intrinsics, but many transcendental operations
need an available vector-library implementation. Its profitability model also accounts
for gathers, register pressure, and code growth. A vector width hint cannot supply a
missing vector math implementation. [LLVM 21 vectorizers](https://releases.llvm.org/21.1.0/docs/Vectorizers.html#vectorization-of-function-calls).

Prototype a target-aware vector math integration on independent voice or neural
activation loops, with explicit native JIT and AOT symbol resolution. Compare strict
and explicitly relaxed policies separately, including exceptional values and numerical
error. Existing `fast_math` enables all LLVM fast-math flags; silently applying those
flags to ordinary programs would change their contract.

The neural example offers another route: compute independent activation inputs in one
loop, evaluate activations in a second, then update state. Preserve the order of its
shared random-number generator and its ordered floating reductions. General loop
distribution requires dependence and failure-order proofs, so first test an equivalent
source variant to establish whether the expected vectorization is worthwhile.

## 6. Coefficient reuse, layout, and instruction footprint

**Profile-guided follow-up, not blanket caching.** Examples such as tempo ping-pong
recompute exponential coefficients at block boundaries even with unchanged controls.
The current source intentionally invokes setters; handlers may have effects, so the
compiler cannot skip arbitrary repeated calls. Start with a proven pure coefficient
function or an explicitly maintained stdlib coefficient cache. Benchmark realistic
automation rates: a cache adds comparison and state traffic when controls change every
block. Sources: [tempo ping-pong](../examples/plugins/effects/tempo_ping_pong.onda),
[smoothing coefficients](../stdlib/std/smoothing.onda), and
[filter coefficient updates](../stdlib/std/filter.onda).

State layout already flattens aggregate and processor leaves; introducing SoA is not a
new optimization in itself. Consider arranging hot scalar state independently of large
delay buffers only after cache measurements identify a problem. The 8,192-point sine
table stores adjacent value/delta pairs in 64 KiB; voice-bank SIMD gathers make it a
specific locality experiment. A smaller table or approximation changes numerical
behavior and belongs in an independently validated stdlib/math experiment.
Sources: [MIR aggregate boundary](mir.md), [state layout](../crates/onda_codegen_llvm/src/orc_backend/mir_native.rs),
and [oscillator table](../stdlib/std/osc.onda).

Generated proc helpers carry `inline=always`; the embedded-room IR illustrates the
possible resulting code footprint. Investigate retaining larger FFT/convolution or
oversampling helpers while inlining small scalar wrappers. Measure compile time,
process latency, code size, and lost constant propagation together. Removing all
inlining hints would also discard useful source-level information.

The promotion transform also runs again when serialized optimized MIR is passed back
through `optimize`. A straight-line state-update probe exposed additional temporaries
on repeated optimization, so structural idempotence is another targeted follow-up.
The retained change caps distinct selected state slots; it does not make the entire
promotion transform idempotent. Production source compilation and the confirmation
measurements run promotion once.

## Acceptance criteria

Use pinned, repeated native runs and compare output snapshots under identical settings.
Include warmed and startup outputs, segmented processing, control changes, and finite
and exceptional-value correctness probes appropriate to each transformation. Check
Wasm on representative changed kernels, especially state promotion and math changes.
The existing [harness](../scripts/bench/README.md) records artifact hashes, compiler
identity, timings, parity, and inspection artifacts; use those rather than hand-timed
CLI invocations. Its scalar-port runner does not exercise bound buffers or host events,
so compilation coverage must not be reported as execution coverage for those cases.

The initial audit retained only bounded scalar promotion. Subsequent implementation
and measured experiments are recorded below.

## Follow-up experiments

Artifacts for this round are in `target/performance-round2/`. Its `baseline` worker
includes the previously retained eight-slot state-promotion change. Results here are
incremental to that compiler, not percentages to add to the initial audit's speedups.
All runs use strict math, 48 kHz, 128-frame blocks, the harness's pinned CPU, nine
rounds of at least 50 ms, and 512 warmup blocks. Variants run sequentially and reverse
order on alternating repetitions. CPU frequency and unrelated desktop activity are
not controlled, so small differences remain inconclusive.

### Guarded LLVM array addresses

Fixed-array element and fixed-array window projections now share a bounded address
builder. Bounds normalization precedes it. The target allocation size must fit the
signed pointer-index width before emission uses `inbounds`; otherwise it retains an
ordinary GEP. Dynamic slices and external buffer addressing keep their existing rules.
MIR additionally rejects fixed aggregates larger than `i32::MAX` logical bytes.
Tests cover native, i686, and wasm32 emission at that boundary, explicit narrower
index layouts, and clamped and trusted reads/writes at O0 and O3.

Three paired runs (`address-confirmation/`) measured neural synth at 524.457 →
511.363 ns/frame (2.5% improvement) and additive synth at 34.438 → 33.540 ns/frame
(2.6%). The other nine selected workloads changed by less than 2% in their aggregate
medians. All 66 startup/warmed output snapshot comparisons were bit-identical.
Both `address-corpus/` and `ranges-corpus/` contain all 54 compiled sources.

The optimized process IR has more vector instructions: neural synth 150 → 244 and
additive synth 41 → 117 relative to the promoted-state baseline. These are static
instruction counts, including setup, not executed SIMD coverage. Vector math
intrinsics may still lower to scalar calls without a vector math implementation.

### Oversampling scratch scope

The MIR producer now initializes top-level oversampling output scratch once per
segment, before the host-frame loop. This retains MIR's definite-initialization
proof while relying on the generated substep schedule's complete overwrite before
decimation. The per-substep output defaults remain intact, including conditionally
unwritten outputs. Processor-helper scratch retains its per-invocation initialization.

Five paired neural-synth runs measured 536.875 → 532.595 ns/frame (0.8%), and three
Wasm runs measured 1457.721 → 1450.956 ns/frame (0.5%). These differences are too small
and variable to establish an end-to-end speedup. This change removes repeated scratch
setup; it is not evidence of a substantial runtime gain. All 16 output snapshot
comparisons were bit-identical. See `scratch-confirmation/` and `wasm-scratch/`.

### Experiments excluded from the compiler

- **Scalar load reuse:** a conservative straight-line MIR pass used transitive call
  effects and invalidated cached loads on possible alias writes and control-flow
  boundaries. Its 132 MIR tests passed, but three native repetitions across 11
  workloads and two Wasm repetitions across six workloads showed no convincing
  improvement. LLVM/Binaryen already remove most of the same work. Removed the pass;
  retained its worker, source, tests, patch, and measurements in this artifact directory.
- **Effect-proven readonly range propagation:** expanded inference to generated
  readwrite-reference parameters when call effects prove them readonly, retaining
  the alias checks. Tests passed, but all 54 MIR and LLVM modules were byte-identical
  to the address variant. Removed it rather than claiming an unobserved performance gain.
- **Neural loop interchange:** a source prototype accumulated independent neurons
  across rows while preserving each neuron's reduction order and RNG call order.
  Outputs were bit-identical, but three runs showed no gain (514.057 → 516.898
  ns/frame). The example remains unchanged; the prototype is `neural-interchanged.onda`.
- **Loop-depth-ranked state promotion:** selected the same bounded eight slots by
  loop depth and access count. It regressed chorus by 10.2%, tape echo by 8.0%, and
  dual FM by 4.9%, despite a 3.1% drum-machine improvement. Removed the heuristic.
  Loop depth alone does not identify the best state to cache; the original deterministic
  selection retains useful control snapshots. See `ranked-confirmation/`.

### Whole-corpus confirmation before increasing the promotion budget

The array-address and scratch-scope changes, with the eight-slot cap, were compared with this round's
baseline across all 39 executable scalar-port examples, with three paired runs per
example (`full-confirmation/`). Neural synth measured 548.607 → 527.014 ns/frame
(3.9% improvement); additive synth measured 35.874 → 35.108 ns/frame (2.1%).
The other 37 aggregate changes ranged from 1.8% faster to 2.6% slower. The largest
apparent regression was voice bank, whose generated code is unchanged; its individual
runs varied substantially, so that difference does not establish a compiler regression.
All 234 startup/warmed output snapshot comparisons were bit-identical. These runs use
default controls and no host events; plugin instruments therefore include idle paths.

All 54 sources compile with MIR, optimized LLVM IR, object, and assembly artifacts in
`final-corpus/`. Validation passed 158 LLVM tests, 126 MIR tests, 1,084 semantic tests,
826 example tests (one ignored), 99 Binaryen tests, and the benchmark harness unit
suite (two integration tests skipped), plus compiler Clippy and formatting checks.
The native corpus confirmation is execution coverage for 39 sources, not all 54.

Runtime is the acceptance criterion. Larger generated code is acceptable when it
improves execution time. For example, the retained address change increases additive
synth's native text from 4,600 to 6,152 bytes and neural synth's from 5,525 to 6,194
bytes. These increases do not count against their measured runtime improvements.

### Larger scalar-state promotion budget

With runtime taking precedence over generated code size, the promotion budget was
retested at 8, 16, and 64 slots. Three paired runs over 11 examples
(`budget-confirmation/`) found little benefit from 16 slots, but repeatable gains at
64. All 132 startup/warmed comparisons were bit-identical. The compiler now retains
64 slots in deterministic state order with the same alias and lifecycle exclusions.
Both the MIR budget test and the segmented/event execution test use 68 state slots,
so they exercise the promoted prefix and the remaining state.

A separate three-pair comparison across all 39 executable examples
(`budget-corpus/`) measured the following incremental effects of changing 8 → 64.
Both variants already include the address and scratch changes above.

| Example | Eight slots, ns/frame | 64 slots, ns/frame | Runtime change |
| --- | ---: | ---: | ---: |
| effects/stereo_chorus | 21.723 | 17.499 | -19.4% |
| effects/wavefolder | 7.542 | 6.191 | -17.9% |
| effects/fdn_reverb | 24.319 | 21.244 | -12.6% |
| effects/schroeder_reverb | 24.024 | 21.166 | -11.9% |
| effects/live_tape_looper | 25.606 | 23.258 | -9.2% |
| effects/tape_echo | 34.921 | 31.803 | -8.9% |
| plugins/effects/reactive_wavefolder | 88.978 | 82.644 | -7.1% |
| instruments/additive_synth | 35.199 | 37.433 | +6.3% |
| plugins/effects/transient_sculptor | 12.149 | 12.843 | +5.7% |
| soundscapes/deep_space_drone | 113.978 | 116.452 | +2.2% |

The equal-weight geometric mean runtime decreased by 2.8%; this is a corpus summary,
not a prediction for a particular application mix. All 234 startup/warmed comparisons
were bit-identical. Additive synth and transient sculptor are repeatable native
regressions rather than measurement noise; the follow-up below addresses them.
The budget remains bounded to avoid unbounded growth of loop-carried values; 64 is a
measured policy, not a claim that every workload's optimal budget is 64.

Three paired Wasm repetitions (`wasm-budget/`) also favored 64 slots: chorus 12.5%
faster, tape echo 14.8%, neural synth 4.7%, additive synth 2.7%, drum machine 14.2%,
and Benjolin 8.7%. All 36 startup/warmed comparisons were bit-identical. These are
within-backend comparisons; they do not change the earlier strict cross-backend parity
limitations. This sample provides no evidence of a Wasm regression from the larger cap.

Final validation with the retained 64-slot policy passed the same 158 LLVM, 126 MIR,
1,084 semantic, and 826 example tests (one ignored), plus the compiler doctest, Clippy,
and formatting checks. Fresh MIR, optimized LLVM IR, native objects, and assembly for
all 54 sources are in `budget-final-corpus/`, whose status is `completed`.
The final process IR contains 261 vector-instruction lines for neural synth and 113
for additive synth. Scalar `tanhf`/`tanf` calls remain in neural synth, and vector math
intrinsics still need a vector implementation to guarantee vector machine calls.
The next larger SIMD opportunity remains vector math and independent-voice scheduling;
static vector counts alone do not establish that those bottlenecks are solved.

## Diagnosing the promotion regressions

The next investigation is recorded in `target/performance-round3/`; comparison workers
and some timing directories remain under `target/performance-round2/`. The previous
`budget64` worker is the baseline for this round. The old `final` worker is the
array-address/scratch compiler with the eight-slot cap, used to check recovery.

`python3 target/performance-round3/reproduce.py` reproduced both regressions and
failed its 3% threshold: additive synth +7.23%, transient sculptor +9.47% in that
run. The same differences persisted at 1,024-frame blocks (about 2.09 and 0.78
ns/frame respectively), so they were not just extra block-setup work.

Promotion eligibility is a correctness property; profitability is a separate decision.
Replacing memory accesses with process-long values changed LLVM's live ranges, SLP
packing, and instruction scheduling. The native additive process grew from 876 to
926 disassembled instruction lines, with 193 → 256 stack-reference lines. Transient
sculptor grew from 291 to 362 lines, with 62 → 125 stack-reference lines, even though
its LLVM vector-instruction count increased. These are static whole-function counts,
including setup, not executed spill counts or a hardware profile.

Field-group experiments established the useful boundaries:

- Leaving transient sculptor's conditionally read attack/release coefficients in
  memory measured 12.505 → 11.732 ns/frame, while retaining its other promoted state.
- For additive synth, retaining the scalar state used in nested voice work and leaving
  unrelated outer-loop state in memory measured 34.841 → 32.631 ns/frame. The eight-slot
  comparison measured 32.825 ns/frame in the same experiment.
- All 132 snapshot comparisons across the field-group, individual-field, and nested-work
  experiments were bit-identical. Their sources and math settings were unchanged.

These results implicate unprofitable promotion and its downstream code-generation
choices. They do not establish a universal rule that fewer loads, more SIMD operations,
or fewer LLVM instructions imply lower runtime.

### Retained selection policy

The cap stays at 64. Selection favors the deepest unconditionally entered loop depth
where eligible scalar state is accessed, so unrelated outer-loop state is not automatically
kept live across nested scalar work. Array-only inner loops and loops inside ordinary
branches do not displace enclosing scalar caching. Flat loops retain caching for state
used outside the loop. An earlier depth-only variant regressed acid bassline by 17.6%:
its conditional note-trigger loop displaced state used on every frame. That variant was
rejected, and a regression test now covers conditional nested work.
Direct loads and loop mutations remain candidates. For repeated helper instances, a
reference read only inside callee branches can remain a selected memory operand instead
of a long-lived cached coefficient. Single helper instances retain their coefficient
caching. Static invocation counts are capped at two and propagated through shared call
graphs; reference-read hints propagate through forwarding wrappers.

Call-transitive effects inform profitability, while the existing conservative reference
writeback policy and alias/lifecycle exclusions remain intact. An early experiment also
removed effect-proven redundant writebacks; it regressed Benjolin despite an unchanged
promoted field set. Restoring those writebacks restored its LLVM IR byte-for-byte. That
independent transformation was excluded. A broader read-only-reference exclusion also
lost chorus/reverb gains, and excluding conditional coefficients without considering
repeated instances regressed the compressor. Those variants were rejected.

The new semantic-lowering tests cover repeated conditional readers, a single conditional
reader, repeated unconditional readers, nested scalar work, array-only nested work,
and conditional nested work.
They verify the actual generated proc/reference patterns rather than source-name filters.
The timing reproducer is the runtime regression check; structural tests do not promise
identical cost-model decisions or timings across CPUs.


The final conditional-loop refinement leaves 49 of 54 complete LLVM modules byte-identical
to the 64-slot baseline. The changed modules are additive synth, transient sculptor,
acid bassline, neural synth, and deep-space drone. In particular, chorus, tape echo,
Benjolin, compressor, glass garden, and the idle FM-bells plugin keep their previous IR.
The revised additive process has 105 vector-instruction lines versus 113 previously;
transient sculptor has 62 versus 77. These decreases are compatible with improved
runtime. No fast-math setting or source-level DSP formula was changed.

Validation after the refinement: 158 LLVM, 126 MIR, 1,090 semantic, and 826 example
tests passed (one example test ignored), plus the compiler doctest, Clippy with warnings
denied, formatting, and diff checks. All 54 sources compiled again to MIR, optimized
LLVM IR, objects, and assembly in `target/performance-round3/guarded-corpus/`.

### Final measurements

Three alternating-order pairs across all 39 executable examples used the same pinned
CPU, strict math, 128-frame blocks, and nine 50-ms rounds as above. The table compares
medians with the previous unselective 64-slot compiler (`guarded-corpus-timing/` under
`target/performance-round2/`). Negative changes mean less time per frame.

| Example | Previous 64-slot ns/frame | Refined ns/frame | Runtime change |
| --- | ---: | ---: | ---: |
| instruments/additive_synth | 37.794 | 35.247 | -6.74% |
| plugins/effects/transient_sculptor | 12.878 | 12.230 | -5.03% |
| feedback/neural_synth | 550.906 | 532.454 | -3.35% |
| instruments/acid_bassline | 67.489 | 67.592 | +0.15% |
| soundscapes/deep_space_drone | 116.678 | 115.117 | -1.34% |

Additive synth and transient sculptor improved in every pair. Deep-space drone improved
by 1.3–2.6% across the pairs. Neural synth's apparent median gain was inconsistent
between pairs (−0.1%, +0.4%, −4.6%), so it is not an established speedup. Acid bassline
is effectively unchanged in the full-sweep median. The geometric mean across all 39
cases decreased by 0.33%; that small corpus-wide difference is not a robust performance
claim. Byte-identical modules also showed timing noise, including shimmer echo and
voice bank, so isolated timing changes there are not attributed to this compiler policy.

A separate three-pair comparison with the old eight-slot compiler (`guarded-repro/`)
measured additive synth −0.23% and transient sculptor +0.77%, passing the reproducer's
3% regression threshold. Both earlier regressions are recovered to measurement-scale
differences while retaining the earlier winners' generated IR. All 234 full-corpus and
12 eight-slot-comparison startup/warmed output snapshots were bit-identical.

Three paired Wasm runs (`guarded-wasm/`) measured additive synth −5.26%, transient
sculptor +1.49%, chorus +0.50%, tape echo −0.21%, Benjolin −0.44%, and acid bassline
−0.26%. Transient's small Wasm cost is retained for the measured native improvement,
consistent with the native-first objective. All 36 startup/warmed Wasm snapshots were
bit-identical. These are same-backend comparisons, not a claim of cross-backend parity.
The final three benchmark groups therefore contain 282 bit-identical comparisons.

The changes remain target-independent MIR selection heuristics; their profitability is
CPU/workload dependent. The measurements do not establish hardware cache-miss counts
or executed spill counts. Further work on vector math and independent-voice scheduling
remains separate from this regression fix.

## Historical cumulative original-to-round-four results

This is a direct comparison of the saved compiler from before this optimization work
(base revision `215c6e0716d8efa91d425af5df605022e5071cfc`) with the retained compiler at
that stage. It does not multiply percentages from intermediate experiments. The workers
are `benchmark_dsp-original` and `benchmark_dsp-guarded`; measurements, worker hashes,
input/source snapshots, and summaries are in `target/performance-round2/original-to-current/`.

Native runtime decreased by **5.0% in the equal-weight geometric mean across 39 examples**;
the median per-example change was −2.5%. Sixteen example medians improved by more than
3%, 22 were within ±3%, and one regressed by more than 3%. These are observed timing
buckets, not statistical significance tests. Measurements used three alternating-order
pairs, each with nine 50-ms rounds, on logical CPU 0 of the i9-14900HX, LLVM 21.1.2 O3,
strict math, 48 kHz, and 128-frame blocks. Frequency and desktop activity were not locked.

The table covers all 39 executable scalar-port examples. The remaining 15 of the 54
compiled sources are ten support modules with empty process functions and five workloads
requiring external buffers. Plugin instruments were measured without MIDI and therefore
cover idle processing only. These native figures are not a full-corpus Wasm result.
Negative changes mean lower runtime.

| Example | Original ns/frame | Current ns/frame | Runtime change |
| --- | ---: | ---: | ---: |
| basic/dual_fm_osc | 94.264 | 91.737 | -2.7% |
| basic/polyphonic_saw | 92.608 | 91.903 | -0.8% |
| basic/saw_filter_saturator | 63.325 | 61.777 | -2.4% |
| basic/sine | 3.191 | 3.204 | +0.4% |
| basic/voice_bank | 1.949 | 1.984 | +1.8% |
| effects/compressor | 15.147 | 14.607 | -3.6% |
| effects/fdn_reverb | 23.519 | 20.484 | -12.9% |
| effects/live_tape_looper | 25.772 | 22.725 | -11.8% |
| effects/schroeder_reverb | 23.578 | 20.313 | -13.9% |
| effects/shimmer_echo | 43.262 | 39.442 | -8.8% |
| effects/stereo_chorus | 23.291 | 16.812 | -27.8% |
| effects/tape_echo | 36.659 | 30.444 | -17.0% |
| effects/wavefolder | 7.415 | 5.992 | -19.2% |
| feedback/benjolin | 50.442 | 47.397 | -6.0% |
| feedback/chaotic_delay_matrix | 69.599 | 67.841 | -2.5% |
| feedback/cybernetic_feedback_graph | 332.108 | 333.289 | +0.4% |
| feedback/diffuse_delay_matrix | 59.992 | 53.423 | -10.9% |
| feedback/neural_synth | 530.575 | 512.703 | -3.4% |
| feedback/resonant_delay_matrix | 43.851 | 39.686 | -9.5% |
| instruments/acid_bassline | 78.817 | 66.630 | -15.5% |
| instruments/additive_synth | 34.642 | 33.747 | -2.6% |
| instruments/drum_machine | 23.796 | 23.202 | -2.5% |
| instruments/fm_bells | 123.032 | 122.808 | -0.2% |
| instruments/formant_percussion | 117.025 | 117.798 | +0.7% |
| instruments/karplus_strong | 20.635 | 19.726 | -4.4% |
| plugins/effects/orbit_flanger | 19.326 | 19.480 | +0.8% |
| plugins/effects/reactive_wavefolder | 87.825 | 83.674 | -4.7% |
| plugins/effects/tempo_ping_pong | 12.551 | 12.614 | +0.5% |
| plugins/effects/transient_sculptor | 11.406 | 11.757 | +3.1% |
| plugins/instruments/fm_bells | 9.734 | 8.897 | -8.6% |
| plugins/instruments/poly_saw | 42.177 | 42.464 | +0.7% |
| soundscapes/aurora_pad | 20.444 | 20.300 | -0.7% |
| soundscapes/deep_space_drone | 110.797 | 111.160 | +0.3% |
| soundscapes/glass_garden | 106.041 | 105.106 | -0.9% |
| soundscapes/granular_cloud | 98.979 | 100.129 | +1.2% |
| soundscapes/orbital_fm | 38.339 | 38.463 | +0.3% |
| soundscapes/wind_chimes | 145.856 | 146.413 | +0.4% |
| spectral/spectral_delay | 34.797 | 35.010 | +0.6% |
| spectral/spectral_freeze | 36.421 | 35.401 | -2.8% |

Transient sculptor remains **3.1% slower than the original compiler**, with regressions
of 3.8%, 3.1%, and 3.2% in the individual pairs. The earlier recovery claim was relative
to the intermediate eight-slot compiler, not this original baseline. Its regression
from increasing the promotion budget was recovered, but the cumulative regression is
not fully resolved. Additive synth is now 2.6% faster than the original, and acid
bassline is 15.5% faster. Tiny changes in sine and voice bank are timing noise: their
complete LLVM modules are byte-identical to the original.

All 234 startup/warmed output snapshot pairs were bit-identical, and all 117 paired
source-hash sets matched. No production code changed during this cumulative measurement.

## Residual transient-sculptor diagnosis

A focused field-ablation experiment isolates the remaining original-to-current regression
to promotion of the clamped control snapshots. The original compiler promoted no state
in this example because its eligible set exceeded the old all-or-nothing eight-slot cap.
The retained compiler promotes 24 slots: eight clamped controls, eight envelope time
parameters, and eight envelope state/output slots. Conditional attack/release coefficients
are already excluded by the earlier fix.

The saved probe worker allows promotion subsets without changing the source or math.
Its zero-promotion LLVM module is byte-identical to the original compiler's module, and
its current-subset module is byte-identical to the retained compiler's module. Thus the
ablation directly reproduces the two endpoints. Three alternating-order repetitions
(`target/performance-round3/residual-transient/`) measured:

| Promotion subset | 128-frame ns/frame | 1,024-frame ns/frame |
| --- | ---: | ---: |
| None (original IR) | 11.224 | 11.161 |
| Current 24 slots | 11.768 | 11.673 |
| Current minus eight clamped controls | 11.197 | Not measured |
| Current minus envelope time parameters | 11.769 | Not measured |
| Current minus envelope `y` state | 12.211 | Not measured |
| Current minus envelope output slots | 11.633 | Not measured |

The roughly 0.5 ns/frame difference persists with eight times as many frames per block,
which rules out block setup as its main source. Keeping envelope state promotion while
leaving clamped controls unpromoted recovers the original speed. Removing the envelope
state promotion instead makes runtime worse.

IR inspection (`residual-inspect/`) shows that the current compiler pairs envelope SIMD
lanes as `[fast_L, slow_R]` and `[fast_R, slow_L]`. The variant without control promotion
pairs them as `[fast_R, fast_L]` and `[slow_R, slow_L]`. The current grouping needs lane
swaps while combining the detectors; the alternative uses channel pairs within each
envelope rate. Both retain the two per-sample scalar exponential calls. The alternative
also hoists its control loads into the loop preheader: leaving them unpromoted does not
mean reloading them every sample. Register allocation and instruction scheduling also
change, so this experiment identifies the responsible MIR decision but does not isolate
the exact cycle cost of each SIMD shuffle or spill.

A second three-repetition ablation (`residual-controls/`) found that omitting just
`attack_db` or just `stereo_link` promotion also recovers the speed (11.193 and 11.211
ns/frame respectively, versus 11.761 for the current set). Omitting both block-only
controls (`release_s` and `slow_attack_s`) measured 11.246. These changes perturb LLVM's
packing choices; they do not establish a semantic reason to blacklist one named control.
All 72 same-block-size output comparisons across both ablations were bit-identical.

This is diagnosis and an experimental remedy, not a newly retained compiler policy.
No production code or example source was changed. A generic selection change must be
checked against the full corpus, since control caching benefits other processors.

## General MIR optimization: branch-sensitive bounds proofs

The examples are a regression corpus, not the specification of which programs deserve
optimization. This step adds a semantic proof rather than another state-promotion cost
rule. No example, processor, parameter, or field name participates in the decision.
The previously retained promotion policy is unchanged; its empirical profitability
heuristics are not a guarantee of optimal code for arbitrary user programs.

The bounds-proof walker now tracks signed integer intervals on true and false comparison
edges, joins surviving paths, and carries value-preserving integer copies/casts. It
invalidates facts on operand mutation, clears them across calls and indirect slice writes,
and starts loops from whole-function facts to avoid using first-iteration assumptions on
backedges. Checked or clamped accesses become unchecked only where the complete index
interval is proven valid. Floating-point comparisons do not supply these proofs.

The reusable `bounds-guarded-` benchmark family in `scripts/bench/workloads.py` covers
lengths 8, 63, and 256, i32/f32 and i64/f64 arithmetic, and valid accesses on either true
or false edges. Dynamic input indices cross both array boundaries. Every kernel loses
three redundant MIR bounds checks. The i64 cases also test propagation through the i32
cast used by array addressing. This matrix supplements the applications with code shapes
they do not currently contain.

Three alternating-order pairs per kernel and backend used the same pinned CPU, strict
math, 128-frame blocks, and nine 50-ms rounds as the earlier measurements. Artifacts are
in `target/performance-round4/`: `matrix-before/`, `matrix-after/`, and `kernel-matrix/`.
The baseline is the previous `guarded` worker; the candidate is `branch-ranges`.

**Native:** all twelve relocatable objects are byte-identical. LLVM already proves these
bounds; the −0.23% observed timing aggregate is noise, not an improvement.

**Wasm:** geometric-mean runtime decreased by **9.04%** across the twelve synthetic
kernels. These are pattern-specific measurements, not a claim about the example corpus
or all Onda programs.

| Index / arithmetic | Array length | Valid edge | Wasm runtime change |
| --- | ---: | --- | ---: |
| i32 / f32 | 8 | true | -12.35% |
| i32 / f32 | 8 | false | -3.25% |
| i64 / f64 | 8 | true | -8.62% |
| i64 / f64 | 8 | false | -3.32% |
| i32 / f32 | 63 | true | -16.35% |
| i32 / f32 | 63 | false | -3.91% |
| i64 / f64 | 63 | true | -13.15% |
| i64 / f64 | 63 | false | -5.88% |
| i32 / f32 | 256 | true | -13.48% |
| i32 / f32 | 256 | false | -10.02% |
| i64 / f64 | 256 | true | -8.33% |
| i64 / f64 | 256 | false | -8.64% |

All 144 startup/warmed output comparisons are bit-identical. All 54 application examples
have byte-identical MIR and LLVM IR before and after this step, so the earlier native
corpus results and the residual transient-sculptor regression remain unchanged.

Validation includes 135 MIR tests (with comparison soundness checks covering both signed
widths and extreme values), 159 LLVM tests, 1,090 semantic tests, 826 example tests (one
ignored), and the compiler doctest. An end-to-end native test at O0 and O3 checks a guarded
dynamic access followed by index mutation and an access that must retain clamped behavior.
Clippy, formatting, and benchmark-harness checks are recorded alongside the timings.

This is a portable bounds optimization, not a new vectorizer or a memory-layout change.
Further SIMD/locality work should expose proven independence and useful memory facts to
the backends, and use target costs for profitability. A higher static vector count or a
favorable instruction-packing change in one application is insufficient evidence.


## Loop induction and native range facts

The next generic step extends MIR branch intervals to loop headers. A bounded abstract
interpretation joins entry and backedge facts, widens expanding endpoints, and accepts
an invariant only after checking that every backedge is contained in it. Both normal
fallthrough and `continue` contribute; `break` and `return` do not. Eight iterations bound
analysis cost, with a conservative whole-function fallback. Nested-loop writes and
potential alias writes invalidate facts; incoming facts about unmodified locals survive.
Derived integer indices reuse the existing overflow-aware arithmetic transfer functions.
This supports forward/reverse and strided loops without specializing for processor names.

The native backend now exports each trusted fixed-array unchecked index as an unsigned
`index < length` predicate through `llvm.assume`. This retains the logical array bound
when MIR removes a clamp; an allocation-wide `inbounds` GEP alone does not express that
bound. Dynamic slice/buffer indexing is unchanged. The assumption relies on the existing
trusted unchecked-index contract, including explicit unsafe accesses. LLVM defines the
intrinsic's semantics in its [language reference](https://releases.llvm.org/21.1.0/docs/LangRef.html#llvm-assume-intrinsic).

The new `bounds-loop-` family contains 16 kernels: lengths 63/256, i32/f32 and i64/f64,
strides 1/3, and forward/reverse traversal. Counts vary with input and cross both validity
boundaries; indices include a derived array offset. All sixteen lose three redundant MIR
clamps. Measurements use three alternating-order pairs per backend, nine 50-ms rounds,
512 warmup blocks, 128-frame blocks, strict math, and the same pinned CPU as earlier work.
Artifacts are in `target/performance-round5/`.

| Measurement | Result |
| --- | --- |
| Wasm, 16 loop kernels | **26.06% lower geometric-mean runtime**; individual improvements 18.70–34.06% |
| Native, i64/f64 length 63, forward stride 3 | **24.01% lower runtime**, 2.582 → 1.962 ns/frame |
| Native, i64/f64 length 256, forward stride 3 | **24.20% lower runtime**, 7.315 → 5.544 ns/frame |

The Wasm measurements compare the MIR-only candidate against the preceding compiler;
adding the native assumption does not change that MIR or the Wasm backend. In the two
native strided cases LLVM changes the processing loop from scalar arithmetic to
`<4 x double>` multiplication/addition, using an interleaved vector load and scalar stores.
The optimized processing IR loses the clamp intrinsics. This is evidence of actual new
vectorization, rather than counting SIMD used during initialization.

The native matrix's observed aggregate is −2.41%, but tiny contiguous kernels vary even
when their emitted objects are byte-identical. In particular the apparent +11.44% for the
length-256 i32/f32 forward unit-stride kernel has identical native object bytes. The
length-256 i64/f64 unit-stride case changes code and initially measured +5.31%, requiring
separate confirmation. These results do not establish a uniform native gain. Seven of
sixteen native objects are unchanged; nine change, including some initialization-only
changes. No floating-point reassociation or fast-math option was introduced.

The first matrix has 192 bit-identical startup/warmed output comparisons across native
and Wasm; the native-assumption matrix adds 96. Source fingerprints match. Outputs are
f32 even for the f64 kernels, so this is output parity, not a whole-private-state identity
claim. Native correctness tests separately exercise negative/oversized counts, forward
and reverse loops, both integer widths, derived offsets, and O0/O3 execution. Abstract
transfer tests cover wrapping arithmetic at signed extrema and iteration-budget fallback.

All 54 example sources compile to optimized MIR, LLVM IR, and native objects. Compared
with the previous branch-proof checkpoint, ten native objects change. MIR differences
are limited to bounds removal in voice-bank/additive-synth parameter normalization loops;
LLVM already eliminated their checks. The wider native differences come from retaining
logical index bounds on existing unchecked accesses, including explicit unsafe indexing.
Thus this pass primarily broadens optimization coverage for user-written loops, rather
than promising another large corpus-wide gain.

Validation: 142 MIR tests, 161 LLVM tests, 1,090 semantic tests, 826 example tests
(one ignored), and the compiler doctest passed. Compiler Clippy with warnings denied,
formatting, diff checks, and the 12-test benchmark-harness suite passed (two integration
tests skipped). Current and preceding corpus inspections are retained in
`performance-round5/corpus-assume/` and `performance-round4/final-corpus-inspect/` under
`target/`; matrix timings, parity records, and individual rounds are retained alongside them.

The 39-example native sweep completed three alternating-order pairs per example, with
all **234 startup/warmed output comparisons bit-identical** and source fingerprints
unchanged (`examples-timings/`). Its median-based geometric-mean change was **−0.36%**;
the three individual paired sweeps were +0.20%, −0.01%, and −0.12%. This does not establish
an application-wide speedup. The disk initially filled during sweep two; after cleanup,
completed runs were retained and the incomplete run was repeated. Other applications
were consuming substantial CPU during later sweeps, and timings of identical emitted
objects also varied materially. Apparent changes such as transient-sculptor −4.91% and
voice-bank +7.72% therefore do not demonstrate a fix or regression: both objects are
unchanged from this round's baseline. The earlier original-to-current 5.0% result remains
a historical checkpoint; multiplying it by this noisy incremental aggregate would not
be a new cumulative measurement.

A final five-pair check used nine 100-ms rounds for the length-256 contiguous f32/f64
kernels and the strided f64 kernel (`confirmation-timings/`). All 30 output comparisons
were bit-identical. The strided kernel improved in every pair (17.63–24.15%). The
contiguous kernels swung in both directions, including −20.77% to +79.18% for the
byte-identical f32 control. This run cannot resolve the contiguous f64 regression risk;
a quiet-machine confirmation is still needed before assigning that case a performance
verdict. Its initial +5.31% is not an established regression, and the later aggregate is
not evidence that it was fixed. This limitation does not affect the compile/IR findings
or correctness checks above.


## Portable dynamic slice facts and vector math investigation

This round uses a fresh saved compiler (`target/performance-round6/benchmark_dsp-before`)
from immediately before these changes. The user raised CPU clock settings before this
round; all comparisons below rerun both workers under the new settings. They do not
compare new timings against older-clock samples. CPU affinity remains pinned, but frequency
is not locked: the recorded policy has an 800 MHz minimum and 5.6 GHz maximum, with the
powersave governor. System activity remains a possible source of variance.

### Portable slice bounds

MIR now tracks direct relationships between an index and the length of the same dynamic
slice descriptor. A nonnegative index below that length permits unchecked loads, stores,
and scalar slice-element call arguments. Value-preserving copies carry the relationships;
normalizing or narrowing copies that may change a value do not. Branch joins intersect
facts, and the loop-header fixed point includes relationships as well as intervals.

Indirect stores reuse the existing slice reference-provenance analysis. They invalidate
facts for possibly aliased locals while retaining facts about disjoint local storage.
Unknown origins and calls remain conservative. Escaping a descriptor through a reference
argument also invalidates its provenance summary. Tests cover aliased scalar mutation,
value copies, descriptor/index/length replacement, joins, loop backedges, empty slices,
and ordered updates/reductions at both integer widths.

In the 16 `slice-loop-*` kernels, unit-stride loops lose all redundant slice clamps.
Stride-three i32 induction can overflow for a general descriptor length, so its loop
accesses retain checks; the guarded final element-zero read is still optimized. The pass
neither assumes a small bound from this benchmark's caller nor silently changes integer
overflow semantics. All sixteen native objects are byte-identical: LLVM already removes
these checks. All 54 strict-math example MIR/LLVM/object artifacts are also unchanged.
This is broader portable proof coverage, not a native application speedup claim.

### Measurements and platform coverage

Three alternating-order baseline/candidate pairs used nine 100-ms rounds per case,
128-frame blocks, 512 warmup blocks, and CPU affinity. Geometric means of the per-case
median runtime ratios were:

| Dynamic slice workloads | Cases | Wasm runtime change |
| --- | ---: | ---: |
| Unit stride, f32/f64, reduction/update, lengths 63/256 | 8 | **−35.09%** |
| Stride three, same widths/operations/lengths | 8 | −0.71% (no established gain) |
| Complete matrix | 16 | **−19.72%** |

Every startup and warmed output comparison was bit-identical (96 comparisons over 48
paired runs). The measurements are in `target/performance-round6/timings/`; its native
results concern the discarded experiment below and must not be attributed to the retained
compiler. Final `slices-portable/` artifacts match all 48 measured candidate Wasm binaries and
MIR hashes. Removing the native-only experiment does not change Wasm generation.
`portable-artifact-comparison.json` records this check, byte-identical native objects for
all 16 slice kernels, and unchanged MIR/LLVM/object artifacts for all 54 examples under
both strict and fast math.

The retained optimization changes shared MIR and introduces no platform-specific runtime
library. The slice regression test checks empty views, ordered updates, and i32/i64 indices,
executes at O0/O3 on the host, and emits ELF for Linux x64, COFF for Windows x64, and Mach-O
for macOS arm64. Cross-emission verifies lowering and object format; it is **not execution
or a performance measurement on Windows or macOS**. The Wasm timings are from V8 on Linux.
Native LLVM already proves these kernel bounds, so the current evidence supports a Wasm
speedup and broader shared proof coverage, without an additional native speedup claim.

### Vector math experiment (not retained)

A Linux x86-64 prototype linked glibc's vector sin/cos/exp/log implementations into Onda
code under existing fast math. Its eight independent-array kernels ran approximately
62–81% faster than the preceding compiler. Sampled differences were 1–3 ULP; those samples
do not establish full-domain accuracy. Tan/tanh controls did not change. This demonstrates
an opportunity, not a portable compiler result or an application-wide speedup.

The first integration exposed vector variants to LLVM before O3. It regressed deep-space
drone by **57.72%** in the three-pair application check. A reduced five-voice oscillator/
filter bank reproduced the loss of masked SIMD. Copying intrinsic function attributes did
not resolve it. Replacing only already-vectorized math intrinsics **after** O3 preserved
the existing loop/tail structure; a focused fresh before/after drone rerun was 59.78 versus
59.44 ns/frame (ratio 0.994). This is evidence that the regression was avoided, not a robust
claim of a 0.6% improvement. The precise internal LLVM profitability decision was not proven.
The stateful-loop SIMD regression test remains in the compiler suite.

The Linux-only implementation and its runtime dependency have been removed from production
code. The late-lowering prototype source, test source, and worker are preserved in
`target/performance-round6/linux-vector-experiment/`; initial measurements remain under
`timings/`, `selected-timings/`, and `vector-math/`. Do not treat their libmvec speedups as
features of the final compiler. The processor-object linking contract is unchanged.

### Next native step: bundled portable vector math

Onda releases Linux x64, Windows x64, and macOS arm64 together. A vector math implementation
must cover those targets, including JIT execution and standalone exported objects.
[SLEEF's supported-platform matrix](https://github.com/shibatch/sleef#supported-environment)
covers these OS/architecture combinations and makes it a grounded candidate. Library
support alone does not establish a working Onda integration or equal speedups on each OS.

The integration should:

- Pin and bundle the implementation, with reproducible builds and license attribution,
  so users do not install a different optional system math library on each OS.
- Select supported x86 SIMD or ARM AdvSIMD from the **compilation target**, and respect
  each platform's calling convention. The JIT must resolve the shipped implementation;
  exported objects must include it or provide an explicit portable linking contract.
- Retain fast-math opt-in and verify both widths over large arguments, range-reduction
  boundaries, underflow/overflow, and the chosen exceptional-value policy. The present
  moderate-range kernel checks are insufficient for a numerical implementation change.
- Preserve loop/tail SIMD, with the five-voice regression test and actual assembly checks.
  Prefer lowering the vector intrinsics already selected by LLVM before changing its
  vectorization cost model. Exact ISA widths and scalar tails need separate coverage.
- Measure the same paired kernel and application corpus on Linux, Windows, and Apple
  Silicon hardware. Cross-compilation is useful coverage but cannot supply those timings.

This work remains open in `docs/todo/backends.md`. No portable vector-math implementation
has been added in this round. Ordinary tan/tanh errno effects and sequential DSP recurrences
also remain separate obstacles; a library choice alone does not remove them.

### Validation of the retained compiler

Final validation passed 147 MIR tests, 163 LLVM tests (including cross-target slice object
emission), 826 example tests with one ignored, all 12 benchmark-harness tests including the
bounded native/Wasm integration tests, clippy with warnings denied, and formatting checks.
An existing harness timeout test raced the kernel's delivery of SIGKILL to a descendant;
its assertion now polls for at most one second instead of requiring immediate termination
when only the parent has been waited for. The initial failure and successful rerun are
retained in `portable-harness-tests*.log`.

## Bundled SLEEF vector math
The next round replaces the Linux-specific runtime experiment with bundled SLEEF 3.9.0
bitcode. The source archive is pinned by commit and SHA-256. Clang 21.1.8 builds the
upstream SSE2, AVX2/FMA, and ARM AdvSIMD implementations, and LLVM 21.1.2 emits the checked-in
bitcode. The generic vector-extension backend did not build for f64 in the releases tried;
no repairs or changes to SLEEF's numerical algorithms were made. The ISA implementations
compile from unmodified upstream sources.

The compiler's ordinary O2/O3 pipeline runs first. Under existing **fast math**, vector
sin/cos/exp/log intrinsics are then replaced with bundled helper calls. It selects the ISA
from the compilation target, respects explicit feature disables, requires affirmative
AVX/AVX2/FMA before using the AVX2 implementation, pads small vectors with 1.0, and splits
wide vectors. Helper arithmetic keeps SLEEF's FP rules and contraction policy; only unused
functions are eliminated after linking. LLVM's original application loop and tail decisions
are retained. Scalar math and other functions, including tan/tanh, keep their existing paths.

Helpers are internal to the generated module, so both JIT code and exported objects include
the implementation. There is no separate SLEEF DLL/shared library or additional linker flag.
This covers Linux/Windows x64 and macOS arm64 code generation. The six embedded modules total
approximately 170 KiB before the compiler links and prunes the functions needed by a program.
The regenerator, checksums, and integration details are in
[`crates/onda_codegen_llvm/math-kernel/`](../crates/onda_codegen_llvm/math-kernel/README.md).
Release packaging includes the SLEEF license on all three platforms.

### Native measurements

Both workers were rerun under the current CPU settings, pinned to CPU 0, with 128-frame
blocks, 512 warmup blocks, nine 100-ms rounds, and three alternating-order pairs. These
results use the same fast-math setting on both workers. The baseline is the retained
portable compiler from round six, not the discarded libmvec candidate.

| Operation | f32 runtime change | f64 runtime change |
| --- | ---: | ---: |
| sin | -61.17% | -59.58% |
| cos | -56.18% | -54.98% |
| exp | -72.34% | -33.14% |
| log | -49.66% | -10.43% |

The geometric mean runtime reduction across those eight kernels is **52.52%**. The four
tan/tanh controls have unchanged objects and measured changes between −0.42% and +0.18%.
These timings demonstrate independent-array math throughput; they do not imply that scalar
oscillators or arbitrary processors become twice as fast.

| Application | Runtime change |
| --- | ---: |
| effects/fdn_reverb | -0.94% |
| instruments/additive_synth | -0.71% |
| soundscapes/deep_space_drone | +0.09% |
| basic/sine | -0.15% |

All four application changes are within about 1%; no additional application speedup is
established. Deep-space drone's +0.09% does not reproduce the early mapping regression.
Across all 54 checked-in example sources, strict-math MIR/LLVM/object artifacts are
byte-identical to round six. Under fast math, only additive synth and deep-space drone
objects change. All MIR is unchanged. These application examples have little independent
vector transcendental work in their hot paths; more general vector-math workloads are
represented by the separate kernels.

### Correctness and portability checks

- All 96 startup/warmed comparisons across the paired kernel/application runs pass the
  harness's absolute/relative tolerance. The eight math kernels differ from scalar output
  by at most one ULP in their sampled moderate input range. Tan/tanh outputs are bit-identical.
- Additional native differential tests cover both precisions, 257-element loops and scalar
  tails, inputs distributed across the finite exponent range, maximum finite trigonometric
  arguments, minimum-normal log inputs, zero/one/pi, and finite exp boundary inputs. The
  tests allow up to four ULP; this is sample coverage, not a proof of a full-domain bound.
  Fast-math NaN/infinity and signed-zero guarantees are not strengthened by this integration.
- Direct JIT tests execute two-, three-, and sixteen-lane vectors through scalar-pointer
  wrappers, checking padding, splitting, and both f32/f64 helper results without depending
  on a particular vectorizer heuristic. The five-voice masked-SIMD regression remains green.
- All eight operation/precision combinations emit Linux ELF, Windows COFF, and macOS ARM64
  Mach-O objects with internal helper definitions. Linux execution is tested here;
  **Windows and macOS execution and performance still need their native runners**.
- All twelve final kernel objects link into an ordinary C executable with `-lm -ldl` and
  no SLEEF/libmvec library, run through the raw processor ABI, and match the expected outputs.
- Regeneration in fresh temporary directories reproduces the manifest and every bitcode
  file byte-for-byte. The generator rejects non-LLVM imports, embedded assembly, unexpected
  exports, and retained generating-host CPU attributes. Normal Cargo builds require no
  regeneration toolchain or network access.

Final validation passes 167 LLVM tests, 826 example tests (one ignored), the native doctest,
clippy with warnings denied, and formatting/diff checks. Measurements, command logs,
output snapshots, AOT smoke checks, numerical-error summaries, and artifact hash comparisons
are retained under `target/performance-round7/`. The final math objects are byte-identical
to those emitted by the measured candidate; the later soft-float exclusion does not change
the measured host configuration. Compile-time and code-size effects have not been treated
as runtime gains.

## Rounds eight and nine: closing the validation gaps

### Generic fix for the residual promotion regression

The residual transient-sculptor regression reproduced under the current machine settings:
three original/pre-fix pairs were +3.83%, +4.39%, and +5.37%, with bit-identical outputs.
The retained refinement distinguishes direct invariant loads from borrowed references.
When all persistent state is scalar and the process has no indirect writes, direct
loop-invariant slots stay in memory. LLVM can distinguish these fixed addresses from
loop stores, hoist the loads, and choose better SIMD groupings. Loop-mutated state and
profitable reference operands retain caching. Aggregate state or descriptor-based writes
keep the previous direct-read caching policy because their lowered memory dependencies
can prevent hoisting. Block-only state also retains caching for block preparation.

This is a conservative profitability heuristic, not a new alias proof. It uses MIR types,
transitive effects, and access depths; there are no source-name or processor-specific
exceptions. A source-to-MIR regression test checks both scalar-only and aggregate-state
variants, while existing tests retain coverage for conditional helper reads, nested loops,
aliasing, state snapshots, block hooks, and events between segments.

The unrestricted alternatives were measured and rejected:

- Excluding all block-only fields recovered transient sculptor but made FM-bells idle
  processing consistently slower (+4.80%, +5.08%, +6.15%). Its eight-lane parameter clamp
  fragmented into smaller groups, and process instructions grew from 810 to 847 with
  eight additional memory accesses. Its 39-example cumulative runtime reduction was 4.94%,
  but that did not justify introducing the new regression. Its partial Wasm sweep is
  diagnostic evidence only.
- Prioritizing loop-used fields in the budget preserved FM bells but did not fix transient
  sculptor (+0.04% against the pre-fix compiler over five pairs).
- Leaving all direct invariants in memory recovered transient sculptor (about 3.4%) but
  regressed acid bassline by 23.3% and orbit flanger by 10.1%. IR inspection shows orbit's
  control loads remaining inside the sample loop, whereas transient's controls hoist into
  the preheader. This motivated the aggregate/indirect-memory guard.

The final guard preserves the exact pre-fix objects for FM bells, acid bassline, and orbit
flanger. Ten of 54 host objects change; nine are executable scalar-port cases and the other
is the external-buffer sample player. Transient's final object is identical to the recovering
invariant-load experiment. Rejected experiments are recorded in `target/performance-round8/`;
final compiler, artifacts, tests, and measurements are in `target/performance-round9/`.

### Final cross-target IR and object inspection

The harness accepts `--target-triple` for native inspection only. Both its CLI and worker
reject cross-target execution. With generic CPUs, all 54 sources compile in strict and
fast-math modes for Linux x64, Windows x64, and macOS ARM64, plus twelve fast-math kernels
per target: **360 verified LLVM modules and emitted/disassembled objects**. MIR is identical
across targets. Linux is included as a generic-CPU control rather than comparing Windows
SSE2 directly with the benchmark machine's AVX2 code.

| Generic target | Examples with vector FP arithmetic, strict / fast | Supported math kernels using SIMD helpers |
| --- | ---: | ---: |
| Linux x64 | 36 / 40 | 8 / 8, SSE2 |
| Windows x64 | 36 / 40 | 8 / 8, SSE2 |
| macOS ARM64 | 21 / 23 | 8 / 8, AdvSIMD |

Counts refer to the process function's explicit vector FP arithmetic/comparisons, not
initialization stores or arithmetic inside helpers. They describe code shape, not execution
speed or the fraction of hot work vectorized. The eight sin/cos/exp/log kernels have internal
helper definitions and no unresolved SLEEF/libmvec imports. Disassembly confirms packed SSE2
and AdvSIMD arithmetic. Scalar tan/tanh controls keep the existing path. Generic ARM64 sometimes
chooses two f32 lanes that are padded into a four-lane helper, and its cost model leaves more
stateful example arithmetic scalar than x64. These are remaining tuning opportunities, not
evidence of equivalent runtime throughput across architectures.

Windows helper calls still use the platform's indirect vector-argument convention and
preserve more vector registers than Linux. A saved IR-only `fastcc` experiment did not remove
that overhead and was not adopted. Investigate late inlining or a private helper ABI with
native Windows measurements before changing that policy. Native Windows/macOS execution
and performance remain deliberately deferred; this round closes the requested inspection
coverage, not those runtime claims.

Final compiler validation passes 167 LLVM, 147 MIR, 1,091 semantics and 826 example tests
(one example ignored), the native doctest, clippy with warnings denied, and formatting.
All 13 harness tests passed, including cross-target rejection and emission. Identical
immutable inspection artifacts share storage to keep the disk footprint bounded.
