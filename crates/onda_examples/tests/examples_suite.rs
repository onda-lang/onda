use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use onda_codegen_llvm::{CompileOptions, ExecutionBackend, TargetOptLevel};
use onda_frontend::{parse_program, parse_program_file, Diagnostic, PrimitiveType};
use onda_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_bound, process_unchecked,
    reset_instance_state, set_param_by_index, trigger_event_by_index, validate_bindings,
    validate_buffers, validate_outputs, InstanceConfig,
};
use onda_semantics::{analyze, analyze_with_options, AnalysisOptions};

const GAIN: &str = r#"
ins {
  in1
}
outs {
  out1
}
params {
  gain = 1.0
}
sample {
  out1 = in1 * gain
}
"#;

const SINE: &str = r#"
outs {
  out1
}
params {
  freq = 440.0
}
init {
  phase = 0.0
}
sample {
  phase = phase + freq * f32(TWO_PI) / SR
  out1 = sin(phase)
}
"#;

const ONE_POLE: &str = r#"
ins {
  in1
}
outs {
  out1
}
params {
  a = 0.1
}
init {
  z = 0.0
}
sample {
  z = z + a * (in1 - z)
  out1 = z
}
"#;

const IF_EXAMPLE: &str = r#"
outs { out1 }
params { gate = 1.0 }
sample {
  if (gate > 0.5) { out1 = 0.25 } else { out1 = -0.25 }
}
"#;

const FOR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = 0.0;
  for i in 0..4 { out1 = out1 + i / 10.0 }
}
"#;

#[cfg(feature = "llvm-orc")]
const RESERVED_METHOD_NAMES_EXAMPLE: &str = r#"
struct Ops {
  def len(self) { return 1.25 }
  def chans(self) { return 0.25 }
  def unsafe_read(self, i) { return f32(i) }
  def unsafe_write(self, i, v) { return v + f32(i) }
}
outs { out1 }
init {
  o = Ops()
}
sample {
  out1 = o.len() + o.chans() + o.unsafe_read(1) + o.unsafe_write(2, 0.5)
}
"#;

const COUNT_SHORTHAND_IO_PARAMS_EXAMPLE: &str = r#"
ins 2
outs 2
params 2
sample {
  out1 = in1 + param1
  out2 = in2 + param2
}
"#;

const LOOP_SUGAR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = 0.0
  loop 4 {
    out1 = out1 + 1.0
  }
}
"#;

const FOR_VAR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 4
}
sample {
  out1 = 0.0
  for i in 0..n {
    out1 = out1 + 1.0
  }
}
"#;

const FOR_PAREN_EXPR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 5
}
sample {
  out1 = 0.0
  for i in 0..(n - 1) {
    out1 = out1 + 1.0
  }
}
"#;

const FOR_DESCENDING_STEP_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = 0.0
  for i @ -1 in 3..=1 {
    out1 = out1 + i / 10.0
  }
}
"#;

const LOOP_VAR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 4
}
sample {
  out1 = 0.0
  loop n {
    out1 = out1 + 1.0
  }
}
"#;

const INIT_CONTROL_FLOW_EXAMPLE: &str = r#"
outs { out1 }
init {
  acc = 0.0;
  for i in 0..4 { acc = acc + i / 10.0 }
  if (acc > 0.5) { acc = acc + sin(0.0) + 1.0 } else { acc = -1.0 }
}
sample {
  out1 = acc
}
"#;

const BLOCK_BRANCH_STATE_REGISTRATION_EXAMPLE: &str = r#"
outs { out1 }
block {
  if (1 < 2) {
    tmp = 2.0
  } else {
    tmp = -1.0
  }
  acc = tmp + 1.0
  sample {
    out1 = acc
  }
}
"#;

const SAMPLE_BRANCH_TYPED_REGISTRATION_EXAMPLE: &str = r#"
outs { out1 }
sample {
  if (1 < 2) {
    tmp: f32 = 2.0
  } else {
    tmp: f32 = -1.0
  }
  out1 = tmp + 1.0
}
"#;

const BLOCK_LOOP_CONTROL_EXAMPLE: &str = r#"
outs { out1 }
init {
  block_value = 0.0
}
block {
  block_value = 0.0
  for i in 0..5 {
    if (i == 1) { continue }
    if (i == 4) { break }
    block_value = block_value + 1.0
  }
  sample {
    out1 = block_value
  }
}
"#;

const SAMPLE_LOOP_CONTROL_EXAMPLE: &str = r#"
outs { out1 }
sample {
  sum: i32 = 0
  i: i32 = 0
  while (i < 6) {
    i = i + 1
    if (i == 2) { continue }
    if (i == 5) { break }
    sum = sum + i
  }
  out1 = f32(sum)
}
"#;

const BLOCK_BREAK_OUTSIDE_LOOP_ERROR_EXAMPLE: &str = r#"
outs { out1 }
block {
  break
  sample {
    out1 = 0.0
  }
}
"#;

const SAMPLE_CONTINUE_OUTSIDE_LOOP_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  continue
  out1 = 0.0
}
"#;

const DEF_CALL_EXAMPLE: &str = r#"
outs { out1 }
params { g = 0.5 }
def mul_add(x, gain) {
  y = x * gain
  return y + 0.5
}
sample {
  out1 = mul_add(1.0, g)
}
"#;

const DEF_MONO_NUMERIC_EXAMPLE: &str = r#"
outs { out1 }
def half(x) {
  return x / 2
}
sample {
  out1 = half(3) + half(3.0)
}
"#;

const DEF_NAMED_DEFAULT_ARGS_EXAMPLE: &str = r#"
outs { out1 }
def mix(a, b = 0.25, c = 0.5) {
  return a + b + c
}
sample {
  out1 = mix(1.0, c = 2.0)
}
"#;

const DEF_POSITIONAL_AFTER_NAMED_ERROR_EXAMPLE: &str = r#"
outs { out1 }
def mix(a, b = 0.25) {
  return a + b
}
sample {
  out1 = mix(a = 1.0, 2.0)
}
"#;

const DEF_CANNOT_CAPTURE_TOP_LEVEL_SYMBOLS_ERROR_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
params { p = 0.5 }
buffers { b: buffer[f32] }
init { s = 1.0 }
def leak() {
  return in1 + p + s + b[0]
}
sample {
  out1 = leak()
}
"#;

const DEF_NO_RETURN_EXAMPLE: &str = r#"
outs { out1 }
def no_ret(x) {
  y = x * 2.0
}
sample {
  out1 = no_ret(0.25)
}
"#;

const DEF_EARLY_RETURN_EXAMPLE: &str = r#"
outs { out1 }
def early() {
  return 1.0
  x = 9.0
  return x
}
sample {
  out1 = early()
}
"#;

const DEF_STRUCT_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
def sum_pair(p) {
  return p.a + p.b
}
init {
  p = Pair(0.25, 0.75)
}
sample {
  out1 = sum_pair(p)
}
"#;

const DEF_STRUCT_ARG_BY_REF_WRITEBACK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
def bump(p) {
  p.a = p.a + 1.0
  return p.a
}
init {
  p = Pair(1.0, 0.0)
}
sample {
  out1 = bump(p) + p.a
}
"#;
const DEF_STRUCT_DATA_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4], gain: f32 }
def read_delay(v, idx) {
  return v.delay[idx] * v.gain
}
init {
  v = Voice(0.5)
  v.delay[0.0] = 2.0
}
sample {
  out1 = read_delay(v, 0.0)
}
"#;

const DEF_STRUCT_ARRAY_INDEXED_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { a: f32, b: f32 }
def sum_voice(v: Voice) {
  return v.a + v.b
}
init {
  voices: Voice[2] = [Voice(), Voice()]
  voices[1].a = 1.0
  voices[1].b = 2.0
}
sample {
  out1 = sum_voice(voices[1])
}
"#;

const DEF_STRUCT_ARRAY_INLINE_FIELD_REF_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { gain: f32, taps: f32[2] }
def read_tap(xs: Voice[], idx: i32) {
  return xs[idx].taps[1]
}
def read_mix(xs: Voice[], idx: i32) {
  return xs[idx].gain + read_tap(xs, idx)
}
init {
  idx: i32 = 0
  voices: Voice[2]
  voices[0].gain = 1.0
  v = voices[0]
  v.taps[1] = 2.0
  voices[1].gain = 3.0
  v = voices[1]
  v.taps[1] = 4.0
}
sample {
  out1 = read_mix(voices, idx)
  idx = idx + 1
}
"#;

const PROC_ARRAY_INDEXED_FIELD_ASSIGN_EXAMPLE: &str = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1

def set_gains(voices, gain):
  for i in 0..2:
    voices[i].gain = gain + f32(i)

init:
  voices: Voice[2] = Voice()
  set_gains(voices, 1.0)

sample:
  out1 = voices[0]() + voices[1]()
"#;

const PROC_ARRAY_PARAM_LEN_EXAMPLE: &str = r#"
proc Voice:
  params:
    gain = 0.0
  outs:
    out1
  sample:
    out1 = gain

outs:
  out1

def set_and_sum(voices):
  total = 0.0
  for i in 0..(voices.len()):
    voices[i].gain = f32(i + 1)
    total = total + voices[i]()
  return total + f32(voices.len())

init:
  voices: Voice[3] = Voice()

sample:
  out1 = set_and_sum(voices)
"#;

const STRUCT_ARRAY_PARAM_LEN_EXAMPLE: &str = r#"
struct Pair:
  x

outs:
  out1

def set_and_sum(pairs):
  total = 0.0
  for i in 0..(pairs.len()):
    pairs[i].x = f32(i + 1)
    total = total + pairs[i].x
  return total + f32(pairs.len())

init:
  pairs: Pair[3]

sample:
  out1 = set_and_sum(pairs)
"#;

const NESTED_STRUCT_FIELD_WRITE_EXAMPLE: &str = r#"
outs:
  out1

struct Inner:
  value: f32 = 0.0

struct Outer:
  inner: Inner

init:
  data = Outer()
  data.inner.value = 3.0

sample:
  out1 = data.inner.value
"#;

const PROC_ARRAY_INIT_EVENT_EXAMPLE: &str = r#"
proc Voice:
  params:
    gain = 1.0
    bias = 0.0

  sample:
    out1 = in1 * gain + bias

init:
  gains: f32[2] = [0.5, 1.5]
  biases: f32[2] = [0.1, 0.2]
  voices: Voice[2] = Voice()

  for i in 0..2:
    voices[i].init(gain = gains[i], bias = biases[i])

sample:
  out1 = voices[0](1.0) + voices[1](1.0)
"#;

const DEF_STRUCTURAL_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct A { x: f32, y: f32 }
struct B { x: f32 }
def read_x(s) {
  return s.x
}
init {
  a = A(0.25, 0.9)
  b = B(0.75)
}
sample {
  out1 = read_x(a) + read_x(b)
}
"#;

const DEF_ARRAY_ARG_BY_REF_WRITE_EXAMPLE: &str = r#"
outs { out1 }
def bump(xs, idx) {
  xs[idx] = xs[idx] + 1.0
  return xs[idx]
}
init {
  arr: f32[4]
  arr[0] = 0.0
  arr[1] = 1.0
  arr[2] = 2.0
  arr[3] = 3.0
}
sample {
  out1 = bump(arr, 1) + arr[1]
}
"#;

const DEF_ARRAY_ARG_FORWARDING_EXAMPLE: &str = r#"
outs { out1 }
def bump(xs, idx) {
  xs[idx] = xs[idx] + 1.0
  return xs[idx]
}
def forward(xs, idx) {
  return bump(xs, idx)
}
init {
  arr: f32[4]
  arr[0] = 0.0
  arr[1] = 1.0
  arr[2] = 2.0
  arr[3] = 3.0
}
sample {
  out1 = forward(arr, 2) + arr[2]
}
"#;

const DEF_LOCAL_ARRAY_ARG_EXAMPLE: &str = r#"
outs { out1 }
def bump(xs) {
  xs[0] = xs[0] + 1.0
  return xs[0]
}
sample {
  tmp = [1.0, 2.0]
  out1 = bump(tmp)
}
"#;

const DEF_EXPLICIT_STRUCT_ARG_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct A { x: f32, y: f32 }
struct B { x: f32 }
def read_x(s: A) {
  return s.x
}
init {
  b = B(0.75)
}
sample {
  out1 = read_x(b)
}
"#;

const DEF_OVERLOAD_ARITY_EXAMPLE: &str = r#"
outs { out1 }
def mix(x) {
  return x + 1.0
}
def mix(x, y) {
  return x + y
}
sample {
  out1 = mix(1.0) + mix(2.0, 3.0)
}
"#;

const DEF_OVERLOAD_TYPED_BEATS_UNTYPED_EXAMPLE: &str = r#"
outs { out1 }
def sel(x) {
  return 10.0
}
def sel(x: f64) {
  return 20.0
}
sample {
  out1 = sel(f64(1.0))
}
"#;

const DEF_OVERLOAD_WIDENING_FALLBACK_EXAMPLE: &str = r#"
outs { out1 }
def h(x: i64) {
  return f32(x) + 1.0
}
def h(flag: bool) {
  if (flag) { return 100.0 } else { return 200.0 }
}
sample {
  out1 = h(i32(3))
}
"#;

const DEF_OVERLOAD_I32_AMBIGUOUS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
def g(x: i64) {
  return f32(x)
}
def g(x: f64) {
  return f32(x)
}
sample {
  out1 = g(i32(7))
}
"#;

const DEF_OVERLOAD_DEFAULTS_EXAMPLE: &str = r#"
outs { out1 }
def d(x) {
  return x
}
def d(x, y = 1.0) {
  return x + y * 10.0
}
sample {
  out1 = d(2.0)
}
"#;

const DEF_OVERLOAD_DEFAULTS_AMBIGUOUS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
def a(x: f64, y = 0.0) {
  return 1.0
}
def a(x: f64, z = 0.0) {
  return 2.0
}
sample {
  out1 = a(f64(1.0))
}
"#;

const DEF_OVERLOAD_STRUCT_AND_SCALAR_EXAMPLE: &str = r#"
outs { out1 }
struct A { x: f32 }
def foo(v: A) {
  return v.x
}
def foo(x: f32) {
  return x + 10.0
}
init {
  a = A(2.0)
}
sample {
  out1 = foo(a) + foo(1.0)
}
"#;

const DEF_OVERLOAD_BUFFER_AND_SCALAR_EXAMPLE: &str = r#"
buffers { b: buffer[f32] }
outs { out1 }
def kind(x: f32) {
  return x
}
def kind(buf: buffer[f32]) {
  return buf[0]
}
sample {
  out1 = kind(1.0) + kind(b)
}
"#;

const STRUCT_METHOD_OVERLOAD_EXAMPLE: &str = r#"
outs { out1 }
struct V {
  def run(self, x) {
    return x
  }
  def run(self, x, y) {
    return x + y
  }
}
init {
  v = V()
}
sample {
  out1 = v.run(1.0) + v.run(1.0, 2.0)
}
"#;

const BUFFER_MONO_CLAMP_READ_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
init {
  idx: i32 = 0
}
sample {
  out1 = buf1[idx]
  idx = idx + 1
}
"#;

const BUFFER_MONO_I32_READ_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[i32]
}
outs {
  out1: i32
}
init {
  idx: i32 = 0
}
sample {
  out1 = buf1[idx]
  idx = idx + 1
}
"#;

const BUFFER_MONO_I64_READ_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[i64]
}
outs {
  out1: i64
}
init {
  idx: i32 = 0
}
sample {
  out1 = buf1[idx]
  idx = idx + 1
}
"#;

const BUFFER_MONO_BOOL_READ_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[bool]
}
outs {
  out1: bool
}
init {
  idx: i32 = 0
}
sample {
  out1 = buf1[idx]
  idx = idx + 1
}
"#;

const BUFFER_MONO_UNSAFE_RW_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
init {
  idx: i32 = 1
}
sample {
  unsafe_write(buf1, idx, 7.0)
  out1 = unsafe_read(buf1, idx)
}
"#;

const BUFFER_STEREO_2D_READ_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
init {
  idx: i32 = 0
}
sample {
  out1 = buf1[1][idx]
  idx = idx + 1
}
"#;

const BUFFER_STEREO_2D_WRITE_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
sample {
  buf1[1][0] = 7.0
  out1 = buf1[1][0]
}
"#;

const BUFFER_STEREO_1D_INDEX_ERROR_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
sample {
  out1 = buf1[0]
}
"#;

const BUFFER_STATIC_CHANS_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
sample {
  out1 = f32(buf1.chans())
}
"#;

const BUFFER_DYNAMIC_CHANS_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[]]
}
outs {
  out1
}
sample {
  out1 = f32(buf1.chans())
}
"#;

const BUFFER_DYNAMIC_LEN_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[]]
}
outs {
  out1
}
sample {
  out1 = f32(buf1.len())
}
"#;

const DEF_BUFFER_MONO_PARAM_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
def read_at(b: buffer[f32], i: i32) {
  return b[i]
}
init {
  idx: i32 = 0
}
sample {
  out1 = read_at(buf1, idx)
  idx = idx + 1
}
"#;

const DEF_BUFFER_STEREO_PARAM_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
def read_r(b: buffer[f32[2]], i: i32) {
  return b[1][i]
}
init {
  idx: i32 = 0
}
sample {
  out1 = read_r(buf1, idx)
  idx = idx + 1
}
"#;

const DEF_BUFFER_DYNAMIC_LEN_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[]]
}
outs {
  out1
}
def frames_of(b: buffer[f32[]]) {
  return f32(b.len())
}
sample {
  out1 = frames_of(buf1)
}
"#;

const DEF_BUFFER_PARAM_TYPE_MISMATCH_ERROR_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f64]
}
outs {
  out1
}
def read_at(b: buffer[f32], i: i32) {
  return b[i]
}
sample {
  out1 = read_at(buf1, 0)
}
"#;

const DEF_BUFFER_DUCK_PARAM_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
def read_at(b, i: i32) {
  return b[i]
}
init {
  idx: i32 = 0
}
sample {
  out1 = read_at(buf1, idx)
  idx: i32 = idx + 1
}
"#;

const DEF_BUFFER_DUCK_PROPAGATION_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
def inner(b, i: i32) {
  return b[i]
}
def outer(b: buffer[f32], i: i32) {
  return inner(b, i)
}
init {
  idx: i32 = 0
}
sample {
  out1 = outer(buf1, idx)
  idx: i32 = idx + 1
}
"#;

const DEF_BUFFER_DUCK_MIXED_ELEM_EXAMPLE: &str = r#"
buffers {
  a: buffer[f32]
  b: buffer[f64]
}
outs {
  out1
}
def read_at(buf, i: i32) {
  return buf[i]
}
sample {
  out1 = read_at(a, 0) + read_at(b, 0)
}
"#;

const DEF_INDEXABLE_ARG_ARRAY_AND_BUFFER_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32]
}
outs {
  out1
}
def read_at(xs, i: i32) {
  return xs[i]
}
init {
  arr: f32[4]
  arr[0] = 1.0
  arr[1] = 2.0
  arr[2] = 3.0
  arr[3] = 4.0
}
sample {
  out1 = read_at(arr, 1) + read_at(buf1, 1)
}
"#;

const DEF_INDEXABLE_ARG_STEREO_BUFFER_EXAMPLE: &str = r#"
buffers {
  b: buffer[f32[2]]
}
outs {
  out1
}
def read_ch(xs, ch: i32, i: i32) {
  return xs[ch][i]
}
sample {
  out1 = read_ch(b, 1, 2)
}
"#;

const STRUCT_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair(0.25, 0.75)
}
sample {
  p.a = 0.5
  out1 = p.a + p.b
}
"#;

const STRUCT_NAMED_DEFAULT_CTOR_EXAMPLE: &str = r#"
outs { out1 }
struct Voice {
  phase: f32 = 0.5
  gain: f32
}
init {
  v = Voice(gain = 1.0)
}
sample {
  out1 = v.phase + v.gain
}
"#;

const STRUCT_INIT_IN_SAMPLE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair(0.25, 0.75)
}
sample {
  p = Pair(1.0, 2.0)
  out1 = p.a
}
"#;

const DATA_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  buf[0.0] = 1.0
  buf[1.9] = 2.0
  buf[-10.1] = 3.0
  buf[100.1] = 4.0
  out1 = buf[-1.2] + buf[0.9] + buf[1.9] + buf[99.1]
}
"#;

const DATA_LEN_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  out1 = f32(buf.len())
}
"#;

const DATA_LEN_STRUCT_FIELD_EXAMPLE: &str = r#"
outs { out1 }
struct Delay { buf: f32[8] }
init {
  d = Delay()
}
sample {
  out1 = f32(d.buf.len())
}
"#;

const DATA_LEN_INVALID_RECEIVER_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x = 0.0
}
sample {
  out1 = f32(x.len())
}
"#;

const DATA_STRUCT_ELEM_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { x: f32, y: f32 }
init {
  buf: Pair[2]
}
sample {
  p = buf[1.0]
  p.x = p.x + 1.0
  p.y = p.y + 0.5
  out1 = p.x + p.y
}
"#;

const STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { x: f32, y: f32 }
struct Bank { buf: Pair[2] }
init {
  b = Bank()
}
sample {
  p = b.buf[1.0]
  p.x = p.x + 1.0
  p.y = p.y + 2.0
  out1 = p.x + p.y
}
"#;

const INIT_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Tap { delay_samples: f32, gain: f32 }
struct Delay { taps: Tap[2] }
init {
  d = Delay()
  t = d.taps[0.0]
  t.delay_samples = 2.0
  t.gain = 3.0
}
sample {
  t = d.taps[0.0]
  out1 = t.delay_samples + t.gain
}
"#;

const DEF_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Tap { delay_samples: f32, gain: f32 }
struct Delay {
  taps: Tap[2]
  def init_taps(self) {
    t = self.taps[0.0]
    t.delay_samples = 2.0
    t.gain = 3.0
  }
}
init {
  d = Delay()
  d.init_taps()
}
sample {
  t = d.taps[0.0]
  out1 = t.delay_samples + t.gain
}
"#;

const DEF_STRUCT_FIELD_NESTED_DATA_STRUCT_ELEM_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Tap { x: f32 }
struct Inner { taps: Tap[2] }
struct Proc {
  inners: Inner[2]
  def init_nested(self) {
    i = self.inners[1.0]
    t = i.taps[0.0]
    t.x = 7.0
  }
}
init {
  p = Proc()
  p.init_nested()
}
sample {
  i = p.inners[1.0]
  t = i.taps[0.0]
  out1 = t.x
}
"#;

const DATA_STRUCT_INLINE_FIELD_READ_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { x: f32, y: f32 }
init {
  idx: i32 = 0
  buf: Pair[2]
  p = buf[0]
  p.x = 1.0
  p = buf[1]
  p.x = 3.0
}
sample {
  out1 = buf[idx].x
  idx = idx + 1
}
"#;

const DATA_STRUCT_INLINE_ARRAY_FIELD_READ_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { taps: f32[2] }
init {
  idx: i32 = 0
  buf: Pair[2]
  p = buf[0]
  p.taps[0] = 1.0
  p.taps[1] = 2.0
  p = buf[1]
  p.taps[0] = 3.0
  p.taps[1] = 4.0
}
sample {
  out1 = buf[idx].taps[1]
  idx = idx + 1
}
"#;

const INIT_STRUCT_INLINE_FIELD_READ_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { x: f32, taps: f32[2] }
init {
  buf: Pair[2]
  p = buf[0]
  p.x = 1.0
  p.taps[1] = 2.0
  p = buf[1]
  p.x = 3.0
  p.taps[1] = 4.0
  total = buf[0].x + buf[1].taps[1]
}
sample {
  out1 = total
}
"#;

const DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { gain: f32, delay: f32[4] }
init {
  voices: Voice[2]
}
sample {
  v = voices[1.8]
  v.gain = v.gain + 0.25
  v.delay[-2.7] = 1.0
  v.delay[99.3] = v.gain
  out1 = v.delay[0.0] + v.delay[3.0]
}
"#;

const STRUCT_FIELD_DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[3] }
struct Bank { voices: Voice[2] }
init {
  b = Bank()
}
sample {
  v = b.voices[0.0]
  v.delay[1.2] = v.delay[1.2] + 0.5
  out1 = v.delay[1.0]
}
"#;

const DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Tap { x: f32 }
struct Grain { taps: Tap[2] }
struct Voice { grains: Grain[2] }
init {
  voices: Voice[2]
}
sample {
  v = voices[1.8]
  g = v.grains[-3.2]
  t = g.taps[99.0]
  t.x = t.x + 0.25
  out1 = t.x
}
"#;

const STRUCT_FIELD_DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
struct Tap { x: f32 }
struct Voice { taps: Tap[2] }
struct Bank { voices: Voice[2] }
init {
  b = Bank()
}
sample {
  v = b.voices[0.0]
  t = v.taps[1.2]
  t.x = t.x + 1.0
  out1 = t.x
}
"#;

const DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[2]
  buf[0.0] = 1.0
}
sample {
  x = buf[0.0]
  x = x + 2.5
  out1 = buf[0.0]
}
"#;

const STRUCT_DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[2] }
init {
  v = Voice()
  v.delay[0.0] = 1.0
}
sample {
  tap = v.delay[0.0]
  tap = tap + 4.0
  out1 = v.delay[0.0]
}
"#;

const INIT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[2]
  buf[0.0] = 3.5
  x = buf[0.0]
}
sample {
  out1 = x
}
"#;

const DEF_STRUCT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE: &str = r#"
outs { out1 }
struct Voice {
  delay: f32[2]

  def read(self, i: i32) {
    tap = self.delay[i]
    return tap
  }
}
init {
  v = Voice()
  v.delay[1.0] = 4.0
}
sample {
  out1 = v.read(1)
}
"#;
const DATA_CONST_CAPACITY_EXAMPLE: &str = r#"
outs { out1 }
struct Delay { buf: f32[SR * 2] }
init {
  d = Delay()
  d.buf[SR * 2 - 1] = 0.75
}
sample {
  out1 = d.buf[SR * 2 - 1]
}
"#;

const DATA_CTOR_CONST_CAPACITY_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[BLOCK_SIZE * 2]
  buf[BLOCK_SIZE * 2 - 1] = 1.25
}
sample {
  out1 = buf[BLOCK_SIZE * 2 - 1]
}
"#;

const DATA_INIT_IN_SAMPLE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  buf: f32[8]
  out1 = 0.0
}
"#;

const TYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
sample {
  buf: f32[4]
  buf[0] = 1.0
  buf[1] = 2.0
  out1 = buf[0] + buf[1]
}
"#;

const UNTYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
sample {
  b = [1, 2, 3]
  out1 = b[0] + b[2]
}
"#;

const TYPED_LOCAL_ARRAY_DEF_EXAMPLE: &str = r#"
outs { out1 }
def mk(x) {
  tmp: f32[2]
  tmp[0] = x
  tmp[1] = x * 2.0
  return tmp[0] + tmp[1]
}
sample {
  out1 = mk(0.5)
}
"#;

const UNTYPED_LOCAL_ARRAY_DEF_EXAMPLE: &str = r#"
outs { out1 }
def pick() {
  b = [1, 2, 3]
  return b[1]
}
sample {
  out1 = pick()
}
"#;

const TYPED_LOCAL_ARRAY_I32_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
sample {
  buf: i32[4]
  buf[0] = 1
  buf[1] = 2
  out1 = buf[0] + buf[1]
}
"#;

const TYPED_LOCAL_ARRAY_BOOL_DEF_EXAMPLE: &str = r#"
outs { out1 }
def pick(flag) {
  bits: bool[2]
  bits[0] = flag
  bits[1] = !flag
  if (bits[0]) { return 1.0 } else { return 0.0 }
}
sample {
  out1 = pick(true) + pick(false)
}
"#;

const TYPED_LOCAL_ARRAY_INIT_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
sample {
  buf: f32[2] = [1.25, 2.75]
  out1 = buf[0] + buf[1]
}
"#;

const TOP_LEVEL_PARAM_ARRAY_EXAMPLE: &str = r#"
outs { out1 }
params { mix: f32[2] = [0.25, 0.75] }
sample {
  out1 = mix[0] + mix[1]
}
"#;

const TOP_LEVEL_INPUT_ARRAY_EXAMPLE: &str = r#"
ins { in1: f32[2] }
outs { out1 }
sample {
  out1 = in1[0] * 2.0 + in1[1]
}
"#;

const TOP_LEVEL_IO_F64_EXAMPLE: &str = r#"
ins { in1: f64[2] }
outs { out1: f64 }
sample {
  out1 = in1[0] + in1[1] * f64(0.5)
}
"#;

const TOP_LEVEL_OUTPUT_ARRAY_EXAMPLE: &str = r#"
outs { out1: f32[2] }
sample {
  out1[0] = 0.25
  out1[1] = out1[0] + 0.5
}
"#;

const GRAPH_IMPLICIT_PROC_FANOUT_EXAMPLE: &str = r#"
proc Source {
  outs { out1 }
  sample {
    out1 = 0.25
  }
}

proc Gain {
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1 * 2.0
  }
}

outs 2

init {
  src = Source()
  gain = Gain()
}

graph {
  src.out1 >> gain.in1
  gain.out1 >> out1
  gain.out1 >> out2
}
"#;

const GRAPH_DELAY_FEEDBACK_EXAMPLE: &str = r#"
proc Acc {
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1 + 1.0
  }
}

outs { out1 }

init {
  acc = Acc()
}

graph {
  acc.out1 >>[1] acc.in1
  acc.out1 >> out1
}
"#;

const GRAPH_PARAM_SAMPLE_OVERRIDE_EXAMPLE: &str = r#"
proc Gain {
  params { gain = 0.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}

ins { in1 }
outs { out1 }

init {
  g = Gain()
}

graph {
  @sample in1 >> g.gain
  g.out1 >> out1
}
"#;

const GRAPH_FANOUT_EXAMPLE: &str = r#"
proc Sum {
  ins { a, b }
  outs { out1 }
  sample {
    out1 = a + b
  }
}

ins { in1 }
outs { out1 }

init {
  s = Sum()
}

graph {
  in1 * 0.25 >> { s.a, s.b }
  s.out1 >> out1
}
"#;

const GRAPH_PROC_BUNDLE_FANOUT_EXAMPLE: &str = r#"
proc Pair {
  outs 2
  sample {
    out1 = 0.25
    out2 = 0.75
  }
}

proc Mono {
  outs { out1 }
  sample {
    out1 = 0.5
  }
}

outs 4

init {
  pair = Pair()
  monos: Mono[1] = Mono()
}

graph {
  pair >> { out1, out2 }
  monos[0] >> { out3, out4 }
}
"#;

const GRAPH_ARRAY_EXPR_EXAMPLE: &str = r#"
ins { a: f32[2], b: f32[2] }
outs { out_st: f32[2] }

graph {
  a * 0.5 + b * 0.25 >> out_st
}
"#;

const GRAPH_ARRAY_DELAY_EXAMPLE: &str = r#"
ins { in_st: f32[2] }
outs { out_st: f32[2] }

graph {
  in_st >>[1] out_st
}
"#;

const GRAPH_ARRAY_BROADCAST_EXAMPLE: &str = r#"
outs { out_st: f32[2] }

graph {
  0.25 >> out_st
}
"#;

const GRAPH_EVENT_ROUTING_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    gain = 0.0
  }
  events {
    set_gain(v: f32) {
      gain = v
    }
  }
  sample {
    out1 = gain
  }
}

outs { out1 }

init {
  voice = Voice()
}

graph {
  voice.out1 >> out1
}

events {
  set_gain(v: f32) {
    voice.set_gain(v)
  }
}
"#;

const GRAPH_PROC_ARRAY_PARAM_DEST_AND_OUTPUT_SOURCE_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 0.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}

outs { out1, out2, out3 }

init {
  voices: Voice[2] = Voice()
}

graph {
  0.25 >> voices[0].gain
  0.75 >> voices[1].gain
  voices[0].out1 >> out1
  voices[1].out1 >> out2
  voices[1].out1 >> out3
}
"#;

const GRAPH_RECEIVER_DELAY_EXAMPLE: &str = r#"
outs { out1 }

graph {
  @sample out1 <<[1] 1.0
}
"#;

const GRAPH_SLICE_SOURCE_EXAMPLE: &str = r#"
ins { in_bus: f32[4] }
outs { out_st: f32[2] }

graph {
  in_bus[1:3] >> out_st
}
"#;

const GRAPH_PROC_LOCAL_GRAPH_EXAMPLE: &str = r#"
proc Swap {
  ins 2
  outs 2
  graph {
    in2 >> out1
    in1 >> out2
  }
}

ins 2
outs 2

init {
  swap = Swap()
}

graph {
  in1 >> swap.in1
  in2 >> swap.in2
  swap.out1 >> out1
  swap.out2 >> out2
}
"#;

const GRAPH_PROC_INPUT_ARRAY_BROADCAST_EXAMPLE: &str = r#"
proc Sum2 {
  ins { in_st: f32[2] }
  outs { out1 }
  sample {
    out1 = in_st[0] + in_st[1]
  }
}

outs { out1 }

init {
  sum = Sum2()
}

graph {
  0.5 >> sum.in_st
  sum.out1 >> out1
}
"#;

const GRAPH_PROC_PARAM_ARRAY_BROADCAST_EXAMPLE: &str = r#"
proc Sum2 {
  params { gains: f32[2] = [0.0, 0.0] }
  outs { out1 }
  sample {
    out1 = gains[0] + gains[1]
  }
}

outs { out1 }

init {
  sum = Sum2()
}

graph {
  0.5 >> sum.gains
  sum.out1 >> out1
}
"#;

const GRAPH_PROC_NAMED_PORT_ALIAS_EXAMPLE: &str = r#"
proc Mix {
  ins { dry, fb }
  outs { wet }
  sample {
    wet = dry + fb
  }
}

outs { out1 }

init {
  mix = Mix()
}

graph {
  0.25 >> mix.in1
  0.5 >> mix.in2
  mix.out1 >> out1
}
"#;

const GRAPH_TOP_LEVEL_NAMED_IO_ALIAS_EXAMPLE: &str = r#"
ins { dry }
outs { wet }

graph {
  in1 >> out1
}
"#;

const GRAPH_TOP_LEVEL_IO_INFERENCE_EXAMPLE: &str = r#"
graph {
  in1 * 0.5 >> out1
}
"#;

const GRAPH_PROC_IO_INFERENCE_EXAMPLE: &str = r#"
proc Mix {
  graph {
    in1 + in2 >> out1
  }
}

graph {
  0.25 >> mix.in1
  0.5 >> mix.in2
  mix.out1 >> out1
}

init {
  mix = Mix()
}
"#;

const GRAPH_PROC_CUSTOM_IO_NAMES_REQUIRE_DECLS_ERROR_EXAMPLE: &str = r#"
proc Mix {
  graph {
    dry + fb >> wet
  }
}

graph {
  0.25 >> mix.in1
  0.5 >> mix.in2
  mix.out1 >> out1
}

init {
  mix = Mix()
}
"#;

const UNSAFE_TOP_LEVEL_ARRAY_EXAMPLE: &str = r#"
outs { out1: f32[2] }
params { p: f32[2] = [1.0, 2.0] }
sample {
  unsafe_write(out1, 0.0, unsafe_read(p, 1.0))
  unsafe_write(out1, 1.0, unsafe_read(out1, 0.0) + unsafe_read(p, 0.0))
}
"#;

const UNSAFE_DATA_BUILTINS_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  unsafe_write(buf, 0.0, 1.5)
  unsafe_write(buf, 1.9, unsafe_read(buf, 0.0) + 2.0)
  out1 = unsafe_read(buf, 1.9)
}
"#;

const UNSAFE_DATA_BUILTINS_STRUCT_FIELD_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4] }
init {
  v = Voice()
}
sample {
  unsafe_write(v.delay, 2.2, 4.0)
  out1 = unsafe_read(v.delay, 2.2)
}
"#;

const UNSAFE_DATA_BUILTINS_TYPED_LOCAL_ARRAY_DEF_EXAMPLE: &str = r#"
outs { out1 }
def run() {
  xs: i32[4]
  unsafe_write(xs, 1.8, 7)
  return unsafe_read(xs, 1.8)
}
sample {
  out1 = run()
}
"#;

const MULTITAP_FEEDBACK_STRUCT_DATA_EXAMPLE: &str =
    include_str!("../../../examples/multitap_feedback_struct_data.onda");
const PROC_GAIN_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/proc_gain_graph.onda");
const PROC_SPLIT_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/proc_split_graph.onda");
const PROC_ARRAY_STEREO_SINE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/proc_array_stereo_sine_graph.onda");
const FEEDBACK_SATURATOR_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/feedback_saturator_graph.onda");
const STD_ONE_POLE_FILE_EXAMPLE: &str = include_str!("../../../examples/std_one_pole.onda");
const STD_ONE_POLE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/std_one_pole_graph.onda");
const STDLIB_F32_FILE_EXAMPLE: &str = include_str!("../../../examples/stdlib_f32.onda");
const STDLIB_F32_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/stdlib_f32_graph.onda");
const SINE_WASM_FILE_EXAMPLE: &str =
    include_str!("../../../examples/web/sine_wasm_worklet/sine_wasm.onda");

const STRUCT_DATA_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4], gain: f32 }
init {
  v = Voice(0.5)
}
sample {
  v.delay[-1.2] = 1.0
  v.delay[1.9] = 2.0
  v.delay[3.7] = 4.0
  out1 = v.delay[99.1] + v.delay[1.2] + v.delay[-8.1] * v.gain
}
"#;

const STRUCT_DATA_IS_PER_INSTANCE_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[2], gain: f32 }
init {
  a = Voice(1.0)
  b = Voice(1.0)
}
sample {
  a.delay[0.0] = 1.0
  b.delay[0.0] = 3.0
  out1 = a.delay[0.0] + b.delay[0.0]
}
"#;

const STRUCT_DATA_FIELD_NON_INDEXED_WRITE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4], gain: f32 }
init {
  v = Voice(1.0)
}
sample {
  v.delay = 1.0
  out1 = 0.0
}
"#;

const IMPLICIT_IO_GAPPED_EXAMPLE: &str = r#"
sample {
  out2 = in3 * 0.5
}
"#;

const SPARSE_DECLARED_IO_EXAMPLE: &str = r#"
ins { in3 }
outs { out3 }
sample {
  out3 = in3
}
"#;

const BUILTIN_CONSTS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(PI) + f32(TWO_PI) + SAMPLE_RATE - SR
}
"#;

const BUILTIN_CONSTS_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(TWO_PI) / SR
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_SAMPLERATE_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(TWO_PI) / SAMPLERATE
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_LOWERCASE_ALIASES_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(pi) + f32(two_pi) + f32(twopi) + samplerate - sample_rate + f32(blocksize) - f32(block_size)
}
"#;

const BUILTIN_CONSTS_LOWERCASE_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(twopi) / samplerate
  out1 = sin(phase)
}
"#;

const EXPORT_MATH_TYPED_OVERLOADS_EXAMPLE: &str = r#"
import std/export_math
outs {
  out1
  out2
}
sample {
  out1 = std::export_math::cos(0.0) + std::export_math::exp(0.0) + std::export_math::log(1.0)
  out2 = f32(std::export_math::cos(f64(0.0)) + std::export_math::exp(f64(0.0)) + std::export_math::log(f64(1.0)))
}
"#;

const BUILTIN_INTRINSICS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = abs(-0.5) + cos(0.0) + sqrt(4.0) + exp(0.0) + log(exp(1.0))
  out1 = out1 + pow(2.0, 3.0) + min(3.0, 4.0) + max(3.0, 4.0) + fma(2.0, 3.0, 4.0)
  out1 = out1 + floor(1.8) + ceil(1.2) + round(1.6) + trunc(1.6)
}
"#;

const STDLIB_MATH_AUTO_IMPORT_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = clamp(2.0, 0.0, 1.0) + std::math::lerp(0.0, 2.0, 0.25) + map(0.0, 1.0, -1.0, 1.0, 0.75)
}
"#;

const STDLIB_MATH_LOCAL_SYMBOL_WINS_EXAMPLE: &str = r#"
outs { out1 }
def clamp(x, lo, hi) {
  return 5.0
}
sample {
  out1 = clamp(2.0, 0.0, 1.0) + std::math::clamp(2.0, 0.0, 1.0)
}
"#;

const STDLIB_RANDOM_GENERIC_RNG_EXAMPLE: &str = r#"
outs { out1: f64, out2: f64, out3: f64 }
init {
  rng = std::random::Rng<f64>(state = 123)
}
sample {
  out1 = rng.next()
  out2 = rng.bipolar()
  out3 = rng.range(f64(-2.0), f64(2.0))
}
"#;

const STDLIB_BUFFER_READ_MONO_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer[f32] }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 2)
}
"#;

const STDLIB_BUFFER_INTERP_STEREO_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer[f32[2]] }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 0, 1) + std::lookup::readL(b, 1, 0.5) + std::lookup::readC(b, 1, 1.0)
}
"#;

const STDLIB_BUFFER_AUTO_IMPORT_ARRAY_AND_BUFFER_EXAMPLE: &str = r#"
buffers { b: buffer[f32] }
outs { out1 }
init {
  a: f32[4]
  a[0] = 1.0
  a[1] = 2.0
  a[2] = 3.0
  a[3] = 4.0
}
sample {
  out1 = a.read(1) + readL(a, 1.5) + b.readC(2.0)
}
"#;

const STDLIB_LOOKUP_WRITE_ARRAY_AND_BUFFER_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer[f32] }
outs { out1 }
init {
  a: f32[4]
}
sample {
  std::lookup::write(a, 1, 2.5)
  std::lookup::write(b, 2, 4.0)
  out1 = std::lookup::read(a, 1) + std::lookup::read(b, 2)
}
"#;

const FLOOR_FRACT_WRAP_NUMERIC_BEHAVIOR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(9007199254740993)
}
sample {
  out1 = floor(1.8) + fract(2.25) + wrap(5.5, 0.0, 2.0) + f32(x - i64(9007199254740993))
}
"#;

const BUILTIN_INT_INTRINSICS_EXAMPLE: &str = r#"
outs { out1 }
init {
  a: i32 = i32(-3)
  b: i32 = 7
  c: i64 = 9
}
sample {
  out1 = f32(abs(a)) + f32(min(a, b)) + f32(max(i64(2), c)) + pow(2, 3)
}
"#;

const BUILTIN_FLOAT_ONLY_TYPE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i32 = 1
}
sample {
  out1 = sin(x)
}
"#;

const BITWISE_OPS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  a: i32 = 6
  b: i32 = 3
  x: i32 = (a & b) + (a | b) + (a ^ b) + (1 << 3) + (8 >> 1) + ~1
  out1 = f32(x)
}
"#;

const BITWISE_FLOAT_OPERAND_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(1.0 & 1)
}
"#;

const ASSERT_PASSES_EXAMPLE: &str = r#"
namespace Config {
  assert(BLOCK_SIZE > 0)
}
outs { out1 }
sample {
  out1 = 1.0
}
"#;

const ASSERT_NAMESPACE_POWER_OF_TWO_ERROR_EXAMPLE: &str = r#"
namespace FFT<N = 4> {
  assert((N & (N - 1)) == 0)
  struct Tag {
    value
  }
}
outs { out1 }
init {
  tag: FFT<6>::Tag
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_FFT_ZERO_SIZE_ERROR_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  fft: std::fft<0>::FFT<f32>
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_COMPLEX_STRUCT_EXAMPLE: &str = r#"
import std/complex
outs 4
init {
  z: std::complex::Complex<f32>
  w: std::complex::Complex<f32>
  z.set(1.0, 2.0)
  w.set(3.0, -4.0)
  z.mul_assign(w)
}
sample {
  out1 = z.real()
  out2 = z.imag()
  out3 = z.magnitude()
  out4 = z.phase()
}
"#;

const STDLIB_COMPLEX_POLAR_F64_EXAMPLE: &str = r#"
import std/complex
outs 3
init {
  z: std::complex::Complex<f64>
  z.set_polar(f64(2.0), f64(0.5))
  z.conjugate()
  z.scale_assign(f64(0.5))
}
sample {
  out1 = f32(z.real())
  out2 = f32(z.imag())
  out3 = f32(z.power())
}
"#;

const STDLIB_FFT_IMPULSE_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f32[8]
  input[0] = 1.0
  input[1] = 0.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real(input)
  out1 = fft.real(0) + fft.real(1) + fft.real(2) + fft.real(3) + fft.real(4) + fft.real(5) + fft.real(6) + fft.real(7)
}
"#;

const STDLIB_FFT_IMPULSE_F64_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f64[8]
  input[0] = f64(1.0)
  input[1] = f64(0.0)
  input[2] = f64(0.0)
  input[3] = f64(0.0)
  input[4] = f64(0.0)
  input[5] = f64(0.0)
  input[6] = f64(0.0)
  input[7] = f64(0.0)
  fft: std::fft<8>::FFT<f64>
}
sample {
  fft.forward_real(input)
  out1 = f32(fft.real(0) + fft.real(1) + fft.real(2) + fft.real(3) + fft.real(4) + fft.real(5) + fft.real(6) + fft.real(7))
}
"#;

const STDLIB_FFT_REAL_PACKED_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f32[8]
  packed: f32[8]
  input[0] = 1.0
  input[1] = 0.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real_packed(input, packed)
  out1 = packed[0] + packed[1] + packed[2] + packed[3] + packed[4] + packed[5] + packed[6] + packed[7]
}
"#;

const STDLIB_FFT_REAL_PACKED_ROUNDTRIP_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[4]
  packed: f32[4]
  output: f32[4]
  input[0] = 1.0
  input[1] = 2.0
  input[2] = 3.0
  input[3] = 4.0
  fft: std::fft<4>::FFT<f32>
}
sample {
  fft.forward_real_packed(input, packed)
  fft.inverse_real_packed(packed, output)
  out1 = output[0]
  out2 = output[1]
  out3 = output[2]
  out4 = output[3]
}
"#;

const STDLIB_FFT_REAL_SPECTRUM_HELPERS_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[8]
  mags: f32[5]
  power: f32[5]
  phase: f32[5]
  input[0] = 0.0
  input[1] = 1.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real_magnitude(input, mags)
  fft.forward_real_power(input, power)
  fft.forward_real_phase(input, phase)
  out1 = mags[0] + mags[1] + mags[2] + mags[3] + mags[4]
  out2 = power[0] + power[1] + power[2] + power[3] + power[4]
  out3 = phase[0] + phase[1] + phase[2] + phase[3] + phase[4]
  out4 = f32(fft.size() + fft.real_bin_count())
}
"#;

const STDLIB_STFT_HANN_WINDOW_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[8]
  mags: f32[5]
  window: f32[8]
  input[0] = 0.0
  input[1] = 1.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  stft: std::fft<8>::STFT<f32>
}
sample {
  stft.set_hann()
  stft.store_window(window)
  stft.forward_real_magnitude(input, mags)
  out1 = mags[0] + mags[1] + mags[2] + mags[3] + mags[4]
  out2 = window[1] + window[6]
  out3 = stft.magnitude(0)
  out4 = f32(stft.size() + stft.real_bin_count())
}
"#;

const STDLIB_REALFFT_STRUCT_EXAMPLE: &str = r#"
import std/fft
import std/osc
outs 1
init {
  saw = std::osc::Saw(freq = 440.0)
  fwd = std::fft<64>::RealFFT()
  inv = std::fft<64>::RealIFFT()
  scratch_re: f32[64]
  scratch_im: f32[64]
}
sample {
  saw.freq = 440.0
  if (fwd.push(saw())) {
    for i in 0..64 {
      scratch_re[i] = 0.0
      scratch_im[i] = 0.0
    }
    scratch_re[0] = fwd.fft.real(0)
    half = 64 >> 1
    for k in 1..half {
      shifted = k + 1
      if (shifted < half) {
        scratch_re[shifted] = fwd.fft.real(k)
        scratch_im[shifted] = fwd.fft.imag(k)
        scratch_re[64 - shifted] = fwd.fft.real(64 - k)
        scratch_im[64 - shifted] = fwd.fft.imag(64 - k)
      }
    }
    inv.load_complex(scratch_re, scratch_im)
  }
  out1 = inv.tick()
}
"#;

const STDLIB_REALFFT_NAMESPACED_PROC_EXAMPLE: &str = r#"
import std/fft
import std/osc

namespace BinShift<N = 64>:
  proc Main:
    outs 1
    params:
      freq = 440.0
    init:
      saw = std::osc::Saw(freq = freq)
      fwd = std::fft<N>::RealFFT()
      inv = std::fft<N>::RealIFFT()
      scratch_re: f32[N]
      scratch_im: f32[N]
    block:
      saw.freq = freq
      sample:
        if (fwd.push(saw())):
          for i in 0..N:
            scratch_re[i] = 0.0
            scratch_im[i] = 0.0
          scratch_re[0] = fwd.fft.real(0)
          half = N >> 1
          for k in 1..half:
            shifted = k + 1
            if (shifted < half):
              scratch_re[shifted] = fwd.fft.real(k)
              scratch_im[shifted] = fwd.fft.imag(k)
              scratch_re[N - shifted] = fwd.fft.real(N - k)
              scratch_im[N - shifted] = fwd.fft.imag(N - k)
          inv.load_complex(scratch_re, scratch_im)
        out1 = inv.tick()

outs 1
init:
  p = BinShift<64>::Main()
sample:
  out1 = p()
"#;

const STDLIB_REALFFT_HANN_OLA_PASSTHROUGH_EXAMPLE: &str = r#"
import std/fft
import std/osc
outs 3
init {
  osc = std::osc::Sine(freq = 220.0)
  fwd = std::fft<64>::RealFFT()
  inv = std::fft<64>::RealIFFT()
  scratch_re: f32[64]
  scratch_im: f32[64]
  delay: f32[64]
  delay_i: i32 = 0
  frames_seen: i32 = 0
}
sample {
  x = osc()
  expected_i = delay_i + 1
  if (expected_i >= 64) {
    expected_i = expected_i - 64
  }
  expected = delay[expected_i]
  delay[delay_i] = x
  delay_i = delay_i + 1
  if (delay_i >= 64) {
    delay_i = 0
  }

  if (fwd.push(x)) {
    fwd.fft.store_real(scratch_re)
    fwd.fft.store_imag(scratch_im)
    inv.load_complex(scratch_re, scratch_im)
  }

  y = inv.tick()
  frames_seen = frames_seen + 1
  if (frames_seen > 192) {
    out1 = y - expected
  } else {
    out1 = 0.0
  }
  out2 = f32(fwd.hop_size())
  out3 = f32(inv.hop_size())
}
"#;

const STDLIB_CONVOLUTION_TIME_DOMAIN_EVENT_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
events {
  set_ir(values: f32[4]) {
    conv.set_impulse(values)
  }
}
init {
  conv = std::convolution<8, 4>::TimeDomainConvolver<f32>()
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_BLOCK_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 4>::BlockConvolver<f32>()
  ir: f32[4] = [1.0, 0.5, 0.25, 0.0]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 8>::ZeroLatencyConvolver<f32>()
  ir: f32[5] = [1.0, 0.5, 0.25, 0.0, 0.125]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_CONST_NAMESPACE_EXAMPLE: &str = r#"
import std/convolution
const FFT_SIZE = 8
const MAX_IR = 8
outs { out1 }
init {
  conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
  ir: f32[5] = [1.0, 0.5, 0.25, 0.0, 0.125]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_ANALYZE_EXAMPLE: &str = r#"
import std/convolution
const FFT_SIZE = 1024
const MAX_IR = 100000
outs { out1 }
init {
  conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_WRAPPER_ANALYZE_EXAMPLE: &str = r#"
import std/convolution
const MAX_IR = 100000
const FFT_SIZE = 1024

namespace convolution_wav_impulse<N = MAX_IR>:
  proc Engine:
    init:
      conv = std::convolution<FFT_SIZE, N>::ZeroLatencyConvolver<f32>()
    sample:
      out1 = 0.0

init:
  engine = convolution_wav_impulse<MAX_IR>::Engine()

sample:
  out1 = 0.0
"#;

const STDLIB_CONVOLUTION_F64_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 4>::TimeDomainConvolver<f64>()
  ir: f64[4] = [1.0, 0.5, 0.25, 0.0]
  conv.set_impulse(ir)
}
sample {
  out1 = f32(conv(f64(in1)))
}
"#;

const NESTED_STRUCT_FIELD_AND_METHOD_EXAMPLE: &str = r#"
outs 2
struct Inner<T>:
  data: T[2]

  def set_pair(self, a: T, b: T):
    self.data[0] = a
    self.data[1] = b

  def sum(self):
    return self.data[0] + self.data[1]

struct Outer<T>:
  inner: Inner<T>

  def init_pair(self, a: T, b: T):
    self.inner.set_pair(a, b)

  def sum(self):
    return self.inner.sum()

init {
  outer: Outer<f32>
}
sample {
  outer.init_pair(1.5, 2.5)
  out1 = outer.inner.data[0]
  out2 = outer.sum()
}
"#;

const MULTILINE_STRUCT_METHOD_CALL_EXAMPLE: &str = r#"
outs { out1 }
struct Pair:
  a: f32
  b: f32

  def set(self, a, b):
    self.a = a
    self.b = b

init {
  p: Pair
}
sample {
  p.set(
    1.25,
    2.75,
  )
  out1 = max(
    p.a,
    p.b,
  )
}
"#;

const NESTED_GENERIC_STRUCT_ARRAY_FIELD_EXAMPLE: &str = r#"
outs 2
struct Stereo<T>:
  v: T[2]

struct Rack:
  items: Stereo<f32>[2]

init {
  rack: Rack
}
sample {
  s = rack.items[1]
  s.v[0] = 1.0
  s.v[1] = 2.0
  out1 = s.v[0]
  out2 = s.v[0] + s.v[1]
}
"#;

const BLOCK_SIZE_CONST_EXAMPLE: &str = r#"
outs { out1 }
init {
  v = BLOCK_SIZE
}
block {
  v = v + BLOCK_SIZE
  sample {
    out1 = v
  }
}
"#;

const BLOCK_SIZE_ALIASES_CONST_EXAMPLE: &str = r#"
outs { out1 }
init {
  v = blocksize + BLOCKSIZE - block_size
}
sample {
  out1 = v
}
"#;

const BLOCK_EXEC_ONCE_PER_PROCESS_EXAMPLE: &str = r#"
outs { out1 }
init {
  ctr = 0.0
}
block {
  ctr = ctr + 1.0
  sample {
    out1 = ctr
  }
}
"#;

const BLOCK_SCALAR_VISIBLE_IN_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
params { freq = 440.0 }
init { phase = 0.0 }
block {
  incr = freq * f32(TWO_PI) / SR
  sample {
    phase = phase + incr
    if (phase > f32(TWO_PI)) { phase = phase - f32(TWO_PI) }
    out1 = sin(phase)
  }
}
"#;

const BLOCK_IO_FORBIDDEN_ERROR_EXAMPLE: &str = r#"
outs { out1 }
block {
  out1 = 0.0
  sample {
    out1 = 0.0
  }
}
"#;

const BUILTIN_CONST_ASSIGN_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  PI = 0.0
  out1 = 0.0
}
"#;

const BUILTIN_CONST_ASSIGN_LOWERCASE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  pi = 0.0
  out1 = 0.0
}
"#;

const NAMESPACE_STRUCT_CTOR_EXAMPLE: &str = r#"
outs { out1 }
namespace FX:
  struct MyStruct:
    field: f32 = 0.75
init:
  a = FX::MyStruct()
sample:
  out1 = a.field
"#;

const NAMESPACE_DEF_RESOLUTION_EXAMPLE: &str = r#"
outs { out1 }
def g(x) {
  return x + 100.0
}
namespace NS {
  def p(x) {
    return x + 10.0
  }
  namespace Inner:
    def run(x):
      return p(x) + g(x)
}
sample {
  out1 = NS::Inner::run(1.0)
}
"#;

const NAMESPACE_TOP_LEVEL_UNQUALIFIED_CALL_ERROR_EXAMPLE: &str = r#"
outs { out1 }
namespace NS:
  def f(x):
    return x
sample:
  out1 = f(1.0)
"#;

const TYPED_NARROWING_ASSIGNMENT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(1.0)
}
sample {
  out1 = x
}
"#;

const IF_CONDITION_BOOL_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  if (1.0) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const IF_BRANCH_TYPE_CONFLICT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  if true {
    x = 1
  } else {
    x = 1.0
  }
}
sample {
  out1 = 0.0
}
"#;

const TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE: &str = r#"
outs { out1 }
init {
  a: f64[8]
  b: i32[4]
  c: i64[2]
  d: bool[2]
  a[0] = 0.5
  b[0] = 2
  c[0] = i64(3)
  d[0] = true
}
sample {
  out1 = f32(a[0.0]) + f32(b[0.0]) + f32(c[0.0]) + f32(d[0.0])
}
"#;

const TYPED_DATA_STRUCT_SCALAR_PRIMITIVES_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Cell { a: i32, b: f64, c: bool }
init {
  cells: Cell[1]
  cell = cells[0]
  cell.a = 2
  cell.b = 3.5
  cell.c = true
}
sample {
  cell = cells[0]
  out1 = f32(cell.a) + f32(cell.b) + f32(cell.c)
}
"#;
const DATA_BOOL_INDEX_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  buf[true] = 1.0
  out1 = buf[0.0]
}
"#;

const DATA_CONST_OOB_INDEX_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  out1 = buf[4]
}
"#;

const TYPED_WIDENING_ASSIGNMENT_OK_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = 1
}
sample {
  out1 = f32(x)
}
"#;

const TYPED_INIT_F64_PRESERVES_PRECISION_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: f64 = f64(1.234567890123)
}
sample {
  if (x == f64(1.234567890123)) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const TYPED_INIT_I64_PRESERVES_VALUE_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(9007199254740993)
}
sample {
  if (x == i64(9007199254740993)) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const TYPED_BLOCK_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
block {
  x: f64 = f64(2.5)
  sample {
    out1 = f32(x)
  }
}
"#;

const TYPED_SAMPLE_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
sample {
  x: i32 = i32(7)
  out1 = f32(x)
}
"#;

const TYPED_DEF_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
def foo() {
  x: i32 = i32(3)
  y: f64 = f64(0.5)
  return f32(x) + f32(y)
}
sample {
  out1 = foo()
}
"#;

const DEF_RETURN_F64_INFERENCE_EXAMPLE: &str = r#"
outs { out1 }
def mydef() {
  return f64(0.5)
}
sample {
  out1 = f32(mydef())
}
"#;

const DEF_MONOMORPHIZES_FROM_CALL_ARGUMENTS_OK_EXAMPLE: &str = r#"
outs { out1 }
def id(x) {
  return x
}
sample {
  out1 = f32(id(f64(1.25)))
}
"#;

const DEF_MONOMORPHIZES_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
outs { out1 }
def twice(x) {
  return x + x
}
sample {
  out1 = twice(1.5) + f32(twice(f64(2.25)))
}
"#;

const NON_GENERIC_DEF_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
def id(x) {
  return x
}
sample {
  out1 = id<f32>(1.0)
}
"#;

const GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair<f64>(f64(1.25), f64(0.5))
}
sample {
  out1 = f32(p.a + p.b)
}
"#;

const GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair(1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box<T> { v: T }
init {
  x = f64(2.5)
  b = Box(x)
}
sample {
  out1 = f32(b.v)
}
"#;

const GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Bank<T> { taps: T[2] }
init {
  b = Bank()
}
sample {
  out1 = 0.0
}
"#;

const GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair<f32, f64>(1.0, 2.0)
}
sample {
  out1 = 0.0
}
"#;

const NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair<f32>(1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box<T> { v: T }
init {
  a = Box<f32>(1.0)
  b = Box<f64>(f64(0.25))
}
sample {
  out1 = a.v + f32(b.v)
}
"#;

const GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Bank<T> { taps: T[2] }
init {
  b = Bank<f64>()
  b.taps[0.0] = f64(1.5)
  b.taps[1.0] = f64(0.5)
}
sample {
  out1 = f32(b.taps[0.0] + b.taps[1.0])
}
"#;

const GENERIC_STRUCT_METHOD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> {
  a: T
  b: T
  def sum(self) {
    return self.a + self.b
  }
}
init {
  p = Pair<f64>(f64(1.25), f64(0.75))
}
sample {
  out1 = f32(p.sum())
}
"#;

const GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain<f64>(g = f64(0.5))
}
sample {
  out1 = f32(p(f64(2.0)))
}
"#;

const GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain(g = 0.5)
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_DEFAULT_ONLY_INFERENCE_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 0.5 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain()
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_ARRAY_INFER_FROM_ARRAY_VAR_OK_EXAMPLE: &str = r#"
proc Tap<T> {
  params { w: T[2] = [0.0, 0.0] }
  outs { out1: T }
  sample {
    out1 = w[0.0] + w[1.0]
  }
}
outs { out1 }
init {
  w0: f64[2]
  w0[0.0] = f64(0.25)
  w0[1.0] = f64(0.75)
  p = Tap(w = w0)
}
sample {
  out1 = f32(p())
}
"#;

const GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE: &str = r#"
proc Hold<T> {
  outs { out1: T }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
init {
  p = Hold()
}
sample {
  out1 = 0.0
}
"#;

const PROC_STATE_STRUCT_CTOR_OK_EXAMPLE: &str = r#"
struct Pair { a: f32, b: f32 }

proc Voice {
  outs { out1 }
  init {
    s = Pair(1.0, 2.0)
  }
  sample {
    out1 = s.a + s.b
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const PROC_STATE_GENERIC_STRUCT_CTOR_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
struct Pair<T> { a: T, b: T }

proc Voice {
  outs { out1 }
  init {
    s = Pair<f64>(f64(1.0), f64(2.0))
  }
  sample {
    out1 = f32(s.a + s.b)
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const PROC_STATE_GENERIC_STRUCT_CTOR_INFERRED_TYPE_ARGS_OK_EXAMPLE: &str = r#"
struct Pair<T> { a: T, b: T }

proc Voice {
  outs { out1 }
  init {
    x = f64(1.0)
    y = f64(2.0)
    s = Pair(x, y)
  }
  sample {
    out1 = f32(s.a + s.b)
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const GENERIC_PROC_TYPE_ARG_ARITY_ERROR_EXAMPLE: &str = r#"
proc Gain<T, U> {
  ins { in1: T }
  outs { out1: T }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  p = Gain<f64>()
}
sample {
  out1 = f32(p(f64(2.0)))
}
"#;

const NON_GENERIC_PROC_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
proc Gain {
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  p = Gain<f64>()
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p1 = Gain<f32>(g = 2.0)
  p2 = Gain<f64>(g = f64(0.25))
}
sample {
  out1 = p1(1.0) + f32(p2(f64(2.0)))
}
"#;

const GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE: &str = r#"
proc Mix<T> {
  ins { in1: T[2] }
  outs { out1: T }
  params { gains: T[2] = [1.0, 0.5] }
  sample {
    out1 = in1[0] * gains[0] + in1[1] * gains[1]
  }
}
outs { out1 }
init {
  p = Mix<f64>()
}
sample {
  out1 = f32(p([f64(2.0), f64(4.0)]))
}
"#;

const GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE: &str = r#"
proc Sum2<T> {
  outs { out1: T }
  init {
    x: T[2]
    x[0.0] = 1.0
    x[1.0] = 2.0
  }
  sample {
    out1 = x[0.0] + x[1.0]
  }
}
outs { out1 }
init {
  p = Sum2<f64>()
}
sample {
  out1 = f32(p())
}
"#;

const GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE: &str = r#"
buffers { buf1: buffer[f64] }
proc Tap<T> {
  buffers { line: buffer[T] }
  outs { out1: T }
  sample {
    out1 = line[0]
  }
}
outs { out1 }
init {
  p = Tap<f64>(line = buf1)
}
sample {
  out1 = f32(p())
}
"#;

const FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
def mydef() {
  return f64(0.5)
}
init {
  x = mydef() * 2
  z = x
  x = x + f64(0.25)
  z = z + f64(0.25)
}
sample {
  out1 = f32(z)
}
"#;

const FIRST_ASSIGNMENT_INT_IS_STICKY_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x = 0
  x = 1.5
}
sample {
  out1 = 0.0
}
"#;

const PROC_FIRST_ASSIGNMENT_FROM_DEF_RETURN_EXAMPLE: &str = r#"
def mydef() {
  return f64(0.5)
}
proc AutoTypeProc {
  outs { out1 }
  init {
    x = mydef() * 2
    z = x
    x = x + f64(0.25)
    z = z + f64(0.25)
  }
  sample {
    out1 = f32(z)
  }
}
outs { out1 }
init {
  p = AutoTypeProc()
}
sample {
  out1 = p()
}
"#;

const TYPED_I32_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_i32() {
  x: i32 = i32(40)
  return f32(x)
}
init {
  xi: i32 = i32(10)
}
block {
  xb: i32 = i32(20)
  sample {
    xs: i32 = i32(30)
    out1 = f32(xi) + f32(xb) + f32(xs) + local_i32()
  }
}
"#;

const TYPED_F64_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_f64() {
  x: f64 = f64(4.0)
  return x
}
init {
  xi: f64 = f64(1.0)
}
block {
  xb: f64 = f64(2.0)
  sample {
    xs: f64 = f64(3.0)
    out1 = f32(xi + xb + xs + local_f64())
  }
}
"#;

const TYPED_I64_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_i64() {
  x: i64 = i64(40)
  return f32(x)
}
init {
  xi: i64 = i64(10)
}
block {
  xb: i64 = i64(20)
  sample {
    xs: i64 = i64(30)
    out1 = f32(xi) + f32(xb) + f32(xs) + local_i64()
  }
}
"#;

const TYPED_BOOL_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_bool_gate() {
  x: bool = true
  if (x) { return 1.0 } else { return 0.0 }
}
init {
  bi: bool = true
}
block {
  bb: bool = false
  sample {
    bs: bool = true
    if (bi && bs && (bb == false) && (local_bool_gate() > 0.5)) {
      out1 = 1.0
    } else {
      out1 = 0.0
    }
  }
}
"#;

const PROC_SINGLE_OUT_CALL_EXAMPLE: &str = r#"
proc GainProc {
  ins { in1 }
  params { gain = 2.0 }
  outs { out1 }
  init { }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  p = GainProc(gain = 3.0)
}
sample {
  out1 = p(0.5)
}
"#;

const PROC_SINGLE_OUT_FIELD_ACCESS_EXAMPLE: &str = r#"
proc NamedGainProc {
  ins { in1 }
  outs { wet }
  sample {
    wet = in1 * 3.0
  }
}
outs { out1 }
init {
  p = NamedGainProc()
}
sample {
  p(0.5)
  out1 = p.wet + p.out1
}
"#;

const PROC_MULTI_OUT_CALL_FIELD_EXAMPLE: &str = r#"
proc SplitProc {
  ins { in1 }
  outs { out1, out2 }
  init { }
  sample {
    out1 = in1
    out2 = in1 * 2.0
  }
}
outs { out1 }
init {
  s = SplitProc()
}
sample {
  out1 = s(0.25).out2
}
"#;

const PROC_MULTI_OUT_FIELD_ALIAS_EXAMPLE: &str = r#"
proc NamedSplitProc {
  ins { in1 }
  outs { dry, wet }
  init { }
  sample {
    dry = in1
    wet = in1 * 2.0
  }
}
outs { out1 }
init {
  p = NamedSplitProc()
}
sample {
  p(0.25)
  out1 = p.out2
}
"#;

const PROC_PARAM_MUTATION_IMMEDIATE_EXAMPLE: &str = r#"
proc ParamProc {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  init { }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  p = ParamProc()
}
sample {
  p.gain = 4.0
  out1 = p(2.0)
}
"#;

const PROC_OPTIONAL_INIT_EXAMPLE: &str = r#"
proc NoInitProc {
  ins { in1 }
  params { gain = 2.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  p = NoInitProc(gain = 4.0)
}
sample {
  out1 = p(0.25)
}
"#;

const PROC_BLOCK_WRAPS_SAMPLE_EXAMPLE: &str = r#"
proc BlockProc {
  ins { in1 }
  outs { out1 }
  block {
    gain = 3.0
    sample {
      out1 = in1 * gain
    }
    gain = gain + 1.0
  }
}
outs { out1 }
init {
  p = BlockProc()
}
sample {
  out1 = p(2.0)
}
"#;

const PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE: &str = r#"
proc InnerProc {
  ins { in1 }
  outs { out1 }
  block {
    gain = 2.0
    sample {
      out1 = in1 * gain
    }
  }
}
proc OuterProc {
  ins { in1 }
  outs { out1 }
  init {
    inner = InnerProc()
  }
  sample {
    out1 = inner(in1)
  }
}
outs { out1 }
init {
  p = OuterProc()
}
sample {
  out1 = p(1.0)
}
"#;

const PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_EXAMPLE: &str = r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 0
}
sample {
  voices[idx]()
  v0 = voices[0]
  v1 = voices[1]
  out1 = v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post
  idx = 1 - idx
}
"#;

const PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE: &str = r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 0
}
sample {
  x = voices[idx]().out1 + 0.0
  v0 = voices[0]
  v1 = voices[1]
  out1 = x * 0.0 + v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post
  idx = 1 - idx
}
"#;

const PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_CLAMPED_INDEX_CONSISTENCY_EXAMPLE: &str = r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 99
}
sample {
  voices[idx]()
  v0 = voices[0]
  v1 = voices[1]
  out1 = v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post
}
"#;

const PROC_ARRAY_DYNAMIC_INDEX_MULTI_CALL_EXPR_EVAL_ORDER_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    sample_count = 0.0
  }
  sample {
    sample_count = sample_count + 1.0
    out1 = sample_count
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 0
}
sample {
  out1 = voices[idx]().out1 * 10.0 + voices[idx]().out1
}
"#;

const PROC_ARRAY_DYNAMIC_INDEX_FIVE_CALL_EXPR_EVAL_ORDER_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    sample_count = 0.0
  }
  sample {
    sample_count = sample_count + 1.0
    out1 = sample_count
  }
}
outs { out1 }
init {
  voices: Voice[5] = [Voice(), Voice(), Voice(), Voice(), Voice()]
  idx: i32 = 3
}
sample {
  out1 = voices[idx]().out1 * 10000.0 + voices[idx]().out1 * 1000.0 + voices[idx]().out1 * 100.0 + voices[idx]().out1 * 10.0 + voices[idx]().out1
}
"#;

fn proc_array_harmonics_block_voice_program(top_level_exec: &str) -> String {
    format!(
        r#"
proc Voice {{
  outs {{ out1 }}
  params {{
    freq = 440.0
    amp = 1.0
  }}
  init {{
    phase = 0.0
  }}
  block {{
    incr = freq / SR
    sample {{
      phase = phase + incr
      if (phase >= 1.0) {{
        phase = phase - 1.0
      }}
      out1 = sin(phase * f32(TWO_PI)) * amp
    }}
  }}
}}
outs {{ out1 }}
init {{
  voices: Voice[10] = Voice()
  for i in 0..10 {{
    h = f32(i + 1)
    voices[i].init(freq = 55.0 * h, amp = 0.12 / h)
  }}
}}
{top_level_exec}
"#
    )
}

const NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE: &str = r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
proc Bank {
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(), Voice()]
    idx: i32 = 0
  }
  sample {
    x = voices[idx]().out1 + 0.0
    v0 = voices[0]
    v1 = voices[1]
    out1 = x * 0.0 + v0.pre * 1000.0 + v1.pre * 100.0 + v0.post * 10.0 + v1.post
    idx = 1 - idx
  }
}
outs { out1 }
init {
  b = Bank()
}
sample {
  out1 = b()
}
"#;

const PROC_BUFFER_MONO_EXAMPLE: &str = r#"
buffers { buf1: buffer[f32] }
proc ReadBufProc {
  buffers { line: buffer[f32] }
  outs { out1 }
  init {
    idx = 0.0
  }
  sample {
    out1 = line[idx]
    idx = idx + 1.0
  }
}
outs { out1 }
init {
  p = ReadBufProc(line = buf1)
}
sample {
  out1 = p()
}
"#;

const PROC_TYPED_STATE_PRESERVED_EXAMPLE: &str = r#"
proc CounterProc {
  outs { out1 }
  init {
    idx: i32 = 0
  }
  sample {
    idx = idx + 1
    out1 = f32(idx)
  }
}
outs { out1 }
init {
  p = CounterProc()
}
sample {
  out1 = p()
}
"#;

const PROC_I32_ARRAY_INCREMENT_PRESERVED_EXAMPLE: &str = r#"
proc CounterArrProc {
  outs { out1 }
  init {
    idx: i32[4]
    idx[0] = 0
  }
  sample {
    idx[0] = idx[0] + 1
    out1 = f32(idx[0])
  }
}
outs { out1 }
init {
  p = CounterArrProc()
}
sample {
  out1 = p()
}
"#;

const PROC_DATA_LEN_METHOD_EXAMPLE: &str = r#"
proc LenProc {
  outs { out1 }
  init {
    buf: f32[8]
  }
  sample {
    out1 = f32(buf.len())
  }
}
outs { out1 }
init {
  p = LenProc()
}
sample {
  out1 = p()
}
"#;

const PROC_BUFFER_MISSING_CTOR_ARG_ERROR_EXAMPLE: &str = r#"
buffers { buf1: buffer[f32] }
proc ReadBufProc {
  buffers { line: buffer[f32] }
  outs { out1 }
  sample {
    out1 = line[0]
  }
}
outs { out1 }
init {
  p = ReadBufProc()
}
sample {
  out1 = p()
}
"#;

const PROC_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE: &str = r#"
proc GainProc {
  params { gain = 2.0 }
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  p = GainProc(3.0)
}
sample {
  out1 = p(1.0)
}
"#;

const PROC_NESTED_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE: &str = r#"
proc InnerProc {
  params { gain = 2.0 }
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
proc OuterProc {
  ins { in1 }
  outs { out1 }
  init {
    inner = InnerProc(3.0)
  }
  sample {
    out1 = inner(in1)
  }
}
outs { out1 }
init {
  p = OuterProc()
}
sample {
  out1 = p(1.0)
}
"#;

const PROC_ARRAY_INPUT_EXAMPLE: &str = r#"
proc SumProc {
  ins { in1: f32[2] }
  outs { out1 }
  sample {
    out1 = in1[0] + in1[1]
  }
}
outs { out1 }
init {
  p = SumProc()
}
sample {
  out1 = p([0.25, 0.75])
}
"#;

const PROC_ARRAY_INPUT_VAR_EXAMPLE: &str = r#"
proc SumProc {
  ins { in1: f32[2] }
  outs { out1 }
  sample {
    out1 = in1[0] + in1[1]
  }
}
outs { out1 }
init {
  p = SumProc()
}
sample {
  x: f32[2] = [0.1, 0.2]
  out1 = p(x)
}
"#;

const PROC_ARRAY_OUTPUT_INDEXED_CALL_EXAMPLE: &str = r#"
proc PairProc {
  ins { in1 }
  outs { out1: f32[2] }
  sample {
    out1[0] = in1
    out1[1] = in1 * 2.0
  }
}
outs { out1 }
init {
  p = PairProc()
}
sample {
  out1 = p(0.5).out2
}
"#;

const PROC_ARRAY_PARAM_CTOR_EXAMPLE: &str = r#"
proc MixProc {
  ins { in1: f32[2] }
  params { gain: f32[2] = [1.0, 1.0] }
  outs { out1 }
  sample {
    out1 = in1[0] * gain[0] + in1[1] * gain[1]
  }
}
outs { out1 }
init {
  p = MixProc(gain = [2.0, 3.0])
}
sample {
  out1 = p([0.5, 0.25])
}
"#;

const PROC_ARRAY_DYNAMIC_INDEX_CLAMP_EXAMPLE: &str = r#"
proc ClampReadProc {
  ins { in1: f32[2] }
  outs { out1 }
  sample {
    idx: i32 = 99
    out1 = in1[idx]
  }
}
outs { out1 }
init {
  p = ClampReadProc()
}
sample {
  out1 = p([0.25, 0.75])
}
"#;

const PROC_ARRAY_DYNAMIC_UNSAFE_READ_EXAMPLE: &str = r#"
proc UnsafeReadProc {
  ins { in1: f32[2] }
  outs { out1 }
  sample {
    idx: i32 = 1
    out1 = unsafe_read(in1, idx)
  }
}
outs { out1 }
init {
  p = UnsafeReadProc()
}
sample {
  out1 = p([0.25, 0.75])
}
"#;

const PROC_ARRAY_DYNAMIC_UNSAFE_WRITE_EXAMPLE: &str = r#"
proc UnsafeWriteProc {
  outs { out1: f32[2] }
  sample {
    idx: i32 = 1
    unsafe_write(out1, idx, 2.0)
  }
}
outs { out1 }
init {
  p = UnsafeWriteProc()
}
sample {
  p()
  out1 = p.out2
}
"#;

const PROC_ARRAY_DYNAMIC_UNSAFE_OOB_COMPILES_EXAMPLE: &str = r#"
proc UnsafeOobProc {
  ins { in1: f32[2] }
  outs { out1 }
  sample {
    idx: i32 = 99
    out1 = unsafe_read(in1, idx)
  }
}
outs { out1 }
init {
  p = UnsafeOobProc()
}
sample {
  out1 = p([0.25, 0.75])
}
"#;

const PROC_ARRAY_CONSTANT_INDEX_OOB_REJECTED_EXAMPLE: &str = r#"
proc OobProc {
  params { a: f32[2] = [1.0, 2.0] }
  outs { out1 }
  sample {
    out1 = a[4]
  }
}
outs { out1 }
init {
  p = OobProc()
}
sample {
  out1 = p()
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 3.0)]
}
sample {
  out1 = voices[1](0.5)
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_FIELD_CALL_EXAMPLE: &str = r#"
proc Pair {
  ins { in1 }
  outs { a, b }
  sample {
    a = in1
    b = in1 * 2.0
  }
}
outs { out1 }
init {
  voices: Pair[2] = [Pair(), Pair()]
}
sample {
  out1 = voices[0](0.5).out2
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 3.0)]
  idx: i32 = 1
}
sample {
  out1 = voices[idx](0.5)
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_OVERSAMPLED_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample 2 {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 3.0)]
  idx: i32 = 1
}
sample {
  out1 = voices[idx](0.5)
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_FIELD_DYNAMIC_INDEX_EXAMPLE: &str = r#"
proc Pair {
  ins { in1 }
  outs { a, b }
  sample {
    a = in1
    b = in1 * 2.0
  }
}
outs { out1 }
init {
  voices: Pair[2] = [Pair(), Pair()]
  idx: i32 = 0
}
sample {
  out1 = voices[idx](0.5).out2
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE: &str = r#"
proc Voice {
  buffers { buf: f32 }
  outs { out1 }
  sample {
    out1 = buf[0]
  }
}
buffers {
  buf1: f32
  buf2: f32
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(buf = buf1), Voice(buf = buf2)]
  idx: i32 = 1
}
sample {
  out1 = voices[idx]()
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_DYNAMIC_INDEX_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 3.0)]
  idx: i32 = 1
}
sample {
  a = voices[idx]
  out1 = a(0.5)
}
"#;

const PROC_INSTANCE_ARRAY_INDEXED_ALIAS_OUT_READ_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 3.0)]
  idx: i32 = 1
}
sample {
  a = voices[idx]
  a()
  out1 = a.out1
}
"#;

const NESTED_PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
proc Bank {
  ins { in1 }
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(gain = 2.0), Voice(gain = 4.0)]
    idx: i32 = 1
  }
  sample {
    a = voices[idx]
    out1 = a(in1)
  }
}
outs { out1 }
init {
  b = Bank()
}
sample {
  out1 = b(0.25)
}
"#;

const PROC_INSTANCE_ARRAY_LEN_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
init {
  voices: Voice[3] = Voice()
}
sample {
  out1 = f32(voices.len())
}
"#;

const NESTED_PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
proc Bank {
  ins { in1 }
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(gain = 1.0), Voice(gain = 4.0)]
  }
  sample {
    out1 = voices[1](in1)
  }
}
outs { out1 }
init {
  b = Bank()
}
sample {
  out1 = b(0.25)
}
"#;

const DEEP_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
proc Bank {
  ins { in1 }
  params { base = 1.0 }
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(gain = base), Voice(gain = base + 1.0)]
    v_idx: i32 = 99
  }
  sample {
    out1 = voices[v_idx](in1)
  }
}
outs { out1 }
init {
  banks: Bank[2] = [Bank(base = 1.0), Bank(base = 100.0)]
  b_idx: i32 = 99
}
sample {
  out1 = banks[b_idx](0.5)
}
"#;

const DEEPER_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = in1 * gain
  }
}
proc Bank {
  ins { in1 }
  params { base = 1.0 }
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(gain = base), Voice(gain = base + 1.0)]
    v_idx: i32 = 99
  }
  sample {
    out1 = voices[v_idx](in1)
  }
}
proc Rack {
  ins { in1 }
  params { base = 1.0 }
  outs { out1 }
  init {
    banks: Bank[2] = [Bank(base = base), Bank(base = base + 10.0)]
    b_idx: i32 = 99
  }
  sample {
    out1 = banks[b_idx](in1)
  }
}
outs { out1 }
init {
  racks: Rack[2] = [Rack(base = 1.0), Rack(base = 100.0)]
  r_idx: i32 = 99
}
sample {
  out1 = racks[r_idx](0.5)
}
"#;

const PROC_INSTANCE_ARRAY_BROADCAST_CTOR_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice(gain = 0.5)
}
sample {
  out1 = voices[1]()
}
"#;

const PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_LITERAL_ARG_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
outs { out1 }
init {
  voices: Voice[2] = Voice(gain = [0.5, 0.8])
}
sample {
  out1 = voices[1]()
}
"#;

const PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_SYMBOL_ARG_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
outs { out1 }
init {
  g = [0.5, 0.8]
  voices: Voice[2] = Voice(gain = g)
}
sample {
  out1 = voices[1]()
}
"#;

const UNTYPED_INIT_ARRAY_FIRST_ELEMENT_TYPE_MISMATCH_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  a = [0, 1.5]
}
sample {
  out1 = 0.0
}
"#;

const PROC_INSTANCE_ARRAY_BROADCAST_CTOR_MIXED_BUFFER_ARRAY_ARG_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  buffers { buf: f32 }
  outs { out1 }
  sample {
    out1 = buf[0] * gain
  }
}
buffers {
  buf1: f32
  buf2: f32
}
outs { out1 }
init {
  voices: Voice[2] = Voice(gain = 0.5, buf = [buf1, buf2])
}
sample {
  out1 = voices[1]()
}
"#;

const NESTED_PROC_INIT_UNTYPED_ARRAY_SYMBOL_ARG_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
proc Bank {
  outs { out1 }
  init {
    g = [0.2, 0.6]
    voices: Voice[2] = Voice(gain = g)
  }
  sample {
    out1 = voices[1]()
  }
}
outs { out1 }
init {
  b = Bank()
}
sample {
  out1 = b()
}
"#;

const TOP_LEVEL_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
outs { out1 }
init {
  voices: Voice[BLOCK_SIZE / 2] = [Voice(gain = 0.5), Voice(gain = 0.75)]
  p = Voice(gain = 1.25)
}
sample {
  out1 = p()
}
"#;

const NESTED_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE: &str = r#"
proc Voice {
  params { gain = 1.0 }
  outs { out1 }
  sample {
    out1 = gain
  }
}
proc Bank {
  outs { out1 }
  init {
    voices: Voice[BLOCK_SIZE / 2] = [Voice(gain = 0.2), Voice(gain = 0.4)]
    p = Voice(gain = 1.5)
  }
  sample {
    out1 = p()
  }
}
outs { out1 }
init {
  b = Bank()
}
sample {
  out1 = b()
}
"#;

const TOP_LEVEL_PROC_INSTANCE_ARRAY_INIT_ARITY_ERROR_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  sample { out1 = 1.0 }
}
outs { out1 }
init {
  voices: Voice[BLOCK_SIZE / 2] = [Voice(), Voice(), Voice()]
}
sample {
  out1 = 0.0
}
"#;

const PROC_NESTED_STATE_EXAMPLE: &str = r#"
proc InnerAcc {
  ins { in1 }
  outs { out1 }
  init {
    acc = 0.0
  }
  sample {
    acc = acc + in1
    out1 = acc
  }
}
proc OuterAcc {
  ins { in1 }
  outs { out1 }
  init {
    inner = InnerAcc()
  }
  sample {
    out1 = inner(in1)
  }
}
outs { out1 }
init {
  p = OuterAcc()
}
sample {
  out1 = p(0.25)
}
"#;

const PROC_DEEP_NESTED_STATE_EXAMPLE: &str = r#"
proc InnerAcc {
  ins { in1 }
  params { gain = 1.0 }
  outs { out1 }
  init {
    acc = 0.0
  }
  sample {
    acc = acc + in1 * gain
    out1 = acc
  }
}
proc MidAcc {
  ins { in1 }
  outs { out1 }
  init {
    inner = InnerAcc(gain = 2.0)
  }
  sample {
    out1 = inner(in1)
  }
}
proc OuterAcc {
  ins { in1 }
  outs { out1 }
  init {
    mid = MidAcc()
  }
  sample {
    out1 = mid(in1)
  }
}
outs { out1 }
init {
  p = OuterAcc()
}
sample {
  out1 = p(0.25)
}
"#;

const PROC_DEEP_NESTED_BUFFER_BIND_EXAMPLE: &str = r#"
buffers { buf1: buffer[f32] }
proc InnerBuf {
  buffers { line: buffer[f32] }
  outs { out1 }
  init {
    idx: i32 = 0
  }
  sample {
    out1 = line[idx]
    idx: i32 = idx + 1
  }
}
proc MidBuf {
  buffers { line: buffer[f32] }
  outs { out1 }
  init {
    inner = InnerBuf(line = line)
  }
  sample {
    out1 = inner()
  }
}
proc OuterBuf {
  buffers { line: buffer[f32] }
  outs { out1 }
  init {
    mid = MidBuf(line = line)
  }
  sample {
    out1 = mid()
  }
}
outs { out1 }
init {
  p = OuterBuf(line = buf1)
}
sample {
  out1 = p()
}
"#;

const TOP_LEVEL_OPTIONAL_INIT_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = 0.75
}
"#;

const STRUCT_METHOD_EXAMPLE: &str = r#"
outs { out1 }
params { freq = 12000.0 }
struct Osc {
  phase: f32
  gain: f32
  def tick(self, hz) {
    self.phase = self.phase + hz * f32(TWO_PI) / SR
    if (self.phase >= f32(TWO_PI)) { self.phase = self.phase - f32(TWO_PI) }
    return sin(self.phase) * self.gain
  }
}
init {
  o = Osc(0.0, 1.0)
}
sample {
  out1 = Osc.tick(o, freq)
}
"#;

const STRUCT_METHOD_DATA_WRITE_EXAMPLE: &str = r#"
outs { out1 }
struct Tap {
  buf: f32[2]
  def write_read(self, x) {
    self.buf[0.0] = x
    return self.buf[0.0]
  }
}
init {
  t = Tap()
}
sample {
  out1 = Tap.write_read(t, 0.75)
}
"#;

const STRUCT_METHOD_SELF_REQUIRED_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Bad {
  x: f32
  def broken(x) {
    return x
  }
}
sample {
  out1 = 0.0
}
"#;

const SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE: &str = r#"
outs { out1 }
init { acc = 0.0 }
sample 4 {
  acc = acc + 1.0
  out1 = acc
}
"#;

const PROC_EQUIV_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE: &str = r#"
proc Counter {
  outs { out1 }
  init { acc = 0.0 }
  sample 4 {
    acc = acc + 1.0
    out1 = acc
  }
}
outs { out1 }
init { c = Counter() }
sample { out1 = c() }
"#;

const PROC_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE: &str = r#"
proc Counter {
  outs { out1 }
  init { x = 0.0 }
  sample 2 {
    x = x + 1.0
    out1 = x
  }
}
outs { out1 }
init { c = Counter() }
sample { out1 = c() }
"#;

const SAMPLE_OVERSAMPLE_FACTOR_2_EXAMPLE: &str = r#"
outs { out1 }
init { x = 0.0 }
sample 2 {
  x = x + 1.0
  out1 = x
}
"#;

const SAMPLE_OVERSAMPLE_INVALID_FACTOR_EXAMPLE: &str = r#"
outs { out1 }
sample 3 {
  out1 = 0.0
}
"#;

const SAMPLE_OVERSAMPLE_NON_LITERAL_FACTOR_EXAMPLE: &str = r#"
outs { out1 }
init { n = 2 }
sample n {
  out1 = 0.0
}
"#;

const SAMPLE_OVERSAMPLE_CONST_EXPR_FACTOR_EXAMPLE: &str = r#"
const OS = 2 * 2
outs { out1 }
sample OS {
  out1 = 0.0
}
"#;

const SAMPLE_OVERSAMPLE_FACTOR_512_SMOKE_EXAMPLE: &str = r#"
outs { out1 }
init { x = 0.0 }
sample 512 {
  x = x + 1.0
  out1 = x
}
"#;

const PROC_SAMPLE_OVERSAMPLE_NAMESPACE_FACTOR_EXAMPLE: &str = r#"
namespace DSP<N = 2>:
  proc Gain:
    outs { out1 }
    sample N * 2:
      out1 = 0.5

outs { out1 }
init:
  g = DSP<4>::Gain()
sample:
  out1 = g()
"#;

const SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
sample 2 {
  out1 = in1
}
"#;

const SAMPLE_OVERSAMPLE_PASSTHROUGH_1X_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1
}
"#;

const SAMPLE_OVERSAMPLE_PASSTHROUGH_4X_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
sample 4 {
  out1 = in1
}
"#;

const PROC_EQUIV_SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE: &str = r#"
proc Passthrough {
  ins { in1 }
  outs { out1 }
  sample 2 {
    out1 = in1
  }
}
ins { in1 }
outs { out1 }
init { p = Passthrough() }
sample { out1 = p(in1=in1) }
"#;

const SAMPLE_OVERSAMPLE_ALIAS_BASELINE_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1 * in1 * in1
}
"#;

const SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
sample 4 {
  out1 = in1 * in1 * in1
}
"#;

const PROC_EQUIV_SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE: &str = r#"
proc Cubic {
  ins { in1 }
  outs { out1 }
  sample 4 {
    out1 = in1 * in1 * in1
  }
}
ins { in1 }
outs { out1 }
init { p = Cubic() }
sample { out1 = p(in1=in1) }
"#;

const SAMPLE_OVERSAMPLE_STD_SINE_1X_EXAMPLE: &str = r#"
import std/osc
init:
  osc = std::osc::Sine(freq = 50.0)
sample:
  out1 = osc()
"#;

const SAMPLE_OVERSAMPLE_STD_SINE_2X_EXAMPLE: &str = r#"
import std/osc
init:
  osc = std::osc::Sine(freq = 50.0)
sample 2:
  out1 = osc()
"#;

const SAMPLE_OVERSAMPLE_STD_SINE_4X_EXAMPLE: &str = r#"
import std/osc
init:
  osc = std::osc::Sine(freq = 50.0)
sample 4:
  out1 = osc()
"#;

const PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_1X_EXAMPLE: &str = r#"
proc SineProc {
  params { freq = 50.0 }
  outs { out1 }
  init { phase = 0.0 }
  sample {
    phase = phase + (freq * f32(TWO_PI) / SR)
    if (phase >= f32(TWO_PI)) {
      phase = phase - f32(TWO_PI)
    }
    out1 = sin(phase)
  }
}
outs { out1 }
init { osc = SineProc() }
sample { out1 = osc() }
"#;

const PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_8X_EXAMPLE: &str = r#"
proc SineProc {
  params { freq = 50.0 }
  outs { out1 }
  init { phase = 0.0 }
  sample 8 {
    phase = phase + (freq * f32(TWO_PI) / SR)
    if (phase >= f32(TWO_PI)) {
      phase = phase - f32(TWO_PI)
    }
    out1 = sin(phase)
  }
}
outs { out1 }
init { osc = SineProc() }
sample { out1 = osc() }
"#;

const SAMPLE_OVERSAMPLE_PERF_BASELINE_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
init {
  s1 = 0.0
  s2 = 0.0
}
sample {
  x = in1 * 1.9
  y = x - (x * x * x) * 0.33333334
  s1 = s1 + (y - s1) * 0.22
  s2 = s2 + (s1 - s2) * 0.17
  out1 = s2 - (s2 * s2 * s2) * 0.1
}
"#;

const SAMPLE_OVERSAMPLE_PERF_N4_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
init {
  s1 = 0.0
  s2 = 0.0
}
sample 4 {
  x = in1 * 1.9
  y = x - (x * x * x) * 0.33333334
  s1 = s1 + (y - s1) * 0.22
  s2 = s2 + (s1 - s2) * 0.17
  out1 = s2 - (s2 * s2 * s2) * 0.1
}
"#;

const SAMPLE_OVERSAMPLE_FACTOR_32_SMOKE_EXAMPLE: &str = r#"
outs { out1 }
init { x = 0.0 }
sample 32 {
  x = x + 1.0
  out1 = x
}
"#;

const PROC_SAMPLE_OVERSAMPLE_FACTOR_64_SMOKE_EXAMPLE: &str = r#"
proc Counter {
  outs { out1 }
  init { x = 0.0 }
  sample 64 {
    x = x + 1.0
    out1 = x
  }
}
outs { out1 }
init { c = Counter() }
sample { out1 = c() }
"#;

const EVENT_SCALAR_UPDATE_EXAMPLE: &str = r#"
outs { out1 }
events {
  set_amp(value: f32) {
    amp = value
  }
}
init {
  amp = 0.0
}
sample {
  out1 = amp
}
"#;

const EVENT_ARRAY_UPDATE_EXAMPLE: &str = r#"
outs { out1 }
events {
  set_curve(values: f32[2]) {
    amp = values[0] + values[1]
  }
}
init {
  amp = 0.0
}
sample {
  out1 = amp
}
"#;

const EVENT_LOCAL_ARRAY_LITERAL_EXAMPLE: &str = r#"
outs { out1 }
events {
  ping() {
    b = [1, 2, 3]
    amp = f32(b[2])
  }
}
init {
  amp = 0.0
}
sample {
  out1 = amp
}
"#;

const EVENT_PROC_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
events {
  note_on(value: f32) {
    voice.note_on(value)
  }
}
init {
  voice = Voice()
}
sample {
  out1 = voice()
}
"#;

const EVENT_PROC_CALL_FROM_TOP_LEVEL_INIT_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
init {
  voice = Voice()
  voice.note_on(0.55)
}
sample {
  out1 = voice()
}
"#;

const EVENT_PROC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
init {
  voice = Voice()
}
sample {
  voice.note_on(0.35)
  out1 = voice()
}
"#;

const EVENT_PROC_ARRAY_DYNAMIC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 99
}
sample {
  voices[idx].note_on(0.42)
  out1 = voices[idx]()
}
"#;

const EVENT_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
events {
  note_on(value: f32) {
    idx: i32 = 1
    voices[idx].note_on(value)
  }
}
init {
  voices: Voice[2] = [Voice(), Voice()]
}
sample {
  out1 = voices[1]()
}
"#;

const EVENT_PROC_ARRAY_ALIAS_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
events {
  note_on(value: f32) {
    v = voices[idx]
    v.note_on(value)
  }
}
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 99
}
sample {
  out1 = voices[1]()
}
"#;

const EVENT_NESTED_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}

proc Bank {
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(), Voice()]
  }
  events {
    note_on(value: f32) {
      idx: i32 = 1
      voices[idx].note_on(value)
    }
  }
  sample {
    out1 = voices[1]()
  }
}

outs { out1 }
events {
  note_on(value: f32) {
    bank.note_on(value)
  }
}
init {
  bank = Bank()
}
sample {
  out1 = bank()
}
"#;

const EVENT_DEEP_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
proc Bank {
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(), Voice()]
    v_idx: i32 = 99
  }
  events {
    note_on(value: f32) {
      voices[v_idx].note_on(value)
    }
  }
  sample {
    out1 = voices[v_idx]()
  }
}
outs { out1 }
events {
  note_on(value: f32) {
    banks[b_idx].note_on(value)
  }
}
init {
  banks: Bank[2] = [Bank(), Bank()]
  b_idx: i32 = 99
}
sample {
  out1 = banks[b_idx]()
}
"#;

const EVENT_DEEPER_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
proc Bank {
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(), Voice()]
    v_idx: i32 = 99
  }
  events {
    note_on(value: f32) {
      voices[v_idx].note_on(value)
    }
  }
  sample {
    out1 = voices[v_idx]()
  }
}
proc Rack {
  outs { out1 }
  init {
    banks: Bank[2] = [Bank(), Bank()]
    b_idx: i32 = 99
  }
  events {
    note_on(value: f32) {
      banks[b_idx].note_on(value)
    }
  }
  sample {
    out1 = banks[b_idx]()
  }
}
outs { out1 }
events {
  note_on(value: f32) {
    racks[r_idx].note_on(value)
  }
}
init {
  racks: Rack[2] = [Rack(), Rack()]
  r_idx: i32 = 99
}
sample {
  out1 = racks[r_idx]()
}
"#;

const EVENT_PROC_ARRAY_ALIAS_FORWARD_FROM_PARENT_PROC_EVENT_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}

proc Bank {
  outs { out1 }
  init {
    voices: Voice[2] = [Voice(), Voice()]
    idx: i32 = 99
  }
  events {
    note_on(value: f32) {
      v = voices[idx]
      v.note_on(value)
    }
  }
  sample {
    out1 = voices[1]()
  }
}

outs { out1 }
events {
  note_on(value: f32) {
    bank.note_on(value)
  }
}
init {
  bank = Bank()
}
sample {
  out1 = bank()
}
"#;

const PROC_EVENT_EXPRESSION_POSITION_ERROR_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
init {
  voice = Voice()
}
sample {
  out1 = voice.note_on(0.5)
}
"#;

const PROC_EVENT_OWNING_SELF_CALL_ERROR_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    self.note_on(0.5)
    out1 = amp
  }
}
outs { out1 }
init {
  voice = Voice()
}
sample {
  out1 = voice()
}
"#;

const PROC_EVENT_UNKNOWN_IN_PARENT_INIT_ERROR_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}

proc Bank {
  outs { out1 }
  init {
    voice = Voice()
    voice.not_real(0.5)
  }
  sample {
    out1 = voice()
  }
}

outs { out1 }
init {
  bank = Bank()
}
sample {
  out1 = bank()
}
"#;

const PROC_EVENT_MISSING_ARG_IN_TOP_LEVEL_SAMPLE_ERROR_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}
outs { out1 }
init {
  voice = Voice()
}
sample {
  voice.note_on()
  out1 = voice()
}
"#;

const PROC_EVENT_SLICE_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[]) {
      sum = values[0] + values[1] + values[2] + values[3]
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
init {
  loader = Loader()
  ir: f32[4] = [0.1, 0.2, 0.3, 0.4]
  loader.set_values(ir)
}
sample {
  out1 = loader()
}
"#;

const PROC_EVENT_SLICE_FROM_PARENT_PROC_STATE_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[]) {
      sum = values[0] + values[1]
    }
  }
  sample {
    out1 = sum
  }
}

proc Bank {
  outs { out1 }
  init {
    loader = Loader()
    values: f32[2] = [0.25, 0.75]
  }
  sample {
    loader.set_values(values)
    out1 = loader()
  }
}

outs { out1 }
init {
  bank = Bank()
}
sample {
  out1 = bank()
}
"#;

const PROC_EVENT_FIXED_ARRAY_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[4]) {
      sum = values[0] + values[1] + values[2] + values[3]
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
init {
  loader = Loader()
  ir: f32[4] = [0.1, 0.2, 0.3, 0.4]
  loader.set_values(ir)
}
sample {
  out1 = loader()
}
"#;

const PROC_EVENT_DIRECT_SLICE_FORWARD_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[]) {
      sum = values[0] + values[1] + f32(values.len())
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
init {
  loader = Loader()
  values: f32[4] = [10.0, 20.0, 30.0, 40.0]
}
sample {
  loader.set_values(values[1:-1])
  out1 = loader()
}
"#;

const LOCAL_SLICE_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [5.0, 10.0, 15.0, 20.0, 25.0]
}
sample {
  mid = values[1:-1]
  out1 = mid[0] + mid[2] + f32(mid.len())
}
"#;

const SLICE_FILL_ASSIGN_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
}
sample {
  values[1:-1] = 0.5
  out1 = values[0] + values[1] + values[2] + values[3] + values[4]
}
"#;

const SLICE_COPY_ASSIGN_EXAMPLE: &str = r#"
outs { out1 }
init {
  src: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
  dst: f32[5] = [10.0, 20.0, 30.0, 40.0, 50.0]
}
sample {
  dst[1:-1] = src[0:3]
  out1 = dst[1] + dst[2] + dst[3]
}
"#;

const SLICE_OVERLAP_COPY_ASSIGN_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
}
sample {
  values[1:] = values[:-1]
  out1 = values[1] + values[2] + values[3] + values[4]
}
"#;

// Full slice a[:] read + write
const SLICE_FULL_READ_WRITE_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[4] = [1.0, 2.0, 3.0, 4.0]
}
sample {
  values[:] = 10.0
  out1 = values[0] + values[1] + values[2] + values[3]
}
"#;

// Start-only slice a[2:]
const SLICE_START_ONLY_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
}
sample {
  tail = values[2:]
  out1 = tail[0] + tail[1] + tail[2] + f32(tail.len())
}
"#;

// Negative start index a[-2:]
const SLICE_NEGATIVE_START_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
}
sample {
  tail = values[-2:]
  out1 = tail[0] + tail[1] + f32(tail.len())
}
"#;

// Reverse overlap: values[:-1] = values[1:]
const SLICE_REVERSE_OVERLAP_EXAMPLE: &str = r#"
outs { out1 }
init {
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
}
sample {
  values[:-1] = values[1:]
  out1 = values[0] + values[1] + values[2] + values[3]
}
"#;

// Slice as def argument
const SLICE_AS_DEF_ARG_EXAMPLE: &str = r#"
outs { out1 }
def sum4(data: f32[]) {
  return data[0] + data[1] + data[2] + data[3]
}
init {
  values: f32[6] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
}
sample {
  out1 = sum4(values[1:-1])
}
"#;

// Slice in event handler
const SLICE_IN_EVENT_EXAMPLE: &str = r#"
outs { out1 }
init {
  data: f32[4] = [0.0, 0.0, 0.0, 0.0]
  total = 0.0
}
events {
  fill(values: f32[]) {
    data[:] = 0.0
    data[:] = values[:4]
    total = data[0] + data[1] + data[2] + data[3]
  }
}
sample {
  out1 = total
}
"#;

const GENERIC_PROC_EVENT_SLICE_EXAMPLE: &str = r#"
proc Loader<T> {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: T[]) {
      sum = f32(values[0]) + f32(values.len())
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
init {
  loader = Loader<f32>()
  values: f32[2] = [0.25, 0.75]
  loader.set_values(values)
}
sample {
  out1 = loader()
}
"#;

const GENERIC_PROC_EVENT_SLICE_WITH_SCALAR_PARAMS_EXAMPLE: &str = r#"
proc Loader<T> {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: T[], start: i32, limit: i32) {
      values_len: i32 = i32(values.len())
      n: i32 = values_len - start
      if (n < 0) { n = 0 }
      if (n > limit) { n = limit }
      sum = f32(n)
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
init {
  loader = Loader<f32>()
  values: f32[4] = [0.25, 0.75, 1.25, 1.75]
  loader.set_values(values, 1, 2)
}
sample {
  out1 = loader()
}
"#;

const TOP_LEVEL_EVENT_SLICE_PARAM_EXAMPLE: &str = r#"
outs { out1 }
init { gate = 0.0 }
events {
  load(values: f32[]) {
    gate = values[0] + f32(values.len())
  }
}
sample {
  out1 = gate
}
"#;

const TOP_LEVEL_EVENT_SLICE_PROC_FORWARD_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[]) {
      sum = values[0] + values[1] + f32(values.len())
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
events {
  load(values: f32[]) {
    loader.set_values(values)
  }
}
init {
  loader = Loader()
}
sample {
  out1 = loader()
}
"#;

const TOP_LEVEL_EVENT_FIXED_ARRAY_PROC_FORWARD_EXAMPLE: &str = r#"
proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[4]) {
      sum = values[0] + values[1] + values[2] + values[3]
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
events {
  load(values: f32[4]) {
    loader.set_values(values)
  }
}
init {
  loader = Loader()
}
sample {
  out1 = loader()
}
"#;

const TOP_LEVEL_EVENT_MIXED_FIXED_AND_SLICE_PARAM_EXAMPLE: &str = r#"
outs { out1 }
init { gate = 0.0 }
events {
  load(head: f32[2], tail: f32[]) {
    gate = head[0] + head[1] + tail[0] + f32(tail.len())
  }
}
sample {
  out1 = gate
}
"#;

const TOP_LEVEL_EVENT_LARGE_FIXED_ARRAY_PROC_FORWARD_EXAMPLE: &str = r#"
const N = 96000

proc Loader {
  params { sum = 0.0 }
  outs { out1 }
  events {
    set_values(values: f32[N]) {
      sum = values[0] + values[N - 1]
    }
  }
  sample {
    out1 = sum
  }
}
outs { out1 }
events {
  load(values: f32[N]) {
    loader.set_values(values)
  }
}
init {
  loader = Loader()
}
sample {
  out1 = loader()
}
"#;

const EVENT_PROC_PARENT_INIT_CALL_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}

proc Bank {
  outs { out1 }
  init {
    voice = Voice()
    voice.note_on(0.62)
  }
  sample {
    out1 = voice()
  }
}

outs { out1 }
init {
  bank = Bank()
}
sample {
  out1 = bank()
}
"#;

const EVENT_PROC_PARENT_BLOCK_CALL_EXAMPLE: &str = r#"
proc Voice:
  params:
    amp = 0.0
  outs:
    out1
  events:
    note_on(value: f32):
      amp = value
  sample:
    out1 = amp

proc Bank:
  outs:
    out1
  init:
    voice = Voice()
  block:
    voice.note_on(0.73)
    sample:
      out1 = voice()

outs:
  out1
init:
  bank = Bank()
sample:
  out1 = bank()
"#;

const EVENT_PROC_PARENT_EVENT_CALLED_FROM_TOP_LEVEL_SAMPLE_EXAMPLE: &str = r#"
proc Voice {
  params { amp = 0.0 }
  outs { out1 }
  events {
    note_on(value: f32) {
      amp = value
    }
  }
  sample {
    out1 = amp
  }
}

proc Bank {
  outs { out1 }
  init {
    voice = Voice()
  }
  events {
    note_on(value: f32) {
      voice.note_on(value)
    }
  }
  sample {
    out1 = voice()
  }
}

outs { out1 }
init {
  bank = Bank()
}
sample {
  bank.note_on(0.81)
  out1 = bank()
}
"#;

const PROC_SELF_RECURSIVE_INSTANCE_ERROR_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    other = Voice()
  }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
sample {
  out1 = 0.0
}
"#;

const PROC_SELF_RECURSIVE_ARRAY_ERROR_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    voices: Voice[2] = Voice()
  }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
sample {
  out1 = 0.0
}
"#;

const EVENT_WRITE_OUTPUT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
events {
  ping() {
    out1 = 1.0
  }
}
sample {
  out1 = 0.0
}
"#;

const EVENT_WRITE_NON_INIT_STATE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
block {
  lfo = 0.0
  sample {
    out1 = lfo
  }
}
events {
  ping() {
    lfo = 1.0
  }
}
"#;

const EVENT_PARAM_IMMUTABLE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
events {
  set_curve(values: f32[2]) {
    values[0] = 0.0
  }
}
sample {
  out1 = 0.0
}
"#;

const EVENT_DUPLICATE_NAME_ERROR_EXAMPLE: &str = r#"
outs { out1 }
events {
  ping(value: f32) {
    amp = value
  }
  ping(value: f32) {
    amp = value * 2.0
  }
}
init {
  amp = 0.0
}
sample {
  out1 = amp
}
"#;

const PROC_EVENT_DUPLICATE_NAME_ERROR_EXAMPLE: &str = r#"
proc Voice {
  outs { out1 }
  init {
    amp = 0.0
  }
  events {
    note_on(v: f32) {
      amp = v
    }
    note_on(v: f32) {
      amp = v * 2.0
    }
  }
  sample {
    out1 = amp
  }
}
sample {
  out1 = 0.0
}
"#;

const PROC_EVENT_NAME_CONFLICT_ERROR_EXAMPLE: &str = r#"
proc Voice {
  outs { note_on }
  init {
    gain = 0.0
  }
  events {
    note_on(v: f32) {
      gain = v
    }
  }
  sample {
    note_on = gain
  }
}
sample {
  out1 = 0.0
}
"#;

const PORT_INDEX_OUTS_WRITE: &str = r#"
ins 2
outs 2
sample {
  outs[0] = ins[0] * 2.0
  outs[1] = ins[1] * 3.0
}
"#;

const PORT_INDEX_INS_READ: &str = r#"
ins 4
outs 1
params {
  idx = 0.0
}
sample {
  out1 = ins[idx]
}
"#;

const PORT_INDEX_PARAMS_READ: &str = r#"
outs 1
params {
  a = 1.0
  b = 2.0
  c = 3.0
  d = 4.0
}
init {
  sel = 0.0
}
sample {
  out1 = params[sel]
  sel = sel + 1.0
}
"#;

const PORT_INDEX_OUTS_LOOP: &str = r#"
ins 4
outs 4
sample {
  for i in 0..4 {
    outs[i] = ins[i] * 0.5
  }
}
"#;

#[test]
fn polyphonic_saw_file_example_analyzes() {
    let parsed = parse_program_file(std::path::Path::new("../../examples/polyphonic_saw.onda"))
        .expect("parse should succeed");
    analyze(parsed).expect("analysis should succeed");
}

fn compile_instance(src: &str, frames: usize) -> (onda_runtime::Instance, usize, usize) {
    compile_instance_with_options(
        src,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
}

fn compile_instance_file(path: &str, frames: usize) -> (onda_runtime::Instance, usize, usize) {
    compile_instance_file_with_options(
        path,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
}

fn compile_instance_with_options(
    src: &str,
    frames: usize,
    options: CompileOptions,
) -> (onda_runtime::Instance, usize, usize) {
    let parsed = parse_program(src).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size,
        },
    )
    .expect("semantic analysis should succeed");
    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    let jit = onda_codegen_llvm::lower_and_jit_with_options(typed, options)
        .expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: frames,
            in_channels,
            out_channels,
        },
    )
    .expect("instance should be created");
    (instance, in_channels, out_channels)
}

fn compile_instance_file_with_options(
    path: &str,
    frames: usize,
    options: CompileOptions,
) -> (onda_runtime::Instance, usize, usize) {
    let parsed = parse_program_file(std::path::Path::new(path)).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: options.sample_rate,
            block_size: options.block_size,
        },
    )
    .expect("semantic analysis should succeed");
    let in_channels = typed.ins.len();
    let out_channels = typed.outs.len();
    let jit = onda_codegen_llvm::lower_and_jit_with_options(typed, options)
        .expect("jit lowering should succeed");

    let instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: frames,
            in_channels,
            out_channels,
        },
    )
    .expect("instance should be created");
    (instance, in_channels, out_channels)
}

fn emit_ir(src: &str) -> String {
    let parsed = parse_program(src).expect("parse should succeed");
    let typed = analyze(parsed).expect("analysis should succeed");
    onda_codegen_llvm::lower_to_llvm_ir_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 4,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    )
    .expect("IR emission should succeed")
}

fn assert_near(a: f32, b: f32, eps: f32) {
    let delta = (a - b).abs();
    assert!(delta <= eps, "expected {a} ~= {b}, delta={delta}");
}

fn rms_after_skip(samples: &[f32], skip: usize) -> f32 {
    let tail = if skip < samples.len() {
        &samples[skip..]
    } else {
        samples
    };
    let energy = tail.iter().map(|sample| sample * sample).sum::<f32>();
    (energy / tail.len().max(1) as f32).sqrt()
}

fn max_abs(samples: &[f32]) -> f32 {
    samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
}

fn assert_non_silent(samples: &[f32], context: &str) {
    let peak = max_abs(samples);
    let rms = rms_after_skip(samples, 0);
    assert!(
        peak > 1e-3 && rms > 1e-4,
        "expected non-silent output for {context}, peak={peak}, rms={rms}"
    );
}

fn state_type_of(typed: &onda_semantics::TypedProgram, name: &str) -> Option<PrimitiveType> {
    typed
        .state_vars
        .iter()
        .zip(typed.state_types.iter())
        .find_map(|(n, ty)| if n == name { Some(*ty) } else { None })
}

fn set_param_f32(instance: &mut onda_runtime::Instance, name: &str, value: f32) {
    let idx = instance
        .param_index(name)
        .unwrap_or_else(|| panic!("missing parameter '{name}'"));
    let bytes = value.to_ne_bytes();
    set_param_by_index(instance, idx, &bytes).expect("param update should succeed");
}

fn set_param_f32_array(instance: &mut onda_runtime::Instance, name: &str, values: &[f32]) {
    let idx = instance
        .param_index(name)
        .unwrap_or_else(|| panic!("missing parameter '{name}'"));
    let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
    for v in values {
        bytes.extend_from_slice(&v.to_ne_bytes());
    }
    set_param_by_index(instance, idx, &bytes).expect("array param update should succeed");
}

fn encode_planar_f32(channels: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn encode_planar_f64(channels: &[Vec<f64>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn encode_planar_i64(channels: &[Vec<i64>]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in channels {
        for sample in ch {
            out.extend_from_slice(&sample.to_ne_bytes());
        }
    }
    out
}

fn decode_planar_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk");
            f32::from_ne_bytes(arr)
        })
        .collect()
}

fn decode_planar_f64(bytes: &[u8]) -> Vec<f64> {
    bytes
        .chunks_exact(std::mem::size_of::<f64>())
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().expect("chunk");
            f64::from_ne_bytes(arr)
        })
        .collect()
}

fn decode_planar_i64(bytes: &[u8]) -> Vec<i64> {
    bytes
        .chunks_exact(std::mem::size_of::<i64>())
        .map(|chunk| {
            let arr: [u8; 8] = chunk.try_into().expect("chunk");
            i64::from_ne_bytes(arr)
        })
        .collect()
}

fn read_wav_mixdown_f32(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("wav should open");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    assert!(channels > 0, "wav must contain at least one channel");

    let interleaved = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .expect("float wav samples"),
        (hound::SampleFormat::Int, 8) => reader
            .samples::<i8>()
            .map(|s| s.map(|v| v as f32 / i8::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int8 wav samples"),
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int16 wav samples"),
        (hound::SampleFormat::Int, 24) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / 8_388_608.0))
            .collect::<Result<Vec<_>, _>>()
            .expect("int24 wav samples"),
        (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int32 wav samples"),
        _ => panic!(
            "unsupported wav format: {:?} {} bits",
            spec.sample_format, spec.bits_per_sample
        ),
    };

    if channels == 1 {
        return interleaved;
    }

    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct FlatIoDesc {
    elem_ty: PrimitiveType,
    array_len: usize,
    offset: usize,
    elem_bytes: usize,
    entry_bytes: usize,
}

fn process_interleaved(
    instance: &mut onda_runtime::Instance,
    in_interleaved: &[f32],
    out_interleaved: &mut [f32],
    frames: usize,
) -> Result<(), Diagnostic> {
    let in_descs = collect_flat_io_descs(
        instance.input_count(),
        |idx| instance.input_type(idx),
        |idx| instance.input_type_bytes(idx),
    )?;
    let out_descs = collect_flat_io_descs(
        instance.output_count(),
        |idx| instance.output_type(idx),
        |idx| instance.output_type_bytes(idx),
    )?;
    let in_channels: usize = in_descs.iter().map(|d| d.array_len).sum();
    let out_channels: usize = out_descs.iter().map(|d| d.array_len).sum();

    let expected_in = frames.saturating_mul(in_channels);
    if in_interleaved.len() < expected_in {
        return Err(Diagnostic::runtime(
            "input buffer too small for requested frame count",
            0,
            0,
        ));
    }
    let expected_out = frames.saturating_mul(out_channels);
    if out_interleaved.len() < expected_out {
        return Err(Diagnostic::runtime(
            "output buffer too small for requested frame count",
            0,
            0,
        ));
    }

    let mut in_buffers = Vec::with_capacity(in_descs.len());
    for (idx, desc) in in_descs.iter().copied().enumerate() {
        let mut bytes = vec![0_u8; desc.entry_bytes.saturating_mul(frames)];
        for ch in 0..desc.array_len {
            let in_channel = desc.offset + ch;
            for frame in 0..frames {
                let sample = in_interleaved[frame * in_channels + in_channel];
                let byte_idx = (ch * frames + frame) * desc.elem_bytes;
                encode_f32_as_primitive(
                    desc.elem_ty,
                    sample,
                    &mut bytes[byte_idx..byte_idx + desc.elem_bytes],
                )?;
            }
        }
        bind_input(instance, idx, bytes.as_ptr(), bytes.len())?;
        in_buffers.push(bytes);
    }

    let mut out_buffers = Vec::with_capacity(out_descs.len());
    for (idx, desc) in out_descs.iter().copied().enumerate() {
        let mut bytes = vec![0_u8; desc.entry_bytes.saturating_mul(frames)];
        bind_output(instance, idx, bytes.as_mut_ptr(), bytes.len())?;
        out_buffers.push(bytes);
    }

    process_bound(instance, frames)?;

    for (idx, desc) in out_descs.iter().copied().enumerate() {
        let bytes = &out_buffers[idx];
        for ch in 0..desc.array_len {
            let out_channel = desc.offset + ch;
            for frame in 0..frames {
                let byte_idx = (ch * frames + frame) * desc.elem_bytes;
                let sample = decode_primitive_as_f32(
                    desc.elem_ty,
                    &bytes[byte_idx..byte_idx + desc.elem_bytes],
                )?;
                out_interleaved[frame * out_channels + out_channel] = sample;
            }
        }
    }
    Ok(())
}

fn benchmark_process_runtime(
    instance: &mut onda_runtime::Instance,
    in_interleaved: &[f32],
    out_interleaved: &mut [f32],
    frames: usize,
    warmup_iters: usize,
    timed_iters: usize,
) -> f64 {
    for _ in 0..warmup_iters {
        process_interleaved(instance, in_interleaved, out_interleaved, frames)
            .expect("warmup processing should succeed");
    }
    let start = Instant::now();
    for _ in 0..timed_iters {
        process_interleaved(instance, in_interleaved, out_interleaved, frames)
            .expect("timed processing should succeed");
    }
    std::hint::black_box(out_interleaved.first().copied().unwrap_or(0.0));
    start.elapsed().as_secs_f64()
}

fn estimate_positive_zero_cross_frequency(samples: &[f32], sample_rate: f32) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count() as f32;
    crossings * sample_rate / samples.len() as f32
}

fn collect_flat_io_descs<TypeFn, BytesFn>(
    count: usize,
    mut type_of: TypeFn,
    mut bytes_of: BytesFn,
) -> Result<Vec<FlatIoDesc>, Diagnostic>
where
    TypeFn: FnMut(usize) -> Option<String>,
    BytesFn: FnMut(usize) -> Option<usize>,
{
    let mut out = Vec::with_capacity(count);
    let mut offset = 0usize;
    for idx in 0..count {
        let ty_text = type_of(idx).unwrap_or_else(|| "f32".to_owned());
        let (elem_ty, array_len) = parse_declared_type(&ty_text)?;
        let elem_bytes = primitive_type_bytes_local(elem_ty);
        let entry_bytes = bytes_of(idx).unwrap_or_else(|| elem_bytes.saturating_mul(array_len));
        out.push(FlatIoDesc {
            elem_ty,
            array_len,
            offset,
            elem_bytes,
            entry_bytes,
        });
        offset = offset.saturating_add(array_len);
    }
    Ok(out)
}

fn parse_declared_type(text: &str) -> Result<(PrimitiveType, usize), Diagnostic> {
    if let Some(bracket) = text.find('[') {
        if !text.ends_with(']') {
            return Err(Diagnostic::runtime("invalid declared type text", 0, 0));
        }
        let elem = &text[..bracket];
        let len_text = &text[bracket + 1..text.len() - 1];
        let len = len_text
            .parse::<usize>()
            .map_err(|_| Diagnostic::runtime("invalid declared array length", 0, 0))?;
        let ty = primitive_type_from_text(elem)?;
        Ok((ty, len.max(1)))
    } else {
        Ok((primitive_type_from_text(text)?, 1))
    }
}

fn primitive_type_from_text(text: &str) -> Result<PrimitiveType, Diagnostic> {
    match text {
        "f32" => Ok(PrimitiveType::F32),
        "f64" => Ok(PrimitiveType::F64),
        "i32" => Ok(PrimitiveType::I32),
        "i64" => Ok(PrimitiveType::I64),
        "bool" => Ok(PrimitiveType::Bool),
        _ => Err(Diagnostic::runtime(
            "unsupported declared primitive type",
            0,
            0,
        )),
    }
}

fn primitive_type_bytes_local(ty: PrimitiveType) -> usize {
    match ty {
        PrimitiveType::F32 | PrimitiveType::I32 => 4,
        PrimitiveType::F64 | PrimitiveType::I64 => 8,
        PrimitiveType::Bool => 1,
    }
}

fn encode_f32_as_primitive(
    ty: PrimitiveType,
    value: f32,
    dst: &mut [u8],
) -> Result<(), Diagnostic> {
    match ty {
        PrimitiveType::F32 => {
            let out: &mut [u8; 4] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f32 destination width", 0, 0))?;
            out.copy_from_slice(&value.to_ne_bytes());
            Ok(())
        }
        PrimitiveType::F64 => {
            let out: &mut [u8; 8] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f64 destination width", 0, 0))?;
            out.copy_from_slice(&(value as f64).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::I32 => {
            let out: &mut [u8; 4] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i32 destination width", 0, 0))?;
            out.copy_from_slice(&(value as i32).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::I64 => {
            let out: &mut [u8; 8] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i64 destination width", 0, 0))?;
            out.copy_from_slice(&(value as i64).to_ne_bytes());
            Ok(())
        }
        PrimitiveType::Bool => {
            let out: &mut [u8; 1] = dst
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid bool destination width", 0, 0))?;
            out[0] = if value == 0.0 { 0 } else { 1 };
            Ok(())
        }
    }
}

fn decode_primitive_as_f32(ty: PrimitiveType, src: &[u8]) -> Result<f32, Diagnostic> {
    match ty {
        PrimitiveType::F32 => {
            let arr: [u8; 4] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f32 source width", 0, 0))?;
            Ok(f32::from_ne_bytes(arr))
        }
        PrimitiveType::F64 => {
            let arr: [u8; 8] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid f64 source width", 0, 0))?;
            Ok(f64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I32 => {
            let arr: [u8; 4] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i32 source width", 0, 0))?;
            Ok(i32::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::I64 => {
            let arr: [u8; 8] = src
                .try_into()
                .map_err(|_| Diagnostic::runtime("invalid i64 source width", 0, 0))?;
            Ok(i64::from_ne_bytes(arr) as f32)
        }
        PrimitiveType::Bool => {
            let b = *src
                .first()
                .ok_or_else(|| Diagnostic::runtime("invalid bool source width", 0, 0))?;
            Ok(if b == 0 { 0.0 } else { 1.0 })
        }
    }
}

fn mk_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("onda_examples_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[path = "examples_suite/analysis_and_stdlib.rs"]
mod analysis_and_stdlib;
#[path = "examples_suite/execution_and_runtime.rs"]
mod execution_and_runtime;
#[path = "examples_suite/generic_defs.rs"]
mod generic_defs;
#[path = "examples_suite/language_core.rs"]
mod language_core;
#[path = "examples_suite/proc_local_defs.rs"]
mod proc_local_defs;
#[path = "examples_suite/slices_and_ports.rs"]
mod slices_and_ports;
#[path = "examples_suite/tuples.rs"]
mod tuples;
