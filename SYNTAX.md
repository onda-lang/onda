# Omni Syntax

This document describes the syntax currently implemented in `omni-llvm`.
It is organized from the simplest idea in Omni, the block, through reusable abstractions and multi-file programs.

## 1. Reading an Omni file

Omni supports both indentation style and brace style syntax.

```omni
outs:
  out1

sample:
  out1 = 0.0
```

```omni
outs { 
  out1 
}

sample { 
  out1 = 0.0
}
```

Basic syntax rules:
- statements can be separated by newline or `;`
- line comments use `#`
- top-level declarations are read in lexical order
- names must be introduced before they are used

## 2. What is a block?

An Omni program is mostly a collection of named blocks.
A block is a section such as `params:`, `init:`, or `sample:` that groups related declarations or executable code.

The main top-level blocks are:
- `ins`
- `params`
- `events`
- `buffers`
- `outs`
- `init`
- `block`
- `sample`
- `graph`
- `const`
- `def`
- `struct`
- `proc` / `processor`
- `namespace`

You will also see file-level declarations that are not blocks:
- `import module/path`
- `include "path.omni"`

There are two big categories:

- Declaration blocks
  - `ins`, `params`, `events`, `buffers`, `outs`, `const`, `def`, `struct`, `proc`, `namespace`
- Executable blocks
  - `init`, `block`, `sample`, `graph`

The executable model is:
- `init` runs when an instance is created or reset
- `block` runs once per host block and can contain a nested `sample`
- `sample` runs once per sample
- `graph` is an alternative to `sample` / `block` and lets you describe routing declaratively

The same basic idea applies both at the top level and inside `proc` declarations.

## 3. Types you will see in blocks

Primitive types:
- `f32`
- `f64`
- `i32`
- `i64`
- `bool`

Compound types:
- fixed arrays: `T[N]`
- tuples: `(T1, T2, ...)`
- buffers:
  - `buffer[T]`
  - `buffer[T[2]]`
  - `buffer[T[]]`

Examples:

```omni
ins:
  audio: f32[2]

params:
  mode: i32 = 0

init:
  taps: f32[8]
  pair = (0.0, 1.0)

buffers:
  src: [f32]
  bus: [f32[2]]
```

## 4. The core blocks

This section walks through the blocks in the order you asked for: `ins`, `params`, `events`, `buffers`, `outs`, `init`, `block`, `sample`, then `graph`.

### 4.1 `ins`

`ins` declares input ports.

```omni
ins:
  in1
  side: f64
  stereo: f32[2]
```

Count shorthand is supported:

```omni
ins 2
```

You can also combine a count with explicit declarations:

```omni
ins 2:
  left
  right
```

Section default types are supported:

```omni
ins<f64>:
  left
  right
  meter: f32
```

Rules:
- omitted input types default to `f32`
- explicit entry types override the section default type
- `ins N` expands to `in1..inN`
- the count can be a compile-time integer expression
- if both a count and an explicit declaration list are present, they must match exactly
- if `inN` is used without an `ins` block, it is implicitly created as `f32`
- compile-time count expressions can use ordinary `const` values and namespace integer template parameters

Scalar ranges are supported on scalar `ins` only:

```omni
ins:
  freq = 440.0 {20.0, 20000.0}
  drive: i32 = 2 {8}
```

Dynamic indexed access is supported when the inputs were declared explicitly:

```omni
const N = 4

ins N
outs N

sample:
  for i in 0..N:
    outs[i] = ins[i] * 0.5
```

Dynamic input indexing rules:
- `ins[i]` is 0-based
- runtime indices are clamped to the valid range
- dynamic indexing requires an explicit `ins` declaration block
- implicit inputs created by using `in1`, `in2`, and so on cannot be indexed

### 4.2 `params`

`params` declares tweakable parameters.

```omni
params:
  gain = 1.0
  mode: i32 = 0
  spread: f32[2] = [0.25, 0.75]
```

Count shorthand and section default types work the same way as `ins`:

```omni
params 2

params<i32>:
  mode
  octave = 0
```

Scalar ranges are supported:

```omni
params:
  freq = 440.0 {20.0, 20000.0}
  feedback = 0.5 {0.0, 0.99}
  voices: i32 = 4 {16}
```

Rules:
- omitted parameter types default to `f32`
- ranges are supported only on scalar params
- arrays are supported, but array params cannot have ranges
- `params[i]` is supported under the same dynamic-indexing rules as `ins[i]`
- top-level params are readable in executable code but are not writable from top-level event handlers
- proc constructor arguments for params are named-only
- scalar parameter families can use count shorthand such as `params N`, while fixed-size parameter arrays can use types such as `T[N]`

### 4.3 `events`

`events` declares callable event handlers.
At the top level, events are host-triggered.
Inside a `proc`, events are receiver-only commands that you call on a proc instance.

Top-level example:

```omni
events:
  note_on(freq_hz = 440.0, amp = 1.0):
    freq_state = freq_hz
    amp_state = amp
    gate = true

  note_off():
    gate = false
```

Proc-level example:

```omni
proc Voice:
  params:
    amp = 0.0

  events:
    note_on(v: f32):
      amp = v

  sample:
    out1 = amp
```

Supported event parameter types:
- primitive scalars
- fixed-size primitive arrays: `T[N]`
- read-only primitive slices: `T[]`
- for proc events only, generic primitive slices such as `T[]` when `T` is a proc generic parameter specialized to a primitive

Rules:
- event params without an explicit type default to `f32`
- fixed-array and slice params are read-only in handlers
- top-level events run immediately on the audio thread
- proc events are reached through explicit receiver calls such as `voice.note_on(...)`
- proc-event calls are statement-only, not expressions
- unqualified calls never resolve to proc events
- top-level handlers may write only to existing top-level state rooted in `init`
- proc handlers may write proc state rooted in `init` declarations and proc params
- handlers cannot write inputs or outputs
- top-level event handlers cannot write top-level params
- unknown top-level event indices are ignored
- a known top-level event with the wrong payload size is a runtime error
- for top-level host events with slice params, the payload layout is `i32 len` followed by contiguous element bytes

Every proc also gets a builtin reserved `init(...)` event:
- it mirrors the proc params in declaration order
- it uses the concrete specialized param types
- it cannot be redefined in the proc `events` block
- it assigns the provided values into the proc params

That makes calls such as these legal:

```omni
voice.init(0.5)
voice.init(gain = 0.5)
voices[i].init(freq = 220.0, amp = 0.1)
```

### 4.4 `buffers`

`buffers` declares external host-bound buffers.

```omni
buffers:
  src: buffer[f32]
  bus: buffer[f32[2]]
  any_bus: buffer[f32[]]
```

Inside a `buffers` block, shorthand forms are accepted:

```omni
buffers:
  mono: f32
  stereo: f32[2]
  dyn: f32[]
```

Count shorthand is supported:

```omni
buffers 2
```

Section default type shorthand is also supported:

```omni
buffers[f32]:
  delay
  scratch
```

Buffer access:

```omni
sample:
  mono0 = src[0]
  left0 = bus[0][0]
  right0 = bus[1][0]
```

Buffer methods:
- `buf.len()` returns the frame count
- `buf.chans()` returns the channel count
- `buf.samplerate()` returns the bound buffer sample rate as `f32`

There is currently no public flattened-length or `total_len` method.

Other supported operations:
- method-style and free-function unchecked access:
  - `buf.unsafe_read(i)`
  - `buf.unsafe_write(i, v)`
  - `unsafe_read(buf, i)`
  - `unsafe_write(buf, i, v)`
- primitive buffer slicing, covered later in the arrays section

Rules:
- `buffers N` expands to `buf1..bufN`
- explicit declarations and count shorthand cannot currently be mixed in the same `buffers` block
- `buffers` count shorthand accepts the same compile-time integer expressions as other section counts, including `const` values and namespace integer template parameters
- runtime binding validates element type and channel constraints

### 4.5 `outs`

`outs` declares output ports.

```omni
outs:
  out1
  stereo: f32[2]
```

Count shorthand and section default types work the same way as `ins`:

```omni
outs 2

outs<f64>:
  left
  right
```

Rules:
- omitted output types default to `f32`
- `outs N` expands to `out1..outN`
- if `outN` is used without an `outs` block, it is implicitly created as `f32`
- `outs[i] = expr` is supported when outputs were declared explicitly and uses clamped 0-based runtime indexing

### 4.6 `init`

`init` is where persistent state is created and where structs and processors are usually constructed.
It runs on instance creation and reset.

```omni
init:
  phase = 0.0
  gain = 0.5
  taps: f32[8]
```

Typical uses:
- create persistent scalar state
- create arrays and tuples
- construct structs
- construct proc instances
- perform one-time setup

Section default scalar types are supported:

```omni
init<f64>:
  phase = 0.0
  last = 0.0
```

Rules:
- a fresh top-level scalar assignment in `init` introduces persistent owner state
- a fresh assignment inside nested control flow in `init` is local to that `init` flow, not persistent state
- assigning to an already visible state symbol updates that state
- `const` declarations are allowed inside `init`
- declaration order is lexical

Example:

```omni
init:
  if ready:
    tmp = 1.0
  else:
    tmp = 2.0
  carried = tmp
```

`tmp` is local to `init`, but because every branch assigns it, it is available later in the same `init` flow.
`carried` becomes persistent state.

For the normative storage/scoping rules, see [SCOPING.md](SCOPING.md).

### 4.7 `block`

`block` is the per-audio-block executable scope.
It is useful when a value should be computed once per host block rather than once per sample.

Structure:

```omni
block:
  precomputed = freq * TWO_PI / SR

  sample:
    phase = phase + precomputed
    out1 = sin(phase)
```

You can think of it as:
- `block pre`
- nested `sample`
- `block post`

Rules:
- top-level statements before the nested `sample:` are block-pre code
- statements after the nested `sample:` are block-post code
- fresh top-level assignments in block-pre introduce block-carried owner state visible to later `sample` and block-post code
- fresh top-level assignments in block-post are visible only after that point
- fresh nested assignments inside `if`, `for`, and `while` stay local
- `block` and `sample` are mutually exclusive with `graph` in the same owner

This is the common pattern for derived per-block values:

```omni
block:
  incr = freq * TWO_PI / SR

  sample:
    phase = phase + incr
    if phase > TWO_PI:
      phase = phase - TWO_PI
    out1 = sin(phase)
```

### 4.8 `sample`

`sample` is the per-sample executable scope.

```omni
sample:
  out1 = in1 * gain
```

Rules:
- fresh assignments in `sample` create locals
- `sample` does not introduce new persistent owner state
- `return` is valid in `def` bodies, not in top-level `sample`

Sample oversampling is supported:

```omni
sample 4:
  out1 = tanh(in1 * 8.0)
```

Rules for oversampled `sample` blocks:
- allowed factors are `1`, `2`, `4`, `8`, `16`, `32`, `64`
- `sample:` is equivalent to `sample 1:`
- the factor must be an integer literal
- invalid factors are semantic errors

Runtime behavior of oversampling:
- input reads are interpolated across oversample substeps
- params are held within the base sample
- outputs are filtered and decimated back to the base rate

### 4.9 `graph`

`graph` describes routing declaratively.
Use it when you want the compiler to wire together proc instances and signal flow for you.

```omni
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

`graph` is supported both at the top level and inside processors.

Supported edge forms:

```omni
src >> dst
dst << src
@block src >> dst
@sample src >> dst
src >>[expr] dst
src >> { a, b }
{ a, b } << src
```

Important rules:
- `graph` is mutually exclusive with `sample` and `block` in the same owner
- `init` may still be used together with `graph`
- proc instances used as graph nodes are typically created in `init`
- unannotated edges targeting proc params default to `@block`
- unannotated edges targeting other destinations default to `@sample`
- `@sample` can override the default `@block` behavior for proc param destinations
- delayed edges use `>>[expr]` or `<<[expr]`, where `expr` must be a compile-time nonnegative integer expression
- delayed edges are sample-rate only
- `>>[0]` does not break a cycle

Current graph sources include:
- top-level inputs and params
- proc outputs
- proc-array slot outputs
- array literals such as `[a, b]`
- indexed reads
- sliced reads
- whole-array reads
- arithmetic and logical expressions built from supported graph sources
- element-wise array expressions where the final shape matches the destination

Current legal destinations include:
- top-level outputs
- proc inputs
- proc params
- proc-array slot inputs and params

Type and scheduling rules:
- graph edges use strict shape matching
- scalar-to-fixed-array broadcast is allowed
- each destination has a single writer
- fan-out is allowed
- cycles are rejected unless a positive sample delay breaks the cycle
- proc nodes are stepped implicitly according to graph reachability and topological order
- graph lowering can be inspected with `omni compile <file> --dump-graph`

Current graph limits:
- user-defined function calls and proc calls are not supported inside graph source expressions
- array-constructor expressions such as `f32[2](...)` are not graph sources
- graph event propagation syntax does not exist; use ordinary `events` blocks for event routing

Proc bundles can route directly into destination sets:

```omni
graph:
  reverb >> { out1, out2 }
  voices[0] >> { left, right }
```

Those forms:
- zip by output order when the proc has the same number of outputs as the destination set
- broadcast when the proc has exactly one output
- otherwise produce a semantic error

## 5. Common syntax inside executable code

Once you know what the major blocks are, the rest of the language is mostly the syntax you use inside `init`, `block`, `sample`, `events`, and `def`.

### 5.1 Variables, assignment, and typing

First assignment infers a type by default:

```omni
sample:
  x = 0
  y = 0.0
```

Explicit declarations pin the type:

```omni
sample:
  x: i64 = 0
```

Assignment rules:
- assigning to an existing visible symbol updates it
- assigning to a new symbol introduces a new symbol according to the storage/scope rules of the current executable scope
- declaration order is lexical

### 5.2 Operators and builtin constants

Supported operators:
- arithmetic: `+`, `-`, `*`, `/`, `%`
- comparisons: `==`, `!=`, `<`, `<=`, `>`, `>=`
- logical: `!`, `&&`, `||`
- bitwise integer ops: `~`, `&`, `|`, `^`, `<<`, `>>`

Bitwise rules:
- bitwise operators accept `i32` and `i64` only
- mixed `i32` and `i64` operands widen to `i64`
- `>>` is an arithmetic right shift

Builtin compile-time constants:
- `PI`, `pi`
- `TWO_PI`, `TWOPI`, `two_pi`, `twopi`
- `SAMPLE_RATE`, `SAMPLERATE`, `SR`, `sample_rate`, `samplerate`
- `BLOCK_SIZE`, `BLOCKSIZE`, `BS`, `block_size`, `blocksize`

Default builtin constant types:
- `PI` and `TWO_PI` are `f64`
- `SAMPLE_RATE` is `f32`
- `BLOCK_SIZE` is `i32`

### 5.2.1 Numeric literals, precision, and automatic narrowing

Numeric literals in Omni have two related behaviors:
- untyped first-assignment inference keeps the language's usual defaults
- pure numeric literal expressions adapt automatically to the surrounding numeric context

What that means in practice:
- an untyped decimal literal such as `0.5` defaults to `f32` when it introduces a new symbol
- an untyped integer literal such as `5` defaults to `i32` when it fits in `i32`, otherwise `i64`
- builtin constants such as `PI` and `TWO_PI` are `f64`
- pure numeric expressions such as `0.5 + 0.25`, `PI * 2.0`, or `TWO_PI / SR` can be narrowed automatically when used in an `f32` or `i32` context

Examples:

```omni
sample:
  x = 0.5        # x becomes f32
  n = 5          # n becomes i32
```

```omni
init:
  phase: f32 = 0.0

block:
  tau = TWO_PI          # tau becomes f32 here
  incr = freq * TWO_PI / SR
```

Because `TWO_PI` is part of a pure numeric literal expression, you usually do not need to write:

```omni
tau = f32(TWO_PI)
```

The language already handles that narrowing in the common case.

Explicit casts are still useful when you want to force a particular type on purpose, for example:
- to pin a declaration as `f64` or `i64`
- to make a mixed-type expression obvious
- to disambiguate overload resolution
- to deliberately opt out of the default literal behavior

User-defined `const` values follow the same general rule:
- untyped `const X = expr` uses the inferred type of the constant expression
- typed `const X: T = expr` evaluates `expr` as `T`

That means typed constants preserve the precision of their declared type, while untyped code still gets Omni's normal `f32` / `i32` defaults where appropriate.

User-defined compile-time constants:

```omni
const MaxVoices = 8
const Hop: i32 = BLOCK_SIZE / 2
```

Rules:
- `const NAME = expr` and `const NAME: T = expr` are supported
- `expr` must be compile-time evaluable
- `const` is supported at top level, inside namespaces, and inside executable scopes
- namespace consts can be referenced through qualified paths such as `NS::VALUE`
- reassignment is rejected
- forward references are not currently supported

### 5.3 Control flow

Supported forms:
- `if (...)`
- `if (...) elif (...) else`
- `for i in A..B`
- `for i in A..=B`
- `for i @ STEP in A..B`
- `loop N`
- `while (...)`
- `break`
- `continue`
- `return`

Examples:

```omni
for i in 0..8:
  sum = sum + taps[i]

for i @ -1 in 10..0:
  dst[i] = src[i]
```

Loop rules:
- `@ STEP` defaults to `1`
- `@ 0` is invalid
- descending loops use a negative step
- loop variables are local to the loop body
- a fresh symbol created inside a loop does not escape the loop

### 5.4 Arrays and slices

Fixed-size arrays are supported in state and local scopes:

```omni
init:
  taps: f32[8]

sample:
  coeffs = [0.5, 0.25, 0.125]
```

Untyped array literals are supported where executable-scope declarations are valid:

```omni
sample:
  a = [0.5, 0.8]
  b = [i64(0), 1]
```

Primitive array and buffer slices use Python-style syntax:

```omni
sample:
  a = buf[:]
  b = buf[2:]
  c = buf[:-1]
  d = buf[1:-2]
```

Rules:
- slice forms are `a[:]`, `a[start:]`, `a[:end]`, `a[start:end]`
- negative bounds are supported
- slice expressions lower to primitive slice views of type `T[]`
- buffer slicing also yields `T[]`

Writable slice assignment is supported:

```omni
sample:
  values[1:-1] = 0.5
  dst[:] = src[:]
```

Rules:
- slice assignment is statement-only
- scalar fill writes the full target slice
- slice copy writes `min(dst_len, src_len)` elements
- overlapping slice copies behave as if they were copied through a temporary
- event payload arrays and slices are read-only and cannot be writable slice targets
- struct-element arrays are not sliceable in the current implementation

### 5.5 Tuples

Tuples are anonymous fixed-length heterogeneous compound values.

```omni
sample:
  pair = (1.0, 2.0)
  triple = (1.0, 42, true)
```

Tuple syntax and rules:
- type syntax is `(T1, T2, ...)`
- maximum arity is 16
- nested tuples are not currently supported
- tuple element access uses compile-time integer indices only

```omni
sample:
  pair = (3.0, 7.0)
  out1 = pair[0]
  out2 = pair[1]
```

Tuple destructuring is supported:

```omni
sample:
  (a, b) = (10.0, 20.0)
```

Tuples can appear:
- in local variables
- in `init` state
- as `def` parameters
- as `def` return values
- as struct fields

Tuples use a flattened ABI internally.

## 6. Functions with `def`

`def` declares reusable functions.

```omni
def wrap_phase(p, upper = TWO_PI):
  if p > upper:
    return p - upper
  return p
```

Supported features:
- positional arguments
- named arguments
- default values
- early return
- multi-line argument lists with an optional trailing comma

Return types:
- a `def` can return a primitive scalar
- a `def` can return a tuple of primitives
- returning structs, arrays, or buffers is not supported

Top-level `def` bodies are lexical-local:
- top-level runtime symbols such as `ins`, `outs`, `params`, `buffers`, and top-level `init` state are not in scope unless passed explicitly

Method-style sugar is supported for ordinary defs:

```omni
def clamp01(x):
  return clamp(x, 0.0, 1.0)

sample:
  out1 = in1.clamp01()
```

That rewrites to `clamp01(in1)`.

### 6.1 `def` parameter kinds

In addition to primitive scalars, `def` parameters support:
- explicit struct types
- typed arrays such as `arr: f32[]`
- untyped arrays such as `arr: []`
- typed buffers such as `buf: buffer[f32]`
- bare buffers such as `buf: buffer`
- generic struct and proc parameters specialized at the call site
- typed tuple params such as `p: (f32, i32)`
- untyped tuple params specialized from the call site

Examples:

```omni
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

Untyped parameters can also be specialized structurally from how they are used inside the `def`.
That means a function like this is valid:

```omni
struct A:
  x: f32
  y: f32

struct B:
  x: f32

def read_x(s):
  return s.x
```

and can be called with both `A` and `B`:

```omni
sample:
  out1 = read_x(a) + read_x(b)
```

The compiler monomorphizes `read_x` from the call sites and the way `s` is used in the body.
In this example, `s.x` means the argument must provide an `x` field with a compatible type.

### 6.2 Overloads and resolution

Top-level defs and struct methods may be overloaded by arity and parameter types.

```omni
def sat(x: f32):
  return x

def sat(x: f64):
  return f32(x)
```

Resolution rules:
- exact typed match wins first
- if no exact typed match exists, numeric widening candidates may be used
- explicit typed parameters outrank generic or duck-typed parameters
- generic or duck-typed parameters outrank untyped parameters
- default arguments participate in overload matching
- return type is not part of overload selection
- equally valid candidates are a semantic error

Current overload support:
- top-level `def`
- struct methods

Current non-support:
- proc-local defs are not overloadable
- explicit `def` type parameter syntax such as `def fn<T>` is intentionally unsupported

## 7. Structs

`struct` declares nominal data types with fields and methods.

```omni
struct Voice:
  phase: f32
  sig: f32

  def tick(self, hz):
    self.phase = self.phase + hz * TWO_PI / SR
    self.sig = sin(self.phase)
```

Supported features:
- field defaults
- methods
- overloaded methods
- tuple fields
- generic structs, covered later

Method rules:
- `self` must be the first method parameter
- methods can read and write struct fields through `self`

Construction:

```omni
init:
  a = Voice()
```

Typed struct declarations are supported in `init`:

```omni
init:
  a: Voice = Voice()
  b: Voice
```

Rules for typed `init` declarations:
- typed struct declarations are `init`-only
- declaration-only form such as `b: Voice` desugars to default-constructor initialization
- for generic structs, typed declarations require explicit type args when the type is still generic

### 7.1 Indexed struct-array field access

For arrays of data structs, one inline field-access dot is supported:

```omni
sample:
  gain = voices[i].level
  tap = voices[i].taps[j]
```

Accepted forms:
- `base[idx].field`
- `base[idx].field[fidx]`

Rejected deeper inline chains:
- `base[idx].field.other`
- `base[idx].field[fidx].other`

Use an intermediate alias when the chain is deeper:

```omni
sample:
  v = voices[i]
  gain = v.settings.level
```

Proc arrays keep their own proc-specific indexed forms such as `voices[i].gain`, `voices[i](...)`, and `voices[i].note_on(...)`.

## 8. Generics

Generics are supported for `struct` and `proc`.
They are compile-time generics, not runtime generics.

```omni
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

### 8.1 What "monomorphization" means

Omni implements generics by monomorphization.
That means the compiler creates a concrete specialized copy for each generic type combination that your program actually uses.

For example, if you use both:

```omni
a = Pair<f32>()
b = Pair<f64>()
```

the compiler treats those as two concrete specializations:
- `Pair<f32>`
- `Pair<f64>`

Likewise, if you instantiate:
- `OnePole<f32>()`
- `OnePole<f64>()`

the compiler lowers them as two separate concrete processors.

There is no runtime generic dispatch layer here.
By the time code generation happens, the generic owner has been specialized to concrete primitive types.

### 8.2 Specializing generic structs and procs

Type arguments can be:
- explicit: `Pair<f64>()`
- inferred in many constructor cases

Rules:
- generic type arguments are restricted to numeric primitives: `f32`, `f64`, `i32`, `i64`
- `bool` is not allowed as a generic type argument
- unresolved generic type parameters in declaration and type positions are errors
- for untyped constructor assignments only, unresolved constructor type parameters default to `f32`

Examples:

```omni
init:
  a = Pair<f32>()
  b = Pair<f64>()
```

```omni
init:
  lp = OnePole()       # unresolved generic constructor defaults to f32 here
  hp = OnePole<f64>()  # explicit specialization
```

### 8.3 What can use the generic type parameter

Inside a specialized generic owner:
- `T(expr)` rewrites to the bound primitive cast
- `T[]` is valid for method and `def` array parameters where a primitive slice would be valid
- typed generic locals such as `x: T = ...` are supported in executable scopes

Generic typed local declarations are supported in:
- `init`
- `sample`
- `block`
- struct methods
- event handlers

In other words, `T` must belong to the current generic owner.
You use it inside the generic `struct` or `proc` that declared it, and after specialization it behaves like an ordinary primitive type.

### 8.4 Generics and `def` monomorphization

Top-level `def` does not use explicit type parameter syntax such as `def fn<T>`.
Instead, polymorphism comes from call-site monomorphization of certain parameter kinds.

These can be monomorphized at the call site:
- untyped arrays such as `arr: []`
- bare buffers such as `buf: buffer`
- generic struct and proc params where the concrete specialization is supplied by the argument
- untyped tuple parameters inferred from tuple literals
- untyped structural params whose bodies access compatible fields or methods

Example:

```omni
def first(arr: []):
  return arr[0]

sample:
  a = [1.0, 2.0]
  b = [1, 2]
  x = first(a)
  y = first(b)
```

The compiler monomorphizes `first` separately for the concrete argument shapes and element types it sees at the call sites.

The same idea applies when a `def` accepts a generic struct or proc parameter:

```omni
struct Box<T>:
  value: T

def read_box(b: Box):
  return b.value
```

If `read_box` is called with both `Box<f32>` and `Box<f64>`, Omni generates concrete specialized versions for those uses.

The same idea also applies to structural untyped params:

```omni
struct A:
  x: f32

struct B:
  x: f32

def read_x(s):
  return s.x
```

If `read_x` is called with both `A` and `B`, Omni resolves and specializes that `def` from the concrete argument shapes at the call sites.

### 8.5 Practical rules of thumb

In practice:
- use generics when the same struct or proc should work over multiple numeric primitive types
- think of each used specialization as its own concrete type
- use explicit casts only when you want to force a type, not because builtin constants like `TWO_PI` need help

## 9. Reusable processors with `proc`

`proc` is Omni's reusable processing unit.
Use it when you want stateful, composable DSP building blocks.

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

`processor` is an alias for `proc`.

### 9.1 What a proc can contain

A proc can contain:
- `ins`
- `params`
- `events`
- `buffers`
- `outs`
- `init`
- `block`
- `sample`
- `graph`
- proc-local `def` helpers

In practice:
- `init` is optional
- `events` is optional
- `block` is optional
- a proc normally has either `sample`, `block`, or `graph` as its execution body

### 9.2 Constructing and calling procs

Proc instances are usually created in `init`:

```omni
init:
  g = Gain(g = 0.5)
```

Constructor rules:
- proc constructor arguments for params and buffers are named-only
- generic procs specialize on construction

Call and access forms:
- `p(...)`
- `p(...).out1`
- `p(...).endpointName`
- `p.out1`
- `p.endpointName`
- statement call form: `p(...)`

For single-output procs, `p(...)` is scalar sugar for the first output.

### 9.3 Proc-local defs

Processors can declare private helper defs that implicitly see proc state.

```omni
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

  events:
    reset():
      reset_state()

  sample:
    out1 = apply(in1)
```

Rules:
- proc-local defs are private to the enclosing proc
- they can be called from proc `init`, `block`, `sample`, `events`, and other proc-local defs
- they can read and write proc state directly, without `self`
- they support parameters, defaults, named arguments, and return values like normal defs
- recursive and mutually recursive proc-local defs are rejected
- proc-local defs are not overloadable

### 9.4 Proc arrays

Arrays of proc instances are supported in `init`.

```omni
init:
  voices: Voice[4] = Voice()
```

Supported proc-array forms:
- literal array construction: `voices: Voice[2] = [Voice(), Voice()]`
- broadcast constructor sugar: `voices: Voice[4] = Voice()`
- compile-time capacity expressions in the array length

Indexed proc-array operations:
- `voices[i](...)`
- `voices[i](...).out1`
- `voices[i].gain`
- `voices[i].gain = value`
- `voices[i].note_on(...)`
- aliasing such as `v = voices[i]`, then `v(...)`

Rules:
- runtime indices are clamped to the valid slot range
- proc-array buffer refs are refreshed on the safe `process_bound` path
- a proc cannot directly instantiate its own type in its own state

If the proc defines a `block` section, indexed proc-array calls use active-slot block-hook semantics:
- `block pre` runs lazily on the first `()` call to that slot in the current block
- `block post` runs once at block end for each slot that was called
- plain slot retrieval alone does not trigger hooks

### 9.5 Using procs in graphs

Procs become especially powerful when combined with `graph`.

```omni
import std/osc

proc StereoGain:
  ins:
    in: f32[2]

  params:
    gain: f32[2] = [1.0, 1.0]

  outs:
    out: f32[2]

  sample:
    out[0] = in[0] * gain[0]
    out[1] = in[1] * gain[1]

outs:
  out: f32[2]

init:
  osc_l = std::osc::Sine<f32>(freq = 220.0)
  osc_r = std::osc::Sine<f32>(freq = 330.0)
  p = StereoGain(gain = [1.0, 0.1])

graph:
  [osc_l.out1, osc_r.out1] >> p.in
  p.out >> out
```

Graph/proc integration rules:
- proc inputs and params are legal graph destinations
- proc outputs are legal graph sources
- proc-array slot inputs, params, and outputs are supported
- bare proc instances and proc-array slots can route into destination sets, using the zip or broadcast rules described in the graph section

## 10. Modularity: namespaces, modules, and imports

Once the core language is clear, the last big piece is how to split programs across files and build reusable modules.

### 10.1 `import`

Use `import` to load another module by path:

```omni
import reverb
import std/osc
import std/filter
```

Resolution rules:
- `import module/path` resolves as `module/path.omni`
- each imported file is imported once
- built-in std modules are available under `std/...`

Current built-in std modules include:
- `std/prelude`
- `std/math`
- `std/export_math`
- `std/complex`
- `std/osc`
- `std/filter`
- `std/env`
- `std/delay`
- `std/data`
- `std/lookup`
- `std/fft`
- `std/convolution`

`std/prelude` is auto-imported during semantic analysis.
Today it brings in `std/math` and `std/lookup`.

Current imported-file restriction:
- declaration-only files are limited to `const`, `struct`, `def`, and `proc`

### 10.2 `include`

`include` inserts another `.omni` file by quoted path:

```omni
include "shared/reverb.omni"
```

Rules:
- the path must be quoted
- the path must end in `.omni`

### 10.3 `namespace`

`namespace` groups declarations under a qualified path.

```omni
namespace my::dsp:
  def sat(x):
    return clamp(x, -1.0, 1.0)
```

Use sites access declarations with `::`:

```omni
sample:
  out1 = my::dsp::sat(in1)
```

Namespace-local compile-time constants are also supported:

```omni
namespace Config:
  const MaxVoices = 8
```

### 10.4 Templated namespaces

Namespaces can also take compile-time integer parameters:

```omni
namespace FFT<N = 256>:
  assert(N > 0)
  assert((N & (N - 1)) == 0)
```

Rules:
- namespace template params require defaults
- args support positional and named forms
- args are normalized as `i32(...)` at compile time
- namespace-local `assert(expr)` is supported for compile-time checks

Inline instantiation and aliases:

```omni
namespace D = std::data<SR, 1>

init:
  a = std::data<SR, 1>::Data<f64>()
  b = D::Data<f64>()
```

Syntax split:
- `<>` is used for namespace instantiation, generic specialization, and section default type modifiers
- `[]` is used for arrays, indexing, slices, and buffer/channel forms

### 10.5 Integer namespace params for sizes and arity

Namespace template parameters are compile-time integers.
They are the main way to make Omni libraries generic over counts, lengths, and arity.

You can use a namespace integer parameter:
- in fixed array sizes such as `T[N]`
- in section counts such as `ins N`, `outs N`, `params N`, and `buffers N`
- in `for` loop bounds
- in compile-time expressions and namespace `assert(...)` checks

That means Omni has two complementary generic mechanisms:
- type generics such as `T` control the numeric scalar type
- namespace integer generics such as `N` control counts and sizes

Together they let you build reusable libraries that are generic over both type and channel/input count.

Example:

```omni
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

In that example:
- `Channels` controls how many inputs and outputs the proc has
- `Channels` also controls the fixed size of the `gains` parameter array
- `T` controls the scalar type used by the ports and parameter array

This is the standard pattern for an Omni library that should work for "any `N` channels of `T`".

At the top level, you use it by instantiating the namespace and then constructing the proc specialization you want:

```omni
outs:
  out: f32[2]

init:
  g = DSP<2>::Gain<f32>(gains = [0.5, 0.25])

graph:
  [in1, in2] >> g
  g.out >> out
```

You can also use a namespace alias when you want to fix the integer parameters once and reuse them:

```omni
namespace Stereo = DSP<2>

init:
  g = Stereo::Gain<f32>(gains = [0.5, 0.25])
```

Related notes:
- ordinary `const` values can also be used in array sizes and section counts
- namespace consts can be derived from namespace integer params and reused elsewhere

## 11. Examples that put it all together

These examples in `examples/` cover the language in progressively richer combinations.

### 11.1 Small stateful patch

`examples/sine.omni`

Why it is useful:
- simple `params`
- persistent `init` state
- `block` plus nested `sample`
- direct output writing

### 11.2 Structs plus reusable defs

`examples/cross_fm.omni`

Why it is useful:
- top-level `def`
- `struct` fields and methods
- stateful objects in `init`
- per-sample interaction between multiple voices

### 11.3 Proc plus graph wiring

`examples/proc_gain_graph.omni`

Why it is useful:
- small reusable `proc`
- proc construction in `init`
- `graph` routing into proc inputs and params
- proc output routed to top-level outputs

### 11.4 Proc arrays plus helper defs

`examples/proc_array_init_harmonics.omni`

Why it is useful:
- arrays of proc instances
- ordinary top-level defs that operate on proc arrays
- builtin proc `init(...)`
- `block` plus nested `sample`

### 11.5 Modular multi-file patch

`examples/reverb.omni` plus `examples/reverb_graph.omni`

Why it is useful:
- split code across modules
- import a local reusable proc from another file
- combine stdlib imports with local imports
- drive a larger proc through `graph`

### 11.6 Generic proc in a larger graph

`examples/cybernetic_feedback_graph.omni`

Why it is useful:
- generic `proc`
- proc specialization with `f64`
- delayed graph edges
- larger graph composition with multiple reusable nodes

### 11.7 Event-driven patch

`examples/preview_events.omni`

Why it is useful:
- top-level `events`
- persistent state mutation from host events
- proc use inside a playable patch

## 12. Summary

If you are new to Omni, the most useful learning order is:
1. understand the block model: `ins`, `params`, `events`, `buffers`, `outs`, `init`, `block`, `sample`, `graph`
2. learn the executable-scope rules for state and locals
3. learn `def` and `struct`
4. learn generics
5. learn `proc` as the main reusable DSP abstraction
6. finish with modules, namespaces, and imports

That path matches how real Omni programs in this repository are structured.
