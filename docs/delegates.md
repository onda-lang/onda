---
title: Hosting delegates
description: Collect, size, decode, and handle overflow for Onda delegate occurrences.
permalink: /docs/delegates/
section: reference
eyebrow: Host integration
---

# Hosting Onda delegates

Delegates carry sparse typed occurrences from Onda code to its containing owner and, for top-level
delegates, to the host. They are the opposite-direction companion to input events. Static `when`
handlers run synchronously inside generated code; optional host collection copies top-level
occurrences into bounded caller-owned storage.

For language syntax and ownership rules, see [the language guide](syntax.md#delegates-and-when).
For raw generated entry-point layouts, see [the processor ABI](processor-abi.md).
For the independent diagnostic stream carried by the same execution-output container, see
[Hosting Onda print output](printing.md).

## Delivery model

Every process segment and input-event dispatch is one independent collection boundary:

1. The caller optionally supplies a delegate batch.
2. Generated execution resets its `used_bytes`, `record_count`, and `overflow_count`.
3. Each top-level occurrence is appended as one complete packed record when it fits.
4. The caller consumes or copies successful records before reusing the batch.

Passing `None` in Rust or `NULL` in C suppresses only the host-facing copy. Payload expressions and
synchronous `when` handlers still run. Generated execution never allocates, grows the storage,
blocks, or calls an arbitrary host callback.

One packed record contains:

```text
u32 delegate index
u32 payload size in bytes
payload bytes
```

The fixed record header is therefore eight bytes. Scalars and fixed arrays are packed in parameter
order. A slice contributes a four-byte element count followed by its contiguous element bytes.
Native hosted APIs use native byte order; complete WebAssembly artifacts use the byte order declared
by their processor descriptor.

## Choosing a capacity

The compiler can report the exact size of one fixed-shape delegate record:

```text
record size = 8-byte record header + fixed payload size
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
`overflow_count` means the returned host stream is incomplete. The record that did not fit is
dropped whole, internal `when` handlers still run, and a later smaller record may still fit.
Increasing capacity requires changing caller-owned storage outside realtime execution; the
processor never resizes it.

## Hosted C API

The hosted API exposes record-inclusive sizing, a reusable batch descriptor, and an occurrence
decoder in [`include/onda.h`](../include/onda.h):

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
  while (onda_delegate_batch_next(&batch, &cursor, &occurrence)) {

    if (occurrence.delegate_index == (uint32_t)delegate &&
        occurrence.payload_size_bytes == sizeof(float)) {
      float value;
      memcpy(&value, occurrence.payload, sizeof(value));
      consume_meter(value);
    }
  }

  if (batch.overflow_count != 0) {
    report_dropped_delegate_occurrences(batch.overflow_count);
  }
}

/* Free only after processing has stopped. */
free(storage);
```

`onda_delegate_payload_bytes` and `onda_delegate_payload_min_bytes` exclude the record header and
are useful for payload validation. `onda_delegate_record_bytes` and
`onda_delegate_record_min_bytes` include it and are the appropriate allocation queries. Exact-size
queries return `-1` for dynamic payloads; every query returns `-1` for an invalid index.

Passing `NULL` as `output.delegate_batch`, or passing a null execution output, is the ordinary path
when the host does not consume delegates. The same singular execution output is available on
initialization, checked, unchecked, segmented-process, and input-event functions; its independent
`print_batch` pointer may be present or absent without affecting delegate capacity.

## Rust runtime API

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
    if occurrence.delegate_index as usize == meter {
        consume_meter_payload(occurrence.payload);
    }
}
if batch.overflow_count != 0 {
    report_overflow(batch.overflow_count);
}
# Ok::<(), onda_frontend::Diagnostic>(())
```

Use `ExecutionOutput::none()` or the corresponding unchecked/event API when collection is not
needed. `Instance::delegate_descriptor` exposes parameter shapes for generic payload decoders.

## Raw processor ABI

Raw native objects use the independent types and helpers in
[`include/onda_processor_abi.h`](../include/onda_processor_abi.h). Artifact metadata supplies each
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

uint32_t status = onda_process(
  state, params, inputs, outputs, 0, frames, ONDA_PROCESSOR_FULL_BLOCK,
  buffers, buffer_frames, buffer_channels, buffer_sample_rates, &output
);
if (status == ONDA_PROCESSOR_EXECUTION_OK) {
  onda_processor_batch_cursor_t cursor = {0};
  onda_processor_delegate_occurrence_t occurrence;
  while (onda_processor_delegate_batch_next(&batch, &cursor, &occurrence)) {
    consume_delegate(&occurrence);
  }
}
```

Pass a null `output.delegate_batch` or null output when collection is not required. The raw and
hosted batch types are intentionally independent even though they implement the same logical
record contract.

## JavaScript processor ABI

`@onda-lang/processor-abi` exports the descriptor metadata, record constants, and complete batch
helpers:

```js
import {
  DELEGATE_RECORD_HEADER_SIZE_BYTES,
  writeDelegateBatch,
  writeExecutionOutput,
  readDelegateBatch,
  decodeDelegateRecords,
} from "@onda-lang/processor-abi";

const fixedRecordBytes = artifact.metadata.metadata.delegates.map((delegate) =>
  delegate.payload_size_bytes == null
    ? null
    : DELEGATE_RECORD_HEADER_SIZE_BYTES + delegate.payload_size_bytes
);

writeDelegateBatch(memory, batchAddress, storageAddress, capacityBytes);
writeExecutionOutput(memory, outputAddress, batchAddress, 0);
const status = exports.onda_process(
  state, params, inputs, outputs, 0, frames, flags,
  buffers, bufferFrames, bufferChannels, bufferSampleRates, outputAddress,
);
if (status === 0) {
  const batch = readDelegateBatch(memory, batchAddress);
  const storage = new Uint8Array(memory.buffer, storageAddress, batch.usedBytes);
  const occurrences = decodeDelegateRecords(
    storage,
    batch.usedBytes,
    artifact.metadata.metadata.delegates,
    artifact.metadata.target.byte_order,
  );
  if (batch.overflowCount) reportOverflow(batch.overflowCount);
  consumeOccurrences(occurrences);
}
```

Addresses and capacities must fit the artifact's pointer profile. Allocate the batch descriptor and
storage before realtime processing and refresh typed-array views after WebAssembly memory growth.

## Web Audio adapter

`@onda-lang/webaudio` owns the raw batch and posts decoded occurrences off the generated processing
call. Configure its reusable capacity during construction and subscribe outside the worklet:

```js
const processor = await createOndaAudioProcessorInitialized(context, artifact, {
  delegateCapacityBytes: 128 * 1024,
});

const unsubscribe = processor.onDelegates(({ occurrences, overflowCount }) => {
  for (const occurrence of occurrences) {
    console.log(occurrence.name, occurrence.values);
  }
  if (overflowCount) console.warn(`${overflowCount} delegate records were dropped`);
});

// Later:
unsubscribe();
```

The default capacity is 64 KiB. A capacity of zero disables record storage. Listeners run on the
main-side message handler, not inside generated DSP execution.

Decoded `i64` payloads remain full-width integers. Native Rust hosts expose them as
`RunEventValue::I64`, JSON transports encode them as canonical decimal strings, and the Web Audio
adapter exposes JavaScript `BigInt` values. This avoids rounding payloads through an `f64` or
JavaScript `Number` boundary.

## Lifetime and failure rules

- Consume or copy occurrences before the next call that reuses the batch.
- A successful call replaces all result counters; batches do not accumulate across calls.
- Segmented processing returns one batch per segment. Combining segments or blocks is a host policy.
- A generated execution failure clears the batch before returning and invalidates hosted runtime
  state according to the normal process/event lifecycle.
- Snapshot and restore operations do not retain or replay delegate batches.
- Missing storage, overflow, and listener configuration never change authored Onda control flow.
