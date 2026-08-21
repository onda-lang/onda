---
title: Language guide
description: The complete guide to Onda syntax, semantics, processors, graphs, generics, and modules.
permalink: /docs/language/
section: reference
eyebrow: Language reference
---

# Onda Language Guide

This guide is the main reference for writing Onda programs. It is organized as
a learning path: start with a small patch, learn the top-level program shape,
then move through executable code, data, functions, structs, processors, graphs,
generics, and modules.

## Contents

1. [A First Patch](#1-a-first-patch)
2. [Source Files](#2-source-files)
3. [Top-Level Program Surface](#3-top-level-program-surface)
4. [Executable Sections](#4-executable-sections)
5. [Types and Values](#5-types-and-values)
6. [Statements and Expressions](#6-statements-and-expressions)
7. [Constants and Compile-Time Code](#7-constants-and-compile-time-code)
8. [Functions with `def`](#8-functions-with-def)
9. [Structs](#9-structs)
10. [Processors with `proc`](#10-processors-with-proc)
11. [Graphs](#11-graphs)
12. [Generics and Compile-Time Parameters](#12-generics-and-compile-time-parameters)
13. [Modules, Namespaces, and `use`](#13-modules-namespaces-and-use)
14. [Reference Notes](#14-reference-notes)

## 1. A First Patch

An Onda file describes an audio processor. This patch exposes one host
parameter, creates persistent state, computes one value per block, and writes
one output sample at a time.

```onda
params:
  freq = 440.0 {20.0, 20000.0}

init:
  phase = 0.0

block:
  incr = freq * TWO_PI / SR

  sample:
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
    out1 = sin(phase)
```

The main parts are:

| Part | Meaning |
| --- | --- |
| `params` | Host-visible control values. |
| `init` | Setup code and persistent state. |
| `block` | Code that runs once per host block. |
| `sample` | Code that runs once per sample. |
| `out1` | A numbered audio output. |

The rest of the language grows from this model. You describe a processor's
surface, write code at the rate where it should run, and use functions,
structs, processors, and graphs when the patch becomes reusable or larger.

## 2. Source Files

Onda supports indentation syntax and brace syntax. These two programs are
equivalent:

```onda
outs:
  out1

sample:
  out1 = 0.0
```

```onda
outs {
  out1
}

sample {
  out1 = 0.0
}
```

Basic source rules:

- Statements can be separated by newlines or `;`.
- Line comments start with `#`.
- Names are introduced before they are used.
- Top-level declarations are processed in lexical order.
- `import module/path` loads `module/path.onda`.
- `include "path.onda"` or `include "path.on"` inserts another file by quoted path.
- Native filesystem-backed entry, import, and include paths must not traverse symbolic links. The
  loader rejects the path and identifies the offending component; virtual sources and immutable
  project images are unaffected.

Top-level forms:

| Form | Purpose |
| --- | --- |
| `ins`, `inputs` | Host input ports. |
| `params`, `kins` | Host-visible parameters. |
| `outs`, `outputs` | Audio-rate output ports. |
| `kouts` | Block-rate control output ports. |
| `buffers` | External host-bound buffers. |
| `events`, `event` | Host-triggered event handlers. |
| `init` | Setup and persistent state. |
| `block` | Per-block code. |
| `sample` | Per-sample code. |
| `graph` | Declarative signal routing. |
| `const`, `const def` | Compile-time values and helpers. |
| `def` | Runtime helper functions. |
| `struct` | Nominal data types. |
| `proc`, `processor` | Reusable DSP processors. |
| `namespace` | Qualified declaration groups and integer templates. |
| `use`, `pub use` | Unqualified lookup imports and re-exports. |

## 3. Top-Level Program Surface

The program surface is the set of values the host can connect to the processor:
inputs, params, outputs, buffers, and events. This chapter covers the top-level
forms. Processor-specific versions of the same ideas are introduced later in
[Processors with `proc`](#10-processors-with-proc).

### Inputs

`ins` declares input ports. `inputs` is an alias.

```onda
ins:
  in1
  side: f64
  stereo: f32[2]
```

Shorthand forms:

```onda
ins 2

ins<f64>:
  left
  right
  meter: f32
```

Rules:

- Omitted input types default to `f32`, or to the section default in `ins<T>`.
- `ins N` expands to `in1..inN`.
- `N` can be a compile-time integer expression.
- If `inN` is used without an `ins` block, that input is implicitly created as `f32`.
- If a count and explicit list are both present, they must match exactly.
- Scalar inputs can have defaults and ranges: `freq = 440.0 {20.0, 20000.0}`.
- A single range value only specifies the max range: `freq = 440.0 {20000.0}`.
- Fixed-size array inputs can have defaults, and array literal defaults must match the declared length.

Explicitly declared homogeneous inputs can be indexed:

```onda
const N = 4

ins N
outs N

sample:
  for i in 0..N:
    outs[i] = ins[i] * 0.5
```

`ins[i]` is 0-based and runtime indices are clamped. Implicit inputs created by
using `in1`, `in2`, and so on cannot be dynamically indexed.

### Parameters

`params` declares host-visible control parameters.

```onda
params:
  gain = 1.0
  mode: i32 = 0
  spread: f32[2] = [0.25, 0.75]
  cutoff = 440.0 {20.0, 20000.0, log, "Hz"}
  mode: i32 = 4 {0, 10, step = 2}
```

At the top level only, `kins` is an alias for `params`.

```onda
kins:
  cutoff = 1200.0
  resonance = 0.5
```

Rules:

- Omitted param types without defaults become `f32`.
- Omitted param types with defaults infer from the default.
- `gain = 0.5` becomes `f32`; `mode = 0` becomes `i32`.
- Scalar params can have host-control domains. Array params cannot.
- `params N` expands to `param1..paramN`; top-level `kins N` expands to `kin1..kinN`.
- Top-level code may declare either `params` or `kins`, not both.
- Top-level `paramN` or `kinN` usage can implicitly create params up to that ordinal.
- Top-level params are readable in executable code but are not writable from top-level event handlers.

A parameter domain extends the existing range braces with `scale`, `curve`,
`unit`, and `step`. Positional fields remain ordered as
`min, max, scale, unit, step`; all fields may be named and named fields may
appear in any order. `curve` is named-only:

```onda
params:
  cutoff = 440.0 {20, 20000, log, "Hz"}
  resonance = 0.5 {0, 1, unit = "%"}
  envelope = 0.5 {0, 1, curve = -4}
  voices: i32 = 4 {min = 0, max = 16, step = 1}
  gain = 1.0 {max = 2, scale = linear}
```

Positional fields must precede named fields, fields cannot be repeated, and
`{max}` retains the existing maximum-only shorthand. `scale` defaults to
`linear`; the other optional fields default to absent.

`scale`, `curve`, `unit`, and `step` describe external control of explicit
top-level `params` (and their top-level `kins` alias). They are not available
on inputs or processor-local params, and do not change the Onda DSP
calculation:

- `linear` maps normalized `n` to `min + n * (max - min)`.
- `log` maps it in logarithmic space to
  `exp(log(min) + n * (log(max) - log(min)))` and requires a floating parameter
  with `0 < min < max`.
- `curve = c` applies SuperCollider-style `lincurve` curvature to the normalized
  value before linear range mapping. For negative `c`, this is
  `expm1(c * n) / expm1(c)`; positive curves use its mirrored form. Negative
  values bend toward `max`, positive values bend toward `min`, and values with
  `abs(c) < 0.001` are linear. Unlike `log`, curves support zero, negative, and
  zero-crossing ranges.
- `unit` is presentation metadata.
- `step` must be positive, must divide the range exactly, and requires the
  default to lie on the resulting grid. External plain and normalized writes
  are clamped and snapped to that grid.
- Ranged `i32` and `i64` params have an implicit step of `1`.
- An `i64` control domain and its range width must fit within
  `[-9007199254740991, 9007199254740991]`, the integer range represented
  exactly by the shared host-control APIs. Unranged `i64` params retain their
  full width through typed/raw parameter storage.
- Logarithmic stepped domains are not supported.
- `curve` may be combined with `step`, but not with `scale = log`.

The step count is the number of intervals from `min` to `max` and must fit the
host descriptor. Normalization, snapping, and units are host-boundary
semantics; Onda code reads the resulting plain parameter value.

The range itself is also a DSP boundary invariant. Generated code clamps each
used ranged top-level parameter once at the start of `init`, once at the start
of each event invocation, and once at the start of each logical process block.
Every read in that entry point uses the resulting typed value. A floating NaN
maps to the range minimum; infinities clamp to the corresponding endpoint.
This protects raw parameter storage writes independently of any host-control
conversion.

Explicitly declared homogeneous params can be indexed directly:

```onda
params 4

sample:
  out1 = params[0] + params[1]
```

`params`, `kins`, and dynamic param views are not first-class arrays. Use direct
`params[i]` or `kins[i]` access in block or sample code rather than assigning,
slicing, passing, returning, or storing the whole surface.

### Outputs

`outs` declares sample-rate audio outputs. `outputs` is an alias.

```onda
outs:
  out1
  stereo: f32[2]
```

`kouts` declares block-rate control outputs.

```onda
kouts:
  rms: f32
  peak: f32
```

Shorthand forms:

```onda
outs 2
kouts<f32> 4

outs<f64>:
  left
  right
```

Rules:

- Omitted output types default to `f32`, or to the section default.
- `outs N` expands to `out1..outN`; `kouts N` expands to `kout1..koutN`.
- Using `outN` without an `outs` block implicitly creates a sample-rate `f32` output.
- Using `koutN` without a `kouts` block implicitly creates a block-rate `f32` control output.
- Top-level `outs` and `kouts` names must be disjoint.
- Numbered `outN` names are audio outputs; use `koutN` for numbered control outputs.
- `outs[i] = expr` is valid in sample-rate code when explicit outputs form one scalar type.
- `kouts[i] = expr` is valid in block-rate code when explicit control outputs form one scalar type.
- Dynamic output indices are 0-based and clamped.

### External Buffers

`buffers` declares host-bound buffers. The section uses the same scalar and fixed-array type
spelling as the rest of Onda, but the brackets describe the channel layout of each buffer rather
than an Onda array value:

```onda
buffers:
  src: buffer<f32>
  bus: buffer<f32[2]>
  any_bus: buffer<f32[]>
```

The short form is canonical inside a `buffers` block:

```onda
buffers:
  mono: f32
  stereo: f32[2]
  dyn: f32[]
```

- `f32` is a mono buffer.
- `f32[2]` is a buffer with exactly two channels.
- `f32[]` is a buffer with any positive runtime channel count.

The explicit `buffer<...>` form is mainly useful where a buffer is itself a type, such as a
function parameter. `buffer<f32[]>` is a channel-count wildcard: it accepts mono and exact-channel
buffers. An exact type such as `buffer<f32[2]>` does not accept a dynamic-channel buffer.

Shorthand forms:

```onda
buffers 2

buffers<f32>:
  delay
  scratch

buffers:
  piano: f32 {88}
  stereo_layers: f32[2] {4}
  named_count: f32 {count = 8}
```

`{N}` declares a fixed collection of `N` independently bound buffers. `{count = N}` is an optional
named spelling of the same declaration. The count belongs to the resource declaration, not its
element type: `stereo_layers` is four buffers, each with two channels. It does not introduce a
general multidimensional-array type.

Buffer access and metadata:

```onda
buffers:
  src: f32
  bus: f32[2]
  piano: f32 {88}
  stereo_layers: f32[2] {4}

sample:
  mono0 = src[0]
  left0 = bus[0, 0]
  n = src.len()
  c = bus.chans()
  sr = src.samplerate()
  sample_count = piano.len()
  middle_c_frames = piano[39].len()
  middle_c0 = piano[39][0]
  right0 = stereo_layers[layer][1, 0]
```

The access forms are deliberately limited to one coordinate pair per selected buffer:

| Declaration | Sample access | Slice access |
| --- | --- | --- |
| `mono: f32` | `mono[frame]` | `mono[start:end]` |
| `stereo: f32[2]` | `stereo[channel, frame]` | `stereo[channel, start:end]` |
| `bank: f32 {N}` | `bank[slot][frame]` | `bank[slot][start:end]` |
| `layers: f32[2] {N}` | `layers[slot][channel, frame]` | `layers[slot][channel, start:end]` |

`bank[slot]` and `layers[slot]` select a first-class buffer and can be passed to a function or used
for metadata queries. A channel alone is not a first-class view; use the channel-and-frame or
channel-and-slice forms above. The flattened `layers[slot, channel, frame]` form is not supported.

A selected buffer can also be bound to an immutable, scoped reference alias:

```onda
buffers:
  layers: f32[] {4}

block:
  source = layers[0]
  frames = source.len()

  sample:
    left = source.readL(0, 0.0)
```

The selector is evaluated once when the alias is bound. The alias retains resource identity rather
than a sample-data pointer: host descriptor rebinding remains visible on subsequent processing
calls. Buffer-reference aliases can be read, written, queried, used as method receivers, sliced,
and passed to buffer parameters. They cannot be rebound, returned, stored in arrays or structs, or
created in `init`. An alias introduced inside a conditional or loop is local to that control-flow
scope.

`std/lookup` provides `readL`/`readC` for clamped linear/cubic interpolation and `readLW`/`readCW`
for their wrap-aware counterparts. The wrapping variants interpolate across the final-to-first
frame boundary and are intended for cyclic tables and loopers.

All source-level coordinates clamp independently. In `stereo[channel, frame]`, for example, the
channel clamps to the channel range and the frame clamps to the frame range before the address is
formed. Fixed buffer-collection selectors likewise clamp and select a descriptor in constant time.
The compiler removes that normalization when it can prove the complete coordinate range is valid.

Buffers also support the general [unchecked indexed access](#unchecked-indexed-access) operations.
They make every supplied buffer coordinate an explicit programmer responsibility:

```onda
sample:
  x = src.read_unsafe(frame)
  y = read_unsafe(bus, channel, frame)
  src.write_unsafe(frame, x)
  write_unsafe(bus, channel, frame, y)
```

Free-call and receiver syntax are equivalent. A fixed buffer collection can either be accessed as
one unchecked operation (`bank.read_unsafe(slot, frame)`) or selected compositionally
(`bank[slot].read_unsafe(frame)`). In the latter form, `bank[slot]` retains ordinary clamped/proven
selection and only the frame (and channel, for a multichannel buffer) is unchecked. A buffer
reference alias made from a collection element supports the same receiver methods.

Rules:

- `buffers N` expands to `buf1..bufN`.
- Explicit declarations and count shorthand cannot currently be mixed in one `buffers` block.
- `.len()` on a buffer collection returns its declared count. Select an element first to query its
  frame count: `bank[i].len()`.
- `.chans()` and `.samplerate()` apply to a selected buffer, not to the collection. Exact channel
  counts are compile-time constants in generated code; dynamic counts come from the bound instance.
- Runtime binding validates element type and channel constraints. Each fixed-array slot binds
  independently and may be omitted.
- Host metadata names physical collection slots `bank[0]`, `bank[1]`, and so on, while separate
  collection metadata preserves the logical `bank` name and its contiguous slot range.
- An unbound slot is a neutral one-frame buffer: reads return the element type's zero, writes are
  discarded, `.len()` is `1`, `.samplerate()` is the host sample rate, and `.chans()` is the exact
  declared channel count or `1` for a dynamic-channel declaration.
- Binding with a zero sample rate unbinds the buffer; the pointer and dimensions are ignored.
- Primitive buffer slices are supported with the same slice syntax as arrays.

### Top-Level Events

Top-level events are host-triggered handlers. They are useful for musical
gestures, one-shot control changes, and stateful commands.

```onda
init:
  freq_state = 440.0
  amp_state = 0.0
  gate = false

events:
  note_on(freq_hz = 440.0, amp = 1.0):
    freq_state = freq_hz
    amp_state = amp
    gate = true

  note_off():
    gate = false
```

Singular event sugar:

```onda
event bang():
  gate = true
```

This is equivalent to an `events:` block with one event. Singular `event ...`
declarations and an `events:` block can be mixed in the same owner.

Supported top-level event parameter types:

- Primitive scalars.
- Fixed-size primitive arrays: `T[N]`.
- Read-only primitive slices: `T[]`.

Rules:

- Event params without explicit types default to `f32`.
- Defaults work for scalar and fixed-size array params.
- Fixed-array and slice params are read-only in handlers.
- Top-level events run immediately on the audio thread.
- Handlers cannot write inputs, outputs, or top-level params.
- Top-level handlers may write only existing top-level state rooted in `init`.
- Unknown top-level event indices are ignored at runtime.
- A known top-level event with the wrong payload size is a runtime error.
- Top-level host events with slice params use payload layout `i32 len` followed by contiguous element bytes.

## 4. Executable Sections

Executable sections determine when code runs. Learn them in this order:
`init` creates state, `sample` produces or processes audio, and `block` wraps
sample code with per-block work.

### `init`

`init` runs when an instance is created and may be rerun explicitly by the
host. It creates persistent state and usually constructs structs and
processors. Ordinary host reset restores the post-init resettable-state
baseline rather than executing source `init` again.

```onda
init:
  phase = 0.0
  gain = 0.5
  taps: f32[8]
```

Typical uses:

- Create persistent scalar state.
- Create arrays and tuples.
- Construct structs.
- Construct proc instances.
- Perform one-time setup.

Host `init` preserves `{retain}` roots and task continuations while rerunning
ordinary declaration initializers and every explicit init statement. Host
`init_all` clears all state first. Either operation atomically captures its
successful result as the new reset baseline; a runtime failure changes neither
the live state nor the prior baseline. Instance creation is equivalent to
allocating zeroed storage, writing parameter defaults, running `init_all`, and
capturing that first baseline.

Section default scalar types are supported:

```onda
init<f64>:
  phase = 0.0
  last = 0.0
```

Rules:

- A fresh top-level scalar assignment in `init` introduces persistent owner state.
- Assigning to an already visible state symbol updates that state.
- `const` declarations are allowed inside `init`.
- Declaration order is lexical.
- A fresh assignment inside nested control flow in `init` is local to that flow, not persistent state.

A direct persistent binding can opt out of ordinary reset with `{retain}` or
the equivalent named form `{reset = retain}`:

```onda
init:
  prepared: f32[4096] {retain}
  generation: i32 = 0 {reset = retain}
  history: f32[128]
```

Ordinary reset preserves `prepared` and `generation` while restoring `history`.
Fresh construction always initializes every binding. Snapshots include both
policies and restore the captured values; `retain` affects reset, not snapshot
semantics. Integer-domain attributes compose with the named reset field, for
example `{MaxPartitions, wrap, reset = retain}`.

```onda
init:
  if ready:
    tmp = 1.0
  else:
    tmp = 2.0
  carried = tmp
```

Here `tmp` is local to the `init` flow, while `carried` becomes persistent state.

### `sample`

`sample` is the per-sample executable scope. It is the most direct way to write
audio-rate code.

```onda
sample:
  out1 = in1 * gain
```

Rules:

- Fresh assignments in `sample` create locals.
- `sample` does not introduce new persistent owner state.
- `return` is valid in `def` bodies, not in top-level `sample`.
- Input/output surfaces are available in `sample`.

#### Oversampled `sample`

Once a normal `sample` block is clear, you can oversample it with `sample N:`.

```onda
sample 4:
  out1 = tanh(in1 * 8.0)
```

Rules:

- Supported factors are `1`, `2`, `4`, `8`, `16`, `32`, `64`, `128`, `256`, and `512`.
- `sample:` is equivalent to `sample 1:`.
- The factor can be any compile-time integer expression resolving to a supported factor.
- Audio input reads are interpolated across oversample substeps.
- Params are control-rate boundaries and are held within the base sample.
- Outputs are filtered and decimated back to the base rate.
- `SR` inside oversampled code is the effective sample rate.
- `HOST_SR` and its aliases always mean the host sample rate.
- `BS` remains the logical host block size.

Generated signal code runs at the rate of the scope that evaluates it. For
example, a host-rate oscillator feeding an oversampled distortion proc is
evaluated once per host sample, then interpolated into the distortion proc. An
oscillator evaluated inside the oversampled scope runs at the oversampled rate.

### `block`

`block` runs once per host audio block. It is useful when a value should be
computed once per block rather than once per sample.

```onda
block:
  incr = freq * TWO_PI / SR

  sample:
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
    out1 = sin(phase)
```

You can think of a `block` with audio outputs as three regions:

1. Block-pre statements before the nested `sample`.
2. The nested per-sample `sample`.
3. Block-post statements after the nested `sample`.

Rules:

- With sample-rate outputs, a `block` section must include a nested `sample`.
- Top-level statements before nested `sample` are block-pre code.
- Statements after nested `sample` are block-post code.
- Fresh top-level assignments in block-pre introduce block-carried owner state visible to later `sample` and block-post code.
- Fresh top-level assignments in block-post are visible only after that point.
- Fresh nested assignments inside `if`, `for`, and `while` stay local.
- `block` and `sample` are mutually exclusive with `graph` in the same owner.

`kouts` programs and processors use `block` without a nested `sample`, because
control outputs are block-rate values.

## 5. Types and Values

Primitive types:

- `f32`
- `f64`
- `i32`
- `i64`
- `bool`

Compound types:

| Type | Example | Notes |
| --- | --- | --- |
| Fixed array | `f32[8]` | Length is compile-time. |
| Slice | `f32[]` | Read-only or writable view depending on source and call usage. |
| Tuple | `(f32, i32)` | Anonymous fixed-length heterogeneous value. |
| Buffer | `buffer<f32>`, `buffer<f32[2]>`, `buffer<f32[]>` | Host-bound external data. |
| Struct | `Voice`, `Box<f32>` | Nominal data type declared with `struct`. |
| Proc | `Gain`, `Sine<f64>` | Stateful processing unit declared with `proc`. |

### Numeric Literals and Casts

Numeric literals and pure numeric constant expressions begin without a source
machine width. During semantic analysis they retain the widest supported
literal representation until a concrete numeric context selects `f32`, `f64`,
`i32`, or `i64`.

A concrete context can come from an annotation, a function parameter or return
type, another concretely typed operand, an interface/state/array element type,
or generic specialization at a call site. Conversion happens once at that
boundary. Runtime arithmetic then executes at the selected width; Onda does not
silently evaluate an `f32` expression through `f64` intermediates.

When no context exists, first assignment uses Onda defaults:

```onda
sample:
  x = 0.5  # f32
  n = 5    # i32 when it fits, otherwise i64
  m = -5   # i32 when it fits, otherwise i64
```

Unary minus preserves the selected numeric type: it works for `f32`, `f64`,
`i32`, and `i64`, including generic code specialized to those types. It is not
defined for `bool`.

Pure numeric expressions adapt directly to their surrounding context:

```onda
sample:
  narrow: f32 = 0.0
  wide: f64 = 0.0

  a = narrow + 0.1  # f32 addition
  b = wide + 0.1    # f64 addition
  c = 0.1           # no context, so f32
```

Builtin constants such as `TWO_PI` have an `f64` standalone type, but a pure
compile-time expression such as `freq * TWO_PI / SR` can convert directly into
an `f32` context. This does not create an `f64` runtime calculation followed by
an `f32` truncation.

Use an explicit annotation or cast when wider runtime evaluation is intended:

```onda
sample:
  wide = f64(narrow) * 0.1
  count = i64(0)
```

### Builtin Constants and Functions

Builtin constants:

| Constant family | Names | Type |
| --- | --- | --- |
| Pi | `PI`, `pi` | `f64` |
| Two pi | `TWO_PI`, `TWOPI`, `two_pi`, `twopi` | `f64` |
| Effective sample rate | `SAMPLE_RATE`, `SAMPLERATE`, `SR`, `sample_rate`, `samplerate` | `f32` |
| Host sample rate | `HOST_SR`, `HOST_SAMPLE_RATE`, `HOST_SAMPLERATE`, `host_sample_rate`, `host_samplerate` | `f32` |
| Block size | `BLOCK_SIZE`, `BLOCKSIZE`, `BS`, `block_size`, `blocksize` | `i32` |

Builtin functions include:

```text
sin cos tan tanh atan atan2 exp log sqrt pow abs fabs
floor ceil round trunc min max fma
```

### Arrays and Slices

Fixed-size arrays can be state or locals:

```onda
init:
  taps: f32[8]

sample:
  coeffs = [0.5, 0.25, 0.125]
  out1 = coeffs[0]
```

An untyped array assignment takes its element type from the first element
using the ordinary first-assignment defaults, then checks every remaining
element against that type. An array literal used directly as a call argument
can instead acquire its element type from the parameter context.

Primitive array and buffer slices use Python-style syntax:

```onda
sample:
  a = buf[:]
  b = buf[2:]
  c = buf[:-1]
  d = buf[1:-2]
```

Rules:

- Slice forms are `a[:]`, `a[start:]`, `a[:end]`, and `a[start:end]`.
- Negative bounds are supported.
- Slice expressions lower to primitive slice views of type `T[]`.
- Buffer slicing also yields `T[]`.
- Struct-element arrays are not sliceable in the current implementation.

Writable slice assignment is statement-only:

```onda
sample:
  values[1:-1] = 0.5
  dst[:] = src[:]
```

Scalar fill writes the full target slice. Slice copy writes
`min(dst_len, src_len)` elements. Overlapping slice copies behave as if copied
through a temporary. Event payload arrays and slices are read-only.

#### Unchecked Indexed Access

`read_unsafe(values, index)` / `values.read_unsafe(index)` and
`write_unsafe(values, index, value)` / `values.write_unsafe(index, value)` provide unchecked scalar
access across the language; they are not limited to external buffers. Supported primitive storage
includes fixed arrays, slices, named input/parameter/output arrays, the uniform dynamic views `ins`,
`params`/`kins`, `outs`, and `kouts`, selected buffers, and fixed buffer collections. Ordinary
direction rules still apply: inputs and parameters are read-only, while output views are write-only.
Buffer forms accept their normal selector, channel, and frame coordinates.

Unchecked coordinates perform no clamp or runtime bounds check. The caller must prove every supplied
index, collection selector, channel, and frame is valid. Violating that contract is memory-unsafe.
These operations are an optimization escape hatch for a bound the compiler cannot express or prove,
not a way to change ordinary clamped indexing semantics. Prefer normal indexing—especially with
ranged integer selectors—when the compiler can establish the bound itself.

`read_unsafe` also selects references from arrays of structs and processors:

```onda
sample:
  cell = cells.read_unsafe(index)
  voice = read_unsafe(voices, index)
  out1 = inspect(cell) + voice()
```

The result has the same alias/reference semantics as `cells[index]` or `voices[index]`; only selector
normalization is omitted. Struct fields and processor calls, fields, events, and named arguments
therefore continue to operate on the selected element. An aggregate result is only valid when
introducing an alias or passing a reference argument; it cannot be used as a scalar value.
Aggregate assignment is not a language operation, so `write_unsafe` remains limited to primitive
storage. An invalid unchecked
aggregate selector has undefined memory-unsafe behavior and may trap, crash, corrupt state, or—for
a processor array—dispatch through an arbitrary state. There is no defined fallback behavior.

### Tuples

Tuples are anonymous fixed-length heterogeneous values.

```onda
sample:
  pair = (1.0, 2.0)
  mixed = (1.0, 42, true)
  out1 = pair[0]
```

Rules:

- Type syntax is `(T1, T2, ...)`.
- Maximum arity is 16.
- Nested tuples are not currently supported.
- Tuple element access uses compile-time integer indices.
- Tuple destructuring is supported: `(a, b) = (10.0, 20.0)`.
- Tuples can be locals, `init` state, `def` params and returns, and struct fields.

## 6. Statements and Expressions

### Assignment and Declarations

First assignment infers a type:

```onda
sample:
  x = 0
  y = 0.0
```

Explicit declarations pin the type:

```onda
sample:
  x: i64 = 0
```

Assigning to an existing visible symbol updates it. Assigning to a new symbol
introduces a symbol according to the storage rules of the current scope.

Integer locals and state may carry a finite storage domain:

```onda
const RingSize = 1024

init:
  bank = 0 {8}
  taps: i32 = 0 {range = 0..128}
  cursor: i32 = 0 {RingSize, wrap}
  samples_seen: i64 = 0 {range = 0..=3999999999, mode = clamp}
```

Domains are supported for both `i32` and `i64`, and every count or range endpoint must be an exact
compile-time integer expression. A single expression is a zero-based count: `{1000}` and
`{count = 1000}` both admit `0..1000`, while `{1000, wrap}` uses the same count with wrapping
normalization. Counts must be positive.

An explicit range uses the same endpoint syntax as `for`: `{begin..end}` is half-open and
`{begin..=end}` is inclusive. The named forms are `{range = begin..end}` and
`{range = begin..=end}`. Half-open ranges require `begin < end`; inclusive ranges require
`begin <= end`. `count` and `range` are mutually exclusive, and a positional count or range must
precede named fields and mode. Integer binding domains do not use comma-separated endpoints.
`{min, max}` instead denotes an inclusive parameter domain inside top-level or processor
`params`. Top-level parameter domains also support `step`, `scale`, and presentation metadata
fields.

`clamp` is the default mode. `wrap` performs modular normalization across the same finite domain.
Bare `clamp`/`wrap` and the explicit `mode = clamp`/`mode = wrap` spellings are equivalent. An
omitted binding type defaults to `i32`, so both `bank = 0 {8}` and `bank = selected {8}` are `i32`
and follow the regular integer assignment rules. Use an explicit `i64` annotation for an `i64`
ranged binding. General numeric clamping remains the job of `clamp(value, lower, upper)`; binding
domains are integer storage invariants intended primarily for indices and wrapping cursors.

Initialization and every later direct or compound assignment normalize once as the value is stored:

```onda
cursor += 1       # wraps 1023 to 0
taps = requested  # clamps to 0..127
```

Reading a ranged binding produces an ordinary `i32` or `i64`; arithmetic does not inherit its
storage mode. The compiler retains the numeric invariant separately and uses it to remove index
normalization and bounds checks when the complete derived range is known to fit a statically sized
collection. This applies to fixed arrays and to selectors for fixed buffer collections, among other
fixed-size indexed storage:

```onda
const TapCount = 8
const VoiceCount = 4

buffers:
  voices: f32 {VoiceCount}

init:
  taps: f32[TapCount]
  tap = 0 {TapCount, wrap}
  voice = 0 {VoiceCount}

sample:
  out1 = taps[tap] + voices[voice][0]
```

Both accesses keep ordinary clamped source semantics, but their selector normalization can disappear
from generated code because the ranged bindings prove the selectors valid. Dynamic buffer frame
counts and dynamic slice lengths generally still require ordinary runtime normalization unless the
compiler can establish their bounds by other means. The physical representation and snapshot layout
of a ranged binding remain the underlying integer type.

### Operators

Supported operators:

| Category | Operators |
| --- | --- |
| Arithmetic | `+`, `-`, `*`, `/`, `%` |
| Comparisons | `==`, `!=`, `<`, `<=`, `>`, `>=` |
| Logical | `!`, `&&`, `||` |
| Bitwise integer | `~`, `&`, `|`, `^`, `<<`, `>>` |

Bitwise operators accept `i32` and `i64`. Mixed `i32` and `i64` operands widen
to `i64`. `>>` is an arithmetic right shift.

Expression precedence, from highest to lowest, is:

1. Grouping, calls, indexing, and slicing
2. Prefix `-`, `!`, `~`
3. `*`, `/`, `%`
4. `+`, `-`
5. `<<`, `>>`
6. `==`, `!=`, `<`, `<=`, `>`, `>=`
7. `&`
8. `^`
9. `|`
10. `&&`
11. `||`

Infix operators at the same tier associate from left to right. Parentheses override this order.

### Control Flow

Supported forms:

```onda
if x > 0.0:
  y = x
elif x < 0.0:
  y = -x
else:
  y = 0.0

for i in 0..8:
  sum = sum + taps[i]

for i in 0..=8:
  sum = sum + f32(i)

for i @ -1 in 10..0:
  dst[i] = src[i]

loop 8:
  sum = sum + taps[_]

while sum < 1.0:
  sum = sum + 0.1
```

Rules:

- `for i in A..B` excludes `B`; `for i in A..=B` includes `B`.
- `@ STEP` defaults to `1`; `@ 0` is invalid.
- Descending loops use a negative step.
- `loop N` is shorthand for `for _ in 0..N`.
- Loop variables are immutable values local to the loop body. Runtime loop
  variables are `i32`; assign a new local when an iteration-derived value
  needs to be changed.
- Fresh symbols created inside loops do not escape the loop.
- A fresh symbol created in every continuing branch of an `if` is available
  afterward. Numeric scalar and tuple-element types join to the smallest type
  that accepts every branch without narrowing (for example, `i32` with `i64`
  becomes `i64`, while `f32` with `i64` becomes `f64`).
- Branch-local arrays must have the same element type and fixed length.
  Supported aggregate aliases must likewise have one compatible runtime
  shape; a branch-dependent shape is a semantic error at the `if`.
- `break` and `continue` are supported in loops.
- `return` is valid in `def` bodies, not in top-level `sample`.

## 7. Constants and Compile-Time Code

Use `const` for compile-time values:

```onda
const MaxVoices = 8
const Hop: i32 = BLOCK_SIZE / 2
const Scale: f32[3] = [0.5, 1.0, 2.0]
const MoreScale: f32[] = [0.25, 0.5, 1.0, 2.0]
```

Rules:

- `const NAME = expr` and `const NAME: T = expr` are supported.
- `expr` must be compile-time evaluable.
- Primitive const arrays are supported at top level and namespace scope.
- `const NAME: T[N] = [ ... ]` declares a fixed-size const array.
- `const NAME: T[] = expr` infers the concrete array length from the initializer.
- Inferred-length const array initializers can be literals, existing const arrays, const-array slices, or array-returning `const def` calls.
- Untyped scalar const declarations remain contextual compile-time numerics and preserve the
  widest supported literal representation until each use site selects a concrete scalar type.
- A typed const fixes its scalar type at the declaration. An untyped pure numeric const may
  specialize directly to `f32` in one context and `f64` in another.
- Once a numeric expression is concretely typed, every runtime operation uses that width and
  observes that type's normal rounding semantics. Use an explicit cast to request wider evaluation.
- Reassignment, forward references, recursion, and mutual recursion are rejected.

`const def` declares compile-time helper functions:

```onda
const def ramp() -> f32[4]:
  values: f32[4]
  for i in 0..4:
    values[i] = f32(i) * 0.25
  return values

const Ramp: f32[4] = ramp()
```

`const def` rules:

- Every `const def` must declare an explicit return type.
- Params support primitive scalars, fixed-size primitive arrays, typed primitive slices such as `f32[]`, and untyped slices `[]`.
- Typed slice params accept compile-time arrays of any length with the matching element type.
- Untyped slice params accept compile-time arrays of any length and primitive element type.
- Slice params support indexed reads and `.len()`, but not indexed writes.
- Array-returning bodies can use local fixed primitive arrays, indexed local-array reads/writes, `if`, `for`, `loop`, `return`, pure builtin math, and calls to earlier visible const defs.
- Scalar-returning const defs can be used by scalar const declarations.
- Fixed-array-returning const defs can be used by const array declarations.

Const arrays and const slices can be passed to ordinary runtime `def` array
params when the callee treats the param as read-only. Writes through the param,
aliases or forwarding to a mutable callee make the param
mutable and reject const-array arguments.

## 8. Functions with `def`

`def` declares reusable runtime functions.

```onda
def wrap_phase(p, upper = TWO_PI):
  if p > upper:
    return p - upper
  return p
```

Supported features:

- Positional arguments.
- Named arguments.
- Default values.
- Early return.
- Optional explicit return type annotations with `->`.
- Multi-line argument lists with an optional trailing comma.
- Method-style sugar for ordinary defs: `x.clamp01()` rewrites to `clamp01(x)`.

Examples:

```onda
def wrap_phase(p, upper = TWO_PI) -> f32:
  if p > upper:
    return p - upper
  return p

def pair(x: f32, y: i32) -> (f32, i32):
  return (x, y)
```

Return rules:

- A `def` can return a primitive scalar.
- A `def` can return a tuple of primitive scalars.
- A runtime `def` with no explicit return type and no `return EXPR` is
  non-value-returning. It may use bare `return` for early exit and can only be
  called as a statement.
- Bare and value returns cannot be mixed. A `def` with an explicit return type
  rejects bare `return`.
- A value-returning `def` must return a value on every reachable path. A
  return nested only in a `for` or `while` loop is not sufficient because the
  loop may execute zero times.
- Explicit annotations can use primitive scalars, tuples of primitive scalars, and generic primitive placeholders belonging to the current generic owner.
- Returning structs, arrays, or buffers is not supported.
- Return checking follows ordinary assignment rules: exact match and implicit widening are allowed; narrowing requires an explicit cast.
- Runtime def call graphs must be acyclic. Direct and mutual recursion are
  rejected because they do not provide a statically bounded realtime workload.
- `const def` remains value-returning and does not accept bare `return`.

Top-level `def` bodies are lexical-local. Top-level runtime symbols such as
inputs, outputs, params, buffers, and `init` state are not in scope unless
passed explicitly.

### Generic Defs

Runtime defs can declare type parameters:

```onda
def id<T>(x: T) -> T:
  return x

def pair<T>(x: T, y: i32) -> (T, i32):
  return (x, y)
```

The compiler monomorphizes generic defs from their call sites. Type arguments
can often be inferred:

```onda
sample:
  a = id(0.5)      # T inferred as f32
  b = id<f64>(1.0) # T provided explicitly
```

Rules:

- Generic def type args are restricted to `f32`, `f64`, `i32`, and `i64`.
- `bool` is not allowed as a generic def type arg.
- Generic type params can appear in scalar params, array params, buffer element params, locals, casts, and supported return annotations.
- `const def` cannot declare type parameters.

### Parameter Kinds

`def` params support:

- Primitive scalars.
- Untyped scalars inferred and specialized from each call site.
- Explicit struct types.
- Typed arrays such as `arr: f32[]`.
- Untyped arrays such as `arr: []`.
- Typed buffers such as `buf: buffer<f32>`.
- Bare buffers such as `buf: buffer`.
- Generic struct and proc parameters specialized at the call site.
- Typed tuple params such as `p: (f32, i32)`.
- Untyped tuple params inferred from the call site.
- Untyped structural params inferred from field and method use.

```onda
def sum(arr: f32[]):
  total = 0.0
  for i in 0..arr.len():
    total = total + arr[i]
  return total

def first(arr: []):
  return arr[0]

def read_first(buf: buffer):
  return buf[0]
```

Untyped parameters can be specialized structurally:

```onda
struct A:
  x: f32

struct B:
  x: f32

def read_x(s):
  return s.x
```

`read_x` can be called with both `A` and `B`; the compiler specializes it from
the concrete argument shape and the field access in the body.

### Overloads

Top-level defs and struct methods can be overloaded by arity and parameter types.

```onda
def sat(x: f32):
  return x

def sat(x: f64):
  return f32(x)
```

Resolution rules:

- Exact typed match wins first.
- If no exact typed match exists, numeric widening candidates may be used.
- Explicit typed params outrank generic or duck-typed params.
- Generic or duck-typed params outrank untyped params.
- Default arguments participate in overload matching.
- Return type is not part of overload selection.
- Equally valid candidates are a semantic error.

Proc-local defs are not overloadable. Runtime defs may still be generic with
syntax such as `def id<T>(x: T) -> T`; those generic defs are specialized from
their call sites.

## 9. Structs

`struct` declares nominal data types with fields and methods.

```onda
struct Voice:
  phase: f32
  sig: f32

  def tick(self, hz):
    self.phase = self.phase + hz * TWO_PI / SR
    self.sig = sin(self.phase)
```

Supported features:

- Field defaults.
- Methods.
- Overloaded methods.
- Tuple fields.
- Generic structs.

Construction:

```onda
init:
  a = Voice()
  b: Voice = Voice()
  c: Voice
```

Rules:

- `self` must be the first method parameter.
- Methods can read and write struct fields through `self`.
- Typed struct declarations are `init`-only.
- Declaration-only form such as `c: Voice` desugars to default-constructor initialization.
- For generic structs, typed declarations require explicit type args when the type is still generic.

### Indexed Struct-Array Field Access

For arrays of data structs, one inline field-access dot is supported:

```onda
sample:
  gain = voices[i].level
  tap = voices[i].taps[j]
```

Accepted forms:

- `base[idx].field`
- `base[idx].field[fidx]`

Deeper inline chains are rejected:

- `base[idx].field.other`
- `base[idx].field[fidx].other`

Use an intermediate alias for deeper access:

```onda
sample:
  v = voices[i]
  gain = v.settings.level
```

Proc arrays use their own indexed forms such as `voices[i].gain`,
`voices[i](...)`, and `voices[i].note_on(...)`.

## 10. Processors with `proc`

`proc` is Onda's reusable stateful processing unit. `processor` is an alias.
Everything in this chapter builds on the top-level sections introduced earlier,
but scoped to a reusable child processor.

```onda
proc Gain:
  ins:
    in1

  params:
    g = 1.0

  outs:
    out1

  sample:
    out1 = in1 * g
```

A proc can contain:

- `ins`
- `params`
- `events` and `event`
- `buffers`
- `outs` or `kouts`
- `init`
- `block`
- `sample`
- `graph`
- Proc-local `def` helpers

In practice, `init` and `events` are optional, and a proc normally has one
execution body: `sample`, `block`, or `graph`.

### Proc Inputs, Params, Outputs, and Buffers

Proc sections use the same surface syntax as the top level, with these
differences:

- `kins` is not valid inside a proc; proc parameter sections are always `params`.
- A processor declares either `outs` or `kouts`, not both.
- `kouts` processors use `block` with no nested `sample`, cannot declare `ins`, and cannot declare `graph`.
- Proc constructor arguments for params and buffers are named-only.
- Proc inputs are bound by positional proc call args or named input args.
- A scalar proc buffer accepts one buffer or a selected collection slot, such as
  `clip = bank[2]`.
- A fixed proc buffer collection requires the same count. A larger collection can be passed only
  through an exact, compile-time subspan such as `clips = bank[1:7]`; both bounds are checked at
  compile time, and the descriptors are forwarded without copying their sample data.

```onda
proc StereoGain:
  ins:
    input: f32[2]

  params:
    gain: f32[2] = [1.0, 1.0]

  outs:
    out: f32[2]

  sample:
    out[0] = input[0] * gain[0]
    out[1] = input[1] * gain[1]
```

### Constructing and Calling Procs

Proc instances are usually created in `init`:

```onda
init:
  g = Gain(g = 0.5)
```

Call and access forms:

```onda
sample:
  y = g(in1)          # single-output scalar sugar
  z = g(in1).out1
  g.g = 0.25
  out1 = g.out1
```

Rules:

- Positional proc call args bind inputs only.
- Named call args can bind inputs or params.
- Named param args store the clamped param value before the call runs.
- Every ranged proc-param write, including construction, builtin `init(...)`,
  named call arguments, and direct assignment, is clamped once before storage.
  Floating NaN maps to the range minimum; later reads use the stored typed
  value without reclamping.
- Generic procs specialize on construction.
- Multiple proc calls in one expression are evaluated in source order.
- Named param args are not supported inside logical `&&` / `||` expressions or `while` conditions.
- For `kouts` procs, use `kout1` or named control outputs.

### Pinned Params

Use `pin` when a proc param should be initialized and updated only through that
proc's controlled code path.

```onda
proc Filter:
  params:
    pin cutoff = 1000.0
    pin q = 0.707
```

Pinned params:

- Can be set by the constructor.
- Can be set by the builtin proc `init(...)` event.
- Can be read or assigned by the owning proc's own `init`, `sample`, `block`, `event`, or proc-local `def` bodies.
- Cannot be accessed directly from outside through `child.cutoff`, `child.cutoff = ...`, `child.coeffs[i]`, `child.coeffs[i] = ...`, or `child(cutoff = ...)`.
- Cause external dynamic `child.params[i]` access to be rejected for that child proc.

`pin` is a reserved keyword. It is only valid as a proc-param prefix.

### Param Update Hooks

A primitive scalar proc param can bind a proc-local update hook with
`=> hook_name`.

```onda
proc Voice:
  params:
    freq = 440.0 {20.0, 20000.0} => update_freq

  init:
    phase_inc = 0.0

  def update_freq():
    phase_inc = freq / SR

  sample:
    out1 = 0.0
```

Hook rules:

- The hook target must be a zero-parameter proc-local `def` in the same proc.
- The hook must have no explicit return type and no `return`.
- Hooks run after the param store and range clamp.
- Construction and builtin `init(...)` run hooks after the proc `init` body, in param declaration order.
- Hooks are immediate per-param reactions; they are not batched.
- Hooks may read owner params, update init-rooted state, and assign named params on child procs.
- Hooks cannot assign owner params, inputs, outputs, child proc I/O or internal state, child dynamic `params[i]`, or call child events.
- If a proc has bound params, dynamic `params[i] = ...` assignments are rejected; assign the named param instead.

Use hooks for single-param derived state. Use an explicit proc event or setter
when several params should rebuild shared state once.

### Proc Events

Proc events are receiver-only commands called on a proc instance. They are
useful for reset, note, trigger, and setter style APIs.

```onda
proc Env:
  params:
    amp = 0.0

  event note_on(v: f32):
    amp = v

  sample:
    out1 = amp
```

Proc-event rules:

- Calls use receiver syntax such as `voice.note_on(...)`.
- Proc-event calls are statements, not expressions.
- Unqualified calls never resolve to proc events.
- A proc cannot call its own event handler as an internal subroutine; put shared logic in a proc-local `def`.
- Proc handlers may write proc state rooted in `init` and proc params.
- Proc handlers cannot write inputs or outputs.
- Generic proc events can use generic primitive placeholders such as `T`, `T[N]`, and `T[]`.

Every proc also gets a reserved builtin `init(...)` event. It mirrors the proc
params in declaration order and adds `all: bool = false`, assigns
provided values into params, reruns that proc instance's `init`, then runs bound
param hooks. Omitted args use defaults.

By default the call preserves `{retain}` roots and compiler-owned retained
state such as task continuations while reinitializing resettable roots. Passing
`all = true` performs the full initialization used by fresh proc
construction. Explicit operations in the initializer still run in either
mode, so a `task.reset()` written there remains effective.

```onda
voice.init(0.5)
voice.init(gain = 0.5)
voices[i].init(freq = 220.0, amp = 0.1)
voice.init(all = true)
```

`all` is reserved as a proc parameter name.

### Tasks

The top-level program and procs can declare statically allocated cooperative
tasks. The standalone and grouped forms are equivalent:

```onda
proc Loader:
  task prepare():
    build_header()
    yield
    build_body()

  tasks:
    clear():
      clear_cache()
```

The same declarations work at top level:

```onda
task prepare():
  build_header()
  yield
  build_body()

block:
  await prepare()
  sample:
    out1 = render()
```

Tasks take no arguments and return no values. They implicitly see their
owner's params, buffers, and init-rooted state, but cannot directly read audio
inputs, write owner outputs, call processor steps, invoke their owner's event
handlers, or invoke other tasks. They may synchronously call child-proc events,
including the child's builtin `init(...)` event.
`yield` suspends the current task; bare `return` or reaching the end completes
it.

The owner advances a task with `await` from block-pre control flow:

```onda
block:
  await prepare()

  sample:
    out1 = process(in1)
```

If the task yields or has failed, the owner stops that activation and produces
neutral outputs. For a top-level task those are the program outputs; for a proc
task only that proc becomes neutral and its parent continues normally. If the
task completes, execution continues after `await` in the same logical block. A
task is reset explicitly with `prepare.reset()` from the owner's `init`, event,
or block-pre scope.

Task continuations are retained state. Ordinary reset and default initialization
preserve them. Proc `init(all = true)` and the host-level all-state initialization
operations restore them to not-started. An explicit `prepare.reset()` in an
initializer always runs. Tasks may use both retained and resettable state; after
an ordinary reset, a suspended task observes the reset values when it resumes.

Tasks are private to their owner and share that owner's declaration namespace.
They cannot be used with a `graph` block. `await` is valid only in structured
block-pre control flow; task reset is valid only in owner `init`, event, and
block-pre code. Neither operation is a first-class callable value.

### Proc-Local Defs

Processors can declare private helper defs that implicitly see proc state.

```onda
proc Filter<T>:
  ins<T> 1
  outs<T> 1

  init:
    state: T = 0.0
    coeff: T = 0.5

  def reset_state():
    state = T(0.0)

  def apply(x: T):
    state = state + (x - state) * coeff
    return state

  event reset():
    reset_state()

  sample:
    out1 = apply(in1)
```

Rules:

- Proc-local defs are private to the enclosing proc.
- They can be called from proc `init`, `block`, `sample`, `events`, and other proc-local defs.
- They can read and write proc state directly, without `self`.
- They support params, defaults, named args, and returns like normal defs.
- Recursive and mutually recursive proc-local defs are rejected.
- Proc-local defs are not overloadable.

### Proc Arrays

Arrays of proc instances are supported in `init`.

```onda
init:
  voices: Voice[4] = Voice()
```

Supported forms:

- Literal array construction: `voices: Voice[2] = [Voice(), Voice()]`.
- Broadcast constructor sugar: `voices: Voice[4] = Voice()`.
- Compile-time capacity expressions in the array length.

Indexed proc-array operations:

```onda
sample:
  voices[i](freq)
  out1 = voices[i].out1
  voices[i].gain = 0.5
  voices[i].note_on(220.0)
```

Rules:

- Runtime indices are clamped to the valid slot range.
- Aliasing such as `v = voices[i]`, then `v(...)`, is supported.
- Proc-array buffer refs resolve through the current validated buffer tables.
- A proc cannot directly instantiate its own type in its own state.

If the proc defines a `block` section, indexed proc-array calls use active-slot
block-hook semantics: block-pre runs lazily on the first `()` call to that slot
in the current block, and block-post runs once at block end for each called slot.
Plain slot retrieval does not trigger hooks.

## 11. Graphs

`graph` gives you a declarative way to wire processor instances and signal flow.

```onda
proc GainProc:
  params:
    gain = 1.0

  sample:
    out1 = in1 * gain

init:
  p = GainProc()

graph:
  in1 >> p.in1
  3.0 >> p.gain
  p.out1 >> out1
```

```onda
import std/osc

params:
  freq = 220.0 {20.0, 20000.0}
  mod = 100.0 {0.0, 1000.0}

init:
  sine = std::osc::Sine()

graph:
  @sample freq + sine.out1 * mod >> sine.freq
  sine.out1 >> out1
```

Supported edge forms:

```onda
src >> dst
dst << src
@block src >> dst
@sample src >> dst
src >>[expr] dst
src >> { a, b }
{ a, b } << src
```

Rules:

- `graph` is mutually exclusive with `sample` and `block` in the same owner.
- `init` may be used with `graph`.
- Proc instances used as graph nodes are typically created in `init`.
- Unannotated edges targeting proc params default to `@block`.
- Unannotated edges targeting other destinations default to `@sample`.
- `@sample` can override the default `@block` behavior for proc param destinations.
- Delayed edges use `>>[expr]` or `<<[expr]`.
- Delay expressions must be compile-time nonnegative integers.
- Delayed edges are sample-rate only.
- Each destination has one writer.
- Fan-out is allowed.
- Cycles are rejected unless a positive sample delay breaks the cycle.
- Proc nodes are stepped implicitly according to graph reachability and topological order.
- Inspect lowering with `onda compile <file> --dump-graph`.

Current graph sources include:

- Top-level inputs and params.
- Proc outputs.
- Proc-array slot outputs.
- Array literals such as `[a, b]`.
- Indexed reads, sliced reads, and whole-array reads.
- Arithmetic and logical expressions built from supported graph sources.
- Element-wise array expressions when the final shape matches the destination.

Current legal destinations include:

- Top-level outputs.
- Proc inputs.
- Proc params.
- Proc-array slot inputs and params.

Type and scheduling rules:

- Graph edges use strict shape matching.
- Scalar-to-fixed-array broadcast is allowed.
- Proc inputs, params, and outputs are legal graph endpoints.
- Bare proc instances and proc-array slots can route into destination sets.
- Destination sets zip by output order when counts match.
- Single-output procs broadcast to destination sets.
- Otherwise, mismatched bundles are semantic errors.

Current graph limits:

- User-defined function calls and proc calls are not supported inside graph source expressions.
- Array-constructor expressions such as `f32[2](...)` are not graph sources.
- Graph event propagation syntax does not exist; use ordinary `events` or `event` declarations.

Example with proc bundles:

```onda
graph:
  reverb >> { out1, out2 }
  voices[0] >> { left, right }
```

## 12. Generics and Compile-Time Parameters

Onda has two complementary compile-time generic mechanisms:

- Type generics on `struct` and `proc`, such as `T`.
- Integer namespace params, such as `N`, for counts, sizes, and arity.

### Type Generics

Generic structs and procs are monomorphized. The compiler creates a concrete
specialized copy for each type combination your program uses.

```onda
struct Pair<T>:
  a: T
  b: T

proc OnePole<T>:
  ins<T> 1
  outs<T> 1

  init:
    state: T = 0.0

  sample:
    state = state + (in1 - state) * 0.1
    out1 = state
```

Specialization:

```onda
init:
  a = Pair<f32>()
  b = Pair<f64>()
  lp = OnePole()       # unresolved constructor type params default to f32
  hp = OnePole<f64>()
```

Rules:

- Generic type args are restricted to `f32`, `f64`, `i32`, and `i64`.
- `bool` is not allowed as a generic type arg.
- Unresolved generic type params in declaration and type positions are errors.
- For untyped constructor assignments only, unresolved constructor type params default to `f32`.
- `T(expr)` rewrites to the bound primitive cast.
- `T[]` is valid for method and `def` array params where a primitive slice is valid.
- Typed generic locals such as `x: T = ...` are supported in executable scopes.

### Def Monomorphization

Runtime defs are specialized from call sites. This applies both to explicit
generic defs such as `def id<T>(x: T) -> T` and to polymorphic parameter shapes
that do not need a named type parameter:

- Untyped scalar params such as `value`, specialized to the concrete primitive
  type at each call site (including `bool`). Pure numeric expressions use the
  ordinary untyped `f32`/`i32` defaults, while explicit casts preserve their
  requested numeric type.
- Untyped arrays such as `arr: []`.
- Bare buffers such as `buf: buffer`.
- Generic struct and proc params supplied by concrete arguments.
- Unsized processor-array params such as `voices: Voice[]`; specialization
  records the concrete capacity supplied at each call site. A fixed
  `voices: Voice[N]` parameter already has a complete source-level ABI.
- Untyped tuple params inferred from tuple literals.
- Untyped structural params inferred from field or method usage.

```onda
def first(arr: []):
  return arr[0]

def id<T>(x: T) -> T:
  return x

sample:
  a = [1.0, 2.0]
  b = [1, 2]
  x = first(a)
  y = first(b)
  z = id<f64>(1.0)
```

### Integer Namespace Params

Namespace template params are compile-time integers.

```onda
namespace DSP<Channels = 2>:
  proc Gain<T>:
    ins<T> Channels
    outs<T> Channels

    params:
      gains: T[Channels]

    sample:
      for i in 0..Channels:
        outs[i] = ins[i] * gains[i]
```

Use namespace integer params in:

- Fixed array sizes such as `T[N]`.
- Section counts such as `ins N`, `outs N`, `params N`, and `buffers N`.
- Loop bounds.
- Compile-time expressions and namespace `assert(...)` checks.

Instantiate inline or through aliases:

```onda
namespace Stereo = DSP<2>

init:
  g = Stereo::Gain<f32>(gains = [0.5, 0.25])
```

## 13. Modules, Namespaces, and `use`

### Imports

Use `import` to load another module:

```onda
import reverb
import std/osc
import std/filter
```

Rules:

- `import module/path` resolves as `module/path.onda`.
- Each imported file is imported once.
- Built-in std modules are available under `std/...`.
- Imported files are declaration-only: `const`, `struct`, `def`, `proc`, `namespace`, and `use`.
- `std/prelude` is auto-imported during semantic analysis.

Current std modules include:

```text
std/prelude std/math std/random std/complex
std/osc std/filter std/env std/delay std/reverb std/pitch_shift std/data std/lookup
std/fft std/convolution std/gain std/levels std/mix
std/noise std/pitch std/smoothing
```

`std/prelude` currently imports `std/math`, `std/lookup`, and `std/random`.

### Includes

`include` inserts another source file by quoted path:

```onda
include "shared/reverb.onda"
include "shared/util.on"
```

Rules:

- The path must be quoted.
- The path must end in `.onda` or `.on`.
- Use `/` path separators.

### Namespaces

`namespace` groups declarations under a qualified path.

```onda
namespace my::dsp:
  def sat(x):
    return clamp(x, -1.0, 1.0)

sample:
  out1 = my::dsp::sat(in1)
```

Namespace-local consts and nested namespaces are supported:

```onda
namespace Config:
  const MaxVoices = 8
```

Templated namespaces take compile-time integer params with defaults:

```onda
namespace FFT<N = 256>:
  assert(N > 0)
  assert((N & (N - 1)) == 0)
```

Rules:

- Namespace template params require defaults.
- Args support positional and named forms.
- Args are normalized as `i32(...)` at compile time.
- Namespace-local `assert(expr)` performs compile-time checks.
- `<>` is used for namespace instantiation, generic specialization, and section default type modifiers.
- `[]` is used for arrays, indexing, slices, and buffer/channel forms.

Namespace aliases:

```onda
namespace D = std::data<SR, 1>

init:
  a = std::data<SR, 1>::Data<f64>()
  b = D::Data<f64>()
```

### Use Declarations

`use` brings namespace members into unqualified lookup. It does not load
modules; use `import` first when the target lives in another module.

```onda
import std/math
import std/random
import std/fft

use std::math
use std::random::Rng
use std::fft<512> as fft512
use std::fft<1024>::FFT as FFT1024
pub use std::lookup

init:
  rng = Rng<f32>()
  a = fft512::FFT<f32>()
  b = FFT1024<f32>()

sample:
  out1 = clamp(in1, -1.0, 1.0)
```

Forms:

- `use Namespace` brings direct declarations in that namespace into unqualified lookup.
- `use Namespace::Symbol` brings one declaration into unqualified lookup.
- `use Namespace as Alias` creates a namespace alias.
- `use Namespace::Symbol as Alias` creates a symbol alias.
- `pub use ...` re-exports the use declaration through imports.

Rules:

- `as` applies only to the whole `use` declaration.
- `use` is allowed at top level and inside `namespace`.
- Plain top-level `use` is private to the source file where it appears.
- Imported files expose only `pub use` declarations to the importing file.
- Fully qualified paths always work.
- Explicit `use` collisions are errors at unqualified use sites; qualify the name to disambiguate.

## 14. Reference Notes

### Section Shorthands

| Section | Count shorthand | Default type shorthand | Generated names |
| --- | --- | --- | --- |
| `ins` | `ins N` | `ins<f64>:` | `in1..inN` |
| `params` | `params N` | `params<i32>:` | `param1..paramN` |
| `kins` | `kins N` | `kins<i32>:` | `kin1..kinN` |
| `outs` | `outs N` | `outs<f64>:` | `out1..outN` |
| `kouts` | `kouts N` | `kouts<f32>:` | `kout1..koutN` |
| `buffers` | `buffers N` | `buffers<f32>:` | `buf1..bufN` |

Section counts can use compile-time integer expressions, ordinary const values,
and namespace integer template params.

### Dynamic Surfaces

Direct indexed access is supported for explicitly declared homogeneous surfaces:

```onda
ins[i]
outs[i] = x
kouts[i] = x
params[i]
kins[i]
child.params[i]
```

These views are not first-class arrays. Do not assign, slice, pass, return, or
store `ins`, `outs`, `kouts`, `params`, `kins`, or child proc dynamic views.

Input/output surfaces are block/sample-bound. `inN`, `outN`, `koutN`, declared
I/O arrays, and synthetic I/O views cannot be read, written, passed, returned,
or stored from `init`, `event`, or top-level `def` bodies.

### Aliases and Reserved Names

- `inputs` aliases `ins`.
- `outputs` aliases `outs`.
- `processor` aliases `proc`.
- Top-level `kins` aliases `params`.
- Control-flow keywords are reserved: `if`, `elif`, `else`, `for`, `in`, `while`, `loop`, `break`, `continue`, `return`, and `assert`.
- `in` separates the loop variable from its range in `for i in A..B`; use names such as `input` for ports and variables.
- `import`, `include`, `use`, `as`, `pub`, and `pin` are reserved for their declaration and modifier syntax.
- `true` and `false` are reserved boolean literals.
- Identifiers beginning with `__onda_` are reserved for compiler-generated symbols.
- Numbered `outN` names are audio outputs; use `koutN` for numbered control outputs.

### Common Current Limits

- Proc-local defs are not overloadable.
- Returning structs, arrays, or buffers from runtime `def` is unsupported.
- Struct-element arrays are not sliceable.
- `graph` source expressions cannot call user-defined functions or procs.
- `graph` has no event-routing syntax.
