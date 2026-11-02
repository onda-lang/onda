# Structured processor storage and references

## Objective

Make processor state and arrays compose at every nesting depth without arbitrary
aggregate-width restrictions or a mandatory call alternative for every array
element. Preserve intended language semantics and improve the information MIR
exposes to LLVM and Binaryen. Compiler-generated code growth is acceptable when
measured runtime gains justify an optimization; it must not be required merely
to represent an array.

The worktree starts at `21f642058427d18191013e7a4f31194151bc69e5`, an isolated
snapshot of the original workspace, including its existing uncommitted changes.
The original workspace is not the implementation destination.

## Existing behavior

- A top-level array of 128 `HSOsc` instances compiles as field arrays.
- The same array inside `SustainOscBank` expands to 6,403 leaf tensors and fails
  the 512-leaf check. That check derives from a 1,024-parameter MIR budget.
- Nested dynamic selection builds slot alternatives and a branch chain.
- MIR describes structs, arrays, reference parameters and projections, but its
  production frontend/backend path flattens aggregates to scalar/tensor leaves.
- Memory analysis retains storage identities but loses precise regions for
  nested projections. Function effects summarize each reference parameter.
- Indexed child event validation incorrectly consults the enclosing processor's
  event names. The receiver's processor must determine member resolution.

## Invariants

1. Arrays retain their element type and extent; nesting composes projections.
2. Selecting one homogeneous instance does not require enumerating other slots.
3. A method's logical receiver does not grow with the number of instances.
4. References retain identity, permission, captured coordinates and lifetime;
   writable references do not imply exclusive access.
5. Shared MIR analyses retain field/element reads, writes, ranges, alignment
   opportunities and overlapping origins. Losing an optimization proof must
   preserve correctness.
6. Initialization, integer storage ranges, pinned roots, snapshot/scratch policy,
   authored provenance and reflection remain correct for nested subobjects.
7. Persistent storage contains logical data and reconstructible selections,
   never native pointers. Buffer rebinding uses current binding tables.
8. Calls preserve argument evaluation, fallible preparation, hooks, active-slot
   scheduling, oversampling, delegate/print ordering and strict floating-point
   semantics. Alias construction does not activate a processor.
9. No new audio-thread heap allocation or per-sample reconstruction of complete
   field tables. Layout computation is shared and cached.
10. Storage and host message layouts are separate consumers of one logical type
    description. Native padding is not serialized into portable snapshots.
11. All hand-authored source files stay below 5,000 lines. Production splits
    follow cohesive responsibilities, not numbered chunks.

## Design decision and implementation stages

The selected implementation uses compositional field tensors and the existing
canonical aggregate layout. A nested `Voice[N]` remains a nominal array field;
its fields become tensors with outer array axes, and selection borrows scalar
elements or contiguous history windows. One family implementation accepts the
owner view and captured coordinates. Scalar children retain their existing
flattened representation.

Three alternatives were reviewed with the codebase-design skill's
`DESIGN-IT-TWICE.md` workflow: direct record references, compositional tensors,
and general aggregate legalization. Compositional tensors preserve the existing
SoA optimizations, per-field effects, parameter pruning, portable snapshots and
both backend adapters without introducing a second aggregate ABI. The physical
helper ABI remains proportional to live fields, independent of instance count.
A constant-word record ABI remains a separate measured design question.

The adapter resolves only declared generated paths, not guessed field names.
Family wrappers and local helpers use unambiguous identities. Uniform constructor
broadcasts and active-child lifecycle work use counted loops; authored constructor
lists retain their order. Heterogeneous bound resources still require selection,
which reuses the receiver's captured coordinate rather than an indexed payload's
coordinate. Generic aggregate views carry state without copying it or persisting
native pointers.

MIR additionally retains a power-of-two divisor of dynamic element offsets through
immutable integer arithmetic, windows, joins and calls. LLVM derives conservative
reference alignment from this fact when exact regions are unavailable. Clamping
includes its endpoint in the proof; alignment never establishes disjointness.

1. **Baseline and regressions.** Preserve a release benchmark worker and source
   corpus from the starting snapshot. Add failing public-seam coverage for large
   nested banks, independent child event names, aliases and nested forwarding.
2. **Compositional storage.** Introduce the selected storage/reference interface;
   preserve array structure before any per-slot expansion. Remove aggregate
   width rejection and remove or correctly relocate artificial function budgets.
3. **MIR and backends.** Lower indexing to direct accesses/calls; preserve precise
   effects, ranges and origins. Support both backend adapters through the shared
   contract. Delete obsolete nested dispatch and slot-expansion machinery as its
   responsibilities move behind the new interface.
4. **Semantic validation.** Exercise init/full/preserve modes, pin policies,
   snapshots, rebinding, block hooks, segments, tasks, events and delegate routing;
   compare independent expected outputs and LLVM/Binaryen execution.
5. **Performance and cleanup.** Measure native and Wasm throughput/callback
   latency, compiler time/memory, code/IR size and alignment/vectorization. Keep
   changes with supported gains, investigate material regressions, document
   evidence and update architecture/MIR documentation. Run appropriate Rust/JS,
   formatting and lint checks.

## Performance validation

Use `scripts/bench/run.py` and a preserved release `benchmark_dsp` worker. Run
timed measurements sequentially without concurrent builds/tests. Alternate
baseline/new runs when a small difference matters. Save raw rounds, tool/binary
identities, source manifests, numerical captures and generated IR under `target`.

Coverage includes tiny scalar processors; dense and random-selection banks at
1/16/128/1,024 voices; nested banks; oscillator/filter work; per-voice history
arrays; forwarded reference helpers; and existing scalar/array/window/SIMD
benchmarks. Include native and Wasm. Preserve ordered reductions; transcendental
differences use the established backend tolerance rather than assuming bitwise
libm equivalence. Assertions about performance require measured evidence.

## Progress and evidence

- Worktree: `/home/francesco/Sources/onda-proc-state`; branch:
  `compiler/structured-proc-state`. Original workspace remains untouched.
- Removed the 512-leaf aggregate rejection and the 1,024-parameter MIR budget.
  Checked sizes, cycles, nesting/shape-node complexity and local limits remain.
- Removed nested per-instance dispatch machinery. Canonical view construction and
  proc-array forwarding use linear canonical field traversal.
- Added independent regressions for events whose names differ from the owner,
  nested history arrays, captured/side-effecting/branch-local aliases, multiple
  outputs and unsafe reads, parameter hooks, forwarded arrays, active lifecycle
  hooks across segments, buffers and rebinding, routed delegates, oversampling,
  pinned initialization, snapshots and colliding source names.
- Wide aggregates and processors with 1,100 fields pass semantic/MIR validation;
  MIR still rejects malformed arguments after removing the parameter budget.
- A copied version of the original `SustainOscBank` with 128 `HSOsc` children
  compiles to MIR and optimized LLVM IR without changing the user's source
  directory. Its storage has 53 tensors rather than 6,403.
- Raw benchmark sources, worker identities, generated MIR/LLVM/Wasm, numerical
  captures, timings and test logs are retained in
  `target/proc-storage-evidence/`. The immutable baseline worker is
  `baseline/benchmark_dsp`; the phase-one candidate is `new/benchmark_dsp`.
  Their SHA-256 identities are
  `7b0fcb9de86468a9d24af9420b05e52fff4556780b33b7e4779fcb5f290d440d`
  and `bb1be8438c86d78a3cb863eeac4ae914051243f3aba6826f06e06ba99d828886`.
  The baseline snapshot is recorded above.

The phase-one measurements below use strict floating-point behavior, 48 kHz, 128-frame
blocks, one pinned CPU, five 75 ms rounds, 32 warmup blocks and 128 latency blocks.
Baseline and candidate runs are sequential, including an initial candidate /
baseline repeat and a final candidate pass. The table uses `baseline/repeat`
and `new/final`. These are local measurements, not portable performance guarantees.

| Nested bank | Native before/after (ns/frame) | Wasm before/after (ns/frame) |
| --- | ---: | ---: |
| 8 voices | 8.31 / 2.89 | 24.57 / 9.42 |
| 16 voices | 13.82 / 3.37 | 52.19 / 17.63 |
| 64 voices | 62.78 / 13.47 | 343.44 / 74.67 |
| Top-level 128, control | 39.11 / 38.62 | 143.15 / 148.39 |

The 64-voice case improves by 4.66× native and 4.60× Wasm in the final comparison.
It has 8 optimized MIR functions and 5 state tensors instead of 71 functions and
194 state entries. Serialized MIR falls from 220,033 to 31,353 bytes; Wasm falls
from 7,994 to 1,217 bytes. Frontend time falls from 81.7 to 24.1 ms and native JIT
from 277.1 to 13.2 ms. A 1,024-voice nested bank retains those same 8 functions
and 5 tensors (24.8 ms frontend, 12.2 ms native JIT in the final scaling run).
Random selected-voice history kernels take 1.44–1.50 native ns/frame and
3.15–3.27 Wasm ns/frame from 16 through 1,024 voices. Native/Wasm captures match;
baseline/candidate bank captures match exactly.

The unchanged top-level control emits byte-identical MIR, LLVM IR and Wasm.
So do all seven scalar, oscillator, filter, fixed-slice, state-window and reduction
controls. Their initial paired measurements differ by at most about 2.2%; later
sweeps show system timing drift despite identical code. The control differences
are therefore not evidence of a compiler regression or improvement. Both sweeps
and every raw round are retained in `new/{controls,final-controls}`.

Instrumented compilation of the 64-voice bank reduces frontend requested Rust
allocation bytes from 195.6 MB to 19.8 MB, and allocation calls from 2,069,271 to
175,387. Process peak RSS after native JIT falls from 241,268 to 69,496 KiB.
The 1,024-voice candidate peaks at 70,016 KiB. Allocation counters exclude LLVM's
C++ allocations; process RSS includes them. These instrumented timings are kept
separate from the throughput table.

## Phase-one verification

- Frontend: 280 unit and 2 integration tests passed.
- Semantics: 1,129 tests passed.
- Examples/runtime: 835 passed; the existing manual oversampling performance
  benchmark remains ignored by the normal suite.
- MIR: 169 unit and 5 analysis-scaling tests passed.
- LLVM: 159 unit tests, all integration suites and the documentation test passed.
- Binaryen JavaScript: 105 tests passed.
- Native/MIR-Binaryen parity: 52 scenarios × optimized and unoptimized Binaryen;
  3,888 bit-exact and 4,096 approximate samples, maximum approximate absolute
  error 1.192e-7 against the established 1e-6 tolerances.
- Strict Clippy for all targets of frontend, semantics, MIR, LLVM and examples
  passed; formatting and diff checks passed. No hand-authored source exceeds
  5,000 lines.
- Dynamic alignment tests verify aligned multiplication, clamp endpoints,
  forwarding and joins; unknown origins remain conservative. A 10,000-definition
  arithmetic chain verifies stack-safe linear analysis.

The cached-output regressions exposed an older parser ambiguity between
`voices[i].output` and `voices[i]().output`. The parser now preserves that
distinction. Indexed and aliased cached reads do not step; explicit calls do.
Existing hook expectations were corrected to observe the documented cache
behavior, with a third block verifying delayed post-hook output visibility.

## Phase-one borrowing boundary (superseded)

The first checkpoint used state-only helper views and temporarily rejected
borrowed children that needed owner callbacks or bound resources. The complete
view implementation below replaces that restriction. The checkpoint remains
available to compare correctness, compiler scaling and runtime performance.

## Reproduction

Run from the worktree. `target/proc-storage-evidence/sources/` and each run's
source manifest preserve the exact DSL inputs. Timed passes ran sequentially
without concurrent builds or tests.

```sh
cargo test -p onda_frontend -p onda_semantics -p onda_mir -p onda_codegen_llvm
cargo test -p onda_examples --test examples_suite
cargo clippy -p onda_frontend -p onda_semantics -p onda_mir -p onda_codegen_llvm -p onda_examples --all-targets -- -D warnings
npm test --workspace @onda-lang/binaryen-web
ONDA_CLI=target/debug/onda node packages/onda_binaryen_web/scripts/verify-backend-parity.mjs

python3 scripts/bench/run.py \
  --source target/proc-storage-evidence/sources/nested-8.onda \
  --source target/proc-storage-evidence/sources/nested-16.onda \
  --source target/proc-storage-evidence/sources/nested-64.onda \
  --source target/proc-storage-evidence/sources/top-128.onda \
  --runner target/proc-storage-evidence/new/benchmark_dsp \
  --backend both --repetitions 5 --round-ms 75 --warmup-blocks 32 \
  --validation-blocks 4 --latency-blocks 128 --ir --timeout-seconds 90 \
  --compare-to target/proc-storage-evidence/baseline/repeat \
  --output target/proc-storage-evidence/new/reproduction
```

## Phase-one completion audit

The core nested-array storage redesign, direct homogeneous calls, limit removal,
MIR alignment facts and associated regression verification are implemented. At that checkpoint the
broader borrowed-processor environment contract was incomplete: free helpers did
not carry owner callback environments or statically bound buffers. Those cases
were diagnosed explicitly. A state-only view was not a complete processor
capability view.

That checkpoint retained synthetic source slot identities with
linear capacity cost. The second stage replaces them with declarations and typed
projections, including logical scalar children; scalar physical flattening remains
a layout choice. The per-live-field physical ABI is the selected design, rather
than an unfinished promise of a constant-word record ABI.

Verification covers the recorded suites and benchmark cases. It does not cover
every combination from the original performance matrix, every host API, or
cross-platform execution. One manual oversampling performance benchmark was
not run. These limits prevent a claim that the entire broader plan is finished
or that every possible compiler workload is regression-free.

A follow-up compile-time audit alternated baseline and candidate order over five
pairs, with seven native controls per sweep, one compile per fresh worker and
identical configuration. Artifacts and raw samples are in
`target/proc-storage-evidence/compile-audit/`; `summary.json` records medians and
median absolute deviations. Total frontend-plus-JIT medians are below.

| Control | Before (ms) | After (ms) | Change |
| --- | ---: | ---: | ---: |
| passthrough | 24.79 | 24.53 | -1.0% |
| osc-phasor | 39.79 | 39.09 | -1.7% |
| filter-onepole | 34.60 | 34.32 | -0.8% |
| slice-fixed | 30.92 | 30.64 | -0.9% |
| state-window-array-f32 | 28.59 | 28.03 | -2.0% |
| state-window-shared-array-f32 | 31.96 | 31.44 | -1.6% |
| operator-reduction-f32 | 29.58 | 29.43 | -0.5% |

The small control differences are consistent with broadly unchanged compilation
cost, with no material regression observed in this audit. They do not justify
claiming an across-the-board speedup. The nested 64-voice bank has a large,
repeatable improvement: frontend plus native JIT falls from 358.8 to 37.3 ms,
about 9.6× faster. Binaryen compilation in the recorded IR-producing run falls
from 958.4 to 464.5 ms, about 2.1× faster.

## Second-stage acceptance criteria

| Area | Required result |
| --- | --- |
| Complete borrowed processor views | One logical view carries state projection, active/lifecycle identity, bound resources and enclosing callback environments through inference, specialization, forwarding and joins. |
| Capacity-independent declarations | One uniform declaration and binding replaces synthetic per-slot registries. Scalar and array children share typed projection identities. |
| MIR and backends | Direct references/calls and typed resource descriptors preserve field effects, overlapping origins and alignment in LLVM and Binaryen. |
| Hardening and measurement | Verify helper depth, independent owners, joins, resources/rebinding, segmented hooks, tasks, events, snapshots and strict ordering; measure compilation and runtime. |

Environment specialization should depend on the processor type, compile context
and environment shape, rather than the physical instance path. Several instances
of one owner shape should share implementations. Joins between genuinely
different environments need an explicit supported representation; they must not
silently erase context or force one implementation per array slot.

Uniform buffer bindings should remain constant-size handles. Heterogeneous
bindings should use prepared resource selection against current host tables,
without reconstructing descriptor tables on each sample or copying processor
state. Persistent selections retain reconstructible identities and coordinates,
not native pointers. Owner and child references may overlap; the environment
must not introduce false exclusivity or alias promises.

Completion requires replacing the owner-context and bound-buffer helper
rejections with passing native/Wasm execution tests, eliminating synthetic slot
metadata for uniform arrays, and retaining the current scaling/runtime gains.
Compiler structure should grow with declared types, genuine specializations and
authored heterogeneous data; actual storage and initialization still grow with
array capacity.

Keep columnar field tensors and the live-field helper ABI as the performance
baseline. A compact record receiver or a new physical layout is a separate
benchmark-driven experiment, not a prerequisite for completing processor views.

## Complete view implementation

The phase-one source checkpoint is `85f9da5ee3de2252ab37bfb780a7732c27bb3f7f`
(`refs/codex/structured-proc-state-phase1`). It preserves the measured columnar
implementation before the remaining work. Evidence for this stage goes under
`target/proc-storage-evidence/complete/`.

The second design comparison considered (1) a small processor-view catalog with
shape specialization, (2) a general declaration/projection algebra shared by
scalar and array instances, and (3) preserving the current state ABI while
legalizing complete environments into direct MIR calls. We combine their useful
parts: typed declared projections and complete view shapes belong to semantics;
ordinary MIR references, calls and explicit control flow remain the backend
contract. Shape identity includes callback declaration/routing and outer owner
environments, while root instances and captured coordinates are runtime data.
Helpers specialize by shape, never by each slot or physical instance path.

The complete view carries processor state, lifecycle identity, bound resources,
and owner callback environment. Source expression selection runs once. Joins
retain environment alternatives and route calls to the corresponding captures.
No inactive capture may be read without initialization, and writable captures do
not imply exclusivity. Scalar and array projections use the same declared model;
physical scalar flattening and columnar arrays remain layout choices.

Mapped heterogeneous buffers use typed immutable logical index maps into current
host binding tables, forwarded through typed parameters. Uniform buffers and
contiguous spans retain their smaller existing forms. Selection performs no
per-sample descriptor gather. Truly unrelated parameter descriptors may use
balanced direct-call selection. Persistent selections store logical identities
and coordinates, never native descriptor or callback pointers.

Implementation order: carry complete shapes through inference/specialization;
legalize captures and operations; replace per-slot declaration maps and generated
name recovery; verify native/Binaryen behavior and measured scaling. New public
execution regressions include nested array/selected-child forwarding, owner
helper projection, separate owner instances sharing one shape, and joins of
children with different owner types. The existing lifecycle, event, resource,
oversampling, snapshot and alignment tests remain required.

## Final completion audit

The second stage replaces the phase-one borrowing restrictions. Processor views
carry state projection, captured coordinates, lifecycle identity, bound resources
and enclosing callback environments through inferred and explicit helper
parameters, selected scalar children, arrays, forwarding and branch joins.
Persistent aliases and task frames retain logical selections and reconstruct
current resource descriptors after a yield or snapshot restore.

Uniform arrays retain one declaration and binding. Shared family generation and
MIR functions no longer grow with capacity; authored heterogeneous constructors
retain their declared order and genuine alternatives. Scalar children share the
logical projection catalog while preserving their compact physical layout.
Generated-path parsing, synthetic per-slot iteration, temporary capability
rejections and the old general dispatch adapter have been removed.

Resource signatures and calls share typed capture plans. Exact owner-frame depth
and resource ordinals distinguish enclosing resources from child resources and
explicit payload parameters. Task signature generation records its own lexical
capture metadata. Parameter and local-variable shadowing use lexical scope,
including when a payload name matches an owner buffer or processor child.

MIR uses direct references/calls, conservative alignment proofs, immutable typed
buffer maps and local descriptor selections. Both backends preserve complete
resource origins, writes, subviews and rebinding without per-sample descriptor
gathers or audio-thread allocation. The physical ABI remains proportional to
live fields and real resource alternatives, independent of uniform array capacity.
Ordinary scalar typing bypasses processor-view lookup, and view-only reachability
work runs only when view specializations exist.

Final validation passes: frontend 280 + 2; semantics 1,140; MIR 173 + 5 scaling + 6
resource tests; native 165 plus integration suites; examples/runtime 847; C API
57; Binaryen 109; and native/Wasm parity 59 scenarios at both Binaryen O4 and
unoptimized settings. The separately invoked manual oversampling benchmark also
passes. Strict Clippy, formatting, diff and 5,000-line source-file checks pass.
Validation was performed on Linux; cross-platform execution is not claimed.

The copied 128-voice Sustain program compiles to MIR and native IR with 53 state
tensors. The final measurements, immutable worker identities, paired compile-time
samples, allocation counts and reproduction instructions are recorded in
[the dated benchmark report](../../scripts/bench/results/2026-11-02-processor-views.md).
The complete-view implementation and cleanup described above are finished; a
compact record ABI or different physical layout remains a separate experiment.
