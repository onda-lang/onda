---
title: C API reference
description: Compile, inspect, host, process, and integrate Onda programs through libonda.
permalink: /docs/api/
section: reference
eyebrow: Host API
---

# Onda C API reference

The static and shared `libonda` libraries expose the same C API through `include/onda.h`. The API
compiles Onda source, inspects immutable program metadata, creates runtime instances, binds host
memory, drives initialization, events, and processing, and collects delegates and print output.

This document describes the complete hosted-library surface. `include/onda.h` remains the canonical
source for exact declarations. The separately shipped `include/onda_processor_abi.h` describes the
lower-level ABI of a generated processor object; it does not require or expose `libonda` handles.

## Linking

Release SDKs contain both library variants and a CMake package:

```cmake
find_package(Onda CONFIG REQUIRED)
target_link_libraries(my_host PRIVATE Onda::Shared)

# Or link the self-contained static library:
target_link_libraries(my_host PRIVATE Onda::Static)
```

Point `CMAKE_PREFIX_PATH` at the extracted SDK. Both imported targets provide the include directory.
`Onda::Static` also carries its platform system-library requirements. When using `Onda::Shared`, the
application must deploy `onda.dll`, `libonda.so`, or `libonda.dylib` in its normal runtime search
path.

Direct C and C++ users include the same header:

```c
#include <onda.h>
```

The declarations have C linkage when included from C++.

## Conventions

### Handles and ownership

Opaque handles are reference-owning library objects:

| Handle | Created by | Released by |
| --- | --- | --- |
| `onda_program_t` | A successful compile function | `onda_program_destroy` |
| `onda_instance_t` | An instance creation function | `onda_instance_destroy` |
| `onda_source_manifest_t` | Compilation through files or a source graph | `onda_source_manifest_destroy` |
| `onda_compile_constants_t` | A compile-constant inspection function | `onda_compile_constants_destroy` |
| `onda_project_image_t` | Capture, deserialize, or file loading | `onda_project_image_destroy` |
| `onda_project_materialization_plan_t` | `onda_project_image_materialize` | `onda_project_materialization_destroy` |

The lifetime verbs are deliberate:

- `*_destroy` consumes an opaque handle. Passing `NULL` is allowed.
- `*_dispose` releases Onda-owned members of a caller-allocated value and resets the value.
- `*_reset` clears reusable counters without releasing caller-owned storage.
- `alloc` and `free` are names reserved for raw allocator callbacks.

Never pass an Onda-owned pointer to the C runtime `free`. Strings returned from program, manifest,
project-image, and descriptor queries are borrowed and remain valid until their owning handle is
destroyed. Occurrence payloads point into caller-owned batch storage and remain valid only until the
storage is modified or reused.

Programs are immutable and may be queried or used to create instances concurrently. An instance has
one exclusive owner at a time: it may move between threads, but operations on the same instance must
never overlap. Program and instance destruction are not realtime-safe.

### Return values

The common conventions are:

- Pointer-returning constructors return `NULL` on failure.
- Most checked runtime operations return `0` on success and a negative API error on invalid input.
- Unchecked generated entry points may additionally return a positive `ONDA_EXECUTION_*` code.
- Count, index, type, and size queries generally return `-1` for invalid input.
- Borrowed-string queries generally return `NULL` for invalid input.
- Optional floating metadata returns NaN when it is absent or invalid.
- Size-query functions write nothing when the destination is `NULL` or too small and return the
  required size.

The exact convention for an individual function is documented in `onda.h` when it differs.

### Diagnostics

`onda_diag_t` carries a code, source span, message, optional file, and optional trace. Initialize it
to zero, dispose it before reusing it as an output, and dispose it when finished:

```c
onda_diag_t diag = {0};
onda_program_t* program = onda_compile(source, &options, &diag);
if (program == NULL) {
  fprintf(stderr, "%s\n", diag.message ? diag.message : "Onda compilation failed");
}
onda_diag_dispose(&diag);
```

Every non-`NULL` diagnostic string is Onda-owned. Copying `onda_diag_t` copies its pointers, not its
ownership; copied diagnostics must not be disposed independently.

### Primitive and layout identifiers

The metadata and binding APIs use these primitive identifiers:

| Identifier | Native value representation |
| --- | --- |
| `ONDA_PRIMITIVE_F32` | `float` |
| `ONDA_PRIMITIVE_F64` | `double` |
| `ONDA_PRIMITIVE_I32` | `int32_t` |
| `ONDA_PRIMITIVE_I64` | `int64_t` |
| `ONDA_PRIMITIVE_BOOL` | `uint8_t`, exactly `0` or `1` |

Packed hosted payloads use native byte order and contain no implicit alignment padding. Fixed arrays
are contiguous. A slice is encoded as a native `int32_t` element count followed by contiguous
elements.

## Minimal host

This example compiles a processor, creates a fully initialized instance, binds one output, and
processes one complete block:

```c
#include <onda.h>
#include <stdio.h>

int main(void) {
  static const char source[] =
    "params:\n"
    "  gain = 0.5\n"
    "sample:\n"
    "  out1 = gain\n";

  onda_compile_options_t options = {
    .sample_rate = 48000.0f,
    .block_size = 128,
  };
  onda_diag_t diag = {0};
  onda_program_t* program = onda_compile(source, &options, &diag);
  if (program == NULL) {
    fprintf(stderr, "%s\n", diag.message ? diag.message : "compile failed");
    onda_diag_dispose(&diag);
    return 1;
  }

  onda_instance_t* instance =
    onda_instance_create_initialized(program, 0, 1, NULL, &diag);
  if (instance == NULL) {
    fprintf(stderr, "%s\n", diag.message ? diag.message : "instance creation failed");
    onda_program_destroy(program);
    onda_diag_dispose(&diag);
    return 1;
  }

  float output[128];
  if (onda_bind_output(instance, 0, output, (int)sizeof(output)) != 0 ||
      onda_process_checked(instance, 128, NULL) != 0) {
    fprintf(stderr, "processing failed\n");
  }

  onda_instance_destroy(instance);
  onda_program_destroy(program);
  onda_diag_dispose(&diag);
  return 0;
}
```

An instance retains its compiled program internally, so the original program handle may be
destroyed immediately after successful instance creation when no further metadata queries or
instances are needed.

## Compilation

### Compile options

All compilation and configuration-inspection entry points use `onda_compile_options_t`:

- `fast_math != 0` enables LLVM fast-math lowering.
- `sample_rate` must be finite and positive.
- `block_size` must be positive.
- `const_inputs` is `NULL` when `const_input_count` is zero.

The resulting program captures these values; instances do not choose another sample rate or block
size later.

### Source entry points

| Function | Input model |
| --- | --- |
| `onda_compile` | One NUL-terminated source string. |
| `onda_compile_file` | A filesystem `.onda`, `.on`, or `.ondaproject` entry. |
| `onda_compile_source_graph` | An exact in-memory graph of source documents and resolutions. |
| `onda_project_image_compile` | An immutable portable project image. |

`onda_compile_file` resolves imports, includes, project entries, and file-backed assets. Source and
asset paths must not traverse symbolic links. `onda_compile_source_graph` never consults the
filesystem: every non-standard-library reference must have one supplied
`onda_source_graph_resolution_t`.

`onda_rewrite_source_references` rewrites every parsed non-standard-library include/import in one
exact UTF-8 document. It returns the non-NUL output length and supports the ordinary two-call size
query pattern.

### Compile-time configuration

Every source model has a matching configuration inspection function:

- `onda_inspect_compile_constants`
- `onda_inspect_compile_constants_file`
- `onda_inspect_compile_constants_source_graph`
- `onda_project_image_inspect_compile_constants`

Omitted `config const` inputs use their authored defaults. Inspection resolves dependent values and
fixed-array lengths under the partial selection and returns immutable
`onda_compile_const_descriptor_t` values:

```c
onda_compile_constants_t* constants =
  onda_inspect_compile_constants_file("processor.onda", &options, NULL, &diag);
if (constants == NULL) {
  /* Inspect diag and stop. */
}

int count = onda_compile_constants_count(constants);
for (int i = 0; i < count; ++i) {
  const onda_compile_const_descriptor_t* descriptor =
    onda_compile_constants_at(constants, i);
  /* descriptor->input is a valid immutable compile input. */
}

options.const_inputs = onda_compile_constants_inputs(constants);
options.const_input_count = (size_t)count;
onda_program_t* program = onda_compile_file("processor.onda", &options, NULL, &diag);
onda_compile_constants_destroy(constants);
```

`onda_compile_constants_inputs` borrows one contiguous input array from the descriptor handle. The
handle must remain alive through the compile call. To override a value, supply native C storage:

```c
float selected = 0.5f;
onda_compile_const_input_t input = {
  .name_utf8 = "QUALITY",
  .element_type = ONDA_PRIMITIVE_F32,
  .is_array = 0,
  .element_count = 1,
  .values = &selected,
};
```

`ONDA_COMPILE_CONST_KIND_SCALAR`, `ONDA_COMPILE_CONST_KIND_FIXED_ARRAY`, and
`ONDA_COMPILE_CONST_KIND_ARRAY` distinguish the declaration shapes. Arrays use contiguous native
values and retain their resolved element count.

### Source manifests

`onda_compile_file`, its inspection counterpart, and exact-source-graph entry points can return an
owned `onda_source_manifest_t` on success or failure. The manifest is suitable for dependency
watching, diagnostics, source capture, and replay.

| Query family | Functions |
| --- | --- |
| Contributing paths | `onda_source_manifest_count`, `onda_source_manifest_path` |
| Unresolved candidate paths | `onda_source_manifest_unresolved_count`, `onda_source_manifest_unresolved_path` |
| Filesystem watch set | `onda_source_manifest_watch_count`, `onda_source_manifest_watch_path` |
| Captured documents | `onda_source_manifest_document_count`, `onda_source_manifest_document_path`, `onda_source_manifest_document_contents` |
| Successful resolutions | `onda_source_manifest_resolution_count`, `onda_source_manifest_resolution_source_path`, `onda_source_manifest_resolution_kind`, `onda_source_manifest_resolution_specifier`, `onda_source_manifest_resolution_target_path` |
| Unresolved references | `onda_source_manifest_unresolved_resolution_count`, `onda_source_manifest_unresolved_resolution_source_path`, `onda_source_manifest_unresolved_resolution_kind`, `onda_source_manifest_unresolved_resolution_specifier`, `onda_source_manifest_unresolved_resolution_candidate_count`, `onda_source_manifest_unresolved_resolution_candidate_path` |
| Cleanup | `onda_source_manifest_destroy` |

Resolution kinds are `ONDA_SOURCE_REFERENCE_INCLUDE` and `ONDA_SOURCE_REFERENCE_IMPORT`. Document
contents are exact byte slices and need not be NUL-terminated. Filesystem compilation returns
canonical absolute paths; source-graph compilation preserves opaque caller-supplied identities and
has an empty watch set.

## Project images and buffer assets

Project images are immutable, filesystem-independent source graphs with canonical typed buffer
assets. They are useful for portable storage, browser/native interchange, reproducible compilation,
and later materialization into editable files.

### Versions and canonical assets

- `onda_project_image_format_version` returns the supported project-image format version.
- `onda_buffer_asset_format_version` returns the canonical `.ondabuffer` format version.
- `onda_current_stdlib_digest` returns a process-lifetime string identifying the embedded standard
  library.
- `onda_buffer_asset_encode` encodes host-native primitive samples.
- `onda_buffer_asset_decode` validates an asset, reports `onda_buffer_asset_info_t`, and optionally
  writes host-native samples.

The encode/decode functions return an `int64_t` required byte count or `-1` on failure and support
size queries with a `NULL` or undersized destination.

### Creating and serializing images

| Function | Purpose |
| --- | --- |
| `onda_project_image_capture` | Capture a successful source manifest below a source root and associate canonical assets. |
| `onda_project_image_load_files` | Load an editable project from a complete in-memory relative-file set. |
| `onda_project_image_deserialize` | Validate and own a serialized canonical image. |
| `onda_project_image_serialize` | Serialize an image with a two-call size query. |
| `onda_project_image_content_digest` | Borrow the canonical content digest. |
| `onda_project_image_destroy` | Destroy the image. |

`onda_project_image_load_files` accepts `.ondabuffer`, WAV, and inline project buffers. If the file
set contains multiple `.ondaproject` files, select one explicitly; otherwise pass `NULL` and require
an unambiguous project.

### Inspecting and compiling images

| Data | Functions |
| --- | --- |
| Entry and standard-library identity | `onda_project_image_entry`, `onda_project_image_stdlib_digest` |
| Documents | `onda_project_image_document_count`, `onda_project_image_document_path`, `onda_project_image_document_contents` |
| Resolutions | `onda_project_image_resolution_count`, `onda_project_image_resolution_source`, `onda_project_image_resolution_kind`, `onda_project_image_resolution_specifier`, `onda_project_image_resolution_target` |
| Logical buffers | `onda_project_image_buffer_count`, `onda_project_image_buffer_name`, `onda_project_image_buffer_asset_id`, `onda_project_image_buffer_element_type`, `onda_project_image_buffer_frames`, `onda_project_image_buffer_channels`, `onda_project_image_buffer_sample_rate` |
| Configuration and compilation | `onda_project_image_inspect_compile_constants`, `onda_project_image_compile` |

Programs compiled from project files or images retain immutable decoded buffer defaults. Instances
use them automatically until the host rebinds or explicitly unbinds the corresponding slot.
Compilation rejects a project-bound asset when reachable Onda code may write it.

### Materializing editable files

`onda_project_image_materialize` returns an owned plan. Inspect it with
`onda_project_materialization_file_count`, `onda_project_materialization_file_path`, and
`onda_project_materialization_file_bytes`, then release it with
`onda_project_materialization_destroy`. File bytes use the same size-query convention as other
binary output functions.

## Program metadata

All metadata belongs to the immutable `onda_program_t`. Returned strings and pointers remain valid
until `onda_program_destroy`. Indices are stable for the program's lifetime.

### Surface discovery

The primary surfaces follow a regular query matrix:

| Surface | Count | Name | Name to index | Type text | Byte size |
| --- | --- | --- | --- | --- | --- |
| Inputs | `onda_input_count` | `onda_input_name` | `onda_input_index` | `onda_input_type` | `onda_input_type_bytes` |
| Outputs | `onda_output_count` | `onda_output_name` | `onda_output_index` | `onda_output_type` | `onda_output_type_bytes` |
| Control outputs | `onda_control_output_count` | `onda_control_output_name` | `onda_control_output_index` | `onda_control_output_type` | `onda_control_output_type_bytes` |
| Parameters | `onda_param_count` | `onda_param_name` | `onda_param_index` | `onda_param_type` | `onda_param_type_bytes` |
| Buffers | `onda_buffer_count` | `onda_buffer_name` | `onda_buffer_index` | `onda_buffer_type` | element size via `onda_buffer_elem_type_bytes` |
| Events | `onda_event_count` | `onda_event_name` | `onda_event_index` | parameter metadata | `onda_event_payload_bytes` |
| Delegates | `onda_delegate_count` | `onda_delegate_name` | `onda_delegate_index` | parameter metadata | delegate sizing queries |
| State | `onda_state_count` | `onda_state_name` | — | `onda_state_type` | `onda_state_type_bytes` |

Primitive element types are returned by `onda_input_elem_type`, `onda_output_elem_type`,
`onda_control_output_elem_type`, `onda_param_elem_type`, and `onda_state_elem_type`. Corresponding
fixed lengths come from `onda_input_array_len`, `onda_output_array_len`,
`onda_control_output_array_len`, `onda_param_array_len`, and `onda_state_array_len`.

Flattened logical slot offsets use `onda_input_slot_offset`, `onda_output_slot_offset`,
`onda_control_output_slot_offset`, and `onda_param_slot_offset`. Packed byte offsets use
`onda_input_byte_offset`, `onda_output_byte_offset`, `onda_control_output_byte_offset`,
`onda_param_byte_offset`, and `onda_state_byte_offset`. `onda_state_total_bytes` returns the total
persistent snapshot size.

### Defaults, ranges, and parameter controls

Input, output, and parameter defaults use `onda_*_has_default` and `onda_*_default_f64`.
`onda_param_default_bytes` preserves the exact scalar or fixed-array representation and supports a
size query. Event defaults use `onda_event_param_has_default` and
`onda_event_param_default_bytes`.

Input, output, and parameter ranges use `onda_*_has_range`, `onda_*_range_min_f64`, and
`onda_*_range_max_f64`. The parameter-only host-control surface adds:

| Function | Meaning |
| --- | --- |
| `onda_param_scale` | `ONDA_PARAM_SCALE_LINEAR`, `ONDA_PARAM_SCALE_LOG`, or `-1`. |
| `onda_param_has_curve`, `onda_param_curve` | Optional finite lincurve shaping. |
| `onda_param_unit_copy` | Optional NUL-terminated UTF-8 presentation unit. |
| `onda_param_has_step`, `onda_param_step_f64`, `onda_param_step_count` | Discrete host-control grid. |
| `onda_param_normalized_to_plain` | Convert and constrain a normalized host value. |
| `onda_param_plain_to_normalized` | Constrain and convert a plain host value. |

The shared `double` host-control surface exactly represents integers only through
`ONDA_MAX_EXACT_HOST_CONTROL_INTEGER`. Raw parameter bytes retain full-width `i64` values.

### Buffers and buffer arrays

`onda_buffer_array_count`, `onda_buffer_array_name`, `onda_buffer_array_first`, and
`onda_buffer_array_len` preserve logical fixed collections over contiguous physical buffer slots.

`onda_buffer_elem_type`, `onda_buffer_elem_type_bytes`, `onda_buffer_channels_kind`, and
`onda_buffer_channels_static` describe one physical slot. Channel kinds are
`ONDA_BUFFER_CHANNELS_MONO`, `ONDA_BUFFER_CHANNELS_STATIC`, and
`ONDA_BUFFER_CHANNELS_DYNAMIC`. `onda_buffer_may_write` reports whether any reachable generated path
may write the slot, including paths reachable from top-level or proc initialization as well as
events and processing.

### Event and delegate parameters

Events and delegates expose parallel parameter metadata:

| Property | Event | Delegate |
| --- | --- | --- |
| Parameter count | `onda_event_param_count` | `onda_delegate_param_count` |
| Name | `onda_event_param_name` | `onda_delegate_param_name` |
| Primitive type | `onda_event_param_elem_type` | `onda_delegate_param_elem_type` |
| Fixed length / slice marker | `onda_event_param_array_len` | `onda_delegate_param_array_len` |
| Is slice | `onda_event_param_is_slice` | `onda_delegate_param_is_slice` |
| Fixed byte offset | `onda_event_param_offset_bytes` | `onda_delegate_param_offset_bytes` |

A scalar has array length `1`; a slice has length `0`. The first slice itself has a fixed offset to
its length prefix. Every parameter following a slice has a runtime-dependent offset, so the offset
query returns `-1`; decode sequentially from the preceding slice length instead.

`onda_event_payload_bytes` returns the exact payload size for a fixed event and `-1` for a dynamic
event. Delegate sizing is described in [Delegates](#delegates).

### Print source metadata

`onda_source_file_count` and `onda_source_file_path` expose the artifact-local source table.
`onda_log_site_count` and `onda_log_site_info` expose each concrete print site's label, source span,
lexical owner, declaration, primitive argument types, and fixed payload size. All pointers in
`onda_log_site_info_t` are borrowed from the program.

## Instance lifecycle

### Creation

| Function | Initialization | Allocator |
| --- | --- | --- |
| `onda_instance_create` | Writes parameter defaults but does not execute Onda `init`. | Onda allocator |
| `onda_instance_create_initialized` | Performs full initialization. | Onda allocator |
| `onda_instance_create_with_allocator` | Uninitialized. | Host allocator |
| `onda_instance_create_initialized_with_allocator` | Fully initialized. | Host allocator |

Creation validates the requested flattened input and output channel counts. The initialized forms
accept `onda_execution_output_t`, allowing authored initialization prints and delegates to be
collected on success. If initialized creation fails, both output batches are cleared and the
diagnostic is the only result. Release every successful instance with `onda_instance_destroy`.

`onda_allocator_t` affects only instance-owned runtime storage. Its `alloc` callback runs
synchronously during creation; no later operation allocates instance storage. `free` may run during
failed creation or destruction. The context and callbacks must remain valid until every associated
instance is destroyed and must support the threads/concurrency used by the host.

### Initialization

`onda_init(instance, mode, output)` reruns authored initialization in place:

- `ONDA_INIT_FULL` initializes the complete state and is required before processing a newly created
  uninitialized instance.
- `ONDA_INIT_PRESERVE_PINNED` reinitializes ordinary state while preserving pinned state and task
  continuations. It is valid only after successful full initialization.

Initialization prepares and observes the instance's current external-buffer bindings. Project
defaults are installed before initialized project creation runs authored initialization; ordinary
unbound slots use their neutral descriptors. Rebinding does not rerun initialization automatically;
call `onda_init` when state derived by an earlier initializer should be recomputed from the new
binding.

The successful path allocates nothing. A failed initialization leaves state indeterminate; the
instance rejects stateful operations until a full initialization or snapshot restore succeeds.

## Parameters and bindings

### Parameter writes

- `onda_set_param_by_index` writes an exact scalar or fixed-array value from native packed bytes.
- `onda_set_param_plain_f64` constrains a scalar plain value to its range and step.
- `onda_set_param_normalized` converts a normalized `[0, 1]` scalar host value into the declared
  plain domain.

Use raw bytes for full-width `i64` parameters and array parameters.

### Audio bindings

`onda_bind_input` and `onda_bind_output` install zero-copy host memory for one declared entry. The
pointer must be naturally aligned, correctly sized for the program's compile-time block size, and
stable until the entry is rebound, unbound with `NULL` plus zero bytes, or the instance is destroyed.
Generated code accesses the memory directly.

Bound input, output, and buffer regions must not overlap. Checked processing validates required
bindings before execution.

### External buffers

`onda_bind_buffer` installs one zero-copy physical buffer slot using a primitive type, positive
frames/channels, and sample rate. A replacement binding is used by subsequent init, event, and
processing calls without reconstructing the instance. The memory must be writable when
`onda_buffer_may_write` is true.

Either of these forms unbinds a buffer:

- `sample_rate == 0`, regardless of the other fields.
- `ptr == NULL`, `frames == 0`, and `channels == 0`.

An unbound buffer remains processable through neutral one-frame storage: reads return zero and
writes are discarded. `onda_reset_buffer_to_project_default` restores an immutable default retained
from a compiled project.

### Explicit validation

- `onda_validate_inputs` validates input bindings.
- `onda_validate_outputs` validates output bindings.
- `onda_validate_buffers` prepares and validates buffer descriptors, including neutral slots.
- `onda_validate_bindings` performs all three steps.
- `onda_prepare_unchecked_process` validates initialization and every binding for subsequent
  unchecked processing.

Changing a prepared binding invalidates the unchecked-processing contract until preparation runs
again.

## Processing

### Checked processing

`onda_process_checked(instance, frames, output)` processes one complete logical activation. `frames`
must be between zero and the compiled block size; only that prefix of each binding is accessed. The
function runs both block-pre and block-post behavior.

`onda_process_checked_segment(instance, start_frame, frames, flags, output)` splits one logical
block while keeping full-block bindings:

- The first segment includes `ONDA_PROCESS_BEGIN_BLOCK`.
- The final segment includes `ONDA_PROCESS_END_BLOCK`.
- An unsplit block uses `start_frame = 0` and `ONDA_PROCESS_FULL_BLOCK`.

Generated local frame `i` accesses host frame `start_frame + i`.

### Unchecked processing

After successful `onda_prepare_unchecked_process`, use `onda_process_unchecked` for one complete
compiled block or `onda_process_unchecked_segment` for segmented processing. Calling these before
preparation, or after changing a prepared binding, violates the API contract.

Unchecked calls avoid repeated host-boundary validation but do not suppress generated runtime safety
checks. `ONDA_EXECUTION_OK` is success;
`ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE` indicates a generated failure. A runtime failure invalidates
state until successful full initialization or snapshot restoration.

## Events

`onda_trigger_event_by_index` validates the packed payload and current buffer descriptors before
running a top-level event. Unknown event indices are deliberately neutral and return success.

`onda_trigger_event_by_index_unchecked` requires successful full initialization and a payload plus
buffer state satisfying the ABI contract. Like unchecked processing, it may return a positive
generated failure code or a negative API error.

Both functions accept an optional execution output and execute synchronously on the calling thread.
Event payloads pack parameters in declaration order using the scalar, fixed-array, and slice layout
described by program metadata.

## Delegates

Top-level delegates are sparse typed output occurrences. Proc-local routing and `when` handlers run
synchronously inside generated code; optional host collection copies top-level occurrences into a
caller-owned `onda_delegate_batch_t`.

One record contains:

```text
u32 delegate_index
u32 payload_size_bytes
u32 call_local_sequence
payload bytes
```

The header size is `ONDA_DELEGATE_RECORD_HEADER_SIZE`. `onda_delegate_payload_bytes` and
`onda_delegate_record_bytes` return exact sizes for fixed delegates. Dynamic delegates return `-1`;
`onda_delegate_payload_min_bytes` and `onda_delegate_record_min_bytes` include every slice length
prefix but no runtime slice elements. There is no exact whole-call capacity because occurrence
count, selected delegates, and slice lengths may all depend on runtime control flow.

```c
uint8_t delegate_storage[64 * 1024];
onda_delegate_batch_t delegates = {
  .storage = delegate_storage,
  .capacity_bytes = sizeof(delegate_storage),
};
onda_execution_output_t output = {
  .delegate_batch = &delegates,
  .print_batch = NULL,
};

int status = onda_process_checked(instance, frames, &output);
if (status == ONDA_EXECUTION_OK) {
  onda_batch_cursor_t cursor = {0};
  onda_delegate_occurrence_t occurrence;
  while (onda_delegate_batch_next(&delegates, &cursor, &occurrence)) {
    /* Decode occurrence.payload using occurrence.delegate_index metadata. */
  }
}
```

`onda_delegate_batch_reset` clears counters without changing storage. The runtime host resets every
supplied batch before entering generated code. `onda_delegate_batch_next` performs linear constant-time cursor
iteration; `onda_delegate_batch_occurrence_at` is convenient for one index but repeated indexed
iteration is quadratic.

Missing storage suppresses only the host copy. Calls and internal handlers still run. Records are
appended whole; insufficient capacity drops the record and increments saturated `overflow_count`.
Successful generated execution leaves the batch for the host to consume before reuse. Generated
failure clears delegate results.

## Printing

Authored `print(...)` statements emit typed scalar occurrences into an independent caller-owned
`onda_print_batch_t`. Generated code does not allocate or format text. One record contains a
`uint32_t` log-site index, a `uint32_t` payload size, a `uint32_t` call-local sequence, and packed
scalar arguments; the header size is `ONDA_PRINT_RECORD_HEADER_SIZE`. When both streams are exposed
through one log, merge their decoded occurrences by `sequence` within that generated call.

```c
uint8_t print_storage[64 * 1024];
onda_print_batch_t prints = {
  .storage = print_storage,
  .capacity_bytes = sizeof(print_storage),
};
onda_execution_output_t output = {
  .delegate_batch = NULL,
  .print_batch = &prints,
};

int status = onda_process_checked(instance, frames, &output);

onda_owned_string_t text = {0};
onda_diag_t diag = {0};
if (onda_format_print_batch(instance, &prints, &text, &diag) == 0) {
  fwrite(text.data, 1, text.length, stdout);
}
onda_owned_string_dispose(&text);
onda_diag_dispose(&diag);
```

`onda_print_batch_reset`, `onda_print_batch_next`, and `onda_print_batch_occurrence_at` parallel the
delegate helpers. The occurrence site index resolves through `onda_log_site_info`.

`onda_format_print_batch` allocates one Onda-owned NUL-terminated string. Its output must be empty on
entry and must later be released with `onda_owned_string_dispose`. `onda_format_print_batch_into`
formats into caller-owned memory without allocation: `out_length` receives the required non-NUL
length, the destination is untouched unless it can hold the complete text plus a trailing NUL, and
the destination must not overlap batch storage.

Print and delegate storage are independently nullable and never share capacity. Omitting print
storage suppresses delivery without suppressing argument evaluation. Overflow drops complete
records and increments `overflow_count`. Unlike delegates, print records emitted before a generated
failure remain available for diagnostics.

## State snapshots and control outputs

`onda_instance_state_bytes` reports the current instance snapshot size;
`onda_state_total_bytes` reports the same packed persistent-state size from program metadata.

`onda_instance_snapshot_state` copies persistent state, excluding scratch and control-output
mirrors. A `NULL` or undersized destination performs a size query. `onda_instance_restore_state`
runs full initialization and then overlays an exact snapshot. Restore failure leaves state
indeterminate.

`onda_control_output_read_bytes` copies the latest held value of one top-level `kouts` entry and
uses the same size-query convention.

<!-- BEGIN C API FUNCTION INDEX -->

## Complete function index

The groups below enumerate every function exported by `include/onda.h`. The earlier sections provide
the behavioral contracts; the header provides exact parameter types.

### Results, diagnostics, and batch utilities

```text
onda_delegate_batch_reset
onda_delegate_batch_next
onda_delegate_batch_occurrence_at
onda_print_batch_reset
onda_print_batch_next
onda_print_batch_occurrence_at
onda_format_print_batch
onda_format_print_batch_into
onda_owned_string_dispose
onda_diag_dispose
```

### Compilation and source graphs

```text
onda_inspect_compile_constants
onda_compile_constants_count
onda_compile_constants_at
onda_compile_constants_inputs
onda_compile_constants_destroy
onda_compile
onda_compile_file
onda_inspect_compile_constants_file
onda_compile_source_graph
onda_inspect_compile_constants_source_graph
onda_rewrite_source_references
onda_source_manifest_count
onda_source_manifest_path
onda_source_manifest_unresolved_count
onda_source_manifest_unresolved_path
onda_source_manifest_watch_count
onda_source_manifest_watch_path
onda_source_manifest_document_count
onda_source_manifest_document_path
onda_source_manifest_document_contents
onda_source_manifest_resolution_count
onda_source_manifest_resolution_source_path
onda_source_manifest_resolution_kind
onda_source_manifest_resolution_specifier
onda_source_manifest_resolution_target_path
onda_source_manifest_unresolved_resolution_count
onda_source_manifest_unresolved_resolution_source_path
onda_source_manifest_unresolved_resolution_kind
onda_source_manifest_unresolved_resolution_specifier
onda_source_manifest_unresolved_resolution_candidate_count
onda_source_manifest_unresolved_resolution_candidate_path
onda_source_manifest_destroy
```

### Project images and assets

```text
onda_project_image_format_version
onda_buffer_asset_format_version
onda_current_stdlib_digest
onda_buffer_asset_encode
onda_buffer_asset_decode
onda_project_image_capture
onda_project_image_deserialize
onda_project_image_load_files
onda_project_image_serialize
onda_project_image_content_digest
onda_project_image_entry
onda_project_image_stdlib_digest
onda_project_image_document_count
onda_project_image_document_path
onda_project_image_document_contents
onda_project_image_resolution_count
onda_project_image_resolution_source
onda_project_image_resolution_kind
onda_project_image_resolution_specifier
onda_project_image_resolution_target
onda_project_image_buffer_count
onda_project_image_buffer_name
onda_project_image_buffer_asset_id
onda_project_image_buffer_element_type
onda_project_image_buffer_frames
onda_project_image_buffer_channels
onda_project_image_buffer_sample_rate
onda_project_image_inspect_compile_constants
onda_project_image_compile
onda_project_image_materialize
onda_project_materialization_file_count
onda_project_materialization_file_path
onda_project_materialization_file_bytes
onda_project_materialization_destroy
onda_project_image_destroy
```

### Programs, instances, execution, and state

```text
onda_program_destroy
onda_instance_create
onda_instance_create_initialized
onda_instance_create_with_allocator
onda_instance_create_initialized_with_allocator
onda_instance_destroy
onda_set_param_by_index
onda_set_param_plain_f64
onda_set_param_normalized
onda_trigger_event_by_index
onda_trigger_event_by_index_unchecked
onda_bind_input
onda_bind_output
onda_bind_buffer
onda_reset_buffer_to_project_default
onda_process_checked
onda_process_checked_segment
onda_init
onda_instance_state_bytes
onda_instance_snapshot_state
onda_instance_restore_state
onda_control_output_read_bytes
onda_validate_bindings
onda_validate_inputs
onda_validate_outputs
onda_validate_buffers
onda_prepare_unchecked_process
onda_process_unchecked
onda_process_unchecked_segment
```

### Program metadata

```text
onda_input_count
onda_output_count
onda_control_output_count
onda_param_count
onda_buffer_count
onda_buffer_array_count
onda_event_count
onda_delegate_count
onda_source_file_count
onda_source_file_path
onda_log_site_count
onda_log_site_info
onda_state_count
onda_input_name
onda_output_name
onda_control_output_name
onda_param_name
onda_buffer_name
onda_buffer_array_name
onda_buffer_array_first
onda_buffer_array_len
onda_event_name
onda_delegate_name
onda_state_name
onda_event_param_count
onda_event_param_name
onda_delegate_param_count
onda_delegate_param_name
onda_input_index
onda_output_index
onda_control_output_index
onda_param_index
onda_buffer_index
onda_event_index
onda_delegate_index
onda_input_type
onda_output_type
onda_control_output_type
onda_param_type
onda_buffer_type
onda_state_type
onda_input_type_bytes
onda_output_type_bytes
onda_control_output_type_bytes
onda_param_type_bytes
onda_state_type_bytes
onda_event_payload_bytes
onda_delegate_payload_bytes
onda_delegate_payload_min_bytes
onda_delegate_record_bytes
onda_delegate_record_min_bytes
onda_event_param_elem_type
onda_event_param_array_len
onda_event_param_is_slice
onda_event_param_offset_bytes
onda_event_param_has_default
onda_event_param_default_bytes
onda_delegate_param_elem_type
onda_delegate_param_array_len
onda_delegate_param_is_slice
onda_delegate_param_offset_bytes
onda_buffer_elem_type
onda_buffer_elem_type_bytes
onda_buffer_channels_kind
onda_buffer_channels_static
onda_buffer_may_write
onda_input_elem_type
onda_output_elem_type
onda_control_output_elem_type
onda_param_elem_type
onda_state_elem_type
onda_input_array_len
onda_output_array_len
onda_control_output_array_len
onda_param_array_len
onda_state_array_len
onda_input_slot_offset
onda_output_slot_offset
onda_control_output_slot_offset
onda_param_slot_offset
onda_input_byte_offset
onda_output_byte_offset
onda_control_output_byte_offset
onda_param_byte_offset
onda_state_byte_offset
onda_state_total_bytes
onda_input_has_default
onda_output_has_default
onda_param_has_default
onda_param_default_bytes
onda_input_default_f64
onda_output_default_f64
onda_param_default_f64
onda_input_has_range
onda_output_has_range
onda_param_has_range
onda_input_range_min_f64
onda_input_range_max_f64
onda_output_range_min_f64
onda_output_range_max_f64
onda_param_range_min_f64
onda_param_range_max_f64
onda_param_scale
onda_param_has_curve
onda_param_curve
onda_param_unit_copy
onda_param_has_step
onda_param_step_f64
onda_param_step_count
onda_param_normalized_to_plain
onda_param_plain_to_normalized
```

<!-- END C API FUNCTION INDEX -->
