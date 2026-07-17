# Direct native processor object

This example compiles the shared
[`sample_player.onda`](../../buffers-fft-convolution/sample_player.onda) to a relocatable object,
links it directly into a C application, and calls the raw processor ABI. It does not link an Onda
runtime, compiler, loader, or linker wrapper.

The Onda program is a small stereo sample player. Its `play(bool)` event starts or stops a
host-bound clip, its `speed` parameter controls playback rate, and its single audio input controls
amplitude. Enabling playback restarts the clip from frame zero; playback stops at the buffer end.

The processor deliberately exercises the complete native call surface:

- one amplitude input and two planar audio outputs;
- one playback-speed parameter and persistent state;
- a host-owned, interleaved, dynamic-channel external buffer;
- all buffer metadata tables: data, frames, channels, and sample rate;
- the `play(bool)` event exported as `onda_event_0`;
- one complete logical block processed with a single `onda_process` call.

[`generate_config.py`](generate_config.py) reads the exact emitted sidecar and generates the small C
header consumed by [`host.c`](host.c). Nothing about target storage sizes, alignments, parameter
bytes, flattened port counts, buffer ordinals, event ordinals, or event payload offsets is copied
by hand. The C host is program-independent: it allocates every declared surface through loops,
binds synthetic data to every input and buffer, triggers fixed-size events with their descriptor
defaults (or zero for required fields without defaults), processes one block, and reports every
output slot. Events with dynamic slice payloads are listed but skipped because a generic host has
no application payload to supply.

The example uses ordinary `malloc` and verifies the actual returned state and parameter addresses
against the descriptor's alignment requirements before calling Onda. A host whose allocator does
not satisfy a descriptor must use its platform's over-aligned allocation API instead.

## Linux x86-64

```bash
onda compile \
  examples/buffers-fft-convolution/sample_player.onda \
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
  examples/buffers-fft-convolution/sample_player.onda \
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
  .\examples\buffers-fft-convolution\sample_player.onda `
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
bound buffer[0] 'clip': 512 frames, 2 channels, 48000 Hz
triggered event[0] 'play' with its default payload
descriptor target: <the selected target triple>
output[0] peak: 0.200000
output[1] peak: 0.200000
```

For another OS or architecture, select or add a target spec matching the final process, then use
that platform's normal C compiler/linker driver. The Onda compiler always stops at the relocatable
object; it does not need or ship the platform linker, SDK, or C runtime.

## What the host builds from the sidecar

The application performs the integration itself:

1. Verify the descriptor and ABI versions and the target profile.
2. Generate target-correct C constants and parameter bytes directly from that sidecar.
3. Allocate and zero state using `runtime.state_size_bytes` and `runtime.state_align_bytes`.
4. Encode parameters using `runtime.param_size_bytes` and `metadata.params[*].byte_offset`.
5. Build flattened input/output pointer tables in metadata slot order.
6. Build the four parallel external-buffer tables in `metadata.buffers` order.
7. Call `onda_init(params, state)` once.
8. Encode fixed event defaults from `metadata.events` and call exports through a generated function
   table.
9. Call `onda_process` once with `ONDA_PROCESSOR_FULL_BLOCK` for the complete block.

The sidecar also determines when pointers are absent. Pass null for a surface only when its
descriptor count or storage size is zero:

- `params` when `runtime.param_size_bytes == 0`;
- `state` when `runtime.state_size_bytes == 0`;
- `inputs` or `outputs` when their flattened metadata slot count is zero;
- all four buffer-table arguments when `metadata.buffers` is empty;
- an event payload when that event's `payload_bytes == 0`.

When a surface is declared, its table is required even if the application considers it unused. In
particular, a non-empty `metadata.buffers` list requires all four parallel tables. An empty
individual buffer binding uses a null data-pointer entry and zero frame/channel entries, rather
than null buffer tables. Every declared input and output channel must likewise have valid
compile-block storage.

The calling application also owns ordinary final-link dependencies and any thread-local
floating-point policy required by its audio environment.
