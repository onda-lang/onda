# Hosted API simplification measurements

Measured on 2026-11-06, Intel Core i9-14900HX, x86-64 Linux 7.3.1, Rust 1.98.0,
release build, LLVM O3, strict math, 48 kHz. Pinned comparisons use logical CPU 2.
Baseline: working tree at the start of this task (HEAD
`58bbcfba1744b980535f2f0834b250bd9455a9cc` plus the existing unstaged `onda bench` work).
Baseline, direct-dispatch and experimental-specialization binaries were preserved under
`/tmp/onda-api-audit` for this session. The final implementation retains the canonical generated
DSP body and uses a direct hosted full-block entry without segment validation or FP setup.

## Generated kernel experiment

The experiment adds a JIT-only full-block entry, inlines the canonical process body and lets LLVM
optimize constant start/frame/flag arguments. It does not hand-author a second DSP body. The paired
runner calls the two native kernels directly, excluding hosted runtime checks. Both entries share
one compiled program, use separate state/storage, and alternate timing order over 11 rounds.
Calibration chooses at least 50 ms per round. Inputs are silent. Audio and packed persistent state
are checked for exact equality for 256 blocks before timing and after every paired round.

| Processor | Frames | Canonical ns/block | Specialized ns/block | Change |
| --- | ---: | ---: | ---: | ---: |
| tiny | 32 | 6.46 | 9.27 | +43.5% |
| tiny | 128 | 9.37 | 8.90 | -4.9% |
| tiny | 512 | 17.12 | 22.64 | +32.2% |
| sine | 32 | 521.63 | 488.94 | -6.3% |
| sine | 128 | 2151.86 | 1941.15 | -9.8% |
| sine | 512 | 10763.04 | 11503.81 | +6.9% |
| cybernetic_feedback_graph | 32 | 12390.54 | 12392.58 | +0.0% |
| cybernetic_feedback_graph | 128 | 49429.75 | 49402.58 | -0.1% |
| cybernetic_feedback_graph | 512 | 197238.77 | 197497.36 | +0.1% |

**Decision:** omit the specialization. Regressions in two tiny cases outweigh an inconsistent sine
improvement; feedback is effectively unchanged. The direct hosted full-block path remains useful
without duplicating generated code. These measurements do not establish a general throughput gain.

`generated-specialization.patch` contains the isolated experiment and its paired benchmark.
To reproduce in a disposable checkout containing this final change:

```sh
git apply scripts/bench/results/2026-11-06-api-simplification/generated-specialization.patch
cargo build --release -p onda_examples --example benchmark_full_block
taskset -c 2 target/release/examples/benchmark_full_block --tiny 32
taskset -c 2 target/release/examples/benchmark_full_block examples/basic/sine.onda 128
taskset -c 2 target/release/examples/benchmark_full_block examples/feedback/cybernetic_feedback_graph.onda 512
```

Repeat each source at 32, 128 and 512 frames. `paired.json` retains all individual measurements.

## Hosted baseline versus final

Nine rounds, 2,000 warmup blocks. Each round uses 2,000,000 blocks for tiny, 20,000 for sine and
2,000 for feedback. Variants run consecutively on CPU 2; this remains sensitive to frequency,
thermal and code-layout differences. There is no supported claim of a broad DSP speedup.

| Processor | Frames | Baseline µs/block | Final µs/block | Change |
| --- | ---: | ---: | ---: | ---: |
| tiny | 32 | 0.012048 | 0.011651 | -3.3% |
| tiny | 128 | 0.013128 | 0.014114 | +7.5% |
| tiny | 512 | 0.033933 | 0.028255 | -16.7% |
| sine | 32 | 0.481704 | 0.467218 | -3.0% |
| sine | 128 | 2.076946 | 2.080936 | +0.2% |
| sine | 512 | 9.986662 | 10.151332 | +1.6% |
| cybernetic_feedback_graph | 32 | 11.617144 | 11.614743 | -0.0% |
| cybernetic_feedback_graph | 128 | 46.555189 | 46.393138 | -0.3% |
| cybernetic_feedback_graph | 512 | 186.738458 | 186.657516 | -0.0% |

`hosted-pinned.json` records commands, options and individual rounds. Tiny source:

```onda
sample:
  out1 = in1 * 0.5
```

`baseline.json` and `comparison.json` retain the initial unpinned exploration, including the
intermediate direct-dispatch build. They were too noisy for decisions and are not the tables above.

Release symbol sizes provide a separate check of host dispatch: the baseline full entry is a
48-byte trampoline to a 1,720-byte segment entry; final full dispatch is a standalone 374-byte
function and segment dispatch is 446 bytes (`nm -S -C`). These are wrapper sizes, not generated DSP
sizes or cycle estimates. The final JIT generation path uses the original canonical DSP body.
