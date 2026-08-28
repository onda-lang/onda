# Direct native processor object

This example compiles the shared
[`sample_player.onda`](../../buffers/sample_player.onda) to a relocatable object,
links it directly into a C application, and calls the raw processor ABI. It does not link an Onda
runtime, compiler, loader, or linker wrapper.

The Onda program is a small stereo sample player. Its `play(bool)` event starts or stops a
host-bound clip and its `speed` parameter controls playback rate. Enabling playback restarts the
clip from frame zero; playback stops at the buffer end.

The processor deliberately exercises the complete native call surface:

- zero audio inputs and two planar audio outputs;
- one playback-speed parameter and persistent state;
- a host-owned, interleaved, dynamic-channel external buffer;
- all buffer metadata tables: data, frames, channels, and sample rate;
- the `play(bool)` event exported as `onda_event_0`;
- one complete logical block processed with a single `onda_process` call.

[`generate_config.py`](generate_config.py) reads the exact emitted sidecar and generates the small C
header consumed by [`host.c`](host.c). Nothing about target storage sizes, alignments, parameter
bytes or control domains, flattened port counts, buffer ordinals, event ordinals, or event payload
offsets is copied by hand. The generated header includes descriptor-derived plain/normalized
conversion and typed parameter-write helpers. The C host is program-independent: it allocates
every declared surface through loops, round-trips every scalar host parameter through its
normalized domain, binds synthetic data to every input and buffer, triggers fixed-size events with
their descriptor defaults (or zero for required fields without defaults), processes one block, and
reports every output slot. Events with dynamic slice payloads are listed but skipped because a
generic host has no application payload to supply.

The example uses ordinary `malloc` and verifies the actual returned state and parameter addresses
against the descriptor's alignment requirements before calling Onda. A host whose allocator does
not satisfy a descriptor must use its platform's over-aligned allocation API instead.

## Linux x86-64

```bash
onda compile \
  examples/buffers/sample_player.onda \
  --emit obj \
  --target-spec targets/linux-x64-generic.toml \
  --output target/sample_player.o \
  --meta-out target/sample_player.onda.json

python3 examples/native/raw_processor_object/generate_config.py \
  target/sample_player.onda.json \
  target/processor_config.h

cc -O2 -Iinclude -Itarget \
  examples/native/raw_processor_object/host.c \
  target/sample_player.o \
  -lm \
  -o target/sample_player_host

target/sample_player_host
```

## macOS on Apple Silicon

Use the checked-in arm64 macOS target and the platform Clang linker driver:

```bash
onda compile \
  examples/buffers/sample_player.onda \
  --emit obj \
  --target-spec targets/macos-arm64-generic.toml \
  --output target/sample_player.o \
  --meta-out target/sample_player.onda.json

python3 examples/native/raw_processor_object/generate_config.py \
  target/sample_player.onda.json \
  target/processor_config.h

clang -O2 -Iinclude -Itarget \
  examples/native/raw_processor_object/host.c \
  target/sample_player.o \
  -lm \
  -o target/sample_player_host

target/sample_player_host
```

## Windows x64

Run these commands in PowerShell from an x64 Visual Studio developer shell. The emitted object is
MSVC-compatible COFF, so the normal `cl.exe` compiler driver can compile the host and invoke the
platform linker:

```powershell
onda.exe compile `
  .\examples\buffers\sample_player.onda `
  --emit obj `
  --target-spec .\targets\windows-x64-generic.toml `
  --output .\target\sample_player.obj `
  --meta-out .\target\sample_player.onda.json

py -3 .\examples\native\raw_processor_object\generate_config.py `
  .\target\sample_player.onda.json `
  .\target\processor_config.h

cl.exe /nologo /O2 /Iinclude /Itarget `
  .\examples\native\raw_processor_object\host.c `
  .\target\sample_player.obj `
  /Fe:.\target\sample_player_host.exe

.\target\sample_player_host.exe
```

`clang-cl` accepts this example's same MSVC-style arguments; replace only `cl.exe` with
`clang-cl`. Both drivers still need the Visual Studio developer environment so the Windows SDK,
C runtime libraries, and linker are discoverable. The host deliberately does not require
`/std:c11`: its former C11-only alignment query was replaced by a check of the addresses actually
returned by `malloc`, and its ABI-version assertion is handled by the preprocessor.

The expected output on all three platforms starts with:

```text
parameter[0] 'speed': plain 1.000000 x, normalized 0.500000
bound buffer[0] 'clip': 512 frames, 2 channels, 48000 Hz
triggered event[0] 'play' with its default payload
descriptor target: <the selected target triple>
output[0] peak: 1.000000
output[1] peak: 1.000000
```

For another OS or architecture, select or add a target spec matching the final process, then use
that platform's normal C compiler/linker driver. The Onda compiler always stops at the relocatable
object; it does not need or ship the platform linker, SDK, or C runtime.

## What the host builds from the sidecar

The application performs the integration itself:

1. Verify the descriptor and ABI versions and the target profile.
2. Generate target-correct C constants and parameter bytes directly from that sidecar.
3. Allocate state using `runtime.state_size_bytes` and `runtime.state_align_bytes`.
4. Encode defaults and parameter-control tables using `runtime.param_size_bytes`,
   `metadata.params[*].byte_offset`, ranges, scalar types, and `param_control`.
5. Build flattened input/output pointer tables in metadata slot order.
6. Build the four parallel external-buffer tables in `metadata.buffers` order.
7. Call `onda_processor_init(params, state, ONDA_PROCESSOR_INIT_FULL, output)` once and reject a nonzero
   execution status.
8. Encode fixed event defaults from `metadata.events` and call exports through a generated function
   table.
9. Preallocate independent optional call-scoped delegate and print batches when the host wants
   occurrences, and pass their pointers through one `onda_processor_execution_output_t`.
10. Call `onda_process` once with `ONDA_PROCESSOR_FULL_BLOCK` for the complete block, stopping
   immediately if an event or process call returns a nonzero execution status.

The generated header exposes:

```c
double processor_param_normalized_to_plain(int index, double normalized);
double processor_param_plain_to_normalized(int index, double plain);
double processor_param_read_plain(const void* params, int index);
int processor_param_set_plain(void* params, int index, double plain);
int processor_param_set_normalized(void* params, int index, double normalized);
```

The indexed conversion wrappers build `onda_processor_param_domain` values from the generated
tables and delegate to the reusable functions in `include/onda_processor_abi.h`:

```c
double onda_processor_param_constrain_plain(
  const onda_processor_param_domain* domain,
  double plain
);
double onda_processor_param_normalized_to_plain(
  const onda_processor_param_domain* domain,
  double normalized
);
double onda_processor_param_plain_to_normalized(
  const onda_processor_param_domain* domain,
  double plain
);
```

They return `NaN` or `-1` for an invalid index, an array, or a numeric parameter without a control
range. Boolean parameters use the `0.5` threshold. Numeric setters clamp first, then snap stepped
domains, then write the declared scalar representation at its generated byte offset. These helpers
are host support generated from the sidecar; they are not exports added to the processor object.
Hosts with their own descriptor loader can construct the same domain structure and call the ABI
header functions directly without using the reference generator.

### Execution output batches

Pass null as the final init/process/event argument when no host-facing occurrences are collected.
Otherwise allocate storage before realtime execution and group the independent batches in one
`onda_processor_execution_output_t`:

```c
uint8_t delegate_storage[4096];
onda_processor_delegate_batch_t delegates = {
  .storage = delegate_storage,
  .capacity_bytes = sizeof(delegate_storage),
};
uint8_t print_storage[4096];
onda_processor_print_batch_t prints = {
  .storage = print_storage,
  .capacity_bytes = sizeof(print_storage),
};
onda_processor_execution_output_t execution_output = {
  .delegate_batch = &delegates,
  .print_batch = &prints,
};

uint32_t status = onda_process(
  state,
  params,
  inputs,
  outputs,
  0,
  block_size,
  ONDA_PROCESSOR_FULL_BLOCK,
  buffers,
  buffer_frames,
  buffer_channels,
  buffer_sample_rates,
  &execution_output
);
if (status == ONDA_PROCESSOR_EXECUTION_OK) {
  onda_processor_batch_cursor_t cursor = {0};
  onda_processor_delegate_occurrence_t occurrence;
  while (onda_processor_delegate_batch_next(&delegates, &cursor, &occurrence)) {
    consume_delegate(&occurrence);
  }
  if (delegates.overflow_count != 0u) {
    report_delegate_overflow(delegates.overflow_count);
  }
}
```

Print records use `ONDA_PROCESSOR_PRINT_RECORD_HEADER_SIZE` plus the fixed
`metadata.log_sites[site_index].payload_size_bytes`. The host resolves labels, scalar types, and
source spans through `metadata.log_sites` and `metadata.source_files`, then decodes or formats only
after generated execution. Print and delegate capacities never compete. Print records emitted
before a generated failure remain available; delegates are cleared on failure.

For fixed payloads, one record occupies
`ONDA_PROCESSOR_DELEGATE_RECORD_HEADER_SIZE + payload_size_bytes`. Dynamic slice sizes and the
number of occurrences depend on runtime execution, so whole-batch capacity remains a host policy.
See [Hosting Onda delegates](../../../docs/delegates.md) for detailed sizing and lifecycle guidance.

The sidecar also determines when pointers are absent. Pass null for a surface only when its
descriptor count or storage size is zero:

- `params` when `runtime.param_size_bytes == 0`;
- `state` when `runtime.state_size_bytes == 0`;
- `inputs` or `outputs` when their flattened metadata slot count is zero;
- all four buffer-table arguments when `metadata.buffers` is empty;
- an event payload when that event's `payload_size_bytes == 0`.

Within a present `buffers` table, an individual null entry denotes an unbound buffer. Supply its
neutral one-frame shape metadata as described by the processor ABI; reads return zero and writes
are discarded by processor-owned storage. Fixed buffer arrays occupy contiguous physical entries;
use `metadata.buffer_arrays` to map each logical group to its first entry and length instead of
parsing generated names.

When a surface is declared, its table is required even if the application considers it unused. In
particular, a non-empty `metadata.buffers` list requires all four parallel tables. Every entry has
positive frame/channel counts and a finite positive sample rate, but its sample pointer may be null
to select neutral storage. Every non-null sample pointer identifies nonempty bound storage. Every
declared input and output channel must likewise have valid compile-block storage.

The calling application also owns ordinary final-link dependencies and any thread-local
floating-point policy required by its audio environment.
