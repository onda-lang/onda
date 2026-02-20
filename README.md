# omni-llvm

Rust workspace for the Omni audio DSL compiler/runtime (LLVM ORC JIT + C ABI).

## Status

Current implementation includes:
- frontend parser (`pest`) + AST + diagnostics
- semantic analysis and typing
- LLVM ORC JIT backend
- runtime API and C ABI
- CLI compile/render commands

The compiler currently targets ORC JIT only.

## Omni Syntax Guide

### Block styles

Omni supports both brace style and indentation style.

```omni
outs {
  out1
}
sample {
  out1 = 0.0
}
```

```omni
outs:
  out1
sample:
  out1 = 0.0
```

Statements can be separated by newlines or `;`.

### Top-level blocks

Available top-level blocks:
- `ins`
- `outs`
- `params`
- `buffers`
- `init`
- `block`
- `sample`
- `def`
- `struct`
- `proc` / `processor`
- `namespace`

### Ports, params, and buffers

Basic declarations:

```omni
ins:
  in1
  side: f64

outs:
  out1
  meter: f32

params:
  gain = 1.0
  mode: i32 = 0
  freq = 500 {8000}
ins:
  in1 = 440 {22000}

buffers:
  ext: buffer[f32]
  bus: buffer[f32[2]]
  dyn: buffer[f32[]]
```

Count shorthand:

```omni
ins 2
outs 1
params 3
buffers 2
```

Count prefix with explicit declarations (supported for `ins`, `outs`, `params`):

```omni
ins 1:
  in1 = 440 {22000}

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
  voices = 8
buffers[f32]:
  line
  flags: i32
```

Rules:
- section default applies only when an entry has no explicit type
- explicit entry type always wins
- for `ins`/`outs`/`params`, a count prefix can be combined with explicit declarations (`ins 2:`); the count must match the number of declared entries
- declaration ranges are supported on scalar `ins` and scalar `params` only:
  - `name = default {min, max}`
  - `name = default {max}` (max-only; lower bound defaults to the primitive type minimum)
- ranges on array declarations are rejected
- if `outN`/`inN` are used without declarations, they are implicitly created as `f32`

Clamping behavior:
- top-level `params` with ranges are hoisted into clamped temporaries once per block in generated JIT code
- top-level `ins` with ranges are hoisted into clamped temporaries once per sample in generated JIT code
- proc params are clamped when assigned (`proc_instance.param = expr`)
- if no range is declared, no clamping is performed

### Primitive types and arrays

Supported primitives:
- `f32`
- `f64`
- `i32`
- `i64`
- `bool`

Array type syntax:

```omni
init:
  taps: f32[SR * 2]
  idx: i32 = 0
```

### Variables and assignment typing

By default, first assignment determines type (auto-like behavior):

```omni
sample:
  x = 0        # x is i32
  y = 0.0      # y is f32
```

Explicit typing pins type:

```omni
sample:
  x: i64 = 0
```

After first declaration, symbol type is fixed for its lifetime in that scope/state.

Arithmetic operators:
- `+`, `-`, `*`, `/`, `%`

### Control flow

Supported:
- `if (...) { ... } else { ... }` or `if ... { ... } else { ... }`
- `if (...) { ... } elif (...) { ... } else { ... }` or `if ... { ... } elif ... { ... } else { ... }`
- `for i in A..B { ... }`
- `loop N { ... }` (sugar)

```omni
sample:
  acc = 0.0
  for i in 0..4:
    acc = acc + 0.25
  if (acc > 0.5):
    out1 = acc
  else:
    out1 = 0.0
```

### Functions (`def`)

Functions support:
- positional and named args
- default args
- early `return`

```omni
def wrap_phase(p, upper = TWO_PI):
  if (p > upper):
    return p - upper
  return p
```

`def` generics are intentionally unsupported. Polymorphism for defs is via typed/untyped params and call-site monomorphization.

Return type behavior:
- a `def` returns the type implied by its returned expression(s)
- mixed numeric returns follow widening rules (`i32 -> i64 -> f32 -> f64` where needed)

### Structs

```omni
struct Voice:
  phase: f32
  sig: f32

  def tick(self, hz):
    self.phase = self.phase + hz * TWO_PI / SR
    self.sig = sin(self.phase)
```

Notes:
- fields can be primitive or array-typed
- methods must take `self` as first parameter
- constructors (`Voice(...)`) are only valid as direct assignments in `init`

### Processors (`proc`)

```omni
proc Gain:
  ins:
    in1
  outs:
    out1
  params:
    g = 1.0
  sample:
    out1 = in1 * g
```

Processor execution blocks:
- `init`: one-time setup
- `sample`: per-sample processing (required)
- optional `block` with pre/sample/post split:
  - pre statements: once per callback
  - nested `sample`: per frame
  - post statements: once per callback

```omni
proc Wrapped:
  outs:
    out1
  block:
    k = 0.5
    sample:
      out1 = out1 + k
    k = k + 0.01
```

Processor construction/calls:
- instantiate in `init`: `p = Gain(g = 0.5)`
- call in `sample`: `out1 = p(0.25)`
- multi-out: `p(...)[index]` or statement call + field reads

### Generics (struct/proc)

Generics are supported for `struct` and `proc` with primitive specialization:

```omni
struct Pair[T]:
  a: T
  b: T

proc OnePole[T]:
  ins[T] 1
  outs[T] 1
  params[T]:
    cutoff = 1000.0
  init:
    z: T = 0.0
  sample:
    out1 = z
```

Usage:

```omni
init:
  p1 = Pair[f32]()
  lp = OnePole[f64](cutoff = 800.0)
```

Type args can also be inferred in many constructor cases.

Current generic scope note:
- generic typed local declarations like `x: T = ...` are currently supported in `init` blocks of generic processors

### Buffers and buffer builtins

External buffer declarations:

```omni
buffers:
  mono: buffer[f32]
  stereo: buffer[f32[2]]
  dyn: buffer[f32[]]
```

Access:
- mono: `mono[i]`
- multichannel: `stereo[ch][i]`
- query length/channels: `mono.len()`, `stereo.chans()`
- unchecked builtins: `unsafe_read(...)`, `unsafe_write(...)`

### Imports and namespaces

```omni
import std/osc
import std/filter

namespace my::dsp:
  def sat(x):
    return clamp(x, -1.0, 1.0)
```

Stdlib modules currently include:
- `std/math`
- `std/osc`
- `std/filter`
- `std/env`
- `std/delay`

### Builtin constants

Available constants:
- `PI`
- `TWO_PI` / `TWOPI`
- `SAMPLE_RATE` / `SR`
- `BLOCK_SIZE`

## CLI

Compile:

`cargo run -p omni_cli -- compile path/to/program.omni`

Render WAV:

`cargo run -p omni_cli -- render path/to/program.omni --output ./omni_out.wav --dur 5`

Render defaults:
- `--output`: `./omni_out.wav`
- `--dur`: `5` seconds
- `--sample-rate` / `--sr`: `48000`
- `--block`: `512`

## C API Quickstart

Header:
- `include/omni_llvm.h`

Basic flow:
1. Compile source with `omni_compile`.
2. Inspect metadata (`*_count`, `*_name`, `*_type`, `*_type_bytes`, offsets/ranges/defaults).
3. Create an instance with `omni_instance_create`.
4. Bind params/ins/outs/buffers with `omni_set_param_by_index`, `omni_bind_input`, `omni_bind_output`, `omni_bind_buffer`.
5. Run audio with `omni_process_bound` (or `omni_validate_*` + `omni_process_unchecked`).
6. Destroy instance and program with `omni_instance_destroy` and `omni_program_destroy`.

Return/sentinel conventions:
- process/bind/set/validate APIs: `0` success, negative error.
- index/count/size-like metadata APIs: `-1` invalid input/index.
- pointer-return metadata APIs (`*_name`, `*_type`): `NULL` invalid input/index.
- floating metadata APIs (`*_default_f64`, `*_range_*_f64`): `NaN` when missing/invalid.

Minimal C sketch:

```c
#include "omni_llvm.h"

omni_diag_t diag = {0};
omni_program_t* prog = omni_compile(src_utf8, &diag);
if (!prog) { /* read diag */ }

int in_count = omni_input_count(prog);
int out_count = omni_output_count(prog);
int param_count = omni_param_count(prog);

omni_instance_t* inst = omni_instance_create(prog, 48000.0f, 512, in_count, out_count, &diag);
if (!inst) { /* read diag */ }

/* bind outputs (and inputs if declared), set params, bind buffers if declared */
/* ... */

if (omni_process_bound(inst, 512) != 0) {
  /* handle error */
}

omni_instance_destroy(inst);
omni_program_destroy(prog);
```

## LLVM Toolchain Bootstrap (Windows)

The repository uses vendored LLVM under `.deps` by default.

1. `pwsh ./scripts/bootstrap-llvm.ps1`
2. `pwsh ./scripts/use-llvm-env.ps1`
3. build/test with Cargo

Configured in `.cargo/config.toml`:
- `LLVM_SYS_211_PREFIX = .deps/llvm/21.1.2`

Notes:
- `llvm-sys = 211.x` maps to LLVM 21.1.x APIs
- this project uses ORC APIs through `llvm-sys`
- optional source build bootstrap exists at `scripts/bootstrap-llvm-source.ps1`

## ORC Backend Notes

- ORC feature is wired and tested
- compiler is ORC-only (`Auto` and explicit ORC use ORC)
- lowering uses LLVM `default<O3>` passes and host-target settings

## Examples

- `examples/sine.omni`
- `examples/cross_fm.omni`
- `examples/karplus_strong_data.omni`
- `examples/block_counter.omni`
- `examples/stdlib_f64.omni`

