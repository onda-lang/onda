# Generated plugin TODO

## Status

Generated plugins are deliberately deferred. The active plugin work is the JUCE-based dynamic VST3
project described in [`../onda-plugin.md`](../onda-plugin.md).

The likely generated-plugin implementation should not depend on JUCE. Generated products have a
frozen host surface and do not need the dynamic editor framework, so thin direct CLAP and VST3
wrappers may be a better fit.

## Intended user experience

The eventual command may be:

```text
onda plugin build path/to/onda-plugin.toml [--release]
```

It produces one plugin product with:

- permanent authored CLAP and VST3 identifiers;
- an exact main audio layout;
- an exact stable host parameter list;
- immutable product metadata;
- an embedded complete Onda source project;
- embedded build assets;
- enough Onda compiler and runtime code to specialize and execute that project for the host.

The first implementation should embed source text, not MIR or native object code. Onda's current
MIR is already specialized for sample rate and block size, and semantic analysis resolves builtin
constants using those values. Treating current MIR as a host-independent template would therefore
be incorrect.

MIR embedding can be reconsidered only after Onda has an explicit, validated late-specialization
boundary. It is an optimization, not a prerequisite for generated plugins.

## Why source is embedded

The host determines sample rate and maximum block size only after loading and configuring the
plugin. The generated bundle must compile an exact specialization for that pair.

Embedding the complete source project:

- preserves the existing language and compiler semantics;
- supports imports and embedded standard-library use without inventing a new intermediate artifact;
- lets the normal frontend validate host-dependent constants and shapes;
- avoids compatibility claims for an unspecialized MIR format that does not yet exist;
- keeps the source project as the portable correctness input.

The cost is that every generated plugin initially contains the frontend, semantic analysis, LLVM
code generator, and runtime. Binary size and first-use compilation latency should be measured
before designing an alternative artifact.

## Proposed architecture

The `onda` CLI owns manifest validation and build orchestration. Format wrappers should live in
small reusable implementation units rather than in CLI command code:

```text
onda CLI
  manifest + source-project loading
  validation specialization
  generated metadata/assets
  build orchestration

generated plugin support
  shared host-neutral adapter
  CLAP wrapper
  VST3 wrapper
  state codec
  embedded source/assets

Onda native core
  frontend + semantics
  LLVM specialization/JIT
  runtime + processor descriptor
```

CLAP and VST3 wrappers share the scheduler, parameter mapper, event bridge, resource binding,
specialization cache, state codec, and realtime ownership code. Only format entry points and host
translations differ.

The generated plugin artifact is a separate shared library/bundle produced by the CLI. It does not
execute inside the `onda` CLI process.

## Scan and initialization behavior

Plugin discovery must never invoke the Onda compiler.

At build time, the CLI compiles one validation specialization and freezes:

- product identity and metadata;
- audio and event capabilities;
- flattened parameter metadata and permanent IDs;
- buffer declarations;
- a public-interface fingerprint.

The wrappers answer discovery queries entirely from generated constants.

At host initialization:

1. Read the host sample rate and maximum block size.
2. Construct the complete specialization key.
3. Compile the embedded source project or load an exactly matching verified cache entry.
4. Confirm that the compiled descriptor has the frozen public-interface fingerprint.
5. Restore embedded resources and compatible state.
6. Create and prepare a complete runtime instance.
7. Activate it at a process boundary.

Until preparation succeeds, an instrument is silent and an effect is dry. LLVM work never occurs
on the audio thread or during plugin discovery.

## Manifest outline

The manifest should be a versioned TOML document:

```toml
schema_version = 1
source = "synth.onda"
kind = "instrument"

[plugin]
name = "My Onda Synth"
vendor = "Example Audio"
version = "1.0.0"
clap_id = "com.example.my-onda-synth"
vst3_class_id = "9f6e90c3-d7df-4e71-a3c6-3b86143ccf43"
midi = "basic"
activity = "keep_alive"

[parameters]
cutoff = "cutoff"
resonance = "reson"

[buffers.sample]
path = "assets/sample.wav"
```

Requirements:

- Product identifiers are authored once and never derived from source names.
- Every host parameter has an explicit permanent ID.
- Removed IDs are not silently reused.
- Product kind, audio layout, parameter list, MIDI capability, and metadata are immutable.
- Buffer bindings must resolve during the build.
- Sample rate and block size are forbidden in the manifest.
- Generated metadata is a projection of the canonical processor descriptor, not a second source
  parser.

## Direct format bindings

The direct wrappers should expose the smallest supported surface rather than grow into a general
plugin framework.

Shared version-one capabilities:

- one exact main input bus and one exact main output bus;
- f32 processing initially, with f64 considered after the base path is proven;
- stable scalar parameters;
- basic note input and sample-offset event scheduling;
- native host state callbacks;
- fixed zero latency;
- no editor initially;
- no sidechains, auxiliary buses, output events, or dynamic layouts.

CLAP is the simpler first direct binding because its ABI is C-based. VST3 requires a carefully
audited implementation of its component/controller model, object lifetimes, interfaces, and
platform entry points. Both wrappers require official validator coverage and representative-host
tests.

Low-level binding code must remain isolated from Onda runtime code. A format callback translates
host data into the shared adapter and returns the adapter's status; it does not own compilation,
resource decoding, or DSP state machinery.

## State

Generated plugin state contains:

- state schema and product identity;
- embedded-source fingerprint and public-interface fingerprint;
- compiler/codegen options;
- host parameter values;
- portable Onda state snapshot;
- any editor-authorized buffer overrides;
- version information needed to diagnose recompilation.

The installed bundle remains the source of immutable source code and build assets. Host state does
not replace the embedded program with arbitrary source.

Sample rate and maximum block size are not restored as preferences. Executable memory, ORC
handles, and native objects are never serialized.

## Required Onda work

This feature depends on:

- the thread-safe native program ownership, documented prepare/unchecked-process lifecycle, and
  plugin-safe failure work shared with the dynamic plugin;
- deterministic complete-project embedding;
- public-interface fingerprints;
- a bounded compiler cache suitable for several plugin instances;
- source-project compilation without filesystem dependence;
- target-specific bundle generation and toolchain discovery;
- validator and host test infrastructure.

## Questions to resolve before implementation

- Whether generated wrapper code lives in this repository or in a dedicated permissively licensed
  support repository consumed by `onda plugin build`.
- Whether the first implementation ships CLAP before VST3 or requires both for release.
- Which low-level VST3 binding is acceptable and what license it carries.
- How compiler/runtime symbols are hidden in bundles containing several Onda plugins.
- How large source-embedded plugins are on each platform and how long cold compilation takes.
- Whether an on-disk compiled-artifact cache can be authenticated and invalidated safely.
- Whether future host-independent MIR templates provide enough size and latency benefit to justify
  a new artifact contract.

## Future MIR optimization

Do not implement this as part of the source-embedded version.

A future MIR-based generated plugin would require:

- symbolic host sample-rate and maximum-block-size values;
- a deterministic specialization pass that replaces them;
- proof that type checking, constant evaluation, static shapes, oversampling, and lowering remain
  correct after late specialization;
- a versioned and validated template-MIR schema distinct from today's specialized MIR;
- descriptor equivalence checks before activation;
- fallbacks for language features that fundamentally require semantic re-analysis.

Only after those invariants exist could a generated bundle omit the frontend and embed template
MIR for faster, smaller host-time LLVM compilation.
