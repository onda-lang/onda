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
- Operation-status functions return `0` on success, `ONDA_API_ERROR_INVALID_ARGUMENT` for malformed
  scalar arguments or required null handles/pointers, and specific negative codes for runtime
  rejection: uninitialized/invalidated state, unknown indices, missing bindings, size mismatch,
  alignment, shape, sample rate, element type, segment bounds, flags or missing control domains.
  `ONDA_API_ERROR_VALIDATION_FAILED` remains for formatting and unavailable project defaults.
  `ONDA_API_ERROR_ALLOCATION_FAILED` reports failure to provision explicitly requested runtime
  storage.
- Generated execution entry points may additionally return a positive `ONDA_EXECUTION_*` code.
- Count, index, type, and size queries generally return `-1` for invalid input.
- Borrowed-string queries generally return `NULL` for invalid input.
- Optional floating metadata returns NaN when it is absent or invalid.
- Size-query functions write nothing when the destination is `NULL` or too small and return the
  required size.

Query sentinels are separate from the named operation error codes even when they have the same
numeric value. The exact convention for an individual function is documented in `onda.h` when it
differs.

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

Native parameter storage and parameter descriptor `default_bytes` use native byte order. Event and delegate
wire payloads contain no implicit alignment padding and use little-endian byte order. Fixed arrays
are contiguous; a top-level slice is encoded as a little-endian signed `int32_t` element count
followed by its packed field tensors.

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
    onda_instance_create(program, NULL, &diag);
  if (instance == NULL) {
    fprintf(stderr, "%s\n", diag.message ? diag.message : "instance creation failed");
    onda_program_destroy(program);
    onda_diag_dispose(&diag);
    return 1;
  }

  onda_configure_current_thread_audio_fp_mode();
  if (onda_init_checked(instance, ONDA_INIT_FULL, NULL) != ONDA_EXECUTION_OK) {
    onda_instance_destroy(instance);
    onda_program_destroy(program);
    return 1;
  }
  float output[128];
  if (onda_bind_output(instance, 0, output, (int)sizeof(output)) != 0 ||
      onda_process_checked(instance, NULL) != 0) {
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
| Captured documents | `onda_source_manifest_document_count`, `onda_source_manifest_document_info` |
| Successful resolutions | `onda_source_manifest_resolution_count`, `onda_source_manifest_resolution_info` |
| Unresolved references | `onda_source_manifest_unresolved_resolution_count`, `onda_source_manifest_unresolved_resolution_info`, `onda_source_manifest_unresolved_resolution_candidate_path` |
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

- `onda_version` returns a process-lifetime string containing the Onda release version.
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
| `onda_project_image_with_buffer_overrides` | Return a new image with supplied buffer bindings replaced or added, retaining sources, constants, and unmentioned buffers. |
| `onda_project_image_deserialize` | Validate and own a serialized canonical image. |
| `onda_project_image_serialize` | Serialize an image with a two-call size query. |
| `onda_project_image_content_digest` | Borrow the canonical content digest. |
| `onda_project_image_destroy` | Destroy the image. |

`onda_project_image_load_files` accepts `.ondabuffer`, WAV, and inline project buffers. If the file
set contains multiple `.ondaproject` files, select one explicitly; otherwise pass `NULL` and require
an unambiguous project.

`onda_project_image_with_buffer_overrides` takes canonical `onda_project_buffer_asset_t`
assets and leaves the input image unchanged, including on failure. Duplicate
names and invalid assets are rejected. Unreferenced assets are discarded, and
declaration compatibility is checked when compiling the resulting image. Pass
`NULL` with a zero count to obtain an independently owned copy.

### Inspecting and compiling images

| Data | Functions |
| --- | --- |
| Entry and standard-library identity | `onda_project_image_entry`, `onda_project_image_stdlib_digest` |
| Documents | `onda_project_image_document_count`, `onda_project_image_document_info` |
| Resolutions | `onda_project_image_resolution_count`, `onda_project_image_resolution_info` |
| Logical buffers | `onda_project_image_buffer_count`, `onda_project_image_buffer_info` |
| Configuration and compilation | `onda_project_image_inspect_compile_constants`, `onda_project_image_compile` |

Programs compiled from project files or images retain immutable decoded buffer defaults. Instances
use them automatically until the host rebinds or explicitly unbinds the corresponding slot.
Compilation rejects a project-bound asset when reachable Onda code may write it.

### Materializing editable files

`onda_project_image_materialize` returns an owned plan. Inspect it with
`onda_project_materialization_file_count`, `onda_project_materialization_file_info`, then release it with
`onda_project_materialization_destroy`. The record borrows file bytes until plan destruction.

## Program metadata

All metadata belongs to the immutable `onda_program_t`. Returned strings and pointers remain valid
until `onda_program_destroy`. Indices are stable for the program's lifetime.

### Borrowed descriptors

Counts and name-to-index lookups remain available. A single descriptor query returns each
surface's immutable metadata; a null pointer means an invalid program or index.

| Surface | Descriptor query | Record |
| --- | --- | --- |
| Input | `onda_input_info` | `onda_value_info_t` |
| Output | `onda_output_info` | `onda_value_info_t` |
| Control output | `onda_control_output_info` | `onda_value_info_t` |
| Parameter | `onda_param_info` | `onda_value_info_t` |
| State | `onda_state_info` | `onda_value_info_t` |
| Buffer | `onda_buffer_info` | `onda_buffer_info_t` |
| Buffer array | `onda_buffer_array_info` | `onda_buffer_array_info_t` |
| Event | `onda_event_info` | `onda_message_info_t` |
| Delegate | `onda_delegate_info` | `onda_message_info_t` |

Value records contain names, type text, primitive element type, array shape, slot and byte offsets,
exact native default bytes, numeric defaults/ranges, control scale, curve, step, unit and enum JSON.
Optional strings and default bytes are null when absent; optional numeric fields use their
`has_*` flags and NaN where appropriate. `slot_offset` is -1 for state. Enum JSON is serialized
once during compilation and borrowed until program destruction. Its shape is
`{"name":"Mode","members":[{"name":"LowPass","label":"Low Pass"},...]}`; member ordinals
use `i32` storage. `onda_state_total_bytes` reports the persistent snapshot size.

Buffer records describe primitive width, channel constraints and whether reachable code may write
the buffer. Buffer-array records identify a contiguous run of physical slots by `first` and `len`.

`onda_param_normalized_to_plain` and `onda_param_plain_to_normalized` implement control-domain
conversion. Numeric arrays share their element domain. Booleans use the `value >= 0.5` threshold;
unranged numeric conversions return NaN. Raw parameter bytes retain full-width `i64` values;
`double` control values are exact only through `ONDA_MAX_EXACT_HOST_CONTROL_INTEGER`.

### Messages and tensor views

Message records contain a recursive `schema_json`, fixed/minimum packed payload sizes, delegate
record sizes, tensor count and an immutable borrowed `payload_plan`. Fixed sizes are -1 for dynamic messages; event record sizes are -1.
The schema preserves nested structs, tuples, arrays, slices, exact defaults and enum/integer domains.
There is no separate scalar-only parameter projection.

`onda_event_tensor_info` exposes each flattened primitive leaf in dispatch order: path, owning
parameter, primitive type, fixed shape, element product and dynamic outer-slice marker. For fixed
parameters, `fixed_element_count` is the view's element count; for slices, multiply it by the
logical slice length. Paths and shapes are borrowed until program destruction.

`onda_payload_layout` resolves either an event or delegate plan from one nonnegative logical length
per top-level slice, in declaration order. It returns packed payload bytes, aligned preparation
workspace bytes, and the number of regions. Pass `NULL, 0` for the region storage to query only
those totals; otherwise supply caller-owned storage for every region. The query allocates nothing
and checks all lengths, overflow and capacity before writing any output. Failures leave both the
summary and region array unchanged and return the corresponding `ONDA_API_ERROR_*` status.

Regions appear in wire order, with one region per primitive tensor and one additional region per
slice length prefix. `parameter_index` identifies the source parameter; `tensor_index` is the
flattened leaf index or `-1` for a length prefix. Each region reports its element count, packed byte
offset/size and aligned workspace offset. Empty tensors retain zero-byte regions, so region count
depends on schema shape, not array capacity or slice length. Offsets exclude delegate record headers.
For example, `gain: f32, points: f32[], enabled: bool` with three points has four regions at
offsets 0, 4, 8 and 20, and a 21-byte payload:

```c
const onda_message_info_t* event = onda_event_info(program, event_index);
int32_t lengths[] = { 3 };
onda_payload_region_t regions[4];
onda_payload_layout_t layout;
int status = onda_payload_layout(event->payload_plan, lengths, 1,
                                 regions, 4, &layout);
```

The plan is borrowed until program destruction and needs no separate destroy call. Queries may
run concurrently with separate output storage. Reserve `layout.workspace_bytes` with
`onda_instance_reserve_event_workspace` before realtime dispatch when necessary. Layout resolution
does not encode values, normalize domains, or dispatch events. Native tensor views remain available
for synchronous calls that do not need packed transport.

### Print source metadata

`onda_source_file_count` and `onda_source_file_path` expose the artifact-local source table.
`onda_log_site_count` and `onda_log_site_info` expose each concrete print site's label, source span,
lexical owner, declaration, primitive argument types, and fixed payload size. All pointers in
`onda_log_site_info_t` are borrowed from the program.

## Instance lifecycle

### Creation

`onda_instance_create(program, allocator, out_diag)` allocates program-shaped storage, installs
parameter and project-buffer defaults, and returns an uninitialized instance. Pass `NULL` for the
default allocator. Sample rate, block size and channel counts come from the compiled program.
Call `onda_init_checked` explicitly and inspect its execution status. Release the instance with
`onda_instance_destroy`.

Hosts own floating-point thread policy. `onda_configure_current_thread_audio_fp_mode()` optionally
enables the audio policy on the calling thread (FTZ/DAZ on x86). Every explicit call reapplies it.
Call it before initialization, events or processing on each execution thread when desired; hosts
that share a thread with other work own saving/restoring the prior mode. Runtime calls never change
this policy implicitly.

`onda_allocator_t` affects only instance-owned runtime storage. Its `alloc` callback runs
synchronously during creation and explicit `onda_instance_reserve_event_workspace` calls. Realtime
dispatch does not allocate. `free` may run during failed creation, reservation, or destruction.
The context and callbacks must remain valid until every associated
instance is destroyed and must support the threads/concurrency used by the host.

`onda_instance_reserve_event_workspace(instance, capacity_bytes, out_diag)` provisions aligned
input preparation storage outside realtime execution. It returns `0` on success,
`ONDA_API_ERROR_INVALID_ARGUMENT` for a null instance or capacity above 2,147,483,640 bytes—the
largest supported workspace after eight-byte alignment—or `ONDA_API_ERROR_ALLOCATION_FAILED` when
the requested capacity cannot be provisioned. Fixed messages fit the initial capacity; dynamic
messages start with at least 64 KiB. A request above the current capacity allocates replacement
storage and frees the old storage; other requests retain it. Reservation can run before or after
initialization, and allocation failure preserves the existing workspace and processor state.
Capacity rejection during event dispatch does not invalidate the instance.

### Initialization

`onda_init_checked(instance, mode, output)` reruns authored initialization in place:

- `ONDA_INIT_FULL` initializes the complete state and is required before processing a newly created
  uninitialized instance.
- `ONDA_INIT_PRESERVE_PINNED` reinitializes ordinary state while preserving pinned state and task
  continuations. It is valid only after successful full initialization.

Initialization observes the instance's current external-buffer bindings. Project
defaults are installed during creation before explicit initialization; ordinary
unbound slots use their neutral descriptors. Rebinding does not rerun initialization automatically;
call `onda_init_checked` when state derived by an earlier initializer should be recomputed from the
new binding.

The successful path allocates nothing. Checked initialization returns an `ONDA_API_ERROR_*` code
when validation rejects the request before generated code runs. It returns
`ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE` when generated initialization fails; that failure leaves
state indeterminate, and the instance rejects stateful operations until a full initialization or
snapshot restore succeeds.

Initialization, processing, events, binding validation and snapshot operations allocate no error
strings, including rejection and generated-failure paths. Rust execution returns
`Result<u32, RuntimeError>`: a small owned validation error or the generated numeric status.
Formatting and conversion to an allocating `Diagnostic` are explicit host operations. Compiler
errors retain their rich `Diagnostic` values, source locations and messages.

Print records emitted before a generated failure remain available; delegate records are cleared.
Validation rejection preserves state. Full initialization or successful restore recovers invalidated
state; preserve-pinned initialization requires initialized state.

## Parameters and bindings

### Parameter writes

- `onda_set_param_by_index` writes an exact scalar or fixed-array value from native packed bytes.
- `onda_set_param_element_by_index` writes one primitive scalar or fixed-array element from native
  bytes without modifying its siblings.
- `onda_set_param_plain_f64` constrains a scalar plain value to its range and step.
- `onda_set_param_element_plain_f64` applies the same conversion to one scalar or fixed-array
  element.
- `onda_set_param_normalized` converts a normalized `[0, 1]` scalar host value into the declared
  plain domain.
- `onda_set_param_element_normalized` applies the same conversion to one scalar or fixed-array
  element.

Use raw bytes for full-width `i64` parameters and whole-array writes.

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
the buffer descriptor's `may_write` field is true.

Either of these forms unbinds a buffer:

- `sample_rate == 0`, regardless of pointer and shape.
- `ptr == NULL`, `frames == 0`, and `channels == 0`.

An unbound buffer remains processable through neutral one-frame storage: reads return zero and
writes are discarded. `onda_reset_buffer_to_project_default` restores an immutable default retained
from a compiled project.

### Process readiness

Bindings validate their shape, extent, type and alignment and update the affected ABI slots
immediately. Rejected binds preserve the previous binding. Unbinding installs neutral buffer
descriptors immediately. `onda_prepare_process` checks initialization and required audio bindings;
it performs no descriptor rebuild. Successful rebinds preserve readiness; unbinding required audio
or a generated failure revokes it. Host memory must remain valid throughout its binding lifetime.

## Processing

### Checked processing

`onda_process_checked(instance, output)` processes exactly the compiled block size, running both
block hooks and bypassing segment validation. Use the segment API for a shorter prefix or offset.

`onda_process_checked_segment(instance, start_frame, frames, flags, output)` splits one logical
block while keeping full-block bindings:

- The first segment includes `ONDA_PROCESS_BEGIN_BLOCK`.
- The final segment includes `ONDA_PROCESS_END_BLOCK`.
- An unsplit block uses `start_frame = 0` and `ONDA_PROCESS_FULL_BLOCK`.

Generated local frame `i` accesses host frame `start_frame + i`.

Both checked processing functions return an `ONDA_API_ERROR_*` code when request or binding
validation fails before execution. They return `ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE` when
generated code fails; that failure invalidates processor state until successful full initialization
or snapshot restoration.

### Unchecked processing

After successful `onda_prepare_process`, use `onda_process_unchecked` for one complete
compiled block or `onda_process_unchecked_segment` for segmented processing. Calling these without initialized state and complete valid bindings violates the API contract.

Unchecked calls avoid repeated host-boundary validation but do not suppress generated runtime safety
checks. `ONDA_EXECUTION_OK` is success;
`ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE` indicates a generated failure. A runtime failure invalidates
state until successful full initialization or snapshot restoration.

## Events

`onda_trigger_event_by_index_checked` validates instance readiness and preflights packed payload data before running a top-level event. It returns `ONDA_EXECUTION_INPUT_REJECTED` for malformed input or
insufficient workspace and `ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE` for handler failure. Unknown
event indices are deliberately neutral and return success.

`onda_trigger_event_by_index_unchecked` requires successful full initialization and a payload plus
buffer state satisfying the ABI contract. Like unchecked processing, it may return a positive
generated failure code or an `ONDA_API_ERROR_*` code.

`onda_trigger_event_views_by_index_checked` is the native zero-copy alternative. It accepts one
`onda_event_tensor_view_t` per primitive schema leaf, in declaration and depth-first field order.
`element_count` is the number of primitive scalars in that leaf; leaves belonging to the same
top-level slice must therefore imply the same logical length after their fixed inner array extents
are removed. Every leaf is one contiguous, naturally aligned native-endian tensor. Struct slices
therefore use structure-of-arrays storage: one view for each primitive field, rather than strided
views into an array-of-structs allocation.

The checked view call verifies tensor count, shapes, shared lengths, alignment, pointer arithmetic,
and canonical bool and ranged-integer values before executing. Unlike packed wire input, borrowed
views represent logical native values, so noncanonical values are rejected rather than normalized.
It then constructs Onda's read-only slice descriptors over the supplied memory without packing or
copying tensor contents into event workspace. Nonempty regions must remain readable and unchanged
until the synchronous call returns and must not overlap memory written by the event. Empty views may
use a null data pointer. As with all C pointer APIs, the caller owns the validity of the described
allocations.

`onda_trigger_event_views_by_index_unchecked` uses exactly the same canonical SoA representation,
but derives the tensor count from the event schema and performs no instance, buffer, tensor shape,
alignment, or value validation. The instance must be fully initialized with prepared buffer
bindings established by successful binding calls, and
`tensors` must point to the exact schema-derived number of valid views. Violating this contract is
undefined behavior. This is the minimum-overhead entry for hosts that prepare and reuse known-correct
views.

All event functions accept an optional execution output and execute synchronously on the calling thread.
Event payloads use the recursive schema returned by `onda_event_info(...)->schema_json`. Encode parameters in
declaration order and fields depth-first; structs are structure-of-arrays tensors, and each dynamic
slice contributes one little-endian `i32` logical length followed by its leaf tensors. Use
`onda_payload_layout` to resolve dynamic wire regions and preparation workspace, reserve additional
workspace before realtime dispatch when necessary, and treat input rejection separately from
handler failure. The processor remains usable after rejected input.

## Delegates

For collection examples across C, Rust, and raw objects, see the
[host output guide](https://onda-lang.org/docs/host-output/#hosted-c-api).

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

The header size is `ONDA_DELEGATE_RECORD_HEADER_SIZE`. The message descriptor's `payload_bytes` and
`record_bytes` contain exact sizes for fixed delegates (-1 for dynamic messages);
`payload_min_bytes` and `record_min_bytes` include every slice length
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

int status = onda_process_checked(instance, &output);
if (status == ONDA_EXECUTION_OK) {
  onda_batch_cursor_t cursor = {0};
  onda_delegate_occurrence_t occurrence;
  int next;
  while ((next = onda_delegate_batch_next(&delegates, &cursor, &occurrence)) > 0) {
    /* Decode occurrence.payload using occurrence.delegate_index metadata. */
  }
  if (next < 0) handle_malformed_batch();
}
```

Each supplied delegate or print batch descriptor and its non-null storage must be disjoint from the
other batch and from every region accessed during the call, including bound input, output, and
buffer storage plus packed event payloads or native event tensor views. This is a required calling
contract even for checked entry points because ordinary binding and payload validation cannot
establish arbitrary C pointer aliasing.

`onda_delegate_batch_reset` clears counters without changing storage. The runtime host resets every
supplied batch before init or process code; an event entry resets it before input preflight, so
rejected events return empty batches.
`onda_delegate_batch_next` performs linear constant-time cursor iteration, returning `1` for a
record, `0` at the end, and `-1` for invalid or malformed input.
Use the cursor API for linear iteration; consume it through the end to validate the complete batch.

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

int status = onda_process_checked(instance, &output);

onda_owned_string_t text = {0};
onda_diag_t diag = {0};
if (onda_format_print_batch(instance, &prints, &text, &diag) == 0) {
  fwrite(text.data, 1, text.length, stdout);
}
onda_owned_string_dispose(&text);
onda_diag_dispose(&diag);
```

`onda_print_batch_reset` and `onda_print_batch_next` parallel the
delegate helpers, including their result conventions. The occurrence site index resolves through
`onda_log_site_info`.

`onda_format_print_batch` allocates one Onda-owned NUL-terminated string. Its output must be empty on
entry and must later be released with `onda_owned_string_dispose`. `onda_format_print_batch_into`
formats into caller-owned memory without allocation: `out_length` receives the required non-NUL
length, the destination is untouched unless it can hold the complete text plus a trailing NUL, and
the destination must not overlap batch storage. Both return `ONDA_API_ERROR_INVALID_ARGUMENT` for
required null inputs and `ONDA_API_ERROR_VALIDATION_FAILED` for malformed batch contents.

Print and delegate storage are independently nullable and never share capacity. Omitting print
storage suppresses delivery without suppressing argument evaluation. Overflow drops complete
records and increments `overflow_count`. Unlike delegates, print records emitted before a generated
failure remain available for diagnostics.

## State snapshots and control outputs

`onda_state_total_bytes(program)` reports the packed persistent-state snapshot size.

`onda_instance_snapshot_state` copies persistent state, excluding scratch and control-output
mirrors. A `NULL` or undersized destination performs a size query. Invalid arguments return `-1`;
a requested copy returns `ONDA_API_ERROR_UNINITIALIZED` or `ONDA_API_ERROR_INVALIDATED` when state is unavailable.
`onda_instance_restore_state` runs full initialization and then overlays an exact snapshot. Invalid
snapshot or binding input is rejected with an `ONDA_API_ERROR_*` code before execution and preserves
the prior state. A generated initializer failure returns `ONDA_EXECUTION_RUNTIME_SAFETY_FAILURE`
and leaves state indeterminate. The internal initialization is silent: restore does not collect or
publish its print output.

`onda_control_output_read_bytes` copies the latest held value of one top-level `kouts` entry and
uses the same size-query convention.

<!-- BEGIN C API FUNCTION INDEX -->

```text
onda_bind_buffer
onda_bind_input
onda_bind_output
onda_buffer_array_count
onda_buffer_array_info
onda_buffer_asset_decode
onda_buffer_asset_encode
onda_buffer_asset_format_version
onda_buffer_count
onda_buffer_index
onda_buffer_info
onda_compile
onda_compile_constants_at
onda_compile_constants_count
onda_compile_constants_destroy
onda_compile_constants_inputs
onda_compile_file
onda_compile_source_graph
onda_configure_current_thread_audio_fp_mode
onda_control_output_count
onda_control_output_index
onda_control_output_info
onda_control_output_read_bytes
onda_current_stdlib_digest
onda_delegate_batch_next
onda_delegate_batch_reset
onda_delegate_count
onda_delegate_index
onda_delegate_info
onda_diag_dispose
onda_event_count
onda_event_index
onda_event_info
onda_event_tensor_info
onda_format_print_batch
onda_format_print_batch_into
onda_init_checked
onda_input_count
onda_input_index
onda_input_info
onda_inspect_compile_constants
onda_inspect_compile_constants_file
onda_inspect_compile_constants_source_graph
onda_instance_create
onda_instance_destroy
onda_instance_reserve_event_workspace
onda_instance_restore_state
onda_instance_snapshot_state
onda_log_site_count
onda_log_site_info
onda_output_count
onda_output_index
onda_output_info
onda_owned_string_dispose
onda_param_count
onda_param_index
onda_param_info
onda_param_normalized_to_plain
onda_param_plain_to_normalized
onda_payload_layout
onda_prepare_process
onda_print_batch_next
onda_print_batch_reset
onda_process_checked
onda_process_checked_segment
onda_process_unchecked
onda_process_unchecked_segment
onda_program_destroy
onda_project_image_buffer_count
onda_project_image_buffer_info
onda_project_image_capture
onda_project_image_compile
onda_project_image_content_digest
onda_project_image_deserialize
onda_project_image_destroy
onda_project_image_document_count
onda_project_image_document_info
onda_project_image_entry
onda_project_image_format_version
onda_project_image_inspect_compile_constants
onda_project_image_load_files
onda_project_image_materialize
onda_project_image_resolution_count
onda_project_image_resolution_info
onda_project_image_serialize
onda_project_image_stdlib_digest
onda_project_image_with_buffer_overrides
onda_project_materialization_destroy
onda_project_materialization_file_count
onda_project_materialization_file_info
onda_reset_buffer_to_project_default
onda_rewrite_source_references
onda_set_param_by_index
onda_set_param_element_by_index
onda_set_param_element_normalized
onda_set_param_element_plain_f64
onda_set_param_normalized
onda_set_param_plain_f64
onda_source_file_count
onda_source_file_path
onda_source_manifest_count
onda_source_manifest_destroy
onda_source_manifest_document_count
onda_source_manifest_document_info
onda_source_manifest_path
onda_source_manifest_resolution_count
onda_source_manifest_resolution_info
onda_source_manifest_unresolved_count
onda_source_manifest_unresolved_path
onda_source_manifest_unresolved_resolution_candidate_path
onda_source_manifest_unresolved_resolution_count
onda_source_manifest_unresolved_resolution_info
onda_source_manifest_watch_count
onda_source_manifest_watch_path
onda_state_count
onda_state_info
onda_state_total_bytes
onda_trigger_event_by_index_checked
onda_trigger_event_by_index_unchecked
onda_trigger_event_views_by_index_checked
onda_trigger_event_views_by_index_unchecked
onda_version
```

<!-- END C API FUNCTION INDEX -->
