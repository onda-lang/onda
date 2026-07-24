# Onda dynamic VST3 plugin specification

- Status: accepted architecture; implementation starts with the Onda SDK prerequisites below
- Implementation repository: separate `onda-plugin` repository
- Implementation language: C++20
- Plugin framework: JUCE
- Initial plugin format: VST3
- Generated plugins: deferred to [`todo/generated-plugins.md`](todo/generated-plugins.md)

## Purpose

The `onda-plugin` project provides fixed VST3 shells that load, compile, run, edit, and persist
Onda source projects inside a DAW. The first release contains two separate plugin products:

| Product | Main input | Main output | Host slots | Note input |
| --- | ---: | ---: | ---: | --- |
| Onda Dynamic Instrument | 0 channels | 2 channels | 32 | Basic MIDI |
| Onda Dynamic Effect | 2 channels | 2 channels | 32 | Basic MIDI |

The plugin products use JUCE for VST3 integration, host parameters, state callbacks, windows, and
the editor. They use Onda's native SDK for compilation, metadata, runtime instances, buffer
binding, event dispatch, segmented processing, and portable state snapshots.

The plugin project must remain a host adapter. It must not reimplement Onda's runtime ownership,
generated-code safety, metadata, or processing contracts in C++.

## Goals

- Deliver reliable dynamic Onda instrument and effect VST3 plugins.
- Let users edit or load complete Onda source projects without rebuilding the plugin.
- Keep the products' host-visible audio layouts, parameter IDs, and MIDI capability fixed.
- Compile and prepare replacement programs away from the audio thread.
- Retain the last known-good program after a failed reload.
- Derive sample rate and maximum block size exclusively from the host.
- Deliver supported MIDI events at their host-provided sample offsets.
- Preserve source, resources, slot mappings, parameters, and compatible Onda state in DAW projects.
- Share compiled code safely between plugin instances while keeping mutable instances exclusive.
- Keep compilation, allocation, decoding, file access, logging, and destruction off the audio
  thread.
- Exercise both JUCE's VST3 integration and Onda's native SDK through repeatable validator and DAW
  tests.

## Non-goals for the first release

- Generated plugins or an `onda plugin bundle` command.
- CLAP, Audio Unit, AAX, LV2, or standalone exports.
- Runtime changes to a product's host-visible audio layout, parameter count, or parameter IDs.
- Sidechain or auxiliary buses.
- Audio or MIDI output events from Onda to the host.
- MIDI SysEx, MIDI 2.0, and complete VST3 note-expression support.
- Transport, tempo, time-signature, playhead, or offline-render context in the Onda language.
- Sample-accurate host parameter automation. Host parameters become visible to Onda at the next
  logical Onda block boundary.
- Non-zero latency reporting.
- Persisting mutations made by Onda to read-write external buffers.
- Automatic voice allocation, voice stealing, or one Onda runtime instance per note.
- Loading executable code or an ORC handle from plugin state.
- Compatibility with an earlier plugin-state schema. The schema starts at version 1.

## Repository boundary

### The Onda repository owns

- The frontend, semantic analysis, MIR, LLVM code generator, and runtime.
- The processor descriptor and all physical layout metadata.
- Thread-safe ownership of immutable compiled programs.
- Exclusive ownership and movement of mutable runtime instances.
- Binding validation and prepared unchecked processing.
- Portable persistent-state snapshots.
- The plugin-safe generated-code failure profile.
- The C ABI used by the C++ plugin adapter.
- Tests that prove native program sharing and instance movement are safe.

The existing native SDK is not a fixed boundary that the plugin must work around. Plugin
implementation is expected to make the required changes in Onda itself, including replacing
`Rc`-based compiled-program ownership with an `Arc`-backed thread-safe handle and extending the C
ABI where the current surface is insufficient. These changes should improve the shared embedding
SDK for all native hosts rather than exist as plugin-only behavior.

The relevant API is implemented by `crates/onda_api` and declared in `include/onda.h`.

The current SDK already provides:

- source-string and filesystem-entry compilation;
- immutable metadata queries, including parameter range, scale, unit, step, and normalized/plain
  conversion;
- runtime instances that retain their own clone of `JitProgram`;
- allocator-aware instance storage;
- input, output, and external-buffer binding validation;
- `onda_prepare_unchecked_process()` and segmented unchecked processing;
- portable persistent-state snapshots and control-output reads.

The plugin must use these existing contracts rather than introduce parallel C++ implementations.
The remaining Onda work is:

- compilation of a complete in-memory source project, not only one source string or filesystem
  entry point;
- compiler, MIR-schema, processor-ABI, and native-SDK version queries;
- a stable public-interface fingerprint;
- owned diagnostic collections with an explicit destruction function; the current C diagnostic
  conversion leaks copied strings and is not suitable for repeated live recompilation;
- bounded runtime fault reporting for the plugin-safe codegen profile.

The existing metadata query surface can remain the canonical native descriptor API if it exposes
every field needed by the plugin and is covered by descriptor-equivalence tests. A second
plugin-specific descriptor format is not required.

The C ABI does not initially need program-level reference counting. `onda_instance_create()`
already clones the program into the instance. The C++ adapter can own one `onda_program_t` in a
RAII `ProgramHandle` and share that owner through `std::shared_ptr`. C-level retain/release should
be added only if another native embedding use case requires independently retained opaque handles.

Likewise, the first plugin does not require a second C `prepared_instance` object solely for
typestate. The existing bind, validate, prepare, and unchecked-process calls are sufficient if Onda
documents their state transitions and tests rebind/reprepare behavior. A Rust `PreparedInstance`
typestate may still be useful internally, but it is not a prerequisite for the C++ adapter.

Safety invariants belong beside the Rust types that enforce them. The C++ adapter may wrap opaque C
handles in RAII classes, but it must not repair thread-safety with an adapter-local unsafe promise.

### The `onda-plugin` repository owns

- Two JUCE `AudioProcessor` implementations and their immutable product metadata.
- Fixed host audio layouts and the 32 stable automation slots.
- Mapping between JUCE audio/MIDI/parameter callbacks and the Onda native SDK.
- Source editing, file watching, compilation requests, diagnostics, and last-known-good behavior.
- The specialization cache and background work coordinator.
- External asset decoding and ownership.
- Realtime-safe replacement and deferred destruction queues.
- Versioned VST3 state serialization.
- The JUCE editor and its message-oriented engine interface.
- CMake integration, bundle construction, validator execution, signing, and release packaging.

The repository pins exact reviewed revisions of JUCE and Onda. Release builds statically link the
required Onda native SDK into each VST3 bundle so users do not need a separately installed Onda
shared library. Onda and JUCE updates are deliberate dependency changes accompanied by the complete
plugin verification matrix.

The plugin repository should separate these implementation layers:

- `ProgramHandle`: move-only C handle ownership wrapped in shared C++ ownership by the cache;
- `CompilerCoordinator`: project snapshots, specialization keys, request coalescing, and
  last-known-good publication;
- `PreparedEngine`: one exclusive Onda instance plus all pointer-stable audio slabs, external
  assets, slot mappings, event payload storage, and logical-block cursor;
- `RealtimeEngine`: bounded replacement/retirement queues and allocation-free `processBlock()`;
- `PluginState`: the bounded versioned serializer independent of JUCE widget state;
- JUCE processor/editor adapters for the instrument and effect products.

No JUCE object owns a raw Onda handle directly except through these RAII layers.

JUCE licensing remains a property of the separate plugin product and must not change the MIT
licensing of the Onda compiler repository.

## Products and stable host surface

The permanent VST3 class UUIDs are:

| Product | VST3 class UUID |
| --- | --- |
| Onda Dynamic Instrument | `8607b79e-dc1c-4bb7-9890-5136a5aecd8b` |
| Onda Dynamic Effect | `241758b4-116b-4bb3-85cb-329ad3b58bde` |

The instrument and effect should initially be separate VST3 targets and bundles. This keeps their
discovery metadata and lifecycle independent and avoids relying on framework support for several
processor classes in one binary.

Both products always expose exactly 32 Onda automation slots:

```text
IDs:       slot01 ... slot32
Names:     Slot 1 ... Slot 32
Type:      normalized float in [0, 1]
Default:   0.5
Automation: automatable, non-discrete
```

The names and IDs never change. The editor separately shows active mappings such as
`Slot 3 -> cutoff` or `Slot 7 -> resonance`. Source parameter names must not become host
parameter IDs or mutate the scanned parameter list.

The plugin constructors and discovery path must not initialize LLVM, read source files, or compile
Onda code. Static product metadata and parameter definitions are ordinary constants.

## Shared native ownership

Onda's native runtime must establish this logical contract:

```text
JitProgram: Clone + Send + Sync
Instance: Send
Instance: not Sync
```

- A compiled program is an immutable shared handle to generated code and metadata.
- Multiple plugin instances may execute the same compiled program concurrently.
- An instance owns all mutable parameters, persistent state, scratch storage, bindings, and
  scheduler state.
- Processing requires exclusive access to an instance.
- An `onda-plugin` `PreparedEngine` combines one exclusive instance with pointer-stable
  audio/resource storage, complete bindings, and successful unchecked-process preparation.
- A compiled program outlives every instance created from it.

`JitProgram` currently stores metadata in `Arc` containers but stores executable code as
`Rc<orc_backend::MirJitProgram>`. This prevents the compiled program and every `Instance` retaining
it from crossing threads. The Onda implementation must:

1. Change the executable owner to `Arc<orc_backend::MirJitProgram>`.
2. Audit `MirJitProgram` and its `NativeOrcProcess` owner, which contains the LLJIT handle and
   resolved immutable function pointers.
3. Establish that generated functions may execute concurrently when every call receives
   instance-exclusive parameter, state, binding, and scheduler storage.
4. Establish that the LLJIT owner cannot be destroyed while any generated function is executing;
   normal `Arc` ownership supplies the lifetime, while hosts must perform the final drop away from
   realtime callbacks.
5. Add `Send`/`Sync` implementations only at the audited ORC-owning type. Do not mark
   `onda_runtime::Instance` as `Sync`; processing and mutation require exclusive access.
6. Add compile-time trait assertions and stress tests that create many instances from one program,
   process them on separate threads, destroy the original program handle early, and vary the order
   in which instances and shared owners are dropped.

The C ABI must document that `onda_program_t` is immutable after compilation, that an
`onda_instance_t` is exclusive to one control/audio owner at a time, and that destroying either
handle is not realtime-safe. The plugin's deferred-retirement queue, not the Onda SDK, determines
the thread on which the final instance and program owners are released.

Compilation may be serialized behind one process-wide compiler coordinator if LLVM requires it.
That coordinator protects compilation and cache insertion only. It is never acquired from a JUCE
audio callback.

## Compilation and reload

### Specialization cache

Compiled programs are keyed by:

```text
complete source-project content hash
+ sample_rate.to_bits()
+ maximum_block_size
+ compiler/codegen options
+ Onda compiler version
+ target triple and CPU feature policy
+ plugin-safe failure-profile version
```

The cache owns weak program references so unused specializations can be reclaimed away from the
audio thread. Concurrent requests for the same key coalesce into one compile. A monotonic reload
generation prevents a slow, stale result from replacing a newer edit.

A cache entry is usable only when the complete key matches. A host change to sample rate or maximum
block size always requires a specialization for the new pair.

### Source project

Dynamic plugin state contains the complete inline source project for portable restoration. It may
also retain file paths and content hashes for editing and file watching, but those paths are not
the correctness source when a DAW project is reopened.

A reload transaction is:

1. Snapshot the editor's complete source project and increment its reload generation.
2. Analyze and compile for the active host specialization.
3. Validate the dynamic product contract.
4. Decode or restore all required buffers.
5. Create, initialize, bind, and prepare a complete replacement instance.
6. Discard the result if a newer generation exists.
7. Offer the prepared replacement through a bounded realtime-safe handoff.
8. Swap ownership at a process boundary.
9. Move the retired instance to a bounded deferred-destruction queue.

Diagnostics belong to the attempted generation. A failed transaction does not partially alter the
active program, mappings, resources, or DSP state.

The plugin must start in a valid fallback state while initial compilation is pending:

- an instrument produces silence;
- an effect passes corresponding input channels through unchanged.

No compile, cache lookup that may block, or `PreparedEngine` construction occurs in
`processBlock()`.

## Dynamic interface validation

The dynamic instrument accepts:

- zero flattened Onda audio inputs;
- zero, one, or two flattened Onda audio outputs;
- the required instrument events defined below.

The dynamic effect accepts:

- zero, one, or two flattened Onda audio inputs;
- zero, one, or two flattened Onda audio outputs;
- optional supported MIDI events.

Flattened declarations bind in declaration order:

```text
input slot 0  -> host left
input slot 1  -> host right
output slot 0 -> host left
output slot 1 -> host right
```

There is no implicit mono duplication, stereo summing, or name-based routing. An omitted Onda
output produces silence on that host channel. More than two flattened inputs or outputs rejects the
reload.

Host-bindable scalar Onda parameters map in declaration order. More than 32 bindable values rejects
the reload. Reordering declarations deliberately changes the mapping while the host continues to
see the same stable `slotNN` parameters.

## Audio I/O

Each prepared Onda instance owns pointer-stable channel slabs sized for the host's advertised
maximum block size. JUCE callback buffers are copied into and out of those slabs; callback-owned
pointers are never retained.

The adapter implements both JUCE float and double processing paths:

- matching scalar types use bulk copies;
- f32 host input to f64 Onda input widens into a preallocated slab;
- f64 host input to f32 Onda input narrows into a preallocated slab;
- output conversion follows the inverse direction.

All inputs are captured before outputs are cleared so in-place or aliased JUCE layouts cannot
violate Onda's non-overlapping binding contract. Conversion loops are allocation-free and
vectorization-friendly.

For a valid effect, host output channels are cleared before declared Onda outputs are copied back.
If no valid effect exists, the adapter performs explicit same-index dry pass-through and clears an
output without a matching input. Instruments always clear outputs before processing.

All Onda slabs and declared external buffers are bound during preparation. The callback only copies
samples, updates scalar storage at defined boundaries, dispatches events, and invokes prepared
unchecked segment processing.

## Parameters

### Bindable types

The adapter supports scalar Onda `f32`, `f64`, `i32`, `i64`, and `bool` parameters.

- A numeric value is host-bindable only when it has a finite range with `min < max`.
- A bool needs no range.
- An un-ranged numeric parameter remains editor-visible but is not mapped to a host slot.
- Parameter arrays remain editor-visible but are not host-bindable because Onda parameter domains
  are intentionally scalar-only.

### Numeric mapping

The plugin consumes Onda's canonical parameter-control metadata and conversion API rather than
reimplementing it. For normalized host value `n` clamped to `[0, 1]` and declared range
`[lo, hi]`:

```text
linear: lo + n * (hi - lo)
log:    lo * (hi / lo) ** n
bool:   n >= 0.5
```

Declared stepped domains snap the resulting plain value to `lo + k * step`; ranged integers carry
an implicit step of one. Endpoints are exact: `0` maps to `lo`, and `1` maps to `hi`. The stable
JUCE slots remain continuous host parameters because their source mappings can change on reload;
Onda's boundary conversion supplies the discrete plateaus where required. Integer ranges that
cannot be represented precisely through JUCE's float-valued parameter boundary are editor-only.
The adapter uses `onda_set_param_normalized()` (or the equivalent structured SDK operation);
`onda_set_param_by_index()` remains a low-level raw-storage escape hatch and is not an automation
API.

This VST3 path is intentionally different from Onda's egui and webview controls. Those controls
already own a plain-valued UI and may perform logarithmic geometry locally. A VST3 host owns a
normalized automation lane, so the plugin must preserve the normalized host value until Onda
performs the mapped write.

The JUCE editor follows one consistent rule:

- knob and slider gestures edit the fixed slot's normalized JUCE value;
- the displayed value is obtained with `onda_param_normalized_to_plain()`;
- explicit numeric entry is parsed as a plain value, converted with
  `onda_param_plain_to_normalized()`, and applied to the JUCE slot with a host-notified parameter
  gesture;
- `processBlock()` reads the normalized slot atomics and applies them with
  `onda_set_param_normalized()` at the next Onda block boundary.

The editor never writes an Onda instance directly. This keeps editor gestures, typed values, host
automation, state restoration, and DAW undo on the same JUCE parameter path.

The first successfully loaded program seeds newly mapped slots from inverse-mapped Onda defaults
through JUCE's host-notified parameter API outside the audio thread. Later reloads preserve current
slot values. Restored host values win over source defaults. The editor provides an explicit
host-notified reset-to-source-defaults action.

### Update timing

At each logical Onda `BEGIN_BLOCK`, the adapter snapshots the 32 atomic JUCE parameter values,
converts mapped slots, and writes Onda parameter storage before processing the segment. Top-level
parameters therefore retain Onda's existing block-rate semantics.

When host events occur at the same boundary, parameter storage is updated first, events are
dispatched second, and the segment carrying `BEGIN_BLOCK` executes last.

Sample-accurate host automation is deferred. Implementing it requires an explicit Onda
parameter-rate design, not only splitting JUCE callback buffers.

## MIDI and instrument event ABI

An instrument source must declare:

```onda
event note_on(id: i32, channel: i32, key: i32, velocity: f32):
event note_off(id: i32, channel: i32, key: i32, velocity: f32):
```

The names, parameter names, order, and types are exact. A dynamic instrument reload fails and keeps
the previous program if either event is absent or malformed.

The initial JUCE MIDI path passes `id = -1` because ordinary MIDI note messages do not carry a
stable VST3 note identifier through the abstraction used by the first implementation. Programs
identify overlapping voices by their own policy, normally using `(channel, key)`.

The wrapper does not allocate voices or create one Onda instance per note. Source code owns voice
allocation, stealing, envelopes, and termination.

These optional events are dispatched when corresponding MIDI messages are available:

```onda
event pitch_bend(channel: i32, value: f32):
event channel_pressure(channel: i32, pressure: f32):
event cc(channel: i32, index: i32, value: f32):
```

Pitch bend is normalized to `[0, 1]` with `0.5` at center. Channel pressure and CC values are
normalized to `[0, 1]`.

VST3 per-note expression events such as pressure, tuning, pan, and brightness are deferred until
the exact information preserved by JUCE's VST3 adapter has been verified. The plugin must not
synthesize stable note IDs or claim sample-accurate expression support it does not receive.

Incoming MIDI is consumed in nondecreasing sample-offset order:

```text
process audio before offset
dispatch all events at offset in host order
continue processing
```

An event at offset zero is dispatched before audio at that offset. An event on a logical block
boundary is dispatched after the previous `END_BLOCK` and before the next `BEGIN_BLOCK`. Equal
offset ordering is stable.

Arbitrary Onda events are editor controls. They are encoded and validated outside the audio
thread, placed into a bounded queue, and dispatched at the next process boundary.

## Logical block scheduling

The host is authoritative:

- sample rate comes from `prepareToPlay`;
- compile block size is the host-reported maximum block size;
- actual callback length comes from each `processBlock` call.

Neither sample rate nor block size is an editor preference or restored state value.

Each prepared instance owns a logical cursor in `[0, maximum_block_size)`. A callback is divided at:

- logical Onda block boundaries;
- supported MIDI event offsets.

For each segment, the adapter calls `onda_process_unchecked_segment` with full-block slab bindings,
`start_frame`, `frames`, and exact `ONDA_PROCESS_BEGIN_BLOCK` / `ONDA_PROCESS_END_BLOCK` flags.
Samples are processed immediately; the adapter does not accumulate a full block or add latency.

JUCE may call `prepareToPlay` again with a new sample rate or maximum block size. The old
specialization must not process under the new configuration. Until an exactly matching prepared
instance arrives, the instrument is silent and the effect is dry.

## External buffers

External buffers follow Onda's existing binding contract:

- Decode, validate, convert, and allocate away from the audio thread.
- Preserve each asset's finite positive sample rate.
- Store frame-major interleaved data in pointer-stable owned memory.
- Enforce scalar type, access mode, and static/dynamic channel constraints.
- Bind every declared buffer before preparing unchecked processing.

Plugin state embeds the original asset data for portability and may additionally store its source
path and content hash for display or relinking. Version 1 persists the original decoded binding,
not mutations made to a read-write buffer by Onda.

Missing, corrupt, incompatible, or oversized resources fail the reload transaction. The active
last-known-good instance remains unchanged.

## State and restoration

The VST3 state payload contains:

- plugin-state schema version;
- complete inline source project and active root file;
- source-project fingerprint;
- compiler and codegen options other than sample rate and block size;
- all 32 host slot values and active slot mappings;
- buffer bindings and embedded original assets;
- portable Onda state snapshot and its interface fingerprint;
- Onda compiler, MIR schema, processor ABI, and native-SDK versions;
- editor size, active file, cursor/view state, and presentation preferences;
- last specialization key for diagnostics and cache lookup.

The current host sample rate and maximum block size are not restored as preferences. Native
executable memory, an ORC handle, and unchecked object code are never serialized.

Restore is transactional:

1. Decode and validate the bounded versioned state payload.
2. Restore the fixed JUCE slot values.
3. Obtain the current host configuration.
4. Load an exact cache entry or request a new compile.
5. Validate the dynamic interface.
6. Restore and bind resources.
7. Create, initialize, and prepare a fresh Onda instance.
8. Overlay the portable state snapshot only when its interface and snapshot schema match.
9. Activate the replacement at a process boundary.

An incompatible Onda snapshot is ignored with a diagnostic; initialized source defaults remain
valid. Source, parameters, resources, and runtime state are never partially restored.

JUCE state callbacks must not synchronously compile. Saving reads a non-realtime snapshot owned by
the control domain. Restoring publishes a control-domain transaction and leaves the current or
fallback audio state active until preparation completes.

## Realtime ownership and failure policy

The audio callback must not:

- allocate or free heap memory;
- compile or optimize code;
- decode or access files;
- lock a mutex or wait on another thread;
- log or format diagnostics;
- resize a queue or payload;
- destroy a program, instance, editor object, or asset allocation.

Prepared replacements move through a bounded single-producer/single-consumer handoff. The audio
thread performs only an ownership swap. Retired ownership moves into a bounded deferred-drop queue
consumed elsewhere. If either queue is full, the new instance is not installed and nothing is
destroyed in the callback.

The current native backend can lower some failures to process-terminating traps. A DAW cannot
safely recover from such a trap. Production release therefore requires a plugin-safe codegen
profile in which every supported runtime failure records a bounded fault code and returns without
unwinding, panicking, throwing, or trapping.

On a runtime fault, the instance is marked faulted. From the next safe boundary, an instrument
produces silence and an effect uses explicit same-index dry pass-through. A preallocated fault code
is published to the editor.

## Editor

The initial editor uses JUCE components. It provides:

- multi-file source editing;
- diagnostics and compile/reload status;
- read-only host sample rate and maximum block size;
- slot mappings and reset-to-source-defaults;
- editor-only Onda parameters;
- buffer import, binding status, metadata, and relinking;
- arbitrary Onda event controls;
- control-output meters and runtime status;
- persisted size and source-view state.

It omits device selection, editable host configuration, standalone transport controls, and
child-process management.

The editor communicates with the engine through bounded commands and immutable snapshots. It never
owns or calls an Onda runtime instance, retains audio pointers, waits for compilation on the
message thread, or shares a mutex with `processBlock`.

## Implementation sequence

The JUCE repository may be scaffolded in parallel with Phase 1, including product metadata,
parameter declarations, fallback audio behavior, and validator wiring. It must not work around a
non-`Send` Onda program with raw-pointer casts or compile a complete project by writing temporary
files. The first end-to-end dynamic reload milestone begins only after the `Arc` ownership and
in-memory project APIs land in Onda.

### Phase 1: Onda native SDK prerequisites

- Replace `JitProgram`'s `Rc<MirJitProgram>` with the audited `Arc` ownership described above;
  prove `JitProgram: Send + Sync` and `Instance: Send`.
- Add an in-memory project compile call accepting a root path plus a bounded table of UTF-8
  path/content pairs. It must use the same project loader and import semantics as normal
  compilation without reading the host filesystem.
- Replace the borrowed/leaked single C diagnostic with an owned diagnostic collection that can
  represent every compile diagnostic and be explicitly destroyed.
- Add native SDK/compiler/MIR/processor-ABI version queries and a deterministic public-interface
  fingerprint derived from the canonical descriptor.
- Document and test the existing bind → validate → prepare → unchecked-process lifecycle,
  including rebind/reprepare and instance movement between non-concurrent threads.
- Add concurrent shared-program/multi-instance processing and destruction stress tests.
- Define the plugin-safe failure profile and bounded instance fault query. This may land after the
  first headless prototype, but it blocks production release.

### Phase 2: headless dynamic VST3 products

- Create the separate `onda-plugin` CMake/JUCE repository with pinned Onda and JUCE revisions.
- Add the Onda C-header/static-library import and the RAII `ProgramHandle`/instance wrappers.
- Build separate instrument and effect VST3 targets.
- Add the fixed layouts, UUIDs, and 32 stable slots.
- Implement `PreparedEngine`, float/double audio conversion, logical-block scheduling, and basic
  MIDI dispatch.
- Add `CompilerCoordinator`, specialization caching, stale-generation suppression, and
  last-known-good reload.
- Add external-buffer ownership, bounded replacement/retirement queues, and deferred destruction.
- Implement versioned state without an editor and exercise it in a headless processor harness.

### Phase 3: editor and integration

- Add the JUCE source editor, owned diagnostics, normalized slot mappings, plain-value display and
  entry, buffers, events, and meters.
- Add file watching and portable inline project state.
- Add representative DAW integration tests on Linux, macOS, and Windows.

### Phase 4: release hardening

- Complete and audit the plugin-safe generated-code profile.
- Prove callback allocation, locking, and destruction constraints.
- Run Steinberg's VST3 validator on both products.
- Document installation, signing/notarization, and debugging.

## Verification

Tests cover:

- normalized/plain parameter mapping, endpoints, rounding, arrays, and the 32-slot limit;
- exact product UUIDs and permanent host parameter IDs;
- dynamic audio-surface validation and channel ordering;
- note and MIDI payload conversion;
- event timing at zero, equal offsets, callback end, and logical block boundaries;
- scheduler flags across changing callback sizes;
- f32/f64 copy and conversion paths;
- buffer validation and missing-resource transactions;
- stale reload suppression and last-known-good retention;
- bounded handoff backpressure and deferred destruction;
- state save/restore with unchanged and changed host configurations;
- simultaneous processing of instances sharing one program;
- source, event, and state fuzzing without callback panic or out-of-range access;
- 44.1, 48, 88.2, 96, and 192 kHz where supported;
- fixed and varying callback sizes from one frame through the advertised maximum;
- realtime and offline host render modes;
- VST3 validator and representative DAW discovery/state smoke tests on all supported platforms.

## Release acceptance criteria

The first production release is complete when:

- separate dynamic instrument and effect VST3 bundles pass the validator;
- both products always expose their fixed UUID, layout, MIDI capability, and 32 parameter slots;
- discovery performs no source loading or compilation;
- host configuration is the only sample-rate and maximum-block-size authority;
- a configuration change activates only an exactly matching specialization;
- timed notes reach the canonical Onda event ABI at correct offsets;
- invalid reloads retain the last known-good program;
- multiple instances can compile, share code, process, reload, save, restore, and close without
  races or audio-thread destruction;
- supported runtime failures cannot terminate or unwind through the host;
- saved state can recreate the same source project, slots, buffers, compatible Onda state, and
  audible result after the DAW and plugin process have closed.

## Deferred work

- Generated products and direct CLAP/VST3 wrappers:
  [`todo/generated-plugins.md`](todo/generated-plugins.md).
- CLAP support for the dynamic shell.
- VST3 per-note expression and stable note-ID propagation.
- Sample-accurate top-level parameter semantics.
- Transport and tempo inputs.
- Sidechains, auxiliary buses, and dynamic host layouts.
- MIDI output and voice-termination notifications.
- Mutable external-buffer snapshots.
- Source-derived tail and non-zero latency reporting.
