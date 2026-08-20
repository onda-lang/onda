# Deferred initialization design

## Status

This document is a design proposal, not implemented language or runtime behavior.

Onda should separate instance allocation from full program initialization. A host first creates an
uninitialized instance, installs project and host buffer bindings, sets its initial parameters, and
then explicitly initializes it. Initialization is synchronous and repeatable. The host chooses the
thread on which it runs and must provide exclusive access to the instance.

This design replaces the proposed `prepare` lifecycle. Buffer-dependent setup uses `init` rather
than introducing a second initialization language construct.

## Motivation

Generated initialization currently runs as part of instance creation, before project-default and
host-provided external buffers are necessarily attached. Consequently, top-level and proc
initializers cannot use buffers even though initialization is the natural place to:

- build convolution kernels from an impulse response;
- construct wavetables or lookup tables from a bound asset;
- analyze a sample and cache derived coefficients;
- initialize a proc from the selected resource shape or contents;
- establish state that should become the instance's reset baseline.

A separate `prepare` phase would solve the ordering problem, but it would duplicate much of
`init`: both would initialize persistent state, traverse the proc tree, and contribute to the reset
baseline. The simpler model is to run initialization only after bindings exist.

Proc `init(...)` must remain callable from ordinary Onda code, including events, `block`, and
`sample`. Onda does not decide whether such a call is a good realtime decision. A convolution proc
may legitimately have an expensive initializer; the program and host decide whether to run it on a
setup thread or accept its cost on the audio thread.

## Terminology

This proposal distinguishes two related operations:

- **full instance initialization** is the host operation that clears and reconstructs the complete
  program state, executes the top-level initialization schedule, and captures a new reset baseline;
- **proc initialization** is the existing callable `proc.init(...)` operation that initializes one
  proc instance according to the language's ordinary call semantics.

The source-level `init` section and proc `init(...)` implementations are shared initialization
logic. The distinction concerns who starts the operation and how much state it governs, not two
different kinds of source code.

## Design principles

- Allocation and full initialization are distinct lifecycle operations.
- Project and host buffers are bindable before generated initialization runs.
- Both top-level and proc initialization may access their visible buffers.
- Full initialization is synchronous, repeatable, and exclusively host-controlled.
- Proc `init(...)` remains callable anywhere that ordinary proc calls are legal.
- Onda guarantees memory safety and a non-concurrent instance contract, not that arbitrary
  initialization meets an audio deadline.
- Rebinding does not implicitly initialize or invalidate an already initialized instance.
- Full initialization establishes the reset baseline; runtime proc initialization does not.
- No `prepare` source section or preparation lifecycle is introduced.

## Instance lifecycle

The initial lifecycle is:

```text
compile program
    |
create instance and allocate all fixed storage
    |
attach project-default buffers
    |
host binds or overrides buffers
    |
host sets initial top-level parameters
    |
host calls full instance init
    |
capture the post-init reset baseline
    |
events / process / reset / snapshots
```

The runtime state machine is deliberately small:

```text
NeedsInit --full init succeeds--> Ready
    ^                              |
    |                              +-- full init succeeds --> Ready
    +------ full init fails -------+
```

Instance creation returns `NeedsInit`. Buffer binding and top-level parameter mutation are allowed
in that state. Processing, top-level events, reset, snapshot capture, and snapshot restoration are
rejected until full initialization succeeds.

A host may call full initialization again on a ready instance. The operation immediately consumes
the old runtime state; it is not transactional. Success installs a new reset baseline. Failure
leaves the instance in `NeedsInit`, and the host may correct its bindings or parameters and retry.
Writes already made to host-owned buffers are not rolled back.

The runtime never calls full initialization implicitly after creation or rebinding. Convenience
hosts may offer a create-bind-init operation, but it must be defined in terms of the explicit
lifecycle.

## Full instance initialization

Full initialization behaves as follows:

1. Require exclusive access to the instance.
2. Preserve external buffer descriptors and their current bindings.
3. Preserve current top-level parameter values.
4. Clear compiler-managed persistent and transient runtime state to its pre-init representation.
5. Reconstruct the statically allocated proc topology and execute the complete top-level `init`
   schedule.
6. Clear or reestablish runtime queues, control mirrors, and other transient bookkeeping according
   to their normal fresh-instance semantics.
7. Copy the resulting physical state into the already allocated reset-baseline storage.
8. Mark the instance `Ready`.

The compiler still determines all physical state and proc storage statically. Creating a proc from
`init` initializes a preallocated slot; repeatable initialization does not dynamically grow or
replace the instance layout.

Preserving current top-level parameters allows the host to configure initialization:

```text
create
set quality = 4
bind impulse
init
```

Proc parameters and pinned parameters are reconstructed through the normal proc construction and
initialization schedule. External bindings live outside the cleared program state so they remain
available throughout initialization.

The top-level source `init` section is not exposed as an Onda event. Only the host can request full
instance initialization. This avoids allowing ordinary source code to recursively reconstruct the
entire containing instance while it is executing.

## Buffer access from initialization

The semantic restriction that forbids buffers in `init` should be removed.

A top-level `init` body may access top-level buffers. A proc `init(...)` body may access buffers
visible in that proc. Defs called from initialization inherit the buffer access permitted by their
ordinary lexical and effect rules.

When constructing a proc, its buffer arguments and mappings must be established before its
initializer body runs:

```onda
buffers:
  impulse: f32

init:
  convolver = Convolver(impulse = impulse)
```

`Convolver.init(...)` can therefore inspect `impulse` during both full instance initialization and
a later explicit `convolver.init(...)` call.

Unbound buffers retain the language's existing neutral-descriptor behavior. Making buffers visible
to initialization does not make every buffer mandatory. A project or host that requires a resource
must validate or guarantee its binding separately unless Onda later gains required-resource
declarations.

Initialization may write writable external buffers under the ordinary binding contract. Such
writes are externally observable and are repeated whenever that initializer is called. Neither
full nor proc initialization is transactional.

## Proc `init(...)` remains an ordinary runtime operation

The reserved proc `init(...)` operation remains callable from events, defs, `block`, `sample`, and
other proc initializers wherever the existing call graph permits it. In particular, the language
does not reject this merely because the initializer reads a large buffer or performs costly work:

```onda
events:
  reset_convolver():
    convolver.init()

sample:
  # Legal if the program chooses to do it, even if it is a poor realtime tradeoff.
  if should_rebuild:
    convolver.init()
```

A proc initialization call mutates only the target proc state and any other state or external
buffers reachable under ordinary language rules. It does not:

- clear the containing program;
- rerun the top-level `init` schedule;
- recapture the instance reset baseline;
- alter the host-visible lifecycle state.

This preserves the useful role of proc `init(...)` as a runtime reset or reconfiguration operation.
Programs that require a cheaper reset can expose a separate purpose-specific event.

## Threading and realtime contract

Full initialization does not create threads or schedule work. The host calls a synchronous entry
point on whichever thread it chooses:

```text
result = onda_instance_init(instance)
```

The same exclusive-instance rule applies to initialization, events, binding mutation, reset,
snapshots, and processing. None of those operations may overlap on one instance. In particular,
calling full initialization concurrently with `process` is invalid. External buffer memory read or
written by initialization must also remain valid and free from conflicting concurrent mutation.

Subject to that ownership contract, a host may call full initialization from an audio callback,
between process calls, from an activation callback, or from a worker/setup thread. A proc may call
its `init(...)` while executing an event or audio body because that execution is already serialized
within the instance.

This is a safety guarantee, not a scheduling guarantee. Buffer traversal and user-written loops may
take time proportional to externally selected resource sizes. Calling an initializer that builds a
large FFT kernel from an audio callback may miss the deadline and cause an audible glitch, but it is
not a language error or data race.

To keep the host choice genuine, repeat full initialization must not itself allocate or acquire
locks after instance creation. Reset-baseline storage and other required bookkeeping are allocated
with the instance. Generated Onda initialization uses statically allocated state. Any future foreign
call or host-import facility will need an explicit contract for operations whose implementations may
allocate, lock, block, or perform I/O.

Documentation should use the following distinction consistently:

- **safe to call on the audio thread** means the operation preserves memory and concurrency safety;
- **suitable for the audio thread** depends on the initializer's actual work and is the program and
  host's responsibility.

## Rebinding and resource changes

External buffers may be rebound between non-overlapping instance calls in both `NeedsInit` and
`Ready`. Rebinding does not automatically change the lifecycle state, rerun initializers, or restore
state.

This is intentionally host-controlled:

- code that reads a buffer directly during processing needs no reinitialization;
- code that derives cached state from a buffer may require proc or full initialization;
- some hosts may deliberately preserve runtime state across a resource change;
- other hosts may initialize a replacement instance and swap it at a block boundary.

The runtime cannot infer the desired policy merely because an initializer once read a buffer.
Dependency tracking would still not reveal whether a particular content mutation requires a reset,
and implicit full initialization would unexpectedly destroy unrelated runtime state.

The host may therefore choose among:

```text
rebind -> continue processing
rebind -> call affected proc.init(...) -> continue processing
rebind -> call full instance init -> continue processing
initialize replacement instance -> swap or crossfade at a block boundary
```

The final option is host orchestration across two exclusively owned instances, not concurrent use
of one instance.

## Reset and snapshots

Successful full initialization captures the complete post-init physical state as the new reset
baseline. Consequently:

- `onda_reset_instance_state` restores the state produced by the most recent successful full
  initialization;
- calling a proc's `init(...)` does not alter that baseline;
- calling full initialization again replaces the baseline;
- reset and snapshot operations are rejected in `NeedsInit`;
- snapshot restoration starts from the receiving instance's current post-init baseline and overlays
  the packed persistent state according to the existing processor descriptor.

External buffer bindings and contents are not copied into the baseline or snapshots. A host that
restores state derived from a different resource binding is responsible for compatibility, just as
it is responsible for deciding whether rebinding requires initialization.

Runtime queues that represent pending input or output activity should be empty after full
initialization and reset according to their existing reset contract. When delegates are
implemented, full initialization must not expose stale delegate records from the previous runtime
state.

## Host API

The runtime and public C API need an explicit full-initialization operation, illustratively:

```text
Rust: Instance::init(...)
C:    onda_instance_init(...)
```

The final signature should follow existing error-reporting and instance-ownership conventions. The
API must:

- be callable more than once;
- work with the instance's current top-level parameters and external bindings;
- reject overlap through the documented exclusive-access contract;
- report generated initialization failures;
- mark the instance `Ready` and replace the reset baseline only after successful generated
  initialization;
- perform no post-creation allocation or locking.

Reflection should expose whether an instance is `NeedsInit` or `Ready` if hosts cannot already
determine that from their own call sequence. Event and process errors should diagnose missing
initialization distinctly from invalid bindings.

Project-aware creation must attach project-default buffers before the first full initialization.
Run hosts then apply command-line or user overrides, set initial params, call initialization, and
only afterward start processing.

### Rename the existing unchecked-process APIs

The existing `prepare_unchecked_process` and `onda_prepare_unchecked_process` functions do not run
initialization. They only validate that current input, output, and buffer bindings are suitable for
unchecked processing. Their names would remain misleading even though this proposal removes the
`prepare` lifecycle.

Prefer removing the redundant entry points and using the existing validation APIs:

```text
Rust: onda_runtime::validate_bindings
C:    onda_validate_bindings
```

If a distinct convenience function remains useful, name it explicitly:

```text
Rust: validate_bindings_for_unchecked_process
C:    onda_validate_bindings_for_unchecked_process
```

## Compiler and backend contract

The compiler must continue producing one deterministic full initialization entry point. The change
is when the runtime calls it and which resource descriptors it receives, not a second MIR execution
phase.

Required compiler work includes:

- permit buffer expressions and metadata access from top-level and proc initialization;
- ensure proc buffer mappings are installed before calling the proc initializer;
- preserve the existing legality of proc `init(...)` calls in runtime call graphs;
- make generated full initialization repeatable over cleared, preallocated state;
- keep native LLVM and Binaryen behavior identical;
- distinguish the host-only full-instance entry point from callable proc init operations in MIR and
  generated symbols.

The native and WebAssembly runtimes must implement the same `NeedsInit` checks, repeat semantics,
failure behavior, parameter preservation, buffer visibility, and baseline capture.

## Diagnostics and tooling

Diagnostics and documentation should cover:

- processing, events, reset, or snapshots before successful full initialization;
- overlapping host operations on one instance;
- invalid or expired external buffer bindings used during initialization;
- generated bounds or other runtime failures during initialization;
- the fact that expensive proc initialization inside `sample` is legal but may miss realtime
  deadlines;
- the distinction between full instance initialization and `proc.init(...)`.

The compiler should not issue a blanket warning merely because `proc.init(...)` is called from an
audio scope. Such a warning cannot know the intended workload or host budget and would undermine
the deliberately user-controlled contract. Tooling may eventually offer opt-in cost linting if it
can provide actionable evidence.

## Explicit non-goals

This proposal does not include:

- a `prepare` source section or `onda_instance_prepare` lifecycle API;
- implicit worker threads or parallel Onda execution;
- concurrent initialization and processing of one instance;
- automatic initialization or invalidation after rebinding;
- automatic dependency tracking between buffers and derived state;
- a static guarantee that arbitrary initialization meets an audio deadline;
- rollback of state or external-buffer writes after failed initialization;
- dynamic compiler-managed allocation from Onda source;
- automatic instance swapping or crossfading.

## Open implementation decisions

- Whether `onda_instance_create` should return `NeedsInit` directly or whether a lower-level allocate
  API should coexist with a convenience create-and-init API.
- The exact pre-init representation used when clearing physical state before a repeated full init.
- Whether initialization failure needs a distinct reflected state or can return to `NeedsInit` with
  a retained diagnostic.
- How current top-level parameter values are preserved efficiently when parameter storage shares the
  physical state allocation.
- Whether bindings should be validated eagerly on `onda_instance_init` or only when generated code
  accesses them, beyond the existing neutral unbound-buffer semantics.
- How snapshot metadata can help hosts detect restoration across incompatible resource bindings
  without making resource policy implicit.

## Implementation outline

1. Rename or remove the existing unchecked-process prepare APIs and update callers and docs.
2. Split instance allocation from generated initialization and add `NeedsInit` lifecycle checks.
3. Attach project-default resources before initialization and expose explicit runtime/C init entry
   points.
4. Permit buffers in initialization and establish child mappings before proc initializer calls.
5. Make repeated full initialization clear state deterministically, preserve current params and
   bindings, and replace the reset baseline without allocation.
6. Implement matching native and WebAssembly lifecycle behavior and failure diagnostics.
7. Update run hosts, daemon, Web Audio adapters, examples, architecture docs, and snapshot tests.
8. Add conformance tests for buffer-dependent init, runtime proc init, repeated full init, rebinding,
   reset baselines, failure/retry, and deliberate audio-scope proc initialization.
