# Delegates design

## Status

This document is a design proposal, not implemented language behavior.

The working source-language vocabulary is:

- `event` / `events` for commands entering an Onda owner;
- `delegate` / `delegates` for typed occurrences leaving an Onda owner;
- `emit` to trigger a delegate;
- `on` to subscribe Onda code to a delegate.

The name `delegate` is provisional, but the directional model in this document should remain useful
if the keyword changes.

## Motivation

Onda currently has two related event forms:

- a top-level event is an immediate host-to-processor entry point;
- a proc event is a receiver-only command invoked on a proc instance.

Neither form lets a processor report a sparse occurrence such as an envelope completing, a voice
being stolen, a sequencer advancing, or an analysis result becoming available. Audio `outs` are the
wrong representation for sparse control occurrences, and `kouts` expose only the latest held value;
they do not preserve occurrence count, ordering, or sample position.

Delegates add the opposite event direction without changing the meaning of existing events:

| Surface | Direction | Meaning | Delivery model |
| --- | --- | --- | --- |
| `event` | host/parent to owner | Invoke a command handler | Immediate call |
| `delegate` | owner to parent/host | Report a typed occurrence | Synchronous internally, queued at the host boundary |
| `outs` | owner to host | Dense sample-rate values | Audio buffers |
| `kouts` | owner to host | Latest block-rate values | Held control values |

Delegates are intended for sparse control data. Programs should continue to use audio or control
outputs for dense per-sample observations.

## Source syntax

### Declarations

A plural block declares any number of delegates:

```onda
delegates:
  finished()
  voice_stolen(voice: i32, note: i32)
  position_changed(position: f32)
```

The singular form declares one delegate:

```onda
delegate finished(reason: i32)
```

A delegate declaration has no body. It is not an empty handler or an abstract function: its
parameter list defines the complete typed payload carried by each occurrence. Producer behavior
lives at an `emit` statement, while consumer behavior lives in `on` subscriptions or in the host.

Delegate parameters initially support the same wire-representable shapes as event parameters:

- primitive scalars;
- fixed-size primitive arrays, `T[N]`;
- read-only primitive slices, `T[]`.

An omitted type defaults to `f32`, consistently with event parameters. Delegate parameters do not
have defaults: every `emit` site is Onda code and must provide the complete payload. Struct payloads
and other aggregate forms remain future work.

Delegate names participate in the owner's normal member-name collision rules. In particular, an
event, task, and delegate in the same owner cannot have the same name. This keeps `owner.name`
resolution and host metadata unambiguous.

### Emission

`emit` is a statement, not an expression:

```onda
emit finished(reason)
emit voice_stolen(voice = index, note = current_note)
```

Arguments use ordinary call binding and coercion rules. All required parameters must be supplied.
Argument expressions are evaluated exactly once, from left to right, even when there are no
subscribers or no host outbox is bound. Attaching a host must never change DSP behavior.

Only the owner of a delegate may emit it. A proc emits one of its own delegates by unqualified name:

```onda
proc Envelope:
  delegate finished()

  init:
    active = true

  sample:
    if active:
      active = false
      emit finished()
```

Code outside the proc cannot use `emit env.finished()`. It may observe `env.finished`, but it
cannot impersonate the producing proc. Owner-local defs may emit owner delegates, subject to the
same execution-context restrictions as their callers.

Emission is allowed from:

- `sample` and `block` code;
- tasks;
- top-level and proc event handlers;
- delegate subscription bodies;
- runtime defs called from one of those contexts.

Emission is not allowed from `init`. Full and preserve-pinned initialization establish or recover
state and must not produce external occurrences, regardless of whether an allocation-only host has
already bound an outbox. A runtime def that may emit a delegate therefore cannot be called from
`init`; this becomes part of its inferred effect.

`emit` has no result. In particular, it does not return whether a host is attached or whether a
bounded outbox accepted the occurrence. Host configuration and backpressure must not influence Onda
control flow.

### Subscriptions

`on` statically subscribes an owner-local handler to an unqualified owner delegate or to a delegate
on a known child proc instance:

```onda
init:
  env = Envelope()

on env.finished():
  emit voice_finished(0)
```

Payload bindings infer their types from the delegate declaration:

```onda
on voice.stolen(note, velocity):
  last_stolen_note = note
  emit voice_stolen(voice_index, note, velocity)
```

Subscription payload bindings are read-only. A subscription body follows event-handler mutation
rules for its owner: it may update existing `init`-rooted owner state, call proc events, call
permitted runtime defs, and emit owner delegates. A top-level subscription cannot write top-level
params; a proc subscription may write its owner's proc params, like a proc event handler. A
subscription cannot directly read or write block/sample I/O or introduce persistent state.

Subscription dispatch is always synchronous and non-suspending. A subscription body cannot use
`yield` or `await`, call a def that may suspend, or reset a task. In particular, a task must not emit
a delegate whose synchronous subscriber can mutate that task's active continuation. A subscriber
that needs to restart work can update ordinary owner state which explicit `init`, event, or
block-pre code observes before calling `task.reset()`.

Subscriptions are static program structure, not runtime delegate objects:

- `on` targets must resolve to a statically known proc instance or a constant proc-array element;
- a delegate may have zero, one, or multiple subscriptions;
- subscriptions run synchronously in source order;
- delegates cannot be assigned, stored, passed, returned, rebound, or unbound by Onda code;
- subscription does not allocate or mutate a runtime listener list.

Wildcard proc-array subscriptions, dynamic proc-array selectors, direct forwarding syntax, and
fanout syntax are intentionally deferred. They should be added only with a clear compile-time
expansion model. An eventual direct-route shorthand could be considered after `on` is proven:

```onda
# Possible future sugar; not part of the initial proposal.
routes:
  env.finished >> voice_finished
```

`on` is independent of the signal `graph` execution form, so delegates work in owners implemented
with `sample`, `block`, or `graph`. The initial design does not add delegate edges to `graph`.

## Example

```onda
proc Envelope:
  params:
    release = 0.999

  event trigger(velocity: f32):
    level = velocity
    active = true

  delegate finished()

  init:
    level = 0.0
    active = false

  sample:
    if active:
      level = level * release
      if level < 0.0001:
        level = 0.0
        active = false
        emit finished()

    out1 = level

delegate voice_finished(voice: i32)

init:
  env = Envelope()

event note_on(velocity: f32):
  env.trigger(velocity)

on env.finished():
  emit voice_finished(0)

sample:
  out1 = env()
```

The host can invoke `note_on` and subscribe to `voice_finished`. It cannot directly observe
`env.finished`, because proc delegates are implementation details until the containing owner
explicitly handles or promotes them.

## Execution semantics

### Internal dispatch

An `emit` performs these steps:

1. Evaluate and coerce the complete payload once.
2. If the delegate is top-level, append its external record to the bound outbox.
3. Invoke each static `on` subscription synchronously in source order.

Step 2 precedes subscription dispatch so a top-level occurrence remains ordered before any derived
top-level delegates emitted by its subscribers. Nested emissions dispatch immediately, producing a
deterministic depth-first order.

Proc delegates have no implicit host visibility. Emitting one only invokes its statically attached
subscriptions. When a subscription promotes it by emitting a top-level delegate, that second
emission creates the host-visible record.

The compiler should remove an emission with no possible observer only when evaluating its payload
is also proven to have no observable effect. Otherwise it must preserve payload evaluation.

### Cycles and bounded execution

Synchronous subscriptions create callable edges. The existing runtime recursion prohibition must
be extended across:

- ordinary runtime calls;
- proc-event calls;
- `emit` to `on` dispatch;
- delegate emissions made by subscription bodies.

Task bodies are entry roots for this analysis. A task's suspension and later resumption are not
synchronous call edges, but every def call, proc-event call, delegate emission, and subscription
dispatch performed within one resumption participates in the same cycle analysis.

Any possible recursive delegate/event cycle is rejected at compile time with a diagnostic showing
the cycle. For example, this is invalid even if a condition might stop it at runtime:

```onda
delegate changed()

on source.changed():
  emit changed()

on changed():
  source.retrigger() # `retrigger` may emit `source.changed`.
```

Static cycle rejection bounds dispatch depth and prevents callback-style reentrancy inside generated
DSP. It does not bound the total number of emissions produced by ordinary loops or sample execution;
the host outbox still needs an overflow policy.

### Failure atomicity

External delegate publication is transactional at the generated entry-point boundary. Before an
event or process entry point begins, the runtime checkpoints the outbox tail and overflow count. If
generated execution fails, records and overflow increments produced by that entry point are rolled
back before the failure is reported. Older undrained records remain intact. This matches the
fail-closed instance lifecycle: hosts must not observe occurrences derived from partially executed
state.

Internal subscription mutations are ordinary processor-state mutations and are not rolled back.
The failed state image is already invalid and remains unusable until full initialization or snapshot
restoration succeeds. A task `yield` is successful execution, not a failure, so delegate records
emitted before the yield are committed normally even though the owner produces neutral output for
that activation.

### Interaction with events

Input events and delegates compose directly:

- an event handler may emit a delegate;
- a delegate subscription may call a proc event;
- a proc event may emit one of that proc's delegates;
- a delegate subscription may transform and promote a proc payload to a top-level delegate.

A top-level event invoked through the current API runs immediately on the audio thread. Delegates it
emits are marked as immediate rather than pretending to have a position inside a process call. If
sample-accurate scheduled input events are added later, their handlers establish a logical sample
position and any delegate emitted by the handler inherits it.

### Interaction with tasks

A task may emit a delegate belonging to the same owner. The payload is evaluated and all internal
subscriptions run synchronously as part of that task resumption. The task may then continue, yield,
return, or reach its end normally. Emission does not suspend the task and subscribers cannot
suspend or reset task continuations.

Each `emit` executes at most once when control passes through that statement in a particular task
run. A later `yield` does not retract the occurrence. Restoring a snapshot whose continuation is
already past the statement resumes without emitting it again; explicitly resetting and restarting
the task may reach and emit it again. Task completion does not create an implicit delegate. Programs
that need completion or progress occurrences declare and emit them explicitly.

A proc task emits only that proc's delegates. As with other proc delegates, the occurrence becomes
host-visible only when a static subscription promotes it to a top-level delegate. A task may also
call a child-proc event that emits a child delegate; its subscriptions still dispatch synchronously
inside the task resumption.

## Timing and ordering

Every external delegate record carries a time kind and, when applicable, a logical frame offset:

| Emission context | Recorded position |
| --- | --- |
| Top-level event invoked outside processing | `immediate` |
| Statically scheduled block prelude or task resumption | segment boundary `start_frame` |
| Lazily activated runtime-indexed proc block/task | `start_frame + local_frame` |
| Base-rate sample body | `start_frame + local_frame` |
| Oversampled substep | containing base-rate frame |
| Block postlude | frame boundary `start_frame + frames` |
| Future scheduled input event | scheduled logical frame |

Multiple occurrences at the same offset retain FIFO order. Oversampled code may therefore produce
multiple ordered occurrences at one host sample; no fractional host-sample timestamp is implied.

Offsets are relative to the current logical compile block. They use the existing segmented process
arguments and do not introduce a hidden runtime cursor. BEGIN and END flags do not assert a segment
position, so a block-pre occurrence uses the actual segment boundary rather than assuming zero. A
runtime-indexed proc-array element whose block activation occurs lazily during its first sample call
uses that sample's base-rate offset, including for delegates emitted by its task. A block-post
occurrence lies on the end boundary and may therefore have offset `block_size`. Host adapters are
responsible for translating that boundary into their native event-time representation.

Zero-frame begin/end segments remain legal. Their prelude and postlude occurrences share the same
boundary offset and are distinguished by FIFO order.

## Host delivery

### Realtime outbox

Generated code must not invoke arbitrary user callbacks from the audio thread. Such callbacks could
allocate, lock, perform UI work, or re-enter the same instance. The canonical low-level delivery
mechanism is instead a bounded realtime-safe outbox associated with the instance.

Each record contains at least:

- delegate index;
- timing kind and logical frame offset;
- payload byte count;
- a copy of the packed payload.

The host drains records after a process segment or immediate event invocation and dispatches them on
the thread appropriate for that integration. A plugin adapter may translate records into the host's
native output-event list while still inside its realtime process callback. A UI-oriented wrapper may
move them to a control thread before invoking application listeners.

A high-level API may expose familiar binding syntax such as:

```text
processor.on("voice_finished", callback)
```

That API is a wrapper over the outbox, not permission for generated code to synchronously enter an
arbitrary callback. No instance operation may overlap processing or delegate draining in a way that
violates the instance's existing exclusive-ownership contract.

The exact ownership API remains to be selected: the host may bind preallocated storage, or instance
creation options may allocate a configured capacity off the realtime thread. Either representation
must satisfy the same portable contract and support native, AOT, and WebAssembly hosts.

### Lifecycle and snapshots

The outbox is transient host-delivery state, not processor state. Pending records, read/write
cursors, and overflow counters are not included in portable snapshots. Snapshot restoration clears
the outbox before making the restored instance live, so records produced after the snapshot cannot
be confused with occurrences from the restored continuation. A suspended task restored past an
`emit` resumes after that statement without recreating its record.

Host-level full and preserve-pinned initialization also clear pending records and the overflow
counter. They cannot emit new records because init-reachable emission is rejected. An explicit
`task.reset()` or a proc's builtin `init(...)` event does not clear the top-level outbox: those are
ordinary authored operations within an event or process entry point, and earlier committed records
retain their order.

Draining commits removal of the returned records and is not part of snapshots. The instance's
exclusive-ownership contract forbids draining concurrently with generated execution, allowing an
entry point to roll its uncommitted tail back on failure without racing a consumer.

### Missing consumers and overflow

An unbound top-level delegate outbox is a valid neutral configuration. Payload expressions and
internal subscriptions still execute, while the external record is discarded.

The outbox never allocates, blocks, or grows during an entry point. When it cannot fit another
record, it drops the newest external record and increments a host-visible overflow counter. Earlier
records retain their order. Overflow does not fail processing and cannot be observed from Onda code.

Hosts that require lossless delivery must provision sufficient capacity for their program and treat
any nonzero overflow count as an integration error. The language cannot promise a finite sufficient
capacity because source loops may emit a runtime-dependent number of occurrences.

Dynamic slice payloads are copied into the outbox at emission time. Their source storage is never
borrowed across the entry-point boundary. A slice that cannot fit is handled by the same whole-record
drop policy; partial records are never visible.

## MIR and processor ABI

### MIR representation

The MIR interface should add delegate descriptors alongside existing input-event descriptors. A
delegate descriptor contains its name and ordered parameter shapes. An `EmitDelegate` operation
references a delegate ID and its already evaluated payload values.

Static subscriptions should lower to ordinary direct handler calls during processor lowering. The
MIR therefore needs no mutable listener-list abstraction. It must nevertheless preserve the
external append-before-subscription ordering for top-level delegates and retain the delegate effect
in call-transitive effect analysis.

Task lowering must preserve the same effect instead of treating `emit` as an ordinary removable
statement. A top-level task resume helper that can emit externally needs the current runtime outbox
and logical timing context. Proc task helpers need synchronous access to their lowered subscription
calls, including any call-transitive promotion to a top-level delegate. The compiler should extend
the shared executable-body and effect analyses used by tasks, defs, and events rather than build a
second task-specific delegate analysis.

MIR validation must enforce:

- delegate IDs and payload arity/types are valid;
- only code belonging to the declaring owner can emit a delegate before proc flattening;
- init-reachable code cannot emit;
- subscription-reachable code cannot suspend or reset a task;
- delegate subscription/call graphs are acyclic;
- slice sources remain valid for the duration of their synchronous copy/dispatch.

### Metadata and payloads

Processor metadata should expose top-level delegates separately from host-triggered events. Each
delegate needs the same kind of reflection currently available for event payloads:

- stable declaration-order index and name;
- parameter names and primitive element types;
- scalar, fixed-array, or slice shape;
- fixed payload size or dynamic minimum size;
- byte offsets where statically defined.

Fixed payloads should reuse the existing event scalar packing and alignment rules. Dynamic slices
should likewise use an explicit length plus copied contiguous element bytes unless implementation
work demonstrates that the current event-slice header is unsuitable for sequential outbox records.
The final format must be identical across LLVM, Binaryen, native AOT, and packaged WebAssembly
artifacts.

The public C API will need delegate reflection, outbox configuration/binding, draining, and overflow
inspection. Exact names are deferred until the storage ownership model is chosen; illustrative
operations are:

```text
onda_delegate_count
onda_delegate_name
onda_delegate_param_count
onda_delegate_param_*
onda_instance_bind_delegate_outbox
onda_instance_drain_delegates
onda_instance_delegate_overflow_count
```

The processor descriptor and JavaScript packages need equivalent metadata and record decoding. The
Web Audio adapter must copy drained records across the worklet boundary without callback-time
allocation; its public listeners should run outside the `AudioWorkletProcessor` render callback.

## Implementation outline

Implementation should proceed in independently testable layers:

1. Add delegate declarations, `emit`, and `on` to the grammar and AST. Extend formatting, source
   ranges, and parser tests at the same time.
2. Add semantic symbols, payload checking, owner access control, task/delegate namespace collision
   checks, init-effect rejection, non-suspending subscription checks, static subscription
   resolution, and cross-event cycle diagnostics. Extend the shared task/def/event effect analysis.
3. Lower proc delegates and subscriptions to direct internal calls in coordination with task
   state-machine lowering. Preserve emission and promotion effects on generated task resume helpers.
   Test nested procs, proc tasks, constant proc-array elements, fanout ordering, payload
   transformation, and dead unobserved emissions before adding a host boundary.
4. Add top-level delegate descriptors and `EmitDelegate` to MIR, including validation, deterministic
   dumps, JSON/MessagePack transport, optimization effects, task runtime/timing context, and backend
   parity fixtures.
5. Add a fixed-payload realtime outbox to native runtime/codegen, the C API, processor metadata,
   AOT artifacts, Binaryen, and the Web Audio adapter. Implement entry-point commit/rollback and
   lifecycle clearing before exposing host listeners.
6. Add dynamic slice records after fixed payload behavior, overflow, timing, and cross-backend
   ordering are proven.
7. Add daemon/run-host transport, UI representation, LSP completion/hover/tokens, documentation,
   and end-to-end examples.

The conformance matrix should cover emission from sample, block pre/post, proc events, top-level
events, defs, subscriptions, and top-level/proc tasks before and after yields; multiple same-frame
emissions; statically scheduled and lazily activated task timing; segmented and zero-frame calls;
oversampled emission ordering; missing outboxes; exact-fit and overflowing outboxes; successful
yield commit and failed-entry rollback; task reset and replay; snapshot restoration without
duplicate emission; lifecycle outbox clearing; fixed arrays and slices; native/Binaryen parity; and
all invalid ownership, task-reentrancy, and cycle forms.

## Diagnostics and tooling

The frontend and LSP should distinguish events from delegates in diagnostics, hover text,
completion, and semantic tokens. Useful errors include:

- attempting to call a delegate as an event;
- attempting to emit an event;
- attempting to emit another proc instance's delegate;
- missing, extra, or incorrectly typed payload arguments;
- declaring a delegate parameter default;
- emitting from `init` or init-reachable code;
- subscribing to a dynamic or unresolved receiver;
- writing a subscription payload binding;
- declaring a task and delegate with the same owner-local name;
- awaiting, yielding, or resetting a task from a subscription;
- recursive event/delegate dispatch with a readable cycle path.

Host-facing UI can present delegates as bindable output events. It should show payload types and
outbox overflow state, but should not display them as buttons like input events or as held meters
like `kouts`.

## Explicit non-goals

The initial design does not include:

- first-class delegate values;
- runtime bind/unbind operations in Onda;
- arbitrary generated-code-to-host callbacks;
- external visibility of nested proc delegates without explicit promotion;
- delegate return values or delivery-dependent branching;
- implicit conversion between delegates and events;
- graph-edge delegate routing;
- wildcard or dynamically indexed proc-array subscriptions;
- suspending subscription handlers or task-continuation mutation during subscription dispatch;
- implicit task progress, yield, or completion delegates;
- fractional timestamps for oversampling;
- an unbounded or lossless realtime queue.

These constraints keep delegate dispatch deterministic, statically analyzable, portable across the
existing backends, and compatible with realtime execution.

## Open implementation decisions

The following details should be resolved during implementation without weakening the semantics
above:

- whether the host binds outbox storage or selects instance-owned capacity at creation;
- the exact packed record header and alignment;
- how draining and overflow acknowledgement are represented in the C API;
- how block-end boundary offsets map to plugin APIs that only accept in-block frame indices;
- whether the first implementation includes slices or starts with scalar/fixed-array payloads;
- whether direct forwarding deserves syntax after `on` is implemented;
- how constant proc-array subscriptions are expanded and ordered;
- whether delegate reflection uses `delegate` terminology throughout the processor ABI or the more
  direction-explicit internal term `output_event`.
