# Complete processor views — 2026-11-02

The complete-view implementation retains the first stage's DSP improvements and
removes its remaining capacity-dependent frontend registries. Borrowed helpers
now preserve owner callbacks, bound buffers, lifecycle identity and captured
coordinates, including selected scalar children, joins and retained task views.
Compiler speed is **not uniformly unchanged**: large banks improve substantially,
while some small controls take longer to analyze.

## Method and identities

Measured on Linux, Intel Core i9-14900HX, pinned CPU 0, strict floating-point
semantics, 48 kHz and 128-frame blocks. Runtime runs use five 75 ms rounds,
32 warmup blocks, four validation blocks and 128 latency blocks. Builds and tests
were stopped during measurements. Compiler controls use five fresh-worker pairs,
alternating order, with identical configuration. Results are local observations.

The [JSON evidence](2026-11-02-processor-views.json) contains worker hashes,
source inputs, raw rounds, allocation counters, paired compiler samples, medians,
MADs, artifact identities and validation counts. Complete generated artifacts and
logs remain in `target/proc-storage-evidence/complete/` in the implementation
worktree. The final worker is `complete/optimized/benchmark_dsp` beneath that
evidence root; the earlier complete-view candidate remains `complete/benchmark_dsp`.

- Original source snapshot: `21f642058427d18191013e7a4f31194151bc69e5`.
- First-stage checkpoint: `85f9da5ee3de2252ab37bfb780a7732c27bb3f7f`.
- Final worker SHA-256: `9c7990e1e2fd8dce3bcc0e1464aaf17dd09c8dde9de962c49103b2bdf425dac4`.

## Runtime and bank compilation

| 64-voice nested bank | Original | Complete model | Improvement |
| --- | ---: | ---: | ---: |
| Native ns/frame | 59.29 | 12.72 | 4.66× |
| Wasm ns/frame | 329.68 | 69.24 | 4.76× |
| Frontend ms | 77.00 | 21.94 | 3.51× |
| Native JIT ms | 262.98 | 13.12 | 20.05× |
| Frontend + JIT ms | 339.98 | 35.06 | 9.70× |
| Binaryen compilation ms | 889.98 | 445.96 | 2.00× |
| MIR functions / state tensors | 71 / 194 | 8 / 5 | |
| Serialized MIR bytes | 220,033 | 31,353 | |
| Wasm bytes | 7,994 | 1,217 | |

These DSP gains were established by the columnar first stage; completing views
preserves them. Plain nested banks retain 8 functions and 5 tensors at 16, 64,
128 and 1,024 voices. A 1,024-voice bank takes 22.71 ms frontend and 11.16 ms JIT
in this run. Forwarding selected children through three helper layers with an
owner callback retains 13 functions and 8 tensors from 16 through 1,024 voices.
Its MIR size changes only from 51,861 to 51,965 bytes as extents change.

Random selected-voice history access remains constant-cost across capacities;
see the per-case native/Wasm rounds in the JSON. These kernels and the callback
kernels pass bitwise native/Wasm comparison. Old/new bank outputs also agree.
The callback workload has no valid old implementation, so no old/new speedup is
claimed for that added capability.

The top-level 128-voice control keeps its DSP behavior while serialized MIR falls
from 81,527 first-stage bytes to 20,201 bytes after compact declaration and
counted initialization lowering. The copied Sustain program with 128 HSOsc
children compiles to native IR with 53 state tensors, versus 6,403 in the
original representation.

## Frontend capacity scaling

Instrumented allocation counts exclude LLVM's C++ allocator. Instrumented timing
is kept separate from the runtime table.

| Nested bank | First-stage requested Rust bytes / calls | Complete model bytes / calls |
| --- | ---: | ---: |
| 64 voices | 19,842,778 / 175,483 | 20,077,727 / 186,321 |
| 1,024 voices | 40,713,526 / 424,430 | 20,077,743 / 186,306 |

The complete environment adds a small fixed cost at 64 voices, and removes the
remaining per-slot frontend growth. Actual state allocation and initialization
still grow with capacity. Genuine authored constructor alternatives also retain
their necessary source-dependent cost.

## Compiler controls and remaining cost

Frontend-plus-native-JIT medians ± MAD, in milliseconds, against the original
worker. Raw samples and a separate five-pair comparison against the first-stage
worker are included in the JSON.

| Control | Original ms | Complete model ms | Change |
| --- | ---: | ---: | ---: |
| passthrough | 25.20 ± 1.92 | 24.74 ± 0.47 | -1.8% |
| osc-phasor | 40.52 ± 1.65 | 44.77 ± 0.59 | +10.5% |
| filter-onepole | 33.46 ± 0.92 | 32.64 ± 0.22 | -2.4% |
| slice-fixed | 33.32 ± 1.62 | 31.93 ± 0.38 | -4.2% |
| state-window-array-f32 | 28.32 ± 1.63 | 31.60 ± 1.15 | +11.6% |
| state-window-shared-array-f32 | 35.44 ± 0.12 | 36.49 ± 1.71 | +3.0% |
| operator-reduction-f32 | 29.99 ± 0.67 | 29.25 ± 0.29 | -2.5% |

The oscillator increase repeats against both references: about 4 ms overall,
with roughly 2.6–2.8 ms additional semantic analysis and about 1 ms additional
parse/load time in the paired medians. The state-window control is also slower
in this sweep; its first-stage comparison shows +4.2%, with appreciable frontend
sample variability. Other controls are mixed. These observations do not support
an across-the-board compiler speedup or a claim of zero compile-time regression.

The seven controls emit byte-identical MIR, optimized LLVM IR and Wasm to the
first-stage worker. Runtime timing changes on these controls do not establish
code-generation gains or losses. The complete stage avoids processor-view
lookup for concrete lexical scalar/resource values and skips two view-specific
reachability traversals when no view specializations exist; remaining measured
frontend cost is reported above rather than hidden by those optimizations.

## Correctness and cleanup

Passing checks: frontend 280 + 2; semantics 1,140; MIR 173 + 5 scaling + 6 resource
tests; native 165 plus integration suites; examples/runtime 847; C API 57;
Binaryen 109; strict Clippy, formatting, diff and source-file-size checks.
The normal suite's ignored oversampling benchmark was invoked separately and
passed (observed 1.38× for N=4, diagnostic local timing).

Native/MIR-Binaryen parity passes 59 scenarios at Binaryen O4 and unoptimized:
3,976 bit-exact samples, 4,096 approximate samples, maximum approximate absolute
error 1.192e-7 within the established absolute/relative 1e-6 tolerances.
Coverage includes complete callback chains, distinct owner instances/types,
joined array owners, scalar children, resource maps and finite span choices,
writes/rebinding, snapshots, yielded task views and lexical payload shadowing.

The compiler retains no temporary owner/buffer borrowing rejection, per-capacity
synthetic slot registry, generated-path decoder or general per-instance call
dispatch adapter. Lexical resources are recorded at signature generation by owner
frame/resource ordinal and actual capture parameter. Writable captures retain
overlapping origins; alignment introduces no exclusivity promise. No descriptor
table gather or audio-thread heap allocation was introduced.

## Reproduction

Build a release worker with the repository's LLVM setup, then use the checked-in
benchmark generators:

```sh
cargo build --release -j2 -p onda_examples --features llvm-orc --example benchmark_dsp
python3 scripts/bench/run.py --filter 'proc-bank|proc-helper-callback' \
  --backend both --ir --bitwise --repetitions 5 --round-ms 75 \
  --warmup-blocks 32 --validation-blocks 4 --latency-blocks 128 \
  --cpu 0 --output target/bench/processor-views
```

The JSON's `sources` object preserves the exact custom inputs used in this report;
write an entry to an `.onda` file and select it with `--source` for exact source
replay. Rebuild/preserve each worker separately, alternate compiler-control order,
and run measurements sequentially. Additional verification and inspection options
are documented in the [benchmark guide](../README.md).
