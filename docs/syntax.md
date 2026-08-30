---
title: Language guide
description: The complete guide to Onda syntax, semantics, processors, graphs, generics, and modules.
permalink: /docs/language/
section: reference
eyebrow: Language reference
---

# Onda Language Guide

This guide is both a learning path and the complete language reference. It
starts with Onda's execution and data-flow model, then introduces ordinary
runtime code before moving into external resources, reusable processors,
messages, graphs, compile-time programming, and modules.

## Contents

1. [How Onda Runs](#1-how-onda-runs)
2. [Source Files](#2-source-files)
3. [Execution and State](#3-execution-and-state)
4. [Values and Runtime Code](#4-values-and-runtime-code)
5. [Audio and Control Interfaces](#5-audio-and-control-interfaces)
6. [Collections](#6-collections)
7. [Functions with `def`](#7-functions-with-def)
8. [Structs](#8-structs)
9. [External Buffers](#9-external-buffers)
10. [Processors](#10-processors)
11. [Events, Printing, and Delegates](#11-events-printing-and-delegates)
12. [Tasks](#12-tasks)
13. [Graphs](#13-graphs)
14. [Compile-Time Programming and Generics](#14-compile-time-programming-and-generics)
15. [Modules, Namespaces, and `use`](#15-modules-namespaces-and-use)
16. [Reference Notes](#16-reference-notes)

## 1. How Onda Runs

An Onda file describes an audio processor. At its simplest, samples flow from
host inputs, through a `sample` body, to host outputs:

```onda
ins:
  input

params:
  gain = 0.5 {0.0, 1.0}

outs:
  output

sample:
  output = input * gain
```

For every host sample, Onda reads `input`, multiplies it by the current
host-visible `gain`, and writes one sample to `output`.

The host creates and configures an instance before audio processing begins.
Onda then follows this execution model:

```text
create and configure the instance
              |
             init
              |
     each logical audio block
      +-------------------+
      | block-pre         |
      | sample x BS       |
      | block-post        |
      +-------------------+
              |
       state is retained
```

A program can use a top-level `sample` directly, as the gain processor does,
or place `sample` inside `block` when it has work that should happen only
once per logical block.

Persistent state is normally introduced in `init` and survives across samples
and blocks. This
oscillator initializes one phase value, updates it for each sample, and sends
the result to an explicitly declared output:

```onda
params:
  freq = 440.0 {20.0, 20000.0}

outs 1

init:
  phase = 0.0

sample:
  out1 = sin(phase)
  phase = phase + freq * TWO_PI / SR
  if phase >= TWO_PI:
    phase = phase - TWO_PI
```

`SR` is the effective sample rate and `TWO_PI` is the circle constant. The
phase created by `init` is retained after each `sample` invocation. The sample
is calculated from the current phase before that state advances, so the first
sample is `sin(0.0)` and the updated phase belongs to the next sample. Advancing
first would produce the same oscillator shifted forward by one sample.

The increment depends only on a control value and the sample rate, so it can be
computed once per block:

```onda
params:
  freq = 440.0 {20.0, 20000.0}

outs 1

init:
  phase = 0.0

block:
  increment = freq * TWO_PI / SR

  sample:
    out1 = sin(phase)
    phase = phase + increment
    if phase >= TWO_PI:
      phase = phase - TWO_PI
```

Here `increment` is refreshed in block-pre and is then available to every
sample in that block. The rest of the language grows from this model: declare
the processor's interface, create persistent state, and place work at the rate
where it belongs.

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
- Newlines are allowed inside parenthesized, bracketed, and angle-bracketed lists. Trailing commas
  are not accepted.
- Names are introduced before they are used.
- Top-level declarations are processed in lexical order.
- A declaration-only source file is valid and does not need an executable `sample`, `block`, or
  `graph` section. This is the normal shape of an imported library module.
- `import module/path` loads `module/path.onda`.
- `include "path.onda"` or `include "path.on"` inserts another file by quoted path.
- Native filesystem-backed entry, import, and include paths must not traverse symbolic links. The
  loader rejects the path and identifies the offending component; virtual sources and immutable
  project images are unaffected.

The exhaustive list of top-level forms is in [Reference Notes](#top-level-forms).
Imports, includes, namespaces, and lookup rules are covered after the core
runtime language in [Modules, Namespaces, and `use`](#15-modules-namespaces-and-use).

## 3. Execution and State

Executable sections determine when code runs:

1. `init` creates persistent state.
2. `sample` produces or processes one audio sample.
3. `block` surrounds the per-sample loop with work performed once per logical
   block.

Where a name is first bound determines its storage lifetime:

| First binding | Lifetime |
| --- | --- |
| Directly in `init` | Persistent owner state. |
| In block-pre | Block-carried owner state, refreshed when block-pre runs and visible to the nested `sample` and block-post. |
| In `sample` | A local for the current sample invocation. |
| Inside nested `if`, `for`, or `while` flow | A local scoped to that flow. |

### `init`

`init` runs when an instance is initialized. It creates persistent state and
usually constructs structs and processors.

```onda
init:
  phase = 0.0
  gain = 0.5
  taps: f32[8]
```

Typical uses include:

- Creating persistent scalar and aggregate state.
- Constructing structs and proc instances.
- Performing one-time setup.
- Reading, writing, or inspecting buffers that are bound for initialization.

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

A fresh binding directly inside `init` becomes persistent. A fresh binding
inside nested control flow is local to that flow:

```onda
params:
  use_high_value: bool = false

init:
  if use_high_value:
    temporary = 2.0
  else:
    temporary = 1.0

  carried = temporary
```

Both branches create the local `temporary`, so it is available after the
conditional. `carried`, introduced directly in `init`, becomes persistent
state. Reinitialization modes and pinned state are covered in
[Reinitialization and Pinned State](#reinitialization-and-pinned-state).

### `sample`

`sample` is the per-sample executable scope. It is the most direct way to write
audio-rate code.

```onda
ins:
  input

params:
  gain = 0.5

outs:
  output

sample:
  output = input * gain
```

For every host sample, Onda reads `input`, multiplies it by the current control
value, and writes one sample to `output`.

Rules:

- Fresh assignments in `sample` create locals.
- `sample` does not introduce new persistent owner state.
- `return` is valid in `def` bodies, not in top-level `sample`.
- Input/output surfaces are available in `sample`.

#### Oversampled `sample`

Once a normal `sample` block is clear, you can oversample it with `sample N:`.

```onda
ins 1
outs 1

params:
  drive = 8.0

sample 4:
  out1 = tanh(in1 * drive)
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

`block` runs once per logical audio block. It is useful when a value should be
computed once per block rather than once per sample.

```onda
params:
  freq = 440.0 {20.0, 20000.0}

outs 1

init:
  phase = 0.0

block:
  increment = freq * TWO_PI / SR

  sample:
    out1 = sin(phase)
    phase = phase + increment
    if phase >= TWO_PI:
      phase = phase - TWO_PI
```

You can think of a `block` with audio outputs as three regions:

1. Block-pre statements before the nested `sample`.
2. The nested per-sample `sample`.
3. Block-post statements after the nested `sample`.

Rules:

- With sample-rate outputs, a `block` section must include a nested `sample`.
- Top-level statements before nested `sample` are block-pre code.
- Statements after nested `sample` are block-post code.
- Params and buffers are available throughout `block`; sample-rate inputs and audio-output writes
  are available only inside its nested `sample`.
- `kouts` are written in block-pre or block-post, never in the nested `sample`.
- Fresh top-level assignments in block-pre introduce block-carried owner state visible to later `sample` and block-post code.
- Fresh top-level assignments in block-post are visible only after that point.
- Fresh nested assignments inside `if`, `for`, and `while` stay local.
- `block` and `sample` are mutually exclusive with `graph` in the same owner.

`kouts` programs and processors use `block` without a nested `sample`, because
control outputs are block-rate values.

## 4. Values and Runtime Code

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
| Struct | `Voice` | Nominal data type declared with `struct`. |
| Proc | `Gain` | Stateful processing unit declared with `proc`. |

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
  narrow: f32 = 0.5
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

Compound assignment reads an existing value, applies an operator, and stores
the result back into the same binding:

```onda
init:
  phase = 0.0
  remaining = 8
  gain = 1.0
  total = 4.0
  index = 9

sample:
  phase += 0.01
  remaining -= 1
  gain *= 0.5
  total /= 2.0
  index %= 8
  out1 = phase
```

On the first sample these statements store `0.01`, `7`, `0.5`, `2.0`, and `1`
respectively.

Arithmetic compound operators are `+=`, `-=`, `*=`, `/=`, and `%=`. Integer
bindings additionally support `&=`, `|=`, `^=`, `<<=`, and `>>=`. Compound
assignment currently works only with a variable or field path. Indexed and
slice targets require an ordinary assignment such as
`values[i] = values[i] + amount`.

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
taps = 200        # clamps to 127
```

Reading a ranged binding produces an ordinary `i32` or `i64`; arithmetic does not inherit its
storage mode. The compiler retains the numeric invariant separately and uses it to remove index
normalization and bounds checks when the complete derived range is known to fit a statically sized
collection. This applies to fixed arrays and other fixed-size indexed storage:

```onda
const TapCount = 8

init:
  taps: f32[TapCount]
  tap = 0 {TapCount, wrap}

sample:
  out1 = taps[tap]
  tap += 1
```

The access keeps ordinary clamped source semantics, but its selector normalization can disappear
from generated code because the ranged binding proves the selector valid. Dynamic lengths generally
still require ordinary runtime normalization unless the compiler can establish their bounds by
other means. The physical representation and snapshot layout
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
init:
  taps = [0.1, 0.2, 0.3, 0.4]

sample:
  x = in1

  if x > 0.0:
    magnitude = x
  elif x < 0.0:
    magnitude = -x
  else:
    magnitude = 0.0

  sum = 0.0
  for i in 0..4:
    sum = sum + taps[i]

  loop 4:
    sum = sum + 0.1

  while sum > 1.0:
    sum = sum - 1.0

  out1 = magnitude * sum
```

Explicit induction widths and descending steps are available when needed:

```onda
def sum_large_range() -> i64:
  total: i64 = 0
  for i: i64 in (i64(2147483648))..(i64(2147483650)):
    total = total + i
  return total

def descending_sum() -> i32:
  total = 0
  for i: i32 @ -1 in 10..0:
    total = total + i
  return total
```

Rules:

- `for i in A..B` excludes `B`; `for i in A..=B` includes `B`.
- Range bounds and `loop` counts accept ordinary expressions, including calls
  such as `values.len()`; parentheses are optional.
- Loop variables default to `i32`; annotate them as `i32` or `i64` with
  `for i: TYPE in ...` when an explicit induction width is required.
- `@ STEP` defaults to `1`; `@ 0` is invalid.
- Descending loops use a negative step.
- `loop N` is shorthand for `for _ in 0..N`.
- Loop variables are immutable values local to the loop body. Assign a new
  local when an iteration-derived value needs to be changed.
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

### Basic Compile-Time Constants

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
- Const arrays are immutable. Their `.len()` and compile-time-indexed elements are themselves
  available to compile-time expressions.
- Inferred-length const array initializers can be literals, existing const arrays, const-array slices, or array-returning `const def` calls.
- Untyped scalar const declarations remain contextual compile-time numerics and preserve the
  widest supported literal representation until each use site selects a concrete scalar type.
- A typed const fixes its scalar type at the declaration. An untyped pure numeric const may
  specialize directly to `f32` in one context and `f64` in another.
- Once a numeric expression is concretely typed, every runtime operation uses that width and
  observes that type's normal rounding semantics. Use an explicit cast to request wider evaluation.
- Reassignment, forward references, recursion, and mutual recursion are rejected.
- Scalar `const` declarations are also valid inside runtime statement scopes and directly inside a
  proc. They are lexical compile-time names, not runtime storage. Const arrays remain limited to
  top-level and namespace scope.

## 5. Audio and Control Interfaces

The audio and control interface is the part of a processor that its host or
parent processor can connect. Inputs carry samples into the processor,
parameters carry control values, and outputs carry audio or block-rate values
out.

### Inputs

`ins` declares input ports. `inputs` is an alias.

```onda
ins:
  in1
  side: f64
  stereo: f32[2]
```

These shorthand forms are alternatives:

```onda
ins 2
```

```onda
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
- Inputs are read-only. A ranged top-level input is clamped once per sample before Onda code reads
  it; floating NaN maps to the range minimum.

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
- Top-level params are read-only to Onda code. Hosts update them; assignment is not a way to modify
  the host control value.

#### Host Control Domains

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

These shorthand forms can be combined when their generated names are disjoint:

```onda
outs 2
kouts<f32> 4
```

The default element type can instead be attached to an explicit section:

```onda
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
- Current-owner outputs are write-only: `out1 = out1` and reading `stereo[0]` are errors. A parent
  may read a child proc's most recently produced output, such as `voice.out1`.
- Audio outputs can be written only in `sample`; control outputs can be written only in block-pre or
  block-post. This timing rule also applies to named output arrays and the `outs[i]` / `kouts[i]`
  views.

## 6. Collections

### Arrays and Slices

Fixed-size arrays can be state or locals:

```onda
init:
  taps: f32[8]                         # eight zero-initialized elements
  gains: f32[3] = [1.0, 0.5, 0.25]   # explicit initializer

sample:
  coeffs = [0.5, 0.25, 0.125]
  out1 = coeffs[0] + gains[1]
```

`name: T[N]` allocates a fixed array and initializes each element with the type's default value.
An explicit initializer must contain exactly `N` compatible elements. `N` is a compile-time integer
expression. The declaration syntax itself is the array constructor; `T[N](...)` is not an Onda
expression.

An untyped array assignment takes its element type from the first element
using the ordinary first-assignment defaults, then checks every remaining
element against that type. An array literal used directly as a call argument
can instead acquire its element type from the parameter context.

Ordinary indexing is 0-based. Runtime selectors are normalized to the valid element range, while a
compile-time out-of-range const-array index is an error. `.len()` returns the fixed length for arrays
of primitives, structs, or procs, and the current length for a slice:

```onda
def sum(values: f32[]):
  total = 0.0
  for i in 0..values.len():
    total += values[i]
  return total
```

Primitive arrays use Python-style slice syntax. These examples slice the
persistent array created in `init`:

```onda
init:
  values: f32[8]

sample:
  all = values[:]
  from_two = values[2:]
  without_last = values[:-1]
  middle = values[1:-2]
```

Rules:

- Slice forms are `a[:]`, `a[start:]`, `a[:end]`, and `a[start:end]`.
- Negative bounds are supported.
- Slice expressions lower to primitive slice views of type `T[]`.
- Buffer slicing also yields `T[]`.
- Struct-element arrays are not sliceable in the current implementation.

Writable slice assignment is statement-only:

```onda
init:
  values: f32[8]
  source: f32[8]

sample:
  values[1:-1] = 0.5
  values[:] = source[:]
```

Scalar fill writes the full target slice. Slice copy writes
`min(dst_len, src_len)` elements. Overlapping slice copies behave as if copied
through a temporary. Event payload arrays and slices are read-only.

Passing a mutable primitive array or slice to a `def` passes a view of its storage, so indexed or
slice writes in the callee update the caller's array. Const arrays may be passed only when the full
callee chain is read-only. A fresh binding from an existing array or slice is a view alias, not a
deep copy; mutate array storage through indexed and slice assignments.

### Tuples

Tuples are anonymous fixed-length heterogeneous values. Tuple values and tuple types use
parentheses; destructuring targets conventionally do not.

```onda
def make_pair() -> (f32, i32):  # tuple type
  return (1.0, 42)              # tuple value

sample:
  value, count = make_pair()    # destructuring targets
  out1 = value + f32(count)
```

Rules:

- Tuple value syntax is `(value1, value2, ...)`; the parentheses are required.
- Tuple type syntax is `(T1, T2, ...)`; the parentheses are required.
- Tuple syntax requires at least two elements and does not accept a trailing comma.
- Maximum arity is 16.
- Nested tuples are not currently supported.
- Tuple element access uses compile-time integer indices.
- Tuple destructuring uses a bare comma-separated target list: `a, b = (10.0, 20.0)`.
  Parentheses around the targets are accepted, but the canonical style omits them. Use `_` to
  discard an element without creating a binding, for example `first, _, third = make_triple()`.
- Multi-output processor calls can be destructured directly; see
  [Constructing and Calling Procs](#constructing-and-calling-procs).
- Tuples can be locals, `init` state, `def` params and returns, and struct fields.
- A tuple binding keeps the arity and element types established by its declaration or first
  assignment. Reassignment accepts compatible values but never changes the binding's type.
- Tuple parameters are mutable local values and follow the same reassignment rules.
- A fresh tuple assigned at the root of `init` or before a block's `sample` section is persistent
  state. Fresh tuples introduced in nested control flow are lexical locals.

Unchecked indexing is deliberately kept out of the normal collection workflow.
Use ordinary indexing unless a proven hot path requires the escape hatch
described in [Unchecked Indexed Access](#unchecked-indexed-access).

## 7. Functions with `def`

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
- Multi-line argument lists; every comma must be followed by another argument.
- Method-style sugar for ordinary defs: `x.clamp01()` rewrites to `clamp01(x)`.
- Left-to-right argument evaluation, including named arguments.

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

Primitive scalar and tuple arguments are values. Arrays, slices, structs, procs, and buffers are
reference-like arguments: the callee receives access to the original aggregate or resource, subject
to its mutability and lifetime rules. This is why a def can update an array element or struct field
without returning the aggregate.

Names declared as callables by an owner cannot be reused by value bindings in
that owner's executable scopes. This includes defs, events, tasks, delegates,
and top-level processor and struct constructors. Function and event parameters,
`when` bindings, local constants, assignment and tuple bindings, and loop
variables all follow this rule. Receiver-qualified methods and callable names
brought into scope from another source file are not owner-local and may still be
shadowed by local values.

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

The complete set of structural, array, buffer, tuple, struct, and proc parameter
forms is collected under [Advanced Function Parameter Kinds](#advanced-function-parameter-kinds).
Generic defs are introduced with the rest of Onda's generic model in
[Compile-Time Programming and Generics](#14-compile-time-programming-and-generics).

## 8. Structs

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

- Typed fields, inferred fields, and field defaults.
- Methods.
- Overloaded methods.
- Methods with their own generic type params.
- Tuple fields.
- Nested structs and fixed arrays of primitives or structs.
- Generic structs.

Field declarations have three forms:

```onda
struct Voice:
  phase                 # f32, default 0.0
  active = false        # bool inferred from the default
  gain: f64 = 1.0       # explicit type and default
  taps: f32[4]          # fixed array, default-filled
```

A bare field defaults to `f32`. A field with `= expr` infers its type from that compile-time
default. A typed field accepts an optional compatible default; otherwise its scalar, tuple, array,
or nested-struct value is initialized from that type's defaults.

Construction:

```onda
init:
  a = Voice()
  b = Voice(0.25, true, 0.5)
  c = Voice(gain = 0.75)
  d: Voice
```

Rules:

- `self` must be the first method parameter.
- Methods can read and write struct fields through `self`.
- Call a method with `voice.tick(...)`; the equivalent explicit form is `Voice.tick(voice, ...)`.
- Constructor arguments bind fields positionally or by name. Omitted fields use their defaults.
- Typed struct declarations are `init`-only.
- Declaration-only form such as `d: Voice` desugars to default-constructor initialization.
- For generic structs, typed declarations require explicit type args when the type is still generic.

Struct instances have reference semantics when passed to defs or bound to an alias. A method or def
that assigns a field updates the original instance; assignment does not implicitly deep-copy an
aggregate.

### Struct Arrays

Fixed arrays of structs are initialized in the same three useful ways as individual structs:

```onda
struct Marker:
  value: f32 = 1.0

init:
  defaults: Marker[2]
  listed: Marker[2] = [Marker(value = 2.0), Marker(value = 3.0)]
  broadcast: Marker[2] = Marker(value = 4.0)

sample:
  selected = listed[1]
  selected.value += 0.5
  out1 = selected.value + broadcast[0].value
```

The broadcast form constructs every element from the same constructor arguments; it does not make
all slots aliases of one instance. Selecting an element produces an alias to that element, so the
compound assignment above updates `listed[1].value`. Runtime selectors are clamped just like
primitive-array selectors.

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

## 9. External Buffers

Buffers are host-bound sample data. Unlike ordinary arrays, their storage and
runtime dimensions come from the host, and rebinding remains visible to later
processing calls.

### Declarations and Channel Layout

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

### Collections and Shorthands

The following are alternative shorthand styles:

```onda
buffers 2
```

```onda
buffers<f32>:
  delay
  scratch
```

```onda
buffers:
  piano: f32 {88}
  stereo_layers: f32[2] {4}
  named_count: f32 {count = 8}
```

`{N}` declares a fixed collection of `N` independently bound buffers. `{count = N}` is an optional
named spelling of the same declaration. The count belongs to the resource declaration, not its
element type: `stereo_layers` is four buffers, each with two channels. It does not introduce a
general multidimensional-array type.

### Access and Metadata

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
  source_frames = src.len()
  source_bound = src.bound()
  bus_channels = bus.chans()
  source_rate = src.samplerate()
  key_count = piano.len()
  middle_c_frames = piano[39].len()
  middle_c_bound = piano[39].bound()
  middle_c0 = piano[39][0]
  right0 = stereo_layers[0][1, 0]
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

### Aliases and Interpolation

A selected buffer can also be bound to an immutable, scoped reference alias:

```onda
buffers:
  layers: f32[] {4}

block:
  source = layers[0]

  sample:
    out1 = source.readL(0, 0.0)
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

### Indexing and Binding Rules

All source-level coordinates clamp independently. In `stereo[channel, frame]`, for example, the
channel clamps to the channel range and the frame clamps to the frame range before the address is
formed. Fixed buffer-collection selectors likewise clamp and select a descriptor in constant time.
The compiler removes that normalization when it can prove the complete coordinate range is valid.

Buffers also support the general [unchecked indexed access](#unchecked-indexed-access) operations.
They make every supplied buffer coordinate an explicit programmer responsibility:

```onda
sample:
  x = src.read_unsafe(0)
  y = read_unsafe(bus, 1, 0)
  src.write_unsafe(0, x)
  write_unsafe(bus, 1, 0, y)
```

These coordinates are valid even for a neutral unbound buffer: every buffer
has at least one frame, and `bus` has exactly two channels. With dynamic
coordinates, the program must establish the same guarantees itself.

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
- `.bound()`, `.chans()`, and `.samplerate()` apply to a selected buffer, not to the collection.
  `.bound()` reports whether that slot currently has a host binding. Exact channel counts are
  compile-time constants in generated code; dynamic counts come from the bound instance.
- Runtime binding validates element type and channel constraints. Each fixed-array slot binds
  independently and may be omitted.
- Host metadata names physical collection slots `bank[0]`, `bank[1]`, and so on, while separate
  collection metadata preserves the logical `bank` name and its contiguous slot range.
- An unbound slot is a neutral one-frame buffer: reads return the element type's zero, writes are
  discarded, `.len()` is `1`, `.samplerate()` is the host sample rate, and `.chans()` is the exact
  declared channel count or `1` for a dynamic-channel declaration. `.bound()` is `false`; all
  other valid bindings report `true`.
- Binding with a zero sample rate unbinds the buffer; the pointer and dimensions are ignored.
- Primitive buffer slices are supported with the same slice syntax as arrays.

## 10. Processors

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

A proc uses the same `const`, `ins`, `params`, `buffers`, `outs`, `kouts`,
`init`, `sample`, and `block` forms as the top level. A proc normally has
one execution body: `sample`, `block`, or `graph`. Proc events, delegates,
and tasks are introduced after ordinary construction and calls are clear.

### Proc Inputs, Params, Outputs, and Buffers

Proc sections use the same surface syntax as the top level, with these
differences:

- `kins` is not valid inside a proc; proc parameter sections are always `params`.
- Proc-local scalar constants may be used by section counts, defaults, shapes, and executable code;
  proc-local const arrays are not supported.
- A processor declares either `outs` or `kouts`, not both.
- `kouts` processors use `block` with no nested `sample`, cannot declare `ins`, and cannot declare `graph`.
- Proc constructor arguments for params and buffers are named-only.
- Proc inputs are bound by positional proc call args or named input args.
- Undeclared `paramN` and `koutN` uses infer numbered proc params and control outputs respectively;
  procs do not infer `kinN`.
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

A call steps the processor once. For a single-output proc, the call itself
produces that output:

```onda
init:
  g = Gain(g = 0.5)

sample:
  out1 = g(in1)
```

Destructure a multi-output proc call to step it once and bind every output in
declaration order:

```onda
sample:
  out1, out2 = stereo(in1, in2)
```

The number of targets must exactly match the processor's output count. This
form also works for nested processors and indexed processor arrays. A dynamic
array index is evaluated once; the selected processor is stepped once, then
its needed outputs are read. Use `_` for an output that does not need a binding.

The following are alternative access forms, not a sequence to copy into one
sample body:

- `g(in1).out1` explicitly selects the output from the call.
- `g.g = 0.25` updates the stored proc parameter without stepping the proc.
- `g.out1` reads the most recently produced output without stepping the proc.

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
- A sample-rate proc step may be called only from sample-rate code. A block-rate `kouts` proc step
  may be called only from block code (or from a task, which advances at block rate).
- Calling a child steps it. Reading `g.out1` or `g.kout1` without `()` returns that child's most
  recently produced output and does not step it.

### Proc-Local Defs

Processors can declare private helper defs that implicitly see proc state.

```onda
proc Filter:
  ins 1
  outs 1

  init:
    state = 0.0
    coeff = 0.5

  def apply(x: f32):
    state = state + (x - state) * coeff
    return state

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

### Private Params

Use `private` when a proc param should be initialized and updated only through
that proc's controlled code path.

```onda
proc Filter:
  params:
    private cutoff = 1000.0
    private q = 0.707
```

Private params:

- Can be set by the constructor.
- Can be set by the builtin proc `init(...)` event.
- Can be read or assigned by the owning proc's own `init`, `sample`, `block`, `event`, or proc-local `def` bodies.
- Cannot be accessed directly from outside through `child.cutoff`, `child.cutoff = ...`, `child.coeffs[i]`, `child.coeffs[i] = ...`, or `child(cutoff = ...)`.
- Cause external dynamic `child.params[i]` access to be rejected for that child proc.

`private` is a reserved keyword. It is only valid as a proc-param prefix.

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

### Proc Arrays

Arrays of proc instances are supported in `init`.

```onda
proc Voice:
  params:
    level = 0.0

  sample:
    out1 = level

params:
  selected: i32 = 0 {0, 3}

init:
  voices: Voice[4] = Voice()

sample:
  out1 = voices[selected](level = 0.5)
```

Supported forms:

- Literal array construction: `voices: Voice[2] = [Voice(), Voice()]`.
- Broadcast constructor sugar: `voices: Voice[4] = Voice()`.
- Compile-time capacity expressions in the array length.

The selected `Voice` is stepped once and produces `0.5`. Other indexed forms
include:

```onda
sample:
  voices[i](freq)
  out1 = voices[i].out1
  voices[i].gain = 0.5
  voices[i].note_on(220.0)
```

This second fence is a syntax summary: it assumes `i`, `freq`, and the shown
members are declared by the surrounding program.

Rules:

- Runtime indices are clamped to the valid slot range.
- Aliasing such as `v = voices[i]`, then `v(...)`, is supported.
- Proc-array buffer refs resolve through the current validated buffer tables.
- A proc cannot directly instantiate its own type in its own state.

If the proc defines a `block` section, indexed proc-array calls use active-slot
block-hook semantics: block-pre runs lazily on the first `()` call to that slot
in the current block, and block-post runs once at block end for each called slot.
Plain slot retrieval does not trigger hooks.

## 11. Events, Printing, and Delegates

Events carry commands inward, while delegates and printing carry sparse
occurrences outward. All of them execute synchronously on the processing thread;
they are not background callbacks.

### Top-Level Events

Top-level events are host-triggered handlers that run against an initialized instance. They are useful for musical
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
- Handlers can read, write, and query declared buffers using the instance's current bindings.
- Aside from declared buffers, top-level handlers may write only existing top-level state rooted in
  `init`.
- Unknown top-level event indices are ignored at runtime.
- A known top-level event with the wrong payload size is a runtime error.
- Top-level host events with slice params use payload layout `i32 len` followed by contiguous element bytes.

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
- Proc handlers can read, write, and query their declared buffers using the instance's current bindings.
- Proc handlers cannot write inputs or outputs.
- Generic proc events can use generic primitive placeholders such as `T`, `T[N]`, and `T[]`.

Every proc also gets a reserved builtin `init(...)` event. It mirrors the proc
params in declaration order and adds `full: bool = false`, assigns
provided values into params, reruns that proc instance's `init`, then runs bound
param hooks. Omitted args use defaults. The call forwards the proc instance's
current buffer bindings, so it can explicitly refresh state derived from a
buffer that the host rebound after construction.

By default the call preserves pinned roots while reinitializing resettable
roots. Passing `full = true` performs the full initialization used by fresh proc
construction. The effect on task continuations is covered in
[Tasks](#12-tasks), and both initialization modes are detailed in
[Reinitialization and Pinned State](#reinitialization-and-pinned-state).

```onda
voice.init(0.5)
voice.init(gain = 0.5)
voices[i].init(freq = 220.0, amp = 0.1)
voice.init(full = true)
```

These call forms assume the corresponding `voice` or `voices` instance was
constructed in the surrounding program.

`full` is reserved as a proc parameter name.

### Printing

`print` is a compiler-known runtime statement. It accepts an optional leading quoted label followed
by zero or more primitive scalar values:

```onda
init:
  phase = 0.0
  print("ready")

event report():
  print("phase", phase)

sample:
  phase = phase + 0.001
  out1 = sin(phase)
```

Here `ready` is emitted during initialization. The host-triggered `report`
event prints the current persistent phase without producing one log occurrence
for every audio sample.

The printable types are exactly `f32`, `f64`, `i32`, `i64`, and `bool`. Aggregates, buffers, and
processor values are rejected; print their scalar members or metadata explicitly. The label is
compile-time text rather than an Onda string value. It supports `\"`, `\\`, `\n`, `\r`, and `\t`
escapes.

Pure numeric literals use the ordinary unconstrained defaults in this statement: `print(3)` records
an `i32`, while `print(3.0)` records an `f32`. Explicit constructors select `i64` or `f64`, and
already-typed expressions retain their type.

Each execution produces one ordered occurrence. Canonical host text renders a labelled occurrence
as `label: value1 value2`, joins unlabelled values with one space, and terminates every occurrence
with a newline. Label control characters are escaped in that text so an occurrence always occupies
one physical line. Integer formatting is exact, including `i64`; floating-point formatting is the
shortest width-correct round-trippable representation, with `.0` retained for integral values.

`print` is valid in authored runtime statement scopes, but it is invalid in compile-time
declarations, `const def` bodies, expressions, graphs, and declaration names. Arguments are always
evaluated in source order even when the host elects not to collect print output.

### Delegates and `when`

Delegates report sparse typed occurrences in the opposite direction from events. An owner declares
them with singular or plural syntax and triggers its own delegate with an ordinary statement call:

```onda
init:
  last_reason: i32 = 0

delegates:
  stopped(reason: i32)

event stop(reason: i32):
  stopped(reason)

when stopped(reason):
  last_reason = reason
```

Delegate parameters have the same scalar, fixed-array, slice, generic specialization, default, and
argument-binding rules as event parameters; an omitted type defaults to `f32`. Delegate calls have
no result. They are valid in
`sample`, structured `block` code, tasks, event and `when` handlers, and owner-local runtime defs.
They are invalid in `init` and in runtime defs reachable from `init`. Only the declaring owner can
call a delegate; `child.finished()` is not a callable surface.

`when` installs a static synchronous subscription. It can observe the current owner or one direct
child ownership layer:

```onda
proc Envelope:
  delegate finished(reason: i32)

  event stop(reason: i32):
    finished(reason)

  sample:
    out1 = 0.0

init:
  child_reason: i32 = 0
  env = Envelope()

when env.finished(reason):
  child_reason = reason

sample:
  out1 = env()
```

A subscription can also target a compile-time-selected proc-array element or a
whole fixed proc array:

```onda
when voices[0].finished(reason):
  first_reason = reason

when voices.finished(index, reason):
  voice_reason = reason
```

A selected proc-array index must be a compile-time constant. A whole fixed proc array adds a
leading inferred `i32` element index; use `_` to ignore it. Bindings are read-only and otherwise
follow event-handler scope rules. Handlers run immediately in declaration order. Nested delegate
calls are depth-first, and recursive event/delegate dispatch is rejected.

Top-level occurrences are returned to the host to process, and their resolution is at the block
level. Currently, delegates are sample-accurate only within Onda via procs.

## 12. Tasks

The top-level program and procs can declare statically allocated cooperative
tasks. They spread divisible preparation work, such as lookup-table
construction, over multiple logical blocks without dynamic allocation or worker
threads.

```onda
init:
  table: f32[8]

task prepare():
  for i in 0..4:
    table[i] = f32(i) / 8.0
  yield
  for i in 4..8:
    table[i] = f32(i) / 8.0

block:
  await prepare()

  sample:
    out1 = table[7]
```

On the first block, `prepare` fills half the table and yields, so the block
produces neutral output. On the next block it resumes, finishes the table, and
processing continues after `await`. Tasks use the same syntax inside a proc.
Several declarations can be grouped under `tasks:`; the standalone and grouped
forms are equivalent.

Tasks take no arguments and return no values. They implicitly see their
owner's params, buffers, and init-rooted state, but cannot directly read audio
inputs, write owner outputs, invoke their owner's event handlers, or invoke
other tasks. They may call builtins and non-yielding defs visible from the
owner, call block-rate processor steps, and synchronously call child-proc
events, including the child's builtin `init(...)` event. Sample-rate processor
steps remain sample-only and therefore cannot be called by a task, directly or
through a def.

`yield` suspends the current task; bare `return` or reaching the end completes
it. A task runs synchronously on the process thread until one of those points;
`yield` is a cooperative boundary, not a time budget or preemption point.

### `await` and Scheduling

The owner advances a task with `await` from block-pre control flow, as in the
complete example above.

If the task yields or has failed, the owner stops that activation and produces
neutral outputs. For a top-level task those are the program outputs; for a proc
task only that proc becomes neutral and its parent continues normally. If the
task completes, execution continues after `await` in the same logical block. A
task is reset explicitly with `prepare.reset()` from the owner's `init`, event,
or block-pre scope.

The containing block is not a coroutine. Its block-pre control flow starts from
the beginning on each activation, so statements and conditions before an
`await` are evaluated again. Completed tasks fall through without rerunning;
the first reached task that yields stops the activation. An incomplete task has
no effect when ordinary control flow bypasses its `await`, leaving the program
responsible for not exposing partially prepared state.

Each proc instance, including each element of a proc array, owns independent
task continuations. An explicitly called block-rate proc runs its block body,
including any reached `await`, on every call just like other block-rate proc
code. Sample-rate proc instances scheduled statically run their block-pre
activation at most once at logical-block begin; a runtime-indexed proc-array
element runs it lazily on its first sample-rate call in that logical block.
Splitting a logical block into process segments never grants additional
resumptions to those scheduled activations, and a zero-frame begin-block
segment still advances statically scheduled tasks.

### Continuations, Reset, and Reinitialization

Task continuations are compiler-pinned state. Preserve-pinned initialization preserves
them. Proc `init(full = true)` and host-level `init(FULL)` restore them to
not-started. An explicit `prepare.reset()` in an initializer always runs. Tasks
may use both pinned and resettable state; after default initialization, a
suspended task observes the reinitialized resettable values when it resumes.
Snapshots include task status and continuation storage, so restoring a suspended
task resumes it from the captured suspension point.

`reset()` invalidates the continuation in constant time. It does not eagerly
clear task-frame storage: restarting the task executes its declarations and
initializers before that storage can be observed again. Full initialization
still initializes the complete continuation image.

Locals that are live across a `yield`, including fixed aggregates and loop
control, become statically allocated continuation state. Runtime handles cannot
cross a suspension point: buffer descriptors, slices, proc aliases, and other
reference-like values must be dead at `yield` and reacquired after resumption.
The compiler rejects only references that are live across the boundary.

Tasks read owner params and current buffer mappings whenever they resume.
Changing a parameter or rebinding a buffer does not reset a task automatically;
the program must call `reset()` when previously prepared or partially prepared
state is no longer valid.

A runtime failure reports through the failing process call and invalidates the
processor state. Hosts must emit silence and reject further stateful operations
until full initialization or snapshot restoration succeeds. This is the same
fail-closed behavior as a runtime failure outside a task.

Tasks are private to their owner and share that owner's declaration namespace.
They cannot be used with a `graph` block. `await` is valid only in structured
block-pre control flow; task reset is valid only in owner `init`, event, and
block-pre code. Neither operation is a first-class callable value.

## 13. Graphs

`graph` gives you a declarative way to wire processor instances and signal flow.

```onda
proc GainProc:
  ins 1

  params:
    gain = 1.0

  outs 1

  sample:
    out1 = in1 * gain

ins 1
outs 1

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

outs 1

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
- Typed array declarations are statements rather than expressions and therefore cannot be graph
  sources; use an array literal or an existing array value.
- Top-level `kouts` and block-rate proc outputs are not supported by `graph`.
- Graph event propagation syntax does not exist; use ordinary `events` or `event` declarations.

Given previously constructed `reverb` and `voices` nodes, proc bundles use the
same routing syntax:

```onda
graph:
  reverb >> { out1, out2 }
  voices[0] >> { left, right }
```

## 14. Compile-Time Programming and Generics

Basic `const` declarations were introduced with runtime values. This chapter
covers host-selected configuration, compile-time helper functions, and the
generic specialization mechanisms shared by defs, structs, and processors.

Onda specializes explicit type parameters on defs, structs, and procs. Runtime
defs can also specialize structurally from untyped parameters. Integer
namespace parameters, used for counts and shapes, are introduced with
namespaces in the next chapter.

### Configuration Constants and `const def`

Use `config const` for the explicitly typed subset of root constants that a host may select for one
compilation:

```onda
config const Channels: i32 = 2
config const Enabled: bool = true
config const Coefficients: f32[Channels] = [0.5, 1.0]
config const Window: f64[] = [0.0, 0.5, 1.0]

const SampleCount: i32 = Channels * BLOCK_SIZE
```

- Every `config const` requires an explicit type. It supports exactly the same value types as a
  typed ordinary const: `bool`, `i32`, `i64`, `f32`, or `f64`, and fixed or inferred-length
  primitive const arrays.
- Configuration constants are allowed only at the executable root. The entry file and its
  `include` files share that root; declaration modules loaded with `import` cannot declare them.
- A host override replaces only that declaration's initializer for one compilation. Derived
  constants, shapes, specialization, assertions, and generated code are recomputed normally.
- Fixed array lengths are resolved from the complete selected configuration. If `Channels` above
  changes, `Coefficients`—whether supplied by the host or evaluated from its default—must have the
  new length or compilation fails.
- Ordinary constants cannot be overridden. With no host input, the source initializer is used.

The native CLI accepts repeatable `--const Name=value` inputs using Onda literal syntax and
`--list-consts` prints the resolved configuration surface. An `.ondaproject` may provide defaults
in its `constants` map; explicit CLI inputs override matching project values.

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
- Compile-time loop evaluation is capped at 1,000,000 iterations per loop.
- Scalar-returning const defs can be used by scalar const declarations.
- Fixed-array-returning const defs can be used by const array declarations.

Const arrays and const slices can be passed to ordinary runtime `def` array
params when the callee treats the param as read-only. Writes through the param,
aliases or forwarding to a mutable callee make the param
mutable and reject const-array arguments.

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
- A type param not constrained by any call argument defaults to `f32`; for example, `zero<T>()`
  called as `zero()` specializes `T` to `f32`.
- Generic type params can appear in scalar params, array params, buffer element params, locals, casts, and supported return annotations.
- `const def` cannot declare type parameters.

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

### Structural Def Specialization

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

## 15. Modules, Namespaces, and `use`

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
std/osc std/filter std/env std/dynamics std/delay std/sample std/reverb std/pitch_shift
std/data std/lookup std/fft std/convolution std/gain std/levels std/mix
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

ins 1
outs 1

sample:
  out1 = my::dsp::sat(in1)
```

Namespace-local consts and nested namespaces are supported:

```onda
namespace Config:
  const MaxVoices = 8
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

Rules:

- Namespace template params require defaults.
- Args support positional and named forms.
- Args are normalized as `i32(...)` at compile time.
- Namespace-local `assert(expr)` performs compile-time checks.
- `<>` is used for namespace instantiation, generic specialization, and section default type modifiers.
- `[]` is used for arrays, indexing, slices, and buffer/channel forms.

Namespace aliases may retain compile-time arguments:

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

ins 1
outs 1

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

## 16. Reference Notes

### Top-Level Forms

| Form | Purpose |
| --- | --- |
| `ins`, `inputs` | Host input ports. |
| `params`, `kins` | Host-visible parameters. |
| `outs`, `outputs` | Audio-rate output ports. |
| `kouts` | Block-rate control output ports. |
| `buffers` | External host-bound buffers. |
| `events`, `event` | Host-triggered event handlers. |
| `delegates`, `delegate` | Typed occurrences reported to an owner or host. |
| `when` | Static synchronous delegate subscription. |
| `print(...)` | Publish bounded host-facing diagnostic output from runtime code. |
| `init` | Setup and persistent state. |
| `block` | Per-block code. |
| `sample` | Per-sample code. |
| `graph` | Declarative signal routing. |
| `task`, `tasks` | Statically allocated cooperative preparation work. |
| `const`, `config const`, `const def` | Compile-time values, host-selected compile inputs, and helpers. |
| `def` | Runtime helper functions. |
| `struct` | Nominal data types. |
| `proc`, `processor` | Reusable DSP processors. |
| `namespace` | Qualified declaration groups and integer templates. |
| `use`, `pub use` | Unqualified lookup imports and re-exports. |

### Reinitialization and Pinned State

The basic `init` lifecycle is described in [Execution and State](#3-execution-and-state).
The following rules matter to hosts that explicitly reinitialize an existing
instance or preserve expensive state.

Host `init(PRESERVE_PINNED)` preserves pinned roots and task continuations while
rerunning ordinary declaration initializers and every explicit init statement.
Host `init(FULL)` also reruns the declaration initializers for pinned roots and
task continuations, and is required before stateful operations on an
uninitialized instance. Both modes execute directly against the instance's
single state image and allocate no memory on the successful path. Their
execution cost depends on the authored initializer; a runtime failure leaves
instance state indeterminate. Initialized convenience creation is equivalent to
allocating storage, writing parameter defaults, and running `init(FULL)`.

Initialization observes the buffer bindings current for that call. Unbound buffers retain their
neutral behavior: reads return zero, writes are discarded, and metadata reports the neutral
one-frame descriptor; `.bound()` distinguishes that fallback from a real binding. A later rebind
is visible immediately to subsequent block, sample, event,
and init entry points, but does not implicitly rerun initialization or change state that an earlier
initializer derived from the old binding. A host that wants to refresh such derived state requests
initialization explicitly. This allows hosts with buffers available at startup to perform one-time
preprocessing in `init` instead of adding setup work to block or sample callbacks. Proc init has the
same access to its declared buffer surface, including buffers supplied by its constructor.

A direct persistent binding can opt out of default initialization with the `pin`
modifier:

```onda
init:
  pin prepared: f32[4096]
  pin generation: i32 = 0
  history: f32[128]
```

`init(PRESERVE_PINNED)` preserves `prepared` and `generation` while reinitializing
`history`. Initialized construction and `init(FULL)` initialize every binding. Snapshots
include both policies and restore the captured values; `pin` affects
initialization, not snapshot semantics. Integer-domain attributes remain
independent, for example
`pin partition = 0 {8, wrap}`.

The modifier is valid only on a fresh persistent value binding introduced
directly by `init`. It applies to the complete state root and supports primitive
scalars, fixed arrays, tuples, structs, and arrays of structs. Individual fields
or elements cannot select another policy unless they are separate init roots.
Proc instances and proc arrays cannot be pinned; their child state owns
its pin status. Params, inputs, outputs, buffers, locals, aliases, and constants
cannot be pinned either. `pin` is not valid on a nested assignment or an update
to an existing binding.

### Advanced Function Parameter Kinds

`def` params support:

- Primitive scalars.
- Untyped scalars inferred and specialized from each call site.
- Explicit struct types.
- Typed primitive slices such as `arr: f32[]`.
- Fixed primitive arrays such as `arr: f32[4]`.
- Untyped arrays such as `arr: []`.
- Struct-array views such as `items: Item[]` and fixed contracts such as `items: Item[4]`.
- Proc-array views such as `voices: Voice[]` and fixed contracts such as `voices: Voice[8]`.
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

def stereo_sum(arr: f32[2]):
  return arr[0] + arr[1]

def read_first(buf: buffer):
  return buf[0]
```

An unsized primitive, struct, or proc array parameter accepts any compatible runtime length and
supports `.len()`. A fixed parameter additionally requires the call-site length to match exactly.
Untyped and unsized aggregate parameters are specialized from their concrete call sites.

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

### Unchecked Indexed Access

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

`read_unsafe` also selects references from arrays of structs and processors.
Given compatible `cells` and `voices` arrays and a proven-valid `index`, the
forms are:

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

### Section Shorthands

| Section | Count shorthand | Default type shorthand | Generated names |
| --- | --- | --- | --- |
| `ins` | `ins N` | `ins<f64>:` | `in1..inN` |
| `params` | `params N` | `params<i32>:` | `param1..paramN` |
| `kins` | `kins N` | `kins<i32>:` | `kin1..kinN` |
| `outs` | `outs N` | `outs<f64>:` | `out1..outN` |
| `kouts` | `kouts N` | `kouts<f32>:` | `kout1..koutN` |
| `buffers` | `buffers N` | `buffers<f32>:` | `buf1..bufN` |
| `init` | — | `init<f64>:` | Fresh untyped scalar bindings use the section type. |

Section counts can use compile-time integer expressions, ordinary const values,
and namespace integer template params. A bare integer or name needs no grouping; wrap a compound or
call expression, for example `outs (channel_count())`.

A section default is contextual typing for every otherwise-untyped declaration in that section; it
is not merely a fallback for ambiguous initializers. The initializer spelling does not override it:
for example, `mode = 0` in `params<f32>:` is `f32`. Add an item type when a declaration needs to
override the section default, such as `mode: i32 = 0`.

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

Input/output surfaces are executable-rate-bound. `inN`, `outN`, `koutN`, declared I/O arrays, and
synthetic I/O views cannot be read, written, passed, returned, or stored from `init`, `event`, task,
or top-level `def` bodies. Within a structured `block`, audio inputs and `outs` are available only in
the nested `sample`; `kouts` are available only in block-pre and block-post. Inputs and params are
read-only through these dynamic views, while current-owner outputs are write-only. Named proc params
remain writable by their owning proc according to the processor rules.

### Aliases and Reserved Names

- `inputs` aliases `ins`.
- `outputs` aliases `outs`.
- `processor` aliases `proc`.
- Top-level `kins` aliases `params`.
- Control-flow keywords are reserved: `if`, `elif`, `else`, `for`, `in`, `while`, `loop`, `break`, `continue`, `return`, and `assert`.
- `in` separates the loop variable from its range in `for i in A..B`; use names such as `input` for ports and variables.
- `import`, `include`, `use`, `as`, `pub`, `private`, `pin`, and `config` are reserved for their declaration and modifier syntax.
- `true` and `false` are reserved boolean literals.
- Identifiers beginning with `__onda_` are reserved for compiler-generated symbols.
- Numbered `outN` names are audio outputs; use `koutN` for numbered control outputs.

### Common Current Limits

- Proc-local defs are not overloadable.
- Returning structs, arrays, or buffers from runtime `def` is unsupported.
- Struct-element arrays are not sliceable.
- `graph` source expressions cannot call user-defined functions or procs.
- `graph` does not support `kouts` or block-rate proc outputs.
- `graph` has no event-routing syntax.
