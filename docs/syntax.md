---
title: Language guide
description: A practical guide and complete reference to the Onda programming language.
permalink: /docs/language/
section: reference
eyebrow: Language reference
---

# Onda Language Guide

Onda is a programming language for generating and processing audio. A program
runs over successive blocks of samples, with explicit places for setup,
per-block work, and per-sample calculations.

## Contents

- [First program](#first-program)
- [Source notation](#source-notation)
- [Audio and control interfaces](#audio-and-control-interfaces)
  - [Inputs](#inputs)
  - [Parameters](#parameters)
  - [Outputs](#outputs)
- [Execution and state](#execution-and-state)
  - [`init`](#init)
  - [`sample`](#sample)
  - [`block`](#block)
- [Values and variables](#values-and-variables)
  - [Types and first assignment](#types-and-first-assignment)
  - [Section shorthands](#section-shorthands)
  - [Assignment](#assignment)
  - [Constants](#constants)
  - [Operators](#operators)
  - [Builtin constants and functions](#builtin-constants-and-functions)
  - [Integer storage domains](#integer-storage-domains)
- [Control flow](#control-flow)
  - [Scope in control flow](#scope-in-control-flow)
- [Arrays and tuples](#arrays-and-tuples)
  - [Arrays and slices](#arrays-and-slices)
  - [Interface arrays](#interface-arrays)
  - [Parameter arrays](#parameter-arrays)
  - [Indexed interfaces and access rules](#indexed-interfaces-and-access-rules)
  - [Tuples](#tuples)
- [Functions](#functions)
  - [Calling functions](#calling-functions)
  - [Specialization and monomorphization](#specialization-and-monomorphization)
  - [Returns](#returns)
  - [Scope](#scope)
  - [Values and shared storage](#values-and-shared-storage)
  - [Parameter forms and defaults](#parameter-forms-and-defaults)
  - [Overloads](#overloads)
- [Structs](#structs)
  - [Fields and construction](#fields-and-construction)
  - [Methods](#methods)
  - [Copies, aliases, and lifetimes](#copies-aliases-and-lifetimes)
  - [Struct arrays](#struct-arrays)
  - [Indexed struct-array field access](#indexed-struct-array-field-access)
- [Enums](#enums)
- [Processors](#processors)
  - [Proc inputs, params, and outputs](#proc-inputs-params-and-outputs)
  - [Constructing and calling procs](#constructing-and-calling-procs)
  - [Proc-local defs](#proc-local-defs)
  - [Private params](#private-params)
  - [Param update hooks](#param-update-hooks)
  - [Processor arrays](#processor-arrays)
  - [Entry processor: `Main`](#entry-processor-main)
- [Events and delegates](#events-and-delegates)
  - [Top-level events](#top-level-events)
  - [Proc events](#proc-events)
  - [Delegates and `when`](#delegates-and-when)
  - [Structured events and delegates](#structured-events-and-delegates)
  - [Printing](#printing)
- [External buffers](#external-buffers)
  - [Declarations and channel layout](#declarations-and-channel-layout)
  - [Collections and shorthands](#collections-and-shorthands)
  - [Access and metadata](#access-and-metadata)
  - [Aliases and interpolation](#aliases-and-interpolation)
  - [Indexing and binding rules](#indexing-and-binding-rules)
  - [Processor buffer bindings](#processor-buffer-bindings)
  - [Buffer binding and initialization](#buffer-binding-and-initialization)
- [Parameter domains](#parameter-domains)
- [Smoothing](#smoothing)
  - [Input smoothing](#input-smoothing)
  - [Parameter smoothing](#parameter-smoothing)
- [Execution options](#execution-options)
  - [Oversampling](#oversampling)
  - [Reinitialization and pinned state](#reinitialization-and-pinned-state)
- [Tasks](#tasks)
  - [`await` and scheduling](#await-and-scheduling)
  - [Continuations, reset, and reinitialization](#continuations-reset-and-reinitialization)
- [Graphs](#graphs)
  - [Edges and scheduling](#edges-and-scheduling)
  - [Sources and destinations](#sources-and-destinations)
  - [Processor arrays and bundles](#processor-arrays-and-bundles)
- [Modules and namespaces](#modules-and-namespaces)
  - [Imports](#imports)
  - [Includes](#includes)
  - [Namespaces](#namespaces)
  - [Use declarations](#use-declarations)
- [Compile-time programming](#compile-time-programming)
  - [Constant arrays](#constant-arrays)
  - [Configuration constants](#configuration-constants)
  - [Compile-time functions](#compile-time-functions)
  - [Evaluation and demand](#evaluation-and-demand)
- [Generics](#generics)
  - [Generic defs](#generic-defs)
  - [Generic structs and processors](#generic-structs-and-processors)
  - [Structural specialization](#structural-specialization)
  - [Integer namespace parameters](#integer-namespace-parameters)
- [Numeric conversions](#numeric-conversions)
  - [Contextual values and defaults](#contextual-values-and-defaults)
  - [Operand promotion](#operand-promotion)
  - [Implicit destination conversions](#implicit-destination-conversions)
  - [Evaluation width and explicit casts](#evaluation-width-and-explicit-casts)
  - [Numeric builtin types](#numeric-builtin-types)
- [Quick reference](#quick-reference)
  - [Declaration and statement index](#declaration-and-statement-index)
  - [Reserved names](#reserved-names)
  - [Aggregate limits](#aggregate-limits)
  - [Unchecked indexed access](#unchecked-indexed-access)
    - [Buffer coordinates](#buffer-coordinates)

For real-world usage, explore the [example cookbook](https://onda-lang.org/docs/examples/):
complete instruments, effects, soundscapes, and projects you can open in the browser playground.

For library APIs, see the [standard-library reference](https://onda-lang.org/docs/stdlib/).
[Getting started](https://onda-lang.org/docs/getting-started/) covers installation and the CLI;
[projects](https://onda-lang.org/docs/projects/) and the
[host API](https://onda-lang.org/docs/api/) cover packaging and integration.

## First program

This complete program multiplies an incoming audio signal by a host-controlled gain:

```onda
params:
  gain = 0.5 {0.0, 1.0}

sample:
  out1 = in1 * gain
```

`params` declares controls for the host—the application running the program.
Here, `gain` has a default value of `0.5` and a range from
`0.0` to `1.0`. `sample` runs once for each audio sample. `in1` and `out1` are
the first audio input and output; using these numbered names creates the ports
implicitly.

Save this as `gain.onda` and check it with `onda compile gain.onda`. Run it with
`onda run gain.onda` to open the interactive host.

State belongs in `init`. For example, this oscillator keeps its phase between samples:

```onda
params:
  freq = 440.0 {20.0, 20000.0}
  level = 0.5 {0.0, 1.0}

init:
  phase = 0.0

block:
  increment = freq * TWO_PI / SR

  sample:
    out1 = sin(phase) * level
    phase += increment
    if phase >= TWO_PI:
      phase -= TWO_PI
```

`init` runs when the host initializes the program. `block` computes the phase
increment once per audio block, and its nested `sample` uses that value for each
sample. `TWO_PI`, `SR` (sample rate), and `sin` are built-ins.

## Source notation

Onda accepts indentation or braces. These sections are equivalent:

```onda
sample:
  out1 = 0.0
```

```onda
sample {
  out1 = 0.0
}
```

Statements end at a newline or `;`. Comments start with `#`. Lists in parentheses,
brackets, or angle brackets may span lines. Parenthesized expressions may break
before or after an operator. Trailing commas are not accepted.

Declare names before using them; declarations are processed in source order.
A library file may contain only declarations, without executable sections.
A typical program puts imports and reusable definitions first, then its
interface, initialization, and processing code.

Keywords cannot be used as ordinary names.
See [Reserved names](#reserved-names) for the reference list.

## Audio and control interfaces

An Onda program receives audio through `ins`, reads host controls through `params`,
and sends audio through `outs`. These declarations describe the interface the host
connects to your program.

### Inputs

`ins` names the incoming audio signals. `inputs` is an alias:

```onda
ins:
  input_left
  input_right
```

Inputs are read-only: your code reads the incoming samples but cannot replace them.
Use them inside `sample`, where each name supplies the current sample's value.

Without an `ins` section, using `in1`, `in2`, and so on creates numbered inputs
implicitly. For explicit numbered declarations, see [Section shorthands](#section-shorthands).

Inputs can have defaults and inclusive ranges, for example
`freq = 440.0 {20.0, 20000.0}`. A single range value gives only the maximum:
`freq = 440.0 {20000.0}`. A ranged top-level input is clamped once per sample
before your code reads it; floating NaN maps to the range minimum.
[Input smoothing](#input-smoothing) can make changes gradual.

### Parameters

`params` declares controls that the host can edit:

```onda
params:
  gain = 0.5 {0.0, 1.0}
  enabled = true
```

`gain` has a default value of `0.5`; its braces give an inclusive range from `0.0`
to `1.0`. The range is optional. `enabled` is a boolean control. Hosts update
these controls; top-level Onda code can read them but cannot assign to them.

At the top level, `kins` is an alias for `params`; declare one or the other, not
both. Using a numbered name such as `param2` or `kin2` can create parameters
implicitly up to that number. [Section shorthands](#section-shorthands) declare
numbered parameters explicitly.

[Parameter domains](#parameter-domains) add logarithmic scales, units, and steps.
[Parameter arrays](#parameter-arrays) group related controls, and
[Parameter smoothing](#parameter-smoothing) explains how hosts handle changes.

### Outputs

`outs` names outgoing audio signals. `outputs` is an alias:

```onda
outs:
  left
  right
```

Assign each output's sample in `sample`. Outputs are write-only inside their
own program: `out1 = out1` is invalid. Keep a local value if you need to reuse a
calculation before writing it to an output.

Without an `outs` section, using `out1`, `out2`, and so on creates numbered audio
outputs implicitly.
[Section shorthands](#section-shorthands) declare them explicitly.

Use `kouts` for control values produced once per block—a group of audio samples:

```onda
kouts:
  level
```

Control outputs are written in `block`, outside any nested `sample`. Numbered
control outputs use `kout1`, `kout2`, and so on.
A top-level program can have both audio and control outputs, with distinct names.
The next section explains when each kind of processing code runs.

## Execution and state

Executable sections belong to the top-level program or a reusable
[processor](#processors), called their *owner* in this guide. They determine
when code runs:

1. `init` initializes persistent state.
2. `block` performs work before and after a logical block's sample loop.
3. `sample` produces or processes each audio sample.

Where a name is first bound determines its storage lifetime:

| First binding | Lifetime |
| --- | --- |
| Directly in `init` | State that survives between samples and blocks. |
| Directly in `block`, before its nested `sample` | Retained for the block, visible inside and after `sample`, and refreshed when this declaration runs again. |
| In `sample` | A local for the current sample invocation. |

### `init`

`init` runs when the host initializes the program. A variable introduced directly
inside it keeps its value between audio samples and blocks:

```onda
init:
  phase = 0.0
  gain = 0.5
```

Assigning to an existing state variable updates it. Variables introduced inside
conditionals or loops follow the [control-flow scope rules](#scope-in-control-flow).
[Reinitialization and pinned state](#reinitialization-and-pinned-state) explains
how a host can reset an existing instance.

### `sample`

`sample` runs once for each audio sample. Variables introduced here are local
to that invocation:

```onda
sample:
  driven = in1 * 2.0
  out1 = tanh(driven)
```

Each invocation recalculates `driven`, then soft-clips it with `tanh` before
writing the output. A `sample` section cannot introduce persistent state or
use `return`.

### `block`

`block` performs work once per logical audio block. Put calculations shared by all
samples before its nested `sample`, and work that uses the completed block after it.
The oscillator in [First program](#first-program) computes its increment this way.

Statements before the nested `sample` run once before processing the block's
samples. The `sample` body runs for each sample. Statements after it run once
after all samples in the block have been processed. `BS` is the number of samples
in a logical block:

```onda
init:
  last_mean = 0.0

block:
  total = 0.0               # Once before the block's samples.

  sample:
    total += in1            # Once per sample.
    out1 = in1

  last_mean = total / BS     # Once after the block's samples.
```

Audio-rate blocks require a nested `sample`; control-output (`kouts`) processors
use `block` without one. Programs with only control outputs also omit the nested
`sample`. Parameters and [buffers](#external-buffers) are available throughout;
[input and output access](#audio-and-control-interfaces) follows each port's rate.

Fresh bindings directly in `block` before `sample`, such as `total` above, are
available inside `sample` and in the statements after it. Names introduced after
`sample` are visible only after their declaration. Bindings inside nested control
flow follow the [control-flow scope rules](#scope-in-control-flow). A
[graph](#graphs) is an alternative to `block` and `sample`; an owner cannot use both
execution styles. [Oversampling](#oversampling) extends `sample` to run at a higher rate.

## Values and variables

### Types and first assignment

Onda has five primitive types:

| Type | Values |
| --- | --- |
| `f32`, `f64` | 32-bit and 64-bit floating-point numbers. |
| `i32`, `i64` | Signed 32-bit and 64-bit integers. |
| `bool` | `true` or `false`. |

The first assignment introduces a variable and chooses its type. An annotation
chooses the type explicitly:

```onda
sample:
  count = 0          # i32
  gain = 0.5         # f32
  enabled = false    # bool
  precise: f64 = 0.5
```

Without another type context, floating literals default to `f32`; integer
literals default to `i32`, or `i64` if their value requires it. Once a variable
has a type, later assignments keep that type. Assigning to an existing visible
name updates it. Where a name is introduced determines its
[lifetime](#execution-and-state).

Numeric casts use `f32(value)`, `f64(value)`, `i32(value)`, or `i64(value)`.
Runtime conversions may widen implicitly; narrowing requires a cast.
Convert a number to a boolean with `value != 0`; there is no `bool(value)` cast.
The exact arithmetic and conversion rules are in [Numeric conversions](#numeric-conversions).

### Section shorthands

Inputs and outputs default to `f32`, including implicit numbered ports. Parameters
infer their types from defaults, or use `f32` without a default.

Whole sections can also select a default type or declare a number of items:

| Section | Count shorthand | Default type shorthand | Generated names |
| --- | --- | --- | --- |
| `ins` | `ins N` | `ins<f64>:` | `in1..inN` |
| `params` | `params N` | `params<i32>:` | `param1..paramN` |
| `kins` | `kins N` | `kins<i32>:` | `kin1..kinN` |
| `outs` | `outs N` | `outs<f64>:` | `out1..outN` |
| `kouts` | `kouts N` | `kouts<f32>:` | `kout1..koutN` |
| `buffers` | `buffers N` | `buffers<f32>:` | `buf1..bufN` |
| `init` | — | `init<f64>:` | Fresh untyped scalar bindings use the section type. |

Section counts accept compile-time integer expressions, including constants and
[integer namespace parameters](#integer-namespace-parameters). A bare integer or
name needs no grouping; enclose compound expressions and calls in parentheses,
for example `outs (channel_count())`.

A section default types every otherwise-untyped declaration in that section,
regardless of the initializer's spelling. For example, `mode = 0` in
`params<f32>:` is `f32`. An item annotation overrides it: `mode: i32 = 0`.

This stereo gain combines count and type shorthands:

```onda
ins<f64> 2
outs<f64> 2

params<f64>:
  gain = 0.5

sample:
  out1 = in1 * gain
  out2 = in2 * gain
```

If an input count and an explicit list are both present, they must match exactly.
Top-level audio and control output shorthands may coexist when their names are disjoint,
for example `outs 2` and `kouts<f32> 4`.

### Assignment

Compound assignment updates an existing value:

```onda
init:
  phase = 0.0
  remaining = 8

sample:
  phase += 0.01
  remaining -= 1
  out1 = phase
```

Arithmetic forms are `+=`, `-=`, `*=`, `/=`, and `%=`. Integers also support
`&=`, `|=`, `^=`, `<<=`, and `>>=`. Each uses ordinary operand promotion, then
checks conversion back to the destination's existing type.

### Constants

`const` gives a name to a compile-time value:

```onda
const MaxVoices = 8
const HalfPi = PI / 2.0
const Precise: f64 = 0.125
```

Constants cannot be reassigned and must be declared before use. Declare them
at the file root or directly inside a namespace, never inside a processor or
executable block. An initializer must be computable at compile time.

Unannotated numeric constants retain their computed `i64`/`f64` precision until
a use selects a type; an annotation or explicit cast fixes the type.
[Compile-time programming](#compile-time-programming) covers constant arrays,
host-selected configuration, and compile-time functions.

### Operators

Supported operators:

| Category | Operators |
| --- | --- |
| Arithmetic | `+`, `-`, `*`, `/`, `%` |
| Comparisons | `==`, `!=`, `<`, `<=`, `>`, `>=` |
| Logical | `!`, `&&`, `\|\|` |
| Bitwise integer | `~`, `&`, `\|`, `^`, `<<`, `>>` |

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

### Builtin constants and functions

Builtin constants:

| Constant family | Names | Numeric family |
| --- | --- | --- |
| Pi | `PI`, `pi` | Floating |
| Two pi | `TWO_PI`, `TWOPI`, `two_pi`, `twopi` | Floating |
| Effective sample rate | `SAMPLE_RATE`, `SAMPLERATE`, `SR`, `sample_rate`, `samplerate` | Floating |
| Host sample rate | `HOST_SR`, `HOST_SAMPLE_RATE`, `HOST_SAMPLERATE`, `host_sample_rate`, `host_samplerate` | Floating |
| Block size | `BLOCK_SIZE`, `BLOCKSIZE`, `BS`, `block_size`, `blocksize` | Integer |

Common numeric functions include:

```text
sin cos tan tanh atan atan2 exp log sqrt pow abs fabs
floor ceil round trunc min max clamp fma
```

### Integer storage domains

Integer locals, state, and scalar struct fields may carry a finite storage domain:

```onda
const RingSize = 1024

init:
  bank = 0 {8}
  taps: i32 = 0 {range = 0..128}
  cursor: i32 = 0 {RingSize, wrap}
  samples_seen: i64 = 0 {range = 0..=100, mode = clamp}
```

Counts and endpoints must be exact compile-time integers. Supported forms are:

| Domain | Values |
| --- | --- |
| `{N}` or `{count = N}` | `0..N`; `N` must be positive. |
| `{begin..end}` or `{range = begin..end}` | Half-open range; `begin < end`. |
| `{begin..=end}` or `{range = begin..=end}` | Inclusive range; `begin <= end`. |

`count` and `range` are mutually exclusive. A positional count or range precedes
named fields and the mode. `clamp` is the default; `wrap` normalizes modulo the
domain size. Bare `clamp` / `wrap` and `mode = clamp` / `mode = wrap` are equivalent.
Initialization and every direct or compound assignment normalize on storage:

```onda
cursor = RingSize - 1
cursor += 1       # wraps 1023 to 0
taps = 200        # clamps to 127
```

Without an annotation, a ranged binding is `i32`, even if its initializer is
another variable. Annotate `i64` explicitly when needed. These domains are integer
storage invariants; use `clamp(value, lower, upper)` for general numeric clamping.
The comma-separated `{min, max}` syntax instead belongs to
[parameter domains](#parameter-domains), whose endpoints are inclusive.

Reading a ranged binding produces an ordinary integer; arithmetic does not inherit
its storage mode. Storage and snapshots use the underlying integer type.
The compiler carries proven ranges through arithmetic, value and read-only-reference
arguments, and scalar returns. All call sites must support a parameter's inferred
range; unknown calls or uncontracted writable references make it unknown.

A selector whose complete range fits a collection needs no index normalization.
For example, `cursor = 0 {1024, wrap}` stays valid for a 1,024-element array after
`cursor += 1`. Dynamic lengths still need normalization unless their bounds are proven.

## Control flow

Use `if` / `elif` / `else` for branches, and `for`, `loop`, or `while` for repetition:

```onda
sample:
  x = in1
  if x > 0.0:
    magnitude = x
  elif x < 0.0:
    magnitude = -x
  else:
    magnitude = 0.0

  total = 0.0
  for i in 0..4:
    total += f32(i)

  loop 4:
    total += 0.1

  while total > 1.0:
    total -= 1.0

  out1 = magnitude * total
```

A loop can specify its integer width and step:

```onda
sample:
  total: i64 = 0
  for i: i64 in 0..4:
    total += i
  for i: i32 @ -1 in 10..0:
    total += i
  out1 = f32(total)
```

- `for i in A..B` excludes `B`; `for i in A..=B` includes `B`.
- Range bounds and `loop` counts accept ordinary expressions.
- Loop variables default to `i32`; annotate them as `i32` or `i64` with
  `for i: TYPE in ...` when an explicit induction width is required.
- `@ STEP` defaults to `1`; `@ 0` is invalid.
- Descending loops use a negative step.
- `loop N` is shorthand for `for _ in 0..N`.
- Loop variables are immutable values local to the loop body. Assign a new
  local when an iteration-derived value needs to be changed.
- `break` and `continue` are supported in loops.

### Scope in control flow

A name introduced inside a loop stays local to that loop. A name introduced in
every continuing branch of an `if` is available afterward. Numeric scalar and
tuple-element types use the smallest type that accepts every branch without
narrowing: `i32` with `i64` becomes `i64`, while `f32` with `i64` becomes `f64`.

Inside `init`, these branch-local names do not become persistent state. Introduce
a binding directly in `init` to retain the result:

```onda
params:
  use_high_value = false

init:
  if use_high_value:
    temporary = 2.0
  else:
    temporary = 1.0

  carried = temporary
```

Both branches create the local `temporary`, so it is available after the
conditional. `carried`, introduced directly in `init`, becomes persistent
state.

## Arrays and tuples

### Arrays and slices

Fixed-size arrays can be state or locals:

```onda
init:
  taps: f32[8]                      # eight zero-initialized elements
  gains: f32[3] = [1.0, 0.5, 0.25]  # explicit initializer

sample:
  coeffs = [0.5, 0.25, 0.125]
  out1 = coeffs[0] + gains[1]
```

`name: T[N]` allocates a fixed array and initializes each element with the type's default value.
An explicit initializer must contain exactly `N` compatible elements. `N` is a compile-time integer
expression.

An untyped array assignment takes its element type from the first element
using the ordinary first-assignment defaults, then checks every remaining
element against that type. An array literal used directly as a call argument
can instead acquire its element type from the parameter context.
Array literals must contain at least one element and cannot provide backing
storage for an empty slice. Slice an existing array, such as `values[:0]`, to
create an empty view.

Ordinary indexing is zero-based. Runtime indices are clamped to the valid element
range; a compile-time out-of-range constant-array index is an error. `.len()`
returns an array's fixed length or a slice's current length:

```onda
init:
  values: f32[8]

sample:
  total = 0.0
  for i in 0..values.len():
    total += values[i]
  out1 = total
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

Slices use `a[:]`, `a[start:]`, `a[:end]`, or `a[start:end]`; negative bounds count
from the end. A primitive slice is a view of type `T[]`. Buffer slices yield the
same type, and [struct arrays](#struct-arrays) follow the same slicing rules.

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

A fresh untyped binding from an existing array or slice aliases its storage;
a typed fixed declaration can make an independent copy. See
[Copies, aliases, and lifetimes](#copies-aliases-and-lifetimes) for the binding rules
and [Values and shared storage](#values-and-shared-storage) for function arguments.

Arrays introduced in separate branches must have the same element type and fixed
length to be used after the branch. Aggregate aliases likewise need a compatible
shape; branch-dependent shapes are rejected. Use ordinary indexing unless a proven
hot path needs [unchecked indexed access](#unchecked-indexed-access).

### Interface arrays

Inputs, parameters, and outputs can group related values into fixed arrays:

```onda
ins:
  in1
  side: f64
  stereo: f32[2]

outs:
  out1
  stereo_out: f32[2]
```

Each input array can have a default; a list must match its declared length.
Array elements follow the [input](#inputs) and [output](#outputs) access rules,
including when they may be read or written.

### Parameter arrays

Declare a fixed array to group related host controls:

```onda
params:
  offsets: f32[4] = 0.0
  harmonics: i32[3] = [1, 2, 4]
  enabled: bool[3] = true
```

A scalar default fills the array; a list supplies one default per element and must
match its length. Parameter arrays support `f32`, `f64`, `i32`, `i64`, `bool`, and
[enum types](#enums). Boolean arrays use toggles without numeric ranges.

A [parameter domain](#parameter-domains)—range, scale, curve, unit, and step—applies
to each element independently. Inside a processor, these arrays follow the same
[write rules](#constructing-and-calling-procs) as scalar parameters; host-control
metadata applies when the processor is exposed as the entry.

`onda run` groups array elements under their logical name, with addresses such as `offsets[0]`.
Each element can be edited, automated, and reset independently; **Reset** restores all declared
parameter defaults. CLI writes use the same addresses, for example `--set 'offsets[1]=7'`.

### Indexed interfaces and access rules

Explicitly declared inputs, parameters, or outputs with one scalar type can be
accessed by index:

```onda
const Channels = 4
ins Channels
outs Channels

sample:
  for i in 0..Channels:
    outs[i] = ins[i] * 0.5
```

The same form works for `params[i]`, top-level `kins[i]`, `kouts[i]`, and public
`child.params[i]`. Indices are zero-based and clamped. Implicit numbered inputs
cannot be dynamically indexed.

These views are not first-class arrays. Do not assign, slice, pass, return, or
store `ins`, `outs`, `kouts`, `params`, `kins`, or child proc dynamic views.

The [input](#inputs) and [output](#outputs) access rules apply to indexed views too.
Numbered I/O names, declared I/O arrays, and these views cannot be read, written,
passed, returned, or stored from `init`, event, task, or top-level `def` bodies.
Dynamic parameter views are read-only, even when a processor can write its own
named parameters.

### Tuples

A tuple groups a fixed number of values, which may have different types:

```onda
sample:
  pair: (f32, i32) = (1.0, 42)
  value, count = pair
  out1 = value + f32(count)
```

Tuple values use `(value1, value2, ...)`; tuple types use `(T1, T2, ...)`.
Both require parentheses and between 2 and 16 elements. Tuples cannot nest.
Element access uses a compile-time integer index.

Destructure with `a, b = pair`; parenthesized targets are accepted, but the usual
style omits them. `_` discards a component without creating a name; that component
is still evaluated. [Multi-output processor calls](#constructing-and-calling-procs)
can also be destructured.

Tuples work as locals, persistent state, function parameters and returns, and
struct fields. Their arity and element types stay fixed after declaration.
Each component follows ordinary numeric context and assignment rules independently.
Tuple parameters are mutable local values.

## Functions

`def` names a reusable calculation. Parameters go in parentheses; `return` supplies
the result. Parameter and return types can be inferred or written explicitly:

```onda
def wrap_value(p, upper = 1.0) -> f32:
  if p > upper:
    return p - upper
  return p

sample:
  out1 = wrap_value(in1)
```

### Calling functions

Arguments can be positional or named. Omitted arguments use the parameter's default:

```onda
sample:
  a = wrap_value(in1)                # upper defaults to 1.0
  b = wrap_value(in1, 2.0)
  c = wrap_value(in1, upper = 2.0)
  out1 = a + b + c
```

Arguments evaluate left to right in source order, including named arguments.
Ordinary functions also support method-style calls: `x.clamp01()` means `clamp01(x)`.

### Specialization and monomorphization

An untyped parameter gets a concrete type from each call. The compiler builds a
version of the function for the argument types it needs. This is called
*monomorphization*: one source definition can produce several statically typed
versions, with no runtime type lookup.

```onda
def twice(x):
  return x + x

sample:
  integer = twice(2)         # i32 argument and result
  single = twice(0.5)        # f32 argument and result
  wide = twice(f64(0.5))     # f64 argument and result
  another = twice(3)         # same i32 specialization
  out1 = single
```

The return type is inferred for each specialization. Different runtime values do
not create new versions. Arrays and other structured arguments also contribute
any compile-time shape information needed by the function; runtime slice lengths
can still vary.

An annotation such as `x: f32` fixes that parameter's type: calls must supply a
compatible value, using ordinary conversions. Explicit type parameters such as
`def twice<T>(x: T) -> T` use the same specialization model. See
[Generic defs](#generic-defs) for type arguments and inference, and
[Structural specialization](#structural-specialization) for field and method requirements.

### Returns

Omit `-> T` to infer the return type, or specify it to check each returned value
against `T`. Returns follow ordinary assignment conversions; runtime narrowing
requires a cast.

- A value-returning function must return on every reachable path. A return inside
  a potentially empty loop is not sufficient.
- With no return annotation and no value return, a function is called as a statement.
  It may use bare `return` to exit early.
- Bare and value returns cannot mix. An explicit return type requires a value.

Return annotations support scalars, scalar tuples, nominal structs, and fixed
primitive or struct arrays, including specialized types. Struct and fixed-array
results are independent copies. Unsized slices and buffers cannot be returned.

Direct and mutual recursion are rejected; a function cannot call itself through
any chain of calls.

### Scope

Top-level functions do not capture the program's runtime state. Pass parameters,
buffers, and state explicitly; read inputs at their allowed rate and pass values.
Outputs themselves cannot be passed into a top-level function.
[Processor-local functions](#proc-local-defs) can access their processor's state.

### Values and shared storage

Scalar and tuple arguments are local values. Changing them inside a function does
not change the caller's variable. Arrays, slices, structs, processors, and buffers
refer to the caller's storage or resource, so permitted writes affect the caller:

```onda
def scale(values: f32[], gain: f32):
  for i in 0..values.len():
    values[i] *= gain

sample:
  values = [1.0, 0.5]
  scale(values, 0.5)
  out1 = values[0]          # 0.5: scale changed the array
```

Write permission is inferred from the function body and the functions it calls.
Const or read-only data is accepted only when that whole call chain is read-only.

### Parameter forms and defaults

| Parameter | Accepts |
| --- | --- |
| `x: f32`, `x: i32`, etc. | A scalar of the declared type, with ordinary conversions. |
| `x` | A scalar, tuple, or structural argument specialized at each call. |
| `p: (f32, i32)` | A tuple with the declared component types. |
| `values: f32[]`, `values: f32[4]` | A primitive view of any compatible length, or exactly four elements. |
| `values: []` | An array whose element type and shape come from the call. |
| `item: Item`, `items: Item[]`, `items: Item[4]` | A [struct](#structs) or struct-array reference. |
| `voice: Voice`, `voices: Voice[]`, `voices: Voice[8]` | A [processor](#processors) or processor-array reference. |
| `buf: buffer<f32>`, `buf: buffer` | A typed [buffer](#external-buffers), or one specialized from the call. |

Unsized array parameters support `.len()`; fixed parameters require an exact length.
Generic struct and processor parameters specialize from concrete arguments.

Defaults are allowed for scalars, tuples, and fixed primitive arrays. Unsized
arrays, buffers, processors, structs, and struct arrays cannot have defaults.
For nominal data, pass an explicit constructor such as `apply(Settings())`, or
create a named value first. The caller owns the temporary for the synchronous
call. Methods follow the same rules.

### Overloads

Top-level functions and struct methods may have several definitions with the same
name, distinguished by parameter types or argument count:

```onda
def sat(x: f32):
  return x

def sat(x: f64):
  return f32(x)
```

Resolution prefers an exact typed match, then typed matches that use permitted
[argument conversions](#implicit-destination-conversions). Explicitly typed
parameters outrank generic or structural parameters, which outrank untyped ones.
Defaults participate in matching; the return type does not. Equally valid
candidates are an error.

Contextual numeric arguments use their ordinary binding defaults when ranking
matches: `sat(0.5)` selects `f32`; `sat(f64(0.5))` selects `f64`. The same applies
to untyped and typed numeric constants, respectively. A constant must fit the
selected parameter type, even when a compile-time conversion is allowed.
Use an annotation or cast to resolve an ambiguous call.

## Structs

`struct` declares nominal data types with fields and methods.

```onda
struct Voice:
  phase: f32
  sig: f32

  def tick(self, hz):
    self.phase = self.phase + hz * TWO_PI / SR
    self.sig = sin(self.phase)
```

A struct has at least one data field. It can contain primitive and tuple fields,
nested structs, and fixed arrays of primitives or structs.

### Fields and construction

Fields may be untyped, inferred from a default, or explicitly typed:

```onda
struct Settings:
  phase                 # f32, default 0.0
  active = false        # bool inferred from the default
  gain: f64 = 1.0       # explicit type and default
  cursor: i32 {8, wrap} # ranged integer field; implicit default is 0
  taps: f32[4]          # fixed array, default-filled
```

A bare field defaults to `f32`. A field with `= expr` infers its type from that compile-time
default. Typed scalar and tuple fields accept compatible defaults. Array and nested-struct fields
are initialized from their type's defaults; supply explicit values through constructor arguments.
An `i32` or `i64` field may use an [integer storage domain](#integer-storage-domains).
A ranged field defaults to `0` unless specified otherwise, then follows the same
normalization rules as a ranged variable.

Construction:

```onda
init:
  a = Settings()
  b = Settings(0.25, true, 0.5)
  c = Settings(gain = 0.75)
  d: Settings
```

- Constructor arguments bind fields positionally or by name. Omitted fields use their defaults.
- Struct constructors and typed struct declarations also work in runtime locals.
- A declaration such as `d: Settings` initializes with the default constructor.
- For generic structs, typed declarations require explicit type arguments when the type is still generic.
- Generic struct type parameters may be used in tuple fields; every tuple element must resolve to a
  primitive type before specialization completes.

### Methods

A method declares `self` as its first parameter and uses it to read or write the
struct's fields. Call it with `voice.tick(hz)` or the equivalent explicit form
`Voice.tick(voice, hz)`. Methods follow the [function rules](#functions), including
arguments, defaults, returns, overloads, and type parameters. Using `Voice` above:

```onda
init:
  voice = Voice()

sample:
  voice.tick(440.0)
  out1 = voice.sig
```

### Copies, aliases, and lifetimes

Structs and fixed arrays can share storage or own independent copies. The binding
form determines which:

| Form | Result |
| --- | --- |
| `alias = existing` for a new name | Alias the existing struct or fixed array. |
| `copy: T = existing` | Create independent fixed storage and copy the contents. |
| `existing = replacement` | Replace the contents without redirecting existing aliases. |
| `result = make_value()` | Own the fixed result returned by the helper. |
| `view: T[] = existing[start:end]` | Capture a slice view; no element storage is copied. |

A typed storage declaration must introduce a new name, including for specialized
generic types. This example shows how an alias and a copy respond to mutation:

```onda
struct Coefficients:
  gain = 1.0
  bias = 0.0

def prepare(gain) -> Coefficients:
  return Coefficients(gain = gain)

def reset(value: Coefficients):
  value = Coefficients()

sample:
  current = prepare(0.25)
  alias = current
  saved: Coefficients = current
  reset(alias)                      # Replaces current; saved stays independent.
  out1 = saved.gain + current.gain
```

Copies and replacements require the same nominal struct type. Tuple fields are
values. Constructor arguments capture their contents in source order, including
named arguments. Fixed primitive and struct arrays support the same copying and
return rules; fixed function parameters retain their declared shapes. Write
permission is inferred through aliases and calls, but copying needs only read access.

Bind a returned aggregate before indexing or selecting fields; use intermediate
aliases for deeper selections. An alias never extends its source's lifetime.
Init selections keep their captured coordinates until reinitialization. Block-owned
data and views survive process segments, intervening events, and snapshots; changing
the original selector does not redirect them. Each processor owns its own backing
storage and selections.

A typed slice initialized from fresh array data retains that backing for its scope.
Init backing follows its root's [initialization policy](#reinitialization-and-pinned-state).
Views cannot themselves be pinned. Copy into longer-lived fixed storage to retain
data beyond a temporary owner's lifetime.
Slices of [external buffer memory](#access-and-metadata) cannot cross process calls;
[message payloads](#structured-events-and-delegates) are borrowed only during
synchronous dispatch.

### Struct arrays

Initialize struct arrays with type defaults, a list, or one constructor broadcast
to every element:

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

These declarations also work in runtime scopes. Broadcast captures its initializer once and fills
independent elements. An explicit list must contain exactly the declared number of elements;
`[Marker()]` is a one-element list. Selecting an element produces an alias to that element, so the
compound assignment above updates `listed[1].value`. Runtime selectors are clamped just like
primitive-array selectors.

Primitive and struct slices can be explicitly typed. The annotation binds a view and requires an
initializer. Bounds are captured when the view is selected, and aliases preserve source permissions.

```onda
sample:
  markers: Marker[4] = Marker(value = 2.0)
  selected: Marker[] = markers[1:]
  markers[1:] = markers[:3]     # Overlap-safe contents copy.
  selected[:] = Marker(value = 5.0)
  saved: Marker[2] = markers[-2:]  # Exact length is proven at compile time.
  out1 = saved[0].value
```

Slice assignment copies the fitting prefix and preserves the destination tail. Struct fill
captures its element once, including for an empty destination. Existing slice bindings cannot be
rebound; use `view[:] = source` to copy contents. Fixed initialization from a slice requires a proven
matching length and never adds a runtime shape assertion.

### Indexed struct-array field access

Select a struct-array element, then read or write its fields:

```onda
struct Note:
  velocity = 1.0
  taps: f32[4]

init:
  notes: Note[4]

sample:
  i = 0
  notes[i].velocity *= 0.5
  out1 = notes[i].velocity + notes[i].taps[0]
```

These forms work in every executable scope. After the first struct-array selection,
assignment targets may continue through fields and indices, for example
`state.phases[i].settings.bands[j].gain += amount`. Selectors evaluate once, left to
right, before the assigned value. Scalar, tuple, struct, and fixed-array fields
follow their ordinary replacement rules.

Array fields accept runtime indices. Tuple fields require compile-time integer
indices and preserve each component's type. Write `current.notes[0].velocity`;
`current.notes.velocity[0]` does not select a struct element.

Expression reads allow one inline field-access dot after the array selection.
For a deeper structure such as `voices[i].settings.level`, first bind
`voice = voices[i]`, then read `voice.settings.level`.

The root must be named storage. Bind a returned struct before selecting its fields;
`current().notes[0].velocity` is invalid. A constructor can still be passed directly
as a [function argument](#parameter-forms-and-defaults), as in `apply(Settings())`.

See [Processor arrays](#processor-arrays) for indexed processor calls and members.

## Enums

A fieldless enum names a finite set of distinct values:

```onda
enum FilterMode:
  LowPass
  HighPass
  BandPass {label = "Band-pass"}

const DefaultMode: FilterMode = FilterMode::LowPass

params:
  mode = DefaultMode

sample:
  if mode == FilterMode::BandPass:
    out1 = 1.0
  else:
    out1 = 0.0
```

Declare enums at the file root or inside a namespace, including namespace templates.
Each has at least one uniquely named member. Members receive dense, zero-based `i32`
ordinals in declaration order; explicit ordinals and associated data are unsupported.
Enums follow ordinary declaration order, imports, aliases, and `use` lookup.

Each enum is a distinct type. It supports assignment, `==`, and `!=`; different
enums cannot mix. Enums work in constants, state, arrays, slices, scalar tuples,
struct fields, function arguments and returns, and event/delegate payloads.
Fields can infer their type from an enum default.

| Form | Meaning |
| --- | --- |
| `FilterMode::LowPass` | A named member. |
| `i32(mode)` | Extract the ordinal for arithmetic, ordering, or numeric builtins. |
| `FilterMode(index)` | Accept `i32` or `i64`, clamp to the member range, then narrow to `i32`. |
| `FilterMode()` | Select the first member; also the omitted scalar initializer. |

A top-level enum parameter becomes a dropdown, including each element of a fixed
parameter array. Its ordinal range and unit step come from the enum; a separate
range, scale, curve, unit, or step is invalid. Labels split identifier words
(`LowPass` becomes `Low Pass`), with an optional member `label` override.
A one-member enum has ordinal zero and a disabled dropdown. Audio ports do not
accept enums.

Enums also work in [configuration constants](#configuration-constants).
Parameter reads, event preparation, and snapshot restoration enforce
valid ordinals, including nested fields and array elements. Host metadata supplies
the qualified enum name and ordered member names/labels; binary values remain `i32`.
Labels are compile-time text, not runtime strings.

## Processors

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

A proc uses the same `ins`, `params`, `outs`, `kouts`, `init`, `sample`, and
`block` forms as the top level. A processing proc has a
`sample`, `block`, or `graph` section. A proc may instead omit all three and
serve through its state, resources, events, delegates, tasks, and helper defs.
Such an event-only proc can be constructed and receive messages, but cannot be
stepped with `instance(...)`. Any declared audio outputs start at their implicit
zero value.

### Proc inputs, params, and outputs

Proc sections use the same syntax as the top level, with these
differences:

- `kins` is not valid inside a proc; proc parameter sections are always `params`.
- A processor declares either `outs` or `kouts`, not both.
- `kouts` processors use `block` with no nested `sample`, cannot declare `ins`, and cannot declare `graph`.
- Undeclared `paramN` and `koutN` uses infer numbered proc params and control outputs respectively;
  procs do not infer `kinN`.

Processors can also declare [external buffers](#external-buffers). Their constructor
binding rules are covered under [Processor buffer bindings](#processor-buffer-bindings).

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

### Constructing and calling procs

Construct a persistent processor in `init`, using named arguments to set its
parameters. Call the instance to step it once; a single-output processor returns
that output:

```onda
init:
  g = Gain(g = 0.5)

sample:
  out1 = g(in1)
```

Destructure a multi-output proc call to step it once and bind every output in
declaration order. For a previously constructed `stereo` instance with two inputs
and two outputs:

```onda
sample:
  out1, out2 = stereo(in1, in2)
```

The number of targets must exactly match the processor's output count. This
form also works for nested processors and indexed processor arrays. A dynamic
array index is evaluated once; the selected processor is stepped once, then
its needed outputs are read. Use `_` for an output that does not need a binding.

Other access forms are:

| Form | Effect |
| --- | --- |
| `g(in1).out1` | Step the processor and select its output. |
| `g.g = 0.25` | Update a stored parameter without stepping. |
| `g.out1` | Read the most recent output without stepping. |

Positional call arguments bind inputs; named arguments bind inputs or parameters.
Parameter arguments are stored before the call runs. Array parameters retain their
shape, including length one: pass `gain = [0.5]` or an array binding for a
`gain: f32[1]` parameter. Multiple processor calls in an expression evaluate in
source order. Named parameter arguments are not supported inside `&&`, `||`, or
`while` conditions.

Every ranged processor-parameter write is clamped once before storage, including
construction, the builtin `init(...)` event, named call arguments, and direct
assignment. Floating NaN maps to the range minimum; reads use the stored value
without reclamping.

Call a sample-rate processor only from sample-rate code. Call a block-rate
`kouts` processor from block code or a [task](#tasks), and read its `kout1` or
named control outputs.

### Proc-local defs

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

These helpers can read and write processor state without `self`. Call them from
the processor's `init`, `block`, `sample`, event handlers, tasks, or other local
functions. They follow the ordinary [function rules](#functions), but cannot be
overloaded or called from outside the processor.

### Private params

Use `private` to hide a processor parameter from direct external access.

```onda
proc Filter:
  params:
    private cutoff = 1000.0
    private q = 0.707
```

The constructor and builtin `init(...)` event can set private parameters. Inside
the processor, its executable code and local functions can read and assign them.
Outside, direct reads, writes, array-element access, and named call arguments
such as `child(cutoff = ...)` are rejected. A processor with any private parameters
also rejects external `child.params[i]` access.

`private` is valid only as a processor-parameter prefix.

### Param update hooks

Attach an update hook to a primitive scalar processor parameter with
`=> hook_name`:

```onda
proc Voice:
  params:
    freq = 440.0 {20.0, 20000.0} => update_freq

  init:
    phase = 0.0
    phase_inc = 0.0

  def update_freq():
    phase_inc = freq / SR

  sample:
    out1 = sin(phase * TWO_PI)
    phase += phase_inc
    if phase >= 1.0:
      phase -= 1.0
```

Changing `freq` recalculates the phase increment; each sample uses the stored result.

Hook rules:

- The hook target must be a zero-parameter proc-local `def` in the same proc.
- The hook must have no explicit return type and no `return`.
- Hooks run immediately after each parameter is clamped and stored; updates are not batched.
- Construction and builtin `init(...)` run hooks after the processor's `init` body,
  in parameter declaration order.
- Hooks may read the owner's parameters, update its state declared in `init`, and
  assign named parameters on child processors.
- Hooks cannot assign the owner's parameters, inputs, or outputs; access child I/O
  or internal state for assignment; write child `params[i]`; or call child events.

Use a hook when one parameter changes derived state. Use an explicit processor
event or setter when several parameters should rebuild shared state once.

### Processor arrays

Arrays of proc instances are supported in `init`.

```onda
proc Voice:
  params:
    level = 0.0

  event set_level(value: f32):
    level = value

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
- Broadcast construction: `voices: Voice[4] = Voice()`.
- Compile-time capacity expressions in the array length.

The selected `Voice` is stepped once and produces `0.5`. With the same declarations,
this alternative sample body shows parameter writes, event calls, and cached output:

```onda
sample:
  voices[selected].level = 0.25
  voices[selected].set_level(0.5)
  voices[selected]()                  # Step the selected voice once.
  out1 = voices[selected].out1        # Read its result without stepping again.
```

- Runtime indices are clamped to the valid slot range.
- Aliasing such as `v = voices[i]`, then `v(...)`, is supported.
- An alias captures its selector once at creation; later changes to `i` do not select another voice.
- Arrays nested inside processors retain the same capacity behavior as top-level arrays.
- A proc cannot directly instantiate its own type in its own state.

For an audio-rate proc with a `block` section, the statements before its nested
`sample` run on the first `()` call to that array element in the current block.
The statements after `sample` run once at block end for each element that was
called. Selecting an element without calling it does not run either set of statements.

Helper parameters can borrow processor arrays and selected instances, including nested children
with owner `when` handlers. Forwarding and branch-selected aliases
preserve the selected state, lifecycle hooks, resources, and callback owner. Several instances of
the same processor type share the helper implementation; their state remains independent.

```onda
def step_voice(voice) -> f32:
  return voice()

def step_bank(voices, index: i32) -> f32:
  return step_voice(voices[index])
```

### Entry processor: `Main`

An entry file may declare a non-generic `proc Main`. The compiler automatically
constructs one `Main` instance per host instance and connects its public
interface to the host:

```onda
proc Main:
  params:
    gain = 0.5 {0.0, 1.0, unit = "x"}

  init:
    state = 1.0

  def next():
    state = state * 0.999
    return state * gain

  sample:
    out1 = next()
```

`Main` follows ordinary processor rules. Imports, constants, configuration
constants, structs, and namespaces stay outside it.

Public params become host controls with their declared defaults and metadata.
Private params remain internal. Host values initialize the instance and refresh
its public params at the start of each logical block, before its `block` code runs,
and before each exported event. These are ordinary proc writes: they clamp and run
bound hooks, including for equal values. Intermediate process segments do not
refresh the params. `Main` may write its own params; those writes last until the
next host refresh and do not update host controls. Use state declared in `init` or
private parameters for persistent internal decisions.

Input/output arrays, buffers, authored events, and delegates retain their public
names and shapes. Host initialization uses the ordinary proc construction path;
the builtin proc `init` event is not an additional exported event. An event-only
`Main` is constructed without being stepped, and any declared audio outputs remain
zero.

Only one unqualified `Main` may occur across the entry and its included files.
It cannot be mixed with bare top-level interface or executable sections. Imported
modules cannot supply an entry. A `Main` inside a namespace remains an ordinary
reusable processor with no automatic entry behavior.

## Events and delegates

Events carry commands inward, while delegates and printing carry sparse
occurrences outward. All of them execute synchronously on the processing thread;
they are not background callbacks.

### Top-level events

Top-level events are handlers the host calls on an initialized instance. Use them
for musical gestures, one-shot control changes, and stateful commands.

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

Use `event` to declare a single handler:

```onda
event bang():
  gate = true
```

This is equivalent to an `events:` block with one event. Singular `event ...`
declarations and an `events:` block can be mixed in the same owner.

Event parameters are read-only and use `f32` when untyped. See
[Structured events and delegates](#structured-events-and-delegates) for supported
payload types, defaults, and copying.

Handlers can update existing state declared in `init` and read, write, or query
currently bound buffers. They cannot access audio I/O or write top-level parameters.
Hosts dispatch events by index: unknown indices are ignored, while a known event
with the wrong payload size is a runtime error.

### Proc events

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
- Parameters follow the shared [message payload rules](#structured-events-and-delegates).

Every proc also gets a reserved builtin `init(...)` event. It mirrors the proc
params in declaration order and adds `full: bool = false`, assigns
provided values into params, reruns that proc instance's `init`, then runs bound
param hooks. Omitted arguments use defaults. The call forwards the proc instance's
current buffer bindings, so it can explicitly refresh state derived from a
buffer that the host rebound after construction.

By default the call preserves pinned roots while reinitializing resettable
roots. Passing `full = true` performs the full initialization used by fresh proc
construction. The effect on task continuations is covered in
[Tasks](#tasks), and both initialization modes are detailed in
[Reinitialization and pinned state](#reinitialization-and-pinned-state).

```onda
voice.init(0.5)
voice.init(gain = 0.5)
voices[i].init(freq = 220.0, amp = 0.1)
voice.init(full = true)
```

These call forms assume the corresponding `voice` or `voices` instance was
constructed in the surrounding program.

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

Delegates share event [parameter and payload rules](#structured-events-and-delegates),
including generic specialization and argument binding. Calls have no result. Only
the declaring owner can emit a delegate; `child.finished()` is invalid.

Emit from `sample`, `block`, tasks, event and `when` handlers, or owner-local
functions. Emission is forbidden in `init` and in functions reachable from `init`.

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
whole fixed proc array. Here `voices` is an array of `Envelope` instances, and
`first_reason` and `voice_reason` are state variables declared in `init`:

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

Host delivery has block-level timing. Within Onda, processor delegate handlers
run immediately and can respond at the exact sample where a delegate is emitted.

### Structured events and delegates

Events and delegates share these parameter forms:

| Payload | Forms |
| --- | --- |
| Scalar | Primitive or enum type; an omitted type defaults to `f32`. |
| Tuple | Scalar components. |
| Struct | A nominal type, including nested fixed fields. |
| Array or slice | `T[N]` or `T[]`, with scalar or struct elements. |

Constant defaults are allowed for scalars, tuples, and fixed primitive arrays.
Slices, structs, and struct arrays require an argument. A no-argument event can
construct and forward default data explicitly, as `reset_bank` does below.

All payload parameters are read-only. Their declared shape is preserved through
processor events, delegates, and `when` bindings:

```onda
struct Note:
  frequency = 440.0
  velocity = 1.0

struct Patch:
  notes: Note[4]
  transpose: f32 = 0.0

proc VoiceBank:
  init:
    current: Patch

  delegate configured(patch: Patch)

  event configure(patch: Patch):
    current = patch          # Copy into proc-owned state.
    configured(current)      # Borrow that state while dispatching.

  sample:
    out1 = current.notes[0].velocity

init:
  bank = VoiceBank()
  observed: Patch

when bank.configured(patch):
  observed = patch           # Retain independent contents after dispatch.

event configure(patch: Patch):
  bank.configure(patch)      # Forward the same read-only payload view.

event reset_bank():
  defaults: Patch
  bank.configure(defaults)   # Explicit storage replaces a parameter default.

sample:
  out1 = bank() + observed.transpose
```

Aggregate payloads are read-only views valid for synchronous dispatch. Forwarding
or `local = payload` aliases those views. To retain contents, assign into existing
owned storage or introduce a typed copy such as `local: Patch = payload`.

A struct slice carries a runtime length; a fixed struct array requires its declared
length. Host input follows the recursive [message schema](https://onda-lang.org/docs/api/#messages-and-tensor-views) and normalizes
integer field domains before dispatch. Struct arrays use one tensor per scalar
field; a struct slice adds one logical length before those tensors. In-language
forwarding borrows live data; published host delegate records snapshot the contents.

### Printing

`print` sends diagnostic output to the host. It accepts an optional leading quoted
label followed by zero or more primitive scalar values:

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

## External buffers

Buffers are host-bound sample data. Unlike ordinary arrays, their storage and
runtime dimensions come from the host, and rebinding remains visible to later
processing calls.

### Declarations and channel layout

Inside `buffers`, declare the element type and channel layout. Brackets describe
the channels in one buffer, rather than an Onda array:

```onda
buffers:
  mono: f32
  stereo: f32[2]
  dyn: f32[]
```

- `f32` is a mono buffer.
- `f32[2]` is a buffer with exactly two channels.
- `f32[]` is a buffer with any positive runtime channel count.

The explicit `buffer<...>` form is also accepted and is useful when naming a
buffer type, such as in a function parameter:

```onda
buffers:
  src: buffer<f32>
  bus: buffer<f32[2]>
  any_bus: buffer<f32[]>
```

`buffer<f32[]>` accepts mono and exact-channel buffers. An exact type such as
`buffer<f32[2]>` does not accept a dynamic-channel buffer.

### Collections and shorthands

Buffers support [section count and type shorthands](#section-shorthands).
Attribute braces declare a collection of separately bound buffers:

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

### Access and metadata

| Query | Result for one buffer |
| --- | --- |
| `.len()` | Total scalar elements across channels. |
| `.samples()` | Samples per channel. |
| `.chans()` | Channel count. |
| `.samplerate()` | Buffer sample rate. |
| `.bound()` | Whether a real host binding is present. |

`buf.len() == buf.samples() * buf.chans()`: a stereo buffer with four samples per
channel has eight elements. On a collection, `.len()` instead returns its slot
count; select `bank[i]` before using individual-buffer queries.

A buffer slice selects one channel and has that channel view's length. Multichannel
buffers require an explicit channel selector. Ordinary arrays and slices have no
channel dimension.

The access forms are deliberately limited to one coordinate pair per selected buffer:

| Declaration | Sample access | Slice access |
| --- | --- | --- |
| `mono: f32` | `mono[sample]` | `mono[start:end]` |
| `stereo: f32[2]` | `stereo[channel, sample]` | `stereo[channel, start:end]` |
| `bank: f32 {N}` | `bank[slot][sample]` | `bank[slot][start:end]` |
| `layers: f32[2] {N}` | `layers[slot][channel, sample]` | `layers[slot][channel, start:end]` |

`bank[slot]` and `layers[slot]` select a first-class buffer and can be passed to a function or used
for metadata queries. A channel alone is not a first-class view; use the channel-and-sample or
channel-and-slice forms above. The flattened `layers[slot, channel, sample]` form is not supported.

### Aliases and interpolation

An alias names a buffer or selected collection entry without copying its data:

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
sample boundary and are intended for cyclic tables and loopers.

### Indexing and binding rules

All source-level coordinates clamp independently. In `stereo[channel, sample]`, for example, the
channel clamps to the channel range and the sample clamps to the sample range before the address is
formed. Fixed buffer-collection selectors likewise clamp and select a descriptor in constant time.
The compiler removes that normalization when it can prove the complete coordinate range is valid.

For proven-valid coordinates, see [Unchecked indexed access](#unchecked-indexed-access).

- Explicit declarations and count shorthand cannot be mixed in one `buffers` block.
- Exact channel counts are compile-time constants; dynamic counts come from the bound instance.
- Runtime binding validates element type and channel constraints. Each fixed-array slot binds
  independently and may be omitted.
- Host metadata names physical collection slots `bank[0]`, `bank[1]`, and so on, while separate
  collection metadata preserves the logical `bank` name and its contiguous slot range.
- An unbound slot has one sample per channel: reads return the element type's zero and writes
  are discarded. `.samples()` is `1`, `.len()` equals `.chans()`, and `.samplerate()` is the host
  sample rate. `.chans()` is the exact declared channel count or `1` for a dynamic-channel declaration. `.bound()` is `false`; all
  other valid bindings report `true`.
- Binding with a zero sample rate unbinds the buffer; the pointer and dimensions are ignored.

### Processor buffer bindings

Processors declare `buffers` with the same syntax as the top level. Buffer arguments
to their constructors are named-only.

- A scalar proc buffer accepts one buffer or a selected collection slot, such as
  `clip = bank[2]`.
- A fixed proc buffer collection requires the same count. A larger collection can be passed only
  through an exact, compile-time subspan such as `clips = bank[1:7]`; both bounds are checked at
  compile time, and the descriptors are forwarded without copying their sample data.

These bindings also work for processor arrays and nested children passed through
helper functions. Forwarded references retain the constructor-bound resources and
resolve them through the current host buffer bindings, so later rebinding remains
visible.

### Buffer binding and initialization

Initialization can read, write, and inspect currently bound buffers, including
buffers supplied to a processor constructor. A later host rebind is visible to
subsequent processing, event, and init calls. It does not rerun initialization or
refresh state derived from the old binding; request initialization explicitly
when that derived state needs rebuilding.

## Parameter domains

Parameter braces describe the range and optional host-control metadata for scalar
parameters and [parameter arrays](#parameter-arrays).
Positional fields are ordered as
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
`{max}` is the maximum-only shorthand. `scale` defaults to
`linear`; the other optional fields default to absent.

`scale`, `curve`, `unit`, and `step` describe external control of `params`
(and their top-level `kins` alias). Processor params can carry the same
attributes; entry exposure applies host domain and descriptor requirements.
They are not available on inputs and do not change ordinary proc stores or
the Onda DSP calculation:

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

## Smoothing

Input smoothing is part of the DSP program. Parameter smoothing is chosen by the
host, or implemented explicitly in DSP with a library processor.

### Input smoothing

Input attribute braces also accept named-only `ramp` and `lag` durations in seconds:

```onda
ins:
  freq = 440.0 {20.0, 20000.0, ramp = 0.02}
  cutoff = 1200.0 {min = 20.0, max = 20000.0, lag = 0.05}
  modulation: f64[2] = [0.0, 1.0] {lag = 0.01}
```

These attributes work on top-level and proc inputs of type `f32` or `f64`, including
fixed-size arrays. They are mutually exclusive, even when one duration is zero.
Durations must be finite, nonnegative compile-time constants. An absent attribute or
a zero duration creates no smoother instance or smoothing state.

A positive duration inserts an ordinary `std::smoothing::Ramp<T>` (linear) or
`std::smoothing::Lag<T>` at the input boundary; no explicit import is needed.
Each input is clamped to its declared range first, then smoothed once before each
execution of the sample body. Every read, including dynamic and unchecked indexed reads,
uses the same cached value. Each array element and proc instance has independent state.
Smoothing advances at the sample body's effective rate, including oversampling; proc
smoothers advance when that proc is stepped. Block boundaries and process segments do
not restart smoothing. Initialization seeds each smoother from the input's clamped
default, or clamped zero when no default is declared; snapshots include its state.

`ramp` reaches a stable target in the specified duration. Each changed target restarts
that duration from the current output. `lag` uses the exponential time constant, reaching
approximately 63.2% of a step after the specified time.

### Parameter smoothing

Parameter declarations do not specify smoothing. The compiler reads the values supplied by the
host and generates no parameter ramp state. `onda run` and the browser playground apply host-owned
30 ms linear ramps to continuous floating-point controls, with independent ramps for array elements.
This is **block-level smoothing**: values advance once per logical block and remain unchanged across
its process segments. Durations at most one block are bypassed; integer, boolean, and stepped
controls update directly. Initial values, parameter reset, and processor restart settle immediately.
Other hosts choose their own policy. Use `std/smoothing` or the smoothed gain processors in
`std/gain` when an audible response should be expressed in the DSP program.

See the [smoothing module](https://onda-lang.org/docs/stdlib/#stdsmoothing)
for the available processors.

## Execution options

The basic [execution model](#execution-and-state) also supports higher sample rates
and selective reinitialization of persistent state.

### Oversampling

Use `sample N:` to run at a multiple of the base sample rate.

```onda
ins 1
outs 1

params:
  drive = 8.0

sample 4:
  out1 = tanh(in1 * drive)
```

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

### Reinitialization and pinned state

Hosts can reinitialize an existing instance in two modes:

| Mode | Effect |
| --- | --- |
| `PRESERVE_PINNED` | Rerun ordinary declaration initializers and every explicit init statement; retain pinned roots and task continuations. |
| `FULL` | Also initialize pinned roots and task continuations. Required for a fresh instance. |

Both modes operate on the instance's state without allocating on the successful
path. Their cost depends on the authored initializer; a runtime failure leaves
state indeterminate. Initialized creation writes parameter defaults and runs
full initialization. [Buffer bindings](#buffer-binding-and-initialization) are
those current at the call.

A direct persistent binding can opt out of preserve-pinned initialization with the `pin`
modifier:

```onda
init:
  pin prepared: f32[4096]
  pin generation: i32 = 0
  history: f32[128]
```

Preserve-pinned initialization retains `prepared` and `generation` while
reinitializing `history`. Full initialization initializes every binding. Snapshots
restore the captured values regardless of pin policy. Integer-domain attributes remain
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

## Tasks

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

### `await` and scheduling

The owner advances a task with `await` in the part of `block` before its nested
`sample`, including inside conditionals or loops there. A program or processor
with only control outputs has no nested `sample` and can use `await` throughout
its `block` body.

If the task yields or has failed, the owner stops that activation and produces
neutral outputs. For a top-level task those are the program outputs; for a proc
task only that proc becomes neutral and its parent continues normally. If the
task completes, execution continues after `await` in the same logical block. A
task is reset explicitly with `prepare.reset()` from the owner's `init`, event,
`block`, or `sample` code.

Each time the owner's block processing starts, it runs from the beginning,
so statements and conditions before an `await` are evaluated again. Only the task
resumes from its suspension point. Completed tasks fall through without rerunning;
the first reached task that yields stops the activation. An incomplete task has
no effect when ordinary control flow bypasses its `await`, leaving the program
responsible for not exposing partially prepared state.

Each proc instance, including each element of a proc array, owns independent
task continuations. An explicitly called block-rate proc runs its block body,
including any reached `await`, on every call just like other block-rate proc
code. For statically scheduled sample-rate proc instances, statements before the
nested `sample` run at most once when the logical block begins. For a runtime-indexed
proc-array element, those statements run on its first sample-rate call in that block.
Splitting a logical block into process segments never grants additional
resumptions to those scheduled activations, and a zero-frame begin-block
segment still advances statically scheduled tasks.

### Continuations, reset, and reinitialization

Task continuations are compiler-pinned state. Preserve-pinned initialization preserves
them. Proc `init(full = true)` and full host initialization restore them to
not-started. An explicit `prepare.reset()` in an initializer always runs. Tasks
may use both pinned and resettable state; after default initialization, a
suspended task observes the reinitialized resettable values when it resumes.
Snapshots include task status and continuation storage, so restoring a suspended
task resumes it from the captured suspension point.

`reset()` invalidates the continuation in constant time. It does not eagerly
clear task-frame storage: restarting the task executes its declarations and
initializers before that storage can be observed again. Full initialization
still initializes the complete continuation image.

Locals that are live across a `yield`, including fixed structs, arrays, and loop
control, become statically allocated continuation state. Data views may cross `yield`
when their backing storage survives suspension: the continuation retains the selected
coordinates and branch choice, then reconstructs access on resumption. This includes
views into task-owned data, owner state, and persistent init selections. A typed slice
initialized from fresh array data retains its backing through suspension.

Views into external buffer memory cannot cross `yield`, because the buffer may be
rebound before resumption. Other resource handles retain their existing restrictions;
reacquire them after resumption if needed. References no longer used after
`yield` do not prevent suspension.

Tasks read owner params and current buffer mappings whenever they resume.
Changing a parameter or rebinding a buffer does not reset a task automatically;
the program must call `reset()` when previously prepared or partially prepared
state is no longer valid.

A runtime failure reports through the failing process call and invalidates the
processor state. Hosts must emit silence and reject further stateful operations
until full initialization or snapshot restoration succeeds. This is the same
fail-closed behavior as a runtime failure outside a task.

Tasks are private to their owner and share that owner's declaration namespace.
They cannot be used with a `graph` block. Neither `await` nor task `reset()` is a
first-class callable value; reset cannot be called inside tasks or functions.

## Graphs

`graph` connects processor instances with signal-flow edges. Create the instances
in `init`, then describe their connections. An owner uses either `graph` or
`block`/`sample` processing.

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

An edge can also compute a value. This oscillator feeds its own output back into
its frequency parameter at sample rate:

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

### Edges and scheduling

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

Edges to processor parameters default to `@block`; other edges default to
`@sample`. An explicit rate overrides the default. Edges that read audio inputs
or processor outputs require `@sample`, including edges targeting parameters.
The oscillator example above uses an explicit `@sample` for this reason.

Delayed edges use `>>[expr]` or `<<[expr]`, where `expr` is a nonnegative
compile-time integer. Delays are sample-rate only.

Each destination has one writer; one source may feed several destinations.
Sample-rate cycles between different processors require a positive sample delay.
Self-feedback reads the processor's cached output from its previous step.
Processors are stepped automatically according to graph reachability and dependency
order. Inspect the generated schedule with `onda compile <file> --dump-graph`.

### Sources and destinations

| Role | Allowed values |
| --- | --- |
| Sources | Top-level inputs and parameters; processor outputs, including processor-array elements; array literals, whole-array reads, indexed reads, and slices; arithmetic, logical, and element-wise expressions built from these values. |
| Destinations | Top-level outputs; processor inputs and parameters, including processor-array elements. |

Array shapes must match the destination, except that a scalar may broadcast to a
fixed array. Source expressions cannot contain user-defined function or processor
calls. Typed array declarations are statements; use an array literal or an
existing array value as a source.

Graphs do not support top-level `kouts`, block-rate processor outputs, or event
propagation. Use ordinary `event` or `events` declarations for messages.

### Processor arrays and bundles

Proc-array slots use the same fixed-array port shapes as ordinary proc instances. The processor
index follows the instance name; an optional second index selects one element of an output array:

```onda
graph:
  stereo_in >> processors[1].input
  processors[1].output >> stereo_out
  processors[1].output[0] >> left_out
  processors[1].output[1] >> right_out
```

`processors[1].output` reads the complete cached output array after that slot has
been stepped. Fan-out reuses that result without stepping the processor again.
Numbered outputs also work, for example `processors[1].out1`.

Bare processor instances and array elements can route into destination sets.
Outputs bind to destinations in declaration order when counts match; a
single-output processor broadcasts to every destination. Other count mismatches
are errors.

Given previously constructed `reverb` and `voices` nodes, proc bundles use the
same routing syntax:

```onda
graph:
  reverb >> { out1, out2 }
  voices[0] >> { left, right }
```

## Modules and namespaces

### Imports

Use `import` to load another module:

```onda
import reverb
import std/osc
import std/filter
```

- `import module/path` resolves as `module/path.onda`.
- Each imported file is imported once.
- Built-in std modules are available under `std/...`.
- Imported files contain declarations: constants, enums, structs, functions, processors, namespaces, and `use`.
- `std/prelude` is imported automatically and loads `std/math`, `std/lookup`, and `std/random`.

See the [standard-library reference](https://onda-lang.org/docs/stdlib/) for module APIs.

### Includes

`include` inserts another source file by quoted path:

```onda
include "shared/reverb.onda"
include "shared/util.on"
```

Paths must end in `.onda` or `.on` and use `/` separators.

Native filesystem entry, import, and include paths must not traverse symbolic
links. The loader reports the offending component. Virtual sources and immutable
project images do not have this restriction.

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

### Use declarations

`use` brings namespace members into unqualified lookup. It does not load
modules; use `import` first when the target lives in another module.

```onda
import std/math
import std/random
import std/osc

use std::math
use std::random::Rng
use std::osc as osc
use std::osc::Sine as SineOsc
pub use std::lookup

ins 1
outs 1

init:
  rng = Rng()
  a = osc::Sine()
  b = SineOsc()

sample:
  out1 = clamp(in1, -1.0, 1.0)
```

Forms:

- `use Namespace` brings direct declarations in that namespace into unqualified lookup.
- `use Namespace::Symbol` brings one declaration into unqualified lookup.
- `use Namespace as Alias` creates a namespace alias.
- `use Namespace::Symbol as Alias` creates a symbol alias.
- `pub use ...` re-exports the use declaration through imports.

Place `use` at the top level or inside a namespace. `as` applies to the whole
declaration. A plain top-level `use` is private to its source file; only `pub use`
is exposed to importers. If imported names collide, use a fully qualified path
to disambiguate them.

## Compile-time programming

Compile-time code computes constants, dimensions, and prepared data before audio
processing begins. The basic [constant declaration](#constants) also supports
arrays and host-selected configuration.

### Constant arrays

```onda
const Scale: f32[3] = [0.5, 1.0, 2.0]
const MoreScale: f32[] = [0.25, 0.5, 1.0, 2.0]
```

`T[N]` fixes the length; `T[]` infers it from a literal, another const array,
a const-array slice, or an array-returning `const def`. Constant arrays are
immutable. Their length and compile-time-indexed elements can be used in
compile-time expressions. Pass them to runtime helpers under the ordinary
[read-only argument rules](#values-and-shared-storage).

### Configuration constants

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
  typed ordinary const: `bool`, `i32`, `i64`, `f32`, `f64`, or an enum, and fixed or inferred-length
  arrays of those types. Enum overrides use `i32` ordinals and clamp to the member range.
- Configuration constants are allowed only at the executable root. The entry file and its
  `include` files share that root; declaration modules loaded with `import` cannot declare them.
- A host override replaces only that declaration's initializer for one compilation. Derived
  constants, shapes, specialization, assertions, and generated code are recomputed normally.
- Fixed array lengths are resolved from the complete selected configuration. If `Channels` above
  changes, `Coefficients`—whether supplied by the host or evaluated from its default—must have the
  new length or compilation fails.
- Ordinary constants cannot be overridden. With no host input, the source initializer is used.
- Configuration arrays follow the same demand rules as ordinary arrays. Inspecting configuration
  values explicitly evaluates their selected initializers.

The native CLI accepts repeatable `--const Name=value` inputs using Onda literal syntax and
`--list-consts` prints the resolved configuration values. An `.ondaproject` may provide defaults
in its `constants` map; explicit CLI inputs override matching project values.

### Compile-time functions

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

- Every `const def` must declare an explicit return type and return a value; bare `return` is invalid.
- Forward references, direct recursion, and mutual recursion are rejected.
- Params support primitive scalars, fixed-size primitive arrays, typed primitive slices such as `f32[]`, and untyped slices `[]`.
- Typed slice params accept compile-time arrays of any positive length with the matching element type.
- Untyped slice params accept compile-time arrays of any positive length and primitive element type.
- Array and slice params reference their caller's storage. Writes update mutable arguments;
  const arrays and their aliases are accepted only by read-only callees.
- Array-returning bodies can use local fixed primitive arrays, indexed local-array reads/writes, `if`, `for`, `loop`, `return`, pure builtin math, and calls to earlier visible const defs.
- Compile-time loop evaluation is capped at 1,000,000 iterations per loop.
- Use scalar results in scalar constant declarations and fixed-array results in
  constant array declarations.
- Calls retain their declared scalar or array element type when evaluated. Untyped slice
  arguments complete body type and shape checks from their metadata, without computing values.
- Local array literals and array-returning calls create owned arrays and infer their element type
  and length. Literal elements use ordinary first-assignment defaults.
- Array aliases, fixed copies, and returns follow the ordinary
  [binding rules](#copies-aliases-and-lifetimes).
- Named const array arguments retain their metadata through forwarding, aliases, copies, and slices.
  Calling `.len()` does not evaluate their elements. Executed element reads and writes demand
  the initializer; independent copies share immutable payloads until a write.

### Evaluation and demand

Constants and const defs are checked when declared, even in unused library code.
The compiler and LSP report unknown names, invalid calls, incompatible known types,
and invalid known array shapes without computing array payloads. Checks that
depend on an untyped array parameter or generic type wait until a concrete use.

Scalar const initializers evaluate when declared. Array payloads evaluate when
required code reads their contents, and cache their value or failure for that
concrete declaration and namespace specialization. Calls inside an unused array
initializer do not execute. Array lengths remain available without its payload.

Constants, function signatures, and body layout metadata use declaration-time
compile options. Tuple selectors keep the index used for type checking; closed
slice bounds keep the context used for their shape proofs. Direct const-def calls
in executable value expressions use that owner's effective context. Runtime array
defaults cache their values separately for each used compile context. `HOST_SR` and
its aliases always retain the host sample rate, including inside const defs.

Const defs use ordinary local variables and execute when called. Untaken branches
inside const defs and short-circuited expression operands do not evaluate their
values. Ordinary runtime statement checking and declaration preprocessing remain
unchanged. Required metadata, such as dimensions and processor layout, is itself
a compile-time use and can demand an array even in an unused function body.

Evaluation is shared across array dependencies. An array is emitted as runtime
constant data only when a reference to it survives folding; `Scale[0]` can fold
to a scalar without retaining the array.

## Generics

Type parameters let functions, structs, and processors share an implementation
across scalar types. Integer namespace parameters specialize sizes and counts.
All specialization happens at compile time. Type arguments accept `f32`, `f64`,
`i32`, `i64`, and enum types; `bool` is not supported as a type argument.

### Generic defs

Runtime defs can declare type parameters:

```onda
def id<T>(x: T) -> T:
  return x

def pair<T>(x: T, y: i32) -> (T, i32):
  return (x, y)
```

Generic defs use [monomorphization](#specialization-and-monomorphization), just like
untyped functions. Type arguments can often be inferred:

```onda
sample:
  a = id(0.5)      # T inferred as f32
  b = id<f64>(1.0) # T provided explicitly
```

- Enum arguments infer their exact nominal type. Distinct enums, or an enum and a numeric type,
  cannot share one type argument; ordinal conversions remain explicit.
- A type param not constrained by any call argument defaults to `f32`; for example, `zero<T>()`
  called as `zero()` specializes `T` to `f32`.
- Scalar inference collects every concrete argument for a type parameter before adapting
  contextual arguments. It uses the [operand promotion table](#operand-promotion), regardless
  of argument order. For `combine<T>(a: T, b: T)`, an `i32` with a contextual integer selects
  `i32`, an `f32` with a contextual integer selects `f32`, and an `i64` with a floating
  argument selects `f64`.
- If every scalar argument is contextual, any floating argument selects `f32`; otherwise
  integer binding defaults select `i32` or `i64`. For example, `combine(2, 0.5)`
  selects `f32` because both arguments are contextual and one is floating.
- Explicit type arguments select the parameter type directly. Every scalar argument must
  then satisfy the ordinary destination-conversion rules, including constant range checks;
  selecting `T` does not permit runtime narrowing.
- Array and buffer arguments constrain a type parameter to their exact element type.
  Conflicting element types are errors. Scalar arguments sharing that type parameter must
  convert to the fixed type, and an explicit type argument must agree with it. Existing
  aggregate references are not converted element by element.
- A call's destination or expected return type does not select its generic type arguments.
  `result: f64 = id(0.5)` still calls `id<f32>` and then widens its result; use
  `id<f64>(0.5)` to select the wider specialization.
- Generic type params can appear in scalar params, array params, buffer element params, locals, casts, and supported return annotations.
- `const def` cannot declare type parameters.

### Generic structs and processors

The compiler creates a concrete struct or processor for each type combination used.

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

- Unresolved generic type params in declaration and type positions are errors.
- For untyped constructor assignments only, unresolved constructor type params default to `f32`.
- `T(expr)` rewrites to the bound scalar conversion, including clamped enum construction.
- `T[]` is valid for method and `def` array params where a primitive slice is valid.
- Typed generic locals such as `x: T = ...` are supported in executable scopes.

### Structural specialization

[Untyped parameters](#specialization-and-monomorphization) specialize without a
named type parameter. They accept scalars (including `bool`), tuples, arrays,
buffers, structs, and processors. `[]` and unsized processor-array parameters take
their shape from the call; fixed processor-array parameters require their declared
capacity. Numeric literals use ordinary binding defaults.

```onda
def first(arr: []):
  return arr[0]

sample:
  a = [1.0, 2.0]
  b = [1, 2]
  x = first(a)     # f32
  y = first(b)     # i32
```

A function can also accept any type with the fields or methods it uses:

```onda
struct A:
  x: f32

struct B:
  x: f32

def read_x(s):
  return s.x

sample:
  a = A(x = 0.25)
  b = B(x = 0.5)
  out1 = read_x(a) + read_x(b)
```

`read_x` accepts either `A` or `B`; each call is specialized for its concrete
argument. See [Parameter forms and defaults](#parameter-forms-and-defaults) for all parameter forms.

### Integer namespace parameters

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

- Namespace template params require defaults.
- Arguments may be positional or named and are normalized as `i32(...)` at compile time.
- Namespace-local `assert(expr)` performs compile-time checks.

Namespace aliases may retain compile-time arguments:

```onda
import std/data

namespace D = std::data<SR, 1>

init:
  a = std::data<SR, 1>::Data<f64>()
  b = D::Data<f64>()
```

`use` aliases can retain namespace arguments too:

```onda
import std/fft
use std::fft<512> as fft512
use std::fft<1024>::FFT as FFT1024
```

## Numeric conversions

Width selection, operand promotion, and destination conversion are separate
steps. These rules apply to runtime code and `const def` evaluation alike.

### Contextual values and defaults

Numeric literals and builtin numeric constants are contextual. An unannotated
numeric const whose initializer is contextual stays contextual through aliases
and folding. These values retain `i64`/`f64` evaluation precision until a use
selects a machine width. A pure numeric literal expression contains only these
values, numeric operators, and numeric builtins.

Annotations, explicit casts, concrete array elements, and typed function
results carry concrete types. Runtime bindings also keep the type chosen at
their first assignment; later uses do not infer it again. A concrete constant
participates in operand promotion with its existing type, even though its
known value may permit conversion at a typed destination.

A context can come from an annotation, an existing destination, a parameter or
return type, an interface or state type, an array element or struct field type,
a concrete operand, or generic specialization. With no such context, fresh
bindings use these defaults:

| Contextual value or expression | Default type |
| --- | --- |
| Floating | `f32` |
| Integer, with every contributing integer literal or untyped const value in the `i32` range | `i32` |
| Integer, requiring a wider literal or untyped const value | `i64` |

Inline integer expressions select this width before their arithmetic executes.
A named untyped const has already evaluated its initializer; its computed value
determines the default at a use. Each new tuple component uses its own default.
An untyped array binding selects its element type from its first element.

```onda
const Small = 64
const Large = 4294967297
const Precise = 16777217.0
const Fixed = f64(Precise)

init:
  small = Small      # i32
  large = Large      # i64
  rounded = Precise  # f32: 16777216.0
  fixed = Fixed      # f64: 16777217.0
```

### Operand promotion

Once operands have concrete widths, arithmetic uses this symmetric table:

| Left / right | `i32` | `i64` | `f32` | `f64` |
| --- | --- | --- | --- | --- |
| `i32` | `i32` | `i64` | `f32` | `f64` |
| `i64` | `i64` | `i64` | `f64` | `f64` |
| `f32` | `f32` | `f64` | `f32` | `f64` |
| `f64` | `f64` | `f64` | `f64` | `f64` |

Comparisons use the same operand type and return `bool`. Numeric branch joins
and scalar generic inference use the same promotion table. `bool` is outside
numeric promotion: logical operators require boolean operands, and numeric
values produce booleans through comparisons such as `value != 0`.

A contextual operand first adapts to its concrete peer. Integer literals can
enter either numeric family; a floating literal keeps its floating family.
For arithmetic and comparison operands, the resulting common type is:

| Contextual operand / concrete peer | `i32` | `i64` | `f32` | `f64` |
| --- | --- | --- | --- | --- |
| Integer | `i32` | `i64` | `f32` | `f64` |
| Floating | `f32` | `f64` | `f32` | `f64` |

Consequently, `integer_i32 / 10.0` uses `f32` division and
`integer_i64 / 10.0` uses `f64` division. Integer-only division stays integer
division. Reversing the operands does not change their common type.

Numeric builtins combine their concrete arguments first, then adapt contextual
arguments to that common type. A destination does not change the arithmetic
width inside a mixed expression containing concrete operands:

```onda
params:
  value: f32 = 16777216.0

sample:
  widened: f64 = value + 1.0  # f32 addition, then f64 conversion: 16777216.0
  wide = f64(value) + 1.0     # f64 addition: 16777217.0
```

To select narrow arithmetic for a concrete `i64` mixed with a float, cast the
integer operand: `f32(count_i64) + 0.5`. Casting the completed expression with
`f32(count_i64 + 0.5)` instead performs `f64` addition before conversion.

### Implicit destination conversions

Assignments, scalar arguments, defaults, returns, and scalar storage writes
accept exactly these implicit conversions for runtime values, including the
identity conversion:

| Source type | Allowed destination types |
| --- | --- |
| `i32` | `i32`, `i64`, `f32`, `f64` |
| `i64` | `i64`, `f64` |
| `f32` | `f32`, `f64` |
| `f64` | `f64` |
| `bool` | `bool` |

Integer-to-float conversion rounds when necessary. The term widening describes
the type relation, not preservation of every integer bit: `f32` has 24 bits of
integer precision and `f64` has 53.

Closed compile-time expressions additionally allow these destination conversions:

| Source family | Allowed destinations | Conversion |
| --- | --- | --- |
| Integer | `i32`, `i64`, `f32`, `f64` | Integer destinations check the computed value's range; floating destinations round normally. |
| Floating | `f32`, `f64` | Normal floating-point rounding. |
| Boolean | `bool` | No numeric conversion. |

This includes typed consts, compile-time-selected const-array elements, and
`const def` results. It does not include an ordinary runtime binding or an
ordinary `def` result merely because an optimizer could compute its value.
Floating-to-integer conversion requires an explicit cast, even for `7.0`.
Inside `const def`, parameters and local values also permit checked constant
conversions; their concrete types still control operand promotion.

Pure inline numeric expressions evaluate at the destination's selected width.
Expressions with concrete operands evaluate their arithmetic first, then
convert the result. The distinction also applies to typed const declarations.

Tuple destinations apply these rules independently to each component. Array
literals and scalar element writes use the destination element type. Existing
array, slice, and buffer references require matching element types; passing
one does not insert an element-by-element numeric conversion.

Range checking at a destination is distinct from integer arithmetic. Arithmetic
wraps at its selected integer width, including contextual operands converted
to that width:

```onda
const Huge = 4294967297

sample:
  zero: i32 = 0
  wrapped = zero + Huge  # i32 arithmetic: Huge wraps to 1
  # value: i32 = Huge   # rejected: the constant is out of range for i32
```

A function argument is a typed destination too: a generic call that selects
`i32` cannot accept `Huge` as a separate argument, whereas its `i32` arithmetic
can wrap during evaluation.

### Evaluation width and explicit casts

At the selected width, integer arithmetic wraps and floating arithmetic rounds
at each operation. Onda does not compute an `f32` expression through `f64`
intermediates. Unary minus preserves the selected numeric type; `~` requires
an integer. Neither supports `bool`.

Contextual numeric const initializers evaluate once at `i64`/`f64` precision.
Uses adapt the computed value rather than re-evaluating the initializer:

```onda
const Half = (2147483647 + 1) / 2

init:
  named: i32 = Half                        # 1073741824
  inline: i32 = (2147483647 + 1) / 2        # -1073741824: i32 addition wraps
  converted = i32((2147483647 + 1) / 2)     # 1073741824: i64 argument, then cast
```

Integer-only literal subexpressions inside floating arithmetic evaluate with
`i64` integer operations, then convert to the surrounding floating width.
For example, `value: f32 = ((16777216 + 1) - 16777216) + 0.0` yields `1.0`.

An explicit cast `TYPE(expr)` evaluates its argument in the argument's own
context, then converts its result. A pure numeric argument with no concrete
peer evaluates at `i64`/`f64` precision. The cast's result type does not select
the argument's arithmetic width:

```onda
sample:
  contextual: f32 = (16777216.0 + 1.0) - 16777216.0  # 0.0: f32 arithmetic
  converted = f32((16777216.0 + 1.0) - 16777216.0)   # 1.0: f64 arithmetic, then cast
```

Cast targets are `f32`, `f64`, `i32`, and `i64`. They accept numeric or boolean
arguments:

| Explicit conversion | Behavior |
| --- | --- |
| `i32` to `i64` | Sign extension. |
| `i64` to `i32` | Keep the low 32 bits, interpreted as signed. |
| Integer to float, or `f64` to `f32` | Normal floating-point rounding. |
| `f32` to `f64` | Preserve the existing `f32` value exactly. |
| Float to integer | Truncate toward zero, saturate at the destination bounds; NaN becomes zero. |
| `bool` to numeric | `false` becomes zero and `true` becomes one. |

To obtain a boolean from a numeric value, use `value != 0`. Zero, including
negative zero, produces `false`; every other value, including NaN, produces
`true`. `bool(expr)` is not a cast form.

### Numeric builtin types

Numeric builtin constants remain contextual: a fresh `value = PI` binding
defaults to `f32`, while
`value: f64 = PI` uses `f64`. With an `f32` frequency, `freq * TWO_PI / SR`
uses `f32` runtime arithmetic; the constants adapt before those operations.

Numeric builtin types follow these rules after contextual argument adaptation:

| Builtin | Argument and result types |
| --- | --- |
| `abs`, `fabs` | Numeric argument; preserve its selected type. |
| `min`, `max`, `clamp(value, lower, upper)` | Numeric arguments; use their common promoted type. |
| `pow` | Numeric arguments and a floating result. With floating operands, use their common promoted type. Two concrete integer operands produce `f32`; a pure contextual call can select a floating destination's width. |
| `sin`, `cos`, `tan`, `tanh`, `atan`, `atan2`, `exp`, `log`, `sqrt`, `floor`, `ceil`, `round`, `trunc`, `fma` | Floating arguments; use `f64` if any adapted argument is `f64`, otherwise `f32`. |

For example, `sin(1)` is rejected: use `sin(1.0)` or `sin(f32(1))`.
In `fma(value_f32, 1, 0)`, the integer literals adapt to the concrete floating
peer, so the call uses `f32`. No numeric builtin accepts `bool`.

## Quick reference

### Declaration and statement index

| Form | Purpose |
| --- | --- |
| `ins`, `inputs` | [Host input ports](#inputs). |
| `params`, `kins` | [Host-visible parameters](#parameters). |
| `outs`, `outputs` | [Audio-rate output ports](#outputs). |
| `kouts` | [Block-rate control output ports](#outputs). |
| `buffers` | [External host-bound buffers](#external-buffers). |
| `events`, `event` | [Command handlers for programs and processors](#events-and-delegates). |
| `delegates`, `delegate` | [Typed occurrences reported to an owner or host](#delegates-and-when). |
| `when` | [Static synchronous delegate subscription](#delegates-and-when). |
| `print(...)` | [Runtime diagnostic output](#printing). |
| `init` | [Setup and persistent state](#init). |
| `block` | [Per-block code](#block). |
| `sample` | [Per-sample code](#sample). |
| `pin` | [State retained during selective reinitialization](#reinitialization-and-pinned-state). |
| `if`, `elif`, `else` | [Conditional branches](#control-flow). |
| `for`, `loop`, `while` | [Repetition](#control-flow). |
| `break`, `continue` | [Loop control](#control-flow). |
| `return` | [Function results and early exits](#returns). |
| `graph` | [Declarative signal routing](#graphs). |
| `task`, `tasks` | [Statically allocated cooperative preparation work](#tasks). |
| `const` | [Compile-time values](#constants). |
| `config const` | [Host-selected compile inputs](#configuration-constants). |
| `const def` | [Compile-time functions](#compile-time-functions). |
| `def` | [Runtime helper functions](#functions). |
| `struct` | [Nominal data types](#structs). |
| `enum` | [Nominal scalar choices with dense `i32` ordinals](#enums). |
| `proc`, `processor` | [Reusable DSP processors](#processors). |
| `proc Main` | [Host entry processor](#entry-processor-main). |
| `namespace` | [Qualified declaration groups and integer templates](#namespaces). |
| `use`, `pub use` | [Unqualified lookup imports and re-exports](#use-declarations). |
| `import`, `include` | [Source loading](#modules-and-namespaces). |
| `await`, `yield`, task `reset()` | [Task scheduling](#await-and-scheduling). |
| `assert(expr)` | [Compile-time namespace checks](#integer-namespace-parameters). |

`print`, `await`, `yield`, and task `reset()` are executable statements rather than
top-level declarations.

### Reserved names

- Control-flow keywords are reserved: `if`, `elif`, `else`, `for`, `in`, `while`, `loop`, `break`, `continue`, `return`, and `assert`.
- `in` separates the loop variable from its range in `for i in A..B`; use names such as `input` for ports and variables.
- `import`, `include`, `use`, `as`, `pub`, `private`, `pin`, `enum`, and `config` are reserved for their declaration and modifier syntax.
- `delegate`, `delegates`, `when`, and `print` are reserved for messages and diagnostic output.
- `true` and `false` are reserved boolean literals.
- Identifiers beginning with `__onda_` are reserved for compiler-generated symbols.

Task syntax also reserves `task`, `tasks`, `await`, and `yield`.
`full` is reserved as a processor parameter name for the builtin init event.

Callable names declared by a program or processor cannot be reused as value names
in its executable scopes. This includes functions, events, tasks, delegates, and
top-level struct and processor constructors. The restriction applies to parameters,
`when` bindings, assignments, destructuring, and loop variables. Receiver-qualified
methods and callables from another source file may still be shadowed by local values.

### Aggregate limits

- Aggregate nesting is limited to 256 levels so malformed or impractically deep
  definitions are rejected without exhausting compiler resources.
- Resolved aggregate layouts may contain at most 65,536 expanded shape nodes across a program.
  Fixed array lengths remain tensor dimensions and do not increase this count.
- Aggregate width and lowered function parameter count have no separate fixed limit. Arrays of
  processors retain their dimensions at every nesting level; increasing the number of instances
  does not multiply the receiver's fields or function parameters. A lowered function may contain
  at most 32,768 locals, including compiler-generated aggregate components.

### Unchecked indexed access

`read_unsafe(values, index)` / `values.read_unsafe(index)` and
`write_unsafe(values, index, value)` / `values.write_unsafe(index, value)` provide unchecked indexed
access across the language; they are not limited to external buffers. Supported primitive storage
includes fixed arrays, slices, named input/parameter/output arrays, the uniform dynamic views `ins`,
`params`/`kins`, `outs`, and `kouts`, selected buffers, and fixed buffer collections. Ordinary
direction rules still apply: inputs and parameters are read-only, while output views are write-only.
Buffer forms accept their normal selector, channel, and sample coordinates.

Unchecked coordinates perform no clamp or runtime bounds check. The caller must prove every supplied
index, collection selector, channel, and sample coordinate is valid. Violating that contract is memory-unsafe.
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
introducing an alias, passing a reference argument, or supplying an aggregate replacement; it
cannot be used as a scalar value.
For arrays and slices of data structs, `write_unsafe(cells, index, replacement)` replaces the
selected element using the ordinary exact-type and snapshot rules for aggregate assignment.
Processor elements remain resources and cannot be replaced. The same valid-index
requirement applies to aggregate selectors; invalid access has no defined fallback.

#### Buffer coordinates

For declared mono `src` and stereo `bus` buffers:

```onda
sample:
  x = src.read_unsafe(0)
  y = read_unsafe(bus, 1, 0)
  src.write_unsafe(0, x)
  write_unsafe(bus, 1, 0, y)
```

These coordinates are valid even for a neutral unbound buffer: every buffer
has at least one sample, and `bus` has exactly two channels. With dynamic
coordinates, the program must establish the same guarantees itself.

Free-call and receiver syntax are equivalent. A fixed buffer collection can either be accessed as
one unchecked operation (`bank.read_unsafe(slot, sample)`) or selected compositionally
(`bank[slot].read_unsafe(sample)`). In the latter form, `bank[slot]` retains ordinary clamped/proven
selection and only the sample (and channel, for a multichannel buffer) is unchecked. A buffer
reference alias made from a collection element supports the same receiver methods.
