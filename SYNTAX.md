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
- Bitwise integer ops: `~`, `&`, `|`, `^`, `<<`, `>>`
- Comparisons: `==`, `!=`, `<`, `<=`, `>`, `>=`
- Logical ops: `!`, `&&`, `||`

Bitwise rules:
- `~`, `&`, `|`, `^`, `<<`, and `>>` are integer-only.
- Valid operand/result types are `i32` and `i64`.
- Mixed `i32`/`i64` operands widen to `i64`.
- `>>` is an arithmetic right shift.

Precedence (low to high):
- `||`
- `&&`
- `|`
- `^`
- `&`
- comparisons
- `<<`, `>>`
- `+`, `-`
- `*`, `/`, `%`

Constants:
- `PI` / `pi`
- `TWO_PI` / `TWOPI` / `two_pi` / `twopi`
- `SAMPLE_RATE` / `SAMPLERATE` / `SR` / `sample_rate` / `samplerate`
- `BLOCK_SIZE` / `BLOCKSIZE` / `BS` / `block_size` / `blocksize`
- Default constant types:
  - `PI`/`TWO_PI`: `f64`
  - `SAMPLE_RATE`: `f32`
  - `BLOCK_SIZE`: `i32`

Compile-time assertions:

```omni
namespace Config:
  assert(BLOCK_SIZE > 0)
```

Rules:
- `assert(expr)` is supported inside namespaces only.
- `expr` must be a compile-time constant expression.
- `expr` must evaluate to `bool`.
- If the condition is `false`, compilation fails.

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
- Top-level overloads by arity and/or parameter type
- Method-style sugar for functions: `value.fn(a, b)` is rewritten as `fn(value, a, b)` when a matching function `fn` is in scope.
- Def scope is lexical-local: top-level runtime symbols (`ins`/`outs`/`params`/`buffers`/`init` state) are not directly visible inside a `def`.

```omni
def wrap_phase(p, upper = TWO_PI):
  if (p > upper):
    return p - upper
  return p
```

Overload examples:

```omni
def mix(x):
  return x

def mix(x, y):
  return x + y
```

```omni
def sat(x: f32):
  return x

def sat(x: f64):
  return f32(x)
```

### Parameter types

In addition to primitive types and struct types, `def` parameters support:

- **Typed array**: `arr: f32[]`, `arr: i64[]` — accepts an array of the given element type with any length. No monomorphization needed.
- **Untyped array**: `arr: []` — accepts an array of any element type. Monomorphized at each call site based on the concrete element type.
- **Typed buffer**: `buf: buffer[f32]`, `buf: buffer[f64[2]]` — accepts a buffer matching the given type/channels.
- **Bare buffer**: `buf: buffer` — accepts any buffer. Monomorphized at each call site based on the concrete buffer type.
- **Generic struct/proc**: `v: Voice` where `Voice[T]` is a generic struct or proc — monomorphized at each call site based on the concrete specialization passed.

```omni
# typed array param
def sum(arr: f32[]):
  total = 0.0
  for i in 0..arr.len():
    total = total + arr[i]
  return total

# untyped array param (monomorphized per call site)
def first(arr: []):
  return arr[0]

# bare buffer param (monomorphized per call site)
def read_first(buf: buffer):
  return buf[0]

# generic struct param (monomorphized per call site)
struct Box[T]:
  val: T = 0.0

def unbox(b: Box):
  return b.val
```

Method-style sugar works with generic params: `voice.process()` desugars to `process(voice)` and monomorphizes correctly.

### Resolution rules

- Exact typed match is preferred.
- If no exact typed match exists, implicit widening candidates may be considered.
- Untyped parameters are lower priority than typed parameters.
- Default arguments participate in overload matching.
- If multiple candidates are equally valid, the call is a semantic error (ambiguous overload).
- Return type is not part of overload selection.
- Overloading currently applies to top-level `def` only.
- Struct methods still cannot be overloaded; duplicate method names in the same struct are rejected.
- For overloads involving generic params: explicit type > generic/duck-typed > untyped.

Explicit `def` type parameters (`def fn[T]`) are intentionally unsupported; polymorphism is through typed/untyped parameters and call-site monomorphization.

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

Typed struct declarations in `init` can use constructor form or declaration-only form:

```omni
import std/data

init:
  # explicit constructor
  a: std::data::Data[f32] = std::data::Data()
  # declaration-only: auto-initializes with default ctor state
  b: std::data::Data[f32]
  # namespace-instantiated owner type works too
  c: std::data[SR, 1]::Data
```

Rules:
- Typed struct declarations are `init`-only.
- Declaration-only form (`x: Type`) desugars to default constructor initialization.
- Generic typed declarations require explicit type args (for example `x: Box[f32]`; `x: Box` is rejected for generic `Box[T]`).
- For untyped constructor assignments (`x = Box()` / `p = Proc()`), unresolved generic constructor type parameters default to `f32`.

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

Processor instance arrays are supported in `init` (top-level and proc-level), for example:

```omni
init:
  voices: Voice[2] = [Voice(), Voice()]
```

Indexed proc-array dispatch supports literal and runtime indices:
- Call/read: `voices[idx](...)`
- Endpoint read from call: `voices[idx](...).outN` (or named endpoint)
- Statement call: `voices[idx](...)`
- Proc-event forwarding: `voices[idx].note_on(...)`
- Proc aliasing: `a = voices[idx]`, then `a(...)` / `a.outN` (or named endpoint)

Runtime indices are clamped to the valid slot range during lowered dispatch.
Ctor buffer bindings are established in `init`; dynamic indexed calls use per-slot buffer refs in runtime state (refreshed on `process_bound`).

For procs that define a `block` section, proc-array `()` calls use active-slot block hook semantics:
- Hook trigger is the proc `()` call itself (expression or statement form), not plain slot retrieval.
- `block pre` runs lazily on the first `()` call to a given slot within the current block.
- `block post` runs once at block end for each slot that was called in that block.
- Dynamic indexed calls do not conservatively trigger hooks for all slots.

For procs without a `block` section, no active-slot hook tracking is emitted (fast path).

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
- Top-level event handlers may write only to top-level state rooted in `init` declarations.
- Proc event handlers may write proc state rooted in `init` declarations and proc params.
- Event handlers cannot write input symbols.
- Event handlers cannot write output symbols (including `outN` aliases).
- Top-level params are immutable in top-level event handlers.
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

Generic type parameters are restricted to numeric primitives: `f32`, `f64`, `i32`, `i64`. Using `bool` as a generic type argument is a semantic error.

Generic typed local declarations (`x: T = expr`) are supported in all executable scopes of a generic owner:
- `init` (top-level and proc)
- `sample` / `block`
- `def` bodies (struct methods)
- `events`

```omni
proc Filter[T]:
  ins[T] 1
  outs[T] 1
  init:
    state: T = 0.0
  sample:
    tmp: T = in1 * 0.5
    state = state + tmp
    out1 = state
```

Unresolved generic type parameters in declaration/type positions produce an error (no implicit defaulting there). For untyped constructor assignments only, unresolved constructor type parameters default to `f32`.

Generic struct and proc types can be used as `def` parameter types for call-site monomorphization (see section 6).

## 10 Arrays

Fixed-size arrays are supported for state/local storage, including typed forms and capacity expressions.
Array indexing and assignment are supported in `init`/`sample`/`def` where valid.

## 11 Imports and namespaces

Imports:
- `import module/path`
- Built-in std modules include:
  - `std/prelude`
  - `std/math`
  - `std/osc`
  - `std/filter`
  - `std/env`
  - `std/delay`
  - `std/data`
  - `std/lookup`
- `std/prelude` is auto-imported (explicit import is optional), and it currently re-exports `std/math` + `std/lookup`.

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

Namespace-local compile-time assertions are supported:

```omni
namespace FFT[N = 256]:
  assert((N & (N - 1)) == 0)
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
