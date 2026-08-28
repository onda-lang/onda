---
title: Web API reference
description: Compile, validate, load, and host Onda processors with the public JavaScript packages.
permalink: /docs/web-api/
section: reference
eyebrow: JavaScript and WebAssembly
---

# Web API reference

Onda publishes four ECMAScript-module packages for browsers and Node.js:

| Package | Purpose |
| --- | --- |
| `@onda-lang/wasm-compiler` | Compile Onda source, workspaces, and project images to complete WebAssembly processor artifacts. |
| `@onda-lang/processor-abi` | Validate artifacts, inspect metadata, prepare parameter controls, and decode raw delegate and print output. |
| `@onda-lang/binaryen-web` | Lower trusted, version-matched Onda MIR directly to a processor artifact. |
| `@onda-lang/webaudio` | Host a complete processor artifact in an `AudioWorklet`. |

All packages are ESM-only. `@onda-lang/wasm-compiler` and `@onda-lang/binaryen-web` support modern
browsers and Node.js 20 or newer. Web Audio construction requires a browser environment with
`AudioWorklet`; the metadata and parameter helpers re-exported by `@onda-lang/webaudio` are ordinary
synchronous JavaScript.

Each published package contains this reference as `api.md`. TypeScript declarations are included
at every exported module path.

## Compile source and start audio

```js
import { createCompiler } from "@onda-lang/wasm-compiler";
import {
  createOndaAudioProcessorInitialized,
} from "@onda-lang/webaudio";

const compiler = await createCompiler();
const { artifact } = await compiler.compileSource(source, {
  sampleRate: audioContext.sampleRate,
  blockSize: 128,
});

const print = ({ text }) => console.debug(text);
const processor = await createOndaAudioProcessorInitialized(
  audioContext,
  artifact,
  {
    params: { gain: 0.5 },
    // A construction-time listener also receives prints emitted by init.
    onPrint: print,
  },
);
processor.node.connect(audioContext.destination);

const stopDelegates = processor.onDelegates(({ occurrences }) => {
  for (const occurrence of occurrences) {
    console.log(occurrence.name, occurrence.values);
  }
});
```

Compilation is an offline operation and may allocate. Constructing the adapter compiles or accepts
a reusable `WebAssembly.Module` before the audio node starts. Normal rendering uses preallocated
storage. Delegate and print callbacks run on the main side after bounded worklet transport; they do
not call JavaScript from generated DSP execution.

## `@onda-lang/wasm-compiler`

### `createCompiler(options?)`

Creates an `OndaCompilerInstance`. With no options, compilation runs directly on the calling
JavaScript thread. Pass `{ worker: true }` to run the frontend and backend in a module worker;
`workerUrl`, `frontendWasm`, and a custom `Worker` constructor are optional integration hooks.

The returned instance provides:

- `compileSource(source, options?)` and `inspectSourceConstants(source, options?)`.
- `compileWorkspace(workspace, options?)` and `inspectWorkspaceConstants(workspace, options?)`.
- `compileProjectImage(bytes, options?)` and `inspectProjectImageConstants(bytes, options?)`.
- `createProjectImage(sourceGraph, buffers?)`, `inspectProjectImage(bytes)`,
  `loadProjectFiles(files, projectFilePath?)`, and `materializeProjectImage(bytes, names?)`.
- `encodeBufferAsset(binding)`, `decodeBufferAsset(bytes)`, and `decodeBufferFile(bytes, path?)`.
- `projectCapabilities()` for supported image, buffer, and standard-library versions.
- `sendLspMessage(message)` and `setLspAnalysisOptions(options?)` for the embedded language server.
- `dispose()`, an idempotent terminal release of compiler and worker resources.

Compile options accept `sampleRate`, `blockSize`, typed compile-constant overrides, and `codegen`.
Code generation can select optimization level `0..4`, shrink level `0..2`, strict or fast math,
SIMD, loop-containing inlining, and optional WAT emission. A successful compilation returns an
`OndaCompilationResult` containing the artifact, resolved source paths, and the exact source graph
when one is available.

Configuration and lifecycle failures throw `OndaCompilerError`. Authored-source, project, MIR, and
code-generation failures throw `OndaCompileError`, whose `diagnostics`, `sourceFiles`, and
`unresolvedSourceFiles` fields support editors and file watchers. `OndaBinaryenError` identifies a
failure in the MIR-to-Wasm backend.

`MIR_SCHEMA_VERSION` is the schema accepted by the bundled backend. `ONDA_VERSION` is the compiler
version. `createDefaultImports()` returns the imports required by current generated modules; it is
currently an empty object because artifacts are self-contained.

The `@onda-lang/wasm-compiler/artifact` subpath re-exports the complete
`@onda-lang/processor-abi` surface without loading the compiler. The
`@onda-lang/wasm-compiler/worker` subpath is a side-effect-only module-worker entry and has no
named exports.

## `@onda-lang/processor-abi`

An `OndaProcessorArtifact` contains WebAssembly bytes, an `OndaProcessorMetadata` descriptor, and
optional WAT. Treat the bytes and descriptor as one integrity-associated pair.

### Validation and files

- `validateProcessorMetadata(metadata, expectedKind?)` validates and returns the current descriptor.
- `validateProcessorArtifact(artifact, options?)` normalizes bytes and optionally inspects the module.
- `validateProcessorModule(module, metadata)` verifies a precompiled module against its descriptor.
- `parseProcessorMetadata(input, expectedKind?)` parses JSON or validates an object.
- `serializeProcessorMetadata(metadata, space?)` validates before producing newline-terminated JSON.
- `createProcessorArtifactFiles(artifact, options?)` creates associated `.wasm` and `.onda.json`
  records, including integrity metadata.
- `loadProcessorArtifactFiles(wasm, metadata)` validates a loaded pair and its integrity association.

Failures throw `OndaArtifactError`. Hosts must reject unsupported artifact-format, ABI, and snapshot
versions rather than guessing layout compatibility.

### Parameter controls

`createParamControl(metadata)` validates one scalar parameter descriptor and returns a reusable
`OndaPreparedParamControl`. `createParamDomain(domain)` does the same from already-decoded values.
Prepared controls expose `constrainPlain`, `normalizedToPlain`, and `plainToNormalized` methods.

The one-shot `constrainParamPlain`, `paramNormalizedToPlain`, and `paramPlainToNormalized`
functions provide the same behavior. Preparing once is preferable for frequently updated UI:
validation and decimal descriptor decoding stay off the interaction path. Mappings preserve exact
endpoints, apply linear, logarithmic, or curved scale, and clamp and snap to the declared grid.

### Delegates and print output

Complete wasm32 processor exports receive an optional execution-output descriptor containing
independently nullable delegate and print batches. Allocate all descriptors and storage before
real-time execution.

`writeDelegateBatch` and `writePrintBatch` initialize reusable batch descriptors.
`writeExecutionOutput` connects their addresses. After a successful generated call,
`readDelegateBatch` or `readPrintBatch` validates the result counters.
`decodeDelegateRecords` converts delegate payloads using `metadata.delegates`.
`decodePrintRecords` preserves primitive types and source sites; `formatPrintRecords` and
`formatPrintBatch` add canonical text formatting.

Batch capacity is host policy. `overflowCount` reports whole records that did not fit; it is not a
byte count. Consume the returned storage before reusing it for another init, process, or event call.
See the [language guide](https://onda-lang.org/docs/language/#delegates) for authored semantics and
the [processor API](https://onda-lang.org/docs/processor-api/#call-scoped-execution-output) for
physical record layout.

The metadata interfaces expose target facts, storage sizes, ports, parameters, states, events,
delegates, print sites, buffers, and integration profiles. Scalar `i64` values decoded from records
use `bigint`; booleans use `boolean`; fixed arrays and slices become JavaScript arrays.

## `@onda-lang/binaryen-web`

### `compileTrustedMir(mir, options?)`

Lowers MIR emitted by the matching Onda semantic frontend. Input may be compact MessagePack, JSON,
or an already-decoded object. The function is synchronous and returns an
`OndaProcessorArtifact`. It is intentionally not a validator for untrusted or hand-authored MIR;
the producer owns semantic, type, bounds, and resource proofs.

`OndaBinaryenOptions` controls the same backend policies exposed through compiler `codegen`.
`SUPPORTED_MIR_SCHEMA_VERSION` identifies the required producer schema. `createDefaultImports()`
returns an empty object for current self-contained artifacts. Backend failures throw
`OndaBinaryenError`.

The package root re-exports the common artifact validation and file helpers. The
`@onda-lang/binaryen-web/artifact` subpath re-exports the complete
`@onda-lang/processor-abi` module without loading Binaryen.

## `@onda-lang/webaudio`

### Construction

- `registerOndaAudioWorklet(context, workletUrl?)` registers the processor once per context.
- `compileOndaProcessorModule(artifact)` validates and compiles a reusable module off the render thread.
- `ondaAudioWorkletNodeOptions(artifact, options?)` builds low-level node options.
- `createOndaAudioProcessor(context, artifact, options?)` allocates an uninitialized adapter.
- `createOndaAudioProcessorInitialized(context, artifact, options?)` allocates and fully initializes it.
- `flattenedAudioChannelCount(ports?)` totals declared physical channels with validation.

`OndaAudioProcessorOptions` accepts initial plain parameter values, external buffers, event,
delegate, and print capacities, an optional construction-time `onPrint` listener, a precompiled
module, custom node options, and an `AudioWorkletNode` constructor. Pass `onPrint` when using the
initialized constructor if initialization output must be observed; registering a listener after
construction cannot replay output from an execution that has already completed. The artifact sample
rate must equal the context sample rate and it must expose at least one audio input or output.

### `OndaAudioProcessor`

The adapter exposes its `node` and validated `metadata`, plus these operations:

- `setParam(nameOrIndex, plain)` and `setParamNormalized(nameOrIndex, normalized)`.
- `trigger(nameOrIndex, values?)` for input events.
- `onDelegates(listener)` and `onPrint(listener)`, each returning an unsubscribe function.
- `init(mode)`, `snapshot()`, and `restoreSnapshot(bytes)`.
- `readControlOutputs()` and `readBuffer(nameOrIndex)`.
- `request(type, fields?, transfer?)` for adapter protocol extensions.
- `close(reason?)`, an idempotent terminal release of adapter-side resources.

Delegate and print batches report `overflowCount` for generated-storage loss and
`transportDropCount` for bounded worklet-to-main queue loss. Loss-only notifications can arrive
without occurrences. Collection is enabled only while the corresponding listener set is nonempty;
setting the capacity to zero disables host delivery even with listeners while preserving language
evaluation semantics.

`ONDA_INIT_FULL` clears and initializes all physical state. `ONDA_INIT_PRESERVE_PINNED` retains
pinned state according to the processor ABI. The package also re-exports the prepared and one-shot
parameter conversion helpers.

`@onda-lang/webaudio/worklet` is the side-effect-only AudioWorklet registration module and has no
named exports.

## Complete exported surface

The following indexes include runtime values and TypeScript-only types from each package root.
They are checked against the shipped declarations so additions cannot silently escape this
reference. Artifact subpaths are complete processor-ABI re-exports; worker and worklet subpaths are
side-effect-only as described above.

### `@onda-lang/processor-abi`

<!-- BEGIN WEB API onda_processor_abi -->
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
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_INIT_FULL
PROCESSOR_INIT_PRESERVE_PINNED
PROCESSOR_SNAPSHOT_FORMAT_VERSION
constrainParamPlain
createParamControl
createParamDomain
createProcessorArtifactFiles
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
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
writeDelegateBatch
writeExecutionOutput
writePrintBatch
<!-- END WEB API onda_processor_abi -->

### `@onda-lang/binaryen-web`

<!-- BEGIN WEB API onda_binaryen_web -->
OndaArtifactError
OndaBinaryenError
OndaBinaryenOptions
OndaProcessorArtifact
OndaProcessorMetadata
PROCESSOR_ABI_VERSION
PROCESSOR_ARTIFACT_FORMAT
PROCESSOR_ARTIFACT_FORMAT_VERSION
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_SNAPSHOT_FORMAT_VERSION
SUPPORTED_MIR_SCHEMA_VERSION
compileTrustedMir
createDefaultImports
createProcessorArtifactFiles
loadProcessorArtifactFiles
parseProcessorMetadata
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
<!-- END WEB API onda_binaryen_web -->

### `@onda-lang/wasm-compiler`

<!-- BEGIN WEB API onda_wasm_compiler -->
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
OndaProcessorMetadata
OndaProjectBufferInfo
OndaProjectCapabilities
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
PROCESSOR_EXECUTION_OK
PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE
PROCESSOR_SNAPSHOT_FORMAT_VERSION
WorkerCompilerOptions
createCompiler
createDefaultImports
createProcessorArtifactFiles
loadProcessorArtifactFiles
parseProcessorMetadata
serializeProcessorMetadata
validateProcessorArtifact
validateProcessorMetadata
validateProcessorModule
<!-- END WEB API onda_wasm_compiler -->

### `@onda-lang/webaudio`

<!-- BEGIN WEB API onda_webaudio -->
ONDA_AUDIO_WORKLET_PROCESSOR_NAME
ONDA_INIT_FULL
ONDA_INIT_PRESERVE_PINNED
OndaAudioProcessor
OndaAudioProcessorOptions
OndaAudioPrintBatch
OndaAudioPrintListener
OndaInitMode
OndaParamDomain
OndaPreparedParamControl
OndaProcessorArtifact
OndaProcessorMetadata
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
<!-- END WEB API onda_webaudio -->
