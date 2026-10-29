---
title: Host output integration
description: Collect, decode, and format delegates and prints through bounded execution-output batches.
permalink: /docs/host-output/
section: reference
eyebrow: Host integration
---

# Host output integration

Onda sends top-level delegate occurrences and authored prints to hosts through one optional
`ExecutionOutput`. This guide covers batch sizing, ownership, decoding, and presentation across
C, Rust, raw processor objects, and Web Audio. The
[processor ABI](https://onda-lang.org/docs/processor-api/#call-scoped-execution-output) defines the
physical layout; the language guide defines
[delegates and when](https://onda-lang.org/docs/language/#delegates-and-when) and
[printing](https://onda-lang.org/docs/language/#printing).

## Shared delivery model

Each initialization, process segment, or input-event invocation is a separate collection boundary.
Prepare reusable batch descriptors and storage outside realtime execution. The host resets counters
and the shared sequence before raw init/process entry; generated event entries reset before input
preflight, so rejected input returns empty batches. Hosted runtime wrappers perform the required
reset for their callers.

Delegate and print batches have independent capacity and independently nullable pointers. Omitting
storage suppresses host delivery while payload expressions, synchronous `when` handlers, and
other authored execution still run. Generated code never allocates, grows a batch, blocks, formats
text, or calls an arbitrary host callback. Every descriptor and non-null storage region must be
mutually disjoint and must not overlap other memory accessed by the processor call, including audio,
external buffers, and event input tensors.

Each complete record has a twelve-byte header followed by its packed payload:

```text
u32 delegate_index or site_index
u32 payload_size_bytes
u32 call_local_sequence
payload bytes
```

Headers and print payload scalars use the artifact's target byte order. Delegate payload scalars
and lengths use little-endian wire order.
The sequence counter is shared by both batches. Merge their records by sequence to present one
call's output in authored order; it is not a timeline across calls or segments.

A record that does not fit is dropped whole and increments the saturated `overflow_count`; a later
smaller record may still fit. Overflow in one batch never consumes the other's capacity. Increasing
capacity is a host operation outside realtime execution.

Consume or copy records before another call reuses the storage. Successful calls replace counters;
batches do not accumulate. Snapshots neither retain nor replay batches. Generated execution failure
clears delegates while preserving earlier prints for diagnostics. Failed initialized construction
clears both batches and returns its diagnostic; snapshot restoration initializes silently.

## Delegates

Delegates carry sparse typed occurrences from Onda code to the containing owner and, for top-level
delegates, to the host. Static `when` handlers run synchronously inside generated code. Language
validation forbids delegate publication reachable from init, so initialization's delegate batch
remains empty.

Payloads follow the recursive message schema in parameter order. Struct arrays use one contiguous
tensor per primitive leaf; nested fixed arrays add tensor axes. Each slice contributes a four-byte
logical element count followed by its leaf tensors. Parameter offsets are fixed only through the
first slice length prefix; later offsets are runtime-dependent and decoders must walk the payload.

### Choosing a capacity

The compiler can report the exact size of one fixed-shape delegate record:

```text
record size = 12-byte record header + fixed payload size
```

For a delegate containing slices, it reports only a minimum. Each slice's runtime element count
determines the remaining size. Hosts with an application-level slice limit can add
`maximum_elements * element_size` to the reported minimum.

There is deliberately no `batch_required_bytes` query. A complete process or event call can produce
a runtime-dependent number and combination of occurrences due to state, branches, sample
processing, tasks, loops, and synchronous delegate routing. Static compilation identifies possible
calls, not the exact path of a future invocation.

Choose capacity as a host policy. For fixed records, a common policy is:

```text
capacity = maximum expected records per call * largest possible record size
```

For dynamic records, include the host's expected or enforced slice bounds. A nonzero
`overflow_count` means the returned host stream is incomplete.

## Prints

An authored `print(...)` evaluates scalar arguments in source order and publishes one typed
occurrence. Hosts resolve each `site_index` through descriptor `metadata.log_sites`, and its source
span through `metadata.source_files`. The site supplies the label, owner, declaration, argument
types, and payload size. Formatting happens after generated execution.

Prints preserve concrete scalar types. Unconstrained numeric literals use the ordinary defaults:
`print(3)` records i32 and `print(3.0)` records f32. Use explicit i64/f64 casts when the wider type
is intended. Canonical formatting retains exact i64 values and width-specific floats, and produces
one newline-terminated UTF-8 line per occurrence. Labels escape control characters, including
Unicode line and paragraph separators.

## Hosted C API

### Collect delegates

The hosted API exposes record-inclusive sizing, a reusable batch descriptor, and an occurrence
decoder in [`include/onda.h`](https://github.com/onda-lang/onda/blob/main/include/onda.h):

```c
int delegate = onda_delegate_index(program, "meter");
int exact_record_bytes = onda_delegate_record_bytes(program, delegate);
int minimum_record_bytes = onda_delegate_record_min_bytes(program, delegate);
if (delegate < 0 || minimum_record_bytes < 0) fail_unknown_delegate();
size_t record_budget = exact_record_bytes >= 0
  ? (size_t)exact_record_bytes
  : choose_dynamic_record_budget((size_t)minimum_record_bytes);

/* Allocate before entering the realtime processing loop. */
size_t maximum_records = 64;
if (record_budget == 0 || maximum_records > UINT32_MAX / record_budget) {
  fail_invalid_delegate_capacity();
}
uint32_t capacity = (uint32_t)(maximum_records * record_budget);
uint8_t* storage = malloc((size_t)capacity);
if (storage == NULL) fail_allocation();
onda_delegate_batch_t batch = {
  .storage = storage,
  .capacity_bytes = capacity,
};
onda_execution_output_t output = {
  .delegate_batch = &batch,
  .print_batch = NULL,
};

int status = onda_process_checked(instance, frames, &output);
if (status == 0) {
  onda_batch_cursor_t cursor = {0};
  onda_delegate_occurrence_t occurrence;
  int next;
  while ((next = onda_delegate_batch_next(&batch, &cursor, &occurrence)) > 0) {

    if (occurrence.delegate_index == (uint32_t)delegate &&
        occurrence.payload_size_bytes == sizeof(float)) {
      float value;
      memcpy(&value, occurrence.payload, sizeof(value));
      consume_meter(value);
    }
  }
  if (next < 0) fail_malformed_batch();

  if (batch.overflow_count != 0) {
    report_dropped_delegate_occurrences(batch.overflow_count);
  }
}

/* Free only after processing has stopped. */
free(storage);
```

`onda_delegate_batch_next` returns `1` for a record, `0` at normal exhaustion, and `-1` for invalid
or malformed input. Indexed lookup returns `0` when the requested occurrence is absent and `-1` for
invalid or malformed input.

`onda_delegate_payload_bytes` and `onda_delegate_payload_min_bytes` exclude the record header and
are useful for payload validation. `onda_delegate_record_bytes` and
`onda_delegate_record_min_bytes` include it and are the appropriate allocation queries. Exact-size
queries return `-1` for dynamic payloads; every query returns `-1` for an invalid index.

For runtime-dependent parameter offsets, hosted Rust metadata returns `None`, the C offset query
returns `-1`, and artifact metadata uses `null`.

### Format prints

Allocate reusable storage outside realtime execution and pass it through the singular output:

```c
uint8_t storage[64 * 1024];
onda_print_batch_t prints = {
  .storage = storage,
  .capacity_bytes = sizeof(storage),
};
onda_execution_output_t output = {
  .delegate_batch = NULL,
  .print_batch = &prints,
};

int status = onda_process_checked(instance, frames, &output);

onda_diag_t diag = {0};
onda_owned_string_t text = {0};
if (onda_format_print_batch(instance, &prints, &text, &diag) == 0) {
  fwrite(text.data, 1, text.length, stdout);
}
onda_owned_string_dispose(&text);
onda_diag_dispose(&diag);
```

`onda_format_print_batch` returns an Onda-owned NUL-terminated string; `length` excludes the NUL.
The bytes are independent of the instance and batch. The output value must be empty on entry; the
function never disposes an existing value implicitly. `onda_format_print_batch_into` provides an
allocation-free caller-buffer size query and formatting path. It always reports the required
non-NUL byte length, writes nothing when capacity is insufficient, and writes a trailing NUL when
capacity is sufficient.

For structured handling, use `onda_print_batch_occurrence_at`, `onda_log_site_info`, and the
artifact-local source table exposed by `onda_source_file_count` / `onda_source_file_path`. Site
metadata gives the decoded label, lexical owner, declaration, source span, argument primitive types,
and fixed payload size.
Batch iteration returns `1` for a decoded record, `0` at normal exhaustion or for an out-of-range
indexed lookup, and `-1` for invalid or malformed input.

To collect initialization prints, supply an output to `onda_instance_create_initialized`,
`onda_instance_create_initialized_with_allocator`, or `onda_init_checked`. Allocation-only
constructors do not execute authored code and therefore take no output.

#### Ownership and allocation

The batch descriptor and its storage are entirely caller-owned; they may live on the stack, in a
host arena, or in reusable heap storage, and Onda never frees them. The raw native and Wasm ABIs
follow the same rule for `ExecutionOutput`, both batch descriptors, and their storage.

`onda_format_print_batch_into` writes directly into caller-owned memory and requires no dispose
call. Its destination must not overlap the packed batch storage. The convenience
`onda_format_print_batch` instead creates an Onda-owned string; its output must start zeroed and be
released with `onda_owned_string_dispose` before the value is passed as output again. A caller
buffer must never be installed in `onda_owned_string_t`. Hosts that need custom allocation for
instance-owned runtime state can use `onda_instance_create_with_allocator` or its initialized
variant.

## Rust runtime

### Collect delegates

`onda_runtime::DelegateBatch` owns no allocation. It borrows reusable storage, and its iterator
returns payload views tied to that borrow:

```rust
use onda_runtime::{process_checked, DelegateBatch, ExecutionOutput};

let meter = instance.delegate_index("meter").expect("declared delegate");
let record_budget = match instance.delegate_record_bytes(meter) {
    Some(exact) => exact,
    None => choose_dynamic_record_budget(instance.delegate_record_min_bytes(meter).unwrap()),
};

// Allocate during host setup, not in the audio callback.
let mut storage = vec![0_u8; 64 * record_budget];
let mut batch = DelegateBatch::from_storage(&mut storage);

process_checked(
    &mut instance,
    frames,
    ExecutionOutput {
        delegate_batch: Some(&mut batch),
        print_batch: None,
    },
)?;
for occurrence in batch.occurrences() {
    let occurrence = occurrence?;
    if occurrence.delegate_index as usize == meter {
        consume_meter_payload(occurrence.payload);
    }
}
if batch.overflow_count != 0 {
    report_overflow(batch.overflow_count);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Use `ExecutionOutput::none()` or the corresponding unchecked/event API when collection is not
needed. `Instance::delegate_descriptor` exposes the recursive schema and prepared payload plan for generic
encoders and decoders, including exact `i64` values and nested defaults.

Rust batch iterators yield `Result<Occurrence, BatchDecodeError>`: normal exhaustion ends the
iterator, while an invalid envelope, malformed record, or trailing bytes produces one error.
`DelegateBatch::occurrence` and `PrintBatch::occurrence` similarly return
`Result<Option<Occurrence>, BatchDecodeError>`, keeping an absent index distinct from malformed
storage.

### Format prints

`onda_runtime` exposes the same caller-owned collection and formatting paths:

```rust
use onda_runtime::{format_print_batch_into, process_checked, ExecutionOutput, PrintBatch};

let mut storage = vec![0_u8; 64 * 1024];
let mut text_storage = vec![0_u8; 64 * 1024];
let mut prints = PrintBatch::from_storage(&mut storage);
process_checked(
    &mut instance,
    frames,
    ExecutionOutput {
        delegate_batch: None,
        print_batch: Some(&mut prints),
    },
)?;

let required = format_print_batch_into(&instance, &prints, &mut text_storage)?;
if required <= text_storage.len() {
    print!("{}", std::str::from_utf8(&text_storage[..required]).unwrap());
}
# Ok::<(), onda_frontend::Diagnostic>(())
```

The Rust `_into` formatter writes no trailing NUL and, like the C path, leaves an undersized
destination untouched while returning the required byte count. The allocating `format_print_batch`
convenience returns a `String`. `decode_print_batch` returns ordered typed scalar values and borrowed
site metadata when a host needs custom routing or presentation. `ExecutionOutput::none()` discards
both host-facing streams. `PrintBatch::occurrences()` yields
`Result<PrintOccurrence, BatchDecodeError>`, and indexed lookup returns
`Result<Option<PrintOccurrence>, BatchDecodeError>`, keeping malformed storage distinct from normal
exhaustion or an absent index.

## Raw native ABI

Raw native objects use the independent types and helpers in
[`include/onda_processor_abi.h`](https://github.com/onda-lang/onda/blob/main/include/onda_processor_abi.h). Artifact metadata supplies each
delegate's `payload_size_bytes`, `payload_min_size_bytes`, and parameter layout. Add
`ONDA_PROCESSOR_DELEGATE_RECORD_HEADER_SIZE` when selecting storage capacity.

```c
uint8_t storage[4096];
onda_processor_delegate_batch_t batch = {
  .storage = storage,
  .capacity_bytes = sizeof(storage),
};
onda_processor_execution_output_t output = {
  .delegate_batch = &batch,
  .print_batch = NULL,
};

onda_processor_execution_output_reset(&output);
uint32_t status = onda_process(
  state, params, inputs, outputs, 0, frames, ONDA_PROCESSOR_FULL_BLOCK,
  buffers, buffer_frames, buffer_channels, buffer_sample_rates, &output
);
if (status == ONDA_PROCESSOR_EXECUTION_OK) {
  onda_processor_batch_cursor_t cursor = {0};
  onda_processor_delegate_occurrence_t occurrence;
  int next;
  while ((next = onda_processor_delegate_batch_next(&batch, &cursor, &occurrence)) > 0) {
    consume_delegate(&occurrence);
  }
  if (next < 0) fail_malformed_batch();
}
```

Pass a null `output.delegate_batch` or null output when collection is not required. The raw and
hosted batch types are intentionally independent even though they implement the same logical
record contract and the same `1` / `0` / `-1` iteration and indexed-lookup results.

## JavaScript and Web Audio

### Raw Wasm delegates

`@onda-lang/processor-abi` exports the descriptor metadata, record constants, and complete batch
helpers:

```js
import {
  DELEGATE_RECORD_HEADER_SIZE_BYTES,
  writeDelegateBatch,
  writeExecutionOutput,
  resetExecutionOutput,
  decodeDelegateBatch,
} from "@onda-lang/processor-abi";

const fixedRecordBytes = artifact.metadata.metadata.delegates.map((delegate) =>
  delegate.payload_size_bytes == null
    ? null
    : DELEGATE_RECORD_HEADER_SIZE_BYTES + delegate.payload_size_bytes
);

writeDelegateBatch(memory, batchAddress, storageAddress, capacityBytes);
writeExecutionOutput(memory, outputAddress, batchAddress, 0);
resetExecutionOutput(memory, outputAddress);
const status = exports.onda_process(
  state, params, inputs, outputs, 0, frames, flags,
  buffers, bufferFrames, bufferChannels, bufferSampleRates, outputAddress,
);
if (status === 0) {
  const { occurrences, overflowCount } = decodeDelegateBatch(
    memory,
    batchAddress,
    artifact.metadata,
  );
  if (overflowCount) reportOverflow(overflowCount);
  consumeOccurrences(occurrences);
}
```

Addresses and capacities must fit the artifact's pointer profile. Allocate the batch descriptor and
storage before realtime processing and refresh typed-array views after WebAssembly memory growth.

### Raw Wasm prints

For JavaScript hosts, `@onda-lang/processor-abi` owns the linear-memory mechanics:

```js
writePrintBatch(memory, printBatchAddress, storageAddress, capacityBytes);
writeExecutionOutput(memory, outputAddress, 0, printBatchAddress);
resetExecutionOutput(memory, outputAddress);

const status = exports.onda_process(
  state, params, inputs, outputs, 0, frames, flags,
  buffers, bufferFrames, bufferChannels, bufferSampleRates, outputAddress,
);

const { text, entries, overflowCount } = formatPrintBatch(
  memory,
  printBatchAddress,
  artifact.metadata,
);
```

The formatter preserves exact `i64` values and applies the same width-specific canonical float
formatting as native hosts. Labels use `\0`, `\\`, `\n`, `\r`, and `\t` for the common
escapes; other control characters and the Unicode line and paragraph separators use lowercase
`\u{hex}` escapes. Consequently, every formatted occurrence occupies exactly one physical line.

### Web Audio adapter

`@onda-lang/webaudio` owns the raw batch and posts decoded occurrences off the generated processing
call. Configure its reusable capacity during construction and subscribe outside the worklet:

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  delegateCapacityBytes: 128 * 1024,
});

const unsubscribe = processor.onDelegates(({
  occurrences,
  overflowCount,
  transportDropCount,
}) => {
  for (const occurrence of occurrences) {
    console.log(occurrence.name, occurrence.values);
  }
  if (overflowCount) console.warn(`${overflowCount} delegate records were dropped`);
  if (transportDropCount) {
    console.warn(`${transportDropCount} delegate records were dropped in transport`);
  }
});

// Later:
unsubscribe();
```

The default capacity is 64 KiB. A capacity of zero disables record storage. The worklet passes a
null delegate-batch pointer while there are no listeners. With listeners, it copies bounded raw
record batches into a preallocated `SharedArrayBuffer` ring, which the main side drains before
payload decoding and listener callbacks run. This requires cross-origin isolation in browsers. The adapter
reports generated-capacity loss as `overflowCount` and bounded transport-queue loss separately as
`transportDropCount`.

Decoded `i64` payloads remain full-width integers. Native Rust hosts expose them as
`RunEventValue::I64`, JSON transports encode them as canonical decimal strings, and the Web Audio
adapter exposes JavaScript `BigInt` values. This avoids rounding payloads through an `f64` or
JavaScript `Number` boundary. JSON transports encode non-finite floating-point values as the strings
`"NaN"`, `"Infinity"`, and `"-Infinity"`; the same spellings are accepted for event input. Finite
floats remain JSON numbers.

Prints use the same bounded transport and listener-based collection policy, with no formatting in
the render callback. Subscribe with `processor.onPrint(...)`, or pass the factory's construction-time
`onPrint` option to observe initialization output. A loss-only notification is delivered even when
no later authored print occurs.

## CLI and run hosts

Ordinary `onda run play` and `onda run render` write authored print text to stdout after generated
execution. In `--control-json` mode stdout remains reserved for the control startup handshake,
plus source graphs when `--report-sources` is requested. The localhost control socket sends ordered
`print` notifications with `text`, structured `entries`,
`overflowCount`, and `transportDropCount`.

The GPU run hosts display prints and subscribed top-level delegate occurrences in a
compact chronological Log immediately after the scope. It remains hidden until the first print or
delegate occurrence of the run session, then stays present for that session even when cleared. It
provides clear, bottom-aware automatic following, print source context, and separate print and
delegate loss counts. New output follows the bottom only while the log is already fully scrolled
down.
