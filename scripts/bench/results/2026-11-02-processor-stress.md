# Processor storage: broad stress pass — 2026-11-02

The broader pass found and fixed two compiler regressions. After those fixes,
all checked programs and numerical comparisons pass. No generated-code runtime
regression was confirmed in this native corpus. The columnar bank improvements
remain: 4.20× and 4.92× faster than the original compiler at 16 and 128 voices.
Completing processor views preserves the first stage's processing performance.

## Correctness and coverage

| Check | Result |
| --- | --- |
| Entire Rust workspace | 3,641 passed, zero failures; four existing ignored tests |
| Ignored oversampling performance test, invoked separately | Passed; 1.356× baseline cost, budget 2.5× |
| Web package tests | 361 passed |
| Checked-in browser example catalog | 54 projects passed |
| Materialized projects with packaged assets | Three passed |
| Native/MIR-Binaryen parity | 60 scenarios × O4 and unoptimized Binaryen |
| Native MIR and LLVM IR inspection | 159 programs passed |
| Native runtime sweep | All 144 eligible current programs passed |
| Benchmark harness, including integration | 11 passed |
| Strict workspace/all-target Clippy, formatting, diff and source-size checks | Passed |

The other three ignored Rust tests require native display/clipboard sessions.
They were not exercised. Browser project verification includes valid Wasm,
exact source manifests and project assets; it does not measure browser DSP speed.
Parity covers 4,000 bit-exact sample comparisons and 4,096 approximate comparisons,
with maximum approximate absolute error 1.192e-7, within the existing 1e-6 tolerances.

The inspection corpus contains every one of the 105 benchmark catalog workloads
and every one of the 54 checked-in `.onda` example files. It covers oscillators,
filters, dynamics, smoothing, delays, reverberation, pitch, noise, envelopes,
FFT/STFT/streaming transforms, convolution, slices, processor banks, callbacks,
state windows, guarded arrays and scalar operators, including f32/f64 cases.

Fifteen example files are library fragments without outputs or require external
buffers. They pass inspection and applicable browser/project checks, but the
native timing harness does not execute them. The JSON manifest identifies them.
Runtime measurements use 128-frame blocks; browser examples compile at 512 frames,
and the parity scenarios execute at four frames. This is Linux/x86-64 evidence,
not a cross-platform or exhaustive-input guarantee.

## Regressions found and fixed

Fix commit: `84dd0e4`.

1. Counted processor-array constructors were checked as external assignments to
   private parameters. A generated constructor loop contains already-validated,
   already-clamped initialization writes; the constructor exemption formerly
   applied only to individual top-level assignments. Treating the generated
   setup subtree consistently fixes `examples/feedback/neural_synth.onda` and
   arrays of processors with private defaults, without changing parameter privacy.
2. The lifecycle guard generator skipped literal indices, assuming their hooks
   were handled eagerly. Shared array declarations invalidate that assumption:
   a sibling using a runtime selector could remove eager hooks for the entire
   array, leaving literal slot zero inactive. Both global and nested arrays now
   use the same per-slot lazy activation rule. Constant selectors remain available
   to MIR/LLVM simplification. Two independent task slots now yield together and
   then produce 22, rather than the erroneous 11.

Regression coverage includes global/nested task arrays with literal/variable
selectors, pinned/full initialization, private array construction and public
events, plus a new native/Wasm task scenario. The minimal cases were red before
the fixes and green afterward. Temporary debugging instrumentation was removed.

## Measurement method

The [JSON evidence](2026-11-02-processor-stress.json) contains all raw benchmark
rows, round samples, callback latency, source fingerprints, output comparisons,
worker identities, follow-up object hashes, source fixtures and check-log hashes.
Complete MIR, LLVM IR, objects, disassembly, captures, commands and logs remain in
`target/proc-storage-evidence/stress-2026-11-02/` in the implementation worktree.

Three immutable release workers were compared:

| Worker | SHA-256 |
| --- | --- |
| Original compiler | `7b0fcb9de86468a9d24af9420b05e52fff4556780b33b7e4779fcb5f290d440d` |
| First columnar stage | `bb1be8438c86d78a3cb863eeac4ae914051243f3aba6826f06e06ba99d828886` |
| Complete views with stress fixes | `c4af8ae26a3a7428e56c7107e5968ff2de3567e31e8ee0769d94be897a073cba` |

Measurements ran sequentially on CPU 0 of an Intel Core i9-14900HX, at 48 kHz,
LLVM O3, strict floating point and the normal native FTZ/DAZ policy. Builds and
test suites were stopped during timing. Worker order rotated across cases.
Each broad run uses five 50 ms rounds, catalog warmup counts, eight startup and
eight warmed validation blocks, and 512 callback-latency samples. Captured sources
match across all comparable workers. The harness uses its deterministic seeded
input pattern and typed output captures.

There were 432 broad attempts: 144 current successes, 140 original successes and
141 first-stage successes. The seven reference failures are existing limitations:
the original compiler's 512-leaf cap rejects the 1,024-voice bank, and both older
workers reject the three complete callback-view workloads. No speedup is assigned
to those unsupported comparisons.

All 136 original/current and 137 first-stage/current ordinary output comparisons
are bit-identical at startup and after warmup: 993,280 sample comparisons, zero
mismatches, maximum absolute difference zero. Four FFT accuracy workloads use
their separate bounded-error contract; all three workers pass that contract.

## Runtime results and follow-up trials

The following are medians of three fresh-worker trial medians. Each trial uses
seven 150 ms rounds, alternating worker order and 2,048 callback-latency samples.
Nine workloads received these longer trials; all 81 runs pass, with bit-identical
startup and warmed outputs against both references.

| Workload | Original ns/frame | First stage | Complete views |
| --- | ---: | ---: | ---: |
| Nested bank, 16 voices | 13.586 | 3.246 | 3.231 |
| Nested bank, 128 voices | 188.153 | 38.236 | 38.233 |
| Smoothed gain | 1.626 | 1.625 | 1.624 |
| STFT, 1,024 points | 6.142 | 6.065 | 6.085 |
| Checked-in basic voice bank | 2.012 | 2.136 | 2.008 |
| Checked-in Glass Garden | 94.645 | 94.783 | 94.549 |
| FM bells plugin, all voices held | 115.737 | 116.369 | 115.714 |
| Poly saw plugin, all voices held | 228.046 | 227.625 | 227.909 |

The unchanged plugin sources were silent because the harness sends no MIDI.
The two explicitly derived active variants copy the existing note-on body into a
one-time block startup loop and hold every voice. They preserve the instrument
code and default parameters. Both produce nonzero startup/warmed output and
match both older compilers bit-for-bit. Their exact derived sources are recorded
in the JSON; these are supplemental cases, separate from the unchanged examples.

The initial sweep flagged passthrough (+28% against original), smoothed gain
(+10%) and STFT (+5%) as slower. All three emit byte-identical optimized LLVM IR
and native objects on every compiler. Longer trials reduced original/current
differences to about -0.9%, -0.1% and -0.9%. Passthrough still varied +4.8% against
the first-stage worker at approximately 0.09 ns/frame despite identical objects.

To isolate generated DSP from worker layout and timing effects, both versions'
saved MIR for those three controls were replayed through the same current worker,
in alternating order, for three seven-round 150 ms trials. LLVM IR remained
identical and output remained bit-identical. Current/original replay ratios were
0.992, 1.000 and 0.998. These results do not establish a generated-code slowdown.
The initial apparent basic voice-bank gain also does not establish a code gain:
its native object is identical across all workers. No gain is claimed from it.

The measured bank gains are substantial and repeat: 4.20× at 16 voices and 4.92×
at 128 voices versus the original representation. The first-stage and complete
workers emit identical objects for both banks. This pass establishes that their
gains survive complete views and the correctness fixes; it does not claim a new
processing speedup over the first columnar stage.

## MIR, LLVM and assembly findings

Whole optimized LLVM IR is byte-identical for 125/140 original comparisons and
128/141 first-stage comparisons. Remaining differences were inspected separately
from timings. Most complete-stage differences are initialization or metadata
renumbering; after ignoring metadata attachments, processing instruction text
matches the first stage except for the two initially idle plugin instruments.
Their active variants have identical processing instruction bytes in object
disassembly and essentially unchanged measured throughput. Byte comparison is
static evidence; relocations and runtime addresses are not a timing model.

| Nested bank | Original MIR functions / tensors | Complete functions / tensors | Original / complete LLVM process instruction lines |
| --- | ---: | ---: | ---: |
| 16 voices | 23 / 50 | 8 / 5 | 301 / 89 |
| 128 voices | 135 / 386 | 8 / 5 | 1,892 / 311 |
| 1,024 voices | Rejected | 8 / 5 | No original comparison |

The 128-voice bank's serialized MIR falls from 425,339 to 31,380 bytes. The same
8 functions and 5 state tensors remain at 1,024 voices; the complete callback
workload at 1,024 voices uses 13 functions and 8 tensors. Real state storage still
grows with capacity, as required; a tensor count is not its allocated byte size.

In optimized bank LLVM, previously scalar processing becomes eight-wide f32
operations. The 128-voice state base carries its proven 64-byte alignment, and
vector accesses retain 32/64-byte alignment as their exact offsets permit.
The new process body has 160 vector instruction lines versus zero in the original.
Static instruction counts describe the emitted code; the runtime trials above
establish the performance effect.

Saved pre-optimization LLVM, `opt` remarks and object disassembly confirm the
voice-processing loop vectorizes at width eight. The ordered floating-point sum
is explicitly left unvectorized when reordering cannot be proved safe. The
complete callback case also retains direct MIR calls; inspection found no new
indirect call or allocator call in these bank/callback LLVM artifacts. The
representation exposes contiguous access and alignment while preserving strict
arithmetic and the existing alias contracts.

## Reproduction

The exact drivers are retained as `audit.py`, `followup.py`, `replay.py` and
`analyze-followup.py` under the evidence directory. They use the existing bounded
benchmark harness; the JSON preserves the generated workload sources and case
manifest. The workers are frozen under `baseline/`, `new/` and the stress
directory's `current-fixed/` inside `target/proc-storage-evidence/`.

Representative current-worker reproduction:

```sh
cargo build --release -j1 -p onda_examples --features llvm-orc --example benchmark_dsp
cargo build --release -j1 -p onda_cli --bin onda
python3 scripts/bench/run.py --all --backend native --ir --cpu 0 \
  --repetitions 5 --round-ms 50 --validation-blocks 8 --latency-blocks 512 \
  --timeout-seconds 180 --output target/bench/stress-catalog
cargo test --workspace -j2
npm run test:web
ONDA_CLI=target/release/onda node packages/onda_binaryen_web/scripts/verify-backend-parity.mjs
```

Use a fresh output directory for each benchmark and keep builds/tests stopped
during measurements. The saved per-case configuration records warmup and bounds.
The native sweep measures steady processing with fixed input/parameter settings,
not every host event schedule or every possible source program.
