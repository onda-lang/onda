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
- `const`
- `events`
- `buffers`
- `init`
- `block`
- `sample`
- `graph`
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
ins<f64> 2
outs<f64>:
  out1
  meter: f32
params<i32>:
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
init<f64>:
  phase = 0.0
  last = 0.0
```

Rules:
- `init<T>` / `init<f64>` applies to untyped scalar declarations in `init`.
- Explicit per-symbol declaration types still win (`x: i32 = ...`).
- Non-scalar section defaults (for example `init<f32[4]>`) are invalid.

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

User-defined compile-time constants:

```omni
const MAX_IR = 100000
const HOP: i32 = BLOCK_SIZE / 2
```

Rules:
- `const NAME = expr` and `const NAME: T = expr` are supported.
- `T` is primitive scalar only in the current implementation (`f32`, `f64`, `i32`, `i64`, `bool`).
- `expr` must be compile-time evaluable.
- `const` is supported at top-level, inside namespaces, and inside executable scopes (`init`, `block`, `sample`, `events`, `def`).
- Namespace consts are accessible from outside via qualified paths such as `NS::VALUE` and instantiated namespace forms like `std::convolution<8, 8>::HopSize`.
- Visibility is lexical and forward references are not supported in the current implementation.
- Reassignment is rejected.
- Builtin compile-time constant names such as `SR` / `SAMPLE_RATE` / `BLOCK_SIZE` remain reserved.

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

## 5.2 Graph routing (`graph`)

`graph` is supported at top-level and inside processors.

```omni
proc Main:
  ins:
    in_st: f32[2]
  outs:
    out_st: f32[2]
  params:
    mix = 0.25

  init:
    rev = Reverb()

  graph:
    in_st[0] >> rev.inL
    in_st[1] >> rev.inR
    mix >> rev.mix
    rev.outL >> out_st[0]
    rev.outR >> out_st[1]
```

Edge forms:

```omni
src >> dst
dst << src
@block src >> dst
@block dst << src
@sample src >> dst
@sample dst << src
src >>[N] dst
dst <<[N] src
```

Rules:
- `graph` is mutually exclusive with `sample` and `block` in the same owner.
- `init` may still be used with `graph`.
- Proc instances used as graph nodes are created in `init`.
- Unannotated edges targeting proc `params` are inferred as `@block`.
- Unannotated edges targeting other destinations are inferred as `@sample`.
- `@sample` may override the default `@block` behavior for proc param destinations.
- `>>[N]` uses a compile-time integer `N >= 0`.
- Delayed edges are sample-rate only.
- `>>[0]` does not break cycles.

Legal destinations:
- top-level outputs
- proc inputs and params
- proc-array slot inputs and params such as `voices[0].gain`

Examples:

```omni
graph:
  in1 >> g.in1
  env.out1 >> g.gain
  @sample lfo.out1 >> lp.cutoff
  src.pair >> out_st
  voices[0].pair[1] >> out1
```

Current source support includes:
- top-level inputs and params
- proc outputs
- proc-array slot outputs
- array literals such as `[in1, in2]`
- indexed reads such as `gains[1]` and `src.pair[1]`
- sliced reads such as `in_bus[1:3]` and `src.pair[:-1]`
- whole-array reads such as `src.pair` and `voices[0].pair`
- pure arithmetic/logical expressions built from supported graph sources
- element-wise array expressions such as `in_a + in_b`, `in_a * 0.5 + in_b * 0.5`, and array-plus-scalar forms where the final edge shape still matches the destination

Receiver syntax:
- `dst << src` is exact graph sugar for `src >> dst`
- all implemented rate/delay forms apply equally to receiver syntax

Current MVP limits:
- user-defined calls and proc calls are not supported inside graph source expressions
- array-constructor expressions such as `f32[2](...)` are not supported as graph sources
- graph event propagation syntax does not exist; use ordinary `events` blocks for control/event routing

Type and scheduling rules:
- graph edges use strict shape matching
- `f32[2] >> f32[2]` is allowed
- `f32 >> f32[2]` is rejected
- `f32[2] >> f32[3]` is rejected
- delayed edges use the same strict shape rules, so `f32[2] >>[1] f32[2]` is allowed and `f32[2] >>[1] f32[3]` is rejected
- each destination has a single writer
- fan-out is allowed
- cycles are rejected unless at least one cycle edge has positive sample delay
- proc nodes are stepped implicitly according to graph reachability and topological order
- delayed edge state persists across blocks/process calls
- `graph` lowering can be inspected with `omni compile <file> --dump-graph`
- graph slice bounds must be compile-time integer expressions

For/loop bounds:
- `A`, `B`, and `N` may be general expressions, including parenthesized forms like `0..(n - 1)`.

## 6 Functions (`def`)

Supported:
- Positional and named arguments
- Default arguments
- Early return
- Top-level `def` and struct-method overloads only, by arity and/or parameter type
- Method-style sugar for functions: `value.fn(a, b)` is rewritten as `fn(value, a, b)` when a matching function `fn` is in scope.
- Def scope is lexical-local: top-level runtime symbols (`ins`/`outs`/`params`/`buffers`/`init` state) are not directly visible inside a `def`.
- Call argument lists may span multiple lines, and a trailing comma is allowed in both function calls and method calls.

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
- **Generic struct/proc**: `v: Voice` where `Voice<T>` is a generic struct or proc — monomorphized at each call site based on the concrete specialization passed.

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
struct Box<T>:
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
- Overloading currently applies to top-level `def` and struct methods.
- Proc-local defs still cannot be overloaded; duplicate names in the same processor are rejected.
- For overloads involving generic params: explicit type > generic/duck-typed > untyped.

Explicit `def` type parameters (`def fn<T>`) are intentionally unsupported; polymorphism is through typed/untyped parameters and call-site monomorphization.

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
  a: std::data::Data<f32> = std::data::Data()
  # declaration-only: auto-initializes with default ctor state
  b: std::data::Data<f32>
  # namespace-instantiated owner type works too
  c: std::data<SR, 1>::Data
```

Rules:
- Typed struct declarations are `init`-only.
- Declaration-only form (`x: Type`) desugars to default constructor initialization.
- Generic typed declarations require explicit type args (for example `x: Box<f32>`; `x: Box` is rejected for generic `Box<T>`).
- For untyped constructor assignments (`x = Box()` / `p = Proc()`), unresolved generic constructor type parameters default to `f32`.

## 8 Processors (`proc`)

Processor blocks:
- `init` (optional)
- `sample` (required)
- `events` (optional)
- `def` (optional, proc-local helper functions — see section 8.2)
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
- Read-only primitive slices (`T[]`)
- Proc-event-only generic primitive slices (`U[]` where `U` is a proc generic type parameter specialized to a primitive)
- Untyped scalar event params default to `f32` (for example `note_on(note)` -> `note: f32`)

Rules:
- Top-level events are host-entry handlers.
- Proc events are receiver-only proc commands reached through explicit proc-instance calls (for example `voice.note_on(...)`).
- Slice event params such as `f32[]` are allowed on both top-level host events and proc events.
- Generic slice event params such as `U[]` are allowed on proc events only, and `U` must specialize to a primitive type before lowering.
- Proc-event calls are statements, not expressions.
- Unqualified calls never resolve to proc events.
- Top-level event handlers may write only to top-level state rooted in `init` declarations.
- Proc event handlers may write proc state rooted in `init` declarations and proc params.
- Event handlers cannot write input symbols.
- Event handlers cannot write output symbols (including `outN` aliases).
- Top-level params are immutable in top-level event handlers.
- Event parameters are immutable.
- Array and slice event parameters are read-only references in handlers.
- Event payload reads from fixed arrays and slices clamp the same way as other primitive array reads.
- Fixed-array event params are lowered internally as array-typed params, not one scalar argument per element.
- For event payload passing, prefer slices (`T[]`) over large fixed arrays (`T[N]`).
- Keep fixed arrays for true fixed-size storage and fixed-shape interfaces where the compile-time size is part of the contract.
- Proc event names must not collide with callable endpoint names in the same proc.
- A proc cannot instantiate its own type directly in its state/`init` (for example `other = Voice()` inside `proc Voice` is invalid).
- Unknown host event indices are ignored.
- Invalid payload size for a known event is a runtime error.
- For top-level host events with slice params, payload bytes are encoded as `i32 len` followed by contiguous element bytes.

## 8.2 Proc-local defs

Processors can contain private `def` blocks that act as helper subroutines with implicit access to proc state. Unlike top-level `def` blocks, proc-local defs can read and write `init`-declared state, params, and other proc-scoped symbols directly — no `self` parameter is needed.

```omni
proc Filter<T>:
  ins<T> 1
  outs<T> 1

  init:
    state: T = 0.0
    coeff: T = 0.5

  def do_reset():
    state = T(0.0)

  def apply(x: T):
    state = state + (x - state) * coeff
    return state

  events:
    reset():
      do_reset()

  sample:
    out1 = apply(in1)
```

Proc-local defs support:
- Parameters (positional, named, defaults) — same as top-level `def`.
- Return values via `return`.
- Calling other proc-local defs.
- Calling namespace-level `def` functions.
- Access to proc generic type parameters (e.g. `T`).

Rules:
- Proc-local defs are always private to the enclosing processor.
- They are callable from `init`, `sample`, `block`, `events`, and other proc-local defs.
- State variables are accessed directly by name (no `self`).
- Parameters and for-loop variables are local to the def; state variables pass through unchanged.
- Recursive and mutually recursive calls are detected and rejected.
- Internally, proc-local defs lower to hidden ordinary defs with an implicit proc receiver. Calls inside the proc are rewritten to those hidden defs, so proc-local defs follow the same normal argument binding/default/return semantics as regular `def` blocks.
- Overloading of proc-local defs is not currently supported.

## 9 Generics

Supported for `struct` and `proc` with primitive specialization:

```omni
struct Pair<T>:
  a: T
  b: T

proc OnePole<T>:
  ins<T> 1
  outs<T> 1
  sample:
    out1 = in1
```

Type arguments can be explicit (`Name<f64>(...)`) or inferred in many constructor cases.

Generic type parameters are restricted to numeric primitives: `f32`, `f64`, `i32`, `i64`. Using `bool` as a generic type argument is a semantic error.

Generic typed local declarations (`x: T = expr`) are supported in all executable scopes of a generic owner:
- `init` (top-level and proc)
- `sample` / `block`
- `def` bodies (struct methods)
- `events`

Generic casts and generic array function params are also supported where the corresponding primitive forms are valid:
- `T(expr)` rewrites to the bound primitive cast inside a specialized generic owner
- `T[]` is valid for `def`/method array parameters

```omni
proc Filter<T>:
  ins<T> 1
  outs<T> 1
  init:
    state: T = 0.0
  sample:
    tmp: T = in1 * 0.5
    state = state + tmp
    out1 = state
```

Unresolved generic type parameters in declaration/type positions produce an error (no implicit defaulting there). This includes generic array function params such as `T[]`. For untyped constructor assignments only, unresolved constructor type parameters default to `f32`.

Generic struct and proc types can be used as `def` parameter types for call-site monomorphization (see section 6).

## 10 Arrays

Fixed-size arrays are supported for state/local storage, including typed forms and capacity expressions.
Array indexing and assignment are supported in `init`/`sample`/`def` where valid.

Slice expressions are also supported on primitive arrays, slices, and primitive buffers/channels:

```omni
sample:
  a = buf[:]
  b = buf[2:]
  c = buf[:-1]
  d = buf[1:-2]
  last = buf[-1]
```

Rules:
- Slice forms are `a[:]`, `a[start:]`, `a[:end]`, and `a[start:end]`.
- Negative slice bounds are supported and are interpreted relative to the logical length.
- Slice expressions lower to normal primitive slice views of type `T[]`.
- Buffer slicing also yields `T[]`, not a new buffer type.

Writable slice assignment is supported for mutable primitive array/buffer targets:

```omni
sample:
  values[1:-1] = 0.5
  dst[:] = src[:]
  values[1:] = values[:-1]
```

Rules:
- Slice assignment is statement-only.
- Scalar fill writes the full target slice.
- Slice copy writes `min(dst_len, src_len)` elements.
- Overlapping slice copies are stable and behave as if the source region is copied through a temporary buffer first.
- Event payload arrays/slices are read-only and cannot be used as writable slice targets.
- Struct-element arrays are not sliceable in the current implementation.

## 11 Imports and namespaces

Imports:
- `import module/path`
- Built-in std modules include:
  - `std/prelude`
  - `std/math`
  - `std/complex`
  - `std/osc`
  - `std/filter`
  - `std/env`
  - `std/delay`
  - `std/data`
  - `std/lookup`
  - `std/fft`
  - `std/convolution`
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
namespace Data<S = SR, C = 1>:
  struct Data<T>:
    storage: T[S * C]
```

Syntax split:
- `<>` is used for namespace instantiation, generic type specialization, and section default type modifiers on `ins` / `outs` / `params` / `init`.
- `[]` is used for arrays, indexing, slices, and buffer/channel forms.

Namespace-local compile-time assertions are supported:

```omni
namespace FFT<N = 256>:
  assert(N > 0)
  assert((N & (N - 1)) == 0)
```

Use sites support inline instantiation and aliases:

```omni
namespace D = Data<SR, 1>

init:
  a = Data<SR, 1>::Data<f64>()
  b = D::Data<f64>()
```

Rules:
- Namespace template params require defaults.
- Namespace template args support positional and named forms.
- Args are normalized as `i32(...)` at compile time.
- Alias declarations are declaration sugar and can appear at top-level or inside namespaces.

`std/fft` currently provides a namespace-parameterized in-place complex FFT:

```omni
import std/fft

init:
  fft: std::fft<256>::FFT<f32>
```

Current API:
- `std::fft<N>::FFT<T>`
- `std::fft<N>::RealFFT<T>`
- `std::fft<N>::RealIFFT<T>`
- namespace contract: `N > 0` and `N` must be a power of two
- `T` is intended for floating-point use (`f32` or `f64`)
- internal storage: `re: T[N]`, `im: T[N]`
- introspection helpers:
  - `size() -> i32`
  - `real_bin_count() -> i32` (`N / 2 + 1` unique bins for real-input spectra)
- packed real-spectrum layout uses `N` scalars:
  - `packed[0..N/2]` = real bins `0..N/2`
  - `packed[N/2 + 1..N - 1]` = imaginary bins `1..N/2 - 1`
- methods:
  - `clear()`
  - `load_real(input: T[])`
  - `load_complex(real: T[], imag: T[])`
  - `load_real_packed(input: T[])`
  - `store_real(output: T[])`
  - `store_imag(output: T[])`
  - `store_magnitude(output: T[])`
  - `store_power(output: T[])`
  - `store_phase(output: T[])`
  - `store_real_packed(output: T[])`
  - `store_real_spectrum_magnitude(output: T[])`
  - `store_real_spectrum_power(output: T[])`
  - `store_real_spectrum_phase(output: T[])`
  - `forward_real(input: T[])`
  - `forward_real_packed(input: T[], output: T[])`
  - `forward_real_magnitude(input: T[], output: T[])`
  - `forward_real_power(input: T[], output: T[])`
  - `forward_real_phase(input: T[], output: T[])`
  - `forward_complex(real: T[], imag: T[])`
  - `forward()`
  - `inverse()`
  - `inverse_real_packed(input: T[], output: T[])`
  - bin accessors returning `T`:
    - `real(i: i32)`
    - `imag(i: i32)`
    - `power(i: i32)`
    - `magnitude(i: i32)`
    - `phase(i: i32)`

Streaming wrappers:
- `std::fft<N>::RealFFT<T>`
  - fields:
    - `fft: FFT<T>`
    - `packed: T[N]`
    - `ready: bool`
  - methods:
    - `clear()`
    - `size() -> i32`
    - `real_bin_count() -> i32`
    - `hop_size() -> i32`
    - `set_rectangular()`
    - `set_hann()`
    - `push(x: T) -> bool`
    - `is_ready() -> bool`
    - `packed_value(i: i32) -> T`
- `std::fft<N>::RealIFFT<T>`
  - fields:
    - `fft: FFT<T>`
  - methods:
    - `clear()`
    - `size() -> i32`
    - `hop_size() -> i32`
    - `set_rectangular()`
    - `set_hann()`
    - `load_packed(input: T[])`
    - `load_complex(real: T[], imag: T[])`
    - `tick() -> T`
    - `is_active() -> bool`

`RealFFT` / `RealIFFT` are streaming STFT-style wrappers:
- default window is Hann
- default hop is `N / 2`
- `RealFFT.push()` emits a new spectrum every hop after the first full frame
- `RealIFFT` performs windowed overlap-add reconstruction and normalizes by the accumulated window power

`std/complex` provides a simple generic complex-number struct for FFT-style arithmetic:

```omni
import std/complex

init:
  z: std::complex::Complex<f32>
  w: std::complex::Complex<f32>
  z.set(1.0, 2.0)
  w.set(3.0, -4.0)
  z.mul_assign(w)
```

Current API:
- `std::complex::Complex<T>`
- `T` is intended for floating-point use (`f32` or `f64`)
- fields:
  - `re: T`
  - `im: T`
- methods:
  - `real()`
  - `imag()`
  - `set(re, im)`
  - `clear()`
  - `copy(other: Complex)`
  - `set_polar(magnitude, phase)`
  - `add_assign(other: Complex)`
  - `add_parts(re, im)`
  - `sub_assign(other: Complex)`
  - `sub_parts(re, im)`
  - `mul_assign(other: Complex)`
  - `mul_parts(re, im)`
  - `scale_assign(gain)`
  - `conjugate()`
  - `power()`
  - `magnitude()`
  - `phase()`

Example:

```omni
import std/fft
import std/osc

params:
  freq = 440.0

init:
  saw = std::osc::Saw(freq = freq)
  fwd = std::fft<64>::RealFFT()
  inv = std::fft<64>::RealIFFT()
  scratch_re: f32[64]
  scratch_im: f32[64]

sample:
  saw.freq = freq

  if (fwd.push(saw())):
    for i in 0..64:
      scratch_re[i] = 0.0
      scratch_im[i] = 0.0

    scratch_re[0] = fwd.fft.re[0]

    half = 64 >> 1
    for k in 1..half:
      shifted = k + 1
      if (shifted < half):
        scratch_re[shifted] = fwd.fft.re[k]
        scratch_im[shifted] = fwd.fft.im[k]
        scratch_re[64 - shifted] = fwd.fft.re[64 - k]
        scratch_im[64 - shifted] = fwd.fft.im[64 - k]

    inv.load_complex(scratch_re, scratch_im)

  out1 = inv.tick()
```

## 12 Example-driven starting points

Useful examples in `examples/`:
- Basic oscillator: `sine.omni`, `std_sine.omni`
- Block/sample structure: `block_counter.omni`, `saw_blep.omni`
- Struct + methods: `cross_fm.omni`
- Processor usage and output forms: `proc_gain.omni`, `proc_split.omni`, `proc_array_stereo_sine.omni`, `reverb.omni`
- Array-heavy DSP: `karplus_strong_data.omni`, `multitap_feedback_struct_data.omni`
- Stdlib and generics: `stdlib_f32.omni`, `stdlib_f64.omni`, `fft_bin_shift.omni`

