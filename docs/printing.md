# Print host integration

This is an internal integration reference for Onda hosts and backend maintainers. The public
language syntax and semantics live in [the language guide](syntax.md#printing).

An authored `print(...)` statement evaluates its scalar arguments in source order and publishes one
typed occurrence. Generated DSP code never allocates text or calls a host formatter. The host may
supply a bounded caller-owned print batch through `ExecutionOutput`; after execution it can format
the batch as canonical, newline-terminated UTF-8.

For the raw entry-point layout, see
[the processor ABI](processor-abi.md#call-scoped-execution-output). Prints and
[delegates](delegates.md) share an execution-output container but never share capacity.

Each print and delegate record carries a sequence from one counter reset at the start of the current
init, event, or process segment. A host presenting both streams merges that call's records by this
sequence. The value is intentionally not a timeline across separate calls or process segments.

Print and delegate batches are independent. Exhausting print capacity cannot drop delegates, and
omitting print storage suppresses delivery without changing Onda execution. Records that do not fit
are dropped whole and increment the saturated `overflow_count`. Records emitted before a generated
runtime failure remain available for diagnostics.

Prints preserve concrete scalar types. With no surrounding parameter type to constrain them, pure
numeric literals use Onda's ordinary defaults: `print(3)` records `i32` and `print(3.0)` records
`f32`. Use `i64(3)` or `f64(3.0)` when the wider type is intentional. Variables and other typed
expressions retain their existing type.

## Hosted C

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

Initialization can print too. Supply an output to `onda_instance_create_initialized`,
`onda_instance_create_initialized_with_allocator`, or `onda_init`. Allocation-only constructors do
not execute authored code and therefore take no output. If initialized construction fails, its
output batches are cleared and the diagnostic is the only result.

### Ownership and allocation

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
both host-facing streams.

## Raw processor ABI and WebAssembly

Raw native and complete core-Wasm artifacts accept
`onda_processor_execution_output_t`, whose batch and storage pointers are independently nullable.
Each print record is `u32 site_index`, `u32 payload_size`, then packed scalar bytes. Resolve the site
through descriptor `metadata.log_sites` and its source span through `metadata.source_files`.

For JavaScript hosts, `@onda-lang/processor-abi` owns the linear-memory mechanics:

```js
writePrintBatch(memory, printBatchAddress, storageAddress, capacityBytes);
writeExecutionOutput(memory, outputAddress, 0, printBatchAddress);

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

## CLI, control clients, and Web Audio

Ordinary `onda run play` and `onda run render` write authored print text to stdout after generated
execution. In `--control-json` mode stdout remains reserved for the startup handshake; the localhost
control socket sends ordered `print` notifications with `text`, structured `entries`,
`overflowCount`, and `transportDropCount`.

The Web Audio worklet transports raw records through a bounded queue and performs no string
formatting in the render callback. Collection is active only while a main-side listener exists.
Subscribe with `processor.onPrint(...)`, or pass the factory's construction-time `onPrint` option
when output from `createOndaAudioProcessorInitialized(...)` initialization must be observed.
Generated batch overflow and worklet-to-main transport loss are separate, and a loss-only
notification is delivered even when no later authored print occurs.

The native and webview run hosts display prints and subscribed top-level delegate occurrences in a
compact chronological Log immediately after the scope. It remains hidden until the first print or
delegate occurrence of the run session, then stays present for that session even when cleared. It
provides clear, bottom-aware automatic following, print source context, and separate print and
delegate loss counts. New output follows the bottom only while the log is already fully scrolled
down.
