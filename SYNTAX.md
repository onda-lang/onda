# Omni Syntax (Current)

This document describes the syntax currently implemented in `omni-llvm`.

## 1 Program structure

Omni supports both brace style and indentation style.

```omni
outs { out1 }
sample { out1 = 0.0 }
```

```omni
outs:
  out1
sample:
  out1 = 0.0
```

Statements can be separated by newline or `;`.
Line comments use `#`.

Top-level blocks:
- `ins`
- `outs`
- `params`
- `events`
- `buffers`
- `init`
- `block`
- `sample`
- `def`
- `struct`
- `proc` / `processor`
- `namespace`

## 2 Types

Primitive types:
- `f32`
- `f64`
- `i32`
- `i64`
- `bool`

Array type syntax:
- `T[N]` (for example `f32[2]`, `i32[SR * 2]`)

Buffer declaration types:
- `buffer[T]` (mono)
- `buffer[T[2]]` (static channel count)
- `buffer[T[]]` (dynamic channel count)

## 3 Ports, params, buffers

Basic declarations:

```omni
ins:
  in1
  side: f64

outs:
  out1
  out2

params:
  gain = 1.0
  mode: i32 = 0

buffers:
  ext: buffer[f32]
  bus: buffer[f32[2]]
```

Count shorthand:

```omni
ins 2
outs 2
params 3
buffers 2
```

Count prefix with explicit declarations (`ins`/`outs`/`params`):

```omni
params 2:
  freq = 500 {8000}
  mix = 0.5 {0.0, 1.0}
```

Section default type shorthand:

```omni
ins[f64] 2
outs[f64]:
  out1
  meter: f32
params[i32]:
  mode
buffers[f32]:
  line
```

Array-typed ports/params are supported:

```omni
ins:
  in_st: f32[2]
outs:
  out_st: f32[2]
params:
  gains: f32[2] = [1.0, 1.0]
```

Rules:
- Explicit entry type overrides section default type.
- For `ins`/`outs`/`params`, count prefix must match explicit declaration count.
- Ranges are supported on scalar `ins` and scalar `params` only:
  - `name = default {min, max}`
  - `name = default {max}` (max-only)
- Ranges on arrays are rejected.
- If `inN`/`outN` are used without declaration, they are implicitly created as `f32`.

`init` also supports section default scalar type shorthand:

```omni
init[f64]:
  phase = 0.0
  last = 0.0
```

Rules:
- `init[T]` / `init[f64]` applies to untyped scalar declarations in `init`.
- Explicit per-symbol declaration types still win (`x: i32 = ...`).
- Non-scalar section defaults (for example `init[f32[4]]`) are invalid.

## 4 Variables, assignment, expressions

First assignment infers type by default:

```omni
sample:
  x = 0
  y = 0.0
```

Explicit declaration pins type:

```omni
sample:
  x: i64 = 0
```

Operators:
- Arithmetic: `+`, `-`, `*`, `/`, `%`
- Comparisons and logical operators are supported in conditions/expressions.

Constants:
- `PI` / `pi`
- `TWO_PI` / `TWOPI` / `two_pi` / `twopi`
- `SAMPLE_RATE` / `SAMPLERATE` / `SR` / `sample_rate` / `samplerate`
- `BLOCK_SIZE` / `BLOCKSIZE` / `BS` / `block_size` / `blocksize`
- Default constant types:
  - `PI`/`TWO_PI`: `f64`
  - `SAMPLE_RATE`: `f32`
  - `BLOCK_SIZE`: `i32`

## 5 Control flow

Supported:
- `if (...) { ... } else { ... }`
- `if (...) { ... } elif (...) { ... } else { ... }`
- `for i in A..B { ... }` (exclusive end)
- `for i in A..=B { ... }` (inclusive end)
- `for i @ STEP in A..B { ... }` (`@ STEP` optional; default step is `1`)
- Descending loops use a negative step (for example `for i @ -1 in 10..0`)
- `@ 0` is invalid
- `loop N { ... }` (sugar)
- `while (...) { ... }`
- `break`
- `continue`
- `return`

## 5.1 Sample oversampling (`sample N`)

`sample` blocks support an optional oversampling factor:

```omni
sample:
  out1 = in1

sample 4:
  out1 = tanh(in1 * 8.0)
```

The same syntax is supported inside processors:

```omni
proc Drive:
  ins: in1
  outs: out1
  sample 8:
    out1 = tanh(in1 * 12.0)
```

Rules:
- Allowed factors: `1`, `2`, `4`, `8`, `16`, `32`, `64`
- `sample:` is equivalent to `sample 1:`
- Factor must be an integer literal
- Invalid factors and non-literal factors are semantic errors

Runtime behavior:
- input reads are interpolated across oversample substeps
- params are held within the base sample
- outputs are filtered/decimated back to base rate by compiler-managed conversion

## 6 Functions (`def`)

Supported:
- Positional and named arguments
- Default arguments
- Early return
- Method-style sugar for functions: `value.fn(a, b)` is rewritten as `fn(value, a, b)` when a matching function `fn` is in scope.

```omni
def wrap_phase(p, upper = TWO_PI):
  if (p > upper):
    return p - upper
  return p
```

`def` generics are intentionally unsupported.

## 7 Structs

Struct fields and methods are supported.
Methods must have `self` as the first argument.

```omni
struct Voice:
  phase: f32
  sig: f32

  def tick(self, hz):
    self.phase = self.phase + hz * TWO_PI / SR
    self.sig = sin(self.phase)
```

## 8 Processors (`proc`)

Processor blocks:
- `init` (optional)
- `sample` (required)
- `events` (optional)
- optional `block` wrapper with pre/sample/post sections

```omni
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

Construction/calls:
- Construct in `init`: `p = Gain(g = 0.5)`
- Call in `sample`: `out1 = p(0.25)` (single-out scalar sugar)
- Direct endpoint call read: `out1 = p(0.25).out1` (or endpoint name)
- Endpoint read: `p.<endpointName>`
- Ordinal endpoint alias: `p.outN` (1-based)
- Statement call form is supported

Single-out procs also support endpoint access forms (`p.out1` and named endpoint aliases).

Processor constructors use named arguments for params/buffers.

## 8.1 Events (`events`)

Events are host-triggered handlers that run immediately on the audio thread when invoked.

Top-level example:

```omni
outs { out1 }
events {
  note_on(note: i32, vel: i32) {
    amp = f32(vel) / 127.0
  }
}
init { amp = 0.0 }
sample { out1 = amp }
```

Proc-level example with explicit forwarding:

```omni
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(v: f32) {
      amp = v
    }
  }
  sample { out1 = amp }
}

events {
  note_on(v: f32) {
    voice.note_on(v)
  }
}
```

Event parameter types:
- Primitive scalars (`f32`, `f64`, `i32`, `i64`, `bool`)
- Fixed-size primitive arrays (`T[N]`)
- Untyped scalar event params default to `f32` (for example `note_on(note)` -> `note: f32`)

Rules:
- Top-level events are host-entry handlers; proc events are reached through explicit calls (for example `voice.note_on(...)`).
- Event handlers may write only to state rooted in `init` declarations.
- Event handlers cannot write `outN`.
- Event parameters are immutable.
- Array event parameters are read-only references in handlers.
- Proc event names must not collide with callable endpoint names in the same proc.
- Unknown host event indices are ignored.
- Invalid payload size for a known event is a runtime error.

## 9 Generics

Supported for `struct` and `proc` with primitive specialization:

```omni
struct Pair[T]:
  a: T
  b: T

proc OnePole[T]:
  ins[T] 1
  outs[T] 1
  sample:
    out1 = in1
```

Type arguments can be explicit (`Name[f64](...)`) or inferred in many constructor cases.

## 10 Arrays

Fixed-size arrays are supported for state/local storage, including typed forms and capacity expressions.
Array indexing and assignment are supported in `init`/`sample`/`def` where valid.

## 11 Imports and namespaces

Imports:
- `import module/path`
- Built-in std modules include:
  - `std/math`
  - `std/osc`
  - `std/filter`
  - `std/env`
  - `std/delay`
  - `std/data`

Include:
- `include "path.omni"`

Namespaces:

```omni
namespace my::dsp:
  def sat(x):
    return clamp(x, -1.0, 1.0)
```

Templated namespaces with compile-time int params are supported:

```omni
namespace Data[S = SR, C = 1]:
  struct Data[T]:
    storage: T[S * C]
```

Use sites support inline instantiation and aliases:

```omni
namespace D = Data[SR, 1]

init:
  a = Data[SR, 1]::Data[f64]()
  b = D::Data[f64]()
```

Rules:
- Namespace template params require defaults.
- Namespace template args support positional and named forms.
- Args are normalized as `i32(...)` at compile time.
- Alias declarations are declaration sugar and can appear at top-level or inside namespaces.

## 12 Example-driven starting points

Useful examples in `examples/`:
- Basic oscillator: `sine.omni`, `std_sine.omni`
- Block/sample structure: `block_counter.omni`, `saw_blep.omni`
- Struct + methods: `cross_fm.omni`
- Processor usage and output forms: `proc_gain.omni`, `proc_split.omni`, `proc_array_stereo_sine.omni`, `reverb.omni`
- Array-heavy DSP: `karplus_strong_data.omni`, `multitap_feedback_struct_data.omni`
- Stdlib and generics: `stdlib_f32.omni`, `stdlib_f64.omni`
