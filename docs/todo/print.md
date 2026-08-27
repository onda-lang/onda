# `print` and host execution output

## Status

This document proposes a host-independent `print` facility for Onda and a dedicated log view in
the run UIs. It also replaces the run UIs' current delegate occurrence list. Delegates remain a
typed language and processor-interface feature; they are application occurrences, not diagnostic
text.

The design deliberately does not introduce runtime strings, perform text formatting in generated
DSP code, or throttle authored `print` calls.

## Language surface

`print` is a compiler-known, non-value-returning runtime statement with these forms:

```onda
print(value)
print("label")
print("label", value1, value2, ...)
```

The optional leading quoted text is a compile-time label stored in log-site metadata. It is not an
Onda string value and cannot be assigned, passed, returned, or constructed dynamically. It supports
the ordinary quoted-text escapes for quotes, backslashes, newlines, carriage returns, and tabs.

Each executed `print` statement produces exactly one ordered print occurrence. A label followed by
values is rendered as `label: value1 value2 ...`; values without a label are separated by one
space. A label without values is rendered by itself. `print` has no return value and is invalid in
an expression.

`print` is valid anywhere authored runtime statements can execute:

- top-level and proc `init`
- block and sample code
- top-level and proc events
- `when` handlers
- tasks
- runtime defs reached from those contexts

It is invalid in compile-time declarations and `const def` bodies. Graphs remain declarative and do
not gain a print expression.

`print` is variadic compiler functionality rather than an ordinary overloaded `def`. Semantic
analysis resolves the concrete shape of every argument at its call site.

## Printable values

The host formatter uses a canonical structural representation:

```text
true
440.0
[1.0, 2.0, 3.0]
(1.0, 42, true)
Point { x: 10.0, y: 20.0 }
```

The supported values are:

- `f32`, `f64`, `i32`, `i64`, and `bool`
- fixed arrays, including arrays of structs
- primitive slices
- tuples
- structs, recursively formatted with their authored type and field names
- buffers, formatted as descriptors containing their element type, frame count, channel count, and
  sample rate rather than their sample contents

Processor instances and processor arrays are not printable. A processor is a compiler-managed
executable instance rather than an Onda data aggregate: its parameters, authored state, transient
I/O, nested processors, buffers, task continuations, and compiler-generated state do not form one
unambiguous printable value. Printing all of it would also cross the processor's ordinary access
boundary and couple `print` to processor-lowering internals.

Authors instead print the values they intend to observe. Parent code may print accessible processor
parameters and outputs individually; code inside a processor may print its own parameters and state
variables:

```onda
init:
  voice = Voice()

sample:
  value = voice()
  print("voice", voice.freq, voice.gain, value)
```

An untyped or generic print argument is rejected when specialization resolves it to a processor.
The diagnostic identifies the processor value and suggests printing its parameters, outputs, or
state values explicitly:

```text
processor instance 'voice' is not printable; print its parameters,
outputs, or state values explicitly
```

Buffer contents are not printed implicitly.

Numbers use a canonical shortest round-trippable representation. The format must preserve exact
`i64` values and define stable spellings for NaN and infinities. Native and JavaScript formatters
must share fixtures so equivalent records produce equivalent text.

## Execution semantics

`print` is an observable runtime effect. Its arguments are evaluated in source order whether or not
the host supplies print storage. Omitting storage discards only the host-facing occurrence and
cannot remove argument evaluation or other authored effects.

There is no compiler or runtime throttling:

- no sampling, coalescing, deduplication, or per-site rate limit
- no special warning for `print` in sample code
- no producer-side collection or value truncation
- no change to how often the authored statement executes

The author owns the execution cost and is responsible for guarding hot-path prints when desired.
The host owns finite output storage so generated execution never allocates, grows memory, blocks,
or invokes an arbitrary callback. Capacity exhaustion is a delivery failure, not throttling: every
statement still evaluates its arguments and attempts one complete append.

If a record does not fit, it is dropped whole and the saturated overflow count is incremented. A
later smaller record may still use the remaining capacity. Records are never partially serialized.

## Log sites and records

Every source `print` statement becomes a statically described log site. Its descriptor contains:

- a stable zero-based site index
- the optional label
- the source span
- the owning source processor or top-level program
- a recursive formatting shape for every argument
- the fixed payload size, or the minimum size when a primitive slice makes it dynamic

One packed occurrence contains:

```text
u32 log-site index
u32 payload size in bytes
payload bytes
```

The payload contains typed values in argument and structural-field order. Static labels, type
names, field names, and punctuation remain in the site descriptor rather than being repeated in
every occurrence. Primitive slices carry their runtime element count followed by their contiguous
elements. The target descriptor continues to define byte order and pointer properties.

Generated code copies typed data only. Decoding and text formatting happen after generated
execution, outside the DSP code.

## MIR model

MIR represents printing explicitly rather than lowering it to a hidden delegate:

```text
LogSite {
    label,
    source,
    owner,
    value_shapes,
}

StatementKind::PublishLog {
    site,
    arguments,
}
```

`PublishLog` is an observable effect in call-transitive effect analysis and optimization. The MIR
validator verifies that arguments match the site's recursive shapes and that all aggregate reads
obey their normal access rules.

Ordinary MIR scalar, tuple, array, slice, buffer, and struct types provide every formatting shape.
Semantic analysis rejects processor instances before MIR lowering, so printing requires no
synthetic processor-state type or snapshot recipe.

## Separate delegate and print batches

Delegates and prints use independent caller-owned batches. They may share private record-batch
helpers, but they must not share capacity: excessive printing must never cause application delegate
occurrences to be dropped, and delegate traffic must never consume print capacity.

The public print batch mirrors the allocation-free delegate batch contract:

```c
typedef struct onda_print_batch {
  uint8_t* storage;
  uint32_t capacity_bytes;
  uint32_t used_bytes;
  uint32_t record_count;
  uint32_t overflow_count;
} onda_print_batch_t;
```

The raw processor ABI defines its independent `onda_processor_print_batch_t`. Hosted and raw batch
types remain independent even when they have the same physical layout.

## Singular execution output

Generated execution already has ordinary audio/control outputs. Delegates and prints are separate
host-facing occurrence output. A nullable, singular `ExecutionOutput` groups their independent
batches without describing them as a returned computation result:

```c
typedef struct onda_execution_output {
  onda_delegate_batch_t* delegate_batch;
  onda_print_batch_t* print_batch;
} onda_execution_output_t;
```

The raw processor ABI mirrors this as `onda_processor_execution_output_t`. Public entry points use
the singular parameter name `output`; the existing audio pointer table remains `outputs`:

```c
uint32_t onda_processor_init(
  const void* params,
  void* state,
  onda_processor_init_mode_t mode,
  onda_processor_execution_output_t* output
);

uint32_t onda_process(
  void* state,
  const void* params,
  const void* const* inputs,
  void* const* outputs,
  int32_t start_frame,
  int32_t frames,
  int32_t flags,
  void* const* buffers,
  const int32_t* buffer_frames,
  const int32_t* buffer_channels,
  const float* buffer_sample_rates,
  onda_processor_execution_output_t* output
);

uint32_t onda_event_N(
  const void* payload,
  const void* params,
  void* state,
  void* const* buffers,
  const int32_t* buffer_frames,
  const int32_t* buffer_channels,
  const float* buffer_sample_rates,
  onda_processor_execution_output_t* output
);
```

`output`, either batch pointer, and either storage pointer may be null. A null print batch suppresses
only host delivery of print occurrences. A null delegate batch retains the existing rule that
synchronous Onda `when` handlers still execute.

## Rust runtime

The Rust runtime exposes the same singular container without allocating:

```rust
pub struct ExecutionOutput<'batch, 'storage> {
    pub delegate_batch: Option<&'batch mut DelegateBatch<'storage>>,
    pub print_batch: Option<&'batch mut PrintBatch<'storage>>,
}
```

Usage remains explicit:

```rust
let mut delegate_storage = [0_u8; 4 * 1024];
let mut print_storage = [0_u8; 64 * 1024];
let mut delegates = DelegateBatch::from_storage(&mut delegate_storage);
let mut prints = PrintBatch::from_storage(&mut print_storage);

process_checked(
    &mut instance,
    frames,
    ExecutionOutput {
        delegate_batch: Some(&mut delegates),
        print_batch: Some(&mut prints),
    },
)?;

for occurrence in prints.occurrences() {
    // Decode, copy, or format after generated execution.
}
```

Checked, unchecked, segmented-process, event, and initialization APIs all accept
`ExecutionOutput`. Convenience helpers may pass `ExecutionOutput::none()` when the caller consumes
neither stream.

## Initialization and instance construction

Top-level and processor `init` code may print. The generated init entry therefore accepts
`ExecutionOutput`, just like process and event entries.

Hosted instance constructors currently perform full initialization. Constructors must accept an
optional initialization `ExecutionOutput` so those occurrences are not secretly retained inside
the instance. High-level UI, daemon, and CLI constructors supply print storage; callers that do not
care pass no output. Subsequent `onda_init` and Rust initialization calls accept the same output
container.

Instance construction must not allocate a hidden print queue or defer full initialization merely
to make init prints observable.

## Call and failure boundaries

Each generated init, process segment, or event call is one output boundary. It resets the supplied
batch counters before authored execution. Hosted checked APIs reset supplied batches before their
own validation as well, so a validation failure never exposes records from an earlier call.

Delegate failure behavior remains unchanged: generated execution failure clears the delegate batch
because an incomplete application occurrence stream is not successful output.

Print records successfully copied before a generated runtime failure remain available, together
with their overflow count. They are diagnostic output and may explain the failure. The processor
status still reports failure, and instance invalidation follows the ordinary runtime lifecycle.

Segmented hosts may aggregate several call-scoped batches into a larger host-side stream by
supplying each segment with the unused tail of caller-owned storage, as the daemon already does for
delegates. Record order is execution order within each call and host submission order across calls.

## API exposure

Every execution API exposes the same logical output while retaining an appropriate representation
for its layer:

| Layer | Print exposure |
| --- | --- |
| Raw native/AOT processor ABI | Caller-owned print batch plus descriptor log-site metadata |
| `onda_runtime` | `ExecutionOutput` with optional borrowed delegate and print batches |
| Hosted C API | `onda_execution_output_t`, occurrence iteration, metadata queries, and post-call formatting helpers |
| Daemon run session | Owned decoded print entries copied away from realtime storage |
| CLI `run play` / `run render` | Formatted lines on stdout; structured notifications when stdout carries the control protocol |
| Daemon/control JSON | Ordered print notification containing entries and overflow count |
| JavaScript processor ABI | `writePrintBatch`, `readPrintBatch`, `decodePrintRecords`, and canonical formatting |
| Direct Wasm hosts | The same raw batch and descriptor contract as native artifacts |
| Web Audio adapter | Raw records leave generated execution and are exposed through `onPrint` on the main side |
| Native and webview run UIs | A dedicated chronological Log panel |

The hosted C formatter supports a size query followed by formatting into caller-owned storage. The
Rust decoder exposes structured values with a canonical `Display` implementation. JavaScript
decoding preserves exact `i64` and non-finite float values before formatting instead of routing
them through lossy JSON numbers.

Control JSON emits formatted text and source metadata. It must not write ordinary print lines into
the stdout protocol stream. Non-control CLI runs print formatted lines to stdout after each
execution call.

## UI behavior

The native and webview run hosts remove the current Delegates panel. Run-host delegate collection
is then enabled only when another host consumer requires it; internal `when` behavior is unaffected.

A Log panel displays print occurrences in chronological order with:

- monospace formatted text
- optional muted source location and processor ownership context
- clear and auto-follow controls
- explicit generated-batch overflow and audio-to-UI transport-drop counts

The UI may bound and evict its already-delivered display history to control UI memory. That is a
presentation retention policy, not producer throttling, and should be reported separately from
generated batch overflow. The UI does not sample or coalesce incoming occurrences.

Logs are cleared on unload and successful program replacement. Whether an explicit processor
restart clears already-rendered UI history is a run-host presentation decision and does not affect
the execution stream.

## Implementation order

1. Add print syntax, semantic shape resolution, source diagnostics, and documentation.
2. Add MIR log sites and `PublishLog`, including validation and effect analysis.
3. Add raw native and Binaryen print serialization plus singular `ExecutionOutput` ABI plumbing.
4. Add processor descriptor metadata and native/JavaScript decoders with shared formatting fixtures.
5. Thread `ExecutionOutput` through runtime, hosted C, initialization, events, daemon, CLI, and
   segmented processing.
6. Expose raw print transport through direct Wasm and Web Audio APIs without formatting in generated
   DSP.
7. Replace the Delegates panels with the native and webview Log panels.
8. Add cross-backend tests for ordering, every printable shape, processor-value rejection,
   init/process/event delivery, absent output, overflow, execution failure, exact `i64`, non-finite
   floats, and control-protocol routing.
