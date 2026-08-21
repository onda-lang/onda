# Cooperative loading tasks

## Status

This document specifies the implemented task language and runtime behavior.

Onda supports statically allocated tasks owned by either the top-level program or a `proc`. They
can spread expensive setup work over multiple logical audio blocks. A task executes cooperatively:
it runs until it reaches `yield` or completes, and it can only be advanced from its owner's
block-pre scope.

An `await` reached by block-pre control flow is also a readiness barrier. If the task yields, the
owner stops that activation without executing the remaining block body or sample body and produces
neutral zero outputs. Task status alone does not gate processing: ordinary user
control flow decides which task, if any, is awaited on each block.

Tasks do not create threads, allocate dynamic storage, or promise that their work fits an audio
deadline. They amortize work that the program deliberately chooses to perform on the audio thread.

## Motivation

Some processors need setup proportional to a bound resource rather than to the audio block size.
For example, a partitioned convolver may need to transform a large impulse response into many FFT
partitions before its sample body can run.

Running the complete build in one call can cause a large realtime spike. Manually expressing the
same operation as a block-carried state machine is possible, but it exposes implementation state,
duplicates control-flow structure, and requires every sample body to test whether loading has
finished.

A cooperative task should make this pattern direct:

```onda
proc Convolver:
  buffers:
    impulse: f32

  init:
    kernel: f32[MaxKernelSize] {retain}
    history: f32[MaxKernelSize]

  tasks:
    load():
      clear_kernel()

      for partition in 0..partition_count:
        build_partition(partition, impulse, kernel)
        yield

  block:
    await load()

    update_runtime_coefficients()

    sample:
      out1 = convolve(in1, kernel, history)
```

Each activation of `Convolver`'s block-pre control flow advances `load` to its next `yield`. A
statically scheduled proc activates at logical-block begin; a runtime-indexed proc-array slot
activates lazily on its first call in the logical block. While `load` is suspended, `Convolver`
returns zero without running `update_runtime_coefficients`, its sample body, or its block-post body.
When the task completes, execution continues after `await` and audio processing begins in that same
logical block, including the call that lazily activated a proc-array slot.

## Design principles

- Tasks are cooperative persistent control flow, not asynchronous jobs.
- Every task and its continuation frame have a statically known layout.
- `await` is a structured block-pre barrier selected by ordinary user control flow.
- Sample code never needs to query task completion.
- Reaching an `await` that yields prevents following block and sample code from observing partial
  task results in that logical block.
- Task status does not implicitly gate a proc when control flow does not reach its `await`.
- The first task suspension stops the owner block, independently of process segmentation.
- Task reset is explicit; buffer rebinding and parameter mutation do not infer dependencies.
- Task continuations and `{retain}` state survive ordinary instance reset; tasks may also
  deliberately observe or update resettable state.
- Tasks are owner-local and non-first-class.
- Tasks reuse ordinary MIR state and control flow rather than requiring a runtime scheduler.

## Source model

### Task declarations and the `tasks` block

The top-level program or a proc may declare tasks either individually with `task NAME():` or in a
grouped `tasks:` block.
Entries inside `tasks:` mirror entries inside `events:` and do not repeat the `task` keyword:

```onda
proc Loader:
  tasks:
    load():
      prepare_destination()
      for i in 0..work_count:
        perform_work_unit(i)
        yield

    clear_cache():
      clear_destination()
```

The standalone form is equivalent:

```onda
proc Loader:
  task load():
    prepare_destination()
    for i in 0..work_count:
      perform_work_unit(i)
      yield

  task clear_cache():
    clear_destination()
```

Grouped and standalone declarations may be mixed within one owner and contribute to the same task
namespace; duplicate task names are rejected. At top level, for example:

```onda
task prepare():
  build_header()
  yield
  build_body()

block:
  await prepare()
  sample:
    out1 = render()
```

Tasks are private to their owner. Their names cannot conflict with another task or another
addressable declaration in that owner. They implicitly see the owner's params, buffers, and
init-rooted state. A task may update owner state, but it cannot directly read audio inputs or write
audio or control outputs.

The initial design has no task parameters, return values, overloading, task-local generic
parameters, or first-class task values. A task declared in a generic proc is specialized together
with its owner.

A task may use ordinary control flow and call builtins, non-yielding defs visible from its owner,
and child-proc events, including the child's builtin `init(...)` event. It cannot call a proc step,
invoke its owner's event handlers, call another task, or use a task control operation. `yield` is
only legal lexically inside the task body. A called def cannot yield on behalf of its caller; this
keeps the continuation frame flat and statically bounded.

### Awaiting a task

A task is advanced with a block-pre statement:

```onda
block:
  await load()

  sample:
    out1 = process(in1)
```

`await` is legal as a statement anywhere within structured control flow belonging to the task's
owner's block-pre body, including `if`, `for`, `while`, and `loop` bodies:

```onda
block:
  selected = choose_loader()

  if selected == 0:
    await load_left()
  else:
    await load_right()

  publish_controls()

  sample:
    out1 = process(in1)
```

It cannot appear in `init`, an event, a def, another task, block-post, or sample code. A task cannot
be awaited through a child-proc receiver; the child owns its tasks and decides when its block-pre
control flow reaches them. `await` is a statement, not an expression, and produces no source value.

Whenever the owner's block-pre control flow is activated for a logical block,
`await load()` behaves as follows:

1. If `load` is not started, start it at the beginning of its body.
2. If it is suspended, resume it immediately after its previous `yield`.
3. Execute until the next `yield`, normal completion, or runtime failure.
4. If it yields, stop the owner's activation at the `await` barrier for this block.
5. If it completes, continue with the next block-pre statement and eventually run the sample and
   block-post bodies.
6. If it was already complete, continue immediately without re-executing the task.
7. If it was already failed, do not resume it and return from the proc at the same barrier.

Statements before `await` execute again on every logical block in which the owner is activated. The
containing block is not itself a coroutine and does not preserve an instruction pointer across
blocks. Statements after `await` run only in the block where the task has completed or was already
complete. State changes performed before the barrier, including changes made by the task before
yielding, are not rolled back.

Conditions and loops surrounding an `await` are also evaluated again from the beginning on every
logical block. If their values change while a task is suspended, the next block may select another
task or bypass task advancement entirely. Tasks may update proc state before yielding, so they can
participate in user-written scheduling decisions for later blocks.

Multiple reached awaits are evaluated in source order. Completed tasks fall through immediately;
the first task that yields returns from the block, so no later statement or await runs in that
logical block. A suspended task whose await is not reached remains suspended. If control flow
bypasses an incomplete task and reaches sample processing, the program is responsible for not
consuming that task's partially produced state.

### Yield

`yield` saves the task continuation and returns control to the process schedule. The next time
block-pre control flow reaches an await for that task, execution resumes immediately after the
yielding statement.

```onda
tasks:
  load():
    for partition in 0..partition_count:
      build_partition(partition)
      yield
```

`yield` does not accept a value. An explicit `return` without a value, or reaching the end of the
task body, completes the task. Tasks do not return completion values in the initial design, so
`return EXPR` is invalid in a task.

Because `yield` is an unconditional suspension point, yielding after the final unit of work may
require one final inexpensive resume to reach task completion. A program can avoid that extra
block by placing the final `yield` conditionally when it matters.

`yield` defines a cooperative boundary, not a work or time budget. Everything between two yields
runs in one audio callback. In particular, a single call such as `build_partition` may still be too
expensive for realtime execution. Onda does not preempt statements, inspect the wall clock, or
infer suitable suspension points.

### Empty returns in runtime defs

The same bare `return` syntax is allowed for early exit from any runtime `def` that does not return
a value. This rule belongs to the `def` construct itself, regardless of its owner or role; it
therefore includes top-level defs, proc-local defs, struct methods, parameter update hooks, and any
future context that accepts a runtime `def` declaration.

```onda
proc Filter:
  init:
    dirty = 1

  def update_if_needed():
    if dirty == 0:
      return

    rebuild_coefficients()
    dirty = 0
```

A runtime def with no explicit return annotation and no `return EXPR` statements is
non-value-returning. Reaching the end of its body or executing `return` completes the call without a
value. It can only be called as a statement. Onda does not add a source-level `void` type or
`-> void` spelling.

Bare and value returns cannot be mixed in one runtime def. A def containing any `return EXPR`, or
declaring an explicit `-> T` return type, must return a value on every reachable return path and
rejects bare `return`. Conversely, a non-value-returning def rejects `return EXPR`. Restrictions on
special def roles such as parameter update hooks become "no value return" rather than "no return
statement."

Compile-time `const def` remains value-returning and does not accept bare `return`.

## Task lifecycle

Each task has the conceptual state machine:

```text
NotStarted --await------> Running
Running    --yield------> Suspended
Suspended  --await------> Running
Running    --return-----> Complete
any        --reset------> NotStarted
Running    --failure----> Failed
Failed     --reset------> NotStarted
```

`Running` exists only while generated code is executing. It is not observable between instance
calls.

Completed tasks remain complete. Awaiting one is an inexpensive status check and does not rerun its
body.

### Initialization and task reset

Fresh instance or proc construction initializes every state root and task frame. Both host-level
program initialization and an in-language call to a proc's builtin `init(...)` respect reset policy
by default: resettable init roots are initialized again, while `{retain}` roots and hidden task
frames keep their current values. The all-state forms select a full reinitialization:

```onda
voice.init()                         # preserve retained roots and task continuations
voice.init(all = true)              # initialize all roots and task frames
```

At the program level these operations are exposed as `init` / `init_all` in Rust,
`onda_init` / `onda_init_all` in C, and `init()` / `initAll()` in JavaScript.

The initializer body still executes normally in both cases. Retain suppresses the retained
binding's declaration initializer; it does not suppress later explicit operations. In particular,
an initializer can deliberately restart a task, and that reset is respected even by the default
form:

```onda
proc Convolver:
  init:
    kernel: f32[MaxKernelSize] {retain}
    load.reset()
```

Initialization remains synchronous and does not execute task work. The next logical block starts a
reset task when its `await` barrier is reached.

An owner can request another load without rerunning its complete initializer:

```onda
proc Convolver:
  event reload():
    load.reset()

  block:
    await load()

    sample:
      out1 = convolve(in1)
```

`reset()` discards the saved continuation and resets task-local frame storage to its initial
representation. It performs no task work. It is allowed from the owner's init, event, and
block-pre scopes, but not from sample code, ordinary defs, or tasks. Resetting a not-started,
suspended, complete, or failed task has the same result. The exclusive instance-execution contract
means source code cannot reset a task while that task is running.

Task control operations are not first-class methods. `load.reset()` resolves statically to the
named task in the current owner.

## Await barriers and neutral outputs

When an executed `await` reaches a task that yields or was already failed, its owner becomes
unavailable for audio or control processing from that activation through the remainder of the
logical block. At top level and for a statically scheduled proc this covers the complete block; for
a lazily activated proc-array slot it begins with the activating call. The generated schedule must
guarantee that:

- statements following the suspended `await` do not execute;
- the nested sample loop does not execute at any oversampling substep;
- block-post statements do not execute;
- every declared audio output produces its type's zero value for every requested frame while the
  proc is unavailable;
- every declared control output produces its type's zero value while the proc is unavailable.

For a proc task, the zero result belongs only to the loading proc. A parent continues normally and
may mix, bypass, or otherwise handle the neutral child result:

```onda
sample:
  dry = in1
  wet = convolver(in1)
  out1 = dry + wet
```

For a top-level task, neutral output is the program's externally visible result for the block.

This design intentionally chooses silence as the owner-level loading policy. Keeping an old kernel
active while a new one is constructed requires separate active and staging state plus an explicit
publication or crossfade policy; it is not implicit task behavior.

This gating belongs to the executed control-flow path, not to the task status in isolation. A
`NotStarted`, `Suspended`, or `Failed` task has no effect on a block whose control flow does not
reach its `await`. Resetting a task also does not make the proc unavailable by itself. User code
may deliberately bypass an incomplete task, but it is then responsible for isolating partially
produced state, typically by building into staging storage and publishing it only on completion.

## Logical blocks and segmented processing

Onda hosts may split one logical block into multiple process segments around sample-accurate events.
Task advancement follows the existing block-pre activation model and is tied to a proc instance's
activation within the logical block, not to the number of process entry calls:

- `ONDA_PROCESS_BEGIN_BLOCK` starts a new logical block and clears the per-instance activation
  markers used by block scheduling;
- statically scheduled proc instances execute block-pre control flow, including conditional awaits,
  when `ONDA_PROCESS_BEGIN_BLOCK` is present;
- a runtime-indexed proc-array slot executes block-pre lazily on its first `()` call in the logical
  block, even when that call occurs in a later segment;
- once an instance or slot has activated, later calls and segments reuse whether its block-pre
  execution completed or returned at an await barrier, without reevaluating its control flow or
  advancing its task again;
- a task that completes during activation allows the activating call and later calls in the logical
  block to continue normally;
- a task that yields during activation keeps that proc instance unavailable for the remainder of
  the logical block;
- resetting a task mid-block does not retroactively change an already activated instance's current
  availability; a lazily scheduled slot that has not yet activated observes the reset if it is first
  called later in that block;
- zero-frame begin-block segments execute statically scheduled block-pre task control, while unused
  runtime-indexed slots remain inactive and perform no work.

This rule prevents process segmentation from granting additional task resumptions while preserving
the existing performance property that unused runtime-indexed proc-array slots do no block work.

## State, bindings, reset, and snapshots

### Continuation storage

Each program instance and proc instance owns an independent frame for each declared task. Proc
arrays therefore have one frame per task per array element. A frame contains:

- task lifecycle and continuation position, which may be encoded together in one program-counter
  field;
- primitive locals live across a `yield`;
- fixed aggregates live across a `yield`;
- loop indices and other control state required to resume execution.

The compiler promotes those values into hidden fixed state. No task operation allocates, locks, or
uses a runtime stack that survives the call.

Runtime handles cannot survive a yield. Buffer descriptors, slices, proc aliases, and other
reference-like values must be reacquired after resumption. Semantic analysis rejects a task when
such a value is live across a `yield`.

### Reset policy for init state

Every init-rooted state binding has one of two reset policies:

- **restore**, the default, restores the binding from the post-init reset baseline;
- **retain** leaves the binding's current value unchanged during ordinary instance reset.

The nondefault policy has an explicit named form and a shorthand:

```onda
init:
  kernel: f32[MaxKernelSize] {reset = retain}
  kernel_len: i32 = 0 {retain}
  normalization = 1.0 {retain}

  history: f32[MaxKernelSize]
  cursor = 0
```

`{retain}` is exactly equivalent to `{reset = retain}`. An empty `{}` has no reset meaning and is not
accepted as shorthand. Unannotated state keeps the existing reset behavior.

The reset field composes with the existing integer-domain fields:

```onda
init:
  partition = 0 {MaxPartitions, wrap, reset = retain}
  selected = 0 {range = 0..8, reset = retain}
```

Positional count or range syntax still precedes bare mode and named fields. `reset = retain` is also
valid without an integer domain. The shorthand `{retain}` is intended for bindings that need no other
attributes.

The annotation is valid only on fresh persistent value bindings introduced directly by an `init`
section. It applies to the complete state root and supports primitive scalars, fixed arrays, tuples,
and structs. Individual elements or fields cannot select a different policy without being declared
as separate init roots. Proc instances, params, inputs, outputs, buffers, locals, aliases, and
compile-time constants do not accept the annotation; child procs declare reset policy on their own
init-rooted state.

`retain` is narrowly a reset and reinitialization policy. It does not restrict ordinary mutation,
snapshot capture, snapshot restoration, or fresh proc construction. Fresh construction always
initializes retained state. An in-language proc `init(...)` preserves it unless
`all = true` is passed.

Task-private continuation storage is implicitly `retain` and does not require source annotations.
Tasks may freely read or write either retained or resettable init state. This is intentionally not
an effect-analysis rule: after ordinary reset or default proc reinitialization, a suspended task may
resume with its continuation intact while resettable state has returned to its initializer value.
That observable combination can be useful, and deciding whether it is coherent belongs to the
program. Authors who need continuation and produced state to share a reset lifetime should mark
that state `{retain}` or explicitly reset the task. Parameter or state changes do not implicitly
reset a task. External-buffer writes remain outside instance reset under the ordinary buffer
binding contract.

### Buffers and parameters

Tasks access their owner's current buffer mappings whenever they resume. They do not capture raw
buffer pointers in their continuation frames. External bindings must satisfy the same validity and
exclusive-access rules as ordinary processing.

Rebinding a buffer does not implicitly reset any task. If a host rebinds a resource while a task
is suspended and does not request a task reset, later resumptions see the new binding and the resulting
derived state may contain work from both resources. This is memory-safe under the binding contract
but is normally a program error. The host or program must invoke an appropriate event that calls
`reset()`.

Owner params are read normally on every resume. A task that needs a stable value across the entire
build can copy the param into a primitive local before its first `yield`; that local then becomes
part of the continuation frame. Parameter mutation does not implicitly reset a task.

### Reset and snapshots

Ordinary instance reset restores only reset-policy state ranges. It leaves every `{retain}` root and
hidden task frame untouched. Consequently:

- a completed task remains complete and retains its derived state;
- a suspended task retains its continuation and resumes after processing continues;
- a failed task remains failed until explicitly reset;
- resettable histories, voices, cursors, and other ordinary DSP state return to their post-init
  baseline;
- task code observes those restored resettable values when it next resumes;
- snapshots include task status, continuation state, and task-mutated persistent state;
- restoring a suspended-task snapshot resumes that task from the recorded suspension point;
- snapshot restoration replaces retained state with the captured value because `retain` affects reset,
  not snapshot semantics;
- external buffer bindings and contents remain outside snapshots, so restoring a loading task under
  incompatible bindings has the same host-responsibility caveat as other buffer-derived state.

State metadata records snapshot inclusion, ordinary-reset policy, and reflection visibility as
independent properties. Task frames are included in snapshots and retained by ordinary reset, but
they do not appear as user-authored state fields in reflection. Their bytes remain part of the
physical state and packed snapshot contract. Artifact snapshot entries retain those physical
segments with `authored = false`, allowing snapshot implementations to copy them without presenting
them as source-authored state.

The ordinary public reset operation copies compiler-described resettable ranges from the post-init
image.
A plugin host can therefore reset temporal DSP state and its own logical-block cursor without
resetting completed IR or table loading tasks. Existing programs without `retain` annotations
retain their current full authored-state reset behavior.

A separate all-state reset operation restores the complete post-init baseline, including retained
roots and hidden task frames. It does not rerun initialization or capture a new baseline. This gives
hosts an explicit way to discard task results and restart loading without weakening the ordinary
reset contract or adding an opaque `force` boolean. The public APIs use `all` consistently:

```text
Rust: reset(instance)          / reset_all(instance)
C:    onda_reset(instance)     / onda_reset_all(instance)
JS:   processor.reset()        / processor.resetAll()
```

Restoring retained roots and task frames is one atomic reset operation: exposing restored state with
an old continuation, or a restored continuation with partially produced state, would be invalid.
Full instance initialization remains distinct because it reruns generated initialization against
the current parameters and bindings and captures a new baseline. In-language proc
`init(all = true)` performs the corresponding full initialization for that proc instance
but does not redefine the host instance's reset baseline.

The instance APIs expose both initialization modes directly:

```text
Rust: init(instance)           / init_all(instance)
C:    onda_init(instance)      / onda_init_all(instance)
JS:   processor.init()         / processor.initAll()
```

`init` begins from the current live image, so retained roots and task frames survive unless an
explicit init statement changes them. Resettable declaration initializers still run. `init_all`
clears the complete physical image first. Both operations execute transactionally in a staging
image; success publishes the image and captures it as the new reset baseline, while failure leaves
both live state and the previous baseline unchanged. Instance creation writes parameter defaults,
runs the all-state form, and captures the initial baseline.

## Failure behavior

A generated runtime failure while executing a task reports through the ordinary process error
channel and transitions that task to `Failed`. A failed task does not resume automatically. A later
`reset()` permits another attempt.

The exact output-memory guarantee for the process call that encountered the failure follows the
general process-entry failure contract. On later blocks, control flow that reaches the failed task's
`await` stops at that barrier and produces neutral owner outputs. Control flow that bypasses the
failed task is unaffected, subject to the same responsibility for isolating partial task state.
The original failing process call reports the error once; reaching the failed task on later blocks
does not repeatedly return the same error. Hosts and adapters must continue permitting later process
calls and control events rather than permanently latching the original process failure. This lets a
task-reset event retry the work and lets ordinary control flow bypass a failed task.

## Realtime contract

Tasks execute synchronously on the thread that calls `process`. They do not move computation off the
audio thread. They are useful when:

- work can be divided into predictably small units;
- temporary silence while loading is acceptable;
- constructing and swapping a replacement instance is unavailable or undesirable;
- the total loading latency may span several blocks.

They are not a substitute for worker-thread construction when even one work unit can exceed the
audio deadline. The language guarantees memory safety, fixed storage, and deterministic cooperative
scheduling, not deadline suitability.

## Compiler and MIR lowering

The initial implementation should lower tasks to ordinary state and control flow before or during
semantic-to-MIR lowering rather than introduce a runtime task scheduler. Task lowering first builds
a typed, binding-identity-aware control-flow graph. The compiler performs backwards liveness over
its suspension edges and promotes only values live across a `yield` into the continuation frame.
Branches, loops, `break`, and `continue` become explicit CFG edges before the resumable state machine
is generated; task bodies are not split by ad hoc syntax-tree rewriting.

The parser and semantic model use the same task representation for both owner kinds. Lowering uses
one CFG, liveness, frame-layout, reset, and resume-state-machine builder. Proc tasks package the
prepared state machine in a private resume function; top-level tasks inline the same prepared
machine into block-pre because ordinary top-level defs cannot capture program state.

For each task, lowering generates conceptually:

- hidden state slots for the continuation position and values live across yields; lifecycle and
  continuation may share one encoded program-counter slot;
- a non-yielding resume region that dispatches on the saved program counter;
- a reset function that restores the task frame's initial representation;
- structured block-schedule control flow that branches on each reached resume result and returns
  through the neutral-output path when the task suspends or is failed.

State metadata records reset policy independently from snapshot persistence and reflection
visibility. Runtime metadata contains coalesced physical ranges for resettable state so instance
reset remains allocation-free and does not inspect authored names or task effects dynamically.
Hidden task-frame ranges are marked `retain` and snapshot-included by construction, but are omitted
from authored-state reflection.

Every `yield` stores the next continuation point and returns a compiler-internal suspended status.
Normal task return stores `Complete`. This status controls lowering of the await barrier and is not
an Onda source value. The generated resume call itself is an ordinary acyclic call and runs to
completion, so the existing fixed-stack and no-recursion MIR contract remains intact.

The task construct does not initially need a public MIR task handle or host runtime API. Task status
is not exposed as a language value, state-reflection entry, or public ABI value in the initial
design. If the MIR schema needs to retain task identity for validation and diagnostics, it may add
compiler-owned metadata while still expressing execution through ordinary functions and state
slots. Native LLVM and Binaryen backends must implement identical logical-block gating,
failure-recovery, and neutral-output behavior.

Useful validation includes:

- every task referenced by `await` or `reset` exists in the current owner;
- `await` occurs only as a statement within the owner's structured block-pre control flow;
- `yield` occurs only in a task body;
- neither `yield` nor task `return` carries a value;
- no call-transitive yield is possible;
- tasks call only builtins, non-yielding defs visible from the owner, and synchronous child-proc
  events; they do not call proc steps, owner events, other tasks, or task control operations;
- no runtime handle or reference-like local is live across a yield;
- all continuation storage has a fixed compile-time layout;
- task call graphs remain acyclic;
- every non-value-returning runtime `def` accepts bare `return` regardless of owner or role, while
  value-returning defs reject it and every return path within one def agrees on whether it carries
  a value;
- a non-value-returning runtime `def` is callable only as a statement and never synthesizes a
  fallback value in an expression context;
- every unavailable owner output has a generated neutral value.

## Explicit non-goals

This proposal does not include:

- worker threads, thread pools, futures, or parallel execution;
- dynamic task creation, task handles, or task collections;
- time-based budgets or preemptive interruption;
- automatic suspension-point placement;
- task parameters or return values;
- values carried by `yield`;
- yielding from defs called by a task;
- awaiting child-proc tasks from outside their owner;
- awaiting from defs, events, tasks, block-post, or sample code;
- preserving an instruction pointer for the containing block across logical blocks;
- implicit proc gating based only on a task being incomplete, suspended, failed, or reset;
- automatic buffer or parameter dependency tracking;
- automatic task reset after rebinding or parameter mutation;
- retaining the previous processor result while loading;
- automatic bypass, crossfade, or double-buffered publication;
- host-visible task handles or task-status APIs;
- use of tasks from `graph` processors in the initial design.

## Resolved design decisions

- The task control operation is spelled `reset()`.
- A task runtime failure is reported by the process call that encounters it. Later awaits of the
  failed task take the neutral-output path without repeatedly reporting the same failure.
- Task status remains compiler-private and is not reflected through the source language or public
  processor ABI in the initial design.
- Zero-frame begin-block segments advance statically scheduled task control. Runtime-indexed
  proc-array slots remain lazy and activate only when first called in the logical block.
- Runtime-indexed proc-array slots support tasks through the existing once-per-logical-block lazy
  block-pre activation model.
- Tasks may call builtins, non-yielding defs visible from their owner, and synchronous child-proc
  events, including child `init(...)`.
- Task continuation lowering uses a typed CFG plus liveness analysis, and only values live across a
  yield occupy frame storage.
- Snapshot inclusion, ordinary-reset policy, and authored-state reflection visibility are distinct
  state metadata properties.

## Implementation structure

1. Add owner-local standalone `task NAME():` declarations and the grouped `tasks:` section at both
   top level and inside procs, task AST representation, valueless `yield`, structured block-pre
   `await`, and bare `return` syntax.
2. Add semantic ownership, scope, placement, return-form, acyclic-call, and task-call validation,
   including bare returns for every non-value-returning runtime `def` independent of owner or role.
   Represent no-result functions explicitly and reject their use in expression contexts.
3. Add init-binding `{reset = retain}` and `{retain}` parsing, typing, and metadata.
   Model snapshot inclusion, ordinary-reset policy, and reflection visibility independently.
4. Build a typed task CFG, perform backwards live-across-yield analysis, reject reference-like live
   values, and lower the CFG into hidden continuation storage plus generated resume state machines.
5. Initialize task frames during fresh construction, preserve them during default proc re-init,
   support `init(all = true)`, and implement task-local `reset()`.
6. Integrate conditional awaited-task advancement and early block return with eager begin-block and
   lazy runtime-indexed proc-slot activation, preserving once-per-logical-block behavior across
   process segments.
7. Generate neutral owner outputs and skip later block/sample execution when a reached await is
   suspended or failed, without globally gating bypassed or merely reset tasks. Gate the complete
   oversampling schedule rather than only the source sample body.
8. Generate coalesced resettable state ranges, preserve retained/task-frame ranges during ordinary
   instance reset, restore the complete baseline during all-state reset, and retain complete task
   state in snapshots without exposing hidden fields as authored state.
9. Implement matching native and Binaryen behavior, including recoverable one-shot task failures in
   hosts and adapters that otherwise latch process errors.
10. Add conformance tests for initial loading, completion in the current block, conditional and
   loop-nested awaits, branch changes between resumptions, bypassed suspended/failed tasks, task
   reset, mid-block events, segmented processing, lazy runtime-indexed proc arrays, oversampling,
   instance reset, snapshots, failures, rebinding, retained scalars/aggregates, integer-domain
   composition, neutral audio/control outputs, and valid and invalid bare-return forms in every
   runtime-def owner context.
