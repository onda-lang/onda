---
title: Web API reference
description: Compile Onda programs and play them in browser applications with the public JavaScript packages.
permalink: /docs/web-api/
section: reference
eyebrow: JavaScript and WebAssembly
---

# Web API reference

Use `@onda-lang/wasm-compiler` to compile Onda source in a browser or Node.js, and
`@onda-lang/webaudio` to play the result through an `AudioContext`.

```bash
npm install @onda-lang/wasm-compiler @onda-lang/webaudio
```

| Package | Use it to |
| --- | --- |
| `@onda-lang/wasm-compiler` | Compile source files or projects to WebAssembly processor artifacts. |
| `@onda-lang/webaudio` | Play an artifact, change parameters, send events, and save state. |
| `@onda-lang/processor-abi` | Validate artifacts, inspect metadata, and build a custom host. |
| `@onda-lang/binaryen-web` | Compile trusted Onda MIR when integrating a separate compiler frontend. |

All packages use ECMAScript modules and include TypeScript declarations. The compiler packages
support modern browsers and Node.js 20 or newer. Playback requires a browser with `AudioWorklet`.
Each published package includes this reference as `api.md`.

## Compile source and start audio

Create a compiler once and reuse it. Worker mode keeps compilation off the browser's main thread.
Call `startAudio(source)` from a user action such as a Play button, passing the Onda source text:

```js
import { createCompiler } from "@onda-lang/wasm-compiler";
import { createOndaAudioProcessorInitialized } from "@onda-lang/webaudio";

const compiler = await createCompiler({ worker: true });

async function startAudio(source) {
  const context = new AudioContext();
  await context.resume();

  const { artifact } = await compiler.compileSource(source, {
    sampleRate: context.sampleRate,
    blockSize: 512,
  });
  const processor = await createOndaAudioProcessorInitialized(context, artifact);
  processor.node.connect(context.destination);
  return { context, processor };
}
```

Compile for the context's exact sample rate. The adapter rejects a mismatch, and the program must
have at least one audio input or output. The initialized factory resolves only after successful
initialization.

Keep the returned processor and context so you can release them when playback ends:

```js
processor.close();
processor.node.disconnect();
await context.close();
```

Call `await compiler.dispose()` when you no longer need compilation. Closing a processor rejects pending
requests and removes its listeners; it does not disconnect the caller-owned node or close the
context.

Your bundler must make the compiler worker and AudioWorklet modules available at runtime. Pass
`workerUrl` to `createCompiler` or `workletUrl` to the audio factory if your deployment serves them
from custom URLs. The [playground example](https://github.com/onda-lang/onda/tree/main/examples/web/onda_wasm_playground)
shows a complete browser integration.

## `@onda-lang/wasm-compiler`

### `createCompiler(options?)`

Returns an `OndaCompilerInstance`. With no options, compilation runs on the calling JavaScript
thread. Pass `{ worker: true }` for a module worker. `workerUrl`, `frontendWasm`, and a custom
`Worker` constructor are available for custom loading environments.

| Task | Methods |
| --- | --- |
| Compile source | `compileSource(source, options?)`, `compileWorkspace(workspace, options?)` |
| Inspect compile-time settings | `inspectSourceConstants`, `inspectWorkspaceConstants`, `inspectProjectImageConstants` |
| Save or reopen portable projects | `createProjectImage`, `inspectProjectImage`, `loadProjectFiles`, `materializeProjectImage`, `compileProjectImage` |
| Read or write typed assets | `encodeBufferAsset`, `decodeBufferAsset`, `decodeBufferFile` |
| Query supported project formats | `projectCapabilities()` |
| Integrate language-server features | `sendLspMessage`, `setLspAnalysisOptions` |
| Release compiler resources | `dispose()` |

### Multiple source files

Supply an entry path and a map of source filenames to text. Imports and includes resolve from this
map and the bundled standard library:

```js
const result = await compiler.compileWorkspace({
  entry: "main.onda",
  sources: {
    "main.onda": mainSource,
    "dsp/filter.onda": filterSource,
  },
}, {
  sampleRate: 48000,
  blockSize: 512,
});
```

A successful result contains `artifact`, resolved `sourceFiles`, and `sourceGraph` when available.
Use the source graph with `createProjectImage` to save a portable project with its buffer assets.
See [Working with projects](https://onda-lang.org/docs/projects/) for the editable project format.

### Compile options and diagnostics

`sampleRate` defaults to 48000 and `blockSize` to 512. `constants` supplies overrides for declared
compile-time settings. Values use booleans, numbers, `bigint` for `i64`, boolean arrays, and matching
numeric typed arrays.
Inspection methods return each constant's type, shape, and resolved value; enum descriptors also
include member names and labels.

A workspace may carry project defaults in `workspace.constants`, using the project file's JSON
representation, including decimal strings for `i64`. Explicit `options.constants` override these
defaults. Pass the workspace defaults to `createProjectImage` when exporting an edited project.

`codegen` controls optimization level (`optimizeLevel`, 0–4), size optimization (`shrinkLevel`, 0–2),
`fastMath`, `simd`, and optional WAT output (`emitText`). See
[the TypeScript declarations](https://github.com/onda-lang/onda/blob/main/packages/onda_wasm_compiler/src/index.d.ts)
for all options and method signatures.

Catch `OndaCompileError` for source and project compilation failures. Its `diagnostics` provide
messages and source locations. `sourceFiles` and `unresolvedSourceFiles` let editors retain file
watches while a program is invalid. `OndaCompilerError` reports compiler setup or lifecycle errors;
`OndaBinaryenError` reports a WebAssembly compilation failure.

The `/artifact` module path exposes artifact helpers without loading the compiler. The `/worker`
path is the worker's entry module and has no named exports.

## `@onda-lang/webaudio`

### Construction

Use `createOndaAudioProcessorInitialized(context, artifact, options?)` for immediate playback.
Use `createOndaAudioProcessor` when you need to change parameters before the first initialization,
then call `processor.init(ONDA_INIT_FULL)` explicitly. Until initialization succeeds, the processor
emits silence and rejects operations that require initialized state.

| Option | Purpose |
| --- | --- |
| `params` | Initial plain parameter values. Names must match the program. |
| `buffers` | Initial external-buffer bindings by name, array group, or slot order. |
| `paramSmoothingSeconds` | Optional host parameter ramps; defaults to `0`. |
| `eventPayloadCapacityBytes` | Reserved dynamic event capacity; defaults to 64 KiB. |
| `delegateCapacityBytes`, `printCapacityBytes` | Output collection capacities; each defaults to 64 KiB. Zero disables that stream. |
| `onPrint` | Initial print listener, including prints from initialization. |
| `compiledModule` | Reuse a module prepared with `compileOndaProcessorModule`. |
| `workletUrl` | Custom URL for the AudioWorklet module. |

The factory registers the worklet and prepares the WebAssembly module before playback. Advanced
hosts can use `registerOndaAudioWorklet`, `compileOndaProcessorModule`, and
`ondaAudioWorkletNodeOptions` separately. See
[the adapter declarations](https://github.com/onda-lang/onda/blob/main/packages/onda_webaudio/src/index.d.ts)
for custom node options and constructors.

### Parameters

For a program with a `cutoff` parameter and an `offsets` array:

```js
await processor.setParam("cutoff", 440);
await processor.setParamNormalized("cutoff", 0.5);
await processor.setParamElement("offsets", 1, 12);
await processor.resetParams();
```

Plain writes use the parameter's declared units. Normalized writes use its range and scale, clamp
to the range, and snap stepped values. Whole arrays require an exact-length array. Indexed names
such as `offsets[1]` also work with `setParam` and `setParamNormalized`. Use
`setParamElementNormalized` for a normalized write to one array element.

`paramSmoothingSeconds` applies finite linear ramps to continuous floating-point parameters.
Integer, boolean, and stepped parameters update directly. Ramps update once per Onda compile block;
they do not interpolate inside individual samples. Durations at most one block are bypassed.
Change the duration live with `setParamSmoothingSeconds(seconds)`, using a finite, nonnegative value.
Initial parameters, reinitialization, and `resetParams()` settle immediately. Ramps are not saved
in processor snapshots.

### Events and output

`trigger(nameOrIndex, values?)` accepts an array in declaration order or an object keyed by parameter
name. Structs are objects with their declared fields; tuples, fixed arrays, and slices are sequences.
For an event with a `note` argument containing `frequency` and `velocity`:

```js
await processor.trigger("note_on", {
  note: { frequency: 523.25, velocity: 0.7 },
});
```

Missing event arguments use declared defaults. Fixed arrays must match their declared length.
Supply `i64` values as `bigint`, exact safe integers, or decimal strings. Unknown names and negative
indices are rejected; unknown nonnegative numeric indices are successful no-ops.

`onPrint`, `onDelegates`, and `onExecutionError` return unsubscribe functions. Register `onPrint`
in the factory options to include initialization output, or subscribe later for future calls:

```js
const stopPrints = processor.onPrint(({ text, overflowCount, transportDropCount }) => {
  console.debug(text);
  if (overflowCount || transportDropCount) console.warn("Some output was dropped");
});

const stopDelegates = processor.onDelegates(({ occurrences }) => {
  for (const occurrence of occurrences) {
    console.log(occurrence.name, occurrence.values);
  }
});
```

Print and delegate delivery requires cross-origin isolation and `SharedArrayBuffer`; ordinary audio
playback remains available without shared memory. Collection is active only while listeners are
registered and capacity is nonzero. `overflowCount` counts records dropped by the processor;
`transportDropCount` counts records lost during delivery to the main thread. A notification may
report loss without carrying any occurrences.

Both listener types run on the main thread in call-local sequence order. They do not run inside
DSP execution. Ordering starts again for each initialization, event, or process segment.

### State and errors

| Operation | Purpose |
| --- | --- |
| `init(ONDA_INIT_FULL)` | Clear and initialize all processor state. |
| `init(ONDA_INIT_PRESERVE_PINNED)` | Reinitialize while retaining pinned state. |
| `snapshot()`, `restoreSnapshot(bytes)` | Save and restore persistent state. Restore does not deliver initialization prints or delegates. |
| `readControlOutputs()` | Read held control-output values. |
| `readBuffer(nameOrIndex)` | Read buffer contents. |
| `close(reason?)` | Release adapter resources; subsequent operations fail. |

Catch rejected parameter, initialization, and event promises. `OndaRuntimeError` exposes an
`operation` and numeric `code` from `ONDA_RUNTIME_ERROR`. `OndaExecutionError` exposes an `operation`
and generated execution `status`:

| Status | Meaning |
| --- | --- |
| `PROCESSOR_EXECUTION_OK` (`0`) | Execution succeeded. |
| `PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE` (`1`) | State is invalidated; audio is silenced until full initialization or snapshot restoration succeeds. |
| `PROCESSOR_EXECUTION_INPUT_REJECTED` (`2`, events only) | The payload or workspace was rejected; state remains usable. |

Use `onExecutionError` for asynchronous processing failures. Initialization and event failures reject
their own promises and are not reported again to this listener. If the adapter closes, including
a terminal control-transport failure, create a new processor. Print/delegate overflow is reported
separately and does not by itself close the processor.

Web Audio adapts its callbacks to Onda's compiled block size; you do not need a segmented-process
method. Audio converts at Web Audio's `f32` boundary: integer inputs truncate and saturate (NaN
becomes zero), boolean inputs use nonzero comparison, and outputs convert to `f32`. Event and
delegate values retain their declared types and precision.

The `/worklet` module path registers the AudioWorklet processor and has no named exports.

## `@onda-lang/processor-abi`

Use this package for artifact files, metadata-driven controls, or a custom WebAssembly host.
An `OndaProcessorArtifact` contains WebAssembly bytes, a metadata descriptor, and optional WAT.
Keep the bytes and descriptor together; they describe one exact compilation.

### Validation and files

- `validateProcessorMetadata(metadata, expectedKind?)` checks a descriptor.
- `validateProcessorArtifact(artifact, options?)` normalizes bytes and optionally inspects the module.
- `validateProcessorModule(module, metadata)` checks a precompiled module against its descriptor.
- `parseProcessorMetadata(input, expectedKind?)` accepts JSON or an object.
- `serializeProcessorMetadata(metadata, space?)` validates and produces JSON.
- `createProcessorArtifactFiles(artifact, options?)` creates matching `.wasm` and `.onda.json` records.
- `loadProcessorArtifactFiles(wasmBytes, metadataJson)` verifies and loads a saved pair.

Failures throw `OndaArtifactError`. Complete modules have no imports and can be instantiated with
`WebAssembly.instantiate(artifact.wasm, {})`. Direct hosts must allocate storage and drive the
[Processor API](https://onda-lang.org/docs/processor-api/); the Web Audio adapter does this for you.

### Parameter controls

Prepare a control with `createParamControl(metadata, elementIndex?)` for repeated conversion, or
use `paramNormalizedToPlain`, `paramPlainToNormalized`, and `constrainParamPlain` for individual
values. Prepared controls expose `constrainPlain`, `normalizedToPlain`, and `plainToNormalized`.
`paramElementMetadata` selects one array element; `createParamDomain` accepts already decoded
values.

Conversions preserve endpoints, clamp ranges, and snap steps. Booleans use a `0.5` threshold;
unranged numbers have no normalized control domain. Full-width `i64` values use `bigint` for direct
writes. Numeric host controls are limited to exactly representable binary64 integer domains.
These helpers are also re-exported by `@onda-lang/webaudio`.

### Structured event and delegate values

`PayloadPlan` prepares a message schema for encoding and decoding. `encode(values)` accepts named
objects or positional arrays; `decode(bytes)` returns named values with `bigint` for `i64`.
Structs must contain exactly their declared fields; unknown names and extra positional values are
rejected. Missing event arguments use declared defaults.

`tensorMetadata` describes each primitive leaf's path, encoding, fixed shape, owning parameter,
slice status, and fixed element count. For a slice, multiply its logical length by
`fixedElementCount` to obtain the tensor length. Tuple path components use `__0`, `__1`, and so on.

For direct Wasm calls, use `sizes(lengths)` or `requiredWorkspace(bytes)` to provision packed input
and aligned workspace, then `writeEventInput` to write the required 16-byte event descriptor.
`OndaAudioProcessor.trigger` performs this preparation for ordinary browser hosts.

Decoded `f32` values use `canonicalF32Number` to return the shortest decimal representation that
round-trips to the same `f32` value.

### Raw delegate and print batches

Direct hosts use `writeDelegateBatch`, `writePrintBatch`, and `writeExecutionOutput` to prepare
caller-owned collection storage. Reset it with `resetExecutionOutput` before each init or process
call; event entries reset it before input validation. Storage must be disjoint from other memory
accessed by the call and remain valid until records have been consumed.

`readDelegateBatch` and `readPrintBatch` validate result counters. `decodeDelegateBatch` decodes a
complete delegate batch; `decodeDelegateRecords` handles an isolated storage region.
`decodePrintRecords` retains typed values and source sites. `formatPrintRecords` and
`formatPrintBatch` produce text. Merge print and delegate records by `sequence` when presenting
one call's output; `overflowCount` counts whole records that did not fit.

See the [processor output layout](https://onda-lang.org/docs/processor-api/#call-scoped-execution-output)
for storage requirements and the
[repository integration guide](https://github.com/onda-lang/onda/blob/main/docs/host-output.md#javascript-and-web-audio)
for complete collection examples.

## `@onda-lang/binaryen-web`

Use `compileTrustedMir(mir, options?)` only when supplying MIR from a matching Onda frontend. For
source text, use `@onda-lang/wasm-compiler` instead. MIR input may be MessagePack, JSON, or an
already decoded object; this synchronous function returns an `OndaProcessorArtifact`. It does not
validate untrusted or hand-authored MIR.

`SUPPORTED_MIR_SCHEMA_VERSION` identifies the required input version. `OndaBinaryenOptions`
controls optimization, fast math, SIMD, and optional WAT. Failures throw `OndaBinaryenError`.
The `/artifact` module path exposes artifact helpers without loading Binaryen.

## Export indexes

Expand a package's index to look up runtime exports and TypeScript types. Exact signatures are
included in each installed package's declarations.

<details markdown="1">
<summary>@onda-lang/processor-abi exports</summary>

<!-- BEGIN WEB API onda_processor_abi -->

```text
DELEGATE_BATCH_SIZE_BYTES
DELEGATE_RECORD_HEADER_SIZE_BYTES
EXECUTION_OUTPUT_SIZE_BYTES
OndaArtifactError
OndaArtifactKind
OndaBufferArrayMetadata
OndaBufferMetadata
OndaDelegateBatch
OndaDelegateMetadata
OndaDelegateOccurrence
OndaDelegateParamMetadata
OndaEventMetadata
OndaEventParamMetadata
OndaIntegrationProfile
OndaEnumMetadata
OndaIoMetadata
OndaLogSiteMetadata
OndaParamControlMetadata
OndaParamDomain
OndaPreparedParamControl
OndaPrintBatch
OndaPrintEntry
OndaProcessorArtifact
OndaProcessorInitMode
OndaProcessorMetadata
OndaScalarType
OndaStateMetadata
OndaTargetInfo
PRINT_BATCH_SIZE_BYTES
PRINT_RECORD_HEADER_SIZE_BYTES
PROCESSOR_ABI_VERSION
PROCESSOR_ARTIFACT_FORMAT
PROCESSOR_ARTIFACT_FORMAT_VERSION
PROCESSOR_BEGIN_BLOCK
PROCESSOR_END_BLOCK
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_FULL_BLOCK
PROCESSOR_INIT_FULL
PROCESSOR_INIT_PRESERVE_PINNED
PROCESSOR_SNAPSHOT_FORMAT_VERSION
constrainParamPlain
createParamControl
paramElementMetadata
createParamDomain
createProcessorArtifactFiles
decodeDelegateBatch
decodeDelegateRecords
decodePrintRecords
formatPrintBatch
formatPrintRecords
loadProcessorArtifactFiles
paramNormalizedToPlain
paramPlainToNormalized
parseProcessorMetadata
readDelegateBatch
readPrintBatch
resetExecutionOutput
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
writeDelegateBatch
writeExecutionOutput
writePrintBatch
OndaPayloadType
OndaPayloadField
OndaPayloadSchema
OndaPayloadDefault
OndaPayloadValue
OndaPayloadSizes
OndaPayloadTensorMetadata
PayloadPlan
canonicalF32Number
EVENT_INPUT_SIZE_BYTES
PROCESSOR_EXECUTION_INPUT_REJECTED
writeEventInput
```

<!-- END WEB API onda_processor_abi -->

</details>

<details markdown="1">
<summary>@onda-lang/binaryen-web exports</summary>

<!-- BEGIN WEB API onda_binaryen_web -->

```text
OndaArtifactError
OndaBinaryenError
OndaBinaryenOptions
OndaProcessorArtifact
OndaProcessorInitMode
OndaProcessorMetadata
PROCESSOR_ABI_VERSION
PROCESSOR_ARTIFACT_FORMAT
PROCESSOR_ARTIFACT_FORMAT_VERSION
PROCESSOR_BEGIN_BLOCK
PROCESSOR_END_BLOCK
PROCESSOR_EXECUTION_INPUT_REJECTED
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_FULL_BLOCK
PROCESSOR_INIT_FULL
PROCESSOR_INIT_PRESERVE_PINNED
PROCESSOR_SNAPSHOT_FORMAT_VERSION
SUPPORTED_MIR_SCHEMA_VERSION
compileTrustedMir
createProcessorArtifactFiles
loadProcessorArtifactFiles
parseProcessorMetadata
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
```

<!-- END WEB API onda_binaryen_web -->

</details>

<details markdown="1">
<summary>@onda-lang/wasm-compiler exports</summary>

<!-- BEGIN WEB API onda_wasm_compiler -->

```text
DirectCompilerOptions
MIR_SCHEMA_VERSION
ONDA_VERSION
OndaArtifactError
OndaBinaryenError
OndaBufferAssetBinding
OndaBufferData
OndaBufferElement
OndaCodegenOptions
OndaCompilationResult
OndaCompileConstDescriptor
OndaCompileConstElement
OndaCompileConstInspectionOptions
OndaCompileConstKind
OndaCompileConstValue
OndaCompileError
OndaCompileOptions
OndaCompilerDiagnostic
OndaCompilerError
OndaCompilerInstance
OndaLspAnalysisOptions
OndaLspMessage
OndaMaterializedProjectFile
OndaProcessorArtifact
OndaProcessorInitMode
OndaProcessorMetadata
OndaProjectBufferInfo
OndaProjectCapabilities
OndaProjectConstant
OndaProjectImageInfo
OndaProjectMaterialization
OndaResolvedCompileConstValue
OndaSerializedProjectImage
OndaSourceDocument
OndaSourceGraph
OndaSourceReferenceKind
OndaSourceResolution
OndaSourceWorkspace
OndaWorkerConstructor
OndaWorkerLike
PROCESSOR_ABI_VERSION
PROCESSOR_ARTIFACT_FORMAT
PROCESSOR_ARTIFACT_FORMAT_VERSION
PROCESSOR_BEGIN_BLOCK
PROCESSOR_END_BLOCK
PROCESSOR_EXECUTION_INPUT_REJECTED
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_FULL_BLOCK
PROCESSOR_INIT_FULL
PROCESSOR_INIT_PRESERVE_PINNED
PROCESSOR_SNAPSHOT_FORMAT_VERSION
WorkerCompilerOptions
createCompiler
createProcessorArtifactFiles
loadProcessorArtifactFiles
parseProcessorMetadata
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
```

<!-- END WEB API onda_wasm_compiler -->

</details>

<details markdown="1">
<summary>@onda-lang/webaudio exports</summary>

<!-- BEGIN WEB API onda_webaudio -->

```text
ONDA_AUDIO_WORKLET_PROCESSOR_NAME
ONDA_INIT_FULL
ONDA_INIT_PRESERVE_PINNED
ONDA_RUNTIME_ERROR
OndaAudioProcessor
OndaAudioProcessorOptions
OndaAudioPrintBatch
OndaAudioPrintListener
OndaExecutionError
OndaExecutionErrorListener
OndaRuntimeError
OndaInitMode
OndaParamDomain
OndaPreparedParamControl
OndaProcessorArtifact
OndaProcessorMetadata
PROCESSOR_EXECUTION_INPUT_REJECTED
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
compileOndaProcessorModule
constrainParamPlain
createOndaAudioProcessor
createOndaAudioProcessorInitialized
createParamControl
createParamDomain
flattenedAudioChannelCount
ondaAudioWorkletNodeOptions
paramNormalizedToPlain
paramPlainToNormalized
registerOndaAudioWorklet
```

<!-- END WEB API onda_webaudio -->

</details>
