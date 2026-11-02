# Final processor-storage integration and merge-readiness pass — 2026-11-02

The branch is merge-ready on the evidence collected here: no outstanding review finding,
no failing compiler/backend test, and no confirmed generated-DSP performance regression.
Four correctness gaps found during this pass were fixed before the final measurements.
This is a Linux verification result, not a claim that every possible program or platform is proven.

The final branch is `compiler/structured-proc-state-final`, based directly on
`ci/gpu-integration` at `5da474af7797d2e2d63757f741ae1c83adfe0626`.
Only the processor implementation/evidence commits were cherry-picked onto that target;
the earlier workspace snapshot commit was excluded. The compiler measured here is
`bb7c65caed391e0261004bc6a8a3b86eba46818a`; `cb05011` subsequently adjusts test fixtures only.
The target branch and the original user checkout were preserved.

[Machine-readable evidence](2026-11-02-processor-final.json) contains worker hashes, sources,
individual results, comparisons, compiler phases, review findings, and exact driver scripts.
Generated MIR, LLVM IR, objects, disassembly, remarks and logs remain under
`target/proc-storage-evidence/final-2026-11-02/`.
Earlier [stress results](2026-11-02-processor-stress.md) remain historical measurements against
older baselines; this report uses a freshly built current merge-target baseline and matching stdlib.

## Standards

Three findings were resolved:

- **P1:** overflowing interface buffer spans could pass validation because saturating arithmetic
  created an empty range. Checked addition now rejects overflow.
- **P1:** local buffer-element call arguments skipped i32 selector validation. Calls now use the
  shared reference validator; both f32 and bool selectors are rejected.
- **P3:** the MIR reference still described the former callback-view restriction. It now documents
  complete captures, deferred resource recipes and symbolic nested selections.

Both validator cases had failing regressions before the fixes. The final independent Standards
review found no outstanding actionable finding. All authored sources remain below 5,000 lines;
the largest is 4,917 lines. The new capture module has one responsibility, and view discovery
is read-only rather than emitting descriptors as a side effect.

## Spec

Two P1 findings were resolved:

- **Joined child resources:** member discovery created a descriptor inside a branch, then lost it
  at the join. Child resources now retain typed declaration recipes and enclosing frames.
  The selected operation resolves the descriptor against captured coordinates and current bindings.
- **Nested storage/resource joins:** an inner same-shape join could create a descriptor that an
  outer union forwarded without initialization on its inactive path. Captures now retain symbolic
  choices until forwarding/use. Every nested tag is initialized on both paths; legalization then
  forms ordinary MIR references from actual source capabilities.

Both cases had failing native regressions before their fixes. Coverage includes LLVM O0/O3,
resource-only views, callbacks, equal/different extents, different owner resource types/counts,
nested storage/resource joins, deep collection windows and snapshot restore after rebinding.
Retention traverses choice tags and symbolic leaves. The physical helper ABI is unchanged, and
selection does not expand by processor-array capacity or construct descriptor tables in processing.

The final independent Spec review found no outstanding actionable finding or scope creep.
Findings: **Standards 3 resolved / 0 outstanding; Spec 2 resolved / 0 outstanding**.
The worst original finding on each axis was P1.

## Final checks

| Check | Result |
| --- | --- |
| Rust workspace | 3,657 passed, 0 failed; four normally ignored tests |
| Manual oversampling budget | Passed: 1.369× runtime, budget 2.5× |
| Remaining ignored tests | Three display-dependent native UI tests |
| Focused final processor-view suite | 16 passed, including the final fixture adjustment |
| Strict workspace Clippy, all targets | Passed with `-D warnings` |
| Formatting, diff whitespace, source-size limit | Passed |
| Browser/package tests | 362 passed |
| Browser catalog and packaged projects | 54 catalog projects + 3 asset projects passed |
| Source → schema-9 MIR → valid Wasm corpus | 68/68 fixtures, zero exclusions |
| Native LLVM/Binaryen differential scenarios | 62 scenarios × optimized/unoptimized configurations |
| Differential outputs | 4,240 bit-exact samples; 4,096 approximate samples, max error 1.192e-7 |
| Exact FMA oracle | 20,020 vectors checked |
| Benchmark harness, including bounded integration/replay | 11 passed |
| Versions, API/stdlib docs, SLEEF/reference provenance, C ABI header | Passed |

Native inspection covered **159 sources: 105 builtin probes and all 54 checked-in `.onda`
example files**. All emitted valid MIR and LLVM. The 15 sources outside the runtime harness's
scalar-port/no-external-buffer contract were also inspected with the baseline compiler; all passed.
External buffers, events, snapshots and segments receive execution coverage through the workspace
and backend differential suites. The native mixed-type resource test uses f64 interleaving as well
as f32 resources; its Wasm parity fixture uses the daemon's supported f32 asset interface.

## Generated-code runtime results

Measurements ran sequentially after builds/tests finished, pinned to CPU 0 on an Intel Core
i9-14900HX, using LLVM 21.1.2, strict floating point, 48 kHz and 128-frame blocks. Both workers
were release builds with the same embedded stdlib. All 140 shared-source comparisons had matching
source fingerprints and settings. The broad pass uses five 50 ms rounds, startup and warmed
snapshots, and 512 latency blocks. Longer follow-ups use three independent trials of seven
150 ms rounds and 2,048 latency blocks, with worker order alternated.

The final compiler ran **144/144 eligible programs**. The baseline ran 140; its four failures were
`proc-bank-1024` (the old 512-leaf aggregate cap) and callback/helper banks at 16/128/1,024 slots
(the old view-specialization limitation). These are baseline capability failures, with successful
final execution.

Across the 140 paired programs, **136 matched bit for bit over 495,616 startup/warmed samples**.
The four FFT diagnostic programs separately passed their existing bounded-error contracts.
There were no output mismatches. LLVM IR was byte-identical for 125 pairs and MIR for 118 pairs.
The broad geometric-mean candidate/baseline runtime ratio was **0.9815**; this equal-workload
summary is descriptive, not an application-wide speedup promise.

Repeated medians establish the processor-bank gains:

| Workload | Baseline ns/frame | Final ns/frame | Speedup |
| --- | ---: | ---: | ---: |
| Processor bank, 16 slots | 13.590 | 3.251 | **4.18×** |
| Processor bank, 128 slots | 188.638 | 38.229 | **4.93×** |
| Active FM-bells plugin | 116.215 | 116.350 | Essentially unchanged |
| Active poly-saw plugin | 227.696 | 226.083 | Essentially unchanged |

The two plugin examples are silent without note events. Derived variants activate every voice
using their checked-in note-on bodies; both produced nonzero output and matched the baseline bit
for bit. Their generated `onda_process` instruction bytes were identical.

### Initial timing outliers

Three broad-pass timings were initially slower: delay-96000 (+5.2%), basic voice-bank (+9.7%) and
spectral-freeze (+6.0%). **All three produced byte-identical native objects.** Their longer
candidate/baseline medians were respectively **1.007, 0.923 and 0.967**. The ordering reversed for
some trials; these measurements do not establish a generated-code regression.

A tiny passthrough control measured +7.4% in the separate-worker follow-up despite an identical
object. Replaying both MIR versions through the same final worker reduced that difference to
+0.3%. Seven controls/outliers were replayed in three trials: **all 21 object pairs were identical**,
and all outputs matched. This isolates the emitted DSP from worker/JIT placement and timing
variation. LLVM textual differences in the voice-bank case did not change its object code.

There were **72 longer native runs and 42 same-worker MIR replays**, adding 98,304 and 49,152
bit-exact sample comparisons respectively. No confirmed generated-DSP slowdown remained.

## MIR, alignment and larger-capacity stress

| Family | Capacities checked | MIR functions | State tensors | Largest live function signature |
| --- | --- | ---: | ---: | ---: |
| Processor bank | 16, 128, 1,024, 16,384 | 8 | 5 | 5 parameters |
| Forwarded callback bank | 16, 128, 1,024, 16,384 | 13 | 8 | 6 parameters |

Tensor count and helper width stay constant while actual storage grows with capacity.
At 128 slots, `onda_process` fell from **1,892 scalar instruction lines to 311 instruction lines**,
including 160 vector instruction lines. Saved pre-optimization IR, `opt` remarks and disassembly
confirm eight-wide processing and the 64-byte aligned state base. The ordered floating-point sum
remains unvectorized when reassociation is unsafe. No new indirect call or allocator call appears
in these bank/callback process bodies. Writable views retain overlapping origins rather than
acquiring an exclusivity promise.

Two additional **16,384-slot** programs compiled and ran successfully and matched ordinary-array
numerical references bit for bit over another 4,096 samples. Their observed runtime was 9.389 μs
per audio frame for the bank and 17.617 μs for the callback/helper bank. These tests extend the
capacity check well beyond the original cap without growing function/tensor counts.

There is still measured optimization headroom. The smaller numerical references ran at about
8.48 μs/frame: they carry fewer cached states and omit callback dispatch. The full bank was 10.7%
slower than its reference, and the callback bank 2.08× slower than its reference. These are
comparisons of different implementations on the final compiler; the old compiler cannot compile
this processor capacity. They do not justify claiming zero processor-abstraction overhead.

In the nested callback case, two routing tensors are initialized but never read by process/helpers;
callbacks use captured coordinates directly. They occupy **131,072 bytes** of state/snapshot
storage at 16,384 slots. The generator remains necessary for top-level delegate subscriptions.
The plan explicitly preserves the existing state ABI, so selective pruning is a future optimization,
not a missing correctness requirement. Cached output stores and conservative callback-loop
vectorization are further opportunities exposed by the current MIR model.

## Compiler time, separately from DSP runtime

The broad pass performs one fresh compilation per worker/source. Across 140 pairs, the geometric
ratios were:

| Compiler phase | Final/baseline | Observation |
| --- | ---: | --- |
| Frontend | 1.0382 | About 3.8% more frontend work |
| LLVM/JIT | 0.9440 | About 5.6% less JIT work |
| Frontend + JIT | 0.9972 | Roughly unchanged overall |

These broad single-compilation statistics are not a universal compiler-time non-regression claim.
Repeated selected workloads show substantial scaling gains:

| Workload | Baseline total compile ms | Final total compile ms |
| --- | ---: | ---: |
| Processor bank, 16 slots | 55.491 | 33.106 |
| Processor bank, 128 slots | 1,238.937 | 32.578 |
| Active FM-bells plugin | 499.810 | 374.517 |
| Active poly-saw plugin | 509.910 | 375.670 |

The 128-slot bank's **38× compile-time improvement** and **4.93× DSP-runtime improvement** are
separate measurements. Some frontend work costs more; native compilation and generated bank code
benefit from the compact representation.

## Scope and reproduction

The native runtime harness covers scalar ports and deterministic startup/warmed blocks. External
buffers and interactive events have separate correctness coverage, not an exhaustive performance
matrix. Display-dependent UI tests and macOS/Windows execution were not performed in this Linux
pass. Cross-target backend tests included in the workspace did pass.

The JSON embeds exact drivers and source fixtures; immutable workers and all generated artifacts
are retained locally. Standard per-case reproduction follows `scripts/bench/README.md`, selecting
`--runner` from the recorded worker directories. Broad and longer drivers preserve every child
command, stdout/stderr and failed baseline attempt. Checksums distinguish the fresh baseline
worker from the final compiler, and all timing conclusions above use this final pass.
