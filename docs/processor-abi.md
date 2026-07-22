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
onda_init(params: Ptr, state: Ptr) -> void

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
) -> void

onda_event_N(
  payload: Ptr,
  params: Ptr,
  state: Ptr,
  buffers: Ptr,
  buffer_frames: Ptr,
  buffer_channels: Ptr,
  buffer_sample_rates: Ptr,
) -> void
```

There is one `onda_event_N` for each declared event, in metadata order. The current ABI uses
unprefixed symbol names and therefore permits one public processor namespace per artifact. A future
ABI may add namespacing for multi-processor libraries without changing MIR.

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
input/output and external-buffer pointer tables, and calls `onda_init`, `onda_process`, and any
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
alignments in `runtime`. It initializes parameter defaults from program metadata, zeroes physical
state, and calls `onda_init` before processing. Physical state uses the backend's selected target
layout and is otherwise opaque.

State-backed control outputs and persistent snapshot entries expose their physical offsets in the
artifact descriptor. Scratch state is deliberately absent from snapshots.

## Portable snapshots

The packed persistent-state snapshot is target-independent. The current snapshot format encodes
declared scalar elements in little-endian byte order, in metadata order, without physical padding or
scratch state. This is distinct from the target-native physical state image, which can use another
byte order or alignment.

Restore begins from a freshly zeroed state followed by `onda_init`, then overlays every persistent
entry from the packed snapshot. This resets instance scratch while preserving declared persistent
state. A host converting between a big-endian physical target and the portable snapshot must encode
and decode each scalar according to metadata rather than copying physical bytes wholesale.

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
requires all four parallel tables; an empty individual binding is represented by a null data entry
and zero frame/channel entries, not by null tables. This rule lets generated code omit redundant
surface-count branches while keeping zero-surface processors minimal.

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

External buffers use four parallel tables in declaration order:

- `buffers`: data pointers.
- `buffer_frames`: i32 frame counts.
- `buffer_channels`: i32 channel counts.
- `buffer_sample_rates`: f32 sample rates.

Samples use interleaved frame-major storage. Metadata declares scalar width, read/write access, and
mono, static, or dynamic channel constraints. Every sample-rate entry is finite and positive,
including for a canonically empty binding. Control outputs are state-backed values at declared
physical offsets and may be observed between processor calls.

## Numerical and failure behavior

Strict compilation preserves declared scalar widths, NaNs, signed zero, rounding behavior, and
one-rounding FMA semantics. Fast math is an explicit compilation policy recorded by the artifact.
Native floating-point control registers belong to the calling thread. Onda's realtime hosts enable
x86 FTZ/DAZ before entering init, process, or event code to prevent subnormal feedback-state stalls;
a raw-object host that wants the same audio policy must configure its calling threads likewise.
Bounds checks, integer division, invalid conversions, or an invalid host contract may trap. A host
must treat a trap as a failed processor instance instead of continuing with possibly corrupted
state.

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
