# Onda MIR

Onda MIR is the backend-neutral executable representation shared by native LLVM codegen and
WebAssembly codegen. It is a core compiler boundary, not a Web-specific translation format.

The production pipeline is:

```text
source
  -> parser AST
  -> semantic analysis and source-level rewrites
  -> fully specialized TypedProgram
  -> MIR construction and backend-neutral validation
  -> MIR canonicalization and optimization
  -> backend legalization
  -> MIR-native LLVM JIT/AOT or Binaryen/WebAssembly lowering
```

## Boundary

MIR begins after all source-language decisions have been made. A valid MIR program has:

- no namespaces, imports, generics, overload sets, graphs, or proc surface syntax
- no implicit declarations or implicit storage classes
- no name-based symbol resolution
- no named/default argument binding
- no unresolved numeric literal types or implicit casts
- no backend-recognized sentinel calls
- explicit left-to-right evaluation order and side effects
- explicit bounds behavior for every indexed operation
- explicit init, process, and event entry points
- symbolic host resources rather than native pointers or WebAssembly offsets

Debug names remain in MIR for dumps and diagnostics, but IDs determine identity.

MIR does not contain LLVM types, LLVM intrinsics, WebAssembly opcodes, ABI pointer widths, target
alignment, or linker policy. Those belong below the MIR boundary.

Source literals and contextual constants have already specialized before MIR construction. Every
MIR scalar operation therefore has one concrete width. Backends must preserve the result as if each
operation executed and rounded at that width; they must not introduce observable implicit wider
intermediates. An explicit cast changes the width, and explicit `fma` retains its separately defined
single-rounding semantics.

Integer `add`, `subtract`, and `multiply` wrap at their declared width. Shift counts are masked to
that width. Signed division and remainder trap on zero; `MIN / -1` wraps to `MIN`, and `MIN % -1`
is zero. Floating-point equality is ordered, `!=` is true for NaN, and relational comparisons are
false for NaN. Float-to-integer casts saturate to the destination range and convert NaN to zero.
These rules are MIR semantics, not backend optimization choices.

## Form

Onda MIR is typed, structured, non-SSA, and in administrative normal form.

"Structured" means control flow remains nested `if` and `loop` regions. This maps directly to
WebAssembly and preserves the structure already guaranteed by the Onda language. LLVM lowering can
emit ordinary blocks and allocas; LLVM's promotion passes recover SSA form.

"Administrative normal form" means complex expressions are linearized into typed locals. Calls and
other side effects are statements, not nested expression nodes. This makes source evaluation order
part of MIR rather than a backend convention.

Source `&&` and `||` are lowered to structured control flow before MIR. They are not ordinary binary
operations because their right-hand sides are conditionally evaluated.

For example, source shaped like:

```onda
out1 = f(x) + g(y)
```

is conceptually represented as:

```text
%0 = call f(x)
%1 = call g(y)
%2 = add %0, %1
output.store out1, frame, %2
```

This is deliberately not SSA. Existing mutable locals and state remain explicit places, and later
passes may promote or fold them without changing the semantic contract.

## Program model

`onda_mir::Program` owns deterministic vectors of:

- source files and source spans
- logical types and nominal struct layouts
- the host-facing input/output/parameter/buffer/event interface
- persistent state and instance scratch state
- immutable constant data
- functions
- init and process entry points

Events name their handler function directly. Functions have one of four capability roles: init,
process, event, or user function. Each function also carries backend-neutral origin and inline
attributes. Compiler-generated hot glue therefore requests consistent treatment without a backend
recognizing a mangled name.

Types are logical. An array has an element type and length, but MIR does not decide its byte offset.
A buffer has an element type, channel constraint, and access mode, but MIR does not represent its
host pointer. This keeps the program valid for both 32-bit WebAssembly and native targets.

State is split into three persistence classes:

- `Snapshot`: language-visible state that participates in snapshot and restore
- `InstanceScratch`: compiler-managed caches and transient per-instance data
- `ControlMirror`: dedicated physical storage named by exactly one control-output descriptor

Host buffer bindings should remain symbolic resources or instance scratch, not become persistent
pointer-sized state. This removes host pointer width from the portable state contract.

Physical storage for every state slot is zero-initialized before the MIR `init` entry runs. The
producer may omit redundant zero assignments, while dynamic and nonzero initialization remains
explicit in `init`. A control output identifies its mirror with a `StateId`; names are diagnostic and
never participate in storage resolution. Control-mirror places are readable but not directly
writable. Only `ControlOutputStore` in the process entry may mutate them, preventing init, event, or
user functions from bypassing the host-visible control-output operation.

Snapshots use a packed little-endian logical layout containing only `Snapshot` slots, in
deterministic MIR state order, with no target ABI padding. Control-output mirrors and
`InstanceScratch` slots are omitted. Restore first resets physical state to its initialized image
and then overlays the packed persistent slots, so transient caches cannot leak across restore and
the snapshot contract is independent of a backend's native alignment and byte-order choices.

AOT sidecar snapshot format 1 serializes each scalar element independently in little-endian byte
order: floats use their IEEE-754 bits, signed integers use two's-complement bits, and booleans are
one byte containing `0` or `1`. The persistent-state manifest records the element size, packed
snapshot offset, target-layout physical offset, and byte size of every included segment. An AOT
host must preserve the complete post-`init` physical state image; restore copies that image first,
then decodes and overlays the manifest's persistent segments. Processor descriptor format 3 carries
this manifest and the explicit `little_endian` / `post_init_physical_state_image` contract.

## Operations

Scalar calculations use explicit unary, binary, comparison, cast, and intrinsic rvalues. Memory and
host interaction use explicit operations:

- loads and stores through typed places
- input and current-frame audio-output loads, plus audio/control output stores
- interface-buffer and function-buffer-reference load/store and metadata queries
- immutable constant-data loads
- checked slice construction, indexed load/store, length, fill, and defined overlap handling
- direct calls with already-bound arguments
- structured `if`, `loop`, `break`, `continue`, and return

Indexed operations carry `Clamp`, `Trap`, or `Unchecked` bounds behavior. A backend never infers
safety semantics from a function name.

`make_slice` applies its bounds mode to the complete `(start, len)` range. Clamp normalizes the start
to `0..=source_len`, negative lengths to zero, and the length to the remaining range. Trap rejects an
invalid component. Unchecked requires the producer to prove the complete range. Empty slices are
valid, including a one-past-end empty view, but every indexed operation on an empty slice traps
because there is no element to clamp to.

`SliceElement` is only a scalar-reference argument. Fixed-array subreferences use `ArrayWindow` for
a fixed-array place or `SliceWindow` for a slice descriptor. The required window length comes from
the callee parameter. `SliceWindow` additionally requires unit stride; checked modes trap rather
than reinterpret a non-contiguous descriptor.

Slice copy is memmove-safe for contiguous or equal-stride overlap. Overlapping unequal-stride views
trap deterministically; MIR does not imply an unrepresented realtime scratch allocation.

Math intrinsics express Onda semantics, not a target implementation. LLVM may map an intrinsic to
LLVM IR or libm; WebAssembly may map it to a native instruction or an Onda-supplied math function.
In particular, `fma` requires one correctly rounded product-plus-add operation: a backend must not
replace it with separately rounded multiply and add instructions. Because core WebAssembly has no
scalar FMA or transcendental instructions, the Binaryen backend links the required pure-Wasm
helper closure into the generated module and then optimizes the combined program. This is target
legalization below MIR: it introduces no host import and does not weaken f32/f64 semantics.

## Scheduling

The MIR producer, rather than each backend, owns Onda scheduling semantics:

- block-pre, sample, and block-post ordering
- process invocation and block-boundary behavior
- logical frame iteration
- top-level and proc oversampling
- proc-array active-slot block hooks
- event execution order
- parameter clamping and update hooks

The process MIR therefore describes the canonical process loop. Backends translate that loop; they
do not independently recreate it from `TypedProgram` regions. The current schema's process entry has
exactly three ordered `i32` value parameters: `(start_frame, frames, flags)`. BEGIN gates block-pre,
END gates block-post, and the sample loop runs local frames `[0, frames)`. `process_frame(offset)`
is the only operation that can produce an audio-I/O frame. It traps unless `0 <= offset < frames`,
then yields `start_frame + offset` against full-block base pointers. Structured dataflow validation
requires a reaching `process_frame` definition to dominate every audio load/store frame use and
rejects any path on which that local is unassigned or overwritten.

Each base-rate sample is also an explicit output transaction: all audio outputs begin as zero-valued
locals, sample code mutates those locals, and MIR commits every output exactly once at the end of the
sample. This preserves unwritten-output semantics while exposing a register/SSA-friendly store
frontier to LLVM, Binaryen, and future backends.

The ABI validates `0 <= start_frame <= block_size`, `0 <= frames <= block_size - start_frame`, and
rejects flag bits other than BEGIN and END. The flags are independent scheduling events: they do not
imply particular positions, and the ABI maintains no continuity cursor. Zero-frame calls are valid,
including at `start_frame = block_size`; their gated block hooks still run. A host may submit a full
block in one call or preserve state across any legal segmentation. The reference AudioWorklet keeps
a host-side compile-block cursor and emits BEGIN/END only at the boundaries it schedules, allowing
Web Audio render quanta to differ from `CompileConfig::block_size`.

This is essential for numerical equivalence between LLVM and WebAssembly.

## Validation

Every MIR program is validated before optimization and after every backend-neutral pass pipeline.
The validator checks
schema/config validity, ID integrity, entry-point roles, call/result arity and types, argument
passing modes and types, assignment and store types, resource references, source references,
one-to-one init/process/event ownership, canonical entry signatures, function-kind resource
capabilities, canonical checked audio frames, return arity and types, boolean branch conditions,
integer indices, writable places, projection validity, resource access modes, intrinsic domains and
arity, structured loop control, reachable fallthrough from result-bearing functions, structured
definite assignment, process-frame dominance, finite ordered numeric interface ranges containing
their defaults, interface-name uniqueness, explicit one-to-one control mirrors, complete checked
slice/window contracts, fixed-array and aggregate signed-i32 size limits, recursive aggregate
rejection, constant-data element-count and logical-byte-size limits, and acyclic realtime call
graphs.

`ValidatedProgram` retains proof of these backend-neutral invariants. It does not promise that a
particular backend implements every valid capability; target legalization remains a separate,
explicit check. `OptimizedProgram` additionally proves that the shared MIR cleanup pipeline reached
its fixed point. Raw `Program` entry points must validate and optimize before delegating to backend
codegen.

Unchecked bounds are a separate producer proof, not an assertion a serialized program may make
about itself. The safe `validate`, `validate_owned`, `from_json`, and `from_messagepack` entry points
reject every reachable `BoundsMode::Unchecked` operation. Onda's semantic lowerer may use the
explicit unsafe trusted-producer constructors only after it has proved those accesses while
constructing MIR. That provenance is retained by `ValidatedProgram` and revalidated without being
downgraded after every shared pass. Raw LLVM and Binaryen APIs therefore cannot accidentally turn
untrusted unchecked indexing into memory unsafety; a trusted backend boundary must be named at the
call site and document the producer whose proof it accepts.

Invalid MIR is always a compiler bug and must be reported as an internal diagnostic with source
context where possible.

## Backend-neutral passes

MIR stays structured and non-SSA while a shared pass pipeline performs cleanup useful to every
backend. The pipeline propagates constants and immutable local copies through structured control
flow, merges identical branch facts, folds target-independent scalar operations and exact simple
intrinsics, applies integer-safe algebraic identities, performs local value numbering for pure
scalar expressions, simplifies constant branches, removes unreachable block tails, deletes unused
nontrapping pure temporaries, removes proven redundant all-bits-zero writes from the straight-line
prefix of pre-zeroed `init` state, and compacts local IDs. Floating-point identities are not applied
under the strict profile: NaNs, signed zero, and signaling behavior make transformations such as
`x - x -> 0` invalid.

The process pass may cache a small working set of scalar state slots in locals, loading once per
segment and committing writable values once at the end. The portable budget is deliberately capped
at eight scalars so a large process loop does not acquire a PHI web or Wasm spills that a target's
register-pressure model would reject. Control mirrors, projected aggregates, and slots whose
transitive callees also access the same state directly remain in memory to preserve alias
coherence. Reference calls keep their aliasing semantics; a backend that cannot take the address of
a register local must legalize that case explicitly.

Loop-written locals and locals passed by read-write reference invalidate propagation facts. Memory
and descriptor loads remain outside common-subexpression elimination until provenance proves their
source. The zero-store analysis stops conservatively across calls, structured control flow, or
unknown aliases. `canonicalize` performs one validated structural round. `optimize` runs its trusted
monotonic cleanup to a fixed point, validates the completed pipeline once, and returns `PassStats`
with an opaque `OptimizedProgram`.

`analysis.rs` exposes call-transitive logical effect summaries and conservative integer ranges.
Effects distinguish state, interface parameters, audio I/O, external buffers, constant data, event
payloads, indirect descriptors, and per-reference reads/writes without encoding a target ABI. Range
facts include the segmented-process contract, interface declarations, constants, and operations
that cannot overflow. These analyses are backend inputs: they let LLVM attach memory and range
attributes today and give future Wasm/native passes one shared place for alias, trap, and
vectorization legality. Aggregate read-write references remain conservative when converted to
descriptors, so LLVM never receives a `readonly` promise that descriptor provenance has not proved.

These passes do not replace LLVM or Binaryen optimization. They keep portable MIR deterministic,
remove producer artifacts before backend legalization, and prevent basic code quality from depending
on one backend's optimizer.

LLVM codegen carries MIR facts into function, parameter, and call-site attributes: memory-free or
read-only functions, reference read/write direction, no-capture/non-null/dereferenceable pointers,
non-throwing/non-allocating behavior, and constant ranges for process segment arguments. It then
applies the standard target-aware O3 pipeline, including LLVM's ordinary loop and SLP vectorization
decisions. MIR does not inject loop hints or override those target heuristics. The output-transaction
form and alias metadata expose independent work to the backend, while another backend remains free
to choose the best target strategy for the same operations.

## Production construction boundary

`onda_semantics::lower_program_to_optimized_mir` lowers a fully specialized `TypedProgram`, validates
it, runs the shared fixed-point cleanup, and returns the opaque `OptimizedProgram` consumed by
production codegen. Construction happens in a fresh program, so an unsupported residual produces
errors rather than a partial artifact. The boundary is available from the CLI:

```bash
onda compile examples/foundations/sine.onda --emit mir
onda compile examples/foundations/sine.onda --emit mir-json --output sine.mir.json
onda compile examples/foundations/sine.onda --emit mir-messagepack --output sine.mir.msgpack
```

The lowering owns:

- deterministic scalar and fixed primitive-array input, audio output, control output, parameter,
  range/default, and state IDs
- persistent scalar, scalarized tuple, and fixed primitive-array state with `init` assignments
- immutable primitive constant arrays with typed, bounds-explicit loads
- symbolic external buffers with safe/unchecked reads and writes plus length/channel/sample-rate metadata
- logical buffer-reference function parameters, including forwarding, metadata, mutation, and slicing
- scalar, fixed primitive-array, and dynamic primitive-slice event interfaces with direct handler IDs
- top-level parameter/input clamp rewrites already produced by semantic analysis
- BEGIN-gated block-pre, a guarded `0..frames` sample loop with checked `process_frame` audio
  addressing, transactional output locals, and END-gated block-post
- direct calls from runtime sections into the reachable user-function closure
- exact propagation of the semantic analysis sample rate and block size
- resolved scalar value parameters, scalar/no-result functions, and scalar locals
- fixed primitive local arrays with safe/unchecked indexing, mutation, length, and slicing
- primitive slice aliases and function parameters with inferred read-only/read-write access
- negative/clamped slice bounds, indexed slice access, fill, defined-overlap copy, and buffer slices
- tuple returns as ordered MIR multi-values, including forwarding calls
- tuple parameters, returns, and locals as ordered scalar components, with destructuring and constant indexing resolved
- data-struct state flattened into backend-neutral scalar, tuple-component, and fixed-array storage
- data-struct parameters and methods as ordered scalar/fixed-array references, including nested struct forwarding
- arrays of data structs as structure-of-arrays storage, including constructor lists, broadcast construction,
  direct indexed field access, retained element aliases, and structure-of-slices function parameters
- scalar slice-element references plus checked contiguous fixed-array windows into flattened storage
- direct indexed data-struct and processor-array elements as typed call operands, with the index
  evaluated and clamped once before resolved structure-of-arrays references are emitted
- recursive processor-array shapes, including fixed paths, indexed selectors, retained aliases,
  fixed-array defaults, and `.len`, with nested dispatch resolved before backend lowering
- direct specialized calls with positional, named, and default arguments normalized to parameter order
- explicit casts, arithmetic, bitwise operations, comparisons, and math intrinsics
- short-circuit `&&` and `||` lowered to structured `if`
- `if`, `while`, directional/inclusive `for`, `break`, `continue`, and value returns
- deterministic function specialization by sample-rate/block-size context for contextual `SR`/`BS`
- source spans and deterministic structural type interning

`onda_semantics::lower_scalar_user_functions_to_mir` remains available as the narrower transactional
API for focused compiler tests and MIR tooling.

Semantic analysis retains each function's resolved scalar-local table, including declarations that
only occur inside nested control flow. It also records whether a function produces a value and
proves that every reachable path in such a function returns; no-result functions therefore do not
acquire a synthetic `f32` MIR result, and neither backend supplies an implicit fallthrough value.

The complete-program API supports ordinary generated processor functions, recursive processor
arrays, direct indexed processor instances, and canonical top-level and per-processor oversampling.
Oversampling expands to fixed MIR arrays, structured substep loops, explicit persistent sinc-filter
taps, interpolation, and decimation; backends do not infer DSP scheduling from semantic metadata.
The source DSP body is represented once regardless of the oversampling factor. Sinc stages with
only one or two iterations retain constant-index small kernels for scalar replacement, while larger
stage traversals use structured strided loops so high factors do not clone the filter graph.
User functions accept resolved scalar, scalar-tuple, primitive-slice, buffer-reference,
data-struct, and data-struct-array parameters. Explicit generic scalar contracts are specialized in
semantics, as are context-independent inferred `f64` calls; an unresolved scalar specialization
fails explicitly instead of being guessed by MIR.

Tuples have no target ABI layout in MIR. A source parameter or return `(f32, i32)` becomes two
ordered function parameter or result types. A direct call supplies or names two scalar values. This
maps to ordinary WebAssembly parameters and multi-value results directly; LLVM lowering may use an
aggregate or an ABI-specific convention without changing MIR.

## Determinism and compatibility

MIR uses indexed vectors and numeric IDs so construction and dumps are deterministic. Hash maps may
be used while building MIR, but their iteration order must not define emitted IDs.

MIR JSON and named-field MessagePack carry the same versioned machine-readable schema for non-Rust
backends. JSON is the inspectable interchange and diagnostic form; MessagePack is the compact
production transport used by the browser compiler and Binaryen backend. It avoids a second schema
and is substantially smaller without making backend code depend on Rust layouts.
Consumers must reject unknown `schema_version` values. Compatible additions retain the version;
incompatible serialized changes increment it. The current schema includes explicit control-mirror IDs and
persistence, checked slice construction, safe scalar/array reference windows, and serialized
function origin/inline hints. It uses canonical decimal-string encoding for `i64` and exact
hexadecimal IEEE bit patterns for non-finite `f32`/`f64`. These encodings avoid JavaScript number
corruption and JSON's lack of NaN/infinity literals. The safe JSON and MessagePack decoders return
`ValidatedProgram` and reject unchecked producer claims. Optimized-program serializers and the
unsafe trusted decoders make the compiler-owned boundary explicit. `onda compile --emit mir-json`
prints or writes JSON; `--emit mir-messagepack` requires `--output`.

`onda_mir::format_program` remains the stable human-readable inspection format used by `--emit mir`.
The formatter and both serialized transports have round-trip coverage.

## Production backend status

The MIR migration has landed. The public JIT API has a single path: it lowers the specialized
program to optimized validated MIR and then uses the MIR-native LLVM ORC implementation. Targeted
LLVM IR and object emission use the same MIR lowering. Runtime and AOT metadata are derived from the
MIR interface together with the physical offsets selected by codegen, so `TypedProgram` is no
longer a semantic side channel for production JIT, object, state-layout, or host-interface data.
The AOT format-3 processor descriptor includes the packed persistent-state segment manifest needed to implement
snapshot/restore without confusing packed offsets with the target's physical state layout.
It also records the resolved LLVM pointer width, byte order, data layout, pointer model, and
relocatable-object integration profile. The logical entry points are specified once in
[`PROCESSOR_ABI.md`](PROCESSOR_ABI.md); target triples select the platform representation rather than
creating a separate Wasm ABI.

The former direct `TypedProgram`/frontend-AST-to-LLVM implementation has been removed. Native JIT,
LLVM IR, object emission, runtime metadata, and AOT metadata all consume the same validated MIR
contract, leaving no parallel LLVM semantics or layout pipeline to drift from the other backends.

## Browser backend

The browser path is current end to end. `onda_compiler_web` compiles one in-memory source or a
virtual multi-file project, resolves the embedded standard library, and returns optimized,
validated MIR in the current schema as compact MessagePack for production or JSON for inspection, with
structured diagnostics. `packages/onda_binaryen_web` accepts either transport and lowers it with
Binaryen.js to an executable DSP Wasm module plus host metadata. Its deliberately named
`compileTrustedMir` entry accepts only output carrying the complete `onda_mir` producer proof; the
JavaScript backend does not pretend that rejecting unchecked bounds alone validates arbitrary MIR.

`packages/onda_wasm_compiler` is the product-facing composition of these layers. It initializes the
embedded frontend Wasm, checks the producer/backend schema versions, keeps the trusted transition
inside the package, and exposes asynchronous source/project APIs, a browser worker, and the
`onda-wasm` build-time CLI. The low-level packages remain independently testable backend boundaries.

The current-schema backend consumes explicit control-mirror state, checked `make_slice`, fixed-array and
slice reference windows, and serialized function attributes. It covers scalar and fixed-array
storage, tuples and multi-value returns, primitive slices, dynamic-slice events, buffers, flattened
data structs, recursive processor arrays, structured control flow, constant data, oversampling, and
segmented audio. Address-taken scalar locals are legalized through per-function scratch slots around
reference calls, so shared state promotion remains portable. Contiguous slice fill uses WebAssembly
SIMD with a scalar tail, while same-representation contiguous slice copy uses bulk-memory
`memory.copy` and therefore retains memmove overlap semantics. Strided cases keep the scalar,
direction-aware implementation. Binaryen validates and optimizes the emitted module.

Binaryen O4, strict arithmetic, SIMD, and ordinary inlining heuristics are the defaults. Relaxed
floating-point optimization is an explicit `fastMath` option. Inlining functions that contain loops
is also explicit rather than default: measurements improved the oversampling case but regressed the
language and saturator cases, so the backend does not force that global heuristic. Binaryen's
process-global optimization controls are restored after every compilation.
O4's more expensive IR flattening improved three of the four affinity-pinned backend workloads and
left the fourth effectively unchanged; the browser pays that extra compilation cost once per
artifact. StackIR generation remains disabled because it did not improve the complete workload
matrix.

The source-driven workflows exercise current compiler MessagePack output rather than hand-authored
legacy MIR:
`npm test` runs backend and AudioWorklet fixtures, `npm run test:onda` runs the real-source slice,
LLVM/Binaryen render parity, and the internal-Wasm FMA oracle, and `npm run test:parity` runs the
differential render subset. The differential suite covers full and segmented blocks, zero-frame
hooks, events, numeric edge rules, the complete f32/f64 math surface, packed snapshots/restores,
buffers, slices, processor arrays, and oversampling.
`npm run test:corpus` discovers all checked-in examples and positive backend fixtures and requires
each source to produce current-schema MIR and valid Binaryen WebAssembly. The source-driven
commands intentionally require the native Rust/LLVM Onda build; the backend fixtures and embedded
compiler asset build do not. The checked-in compiler playground performs Rust semantic compilation
and Binaryen O4 optimization in a module worker and can export the complete Wasm module with its
integrity-checked descriptor. The separate AOT sample-player example performs both compiler stages
before deployment and ships only that descriptor, module, Web Audio adapter, and audio asset. Both
use `packages/onda_webaudio` for AudioWorklet playback.

## Realtime floating-point environment

The native runtime configures every processing thread through the backend-independent
`onda_realtime` utility. On x86 this enables denormals-are-zero and flush-to-zero once per thread.
Decaying feedback and oversampling filters otherwise enter the subnormal range and can suffer large,
core-dependent stalls even when their generated LLVM is otherwise optimal. CPAL callbacks, offline
runtime processing, daemon rendering, and benchmarks now share this policy instead of relying on a
particular host to remember it. MIR still defines strict-width arithmetic; the realtime execution
environment deliberately treats inaudible subnormal tails as zero.

Core WebAssembly still has no scalar fused multiply-add instruction. The backend therefore links a
strict software FMA from its internal pure-Wasm math kernel. It matches one-rounding f32/f64 FMA
semantics without a JavaScript import or allocation, but native LLVM can still select hardware FMA;
dense per-sample FMA may therefore remain a browser performance limitation rather than a semantic
mismatch. The reproducible development comparison and its measurement caveats are recorded in
[the backend benchmark report](BACKEND_BENCHMARKS.md).

## Backend invariants

- Production LLVM JIT/AOT and browser WebAssembly codegen consume the same optimized MIR
  contract.
- Native and WebAssembly differential renders remain a required regression gate as the language and
  MIR evolve.
- `--emit mir` remains deterministic and useful for debugging compiler behavior; current-schema JSON and
  MessagePack remain checked, versioned views of the same backend interchange form.
- Adding a backend must not require reimplementing Onda language semantics or scheduling.
