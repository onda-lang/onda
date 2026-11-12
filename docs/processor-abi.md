---
title: Processor API and ABI reference
description: Host compiled Onda processor objects directly through the portable native and WebAssembly ABI.
permalink: /docs/processor-api/
section: reference
eyebrow: Processor integration
---

# Processor API and ABI reference

Use the Processor API to run a compiled Onda processor directly in a native application or a
WebAssembly runtime. Your host owns storage, audio scheduling, and event delivery. Native objects
require the header
[`onda_processor_abi.h`](https://github.com/onda-lang/onda/blob/main/include/onda_processor_abi.h)
and your platform linker; they do not require `libonda`.

For managed instance storage and checked native calls, use the
[C API](https://onda-lang.org/docs/api/). For browser audio playback, use the
[Web Audio adapter](https://onda-lang.org/docs/web-api/#onda-langwebaudio).

## Build and host a processor

Compile a program to an object and its matching descriptor:

```bash
onda compile patch.onda --emit obj --output patch.o --meta-out patch.onda.json
```

Use a target spec or `--target-triple` when compiling for another platform. Link the object with your
application using that platform's compiler and linker.

A host then:

1. Reads the descriptor and verifies its versions and target.
2. Allocates parameter and state storage with the reported sizes and alignments.
3. Writes parameter defaults and prepares audio and external-buffer tables.
4. Calls `onda_processor_init` with `FULL` and the current buffer bindings.
5. Calls `onda_process` for each block, checking its result before using the output.
6. Releases host-owned storage when processing is finished.

The [direct native example](https://github.com/onda-lang/onda/tree/main/examples/native/raw_processor_object)
includes a complete C host and link commands for Linux, macOS, and Windows. It reads sizes, offsets,
and defaults from the descriptor rather than hard-coding them.

## Versioned artifact descriptor

The paired JSON descriptor describes how to load and call the processor:

- `format: "onda-processor"` and `format_version` for the descriptor schema.
- `abi_version` for the logical entry-point and storage contract in this document.
- `artifact_kind`, currently `relocatable_object` or `webassembly_module`.
- `target`, including the resolved triple, pointer width, byte order, pointer model, and calling
  convention.
- `integration`, including symbols that must survive integration and an artifact-specific profile.
- `compile`, `exports`, `runtime`, and `metadata` for specialization, symbol names, allocation, and
  the program interface.

Hosts must reject unknown descriptor or ABI versions. Use the descriptor from the same compilation
as the object or module; storage layouts can differ between targets and compilations.

## Logical processor interface

The signatures below use `Ptr`, an abstract pointer to host-owned storage. `i32` arguments are
signed 32-bit values. Public LLVM entry points use the target's C calling convention; complete core
WebAssembly modules use ordinary core-Wasm function calls.

```text
onda_processor_init(
  params: Ptr,
  state: Ptr,
  mode: InitMode,
  buffers: Ptr,
  buffer_frames: Ptr,
  buffer_channels: Ptr,
  buffer_sample_rates: Ptr,
  output: Ptr,
) -> i32

onda_process(
  state: Ptr,
  params: Ptr,
  inputs: Ptr,
  outputs: Ptr,
  start_frame: i32,
  frames: i32,
  flags: i32,
  buffers: Ptr,
  buffer_frames: Ptr,
  buffer_channels: Ptr,
  buffer_sample_rates: Ptr,
  output: Ptr,
) -> i32

onda_event_N(
  input: Ptr,  // const EventInput*
  params: Ptr,
  state: Ptr,
  buffers: Ptr,
  buffer_frames: Ptr,
  buffer_channels: Ptr,
  buffer_sample_rates: Ptr,
  output: Ptr,
) -> i32
```

Every packed event call requires an `EventInput` descriptor:

```c
struct EventInput {
  const uint8_t *payload;
  uint32_t payload_bytes;
  uint8_t *workspace;
  uint32_t workspace_capacity_bytes;
};
```

It uses target C layout (16 bytes on wasm32, 32 bytes on native 64-bit targets). The descriptor
itself is required even for an empty payload. Workspace starts at an eight-byte-aligned address;
its capacity is provisioned by the host before realtime execution. Input, workspace, descriptor,
state, parameters, and output storage are disjoint for the call. Native callers supply valid memory
regions; wasm entry points also check regions against linear-memory bounds.

Before running a handler, the entry validates lengths, byte extents, exact input consumption,
alignment, and workspace capacity. Rejection returns status `2` with empty output batches and leaves
workspace and processor state unchanged. Accepted input is copied into aligned native tensors;
bools become zero or one and ranged integers are normalized before observation. Status `1` means
execution failed after entry and requires the usual instance recovery. Neither path allocates.

There is one `onda_event_N` for each declared event, in metadata order. Use `exports` in the
descriptor to find the entry points for the compiled processor.

Native relocatable objects also advertise one trusted `onda_event_views_N` entry per event through
the optional `exports.event_views` array. It has the same trailing arguments and result as
`onda_event_N`, but its first argument is a pointer to the schema-derived array below:

```c
struct onda_processor_event_tensor_view {
  const void *data;
  int32_t element_count;
};
```

Each view is one contiguous, naturally aligned, native-endian primitive tensor in canonical
depth-first SoA leaf order. `element_count` counts primitive scalars; for a slice leaf it is the
slice length times that leaf's fixed inner element count. The entry borrows every tensor only for
the synchronous call, resets call-scoped output, and neither allocates nor uses an event workspace.
It deliberately does not receive a tensor count or validate shape, alignment, canonical bool and
ranged-integer values, storage overlap, or the other processor pointers. Passing anything other
than the exact views and processor storage required by the paired descriptor is undefined behavior.
Use `onda_event_N` at trust boundaries.

This entry is a native-object capability, not part of the WebAssembly profiles. Native hosts can
derive its flat tensor contract once from the event schema with
`onda_processor_abi::payload::PayloadPlan`; JavaScript hosts can inspect the equivalent
`PayloadPlan.tensorMetadata`. The metadata records expose each leaf's path, primitive encoding,
fixed shape, owning top-level parameter, slice status, and fixed element count.

`InitMode` has two values: `PRESERVE_PINNED = 0` and `FULL = 1`. Use `FULL` before processing new
storage. It clears state and runs initialization. Use `PRESERVE_PINNED` only after successful full
initialization to rerun initialization while retaining pinned state and suspended tasks, unless
authored init code changes them.

Initialization reads the current external-buffer bindings. Pass null for `output` if you do not
collect initialization prints. Delegate publication is not allowed during initialization.
Initialization is not transactional: a failure may leave state partially changed.

All entry points return an execution status:

| Status | Meaning and host action |
| --- | --- |
| `0` (`OK`) | The call completed successfully. |
| `1` (`RUNTIME_SAFETY_FAILURE`) | Stop processing, discard partial audio, and recover with full initialization or snapshot restoration. |
| `2` (`INPUT_REJECTED`, events only) | Correct the payload or workspace capacity and retry; processor state remains usable. |

### Call-scoped execution output

For language semantics, see [delegates](https://onda-lang.org/docs/language/#delegates-and-when) and
[printing](https://onda-lang.org/docs/language/#printing). The
[host output guide](https://github.com/onda-lang/onda/blob/main/docs/host-output.md) covers sizing, ownership, decoding,
and presentation across host APIs.

`output` is null when the host consumes neither occurrence stream. Otherwise it points to two
independently nullable pointers followed by the generated call-local counter:

```text
delegate_batch: Ptr
print_batch: Ptr
next_sequence: u32
```

Each present pointer addresses an independent caller-owned batch with this physical shape; native
targets use native pointers and complete wasm32 modules use little-endian 32-bit linear-memory
offsets:

```text
storage: Ptr
capacity_bytes: u32
used_bytes: u32
record_count: u32
overflow_count: u32
```

Every supplied batch descriptor and non-null storage region is mutually disjoint and does not
overlap any other region accessed through the entry-point call. This includes parameter and state
storage, audio pointer tables and samples, external-buffer descriptor tables and samples, packed
event input and workspace, native event tensor views, and the execution-output descriptor itself.

The fixed header of every contiguous record is three `u32` values followed immediately by payload
bytes. A delegate record stores declaration-order delegate index, payload byte count, and call-local
sequence. Delegate payloads use the recursive schema described below, in little-endian wire order.
Record headers retain the artifact target's byte order.

A print record stores log-site index, payload byte count, and the same call-local sequence. Its
payload contains only the site's
primitive scalar arguments without padding: four bytes for `f32`/`i32`, eight for `f64`/`i64`, and
one zero-or-one byte for `bool`. `metadata.source_files` and `metadata.log_sites` supply labels,
source spans, lexical ownership, argument types, and fixed payload sizes. Scalar and header byte
order is the artifact target's byte order.

For one fixed-shape occurrence, exact storage is the twelve-byte record header plus the descriptor's
`payload_size_bytes`. A dynamic delegate has no exact pre-execution size; its descriptor reports
`payload_min_size_bytes`, including each slice's four-byte length prefix but no slice elements.
There is no exact whole-batch size because occurrence counts, delegate selection, and slice lengths
may depend on runtime control flow. Capacity is a host policy, and `overflow_count` reports when it
was insufficient.

Before every init or process entry, the host resets the counters of each supplied batch and resets
`next_sequence` to zero. An input-event entry performs that reset itself before preflight, so
rejected input returns empty batches. Publications into either present batch consume the
shared counter. A complete record is appended only when it fits. Otherwise it is discarded whole
and that batch's overflow counter saturates at `u32::MAX`; a later smaller record may still fit. Null output,
batch, or storage is neutral and does not count overflow. Generated execution failure clears
delegate results but retains print records and overflow already produced, because they may diagnose
the failure. Storage
is caller-owned, never allocated or retained by the processor, and is not part of snapshots. Hosts
that expose a combined log merge the two decoded batches by `sequence`; sequences have no meaning
across separate entry calls.

## Pointer and target profiles

For a native LLVM object, `Ptr` is a real pointer with the width, byte order, data layout, and C ABI
selected by LLVM from the resolved target triple and target options. The application owns storage,
linking, symbol visibility, and any platform runtime dependencies.

For an LLVM WebAssembly relocatable object, `Ptr` is the target's linear-memory offset type. A
wasm32 object therefore lowers `Ptr` to a 32-bit offset. The application linker owns final memory,
symbol export, and module policy.

For a complete wasm32 module emitted by Binaryen, `Ptr` is an unsigned i32 byte offset into the
module's exported linear memory. The module exports `memory` and an immutable i32 `__heap_base`;
the host allocates at or above that address and grows memory before creating views. Memory growth
invalidates JavaScript typed arrays and `DataView` instances.

The target triple is a code-generation choice, not a different processor ABI. It selects such facts
as instruction set, pointer size, C calling convention details, object representation, relocation
model, and physical alignment. The descriptor records the resolved facts that an embedding host
needs but does not replace a target SDK, sysroot, linker, or platform ABI documentation.

## Relocatable-object integration

Onda emits relocatable objects; the application links them into an executable or module.
`integration.required_symbols` lists processor symbols the final link must retain.
`integration.link_libraries`, when present, lists required native libraries. Vector-math helpers
are included in the object; no separate SLEEF library is required. A destination toolchain and
platform math library may still be needed, such as `-lm` on Unix.

For `native_relocatable_object`, link the object with your application normally.
For `webassembly_relocatable_object`, create a module with no `_start`, retain the processor
exports, and export its linear memory. The profile declares `no_entry: true` and
`export_memory: true`; exact linker flags depend on your toolchain. A relocatable Wasm object
cannot be passed directly to `WebAssembly.instantiate`.

### Direct native use

`include/onda_processor_abi.h` is the canonical C declaration of the current ABI entry points. An
application links the emitted object, allocates storage from the exact paired descriptor, builds the
input/output and external-buffer pointer tables, optionally prepares an
`onda_processor_execution_output_t` containing independently allocated delegate and print batches,
resets it immediately before entry, and calls `onda_processor_init`, `onda_process`, and any
`onda_event_N` functions directly. Native descriptors may additionally advertise
`onda_event_views_N`; `onda_processor_event_tensor_view_t` and
`onda_processor_event_views_fn` declare that trusted entry. No Onda runtime or compiler library is
required.

The application must reject descriptor/ABI versions it does not implement and must verify that the
descriptor's target, pointer width, byte order, and calling convention match the linked process. It
also owns its audio-thread floating-point environment, scheduling, validation, snapshots, and final
linker policy.

## Complete core-WebAssembly integration

An artifact with kind `webassembly_module` is ready for `WebAssembly.instantiate`. Its integration
profile names its `memory` and `__heap_base` exports and declares any imports. Current complete
modules are self-contained and can be instantiated with an empty import object (`{}`).

The host still allocates processor storage and calls the entry points below. The
[Web API](https://onda-lang.org/docs/web-api/) provides artifact validation and playback helpers.

## Storage and initialization

The host allocates non-overlapping parameter and physical-state regions using the sizes and minimum
alignments in `runtime`. It initializes parameter defaults from program metadata and calls
`onda_processor_init(params, state, FULL, buffers, buffer_frames, buffer_channels,
buffer_sample_rates, output)` before processing. The four buffer tables describe the bindings
current for this initialization call and follow the same rules as process and event calls. Pass
null for `output` when initialization output is not consumed. Physical state uses the backend's
selected target layout and is otherwise opaque.

Honor the descriptor's reported alignment, including when cloning state. Do not infer physical
storage size or padding from the declared Onda types.

Hosts may replace buffer descriptors between entry-point calls. The next init, event, or process
call observes the replacement; rebinding alone does not execute initialization or recompute state
previously derived from a buffer. Supplying bindings to init is intended for one-time preprocessing
when the host already owns the source buffers before processing begins.

State-backed control outputs and persistent snapshot entries expose their physical offsets in the
artifact descriptor. Scratch state is deliberately absent from snapshots.

Snapshot every entry in `metadata.states`, including entries with `authored: false`, so suspended
tasks can resume correctly. Use `authored` only when deciding which state to display to users.

## Portable snapshots

The packed persistent-state snapshot is target-independent. The current snapshot format encodes
persistent scalar elements in little-endian byte order, in metadata order, without physical padding
or scratch state. It includes pinned authored roots and compiler-owned task frames. This is
distinct from the target-native physical state image, which can use another byte order or alignment.

Restore begins with `onda_processor_init(params, state, FULL, buffers, buffer_frames,
buffer_channels, buffer_sample_rates, NULL)`, using the current bindings, then overlays every
persistent entry from the packed snapshot. Restore initialization output is intentionally
suppressed. This resets instance scratch while preserving persistent state and task continuations.
A host converting between a big-endian physical target and the portable snapshot must encode
and decode each scalar according to metadata rather than copying physical bytes wholesale.

For state with `integer_range` metadata, normalize every restored element into the inclusive
`min..=max` interval using its declared `clamp` or Euclidean `wrap` mode. This also applies to every
element of a fixed array.

## Audio ports

`inputs` and `outputs` point to tables of `Ptr`, one entry per flattened audio channel in metadata
order. Each channel points to a full compile-block scalar array. A null table is valid when the
corresponding flattened channel count is zero.

Audio scalar width belongs to the Onda interface. A platform adapter may convert its native audio
format—for example, Web Audio f32 planar channels—to the declared scalar width. Such conversion is
adapter behavior, not processor code generation.

### Absent surfaces and null pointers

The paired descriptor is the authority for whether a pointer argument has storage behind it. A raw
host passes null exactly when the corresponding surface is absent:

- `params` when `runtime.param_size_bytes` is zero;
- `state` when `runtime.state_size_bytes` is zero;
- `inputs` or `outputs` when the corresponding flattened metadata slot count is zero;
- all four external-buffer table pointers when `metadata.buffers` is empty;
- `EventInput.payload` when its byte count is zero, and workspace when its required size is zero.
  The `EventInput` descriptor itself is always present.

A declared surface is not absent merely because the application does not use it. Every declared
input/output slot requires valid compile-block storage. A non-empty buffer declaration list
requires all four parallel tables. Each descriptor has positive frame/channel counts and a finite
positive sample rate. A null sample pointer denotes an unbound buffer; generated code redirects
reads to zero and discards writes. Give unbound buffers one frame, retain the declared channel
count for exact-channel buffers, and use one channel for dynamic-channel buffers.

## Segmented processing

An artifact is specialized for `compile.block_size`. A host may divide one logical compile block
into calls using `start_frame` and `frames`; the range must remain inside the compile block.

- `flags & 1` is `BEGIN_BLOCK` and gates the program's beginning-of-block work.
- `flags & 2` is `END_BLOCK` and gates its end-of-block work.

The flags do not maintain a hidden cursor. Zero-frame calls are valid. A host whose callback size
differs from the compile block owns a cursor and splits callbacks at logical block boundaries. Audio
pointer tables continue to address complete compile-block storage; generated code derives logical
audio indices from `start_frame`.

## Parameters, events, delegates, buffers, and control outputs

Parameter storage follows the paired descriptor's offsets and scalar shapes. Every event and
delegate also carries a recursive `schema`: named parameters with scalar, primitive tuple, nominal
struct, fixed array, or top-level slice types. Fields include constant defaults and integer domains.
Use the schema to encode and decode host values.

Wire payloads are packed little-endian in parameter and depth-first field order. Every primitive
leaf is one contiguous tensor with outer array axes before inner field-array axes. A runtime slice
contributes one signed `i32` logical length, then its field tensors; there is no padding or separate
length per struct field. Prepared workspace inserts scalar alignment padding and uses native byte
order. Its required size is computed by the shared payload planner, independently of wire size.

Use Rust's `onda_processor_abi::payload::PayloadPlan` or JavaScript's `PayloadPlan` to encode logical
values, calculate workspace sizes, and inspect tensor shapes. Prepare payloads and reserve storage
before realtime execution. See the [C event API](https://onda-lang.org/docs/api/#events) or
[Web API](https://onda-lang.org/docs/web-api/) for managed dispatch.

Every parameter has `range_min_repr`, `range_max_repr`, and
`param_control`. `param_control` is null for a parameter without a numeric host-control domain;
otherwise it contains:

- `scale`: `linear` or `log`;
- `curve`: an optional finite SuperCollider-style `lincurve` value, mutually
  exclusive with `scale = log`;
- `unit`: optional display text;
- `step_repr`: the optional plain-domain step encoded in the declared scalar representation;
- `step_count`: the number of equal intervals between the inclusive endpoints.

Optional `enumeration` metadata describes scalar and fixed-array parameters/control outputs and
scalar leaves of event/delegate schemas. It contains a qualified `name` and
ordered `members`, each with a source `name` and display `label`. An enum's member index is its
ordinary signed `i32` representation. Its range is `[0, members.length - 1]`; enum parameters use
linear scale and unit step. A one-member enum has equal zero bounds and `step_count: 0`.

Enum values use `i32` storage. Packed event preparation normalizes them through `integer_range`;
normalize them when restoring snapshots too.

The raw processor object does not export parameter conversion functions. Native hosts decode each
numeric control into the `onda_processor_param_domain` structure from
`include/onda_processor_abi.h`, whose header-only functions implement clamping, snapping, and
plain/normalized conversion without linking the Onda runtime. The structure carries the declared
scalar type so stepped floating-point grids are validated at their actual storage precision. The
reference generator in
`examples/native/raw_processor_object` emits decoded tables, indexed wrappers, typed reads, and
typed writes around that shared header implementation.

<details markdown="1">
<summary>Parameter conversion rules</summary>

For a scalar numeric parameter, normalized-to-plain conversion is:

1. Map NaN to zero and clamp the normalized input to `[0, 1]`.
2. Return the exact range endpoint for normalized zero or one.
3. If `curve` is present, transform `n` with the SuperCollider-style `lincurve`
   mapping, then apply `min + n * (max - min)`. Otherwise apply that linear
   mapping directly for `linear`, or the overflow-safe equivalent
   `exp(log(min) + n * (log(max) - log(min)))` for `log`.
4. Map a plain NaN to `min`, then clamp the plain value to the inclusive range.
5. For a stepped domain, snap to `min + round((plain - min) / step) * step` and clamp again.
6. Convert to the declared scalar width when writing parameter storage.

Plain-to-normalized first performs the same plain clamping and step snapping, preserves exact
endpoints, then applies the inverse curved, linear, or logarithmic mapping. Boolean plain and
normalized host-control values use the threshold `value >= 0.5` and store one byte containing zero
or one. Fixed parameter arrays retain their logical name, type, length, and per-element defaults.
Their numeric range and control metadata apply independently to each element, addressed in storage
at `byte_offset + index * element_size_bytes`. Unranged numeric parameters have no normalized
host-control domain.

Because the shared host-control surface uses binary64 values, an `i64` control domain and its
range width are restricted to the exactly representable integer interval
`[-9007199254740991, 9007199254740991]`. This restriction does not apply to unranged `i64`
parameters written through their typed/raw storage representation.

</details>

External buffers use four parallel tables in declaration order:

- `buffers`: writable sample pointers, with null entries denoting unbound buffers.
- `buffer_frames`: i32 frame counts.
- `buffer_channels`: i32 channel counts.
- `buffer_sample_rates`: f32 sample rates.

All four descriptor tables remain immutable for an entry-point call and do not overlap parameter,
state, audio, or external-buffer sample storage. The non-overlap rule is between descriptor-table
storage and the regions reached through non-null host storage pointers.

Fixed resource arrays occupy contiguous physical slots. `metadata.buffer_arrays` records each
logical group name, its first physical slot, and its length, so hosts can bind a whole bank without
parsing generated slot names. Each slot has its own `metadata.buffers[first + slot].may_write`
value. Supply writable sample memory whenever that value is true.

Samples use interleaved frame-major storage. Metadata declares scalar width, read/write access, and
mono, static, or dynamic channel constraints. Every sample-rate entry is finite and positive,
and every non-null pointer, frame count, and channel count denotes nonempty prepared storage. Null
buffer entries use processor-owned zero and discard storage.
Control outputs are state-backed values at declared physical offsets and may be observed between
processor calls.

## Numerical and failure behavior

Strict compilation preserves declared scalar widths, NaNs, signed zero, rounding behavior, and
one-rounding FMA semantics. Fast math is an explicit compilation policy recorded by the artifact.
Native floating-point control registers belong to the calling thread. Onda's realtime hosts enable
x86 FTZ/DAZ before entering init, process, or event code to prevent subnormal feedback-state stalls;
a raw-object host that wants the same audio policy must configure its calling threads likewise.
Bounds checks, integer division, and other generated safety checks return
`RUNTIME_SAFETY_FAILURE` instead of trapping. Invalid host pointers, storage extents, or other
violations of the raw ABI remain outside generated-code recovery and can still trap or cause
undefined behavior. `RUNTIME_SAFETY_FAILURE` means execution may have partially changed state or
output; a host must stop using that processor state until full initialization or snapshot restoration
establishes valid state again, and audio hosts discard partial output. `INPUT_REJECTED` completes no
handler work, preserves processor state, returns empty output batches, and leaves the processor
usable after the host corrects its payload or workspace capacity.

## Web Audio reference adapter

Use [`@onda-lang/webaudio`](https://onda-lang.org/docs/web-api/#onda-langwebaudio) to connect a
complete WebAssembly artifact to an `AudioContext`. It handles audio scheduling, parameter and
event messages, buffer bindings, and snapshots. Configure dynamic event capacity before playback;
oversized events are rejected rather than growing memory during rendering.

## C header reference

The release SDK installs `include/onda_processor_abi.h` and this document together. The header is
self-contained and header-only except for the processor-specific `onda_processor_init`,
`onda_process`, generated `onda_event_N`, and optional native `onda_event_views_N` symbols supplied
by the compiled object. Include it from C or C++; no `libonda` linkage is required to call a
processor object.

The public declarations fall into four groups:

- ABI versions, execution results, initialization modes, and segmented-processing flags.
- Function-pointer signatures and the generated init, process, packed-event, and native event-view
  entry points.
- Caller-owned delegate, print, execution-output, occurrence, and cursor records.
- Inline batch iteration and parameter-domain validation/conversion helpers.

The inline batch iterators validate record boundaries before returning a payload view. Each next
call returns `1` for a record, `0` only at the exact end of a valid batch, and `-1` for invalid or
malformed input. Sequential iteration with `onda_processor_delegate_batch_next` or
`onda_processor_print_batch_next` is linear in record count and constant-space. The random-access
convenience functions validate the complete batch and return `0` only when the requested index is
absent; repeated indexed iteration is therefore quadratic, so use a cursor when consuming a whole
batch.

All parameter conversion helpers are allocation-free and constant-time. Prepare and validate a
domain once when constructing host controls rather than validating descriptor text on the audio
thread.

<details markdown="1">
<summary>All processor C functions</summary>

<!-- BEGIN PROCESSOR C API FUNCTION INDEX -->

```text
onda_process
onda_processor_batch_next_record
onda_processor_delegate_batch_next
onda_processor_delegate_batch_reset
onda_processor_execution_output_reset
onda_processor_float_grid_value_matches
onda_processor_init
onda_processor_integer_domain_value_is_valid
onda_processor_lincurve_normalized_to_unit
onda_processor_lincurve_unit_to_normalized
onda_processor_linear_plain_to_unit
onda_processor_linear_unit_to_plain
onda_processor_param_constrain_plain
onda_processor_param_domain_is_valid
onda_processor_param_normalized_to_plain
onda_processor_param_plain_to_normalized
onda_processor_print_batch_next
onda_processor_print_batch_reset
```

<!-- END PROCESSOR C API FUNCTION INDEX -->

</details>
