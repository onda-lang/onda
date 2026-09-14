# Structured data proposal

Status: locked design; implemented.

## Implementation progress

The implementation supports nominal struct and fixed primitive/struct-array returns, runtime
constructors and arrays, independent typed copies, replacement through existing references, and
top-level persistent initialization from helpers. Nested defaults, tuple fields, fixed array
fields, local branch-selected aliases, and ranged field stores share canonical leaf storage.
Typed primitive and struct slices preserve permissions and captured bounds; struct slice copy
and fill handle overlap, empty selections, and fitting prefixes. Fixed declarations accept slices
with proven exact lengths. Helper permission inference follows reference origins through aliases,
branch joins, and transitive calls. Fixed parameter shapes survive into MIR.

Both backends consume explicit result references and prepared invocation scratch. Grouped leaf
copies validate all potentially failing overlaps before writing; an explicit MIR producer proof
removes impossible checks for canonical leaves. Canonical field separation avoids redundant copy
scratch. Local and persistent struct initialization share one implementation.
Struct helper tensor fields use the common slice descriptor ABI, retaining stride through mutable
and read-only forwarding. Fixed shape contracts remain explicit at helper entry, and shared MIR
bounds proofs eliminate checks for statically bounded reads and writes.

Top-level block-owned data and views now survive process segmentation, intervening events, and
snapshot restoration. Selection reconstruction retains scalar coordinates and branch choices;
backing storage is persistent, and snapshots contain no slice pointers. Logical struct-slice
lengths are derived from restored tensors. External-memory views cannot cross process boundaries.
Selection storage depends on view shape, not the number of array elements.

Persistent init selections now reconstruct against each runtime instance and follow full/resettable
initialization. Fresh slice backing participates in that lifecycle; aliases into expired init-local
storage and pinned views are rejected. Procs retain owned block data and selections on each instance,
including nested proc arrays. Tasks retain owned fixed data and eligible views across yield, including
views into persistent init selections. Proc helpers and tasks share storage classification, binding
rewriting, and symbolic selection planning; normal data replacement initializes continuation storage.

Native tests cover relocation, reset, branch selection, and independent instances at O0 and O3.
Native/Binaryen parity includes segmented processing, snapshots, events, and task suspension across
these scopes. A 4-to-4096-element check verifies that retained-view metadata and helper/local counts
remain independent of array extent.

Structured events and delegates now support nominal structs, tuples, fixed arrays, and runtime
struct slices through synchronous read-only views. Recursive host schemas include nested constant
defaults and integer domains. A compiler-free tensor planner drives aggregate layout, MIR message
validation, aligned input preparation, serialization, and native host codecs; JavaScript implements
the same wire contract with native/Binaryen parity coverage.

Processor ABI and descriptor version 6 pass an explicit event input descriptor. Both backends
preflight complete packed input and workspace capacity before any handler or output mutation,
normalize bools and ranged integers, and prepare aligned native tensors without dispatch allocation.
Rejected input leaves the instance usable. Host publication captures complete little-endian records;
internal forwarding retains live views. Native scheduling reuses prepared encoders, and WebAudio
encodes values before transferring bytes to the worklet. Native C/Rust hosts can reserve larger
workspace outside realtime execution.

The native event editor accepts nested JSON values, including decimal strings for exact i64.
Language/ABI documentation, formatter support, examples, browser staging, and raw C integration
cover the completed surface. Acceptance includes malformed and capacity-rejected raw calls,
normalization at i64 extremes, mixed scalar alignment, nested defaults, live subscribers, immutable
host records, and fixed/dynamic structured host round trips. The full Rust workspace and browser
unit suites pass; parity covers 19 scenarios with 320 bit-exact and 12 approximate samples.

## Purpose

Make structs and arrays of structs useful in ordinary DSP code, events, and delegates with
one small set of rules. Structured messages are part of the core, including runtime-length
struct slices. Structured params remain outside this proposal.

The model extends Onda's existing aggregate references without a `copy` keyword or
ownership-based return restrictions. Declaring storage, passing references,
and copying contents are ordinary operations; users do not track whether a value originated
in a constructor, a parameter, or a locally owned root.

## The model

Scalars and primitive-only tuples retain value semantics. Structs and arrays follow these rules:

| Operation | Meaning |
| --- | --- |
| `view = existing` introduces a name | Alias the existing aggregate, with the same permissions. |
| `value: T = expression` declares data storage | Allocate independent fixed storage and initialize its contents from the expression. |
| `value: T` | Allocate independent fixed storage initialized from the type's defaults. |
| `view: T[] = expression` introduces a name | Bind a slice view, preserving source permissions; an initializer is required. |
| `value = constructor_or_returned_value` introduces a name | Bind the new result storage, inferring its type; array literals also create new storage. |
| `destination = expression` assigns an existing aggregate | Copy contents into the destination; preserve its storage identity. |
| `return expression` returns fixed data | Return an independent value, regardless of the expression's origin. |
| Pass a struct, array, or slice to a `def` | Pass access to its storage; infer required write permission from the helper body. |
| Pass a struct, array, or slice to an event or delegate | Pass read-only access for synchronous dispatch. |

A typed **fixed-data declaration** creates storage. A slice declaration binds a view and
requires an initializer; it never allocates independent element storage. An annotation on a
function or event parameter describes its accepted type; it does not change argument passing.

The storage-creating declaration must introduce a new name under Onda's scope rules. Adding a
type annotation to an existing alias cannot detach it or upgrade its permissions; diagnose such
a redeclaration. To obtain independent data, declare another name.

An established aggregate binding never redirects. Assigning to a def parameter or method
receiver writes through it, just as assigning one of its fields does. Assignment to existing
storage behaves identically for a named source, constructor, or function result.

```onda
struct Note:
  frequency: f32 = 440.0
  velocity: f32 = 1.0

def transpose(note: Note, semitones: f32):
  note.frequency *= pow(2.0, semitones / 12.0)

def first(notes: Note[]) -> Note:
  return notes[0]

init:
  notes: Note[4]

sample:
  selected = notes[0]           # Alias element zero.
  saved: Note = selected        # Independent contents.
  transpose(selected, 12.0)     # Changes element zero.
  transpose(saved, -12.0)       # Changes only saved.
  selected = notes[1]           # Replaces element zero; keeps the selection.
  captured = first(notes[:])    # Independent returned value.
```

There is no `copy`, `ref`, `mut`, move operation, borrowed return, or source-level ownership
qualifier in this design. Independence means independent observations, not a mandatory physical
memcpy. The compiler may eliminate unobservable storage and copies.

The remaining distinction is intentional: an untyped binding to existing aggregate data is a
cheap alias; a typed fixed-data declaration requests independent storage. This follows the existing
use of `name: T[N]` to allocate arrays and keeps large copies visible at declaration sites.
Returning or replacing an aggregate can also copy large data; neither operation promises zero cost.

## Data shapes and construction

Support nominal structs, nested structs, fixed arrays of primitives or structs, and
primitive-only tuple fields. Resolve generic arguments and all owned array lengths before
storage planning. Reject recursive by-value layouts and overflowing sizes.

Do not add arrays of arrays, arrays of tuples, aggregate tuple elements, structural conversions,
or elementwise aggregate arithmetic here. Procs and buffers remain resources and cannot be
embedded in these data values. Slices and other references cannot be stored in struct fields.

Constructors and typed data declarations work in runtime locals as well as `init`. Defaults
initialize every field and array element independently. A data-array broadcast evaluates its
initializer once and fills independent elements; an array literal evaluates each initializer
once in source order. Aggregate constructor fields capture contents, not references.
Each field or element captures its contents before evaluating the next initializer, including
named fields supplied out of declaration order. Later initializer effects cannot change earlier
captures. An explicit initializer supplies the initial contents directly; it does not require
default-filling the destination and then overwriting it.

Select aggregate storage through named bindings. Bind a constructor or returned aggregate first
(`patch = make_patch()`), then select its fields or elements. Use intermediate aliases when a
selection needs further field/index composition; arbitrary postfix chains on call results are
outside this design. Preserve current clamped indexing and empty-slice failures. Selecting a view
evaluates and normalizes each selector once.

Whole-aggregate copies require the same nominal type, including generic arguments and fixed
shape. A typed declaration does not truncate or pad its initializer. Constructor scalar fields
retain ordinary contextual typing and numeric widening.

Broadcast initialization is an explicit exception to matching the whole array shape:
`items: Note[4] = Note()` captures one element and initializes four independent elements.
Whole-array replacement requires an array: `items = other_four` copies the exact shape,
while `items = Note()` is rejected. Use `items[:] = Note()` to fill existing elements.
A fixed-data declaration may copy from a slice only when its exact length is statically
proven to match. Runtime-length sources require explicit slice assignment; fixed initialization
does not insert a runtime shape assertion.

## Replacement and returns

```onda
def reset_note(note: Note):
  note = Note()

def replace_note(note: Note, replacement: Note):
  note = replacement

def duplicate(note: Note) -> Note:
  return note
```

The first two helpers modify the caller's object. The last returns independent contents.
Returning an input, a local alias, a nested field, or a selected array element follows the same
rule. There is no return-eligibility analysis based on ownership origin.

These forms both replace existing state and have the same copy semantics:

```onda
coeffs = make_coeffs(freq)
```

```onda
prepared = make_coeffs(freq)
coeffs = prepared
```

Evaluate a replacement's target and selectors once, then prepare the complete right-hand
contents before storing them. Existing aliases continue to observe the destination's storage.
Self-assignment and overlapping fixed aggregate replacement have snapshot semantics.
This is an observational rule: disjoint copies need no extra snapshot buffer. The content-copy
operation invokes no callbacks. Effects explicitly performed while evaluating the right-hand
expression remain visible and are not rolled back.

`state = transform(state)` must behave as if the function computed into independent result
storage before replacement. Forwarding the destination is an optimization requiring proof
that argument effects, aliases, nested observers, and failures cannot distinguish it.

Returns always have a fixed resolved data type. Returning an unsized slice is unsupported;
allocate a fixed result and use slice assignment when a dynamic source must be retained.

## Calls, permissions, and evaluation

Aggregate calls always pass references, independently of callee effects. A helper may read or
write as its body requires. Infer those requirements through projections and transitive calls;
never silently copy an argument to make a forbidden write legal.

An event's read-only payload can be passed to a read-only helper. To modify independent data,
declare local storage first:

```onda
event note_on(note: Note):
  shifted: Note = note
  transpose(shifted, 12.0)
```

Scalars and primitive-only tuples remain value arguments, including when read from struct
fields or payloads. A def may modify its local scalar or tuple parameter without changing the
caller. Direct writes to an embedded scalar or tuple still require access to its container.

Overlapping references are legal. Reads observe preceding writes, including effects through
other arguments, owner state, and synchronous handlers. Write permission does not imply
exclusivity or a backend `noalias` promise. Mutating algorithms must capture inputs they need
before overwriting possibly overlapping storage.
Cached loads and range proofs must account for writes through overlapping arguments and nested
calls. Read-only permission restricts access through that reference; it is not an immutability
or invariant-load guarantee about the backing memory.

Evaluate receivers and arguments in source order, including named arguments; rearrange only
prepared operands into parameter order. Primitive arguments capture values. Aggregate
arguments select live storage: a later argument may mutate data selected by an earlier one.
Constructors and typed storage initializers instead capture their contents at evaluation.

Every writable access retains its destination's store rules, including ranged integer fields.
A generic helper must preserve or specialize these rules. Existing param permissions and hooks
remain authoritative; reject new writable helper access into params where ordinary references
cannot express their hook contract. Never erase store rules through a raw primitive pointer.

## Arrays and slices

`T[N]` is fixed data; `T[]` is a view. Both primitive and struct elements use the same slice
rules. Helpers and messages may accept runtime-length slices without an authored maximum.

```onda
def prepare(incoming: Note[]):
  working: Note[8]
  working[:] = incoming[:]
```

Slice assignment copies the fitting prefix, `min(destination.len(), source.len())`, preserving
the destination tail. It does not resize either side or default-fill existing storage. Element
types must match, including nested fixed shapes. A fill such as `working[:] = Note()` captures
one element before writing and fills independent contents.

A slice binding fixes its selection and length. Use `view[:] = source[:]` for copying and
`view[i] = value` for element replacement. Bare assignment to an established slice binding is
rejected: there is no slice rebinding or second whole-slice replacement operation.

```onda
def replace(dst: Note[4], src: Note[4]):
  dst = src

def replace_prefix(dst: Note[], src: Note[]):
  dst[:] = src[:]
```

The parameter's declared fixed-array or slice contract determines the available assignment
operation, even when specialization knows a slice argument's actual length. An annotated
binding such as `view: Note[] = notes[:]` captures a view with the source's permissions;
`saved: Note[4] = notes` instead creates independent fixed storage.

Supported overlapping slice copies preserve source contents as if through temporary storage.
Retain the existing failure for overlapping unequal-stride views rather than introducing
runtime-sized scratch. Check every potentially failing struct leaf overlap before the first write;
canonical leaf copies carry a trusted MIR proof when equal strides make that check unnecessary.
This is a slice-operation rule, not a restriction on overlapping function arguments.

## Events and delegates are core

Events and delegates accept primitives, primitive-only tuples, structs, fixed arrays of
primitives or structs, and slices of primitives or structs. The same shapes work in proc
events, top-level events, delegates, and their `when` bindings.

All payload bindings are read-only. Scalars and tuples are captured values; structs, arrays,
and slices are live views for synchronous dispatch. Forwarding a payload passes the same
storage access, without copying its contents at every routing step.

```onda
struct Patch:
  voices: Note[4]

proc VoiceBank:
  init:
    patch: Patch

  delegate configured(applied: Patch)

  event configure(next: Patch):
    patch = next
    configured(patch)

  sample:
    out1 = 0.0

init:
  bank = VoiceBank()

delegate patch_applied(applied: Patch)

event configure(next: Patch):
  bank.configure(next)

when bank.configured(applied):
  patch_applied(applied)
```

The incoming event borrows its payload. `patch = next` captures independent state. Subsequent
forwarding borrows that state. Arrays of structs can also appear directly as `Note[4]` or
`Note[]` parameters; a wrapper struct is not required. Struct and struct-array parameters cannot
have defaults: they borrow caller-owned storage, so every call must provide a source. APIs that need
default behavior construct explicitly owned data in a separate no-argument helper or event and pass
that storage onward.

Handlers retain data by ordinary assignment into declared state, or by a typed local data
declaration. They cannot store a payload reference beyond dispatch. Forwarding and reading a
payload incur no implicit full-content copy. Forwarding cost depends on its view descriptor
(leaf/axis count), not its runtime element count; authored reads still cost the work they perform.

**Read-only does not mean frozen.** Another writable alias can change the source during
synchronous execution. Later handlers can observe those changes. This is the same reference
behavior as ordinary aggregate calls, not a separate event capture model. If stable contents
are needed, create independent storage before dispatch and avoid writable access to it during
the dispatch. Handlers may capture their own fixed data with a typed declaration.

Keep current declaration-order subscriptions, depth-first nested dispatch, and rejection of
recursive call/dispatch graphs. Publish a top-level host record before its local subscribers,
as today. The record captures contents at publication; later source writes do not change it.
Disabling host collection or dropping an overflowing record never changes internal execution.

A configuration handler may validate, calculate derived data, replace state, and then publish.
It is not an automatic transaction. Complete related state writes before invoking observers;
failures retain Onda's existing execution-failure behavior. Ordinary structured state does not
expose controls, trigger param hooks, or automatically maintain derived coefficients.

## Storage lifetime without an ownership surface

Owned data uses Onda's existing init, block, lexical-local, and task-continuation lifetimes.
A fixed-data declaration or a bound function result creates storage in its declaration's scope.
Branches and loops retain the existing visibility and initialization rules.
An alias to an existing local never promotes that local into persistent state. For example,
capturing init-local data in a persistent root requires a typed data declaration or a returned
value, rather than an alias whose source expires when initialization finishes.

A reference may remain in use while its binding is in scope and its backing storage remains
valid. Runtime selection does not shorten that lifetime. The compiler tracks origins
for storage safety and permissions, not to decide whether fixed data may be returned.
Bind a returned owner before selecting its fields or elements. Temporary arguments survive all
argument evaluation and the complete synchronous call. A typed slice binding directly initialized
from fresh array data retains that backing for its declaration scope, including persistent init
and block-carried storage. This lifetime extension applies to fresh temporaries, not aliases of
existing shorter-lived locals. These rules apply uniformly to every executable block, including
proc bodies, helpers, events, delegates, nested control flow, and task continuations.
An extended temporary in `init` is ordinary unpinned backing state, reconstructed when its
declaration executes, including `init(PRESERVE_PINNED)`. Its view cannot be pinned. To preserve
the backing data, declare an explicit owned root such as `pin patch = make_patch()` and then
select `selected = patch.voices[index]`. Snapshots include hidden backing roots and their selections.

At an `if` join, compatible views select the storage chosen by the executed branch, and write
access requires permission on every reachable origin. Independent branch results retain their
storage through the merged binding's scope. Returning either kind simply captures fixed data.
Views into loop-local storage cannot escape the iteration. Repeated execution never accumulates
retained allocations.

Runtime indexing into struct arrays declared in `init` already selects aliases. Extend that
behavior consistently across scopes rather than restricting selections to one execution entry:

```onda
init:
  notes: Note[4]
  index: i32 = 0
  initial = notes[index]

block:
  selected = notes[index]

  sample:
    out1 = selected.frequency
```

`initial` retains the element selected during initialization. `selected` evaluates and clamps
its selector once in block-pre and retains that selection throughout the logical block,
including process segments and block-post. Later changes to `index` do not redirect either
alias; writes to the selected element remain visible through it. Copying the element or
reevaluating the selector is not an equivalent substitute.

- Persistent references retain symbolic storage identity and captured normalized selectors,
  slice lengths, or branch choices as needed. Resolve addresses against the current instance;
  never store raw addresses in snapshots. Descriptor size depends on shape, not element count.
- A runtime branch may select between compatible roots when every reachable backing lifetime
  covers subsequent uses. Capture which storage was selected without copying its contents.
- Captured selections follow their binding's initialization and snapshot lifecycle: init
  selections are recreated when their declarations run; block-pre selections are refreshed
  once per logical block. Replacing backing contents does not redirect an alias. An alias has
  no independently pinnable state.
- Owned block-carried data and eligible views survive segmented processing and intervening
  events. Task views may survive `yield` when their backing storage survives suspension;
  capture their selections in the continuation and reconstruct addresses on resumption.
- Reject references whose backing storage expires before a use, including escapes from
  loop-local storage. Payload views expire at dispatch return. A helper's reference parameter
  is invocation-scoped. Views into external buffer memory cannot survive boundaries where that
  memory may be rebound or retired; resource-identity buffer aliases retain their existing rules.

These lifetime rules are checked at compile time and do not depend on the host's actual
segmentation choices. Temporary lifetime extension does not extend borrowed payload or external
memory lifetimes. Retained backing storage and selection descriptors participate in storage
planning; repeated execution introduces no accumulating allocations.

## One layout and bounded execution

Use one recursive SoA leaf/shape planner for materialized internal aggregates and prepared
host payloads. Traverse fields in declaration order, tuple components in element order, and
array axes outermost first. Each primitive leaf tensor is row-major: `Complex[4]` stores real
and imaginary regions separately; an array of structs with `Complex[8]` fields adds an outer axis.

Views carry the selected leaf offsets and strides. A fixed logical array shape does not imply
contiguous selected storage. Helpers must accept the same views from local, persistent, result,
and prepared host storage without per-call repacking. Keep target alignment and wire scalar
encoding separate from logical shape; derive all operations from the shared planner.

Keep array extents in tensors rather than expanding elements into function arguments or view
metadata. Unsized data-array helpers should share a length-independent implementation across
source lengths. Read-only helpers can share code for mutable and read-only callers. Specialize
only where resolved types, fixed shapes, or store contracts change generated behavior; permission
alone does not require duplicate machine code.

Plan fixed result and local scratch storage against the acyclic call/dispatch graph. Distinct
simultaneously live results need distinct storage, including `inspect(make(), make())` and
results used by nested handlers. Reuse slots only after their values and views are dead.
Block-carried data and task continuations are state, not reusable invocation scratch.

No callback-time allocation, memory growth, or runtime-sized stack allocation is introduced.
Use prepared instance scratch as the conservative baseline, with scalar replacement and safe
copy elimination as optimizations. Use loops or compatible bulk copies for large arrays;
do not emit a separate instruction sequence for every array element.
Reuse compatible return storage directly for a newly bound result or typed declaration when
safe; a chain of value-returning helpers need not materialize a copy at every return. Copies
must respect leaf layout and store domains before being replaced with bulk memory operations.
An unreached branch performs no aggregate initialization or copying. Report checked target
layout limits and maximum planned live storage; do not substitute huge stack frames for planning.
The portable limits are 512 canonical primitive leaf tensors per aggregate, 1,024 positional
parameters per lowered function, and 32,768 lowered locals per function. Tensor extents do not
consume leaf slots because they remain fixed-array dimensions rather than expanded ABI entries.

Sample-rate construction, copying, and returns are legal. Their execution cost belongs to the
programmer; optional size/storage reports can expose costs without forbidding ordinary code.
Permission analysis must not turn reference calls into hidden copies.
Runtime-length message execution scales with the admitted payload length and authored handler
work. Prepared host capacity bounds admitted storage; this is not a constant-time dispatch promise.

## Host transport

Use the same recursive payload schema for events and delegates: nominal types, ordered fields,
scalar encodings, domains/defaults, fixed lengths, and slice element shapes. Host wire leaf order
matches canonical SoA; wire bytes contain no pointers or native padding. A slice carries a checked
nonnegative `i32` length followed by its leaf regions. Later parameter offsets may be dynamic.

Host input is validated and normalized once into caller-prepared aligned SoA workspace before
handlers run. Normalize ranged fields exactly as ordinary construction does and do not modify
host input. Start with this single preparation path; direct borrowing of compatible host bytes
is an optional future optimization, not a second required execution representation.

The raw entry receives actual payload length and workspace capacity. Shared schema-based sizing
reports required workspace from payload shapes/lengths, independently of values needing
normalization. Hosted native and browser adapters provision this storage outside realtime
execution. Check offsets, arithmetic, complete payload boundaries, and capacity before dispatch.
Input/workspace/output storage must obey the ABI's disjointness and lifetime requirements.

Malformed or oversized input runs no handlers, preserves persistent state and existing output
records, and leaves the instance usable. Distinguish input rejection from failure during handler
execution consistently in raw, hosted native, and Wasm paths. Keep prepared workspace alive through nested
forwarding and separate from persistent snapshots and output batches.

Outgoing delegates serialize publication-time contents into the existing prepared output batch.
Emit a complete record or drop it whole with overflow reporting. Hosts consume or copy records
before reusing the batch. No source-level maximum slice length, dynamic dispatch allocation,
or automatic persistent state-query interface is added.

## Delivery and acceptance

The feature is complete only when local DSP data and structured events/delegates work together
in native LLVM and Binaryen. Implement in this order without treating messages as optional:

1. An independently usable local-DSP milestone in both backends: common named places/views,
   runtime construction, typed independent storage, replacement,
   returns, and primitive/struct slices. Prove a coefficient-returning helper and a mutating
   spectral helper. Include persistent selections and segmented block-carried data/views from
   the start, with lifetime checks and snapshot-safe selection descriptors.
2. Synchronous structured events/delegates, recursive host schema, input preparation, output
   serialization, and native/browser adapters. Prove configuration -> state -> delegate -> host.
3. Integrate language documentation, formatter, diagnostics, LSP, examples, and stdlib usage.

Focused acceptance cases:

| Case | Required result |
| --- | --- |
| Alias an element; replace its contents; observe it through another alias | Storage selection stays fixed; observers see the replacement. |
| Declare `saved: Note = source`; mutate source | Saved contents remain independent. |
| Initialize by broadcast, replace a fixed array, or fill its slice | Evaluate broadcast once; require exact array shape for replacement; reject scalar/struct whole-array replacement. |
| Initialize fixed storage from a slice | Accept only a statically proven exact length; reject dynamic or mismatched lengths. |
| Annotate an already-bound alias as an attempted independent declaration | Reject redeclaration; never detach or upgrade the alias. |
| Declare `view: Note[] = notes[:]`, or omit its initializer | Preserve selection and source permissions; reject the missing initializer. |
| Replace through fixed-array and slice parameters | Fixed arrays permit whole replacement; slices require slice assignment regardless of specialization. |
| Return an input, local alias, nested field, or array element | Same fixed-value return rule in every case. |
| Extract a returned expression into a named local before assignment | No new keyword or ownership restriction. |
| Select a field of a temporary; pass two results from one helper | Backing storage survives every use without collisions. |
| Join owned and borrowed branches; read, write, and return the result | Correct selected storage, conservative permissions, ordinary value return. |
| Evaluate constructors/literals whose later initializers mutate earlier sources | Earlier fields/elements retain their captured contents. |
| Self-multiply complex values; use clamped aliases to the same element | Ordered aliasing, correct captured operands, no exclusivity assumptions. |
| Replace a destination that aliases a function input | Observationally equivalent to independent result storage. |
| Copy short/long/empty slices; copy supported overlapping slices | Fitting prefix, preserved tail, correct overlap; invalid overlap fails before writes. |
| Write a ranged field through a helper or whole-object replacement | Preserve its store domain. |
| Forward a struct, fixed struct array, or struct slice through events/delegates | Read-only live views, preserved shape/length, no payload-sized per-hop copy. |
| Mutate a payload source through another legal alias between subscribers | Later reads see writes; an earlier host record remains unchanged. |
| Capture a payload into state; mutate its source afterward | State remains independent. |
| Run an event between process segments with live owned block data | Event scratch does not corrupt carried data. |
| Select an init-owned element in init or block-pre; change the selector and mutate the element later | Preserve the captured selection across uses and segments; observe changes to its contents. |
| Retain a runtime-selected view across `yield` with surviving backing storage | Preserve selection and permissions; reconstruct the view on resumption. |
| Snapshot and restore runtime selections, including a branch-selected root | Restore root choice and coordinates against the restored instance without retaining raw addresses. |
| Bind fresh array data directly to a typed slice in init or block-pre | Retain its hidden backing storage for the binding's lifetime. |
| Reinitialize a view into an init temporary, or into an explicitly pinned root | Recreate ordinary hidden backing data; preserve explicitly pinned data under `PRESERVE_PINNED`; recreate the selection. |
| Alias init-local data into a persistent name, or carry an expired payload/external-memory view across a boundary | Diagnose the lifetime escape; never silently copy or promote existing backing storage. |
| Send malformed, oversized, or out-of-domain host input | Reject invalid layouts/capacity before execution; normalize valid fields before observation. |
| Disable host output collection or overflow its batch | Internal dispatch and state effects remain unchanged. |
| Use nested SoA bool/f64/struct-array fields from internal and prepared host storage | Same helper results and aligned, stride-correct access in both backends. |

Measure small coefficient/configuration objects and large spectral arrays. Inspect storage size,
copy cost, forwarding cost, and compile time. The central performance promises are bounded storage
and cheap reference calls/forwarding, not zero-cost value replacement or serialization.

## Deliberately outside this proposal

Structured params and proc constructor controls; aggregate audio/graph routing; borrowed returns;
reference fields; exclusive references; new tuple/array nesting forms; and structured compile-time
functions. Existing scalar/primitive-array params remain intact.

## Design tradeoffs

This design chooses cheap aliases and calls with ordinary content replacement/returns.
It removes authored ownership tracking while retaining Onda's existing reference conventions.
Its two visible tradeoffs are the distinction between alias bindings and fixed-data declarations,
and live rather than immutable aggregate messages. Both are stated directly and apply uniformly.

Making every new binding a value copy would remove the first distinction but make familiar
aggregate selections copy potentially large data. Immutable message snapshots would remove the
second but require bounded snapshot capacity for dynamic slices or a stronger restriction on
writable aliases during dispatch. Neither is needed for the proposed core.

## References

- [Language guide](../syntax.md)
- [Compiler architecture](../architecture.md)
- [Delegate host integration](../delegates.md)
- [Aggregate layout planner](../../crates/onda_semantics/src/aggregate_layout.rs)
- [MIR types](../../crates/onda_mir/src/types.rs)
