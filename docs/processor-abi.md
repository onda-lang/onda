# Onda processor ABI

This document specifies the backend-neutral contract between a compiled Onda processor and its
host. The contract is not WebAssembly-specific: LLVM native objects, LLVM WebAssembly objects, and
complete modules emitted by the Binaryen backend implement the same logical processor interface.
Target and artifact profiles define how that interface is represented on a particular platform.

## Versioned artifact descriptor

Every processor artifact is paired with a JSON descriptor whose common envelope contains:

- `format: "onda-processor"` and `format_version` for the descriptor schema.
- `abi_version` for the logical entry-point and storage contract in this document.
- `artifact_kind`, currently `relocatable_object` or `webassembly_module`.
- `backend` and `mir_schema_version` for provenance.
- `target`, including the resolved triple, pointer width, byte order, pointer model, and calling
  convention.
- `integration`, including symbols that must survive integration and an artifact-specific profile.
- `compile`, `exports`, `runtime`, and `metadata` for specialization, symbol names, allocation, and
  the program interface.

Hosts must reject unknown descriptor or ABI versions. A descriptor belongs to the exact bytes with
which it was emitted; physical layouts must not be borrowed from another backend or compilation.
The Rust `onda_processor_abi` types and the TypeScript `@onda-lang/processor-abi` declarations are
the same schema. A shared conformance fixture exercises the complete logical metadata record and
the directly loadable core-WebAssembly profile; profile-specific facts use one tagged integration
record rather than backend-specific descriptor shapes.

## Logical processor interface

The signatures below use `Ptr`, an abstract pointer to host-owned storage. `i32` arguments are
signed 32-bit values. Public LLVM entry points use the target's C calling convention; complete core
WebAssembly modules use ordinary core-Wasm function calls.

```text
onda_processor_init(params: Ptr, state: Ptr, all: i32) -> i32

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
) -> i32

onda_event_N(
  payload: Ptr,
  params: Ptr,
  state: Ptr,
  buffers: Ptr,
  buffer_frames: Ptr,
  buffer_channels: Ptr,
  buffer_sample_rates: Ptr,
) -> i32
```

There is one `onda_event_N` for each declared event, in metadata order. The current ABI permits one
public processor namespace per artifact. A future ABI may add artifact-specific namespacing for
multi-processor libraries without changing MIR.

For `onda_processor_init`, a nonzero `all` clears the complete physical state image before executing
init; zero executes against the supplied image and therefore preserves pinned state unless init
explicitly changes it. Raw ABI initialization is not transactional: a host that needs rollback must
run it against a staging image and publish that image only after a zero status.

Processor ABI version 5 renamed this raw entry point from `onda_init` and added the `all` argument.
This keeps it distinct from the instance-level C API, whose `onda_init(instance)` operation is
transactional and captures a new reset baseline after successful initialization.

Every entry point returns zero on success or a positive execution-failure code. Code `1` is
`RUNTIME_SAFETY_FAILURE`, produced when generated code encounters a checked condition from which it
cannot continue safely. The host must not publish a staging state after any nonzero result; it may
discard that image or restore a known-good image before calling `onda_processor_init` again.

The process order intentionally places state, parameters, and audio tables before segment controls
and optional buffer tables. This keeps the hottest pointers in argument registers on common native
C ABIs without introducing a target-specific entry point.

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

The native compiler deliberately stops at a relocatable object for every LLVM AOT target. Onda does
not bundle or invoke a linker. The descriptor's `integration.required_symbols` lists processor
symbols the final application must retain at its chosen integration boundary.

The `native_relocatable_object` profile requires no Onda-specific entry point. The user's normal
platform linker combines the object with the host and any required runtime libraries.

The `webassembly_relocatable_object` profile additionally declares `no_entry: true` and
`export_memory: true`. A typical final link therefore has no conventional `_start`, retains the
processor symbols as exports, and exposes the selected linear memory. Exact linker flags belong to
the user's toolchain rather than the Onda compiler.

The native compiler validates that LLVM WebAssembly output is a version-1 Wasm binary with a
relocatable `linking` section. It does not pretend that the object is directly instantiable.

### Direct native use

`include/onda_processor_abi.h` is the canonical C declaration of the current ABI entry points. An
application links the emitted object, allocates storage from the exact paired descriptor, builds the
input/output and external-buffer pointer tables, and calls `onda_processor_init`, `onda_process`, and any
`onda_event_N` functions directly. No Onda runtime or compiler library is required.

The application must reject descriptor/ABI versions it does not implement and must verify that the
descriptor's target, pointer width, byte order, and calling convention match the linked process. It
also owns its audio-thread floating-point environment, scheduling, validation, snapshots, and final
linker policy. These responsibilities are ordinary consumers of the raw ABI, not generated object
entrypoints.

## Complete core-WebAssembly integration

The browser-safe Binaryen backend emits a complete, directly instantiable core-WebAssembly module
because browser WebAssembly APIs do not expose relocatable objects or a linker. Its integration
profile names the `memory` and `__heap_base` exports and declares the module imports. Current
Binaryen artifacts are self-contained and have no imports, including for transcendental math and
strict fused multiply-add helpers.

This complete-module profile is a packaging choice. It implements the same logical processor
contract as an LLVM object and does not make Web Audio part of the ABI.

## Storage and initialization

The host allocates non-overlapping parameter and physical-state regions using the sizes and minimum
alignments in `runtime`. It initializes parameter defaults from program metadata and calls
`onda_processor_init(params, state, 1)` before processing. Physical state uses the backend's selected
target layout and is otherwise opaque.

State-backed control outputs and persistent snapshot entries expose their physical offsets in the
artifact descriptor. Scratch state is deliberately absent from snapshots.
`runtime.state_reset_ranges` lists the coalesced physical ranges copied from the post-init baseline
by ordinary reset; pinned authored roots and compiler-owned task continuations are outside those
ranges. An all-state reset restores the complete baseline instead.

`metadata.states` includes every packed snapshot entry. Its `authored` flag preserves the explicit
MIR state provenance and is false for compiler-owned task frames, allowing snapshot implementations
to preserve suspended tasks while authored-state reflection omits their implementation storage.

## Portable snapshots

The packed persistent-state snapshot is target-independent. The current snapshot format encodes
persistent scalar elements in little-endian byte order, in metadata order, without physical padding
or scratch state. It includes pinned authored roots and compiler-owned task frames. This is
distinct from the target-native physical state image, which can use another byte order or alignment.

Restore begins with `onda_processor_init(params, state, 1)`, then overlays every persistent entry
from the packed snapshot. This resets instance scratch while preserving persistent state and task
continuations.
A host converting between a big-endian physical target and the portable snapshot must encode
and decode each scalar according to metadata rather than copying physical bytes wholesale.

Processor ABI version 4 adds optional `integer_range` metadata to scalar `i32` and `i64` state.
After overlaying each such snapshot entry, the host must normalize it into the inclusive
`min..=max` interval using the declared `clamp` or Euclidean `wrap` mode. This restores the storage
invariant before generated code can rely on it to remove bounds checks. The packed snapshot byte
representation is unchanged, so its format version remains 1.

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
- an event's `payload` when that event's `payload_size_bytes` is zero.

A declared surface is not absent merely because the application does not use it. Every declared
input/output slot requires valid compile-block storage. A non-empty buffer declaration list
requires all four parallel tables. Each descriptor has positive frame/channel counts and a finite
positive sample rate. A null sample pointer denotes an unbound buffer; generated code redirects
reads to processor-private zero storage and writes to distinct discard storage. Hosts give unbound buffers
one frame, retain the declared channel count for exact-channel buffers, and use one channel for
dynamic-channel buffers. This keeps omitted resource bindings neutral without exposing the
implementation's separate read/write pointers to hosts.

## Segmented processing

An artifact is specialized for `compile.block_size`. A host may divide one logical compile block
into calls using `start_frame` and `frames`; the range must remain inside the compile block.

- `flags & 1` is `BEGIN_BLOCK` and gates the program's beginning-of-block work.
- `flags & 2` is `END_BLOCK` and gates its end-of-block work.

The flags do not maintain a hidden cursor. Zero-frame calls are valid. A host whose callback size
differs from the compile block owns a cursor and splits callbacks at logical block boundaries. Audio
pointer tables continue to address complete compile-block storage; generated code derives logical
audio indices from `start_frame`.

## Parameters, events, buffers, and control outputs

Parameter and event-payload storage follows the exact offsets and scalar shapes in the paired
descriptor. A dynamic event slice contains its scalar data after the fixed payload header and stores
the generated offset and length in that header. Event handlers receive the same parameter, state,
and buffer bindings as processing.

Descriptor format version 2 gives every parameter `range_min_repr`, `range_max_repr`, and
`param_control`. `param_control` is null for a parameter without a numeric host-control domain;
otherwise it contains:

- `scale`: `linear` or `log`;
- `curve`: an optional finite SuperCollider-style `lincurve` value, mutually
  exclusive with `scale = log`;
- `unit`: optional display text;
- `step_repr`: the optional plain-domain step encoded in the declared scalar representation;
- `step_count`: the number of equal intervals between the inclusive endpoints.

The raw processor object does not export parameter conversion functions. Native hosts decode each
numeric control into the `onda_processor_param_domain` structure from
`include/onda_processor_abi.h`, whose header-only functions implement clamping, snapping, and
plain/normalized conversion without linking the Onda runtime. The structure carries the declared
scalar type so stepped floating-point grids are validated at their actual storage precision. The
reference generator in
`examples/native/raw_processor_object` emits decoded tables, indexed wrappers, typed reads, and
typed writes around that shared header implementation.

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
or one. Parameter arrays and un-ranged numeric parameters do not have normalized host-control
domains.

Because the shared host-control surface uses binary64 values, an `i64` control domain and its
range width are restricted to the exactly representable integer interval
`[-9007199254740991, 9007199254740991]`. This restriction does not apply to unranged `i64`
parameters written through their typed/raw storage representation.

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
parsing generated slot names. Selection clamps once and computes `first + selector` in constant
time. Each physical slot has its own `metadata.buffers[first + slot].may_write` value. A false value
proves that reachable processor code does not write that slot; selectors that cannot be resolved
statically conservatively mark every slot they may select.

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
undefined behavior. A host must treat every nonzero execution result as a failed processor state
instead of continuing with potentially partial state or output writes.

## Web Audio reference adapter

`packages/onda_webaudio` is an optional reference adapter for complete WebAssembly modules. It owns
Web Audio node construction, render-quantum scheduling, f32 marshaling, parameter/event messages,
buffer bindings, and snapshot requests. None of those policies are required when embedding an Onda
processor in a native application, an offline renderer, a plugin API, a worker, or another Wasm
runtime. The adapter compiles the `WebAssembly.Module` outside the audio rendering thread, caches
typed views over processor memory, uses a bulk-copy fast path for full-block f32 audio, and locks
host linear-memory allocation after construction. Dynamic event payload storage is preallocated to a
configurable capacity; an oversized event fails rather than growing linear memory while audio is
running.
