use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use omni_codegen_llvm::{CompileOptions, ExecutionBackend};
use omni_frontend::{parse_program, parse_program_file, Diagnostic, PrimitiveType};
use omni_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_bound, process_unchecked,
    reset_instance_state, set_param_by_index, trigger_event_by_index, validate_bindings,
    validate_buffers, validate_outputs, InstanceConfig,
};
use omni_semantics::{analyze, analyze_with_options, AnalysisOptions};

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
  voices.a[1] = 1.0
  voices.b[1] = 2.0
}
sample {
  out1 = sum_voice(voices[1])
}
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
    include_str!("../../../examples/multitap_feedback_struct_data.omni");
const PROC_GAIN_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/proc_gain_graph.omni");
const PROC_SPLIT_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/proc_split_graph.omni");
const PROC_ARRAY_STEREO_SINE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/proc_array_stereo_sine_graph.omni");
const FEEDBACK_SATURATOR_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/feedback_saturator_graph.omni");
const STD_ONE_POLE_FILE_EXAMPLE: &str = include_str!("../../../examples/std_one_pole.omni");
const STD_ONE_POLE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../examples/std_one_pole_graph.omni");
const STDLIB_F32_FILE_EXAMPLE: &str = include_str!("../../../examples/stdlib_f32.omni");
const STDLIB_F32_GRAPH_FILE_EXAMPLE: &str = include_str!("../../../examples/stdlib_f32_graph.omni");

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
  a = [0, i64(1)]
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
events {
  ping() {
    lfo = 1.0
  }
}
sample {
  lfo: f32 = 0.0
  out1 = lfo
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

fn compile_instance(src: &str, frames: usize) -> (omni_runtime::Instance, usize, usize) {
    compile_instance_with_options(
        src,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    )
}

fn compile_instance_file(path: &str, frames: usize) -> (omni_runtime::Instance, usize, usize) {
    compile_instance_file_with_options(
        path,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    )
}

fn compile_instance_with_options(
    src: &str,
    frames: usize,
    options: CompileOptions,
) -> (omni_runtime::Instance, usize, usize) {
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
    let jit = omni_codegen_llvm::lower_and_jit_with_options(typed, options)
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
) -> (omni_runtime::Instance, usize, usize) {
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
    let jit = omni_codegen_llvm::lower_and_jit_with_options(typed, options)
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

fn state_type_of(typed: &omni_semantics::TypedProgram, name: &str) -> Option<PrimitiveType> {
    typed
        .state_vars
        .iter()
        .zip(typed.state_types.iter())
        .find_map(|(n, ty)| if n == name { Some(*ty) } else { None })
}

fn set_param_f32(instance: &mut omni_runtime::Instance, name: &str, value: f32) {
    let idx = instance
        .param_index(name)
        .unwrap_or_else(|| panic!("missing parameter '{name}'"));
    let bytes = value.to_ne_bytes();
    set_param_by_index(instance, idx, &bytes).expect("param update should succeed");
}

fn set_param_f32_array(instance: &mut omni_runtime::Instance, name: &str, values: &[f32]) {
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

fn read_wav_mono_f32(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("wav should open");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "expected mono wav");

    match (spec.sample_format, spec.bits_per_sample) {
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
        (hound::SampleFormat::Int, 24) | (hound::SampleFormat::Int, 32) => reader
            .samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<Result<Vec<_>, _>>()
            .expect("int24/int32 wav samples"),
        _ => panic!(
            "unsupported wav format: {:?} {} bits",
            spec.sample_format, spec.bits_per_sample
        ),
    }
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
    instance: &mut omni_runtime::Instance,
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
    instance: &mut omni_runtime::Instance,
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
    let dir = std::env::temp_dir().join(format!("omni_examples_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn gain_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(GAIN, frames);
    set_param_f32(&mut instance, "gain", 0.5);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32).collect();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, out) in output.iter().enumerate() {
        assert_near(*out, input[idx] * 0.5, 1e-6);
    }
}

#[test]
fn events_metadata_and_scalar_dispatch_work() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.event_count(), 1);
    assert_eq!(instance.event_name(0), Some("set_amp"));
    assert_eq!(instance.event_index("set_amp"), Some(0));
    assert_eq!(instance.event_payload_bytes(0), Some(4));

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let payload = 0.75_f32.to_ne_bytes();
    trigger_event_by_index(&mut instance, 0, &payload).expect("event trigger should succeed");
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn reset_instance_state_restores_initial_runtime_state() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let payload = 0.5_f32.to_ne_bytes();
    trigger_event_by_index(&mut instance, 0, &payload).expect("event trigger should succeed");
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }

    reset_instance_state(&mut instance);
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn event_array_payload_dispatch_and_unknown_index_ignore() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_ARRAY_UPDATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.event_count(), 1);
    assert_eq!(instance.event_payload_bytes(0), Some(8));

    trigger_event_by_index(&mut instance, 99, &[1, 2, 3])
        .expect("unknown event index should be ignored");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    let mut payload = Vec::new();
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    payload.extend_from_slice(&0.75_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, 0, &payload).expect("event trigger should succeed");
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn event_handler_local_array_literal_declaration_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_LOCAL_ARRAY_LITERAL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    trigger_event_by_index(&mut instance, 0, &[]).expect("event trigger should succeed");
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn event_payload_mismatch_returns_runtime_error() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(EVENT_SCALAR_UPDATE_EXAMPLE, frames);
    let err = trigger_event_by_index(&mut instance, 0, &[])
        .expect_err("payload mismatch should return runtime error");
    assert!(
        err.message.contains("expects"),
        "expected payload-size error, got '{}'",
        err.message
    );
}

#[test]
fn proc_event_forwarding_from_top_level_event_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn proc_event_call_from_top_level_init_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_CALL_FROM_TOP_LEVEL_INIT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.55, 1e-6);
    }
}

#[test]
fn proc_event_call_from_top_level_sample_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.35, 1e-6);
    }
}

#[test]
fn proc_event_dynamic_proc_array_call_from_top_level_sample_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_ARRAY_DYNAMIC_CALL_FROM_TOP_LEVEL_SAMPLE_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.42, 1e-6);
    }
}

#[test]
fn proc_array_indexed_event_forwarding_from_top_level_event_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn proc_array_alias_event_forwarding_from_top_level_event_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_ARRAY_ALIAS_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.66_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.66, 1e-6);
    }
}

#[test]
fn proc_event_call_from_parent_proc_init_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_PARENT_INIT_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.62, 1e-6);
    }
}

#[test]
fn proc_event_call_from_parent_proc_block_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_PROC_PARENT_BLOCK_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.73, 1e-6);
    }
}

#[test]
fn proc_event_call_from_parent_proc_event_via_top_level_sample_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_PARENT_EVENT_CALLED_FROM_TOP_LEVEL_SAMPLE_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.81, 1e-6);
    }
}

#[test]
fn nested_proc_array_indexed_event_forwarding_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_NESTED_PROC_ARRAY_INDEXED_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.7_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.7, 1e-6);
    }
}

#[test]
fn deep_nested_proc_array_dynamic_index_event_forwarding_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(EVENT_DEEP_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.65_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.65, 1e-6);
    }
}

#[test]
fn deeper_nested_proc_array_dynamic_index_event_forwarding_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_DEEPER_NESTED_PROC_ARRAY_DYNAMIC_FORWARD_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.6_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn proc_array_alias_event_forwarding_from_parent_proc_event_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        EVENT_PROC_ARRAY_ALIAS_FORWARD_FROM_PARENT_PROC_EVENT_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let idx = instance
        .event_index("note_on")
        .expect("top-level forwarding event must exist");
    trigger_event_by_index(&mut instance, idx, &0.68_f32.to_ne_bytes())
        .expect("forwarding event trigger should succeed");
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.68, 1e-6);
    }
}

#[test]
fn events_reject_forbidden_writes_and_immutability() {
    let parsed = parse_program(EVENT_WRITE_OUTPUT_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("events should reject output writes");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("cannot assign to output symbol")),
        "expected output-write error, got {:?}",
        errs
    );

    let parsed =
        parse_program(EVENT_WRITE_NON_INIT_STATE_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("events should reject non-init-root writes");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("init-root state") && d.message.contains("lfo")),
        "expected init-root write restriction error, got {:?}",
        errs
    );

    let parsed = parse_program(EVENT_PARAM_IMMUTABLE_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("events should reject param mutation");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("immutable event array parameter")),
        "expected immutable event param error, got {:?}",
        errs
    );
}

#[test]
fn proc_events_reject_expression_position_and_owning_self_calls() {
    let parsed =
        parse_program(PROC_EVENT_EXPRESSION_POSITION_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("proc event expression use should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("statement-only") && d.message.contains("voice.note_on")),
        "expected statement-only proc event error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_EVENT_OWNING_SELF_CALL_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("owning self proc event call should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("cannot call event 'Voice.note_on' on the owning proc instance")
        }),
        "expected owning-proc self-call error, got {:?}",
        errs
    );
}

#[test]
fn proc_events_reject_unknown_targets_and_bad_argument_shapes() {
    let parsed = parse_program(PROC_EVENT_UNKNOWN_IN_PARENT_INIT_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let errs = analyze(parsed).expect_err("unknown proc event target should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("unknown processor event 'voice.not_real'; expected one of [note_on]")
        }),
        "expected unknown proc event error, got {:?}",
        errs
    );

    let parsed = parse_program(PROC_EVENT_MISSING_ARG_IN_TOP_LEVEL_SAMPLE_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let errs = analyze(parsed).expect_err("missing proc event argument should fail");
    assert!(
        errs.iter().any(|d| {
            d.message.contains(
                "processor event call 'voice.note_on(...)' is missing required argument 'value'",
            )
        }),
        "expected missing proc event argument error, got {:?}",
        errs
    );
}

#[test]
fn proc_event_slice_params_accept_internal_array_sources() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_SLICE_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_SLICE_FROM_PARENT_PROC_STATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn slices_lower_to_array_views_for_direct_calls_and_local_aliases() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_EVENT_DIRECT_SLICE_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 52.0, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(LOCAL_SLICE_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 33.0, 1e-6);
}

#[test]
fn slice_assignments_fill_copy_and_preserve_overlap_semantics() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_FILL_ASSIGN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 7.5, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_COPY_ASSIGN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 6.0, 1e-6);

    let (mut instance, in_channels, out_channels) =
        compile_instance(SLICE_OVERLAP_COPY_ASSIGN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 10.0, 1e-6);
}

#[test]
fn proc_event_fixed_array_params_accept_internal_array_sources() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_EVENT_FIXED_ARRAY_FROM_TOP_LEVEL_INIT_STATE_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
}

#[test]
fn top_level_events_accept_slice_payloads() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_SLICE_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(2_i32).to_ne_bytes());
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    payload.extend_from_slice(&0.75_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("slice event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 2.25, 1e-6);
}

#[test]
fn top_level_events_forward_slice_payloads_to_proc_events() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_SLICE_PROC_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(2_i32).to_ne_bytes());
    payload.extend_from_slice(&0.5_f32.to_ne_bytes());
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("slice forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 2.75, 1e-6);
}

#[test]
fn top_level_slice_event_truncated_payload_returns_runtime_error() {
    let frames = 1;
    let (mut instance, _, _) = compile_instance(TOP_LEVEL_EVENT_SLICE_PARAM_EXAMPLE, frames);
    let event_idx = instance.event_index("load").expect("load event must exist");

    let mut payload = Vec::new();
    payload.extend_from_slice(&(3_i32).to_ne_bytes());
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    payload.extend_from_slice(&0.75_f32.to_ne_bytes());
    let err = trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect_err("truncated slice payload should fail");
    assert!(
        err.message.contains("payload"),
        "expected payload-related runtime error, got {:?}",
        err
    );
}

#[test]
fn top_level_events_forward_fixed_array_payloads_to_proc_events() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_FIXED_ARRAY_PROC_FORWARD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), Some(16));

    let mut payload = Vec::new();
    payload.extend_from_slice(&0.1_f32.to_ne_bytes());
    payload.extend_from_slice(&0.2_f32.to_ne_bytes());
    payload.extend_from_slice(&0.3_f32.to_ne_bytes());
    payload.extend_from_slice(&0.4_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("fixed-array forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
}

#[test]
fn top_level_events_accept_mixed_fixed_and_slice_payloads() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_EVENT_MIXED_FIXED_AND_SLICE_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload = Vec::new();
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    payload.extend_from_slice(&0.75_f32.to_ne_bytes());
    payload.extend_from_slice(&(2_i32).to_ne_bytes());
    payload.extend_from_slice(&1.5_f32.to_ne_bytes());
    payload.extend_from_slice(&2.5_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("mixed fixed/slice event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 4.5, 1e-6);
}

#[test]
fn top_level_events_forward_large_fixed_array_payloads_to_proc_events() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        TOP_LEVEL_EVENT_LARGE_FIXED_ARRAY_PROC_FORWARD_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), Some(96000 * 4));

    let mut payload = vec![0_u8; 96000 * 4];
    payload[0..4].copy_from_slice(&0.25_f32.to_ne_bytes());
    payload[(96000 - 1) * 4..96000 * 4].copy_from_slice(&0.75_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("large fixed-array forwarding event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
}

#[test]
fn generic_proc_events_accept_generic_slice_params() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EVENT_SLICE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 2.25, 1e-6);
}

#[test]
fn generic_proc_events_accept_generic_slice_and_scalar_params() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EVENT_SLICE_WITH_SCALAR_PARAMS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 2.0, 1e-6);
}

#[test]
fn procs_reject_direct_self_recursive_instantiation() {
    let parsed =
        parse_program(PROC_SELF_RECURSIVE_INSTANCE_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("direct self-recursive proc state should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("processor 'Voice' cannot instantiate itself as state symbol 'other'")
        }),
        "expected direct self-recursive proc-instance error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_SELF_RECURSIVE_ARRAY_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("direct self-recursive proc arrays should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("processor 'Voice' cannot instantiate itself as processor array 'voices'")
        }),
        "expected direct self-recursive proc-array error, got {:?}",
        errs
    );
}

#[test]
fn events_reject_duplicate_and_conflicting_names() {
    let errs = parse_program(EVENT_DUPLICATE_NAME_ERROR_EXAMPLE)
        .expect_err("duplicate top-level events should fail at parse");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("duplicate event declaration 'ping'")),
        "expected duplicate top-level event error, got {:?}",
        errs
    );

    let errs = parse_program(PROC_EVENT_DUPLICATE_NAME_ERROR_EXAMPLE)
        .expect_err("duplicate proc events should fail at parse");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("duplicate event declaration 'note_on'")),
        "expected duplicate proc event error, got {:?}",
        errs
    );

    let parsed =
        parse_program(PROC_EVENT_NAME_CONFLICT_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("conflicting proc event names should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("event name conflicts with an existing callable/endpoint name")
        }),
        "expected proc event name conflict error, got {:?}",
        errs
    );
}

#[test]
fn io_and_param_count_shorthand_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(COUNT_SHORTHAND_IO_PARAMS_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 2);

    set_param_f32(&mut instance, "param1", 0.5);
    set_param_f32(&mut instance, "param2", -1.0);

    let input = vec![
        1.0_f32, 10.0_f32, //
        2.0_f32, 20.0_f32, //
        3.0_f32, 30.0_f32, //
        4.0_f32, 40.0_f32,
    ];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = vec![
        1.5_f32, 9.0_f32, //
        2.5_f32, 19.0_f32, //
        3.5_f32, 29.0_f32, //
        4.5_f32, 39.0_f32,
    ];
    for (actual, target) in output.iter().zip(expected.iter()) {
        assert_near(*actual, *target, 1e-6);
    }
}

#[test]
fn sine_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(SINE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let phase_step = 440.0_f32 * 6.2831855_f32 / 48_000.0_f32;
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32 * phase_step).sin();
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn one_pole_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(ONE_POLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.1, 1e-6);
    assert!(output[frames - 1] > output[0]);
    assert!(output[frames - 1] < 1.0);
}

#[test]
fn if_statement_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(IF_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.25, 1e-6);
    }

    set_param_f32(&mut instance, "gate", 0.0);
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, -0.25, 1e-6);
    }
}

#[test]
fn for_loop_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(FOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn for_loop_accepts_variable_bound() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(FOR_VAR_BOUND_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn for_loop_accepts_parenthesized_expression_bound() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(FOR_PAREN_EXPR_BOUND_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn for_loop_supports_descending_step_and_inclusive_end() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(FOR_DESCENDING_STEP_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn loop_sugar_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(LOOP_SUGAR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn loop_sugar_accepts_variable_bound() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(LOOP_VAR_BOUND_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn init_control_flow_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_CONTROL_FLOW_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.6, 1e-6);
    }
}

#[test]
fn block_nested_branch_state_registration_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_BRANCH_STATE_REGISTRATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn sample_nested_branch_typed_registration_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_BRANCH_TYPED_REGISTRATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn block_loop_control_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_LOOP_CONTROL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn sample_loop_control_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_LOOP_CONTROL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]
fn block_break_outside_loop_is_rejected() {
    let parsed =
        parse_program(BLOCK_BREAK_OUTSIDE_LOOP_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("block break outside loop should fail");
    assert!(
        errs.iter().any(|d| d
            .message
            .contains("break is only allowed inside for/while/loop bodies")),
        "expected block break diagnostic, got {:?}",
        errs
    );
}

#[test]
fn sample_continue_outside_loop_is_rejected() {
    let parsed =
        parse_program(SAMPLE_CONTINUE_OUTSIDE_LOOP_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("sample continue outside loop should fail");
    assert!(
        errs.iter().any(|d| {
            d.message
                .contains("continue is only allowed inside for/while/loop bodies")
        }),
        "expected sample continue diagnostic, got {:?}",
        errs
    );
}

#[test]
fn def_call_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(DEF_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_monomorphizes_scalar_numeric_calls() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_MONO_NUMERIC_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]
fn def_named_default_args_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_NAMED_DEFAULT_ARGS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.25, 1e-6);
    }
}

#[test]
fn def_overload_by_arity_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_ARITY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]
fn def_overload_exact_typed_beats_untyped() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_TYPED_BEATS_UNTYPED_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 20.0, 1e-6);
    }
}

#[test]
fn def_overload_widening_fallback_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_WIDENING_FALLBACK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn def_overload_i32_numeric_tie_is_ambiguous() {
    let parsed =
        parse_program(DEF_OVERLOAD_I32_AMBIGUOUS_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject ambiguous i32 overload tie (i64 vs f64 widening)"
    );
}

#[test]
fn def_overload_defaults_participate_in_resolution() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_DEFAULTS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn def_overload_defaults_can_be_ambiguous() {
    let parsed =
        parse_program(DEF_OVERLOAD_DEFAULTS_AMBIGUOUS_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject ambiguous overloads when defaults produce equivalent matches"
    );
}

#[test]
fn def_overload_supports_struct_and_scalar_variants() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_OVERLOAD_STRUCT_AND_SCALAR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]
fn def_overload_supports_buffer_and_scalar_variants() {
    let parsed =
        parse_program(DEF_OVERLOAD_BUFFER_AND_SCALAR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should accept overloads that differ by buffer vs scalar parameter types"
    );
}

#[test]
fn struct_methods_support_overloading() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_METHOD_OVERLOAD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn positional_after_named_is_rejected() {
    let parsed =
        parse_program(DEF_POSITIONAL_AFTER_NAMED_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject positional args after named args"
    );
}

#[test]
fn def_cannot_capture_top_level_symbols() {
    let parsed = parse_program(DEF_CANNOT_CAPTURE_TOP_LEVEL_SYMBOLS_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject top-level ins/params/state/buffers referenced in def scope"
    );
}

#[test]
fn def_without_return_defaults_to_zero() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(DEF_NO_RETURN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn def_return_exits_early() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_EARLY_RETURN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_struct_argument_is_passed_by_ref_with_writeback() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARG_BY_REF_WRITEBACK_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 4.0, 1e-6);
}
#[test]
fn def_struct_arg_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_struct_data_arg_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_DATA_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_struct_array_indexed_arg_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INDEXED_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn def_structural_arg_compiles_with_multiple_matching_structs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCTURAL_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_array_arg_is_passed_by_ref_with_writeback() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_ARRAY_ARG_BY_REF_WRITE_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 4.0, 1e-6);
}

#[test]
fn def_array_arg_writeback_propagates_through_nested_def_calls() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_ARRAY_ARG_FORWARDING_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 6.0, 1e-6);
}

#[test]
fn def_accepts_local_sample_array_arguments() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_LOCAL_ARRAY_ARG_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 2.0, 1e-6);
}

#[test]
fn def_explicit_struct_annotation_is_nominal() {
    let parsed =
        parse_program(DEF_EXPLICIT_STRUCT_ARG_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject passing non-matching struct to explicitly typed def parameter"
    );
}

#[test]
fn struct_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn struct_named_default_ctor_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_NAMED_DEFAULT_CTOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn namespaced_struct_ctor_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_STRUCT_CTOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn namespaced_def_resolution_uses_parent_then_global() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_DEF_RESOLUTION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 112.0, 1e-6);
    }
}

#[test]
fn top_level_must_use_fully_qualified_namespaced_call() {
    let parsed = parse_program(NAMESPACE_TOP_LEVEL_UNQUALIFIED_CALL_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject unqualified call to namespaced function at top level"
    );
}

#[test]
fn import_and_include_resolve_transitively_from_entry_file() {
    let dir = mk_temp_dir("import_include");
    let main = dir.join("main.omni");
    let filter = dir.join("filter.omni");
    let shared = dir.join("shared.omni");

    fs::write(
        &shared,
        r#"
def shared(x) {
  return x * 2.0
}
"#,
    )
    .expect("write shared");

    fs::write(
        &filter,
        r#"
include "./shared.omni"
namespace DSP:
  struct S:
    x: f32 = 1.0
  def run(v):
    return shared(v) + 1.0
"#,
    )
    .expect("write filter");

    fs::write(
        &main,
        r#"
import filter
outs { out1 }
init {
  s = DSP::S()
}
sample {
  out1 = DSP::run(2.0) + s.x
}
"#,
    )
    .expect("write main");

    let parsed = parse_program_file(&main).expect("parse program file");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("semantic analysis");
    let jit = omni_codegen_llvm::lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 64,
            fast_math: false,
        },
    )
    .expect("jit lowering");
    let mut instance = create_instance(
        jit,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 64,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .expect("instance");
    let mut output = vec![0.0_f32; 64];
    process_interleaved(&mut instance, &[], &mut output, 64).expect("process");
    assert_near(output[0], 6.0, 1e-6);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn builtin_std_import_resolves_without_local_std_path() {
    let dir = mk_temp_dir("builtin_std_import");
    let main = dir.join("main.omni");
    let shadow_std_dir = dir.join("std");
    fs::create_dir_all(&shadow_std_dir).expect("create local std dir");
    fs::write(
        shadow_std_dir.join("osc.omni"),
        r#"
def broken() {
  return unknown_symbol
}
"#,
    )
    .expect("write local shadow std file");
    fs::write(
        &main,
        r#"
import std/osc
outs { out1 }
init { o = std::osc::Sine(freq = 220.0) }
sample { out1 = o() }
"#,
    )
    .expect("write main");

    let parsed = parse_program_file(&main).expect("parse program file");
    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("semantic analysis");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn struct_initialization_in_sample_is_rejected() {
    let parsed = parse_program(STRUCT_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject struct ctor in sample"
    );
}

#[test]
fn data_read_write_clamps_and_truncates() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(DATA_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 12.0, 1e-6);
    }
}

#[test]
fn unsafe_data_builtins_read_write_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNSAFE_DATA_BUILTINS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]
fn unsafe_data_builtins_support_struct_field_data() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNSAFE_DATA_BUILTINS_STRUCT_FIELD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn unsafe_data_builtins_support_typed_local_array_in_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNSAFE_DATA_BUILTINS_TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]
fn data_len_returns_data_capacity() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(DATA_LEN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn data_len_supports_struct_data_field_receiver() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_LEN_STRUCT_FIELD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]
fn data_len_rejects_non_data_receiver() {
    let parsed =
        parse_program(DATA_LEN_INVALID_RECEIVER_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject x.len() for scalar x"
    );
}

#[test]
fn data_struct_elements_support_alias_field_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.5;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn struct_field_data_struct_elements_support_alias_field_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 3.0;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn init_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]
fn def_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]
fn def_struct_field_nested_data_struct_elements_support_alias_field_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_STRUCT_FIELD_NESTED_DATA_STRUCT_ELEM_ALIAS_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]
fn data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = 1.0 + ((idx + 1) as f32) * 0.25;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn struct_field_data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.5;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.25;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn struct_field_data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        STRUCT_FIELD_DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.0;
        assert_near(*sample, expected, 1e-6);
    }
}

#[test]
fn primitive_data_local_alias_binding_is_rejected() {
    let parsed = parse_program(DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should allow primitive array indexed reads as scalar copies via 'x = buf[i]'"
    );
}

#[test]
fn primitive_struct_field_data_local_alias_binding_is_rejected() {
    let parsed =
        parse_program(STRUCT_DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should allow primitive struct-array indexed reads as scalar copies via 'x = v.delay[i]'"
    );
}

#[test]
fn init_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]
fn def_struct_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}
#[test]
fn typed_local_array_declaration_in_sample_is_allowed() {
    let parsed = parse_program(DATA_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should allow primitive T[N] declarations in sample"
    );
}

#[test]
fn typed_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn untyped_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn typed_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn untyped_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn typed_local_i32_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_I32_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn typed_local_bool_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_BOOL_DEF_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn typed_local_array_initializer_in_sample_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_INIT_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn top_level_param_array_defaults_and_set_param_slots_work() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    set_param_f32_array(&mut instance, "mix", &[1.5, 0.75]);
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.25, 1e-6);
    }
}

#[test]
fn declared_param_metadata_reports_array_as_single_entry() {
    let frames = 4;
    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);
    assert_eq!(instance.param_count(), 1);
    assert_eq!(instance.param_index("mix"), Some(0));
    assert_eq!(instance.param_name(0), Some("mix"));
    assert_eq!(instance.param_type(0).as_deref(), Some("f32[2]"));
    assert_eq!(instance.param_type_bytes(0), Some(8));
}

#[test]
fn declared_io_metadata_reports_arrays_as_single_entries() {
    let frames = 4;
    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);
    assert_eq!(instance.input_count(), 1);
    assert_eq!(instance.input_name(0), Some("in1"));
    assert_eq!(instance.input_type(0).as_deref(), Some("f32[2]"));
    assert_eq!(instance.input_type_bytes(0), Some(8));
}

#[test]
fn top_level_input_array_reads_indexed_channels() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 1);

    let input = vec![
        1.0_f32, 0.5, //
        2.0, 1.0, //
        -1.0, 2.0, //
        0.25, -0.5,
    ];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    let expected = [2.5_f32, 5.0, 0.0, 0.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn top_level_output_array_writes_indexed_channels() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OUTPUT_ARRAY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 0.25, 1e-6);
        assert_near(output[base + 1], 0.75, 1e-6);
    }
}

#[test]
fn graph_implicitly_steps_proc_nodes_and_fanout_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_IMPLICIT_PROC_FANOUT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 0.5, 1e-6);
        assert_near(output[base + 1], 0.5, 1e-6);
    }
}

#[test]
fn graph_delayed_feedback_persists_across_process_calls() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_DELAY_FEEDBACK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");
    let expected_first = [1.0_f32, 2.0, 3.0, 4.0];
    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");
    let expected_second = [5.0_f32, 6.0, 7.0, 8.0];
    for (sample, target) in second.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_sample_override_for_param_destinations_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PARAM_SAMPLE_OVERRIDE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.1_f32, 0.2, 0.3, 0.4];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_fanout_destinations_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(GRAPH_FANOUT_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, -0.5, 0.0, 2.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    let expected = [0.5_f32, -0.25, 0.0, 1.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_proc_bundle_destinations_run_for_proc_and_proc_array_slot_sources() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_BUNDLE_FANOUT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = [
        0.25_f32, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5,
    ];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_array_expressions_run_element_wise() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_EXPR_EXAMPLE, frames);
    assert_eq!(in_channels, 4);
    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 4.0, 2.0, 8.0, //
        2.0, 5.0, 4.0, 10.0, //
        3.0, 6.0, 6.0, 12.0, //
        4.0, 7.0, 8.0, 14.0,
    ];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        1.0_f32 * 0.5 + 2.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        2.0 * 0.5 + 4.0 * 0.25,
        5.0 * 0.5 + 10.0 * 0.25,
        3.0 * 0.5 + 6.0 * 0.25,
        6.0 * 0.5 + 12.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        7.0 * 0.5 + 14.0 * 0.25,
    ];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_array_delays_persist_and_shift_element_wise() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_DELAY_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 2);

    let first_input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    let mut first_output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &first_input, &mut first_output, frames)
        .expect("first process should succeed");
    let expected_first = [
        0.0_f32, 0.0, //
        1.0, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];
    for (sample, target) in first_output.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let second_input = vec![
        5.0_f32, 50.0, //
        6.0, 60.0, //
        7.0, 70.0, //
        8.0, 80.0,
    ];
    let mut second_output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &second_input, &mut second_output, frames)
        .expect("second process should succeed");
    let expected_second = [
        4.0_f32, 40.0, //
        5.0, 50.0, //
        6.0, 60.0, //
        7.0, 70.0,
    ];
    for (sample, target) in second_output.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_scalar_broadcast_to_array_outputs_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_BROADCAST_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 0.25, 1e-6);
    }
}

#[test]
fn graph_receiver_delay_runs_as_one_sample_delay() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_RECEIVER_DELAY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");
    let expected_first = [0.0_f32, 1.0, 1.0, 1.0];
    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");
    for sample in second {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]
fn graph_slice_sources_route_runtime_channels() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_SLICE_SOURCE_EXAMPLE, frames);
    assert_eq!(in_channels, 4);
    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, 100.0, 1000.0, //
        2.0, 20.0, 200.0, 2000.0, //
        3.0, 30.0, 300.0, 3000.0, //
        4.0, 40.0, 400.0, 4000.0,
    ];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 100.0, //
        20.0, 200.0, //
        30.0, 300.0, //
        40.0, 400.0,
    ];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn proc_local_graphs_compile_and_run_through_top_level_graphs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_LOCAL_GRAPH_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 1.0, //
        20.0, 2.0, //
        30.0, 3.0, //
        40.0, 4.0,
    ];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_scalar_broadcast_to_proc_input_arrays_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_INPUT_ARRAY_BROADCAST_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]
fn graph_scalar_broadcast_to_proc_param_arrays_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_PARAM_ARRAY_BROADCAST_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]
fn graph_proc_named_ports_accept_numbered_aliases() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_NAMED_PORT_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]
fn graph_top_level_named_io_accept_numbered_aliases() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_NAMED_IO_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.25_f32, -0.5, 1.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_top_level_io_is_inferred_from_graph_block() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_IO_INFERENCE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.5_f32, -0.25, 0.0, 1.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    let expected = [0.25_f32, -0.125, 0.0, 0.5];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn graph_proc_io_is_inferred_from_proc_graph_block() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_IO_INFERENCE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]
fn graph_proc_custom_io_names_require_declarations() {
    let parsed = parse_program(GRAPH_PROC_CUSTOM_IO_NAMES_REQUIRE_DECLS_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let errs = analyze(parsed).expect_err("undeclared custom graph proc IO should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("not a declared output"))
            && errs.iter().any(|d| d.message.contains("unknown endpoint")),
        "expected graph undeclared-io diagnostic, got {errs:?}"
    );
}

#[test]
fn proc_gain_graph_example_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_GAIN_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.5_f32, -0.25, 0.0, 1.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [1.5_f32, -0.75, 0.0, 3.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn proc_split_graph_example_routes_both_outputs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SPLIT_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 2);

    let input = vec![0.25_f32, -0.5, 1.0, 0.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        0.25_f32, 0.5, //
        -0.5, -1.0, //
        1.0, 2.0, //
        0.0, 0.0,
    ];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn proc_array_stereo_sine_graph_example_runs_stereo() {
    let frames = 64;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_STEREO_SINE_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let left_abs: f32 = output.iter().step_by(2).map(|x| x.abs()).sum();
    let right_abs: f32 = output.iter().skip(1).step_by(2).map(|x| x.abs()).sum();
    assert!(
        left_abs > 1.0,
        "expected left channel activity, got {left_abs}"
    );
    assert!(
        right_abs > 0.05,
        "expected right channel activity, got {right_abs}"
    );
    assert!(
        left_abs > right_abs * 5.0,
        "expected left channel to dominate due to gain scaling, left={left_abs}, right={right_abs}"
    );
}

#[test]
fn feedback_saturator_graph_example_runs_delayed_feedback() {
    let frames = 6;
    let (mut instance, in_channels, out_channels) =
        compile_instance(FEEDBACK_SATURATOR_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.5, 1e-6);
    assert_near(output[1], 0.24375, 1e-6);
    assert_near(output[2], 0.121150754, 1e-6);
    for window in output.windows(2) {
        assert!(
            window[0] >= window[1] && window[1] >= 0.0,
            "expected positive decaying feedback tail, got {output:?}"
        );
    }
}

#[test]
fn reverb_graph_example_matches_sample_version() {
    let frames = 256;
    let sample_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/reverb_sample.omni"
    );
    let graph_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/reverb_graph.omni"
    );
    let (mut sample_instance, sample_in_channels, sample_out_channels) =
        compile_instance_file(sample_path, frames);
    let (mut graph_instance, graph_in_channels, graph_out_channels) =
        compile_instance_file(graph_path, frames);
    assert_eq!(sample_in_channels, graph_in_channels);
    assert_eq!(sample_out_channels, graph_out_channels);

    let mut sample_output = vec![0.0_f32; frames * sample_out_channels];
    let mut graph_output = vec![0.0_f32; frames * graph_out_channels];
    process_interleaved(&mut sample_instance, &[], &mut sample_output, frames)
        .expect("sample version should run");
    process_interleaved(&mut graph_instance, &[], &mut graph_output, frames)
        .expect("graph version should run");

    for (sample, graph) in sample_output.iter().zip(&graph_output) {
        assert_near(*graph, *sample, 1e-6);
    }
}

#[test]
fn std_one_pole_graph_example_matches_sample_version() {
    let frames = 128;
    let (mut sample_instance, sample_in_channels, sample_out_channels) =
        compile_instance(STD_ONE_POLE_FILE_EXAMPLE, frames);
    let (mut graph_instance, graph_in_channels, graph_out_channels) =
        compile_instance(STD_ONE_POLE_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(sample_in_channels, graph_in_channels);
    assert_eq!(sample_out_channels, graph_out_channels);

    let mut sample_output = vec![0.0_f32; frames * sample_out_channels];
    let mut graph_output = vec![0.0_f32; frames * graph_out_channels];
    process_interleaved(&mut sample_instance, &[], &mut sample_output, frames)
        .expect("sample version should run");
    process_interleaved(&mut graph_instance, &[], &mut graph_output, frames)
        .expect("graph version should run");

    for (sample, graph) in sample_output.iter().zip(&graph_output) {
        assert_near(*graph, *sample, 1e-6);
    }
}

// ─── Tuple tests ──────────────────────────────────────────────────

const TUPLE_RETURN_BASIC: &str = r#"
outs { out1, out2 }

def calcPair(x):
  return (x * 2.0, x + 1.0)

sample {
  (a, b) = calcPair(3.0)
  out1 = a
  out2 = b
}
"#;

#[test]
fn tuple_return_and_destructure_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_RETURN_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6); // 3.0 * 2.0
    assert_near(output[1], 4.0, 1e-6); // 3.0 + 1.0
}

const TUPLE_ELEMENT_ACCESS: &str = r#"
outs { out1 }

def makePair(x):
  return (x, x * 10.0)

def readSecond(x):
  p = makePair(x)
  return p[1]

sample {
  out1 = readSecond(5.0)
}
"#;

#[test]
fn tuple_element_access_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_ELEMENT_ACCESS, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 50.0, 1e-6); // 5.0 * 10.0
}

const TUPLE_LITERAL_ASSIGN: &str = r#"
outs { out1, out2 }

def addPair():
  p = (10.0, 20.0)
  return p[0] + p[1]

sample {
  out1 = addPair()
  (x, y) = (1.0, 2.0)
  out2 = x + y
}
"#;

#[test]
fn tuple_literal_assign_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_LITERAL_ASSIGN, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 30.0, 1e-6); // 10.0 + 20.0
    assert_near(output[1], 3.0, 1e-6);  // 1.0 + 2.0
}

const TUPLE_MIXED_TYPES: &str = r#"
outs { out1, out2 }

def calcIdx(pos):
  pos_floor = floor(pos)
  idx = i32(pos_floor)
  t = pos - pos_floor
  return (idx, t)

sample {
  (idx, t) = calcIdx(3.7)
  out1 = f32(idx)
  out2 = t
}
"#;

#[test]
fn tuple_mixed_types_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_MIXED_TYPES, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 3.0, 1e-6);  // floor(3.7) = 3
    assert_near(output[1], 0.7, 1e-5);  // 3.7 - 3.0 = 0.7
}

const TUPLE_PARAM_BASIC: &str = r#"
outs { out1, out2 }

def sumPair(p: (f32, f32)):
  return p[0] + p[1]

def swapPair(p: (f32, f32)):
  return (p[1], p[0])

sample {
  out1 = sumPair((3.0, 7.0))
  (a, b) = swapPair((10.0, 20.0))
  out2 = a - b
}
"#;

#[test]
fn tuple_param_basic_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_PARAM_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 10.0, 1e-6); // 3.0 + 7.0
    assert_near(output[1], 10.0, 1e-6); // 20.0 - 10.0
}

const TUPLE_PARAM_MIXED_TYPES: &str = r#"
outs { out1, out2 }

def extractPair(p: (i32, f32)):
  return (f32(p[0]) * 2.0, p[1] + 1.0)

sample {
  (a, b) = extractPair((3, 7.5))
  out1 = a
  out2 = b
}
"#;

#[test]
fn tuple_param_mixed_types_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_PARAM_MIXED_TYPES, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6);  // i32(3) * 2.0
    assert_near(output[1], 8.5, 1e-6);  // 7.5 + 1.0
}

const TUPLE_STATE_BASIC: &str = r#"
outs { out1, out2 }

init:
  pair = (10.0, 20.0)

sample {
  out1 = pair[0]
  out2 = pair[1]
}
"#;

#[test]
fn tuple_state_basic_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_STATE_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 10.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
}

const TUPLE_STATE_WRITE: &str = r#"
ins { in1 }
outs { out1, out2 }

init:
  pair = (0.0, 0.0)

sample {
  pair[0] = in1
  pair[1] = in1 * 2.0
  out1 = pair[0]
  out2 = pair[1]
}
"#;

#[test]
fn tuple_state_write_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_STATE_WRITE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 2);

    let input = vec![5.0_f32];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 5.0, 1e-6);
    assert_near(output[1], 10.0, 1e-6);
}

const TUPLE_STATE_PERSISTENCE: &str = r#"
ins { in1 }
outs { out1, out2 }

init:
  pair = (0.0, 0.0)

sample {
  out1 = pair[0]
  out2 = pair[1]
  pair[0] = pair[0] + in1
  pair[1] = pair[1] + 1.0
}
"#;

#[test]
fn tuple_state_persistence_compiles_and_runs() {
    let frames = 3;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_STATE_PERSISTENCE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 2);

    let input = vec![1.0_f32, 2.0, 3.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    // Frame 0: out1=0, out2=0, then pair=(0+1, 0+1) = (1, 1)
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    // Frame 1: out1=1, out2=1, then pair=(1+2, 1+1) = (3, 2)
    assert_near(output[2], 1.0, 1e-6);
    assert_near(output[3], 1.0, 1e-6);
    // Frame 2: out1=3, out2=2, then pair=(3+3, 2+1) = (6, 3)
    assert_near(output[4], 3.0, 1e-6);
    assert_near(output[5], 2.0, 1e-6);
}

#[test]
fn stdlib_f32_graph_example_matches_sample_version() {
    let frames = 256;
    let (mut sample_instance, sample_in_channels, sample_out_channels) =
        compile_instance(STDLIB_F32_FILE_EXAMPLE, frames);
    let (mut graph_instance, graph_in_channels, graph_out_channels) =
        compile_instance(STDLIB_F32_GRAPH_FILE_EXAMPLE, frames);
    assert_eq!(sample_in_channels, graph_in_channels);
    assert_eq!(sample_out_channels, graph_out_channels);

    let mut sample_output = vec![0.0_f32; frames * sample_out_channels];
    let mut graph_output = vec![0.0_f32; frames * graph_out_channels];
    process_interleaved(&mut sample_instance, &[], &mut sample_output, frames)
        .expect("sample version should run");
    process_interleaved(&mut graph_instance, &[], &mut graph_output, frames)
        .expect("graph version should run");

    for (sample, graph) in sample_output.iter().zip(&graph_output) {
        assert_near(*graph, *sample, 1e-5);
    }
}

#[test]
fn graph_nodes_remain_addressable_from_top_level_events() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_EVENT_ROUTING_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert!(output.iter().all(|sample| (*sample).abs() <= 1e-6));

    let idx = instance
        .event_index("set_gain")
        .expect("top-level graph event must exist");
    trigger_event_by_index(&mut instance, idx, &0.75_f32.to_ne_bytes())
        .expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert!(output.iter().all(|sample| (*sample - 0.75).abs() <= 1e-6));
}

#[test]
fn bound_io_writes_directly_for_f32_arrays() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.input_index("in1"), Some(0));
    assert_eq!(instance.output_index("out1"), Some(0));

    let in_bytes = encode_planar_f32(&[vec![1.0, 2.0, -1.0, 0.25], vec![0.5, 1.0, 2.0, -0.5]]);
    bind_input(&mut instance, 0, in_bytes.as_ptr(), in_bytes.len()).expect("bind input");

    let mut bound_out = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, bound_out.as_mut_ptr(), bound_out.len()).expect("bind output");

    process_bound(&mut instance, frames).expect("process bound");

    let copied_bound = decode_planar_f32(&bound_out);
    let expected = [2.5_f32, 5.0, 0.0, 0.0];
    for (sample, target) in copied_bound.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn bound_io_writes_directly_for_f64_declared_types() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_IO_F64_EXAMPLE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.input_type(0).as_deref(), Some("f64[2]"));
    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    let in_bytes = encode_planar_f64(&[vec![1.0, 2.0, 4.0, 0.0], vec![0.5, 2.0, -2.0, 1.0]]);
    bind_input(&mut instance, 0, in_bytes.as_ptr(), in_bytes.len()).expect("bind input");
    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_bound(&mut instance, frames).expect("process bound");

    let out = decode_planar_f64(&out_bytes);
    let expected = [1.25_f64, 3.0, 3.0, 0.5];
    for (sample, target) in out.iter().zip(expected) {
        let delta = (*sample - target).abs();
        assert!(
            delta <= 1e-9,
            "expected {sample} ~= {target}, delta={delta}"
        );
    }
}

#[test]
fn buffer_mono_read_uses_clamped_index_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);
    assert_eq!(instance.buffer_name(0), Some("buf1"));
    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer[f32]"));
    assert_eq!(instance.buffer_index("buf1"), Some(0));

    let mut buf = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn buffer_i32_mono_read_uses_clamped_index_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_I32_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer[i32]"));

    let mut buf = vec![10_i32, 20_i32];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::I32,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn buffer_i64_mono_read_uses_clamped_index_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_I64_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer[i64]"));

    let mut buf = vec![10_i64, 20_i64];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::I64,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn buffer_bool_mono_read_uses_clamped_index_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_BOOL_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer[bool]"));

    let mut buf = vec![1_u8, 0_u8];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::Bool,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = [1.0_f32, 0.0, 0.0, 0.0];
    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn unsafe_builtins_support_mono_buffers() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_UNSAFE_RW_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![1.0_f32, 2.0, 3.0, 4.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 7.0, 1e-6);
    }
    assert_near(buf[1], 7.0, 1e-6);
}

#[test]
fn validate_bindings_and_process_unchecked_work() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    validate_bindings(&mut instance).expect("validate bindings should succeed");
    unsafe {
        process_unchecked(&mut instance).expect("unchecked process should succeed");
    }

    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn validate_bindings_rejects_missing_required_bindings() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let result = validate_bindings(&mut instance);
    assert!(
        result.is_err(),
        "validate_bindings should reject missing required output binding"
    );
}

#[test]
fn validate_domains_allow_partial_revalidation() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf_a = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf_a.as_mut_ptr().cast::<u8>(),
        buf_a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer A");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    validate_buffers(&mut instance).expect("validate buffers should succeed");
    validate_outputs(&mut instance).expect("validate outputs should succeed");
    unsafe {
        process_unchecked(&mut instance).expect("unchecked process should succeed");
    }
    let out_a = decode_planar_f32(&out_bytes);
    let expected_a = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in out_a.iter().zip(expected_a) {
        assert_near(*sample, target, 1e-6);
    }

    let mut buf_b = vec![3.0_f32, 4.0];
    bind_buffer(
        &mut instance,
        0,
        buf_b.as_mut_ptr().cast::<u8>(),
        buf_b.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer B");

    validate_buffers(&mut instance).expect("validate buffers after rebind should succeed");
    unsafe {
        process_unchecked(&mut instance).expect("unchecked process should succeed");
    }
    let out_b = decode_planar_f32(&out_bytes);
    let expected_b = [4.0_f32, 4.0, 4.0, 4.0];
    for (sample, target) in out_b.iter().zip(expected_b) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn buffer_stereo_two_dim_read_and_clamp_work() {
    let frames = 6;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STEREO_2D_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 30.0, 40.0, 40.0, 40.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn buffer_stereo_two_dim_write_works() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STEREO_2D_WRITE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 7.0, 1e-6);
    }
    assert_near(buf[1], 7.0, 1e-6);
}

#[test]
fn buffer_stereo_rejects_one_dim_indexing() {
    let parsed = parse_program(BUFFER_STEREO_1D_INDEX_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject one-dimensional indexing on multichannel buffers"
    );
}

#[test]
fn buffer_static_chans_returns_declared_channel_count() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STATIC_CHANS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 2.0, 1e-6);
    }
}

#[test]
fn buffer_dynamic_chans_returns_runtime_channel_count() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_DYNAMIC_CHANS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, 100.0, //
        2.0, 20.0, 200.0, //
        3.0, 30.0, 300.0, //
        4.0, 40.0, 400.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        3,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]
fn buffer_dynamic_len_returns_runtime_frame_count() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_DYNAMIC_LEN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]
fn def_can_take_mono_buffer_typed_param() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_MONO_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn def_dynamic_buffer_len_returns_runtime_frame_count() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DYNAMIC_LEN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]
fn def_can_take_stereo_buffer_typed_param() {
    let frames = 6;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_STEREO_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 30.0, 40.0, 40.0, 40.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn def_buffer_typed_param_rejects_element_type_mismatch() {
    let parsed =
        parse_program(DEF_BUFFER_PARAM_TYPE_MISMATCH_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject def buffer typed param element type mismatch"
    );
}

#[test]
fn unsafe_builtins_support_top_level_arrays() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(UNSAFE_TOP_LEVEL_ARRAY_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 2.0, 1e-6);
        assert_near(output[base + 1], 3.0, 1e-6);
    }
}

#[test]
fn multitap_feedback_struct_data_example_compiles_and_runs() {
    let frames = 128;
    let (mut instance, in_channels, out_channels) =
        compile_instance(MULTITAP_FEEDBACK_STRUCT_DATA_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert!(
        output.iter().all(|v| v.is_finite()),
        "multitap example output should be finite"
    );
}

#[test]
fn struct_data_field_clamps_and_truncates() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_DATA_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]
fn struct_data_is_per_instance() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_DATA_IS_PER_INSTANCE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn struct_data_field_non_indexed_write_is_rejected() {
    let parsed = parse_program(STRUCT_DATA_FIELD_NON_INDEXED_WRITE_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject non-indexed write to Data struct field"
    );
}

#[test]
fn implicit_io_infers_and_fills_gaps() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(IMPLICIT_IO_GAPPED_EXAMPLE, frames);
    assert_eq!(in_channels, 3);
    assert_eq!(out_channels, 2);

    let mut input = vec![0.0_f32; frames * in_channels];
    for frame in 0..frames {
        input[frame * in_channels] = 10.0;
        input[frame * in_channels + 1] = 20.0;
        input[frame * in_channels + 2] = (frame + 1) as f32;
    }

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * out_channels], 0.0, 1e-6);
        assert_near(
            output[frame * out_channels + 1],
            ((frame + 1) as f32) * 0.5,
            1e-6,
        );
    }
}

#[test]
fn sparse_declared_io_is_expanded() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SPARSE_DECLARED_IO_EXAMPLE, frames);
    assert_eq!(in_channels, 3);
    assert_eq!(out_channels, 3);

    let mut input = vec![0.0_f32; frames * in_channels];
    for frame in 0..frames {
        input[frame * in_channels + 2] = (frame + 1) as f32;
    }

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * out_channels], 0.0, 1e-6);
        assert_near(output[frame * out_channels + 1], 0.0, 1e-6);
        assert_near(output[frame * out_channels + 2], (frame + 1) as f32, 1e-6);
    }
}

#[test]
fn builtin_consts_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_CONSTS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = std::f32::consts::PI + 2.0 * std::f32::consts::PI;
    for sample in &output {
        assert_near(*sample, expected, 2e-3);
    }
}

#[test]
fn builtin_consts_support_lowercase_aliases() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_CONSTS_LOWERCASE_ALIASES_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    let expected = 5.0 * std::f32::consts::PI;
    for sample in &output {
        assert_near(*sample, expected, 2e-3);
    }
}

#[test]
fn builtin_consts_use_compile_time_sample_rate() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_SR_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 4.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-5);
    assert_near(output[1], 0.0, 1e-5);
    assert_near(output[2], -1.0, 1e-5);
    assert_near(output[3], 0.0, 1e-5);
}

#[test]
fn builtin_consts_support_samplerate_alias() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_SAMPLERATE_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 4.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-5);
    assert_near(output[1], 0.0, 1e-5);
    assert_near(output[2], -1.0, 1e-5);
    assert_near(output[3], 0.0, 1e-5);
}

#[test]
fn builtin_consts_support_lowercase_samplerate_alias() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_LOWERCASE_SR_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 4.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-5);
    assert_near(output[1], 0.0, 1e-5);
    assert_near(output[2], -1.0, 1e-5);
    assert_near(output[3], 0.0, 1e-5);
}

#[test]
fn builtin_intrinsics_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INTRINSICS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let x = (-0.5_f32).abs()
        + (0.0_f32).cos()
        + (4.0_f32).sqrt()
        + (0.0_f32).exp()
        + (1.0_f32).exp().ln();
    let y = 2.0_f32.powf(3.0) + 3.0_f32.min(4.0) + 3.0_f32.max(4.0) + (2.0_f32).mul_add(3.0, 4.0);
    let z = (1.8_f32).floor() + (1.2_f32).ceil() + (1.6_f32).round() + (1.6_f32).trunc();
    let expected = x + y + z;
    for sample in &output {
        assert_near(*sample, expected, 1e-4);
    }
}

#[test]
fn stdlib_math_is_auto_imported() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_AUTO_IMPORT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn stdlib_math_auto_import_allows_local_symbol_override() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_LOCAL_SYMBOL_WINS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn stdlib_buffer_read_mono_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_READ_MONO_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![1.0_f32, 2.0, 3.0, 4.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");

    let out = decode_planar_f32(&out_bytes);
    for sample in &out {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn stdlib_buffer_read_linear_and_cubic_with_channel_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_INTERP_STEREO_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");

    let out = decode_planar_f32(&out_bytes);
    for sample in &out {
        assert_near(*sample, 37.0, 1e-6);
    }
}

#[test]
fn stdlib_buffer_is_auto_imported_for_arrays_and_buffers() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_AUTO_IMPORT_ARRAY_AND_BUFFER_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");

    let out = decode_planar_f32(&out_bytes);
    for sample in &out {
        assert_near(*sample, 34.5, 1e-6);
    }
}

#[test]
fn stdlib_lookup_write_array_and_buffer_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_LOOKUP_WRITE_ARRAY_AND_BUFFER_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![0.0_f32; 4];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");

    let out = decode_planar_f32(&out_bytes);
    for sample in &out {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]
fn floor_fract_wrap_numeric_behavior_is_stable() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(FLOOR_FRACT_WRAP_NUMERIC_BEHAVIOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.75, 1e-6);
    }
}

#[test]
fn builtin_int_intrinsics_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INT_INTRINSICS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 17.0, 1e-6);
    }
}

#[test]
fn float_only_builtin_rejects_integer_arguments() {
    let parsed =
        parse_program(BUILTIN_FLOAT_ONLY_TYPE_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject integer argument for float-only builtin"
    );
}

#[test]
fn data_capacity_supports_compile_time_constants() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CONST_CAPACITY_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 16_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn data_ctor_capacity_supports_compile_time_constants() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CTOR_CONST_CAPACITY_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn block_size_constant_is_available_in_init_and_block() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_CONST_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, (frames as f32) * 2.0, 1e-6);
    }
}

#[test]
fn block_size_aliases_are_available() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_ALIASES_CONST_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, frames as f32, 1e-6);
    }
}

#[test]
fn block_executes_once_per_process_call() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_EXEC_ONCE_PER_PROCESS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("first process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("second process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn block_assigned_scalar_is_visible_in_nested_sample() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_SCALAR_VISIBLE_IN_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // We only need to assert the program compiles/runs and generates non-zero audio.
    assert!(output.iter().any(|v| v.abs() > 1e-6));
}

#[test]
fn block_cannot_access_outputs() {
    let parsed = parse_program(BLOCK_IO_FORBIDDEN_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject output access in block"
    );
}

#[test]
fn builtin_const_assignment_is_rejected() {
    let parsed = parse_program(BUILTIN_CONST_ASSIGN_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject builtin constant assignment"
    );
}

#[test]
fn builtin_const_lowercase_assignment_is_rejected() {
    let parsed =
        parse_program(BUILTIN_CONST_ASSIGN_LOWERCASE_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject lowercase builtin constant assignment"
    );
}

#[test]
fn typed_narrowing_assignment_is_rejected() {
    let parsed =
        parse_program(TYPED_NARROWING_ASSIGNMENT_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject implicit i64->f32 narrowing"
    );
}

#[test]
fn if_condition_must_be_bool() {
    let parsed = parse_program(IF_CONDITION_BOOL_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject non-bool if condition"
    );
}

#[test]
fn init_if_branches_cannot_introduce_conflicting_state_types() {
    let parsed =
        parse_program(IF_BRANCH_TYPE_CONFLICT_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("branch type conflict should be rejected");
    assert!(
        errs.iter().any(|d| {
            d.message.contains("state symbol 'x'")
                && d.message.contains("conflicting types")
                && d.message.contains("across branches")
        }),
        "expected branch type conflict diagnostic, got {:?}",
        errs
    );
}

#[test]
fn typed_data_primitive_elements_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut out = vec![0.0_f32; out_channels];
    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");
    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "typed array elements should preserve runtime values across primitive types"
    );
}

#[test]
fn typed_data_struct_scalar_primitives_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_STRUCT_SCALAR_PRIMITIVES_OK_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut out = vec![0.0_f32; out_channels];
    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");
    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "Struct[N] should support all primitive scalar field types"
    );
}
#[test]
fn data_index_must_be_numeric() {
    let parsed = parse_program(DATA_BOOL_INDEX_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject bool array index"
    );
}

#[test]
fn data_constant_out_of_range_index_is_rejected_in_codegen() {
    let parsed = parse_program(DATA_CONST_OOB_INDEX_ERROR_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    let result = omni_codegen_llvm::lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 64,
            fast_math: false,
        },
    );
    assert!(
        result.is_err(),
        "codegen should reject out-of-range constant array index"
    );
}

#[test]
fn def_return_type_is_inferred_from_return_expression() {
    let parsed = parse_program(DEF_RETURN_F64_INFERENCE_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    let mydef = typed
        .defs
        .iter()
        .find(|d| d.name == "mydef")
        .expect("mydef should be present");
    assert_eq!(mydef.return_ty, omni_semantics::ReturnType::Scalar(PrimitiveType::F64));
}

#[test]
fn def_monomorphizes_from_call_arguments_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_MONOMORPHIZES_FROM_CALL_ARGUMENTS_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn def_monomorphizes_multiple_specializations_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_MONOMORPHIZES_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.5, 1e-6);
    }
}

#[test]
fn non_generic_def_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_DEF_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic defs"
    );
}

#[test]
fn def_declaration_rejects_generic_type_params_syntax() {
    let parsed = parse_program(
        r#"
outs { out1 }
def bad<T>(x) {
  return x
}
sample { out1 = 0.0 }
"#,
    );
    assert!(
        parsed.is_err(),
        "parser should reject generic type params on defs"
    );
}

#[test]
fn generic_struct_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]
fn generic_struct_ctor_infers_type_args_from_arguments() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn generic_struct_ctor_infers_type_args_from_variable_arguments() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]
fn generic_struct_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn generic_struct_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor calls with wrong type argument count"
    );
}

#[test]
fn non_generic_struct_ctor_rejects_type_args() {
    let parsed = parse_program(NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic struct ctors"
    );
}

#[test]
fn generic_struct_multiple_specializations_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn generic_struct_array_field_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn generic_struct_method_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_METHOD_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn struct_method_local_int_inference_matches_def_for_bitwise_ops() {
    let src = r#"
struct Bits<T>:
  def run(self, n: i32):
    bits = 0
    value = n
    while (value > 1):
      value = value >> 1
      bits = bits + 1
    return f32(bits)
outs { out1 }
init:
  b = Bits<f32>()
sample:
  out1 = b.run(8)
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn struct_method_untyped_numeric_calls_compile_and_run() {
    let src = r#"
struct Math:
  def mix(self, x, y):
    return x * y + x

outs { out1 }
init:
  m = Math()
sample:
  a = m.mix(f32(1.5), f32(2.0))
  b = f32(m.mix(f64(1.25), f64(4.0)))
  out1 = a + b
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 10.75, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_infers_type_args_from_arguments() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_infers_type_args_from_defaults() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_DEFAULT_ONLY_INFERENCE_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_infers_array_generic_type_from_array_variable() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_INFER_FROM_ARRAY_VAR_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn generic_proc_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_PROC_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic proc ctor calls with wrong type argument count"
    );
}

#[test]
fn non_generic_proc_ctor_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_PROC_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic proc ctors"
    );
}

#[test]
fn generic_proc_multiple_specializations_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]
fn generic_proc_array_decl_types_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]
fn generic_proc_init_typed_array_generic_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn generic_proc_buffer_decl_type_analyzes_and_codegen_compiles() {
    let parsed = parse_program(GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE)
        .expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    let result = omni_codegen_llvm::lower_and_jit_with_options(
        typed,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 48_000.0,
            block_size: 64,
            fast_math: false,
        },
    );
    assert!(
        result.is_ok(),
        "codegen should succeed for generic proc buffer<T> specialization"
    );
}

#[test]
fn proc_state_struct_ctor_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_STATE_STRUCT_CTOR_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn proc_state_generic_struct_ctor_with_explicit_type_args_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_EXPLICIT_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn proc_state_generic_struct_ctor_infers_type_args_compile_and_run() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_INFERRED_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn first_assignment_uses_def_return_type_and_alias_keeps_type() {
    let parsed = parse_program(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE)
        .expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    assert_eq!(state_type_of(&typed, "x"), Some(PrimitiveType::F64));
    assert_eq!(state_type_of(&typed, "z"), Some(PrimitiveType::F64));

    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn first_assignment_from_int_literal_stays_i32() {
    let parsed =
        parse_program(FIRST_ASSIGNMENT_INT_IS_STICKY_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject implicit f32 assignment after x = 0 infers i32"
    );
}

#[test]
fn proc_first_assignment_uses_def_return_type() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_FIRST_ASSIGNMENT_FROM_DEF_RETURN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn typed_widening_assignment_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_WIDENING_ASSIGNMENT_OK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn typed_init_f64_state_preserves_precision() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_F64_PRESERVES_PRECISION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn typed_init_i64_state_preserves_value() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_I64_PRESERVES_VALUE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn typed_block_declaration_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BLOCK_DECLARATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]
fn typed_sample_declaration_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_SAMPLE_DECLARATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]
fn typed_def_declaration_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DEF_DECLARATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]
fn typed_i32_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I32_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]
fn typed_f64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_F64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]
fn typed_i64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]
fn typed_bool_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BOOL_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn def_can_infer_duck_typed_mono_buffer_param() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![2.0_f32, 4.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [2.0_f32, 4.0, 4.0, 4.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn def_duck_typed_buffer_inference_propagates_through_def_calls() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PROPAGATION_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![1.5_f32, 3.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [1.5_f32, 3.0, 3.0, 3.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn def_duck_typed_buffer_param_allows_mixed_element_types() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_MIXED_ELEM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut a = vec![1.0_f32];
    bind_buffer(
        &mut instance,
        0,
        a.as_mut_ptr().cast::<u8>(),
        a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind f32 buffer");

    let mut b = vec![2.0_f64];
    bind_buffer(
        &mut instance,
        1,
        b.as_mut_ptr().cast::<u8>(),
        b.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind f64 buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]
fn def_indexable_param_accepts_array_and_buffer_call_sites() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_ARRAY_AND_BUFFER_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 22.0, 1e-6);
    }
}

#[test]
fn def_indexable_param_supports_two_dimensional_buffer_indexing() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_STEREO_BUFFER_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 30.0, 1e-6);
    }
}

#[test]
fn proc_single_out_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn proc_single_out_field_access_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_FIELD_ACCESS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn proc_multi_out_call_field_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_CALL_FIELD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]
fn proc_multi_out_field_alias_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_FIELD_ALIAS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]
fn proc_param_mutation_is_immediate() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_PARAM_MUTATION_IMMEDIATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]
fn proc_init_block_is_optional() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_OPTIONAL_INIT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_state_typed_in_init_keeps_type_in_sample() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_TYPED_STATE_PRESERVED_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
    assert_near(output[3], 4.0, 1e-6);
}

#[test]
fn proc_i32_array_increment_keeps_integer_inference() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_I32_ARRAY_INCREMENT_PRESERVED_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
    assert_near(output[3], 4.0, 1e-6);
}

#[test]
fn proc_data_len_method_matches_top_level_behavior() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DATA_LEN_METHOD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]
fn proc_block_wraps_sample_once_per_block() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BLOCK_WRAPS_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output_a, frames).expect("process should succeed");
    for sample in &output_a {
        assert_near(*sample, 6.0, 1e-6);
    }

    let mut output_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output_b, frames).expect("process should succeed");
    for sample in &output_b {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn proc_without_block_has_only_step_entrypoint() {
    let parsed = parse_program(PROC_OPTIONAL_INIT_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(def_names.contains("NoInitProc.__proc_step"));
    assert!(!def_names.contains("NoInitProc.__proc_block_pre"));
    assert!(!def_names.contains("NoInitProc.__proc_block_post"));
}

#[test]
fn proc_nested_block_runs_when_outer_has_no_block() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn proc_outer_without_user_block_gets_effective_block_entrypoints_when_needed() {
    let parsed =
        parse_program(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(def_names.contains("OuterProc.__proc_block_pre"));
    assert!(def_names.contains("OuterProc.__proc_block_post"));
}

#[test]
fn proc_array_dynamic_index_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]
fn proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]
fn nested_proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block()
{
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]
fn proc_array_dynamic_index_block_hooks_use_same_clamped_slot_for_guard_and_call() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_CLAMPED_INDEX_CONSISTENCY_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 100.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 201.0, 1e-6);
}

#[test]
fn proc_array_dynamic_multi_call_expression_preserves_left_to_right_call_eval_order() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_MULTI_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 12.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 34.0, 1e-6);
}

#[test]
fn proc_array_dynamic_five_call_expression_preserves_left_to_right_eval_order() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_FIVE_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");
    assert_near(out_a[0], 12345.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");
    assert_near(out_b[0], 67900.0, 1e-6);
}

#[test]
fn proc_can_bind_and_read_top_level_buffer() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BUFFER_MONO_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [10.0_f32, 20.0, 20.0, 20.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn proc_buffer_missing_ctor_arg_is_rejected() {
    let parsed =
        parse_program(PROC_BUFFER_MISSING_CTOR_ARG_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject proc ctor missing required buffer arg"
    );
}

#[test]
fn proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject positional proc constructor arguments"
    );
}

#[test]
fn nested_proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_NESTED_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject positional nested proc constructor arguments"
    );
}

#[test]
fn sample_oversample_factor_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);
    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(proc_in_channels, 0);
    assert_eq!(proc_out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    let mut proc_output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    process_interleaved(&mut proc_instance, &[], &mut proc_output, frames)
        .expect("proc process should succeed");
    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]
fn sample_oversample_factor_is_recorded_in_typed_program() {
    let parsed = parse_program(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");
    assert_eq!(typed.sample_oversample_factor, 4);
}

#[test]
fn proc_sample_oversample_factor_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);
    let (mut top_level_instance, top_in_channels, top_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_2_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(top_in_channels, 0);
    assert_eq!(top_out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    let mut top_level_output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    process_interleaved(&mut top_level_instance, &[], &mut top_level_output, frames)
        .expect("top-level process should succeed");
    for (actual, expected) in output.iter().zip(top_level_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]
fn sample_oversample_factor_32_compiles_and_runs_smoke() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_32_SMOKE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert!(output.iter().all(|v| v.is_finite()));
    assert!(output[frames - 1] > output[0]);
}

#[test]
fn proc_sample_oversample_factor_64_compiles_and_runs_smoke() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_64_SMOKE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert!(output.iter().all(|v| v.is_finite()));
    assert!(output[frames - 1] > output[0]);
}

#[test]
fn sample_oversample_invalid_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_INVALID_FACTOR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "invalid oversample factor should be rejected"
    );
    let diags = result.expect_err("expected semantic diagnostics");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("{1,2,4,8,16,32,64}") && d.message.contains("got 3")),
        "expected explicit allowed-factor diagnostic, got: {diags:?}"
    );
}

#[test]
fn sample_oversample_non_literal_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_NON_LITERAL_FACTOR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "non-literal oversample factor should be rejected"
    );
    let diags = result.expect_err("expected semantic diagnostics");
    assert!(
        diags.iter().any(|d| {
            d.message.contains("integer literal") && d.message.contains("{1,2,4,8,16,32,64}")
        }),
        "expected integer-literal diagnostic, got: {diags:?}"
    );
}

#[test]
fn sample_oversample_interpolates_input_reads() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);
    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);
    assert_eq!(proc_in_channels, 1);
    assert_eq!(proc_out_channels, 1);

    let input = vec![0.0_f32, 1.0, 2.0, 3.0];
    let mut output = vec![0.0_f32; frames];
    let mut proc_output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc process should succeed");
    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]
fn sample_oversample_passthrough_preserves_more_high_band_level() {
    let frames = 4096;
    let sample_rate = 48_000.0_f32;
    let freq = sample_rate * 0.2;
    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_1X_EXAMPLE, frames);
    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_4X_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| (f32::sin(2.0 * std::f32::consts::PI * freq * idx as f32 / sample_rate)))
        .collect::<Vec<_>>();
    let mut base_output = vec![0.0_f32; frames];
    let mut over_output = vec![0.0_f32; frames];
    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("base passthrough should succeed");
    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled passthrough should succeed");

    let base_rms = rms_after_skip(&base_output, 512);
    let over_rms = rms_after_skip(&over_output, 512);
    let attenuation_db = 20.0 * f32::log10(over_rms / base_rms);
    assert!(
        attenuation_db > -3.0,
        "expected 4x oversampled passthrough to stay within 3 dB at 9.6 kHz, got {attenuation_db} dB"
    );
}

#[test]
fn sample_oversample_reduces_high_frequency_energy_on_nonlinear_patch() {
    let frames = 64;
    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_BASELINE_EXAMPLE, frames);
    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();
    let mut base_output = vec![0.0_f32; frames];
    let mut over_output = vec![0.0_f32; frames];
    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("baseline process should succeed");
    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled process should succeed");

    let high_freq_energy = |samples: &[f32]| -> f32 {
        samples
            .windows(2)
            .map(|w| {
                let d = w[1] - w[0];
                d * d
            })
            .sum::<f32>()
    };
    let base_energy = high_freq_energy(&base_output);
    let over_energy = high_freq_energy(&over_output);
    assert!(
        over_energy < base_energy * 0.5,
        "expected oversampling to reduce high-frequency energy, base={base_energy}, oversampled={over_energy}"
    );
}

#[test]
fn sample_oversample_proc_and_top_level_match_on_nonlinear_patch() {
    let frames = 64;
    let (mut top_level_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);
    let (mut proc_instance, _, _) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();
    let mut top_level_output = vec![0.0_f32; frames];
    let mut proc_output = vec![0.0_f32; frames];
    process_interleaved(
        &mut top_level_instance,
        &input,
        &mut top_level_output,
        frames,
    )
    .expect("top-level oversampled process should succeed");
    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc oversampled process should succeed");

    for (actual, expected) in top_level_output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]
fn sample_oversample_keeps_proc_sine_pitch_constant() {
    let frames = 48_000;
    let sample_rate = 48_000.0_f32;
    let (mut base_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_1X_EXAMPLE, frames);
    let (mut os2_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_2X_EXAMPLE, frames);
    let (mut os4_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_4X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];
    let mut out_2x = vec![0.0_f32; frames];
    let mut out_4x = vec![0.0_f32; frames];
    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");
    process_interleaved(&mut os2_instance, &[], &mut out_2x, frames)
        .expect("2x process should succeed");
    process_interleaved(&mut os4_instance, &[], &mut out_4x, frames)
        .expect("4x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);
    let f2 = estimate_positive_zero_cross_frequency(&out_2x, sample_rate);
    let f4 = estimate_positive_zero_cross_frequency(&out_4x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );
    assert!(
        (f2 - f1).abs() < 1.5,
        "expected 2x oversampling pitch to match 1x, got f1={f1}, f2={f2}"
    );
    assert!(
        (f4 - f1).abs() < 1.5,
        "expected 4x oversampling pitch to match 1x, got f1={f1}, f4={f4}"
    );
}

#[test]
fn proc_sample_oversample_keeps_local_sine_pitch_constant() {
    let frames = 48_000;
    let sample_rate = 48_000.0_f32;
    let (mut base_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_1X_EXAMPLE, frames);
    let (mut os8_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_8X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];
    let mut out_8x = vec![0.0_f32; frames];
    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");
    process_interleaved(&mut os8_instance, &[], &mut out_8x, frames)
        .expect("8x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);
    let f8 = estimate_positive_zero_cross_frequency(&out_8x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );
    assert!(
        (f8 - f1).abs() < 1.5,
        "expected proc sample 8 pitch to match 1x, got f1={f1}, f8={f8}"
    );
}

#[test]
#[ignore = "perf benchmark; run manually"]
fn sample_oversample_n4_performance_budget_benchmark() {
    const FRAMES: usize = 128;
    const WARMUP_ITERS: usize = 256;
    const TIMED_ITERS: usize = 4096;
    const TARGET_RATIO: f64 = 2.5;

    let (mut baseline, baseline_in_channels, baseline_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_BASELINE_EXAMPLE, FRAMES);
    let (mut oversampled, os_in_channels, os_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_N4_EXAMPLE, FRAMES);
    assert_eq!(baseline_in_channels, 1);
    assert_eq!(baseline_out_channels, 1);
    assert_eq!(os_in_channels, 1);
    assert_eq!(os_out_channels, 1);

    let input = (0..FRAMES)
        .map(|idx| ((idx % 97) as f32 / 48.0) - 1.0)
        .collect::<Vec<_>>();
    let mut baseline_output = vec![0.0_f32; FRAMES];
    let mut os_output = vec![0.0_f32; FRAMES];

    let baseline_secs = benchmark_process_runtime(
        &mut baseline,
        &input,
        &mut baseline_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );
    let os_secs = benchmark_process_runtime(
        &mut oversampled,
        &input,
        &mut os_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );
    let ratio = os_secs / baseline_secs.max(f64::EPSILON);
    eprintln!(
        "oversample N=4 runtime ratio: {:.3}x (baseline={:.6}s, os4={:.6}s, frames={}, iters={})",
        ratio, baseline_secs, os_secs, FRAMES, TIMED_ITERS
    );

    if std::env::var("OMNI_ENFORCE_OVERSAMPLE_PERF_BUDGET")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        assert!(
            ratio <= TARGET_RATIO,
            "oversample N=4 runtime ratio {:.3}x exceeded target {:.3}x",
            ratio,
            TARGET_RATIO
        );
    }
}

#[test]
fn proc_array_input_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_array_input_from_local_array_symbol_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_VAR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.3, 1e-6);
    }
}

#[test]
fn proc_array_output_field_read_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_OUTPUT_INDEXED_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_array_param_constructor_args_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_PARAM_CTOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_index_reads_use_clamped_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_INDEX_CLAMP_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_unsafe_read_bypasses_clamp_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_UNSAFE_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_unsafe_write_bypasses_clamp_path() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_UNSAFE_WRITE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_unsafe_oob_compiles() {
    let frames = 4;
    let (_instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_UNSAFE_OOB_COMPILES_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
}

#[test]
fn proc_array_constant_index_out_of_range_is_rejected() {
    let parsed = parse_program(PROC_ARRAY_CONSTANT_INDEX_OOB_REJECTED_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject out-of-range constant proc-array indexing"
    );
}

#[test]
fn proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_field_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_FIELD_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_call_dynamic_index_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_call_dynamic_index_with_oversampled_callee_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_OVERSAMPLED_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert!(
            sample.is_finite() && *sample > 0.0 && *sample < 2.0,
            "expected finite oversampled dynamic dispatch output in (0,2), got {}",
            sample
        );
    }
}

#[test]
fn proc_instance_array_indexed_field_call_dynamic_index_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_FIELD_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_call_dynamic_index_selects_slot_buffer_binding() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2 = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");
    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2.as_mut_ptr().cast::<u8>(),
        buf2.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    for sample in out {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_alias_call_dynamic_index_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_alias_out_read_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_ALIAS_OUT_READ_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn nested_proc_instance_array_indexed_alias_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn proc_instance_array_len_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_LEN_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_call_dynamic_index_buffer_refs_refresh_on_process_bound() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");
    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_bound(&mut instance, frames).expect("process bound with old buf2");
    let out_old = decode_planar_f32(&out_bytes);
    for sample in out_old {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    process_bound(&mut instance, frames).expect("process bound with new buf2");
    let out_new = decode_planar_f32(&out_bytes);
    for sample in out_new {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn proc_instance_array_indexed_call_dynamic_index_buffer_refs_do_not_refresh_on_process_unchecked()
{
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");
    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_bound(&mut instance, frames).expect("process bound to seed proc-slot refs");
    let out_seed = decode_planar_f32(&out_bytes);
    for sample in out_seed {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    validate_buffers(&mut instance).expect("validate buffers after rebind");
    validate_outputs(&mut instance).expect("validate outputs");
    unsafe {
        process_unchecked(&mut instance).expect("unchecked process after rebind");
    }
    let out_unchecked = decode_planar_f32(&out_bytes);
    for sample in out_unchecked {
        assert_near(sample, 0.75, 1e-6);
    }

    process_bound(&mut instance, frames).expect("process bound refreshes refs");
    let out_refreshed = decode_planar_f32(&out_bytes);
    for sample in out_refreshed {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn nested_proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn deep_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEP_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 50.5, 1e-6);
    }
}

#[test]
fn deeper_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEPER_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 55.5, 1e-6);
    }
}

#[test]
fn top_level_proc_instance_array_broadcast_ctor_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]
fn top_level_proc_instance_array_broadcast_ctor_array_literal_arg_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_LITERAL_ARG_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]
fn top_level_proc_instance_array_broadcast_ctor_array_symbol_arg_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_SYMBOL_ARG_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]
fn untyped_init_array_first_element_type_is_enforced() {
    let parsed = parse_program(UNTYPED_INIT_ARRAY_FIRST_ELEMENT_TYPE_MISMATCH_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject untyped init array literals whose later elements are not assignable to the first element type"
    );
}

#[test]
fn top_level_proc_instance_array_broadcast_ctor_mixed_buffer_array_arg_analyzes() {
    let parsed = parse_program(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_MIXED_BUFFER_ARRAY_ARG_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should allow broadcast processor-array ctor with scalar and per-slot buffer arguments"
    );
}

#[test]
fn nested_proc_init_untyped_array_symbol_arg_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INIT_UNTYPED_ARRAY_SYMBOL_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]
fn top_level_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn nested_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]
fn top_level_proc_instance_array_initializer_arity_is_rejected() {
    let parsed = parse_program(TOP_LEVEL_PROC_INSTANCE_ARRAY_INIT_ARITY_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject mismatched constructor count for top-level processor arrays"
    );
}

#[test]
fn proc_nested_state_persists_across_samples() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_STATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.25, 1e-6);
    assert_near(output[1], 0.5, 1e-6);
    assert_near(output[2], 0.75, 1e-6);
    assert_near(output[3], 1.0, 1e-6);
}

#[test]
fn proc_deep_nested_state_persists_across_samples() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_STATE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5, 1e-6);
    assert_near(output[1], 1.0, 1e-6);
    assert_near(output[2], 1.5, 1e-6);
    assert_near(output[3], 2.0, 1e-6);
}

#[test]
fn proc_deep_nested_buffer_binding_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_BUFFER_BIND_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![3.0_f32, 4.0];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    process_bound(&mut instance, frames).expect("process bound");
    let out = decode_planar_f32(&out_bytes);
    let expected = [3.0_f32, 4.0, 4.0, 4.0];
    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]
fn top_level_init_block_is_optional() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OPTIONAL_INIT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn struct_method_compiles_and_runs() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_METHOD_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 1.0, 1e-5);
    assert_near(output[1], 0.0, 1e-5);
    assert_near(output[2], -1.0, 1e-5);
    assert_near(output[3], 0.0, 1e-5);
}

#[test]
fn struct_method_data_write_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_METHOD_DATA_WRITE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn struct_method_requires_self_param() {
    let parsed =
        parse_program(STRUCT_METHOD_SELF_REQUIRED_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject struct method without self first parameter"
    );
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_gain_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        GAIN,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    set_param_f32(&mut instance, "gain", 0.5);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32).collect();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, out) in output.iter().enumerate() {
        assert_near(*out, input[idx] * 0.5, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_sine_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        SINE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let phase_step = 440.0_f32 * 6.2831855_f32 / 48_000.0_f32;
    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32 * phase_step).sin();
        assert_near(*sample, expected, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_one_pole_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        ONE_POLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.1, 1e-6);
    assert!(output[frames - 1] > output[0]);
    assert!(output[frames - 1] < 1.0);
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_if_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        IF_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.25, 1e-6);
    }

    set_param_f32(&mut instance, "gate", 0.0);
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, -0.25, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_for_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        FOR_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_def_call_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_CALL_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_def_return_exits_early() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_EARLY_RETURN_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_struct_compiles_and_runs() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        STRUCT_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn explicit_orc_struct_reserved_method_names_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        RESERVED_METHOD_NAMES_EXAMPLE,
        frames,
        CompileOptions {
            backend: ExecutionBackend::OrcJit,
            sample_rate: 48_000.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]
fn analyze_supports_typed_init_generic_struct_ctor_decl() {
    let src = r#"
import std/data
outs { out1 }
init {
  line: std::data::Data<f32> = std::data::Data()
}
sample {
  out1 = line.read(0)
}
"#;
    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");
    let result = analyze(parsed);
    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]
fn analyze_supports_typed_init_generic_struct_default_ctor_decl() {
    let src = r#"
import std/data
outs { out1 }
init {
  line: std::data::Data<f32>
}
sample {
  out1 = line.read(0)
}
"#;
    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");
    let result = analyze(parsed);
    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]
fn analyze_rejects_typed_init_generic_struct_default_ctor_decl_without_type_args() {
    let src = r#"
import std/data
outs { out1 }
init {
  line: std::data::Data
}
sample {
  out1 = line.read(0)
}
"#;
    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"
    );
}

#[test]
fn analyze_rejects_typed_init_namespace_instantiated_struct_default_ctor_decl_without_type_args() {
    let src = r#"
import std/data
outs { out1 }
init {
  line: std::data<SR, 1>::Data
}
sample {
  out1 = line.read(0)
}
"#;
    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"
    );
}

// ---- Phase 0: Namespace Const Fixes — Integration tests ----

#[test]
fn nested_namespace_template_compiles_and_runs() {
    let src = r#"
namespace Outer<A = 1>:
  namespace Inner<B = 2>:
    struct S:
      x: f32
      def val(self):
        return f32(A + B)
outs 1
init:
  s = Outer<10>::Inner<20>::S()
sample:
  out1 = s.val()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 30.0, 1e-6);
    }
}

#[test]
fn three_level_nested_namespace_template_compiles_and_runs() {
    let src = r#"
namespace L1<A = 1>:
  namespace L2<B = 2>:
    namespace L3<C = 3>:
      def sum():
        return f32(A + B + C)
outs 1
sample:
  out1 = L1<10>::L2<20>::L3<30>::sum()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 60.0, 1e-6);
    }
}

#[test]
fn generic_struct_inside_namespace_template_t_s_pattern_compiles_and_runs() {
    let src = r#"
namespace Data<S = SR>:
  struct Store<T>:
    buf: T[S]
    def write_first(self, v: T):
      self.buf[0] = v
    def read_first(self):
      return self.buf[0]
outs 1
init:
  s = Data<4>::Store<f32>()
sample:
  s.write_first(0.75)
  out1 = s.read_first()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn generic_struct_method_typed_array_param_compile_and_run() {
    let src = r#"
namespace NS:
  struct Store<T>:
    buf: T[2]
    def load(self, input: T[]):
      self.buf[0] = input[0]
      self.buf[1] = input[1]
    def sum(self):
      return self.buf[0] + self.buf[1]
outs 1
init:
  input: f64[2] = [1.25, 0.75]
  s = NS::Store<f64>()
sample:
  s.load(input)
  out1 = f32(s.sum())
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn generic_proc_t_cast_integer_specialization_compile_and_run() {
    let src = r#"
proc ConstVal<T>:
  outs<T> 1
  init:
    v: T = T(3)
  sample:
    out1 = v
outs 1
init:
  p = ConstVal<i64>()
sample:
  out1 = f32(p())
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn generic_proc_inside_namespace_template_compiles_and_runs() {
    let src = r#"
namespace FX:
  proc Gain<T>:
    ins<T> 1
    outs<T> 1
    params<T>:
      g = 1.0
    sample:
      out1 = in1 * g
outs 1
init:
  g = FX::Gain<f64>(g = f64(0.5))
sample:
  out1 = f32(g(f64(2.0)))
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn namespace_qualified_generic_type_from_enclosing_generic_owner_compiles_and_runs() {
    let src = r#"
namespace NS:
  struct Pair:
    a: f64
    b: f64
proc Container:
  outs 1
  init:
    p = NS::Pair(f64(1.0), f64(2.0))
  sample:
    out1 = f32(p.a + p.b)
outs 1
init:
  c = Container()
sample:
  out1 = c()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn multiple_specializations_of_generic_struct_inside_namespace_template_compiles_and_runs() {
    let src = r#"
namespace Data<S = 4>:
  struct Store<T>:
    buf: T[S]
    def first(self):
      return self.buf[0]
outs 1
init:
  sf = Data<4>::Store<f32>()
  sd = Data<4>::Store<f64>()
sample:
  out1 = sf.first() + f32(sd.first())
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn nested_template_with_alias_compiles_and_runs() {
    let src = r#"
namespace Outer<S = SR>:
  namespace Inner<C = 1>:
    struct Buf:
      data: f32[S * C]
      def capacity(self):
        return i32(S * C)
namespace MyBuf = Outer<100>::Inner<2>
outs 1
init:
  b = MyBuf::Buf()
sample:
  out1 = f32(b.capacity())
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 200.0, 1e-6);
    }
}

#[test]
fn namespace_local_alias_to_generic_struct_runtime_compile_and_run() {
    let src = r#"
namespace A:
  namespace Data<S = 4>:
    struct Store<T>:
      storage: T[S]
  namespace D = Data<4>
  proc Runner:
    outs 1
    init:
      s = D::Store<f32>()
    sample:
      s.storage[0] = 1.25
      out1 = s.storage[0]
outs 1
init:
  r = A::Runner()
sample:
  out1 = r()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn namespace_local_alias_to_generic_struct_proc_method_sugar_compile_and_run() {
    let src = r#"
namespace A:
  namespace Data<S = 4>:
    struct Store<T>:
      storage: T[S]
      def write_first(self, v: T):
        self.storage[0] = v
      def read_first(self):
        return self.storage[0]
  namespace D = Data<4>
  proc Runner:
    outs 1
    init:
      s = D::Store<f32>()
    sample:
      s.write_first(1.25)
      out1 = s.read_first()
outs 1
init:
  r = A::Runner()
sample:
  out1 = r()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]
fn nested_namespace_instantiated_generic_struct_with_nested_struct_field_compiles_and_runs() {
    let src = r#"
namespace Outer<S = 3>:
  namespace Inner<C = 2>:
    struct Pair<T>:
      values: T[S * C]
      def write_ends(self, a: T, b: T):
        self.values[0] = a
        self.values[S * C - 1] = b
      def sum_ends(self):
        return self.values[0] + self.values[S * C - 1]

    struct Wrap<T>:
      pair: Pair<T>
      def init_vals(self, a: T, b: T):
        self.pair.write_ends(a, b)
      def sum_vals(self):
        return self.pair.sum_ends()

outs 1
init:
  w = Outer<3>::Inner<2>::Wrap<f32>()
sample:
  w.init_vals(1.0, 2.5)
  out1 = w.sum_vals()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]
fn namespace_param_return_without_i32_cast_infers_i32() {
    let src = r#"
namespace Outer<S = SR>:
  struct Buf:
    def capacity(self):
      return S
outs 1
init:
  b = Outer<200>::Buf()
sample:
  frames: i32 = b.capacity()
  out1 = f32(frames)
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 200.0, 1e-6);
    }
}

#[test]
fn generic_typed_decl_in_proc_sample_block() {
    let parsed = parse_program(
        r#"
proc Gen<T> {
  outs { out1: T }
  init {
    v: T = 1.0
  }
  sample {
    tmp: T = v + 1.0
    out1 = tmp
  }
}
outs { out1 }
init {
  g = Gen<f64>()
}
sample {
  out1 = f32(g.out1)
}
"#,
    )
    .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should accept generic typed declarations in proc sample blocks: {:?}",
        result.err()
    );
}

#[test]
fn generic_typed_decl_in_struct_method() {
    let src = r#"
namespace NS:
  struct Pair<T>:
    a: T
    b: T
    def sum(self):
      tmp: T = self.a + self.b
      return tmp
outs 1
init:
  p = NS::Pair<f64>(3.0, 4.0)
sample:
  out1 = f32(p.sum())
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(src, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]
fn generic_typed_decl_in_proc_event() {
    let parsed = parse_program(
        r#"
proc Gen<T> {
  outs { out1: T }
  events {
    reset() {
      tmp: T = 42.0
      v = tmp
    }
  }
  init {
    v: T = 0.0
  }
  sample {
    out1 = v
  }
}
outs { out1 }
init {
  g = Gen<f64>()
}
sample {
  out1 = f32(g.out1)
}
"#,
    )
    .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "semantic analysis should accept generic typed declarations in proc event blocks: {:?}",
        result.err()
    );
}

#[test]
fn bool_type_arg_rejected_for_struct() {
    let parsed = parse_program(
        r#"
outs { out1 }
struct Box<T> { v: T }
init {
  b = Box<bool>(true)
}
sample {
  out1 = 0.0
}
"#,
    )
    .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject 'bool' as a generic type argument for structs"
    );
}

#[test]
fn bool_type_arg_rejected_for_proc() {
    let parsed = parse_program(
        r#"
proc Gen<T> {
  outs { out1: T }
  sample { out1 = 0.0 }
}
outs { out1 }
init {
  g = Gen<bool>()
}
sample {
  out1 = 0.0
}
"#,
    )
    .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject 'bool' as a generic type argument for processors"
    );
}

// ---------------------------------------------------------------------------
// Phase 3: Generic def parameters — typed array, untyped array, bare buffer
// ---------------------------------------------------------------------------

// Typed array param analysis/parsing coverage.
// Compile-and-run coverage for typed/untyped array params appears below.

const DEF_TYPED_BUFFER_PARAM: &str = r#"
ins { in1 }
outs { out1 }
buffers { buf }
def read_buf(b: buffer[f32], idx: i32) {
  return b[idx]
}
sample {
  buf[0] = in1
  out1 = read_buf(buf, 0)
}
"#;

#[test]
fn def_typed_buffer_param_baseline() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(DEF_TYPED_BUFFER_PARAM, frames);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32 * 0.25).collect();
    let mut output = vec![0.0_f32; frames];

    let buf_idx = instance.buffer_index("buf").expect("buf");
    let buf_data = vec![0.0_f32; frames];
    bind_buffer(
        &mut instance,
        buf_idx,
        buf_data.as_ptr() as *mut u8,
        frames,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        assert_near(*sample, input[idx], 1e-6);
    }
}

// Parse-only tests for new syntax
#[test]
fn parse_typed_array_param_syntax() {
    let src = r#"
outs { out1 }
def foo(arr: f32[]) {
  return arr[0]
}
sample { out1 = 0.0 }
"#;
    let parsed = parse_program(src);
    assert!(
        parsed.is_ok(),
        "should parse f32[] param type: {:?}",
        parsed.err()
    );
}

#[test]
fn parse_untyped_array_param_syntax() {
    let src = r#"
outs { out1 }
def foo(arr: []) {
  return arr[0]
}
sample { out1 = 0.0 }
"#;
    let parsed = parse_program(src);
    assert!(
        parsed.is_ok(),
        "should parse [] param type: {:?}",
        parsed.err()
    );
}

#[test]
fn parse_bare_buffer_param_syntax() {
    let src = r#"
outs { out1 }
def foo(b: buffer) {
  return b[0]
}
sample { out1 = 0.0 }
"#;
    let parsed = parse_program(src);
    assert!(
        parsed.is_ok(),
        "should parse bare buffer param type: {:?}",
        parsed.err()
    );
}

#[test]
fn parse_mixed_new_param_types() {
    let src = r#"
outs { out1 }
def process(arr: f32[], b: buffer, x: f32) {
  return x
}
sample { out1 = 0.0 }
"#;
    let parsed = parse_program(src);
    assert!(
        parsed.is_ok(),
        "should parse mixed new param types: {:?}",
        parsed.err()
    );
}

// Test that typed array param is correctly analyzed
#[test]
fn analyze_typed_array_param() {
    let src = r#"
outs { out1 }
def sum_arr(arr: f32[], n: i32) {
  result = 0.0
  for i in 0..n {
    result = result + arr[i]
  }
  return result
}
init {
  data: f32[4] = [1.0, 2.0, 3.0, 4.0]
}
sample {
  out1 = sum_arr(data, 4)
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "analysis should succeed: {:?}",
        result.err()
    );
}

// Test that bare buffer param is correctly analyzed
#[test]
fn analyze_bare_buffer_param() {
    let src = r#"
ins { in1 }
outs { out1 }
buffers { buf }
def read_first(b: buffer) {
  return b[0]
}
sample {
  buf[0] = in1
  out1 = read_first(buf)
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "analysis should succeed: {:?}",
        result.err()
    );
}

#[test]
fn bitwise_ops_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(BITWISE_OPS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 24.0, 1e-6);
    }
}

#[test]
fn bitwise_ops_reject_float_operands() {
    let parsed = parse_program(BITWISE_FLOAT_OPERAND_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject float operands for bitwise ops"
    );
}

#[test]
fn assert_compile_time_check_passes() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(ASSERT_PASSES_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn assert_rejects_false_namespace_compile_time_condition() {
    let parsed =
        parse_program(ASSERT_NAMESPACE_POWER_OF_TWO_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject false compile-time assert conditions"
    );
}

#[test]
fn stdlib_fft_impulse_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_IMPULSE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-4);
    }
}

#[test]
fn stdlib_complex_struct_compile_and_run() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_COMPLEX_STRUCT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 11.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 125.0_f32.sqrt(), 1e-5);
    assert_near(output[3], 2.0_f32.atan2(11.0), 1e-5);
}

#[test]
fn stdlib_complex_polar_f64_compile_and_run() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_COMPLEX_POLAR_F64_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5_f32.cos(), 1e-5);
    assert_near(output[1], -0.5_f32.sin(), 1e-5);
    assert_near(output[2], 1.0, 1e-6);
}

#[test]
fn stdlib_fft_f64_impulse_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_IMPULSE_F64_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 8.0, 1e-4);
    }
}

#[test]
fn stdlib_fft_real_packed_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_PACKED_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 5.0, 1e-4);
    }
}

#[test]
fn stdlib_fft_real_packed_roundtrip_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_PACKED_ROUNDTRIP_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 1.0, 1e-4);
        assert_near(output[base + 1], 2.0, 1e-4);
        assert_near(output[base + 2], 3.0, 1e-4);
        assert_near(output[base + 3], 4.0, 1e-4);
    }
}

#[test]
fn stdlib_fft_real_spectrum_helpers_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_SPECTRUM_HELPERS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 5.0, 1e-4);
        assert_near(output[base + 1], 5.0, 1e-4);
        assert_near(output[base + 2], -0.5 * std::f32::consts::PI, 1e-4);
        assert_near(output[base + 3], 13.0, 1e-6);
    }
}

#[test]
fn stdlib_stft_hann_window_compile_and_run() {
    let frames = 2;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_STFT_HANN_WINDOW_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        let base = frame * out_channels;
        assert_near(output[base], 0.941_275_5, 1e-4);
        assert_near(output[base + 1], 0.376_510_2, 1e-4);
        assert_near(output[base + 2], 0.188_255_1, 1e-4);
        assert_near(output[base + 3], 13.0, 1e-6);
    }
}

#[test]
fn stdlib_realfft_struct_compile_and_run() {
    let frames = 128;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_STRUCT_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|v| v.abs() > 1e-4),
        "real fft proc should produce non-zero output after its frame latency"
    );
}

#[test]
fn stdlib_realfft_namespaced_proc_compile_and_run() {
    let frames = 128;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_NAMESPACED_PROC_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|v| v.abs() > 1e-4),
        "namespaced real fft proc should produce non-zero output after its frame latency"
    );
}

#[test]
fn stdlib_realfft_hann_ola_passthrough_compile_and_run() {
    let frames = 2048;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_HANN_OLA_PASSTHROUGH_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let mut peak = 0.0_f32;
    for frame in 256..frames {
        let base = frame * out_channels;
        peak = peak.max(output[base].abs());
        assert_near(output[base + 1], 32.0, 1e-6);
        assert_near(output[base + 2], 32.0, 1e-6);
    }
    assert!(
        peak < 0.02,
        "hann overlap-add passthrough should reconstruct the delayed signal closely, peak error was {peak}"
    );
}

#[test]
fn stdlib_convolution_time_domain_event_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_TIME_DOMAIN_EVENT_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("set_ir")
        .expect("set_ir event must exist");
    let mut payload = Vec::new();
    payload.extend_from_slice(&1.0_f32.to_ne_bytes());
    payload.extend_from_slice(&0.5_f32.to_ne_bytes());
    payload.extend_from_slice(&0.25_f32.to_ne_bytes());
    payload.extend_from_slice(&0.0_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, idx, &payload).expect("event trigger should succeed");

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 0.5, 1e-6);
    assert_near(output[2], 0.25, 1e-6);
    assert_near(output[3], 0.0, 1e-6);
    for sample in output.iter().skip(4) {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]
fn stdlib_convolution_block_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_BLOCK_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.0, 1e-5);
    assert_near(output[1], 0.0, 1e-5);
    assert_near(output[2], 0.0, 1e-5);
    assert_near(output[3], 0.0, 1e-5);
    assert_near(output[4], 1.0, 1e-4);
    assert_near(output[5], 0.5, 1e-4);
    assert_near(output[6], 0.25, 1e-4);
    assert_near(output[7], 0.0, 1e-4);
}

#[test]
fn stdlib_convolution_zero_latency_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_ZERO_LATENCY_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);
    assert_near(output[1], 0.5, 1e-4);
    assert_near(output[2], 0.25, 1e-4);
    assert_near(output[3], 0.0, 1e-4);
    assert_near(output[4], 0.125, 1e-4);
    for sample in output.iter().skip(5) {
        assert_near(*sample, 0.0, 1e-4);
    }
}

#[test]
fn stdlib_convolution_zero_latency_with_const_namespace_args_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) = compile_instance(
        STDLIB_CONVOLUTION_ZERO_LATENCY_CONST_NAMESPACE_EXAMPLE,
        frames,
    );
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);
    assert_near(output[1], 0.5, 1e-4);
    assert_near(output[2], 0.25, 1e-4);
    assert_near(output[3], 0.0, 1e-4);
    assert_near(output[4], 0.125, 1e-4);
    for sample in output.iter().skip(5) {
        assert_near(*sample, 0.0, 1e-4);
    }
}

#[test]
fn stdlib_convolution_zero_latency_large_const_namespace_args_analyze() {
    let parsed = parse_program(STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_ANALYZE_EXAMPLE)
        .expect("parse should succeed");
    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 44_100.0,
            block_size: 1024,
        },
    )
    .expect("semantic analysis should succeed");
}

#[test]
fn stdlib_convolution_zero_latency_large_const_wrapper_namespace_analyze() {
    let parsed = parse_program(STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_WRAPPER_ANALYZE_EXAMPLE)
        .expect("parse should succeed");
    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 44_100.0,
            block_size: 1024,
        },
    )
    .expect("semantic analysis should succeed");
}

#[test]
fn stdlib_convolution_generic_f64_compile_and_run() {
    let frames = 8;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_F64_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);
    assert_near(output[1], 0.5, 1e-4);
    assert_near(output[2], 0.25, 1e-4);
    assert_near(output[3], 0.0, 1e-4);
}

#[test]
fn convolution_wav_impulse_example_reproduces_ir_from_event_payload() {
    let src = include_str!("../../../examples/convolution_wav_impulse.omni");
    let ir_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("impulse.wav");
    let ir = read_wav_mono_f32(ir_path.to_str().expect("utf8 path"));
    assert_eq!(
        ir.len(),
        87_085,
        "expected fixed impulse length for example"
    );

    let frames = 131_072;
    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        src,
        frames,
        CompileOptions {
            backend: ExecutionBackend::Auto,
            sample_rate: 44_100.0,
            block_size: frames,
            fast_math: false,
        },
    );
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let event_idx = instance
        .event_index("load_ir")
        .expect("load_ir event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), None);

    let mut payload =
        Vec::with_capacity(std::mem::size_of::<i32>() + ir.len() * std::mem::size_of::<f32>());
    payload.extend_from_slice(&(ir.len() as i32).to_ne_bytes());
    for sample in &ir {
        payload.extend_from_slice(&sample.to_ne_bytes());
    }
    trigger_event_by_index(&mut instance, event_idx, &payload)
        .expect("event trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let latency = 0usize;
    for sample in output.iter().take(latency) {
        assert_near(*sample, 0.0, 1e-4);
    }

    for (idx, expected) in ir.iter().enumerate() {
        assert_near(output[latency + idx], *expected, 1e-3);
    }
}

#[test]
fn nested_struct_field_and_method_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(NESTED_STRUCT_FIELD_AND_METHOD_EXAMPLE, frames);

    let mut outputs = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut outputs, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * 2;
        assert_near(outputs[base], 1.5, 1e-6);
        assert_near(outputs[base + 1], 4.0, 1e-6);
    }
}

#[test]
fn multiline_struct_method_call_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(MULTILINE_STRUCT_METHOD_CALL_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.75, 1e-6);
    }
}

#[test]
fn nested_generic_struct_array_field_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(NESTED_GENERIC_STRUCT_ARRAY_FIELD_EXAMPLE, frames);

    let mut outputs = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut outputs, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * 2;
        assert_near(outputs[base], 1.0, 1e-6);
        assert_near(outputs[base + 1], 3.0, 1e-6);
    }
}

#[test]
fn stdlib_fft_rejects_zero_size() {
    let parsed = parse_program(STDLIB_FFT_ZERO_SIZE_ERROR_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject zero-sized std::fft instantiations"
    );
}

#[test]
fn analyze_mutable_typed_array_param() {
    let src = r#"
outs { out1 }
init {
  data: f32[4] = [1.0, 2.0, 3.0, 4.0]
}
def write_first(arr: f32[]) {
  arr[0] = 9.0
  return arr[0]
}
sample {
  out1 = write_first(data)
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "analysis should allow mutable array params in def: {:?}",
        result.err()
    );
}

#[test]
fn analyze_mutable_buffer_param() {
    let src = r#"
ins { in1 }
outs { out1 }
buffers { buf }
def write_first(b: buffer[f32], x: f32) {
  b[0] = x
  return b[0]
}
sample {
  out1 = write_first(buf, in1)
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "analysis should allow mutable buffer params in def: {:?}",
        result.err()
    );
}

// Test generic struct def param (monomorphization)
#[test]
fn analyze_generic_struct_def_param() {
    let src = r#"
outs { out1 }
struct Box<T> {
  value: T = 0.0
}
def unbox(b: Box) {
  return b.value
}
init {
  mybox = Box<f32>(value = 42.0)
}
sample {
  out1 = unbox(mybox)
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_ok(),
        "analysis should succeed for generic struct def param: {:?}",
        result.err()
    );
}

// Test generic struct def param compiles and runs
const DEF_GENERIC_STRUCT_PARAM: &str = r#"
outs { out1 }
struct Box<T> {
  value: T = 0.0
}
def unbox(b: Box) {
  return b.value
}
init {
  mybox = Box<f32>(value = 42.0)
}
sample {
  out1 = unbox(mybox)
}
"#;

#[test]
fn def_generic_struct_param_compiles_and_runs() {
    let frames = 2;
    let (mut instance, _, _) = compile_instance(DEF_GENERIC_STRUCT_PARAM, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 42.0, 1e-6);
    }
}

// ── Array param ABI fix tests (ptr+len) ──────────────────────────────

const ARRAY_PARAM_SUM_EXAMPLE: &str = r#"
outs { out1 }
init {
    data: f32[4]
    data[0] = 1.0
    data[1] = 2.0
    data[2] = 3.0
    data[3] = 4.0
}
def sum_array(arr: f32[]) {
    total = 0.0
    n = arr.len()
    for i in 0..n {
        total = total + arr[i]
    }
    return total
}
sample {
    out1 = sum_array(data)
}
"#;

#[test]
fn array_param_sum_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_SUM_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_SINGLE_DEF_EXAMPLE: &str = r#"
outs { out1 }
init {
    data: f32[2]
    data[0] = 10.0
    data[1] = 20.0
}
def first(arr: f32[]) { return arr[0] }
sample {
    out1 = first(data)
}
"#;

#[test]
fn array_param_single_def_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_SINGLE_DEF_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_DEF_TO_DEF_EXAMPLE: &str = r#"
outs { out1 }
init {
    data: f32[2]
    data[0] = 10.0
    data[1] = 20.0
}
def first(arr: f32[]) { return arr[0] }
def wrap_first(arr: f32[]) { return first(arr) }
sample {
    out1 = wrap_first(data)
}
"#;

#[test]
fn array_param_def_to_def_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_DEF_TO_DEF_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_LEN_FORWARDED_EXAMPLE: &str = r#"
outs { out1 }
init {
    data: f32[3]
    data[0] = 5.0
    data[1] = 10.0
    data[2] = 15.0
}
def get_len(arr: f32[]) { return arr.len() }
sample {
    out1 = get_len(data)
}
"#;

#[test]
fn array_param_len_forwarded_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_LEN_FORWARDED_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const ARRAY_PARAM_MUTATION_EXAMPLE: &str = r#"
outs { out1 }
init {
    data: f32[4]
    data[0] = 1.0
    data[1] = 2.0
    data[2] = 3.0
    data[3] = 4.0
}
def write_and_sum(arr: f32[]) {
    arr[0] = arr[1] + arr[2]
    return arr[0] + arr[3]
}
sample {
    out1 = write_and_sum(data)
}
"#;

#[test]
fn array_param_mutation_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_MUTATION_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 9.0, 1e-6);
    }
}

const BUFFER_PARAM_MUTATION_EXAMPLE: &str = r#"
ins { in1 }
outs { out1 }
buffers { buf: buffer[f32] }
def write_first(b: buffer[f32], x: f32) {
    b[0] = x
    return b[0]
}
sample {
    out1 = write_first(buf, in1)
}
"#;

#[test]
fn buffer_param_mutation_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(BUFFER_PARAM_MUTATION_EXAMPLE, frames);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32 * 0.25).collect();
    let mut output = vec![0.0_f32; frames];

    let buf_idx = instance.buffer_index("buf").expect("buf");
    let mut buf_data = vec![0.0_f32; frames];
    bind_buffer(
        &mut instance,
        buf_idx,
        buf_data.as_mut_ptr() as *mut u8,
        frames,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        assert_near(*sample, input[idx], 1e-6);
    }
}

const UNTYPED_ARRAY_PARAM_FROM_INIT_ARRAY_EXAMPLE: &str = r#"
outs { out1 }
init {
    data = [0.5, 1.0, 2.5]
}
def sum_first_last(arr: []) {
    return arr[0] + arr[2]
}
sample {
    out1 = sum_first_last(data)
}
"#;

#[test]
fn untyped_array_param_from_init_array_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(UNTYPED_ARRAY_PARAM_FROM_INIT_ARRAY_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_F32_TYPE_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo<f32>[1000]
}
sample {
  s = data[10]
  s.v[0] = 0.5
  s.v[1] = 1.5
  out1 = s.v[0] + s.v[1]
}
"#;

#[test]
fn generic_struct_array_explicit_f32_type_arg_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_F32_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_IMPLICIT_DEFAULT_F32_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo[1000]
}
sample {
  s = data[10]
  s.v[0] = 1.0
  s.v[1] = 2.0
  out1 = s.v[0] + s.v[1]
}
"#;

const GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE: &str = r#"
outs { out1 }
struct Complex<T> {
  re: T
  im: T
  def set(self, re, im) {
    self.re = re
    self.im = im
  }
  def mul_parts(self, re, im) {
    old_re = self.re
    old_im = self.im
    self.re = old_re * re - old_im * im
    self.im = old_re * im + old_im * re
  }
  def sum(self) {
    return self.re + self.im
  }
}
init {
  bins: Complex<f32>[4]
}
sample {
  bins[1].set(1.0, 2.0)
  bins[1].mul_parts(3.0, -4.0)
  out1 = bins[1].sum()
}
"#;

const GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_F64_EXAMPLE: &str = r#"
outs { out1 }
struct Complex<T> {
  re: T
  im: T
  def set_polar(self, magnitude, phase) {
    self.re = magnitude * cos(phase)
    self.im = magnitude * sin(phase)
  }
  def conjugate(self) {
    self.im = -self.im
  }
  def scale_assign(self, gain) {
    self.re = self.re * gain
    self.im = self.im * gain
  }
  def sum(self) {
    return self.re + self.im
  }
}
init {
  bins: Complex<f64>[4]
}
sample {
  bins[0].set_polar(f64(2.0), f64(0.5))
  bins[0].conjugate()
  bins[0].scale_assign(f64(0.5))
  out1 = f32(bins[0].sum())
}
"#;

const STDLIB_COMPLEX_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE: &str = r#"
import std/complex
outs { out1 }
init {
  bins: std::complex::Complex<f32>[4]
}
sample {
  bins[1].set(1.0, 2.0)
  bins[1].mul_parts(3.0, -4.0)
  out1 = bins[1].real() + bins[1].imag()
}
"#;

#[test]
fn generic_struct_array_implicit_default_f32_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_IMPLICIT_DEFAULT_F32_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]
fn generic_struct_array_indexed_method_calls_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]
fn generic_struct_array_indexed_method_calls_f64_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_F64_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = 0.5_f32.cos() - 0.5_f32.sin();
    for sample in &output {
        assert_near(*sample, expected, 1e-5);
    }
}

#[test]
fn stdlib_complex_array_indexed_method_calls_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(STDLIB_COMPLEX_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_F64_TYPE_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo<f64>[1000]
}
sample {
  s = data[10]
  s.v[0] = f64(0.5)
  s.v[1] = f64(1.5)
  out1 = f32(s.v[0] + s.v[1])
}
"#;

#[test]
fn generic_struct_array_explicit_f64_type_arg_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_F64_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_I32_TYPE_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo<i32>[1000]
}
sample {
  s = data[10]
  s.v[0] = 1
  s.v[1] = 2
  out1 = f32(s.v[0] + s.v[1])
}
"#;

#[test]
fn generic_struct_array_explicit_i32_type_arg_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_I32_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_I64_TYPE_ARG_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo<i64>[1000]
}
sample {
  s = data[10]
  s.v[0] = i64(1)
  s.v[1] = i64(3)
  out1 = f32(s.v[0] + s.v[1])
}
"#;

#[test]
fn generic_struct_array_explicit_i64_type_arg_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_I64_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_BOOL_TYPE_ARG_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Stereo<T> {
  v: T[2]
}
init {
  data: Stereo<bool>[1000]
}
sample {
  out1 = 0.0
}
"#;

#[test]
fn generic_struct_array_explicit_bool_type_arg_is_rejected() {
    let parsed =
        parse_program(GENERIC_STRUCT_ARRAY_EXPLICIT_BOOL_TYPE_ARG_ERROR_EXAMPLE).expect("parse");
    let errs = analyze(parsed).expect_err("bool generic arg should be rejected");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("bool") && d.message.contains("generic type")),
        "expected bool generic type arg rejection, got {:?}",
        errs
    );
}

const GENERIC_STRUCT_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T, U> {
  a: T
  b: U
}
init {
  p = Pair<f32, i64>(1.5, i64(2))
}
sample {
  out1 = p.a + f32(p.b)
}
"#;

#[test]
fn generic_struct_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T, U> {
  a: T
  b: U
}
init {
  data: Pair<f32, i64>[1000]
}
sample {
  s = data[10]
  s.a = 0.5
  s.b = i64(3)
  out1 = s.a + f32(s.b)
}
"#;

#[test]
fn generic_struct_array_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_ARRAY_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

const GENERIC_PROC_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"
proc Duo<T, U> {
  ins {
    in1: T
    in2: U
  }
  outs {
    out1: T
    out2: U
  }
  sample {
    out1 = in1
    out2 = in2
  }
}
outs { out1 }
init {
  p = Duo<f32, i64>()
}
sample {
  p(1.25, i64(2))
  out1 = p.out1 + f32(p.out2)
}
"#;

#[test]
fn generic_proc_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(
        GENERIC_PROC_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.25, 1e-6);
    }
}

const TOP_LEVEL_CONST_EVENT_SIZE_EXAMPLE: &str = r#"
const N = 3

proc Voice {
  params { sum = 0.0 }
  outs { out1 }
  events {
    load(values: f32[N]) {
      sum = values[0] + values[1] + values[2]
    }
  }
  sample {
    out1 = sum
  }
}

outs { out1 }
events {
  load(values: f32[N]) {
    voice.load(values)
  }
}
init {
  voice = Voice()
}
sample {
  out1 = voice()
}
"#;

#[test]
fn top_level_consts_can_drive_event_sizes_and_proc_apis() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_CONST_EVENT_SIZE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let event_idx = instance.event_index("load").expect("load event must exist");
    assert_eq!(instance.event_payload_bytes(event_idx), Some(12));

    let mut payload = Vec::new();
    payload.extend_from_slice(&1.0_f32.to_ne_bytes());
    payload.extend_from_slice(&2.0_f32.to_ne_bytes());
    payload.extend_from_slice(&3.0_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, event_idx, &payload).expect("event trigger should work");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

const CONST_SCOPE_COMPILE_AND_RUN_EXAMPLE: &str = r#"
const N = 3

outs { out1 }

def bonus() {
  const X = 0.5
  return X
}

init {
  const BASE: i32 = 1
  vals: f32[N] = [0.0, 0.0, 0.0]
  vals[BASE + 1] = 1.25
  seed = vals[2]
}

sample {
  const SCALE: f32 = 2.0
  out1 = seed + bonus() + SCALE
}
"#;

#[test]
fn consts_work_in_def_init_and_sample_scopes() {
    let frames = 4;
    let (mut instance, _, _) = compile_instance(CONST_SCOPE_COMPILE_AND_RUN_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.75, 1e-6);
    }
}

const CONST_ASSIGN_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  const X = 1
  X = 2
  out1 = 0.0
}
"#;

const CONST_RESERVED_NAME_ERROR_EXAMPLE: &str = r#"
const SR = 1
outs { out1 }
sample {
  out1 = 0.0
}
"#;

const CONST_RUNTIME_INIT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  x = 1.0
  const BAD = x
  out1 = 0.0
}
"#;

const NAMESPACE_CONST_ACCESS_EXAMPLE: &str = r#"
import std/convolution

outs {
  out1
  out2
}

sample {
  out1 = f32(std::convolution<8, 8>::HopSize)
  out2 = f32(std::convolution::HopSize)
}
"#;

#[test]
fn consts_reject_assignment_reserved_names_and_runtime_initializers() {
    let errs = parse_program(CONST_ASSIGN_ERROR_EXAMPLE)
        .expect_err("assigning to a const should be rejected");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("cannot assign to constant 'X'")),
        "expected const assignment error, got {:?}",
        errs
    );

    let errs = parse_program(CONST_RESERVED_NAME_ERROR_EXAMPLE)
        .expect_err("builtin const names should be reserved");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("constant name 'SR' is reserved")),
        "expected reserved const name error, got {:?}",
        errs
    );

    let errs = parse_program(CONST_RUNTIME_INIT_ERROR_EXAMPLE)
        .expect_err("runtime initializer should be rejected");
    assert!(
        errs.iter().any(|d| {
            d.message.contains("const 'BAD'") && d.message.contains("non-compile-time symbol 'x'")
        }),
        "expected compile-time const initializer error, got {:?}",
        errs
    );
}

#[test]
fn namespace_consts_are_accessible_via_qualified_paths() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_CONST_ACCESS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * 2], 4.0, 1e-6);
        assert_near(output[frame * 2 + 1], 128.0, 1e-6);
    }
}

// ── proc-local defs ──────────────────────────────────────────────────────────

const PROC_LOCAL_DEF_VOID_EXAMPLE: &str = r#"
proc Gain {
  ins 1
  outs 1

  init {
    level = 0.0
  }

  def _reset() {
    level = 0.0
  }

  events {
    set_level(v) {
      level = v
    }
    reset() {
      _reset()
    }
  }

  sample {
    out1 = in1 * level
  }
}

ins 1
outs 1
events {
  reset() {
    g.reset()
  }
}
init {
  g = Gain()
  g.set_level(0.5)
}
sample {
  out1 = g(in1)
}
"#;

#[test]
fn proc_local_def_void_call_from_event() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_VOID_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }

    // Trigger reset event
    let idx = instance
        .event_index("reset")
        .expect("reset event must exist");
    trigger_event_by_index(&mut instance, idx, &[]).expect("reset trigger should succeed");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed after reset");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_SHARED_HELPER_EXAMPLE: &str = r#"
proc Holder {
  outs 1

  init {
    value = 0.0
  }

  def _clear() {
    value = 0.0
  }

  events {
    set_value(v) {
      _clear()
      value = v
    }
    reset() {
      _clear()
    }
  }

  sample {
    out1 = value
  }
}

outs 1
events {
  reset() {
    h.reset()
  }
}
init {
  h = Holder()
  h.set_value(3.0)
}
sample {
  out1 = h()
}
"#;

#[test]
fn proc_local_def_shared_helper_called_from_multiple_events() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_SHARED_HELPER_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }

    let idx = instance
        .event_index("reset")
        .expect("reset event must exist");
    trigger_event_by_index(&mut instance, idx, &[]).expect("reset trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed after reset");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_WITH_PARAMS_EXAMPLE: &str = r#"
proc Scaler {
  outs 1

  init {
    value = 0.0
  }

  def _set_scaled(base, factor) {
    value = base * factor
  }

  events {
    apply(v) {
      _set_scaled(v, 2.0)
    }
  }

  sample {
    out1 = value
  }
}

init {
  s = Scaler()
  s.apply(1.5)
}

sample {
  out1 = s()
}
"#;

#[test]
fn proc_local_def_with_params() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_WITH_PARAMS_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_TRANSITIVE_EXAMPLE: &str = r#"
proc Counter {
  outs 1

  init {
    count = 0.0
  }

  def _zero() {
    count = 0.0
  }

  def _full_reset() {
    _zero()
  }

  events {
    reset() {
      _full_reset()
    }
    set(v) {
      count = v
    }
  }

  sample {
    out1 = count
  }
}

outs 1
events {
  reset() {
    c.reset()
  }
}
init {
  c = Counter()
  c.set(5.0)
}
sample {
  out1 = c()
}
"#;

#[test]
fn proc_local_def_transitive_call() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_TRANSITIVE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }

    let idx = instance
        .event_index("reset")
        .expect("reset event must exist");
    trigger_event_by_index(&mut instance, idx, &[]).expect("reset trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed after reset");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_CALLED_FROM_SAMPLE_EXAMPLE: &str = r#"
proc Clipper {
  ins 1
  outs 1

  init {
    threshold = 1.0
  }

  def _clamp(x) {
    if (x > threshold) {
      return threshold
    }
    if (x < -threshold) {
      return -threshold
    }
    return x
  }

  events {
    set_threshold(v) {
      threshold = v
    }
  }

  sample {
    out1 = _clamp(in1)
  }
}

init {
  c = Clipper()
  c.set_threshold(0.5)
}

sample {
  out1 = c(in1)
}
"#;

#[test]
fn proc_local_def_called_from_sample() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_CALLED_FROM_SAMPLE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.3, 0.8, -0.2, -0.9];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 0.3, 1e-6);
    assert_near(output[1], 0.5, 1e-6);
    assert_near(output[2], -0.2, 1e-6);
    assert_near(output[3], -0.5, 1e-6);
}

const PROC_LOCAL_DEF_WHILE_COND_EXAMPLE: &str = r#"
proc CounterLoop {
  outs 1

  init {
    n: i32 = 0
    total = 0.0
  }

  def step() {
    n = n - 1
    return n >= 0
  }

  sample {
    n = 3
    total = 0.0
    while (step()) {
      total = total + 1.0
    }
    out1 = total
  }
}

init { c = CounterLoop() }
sample { out1 = c() }
"#;

#[test]
fn proc_local_def_called_from_while_condition() {
    let frames = 2;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_WHILE_COND_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_NESTED_RETURN_F64_EXAMPLE: &str = r#"
proc BranchReturn {
  ins 1
  outs 1

  init {
    gate = 1.0
  }

  def choose(x: f64) {
    if (gate > 0.5) {
      return x
    } else {
      return f64(0.25)
    }
  }

  sample {
    out1 = f32(choose(f64(in1)))
  }
}

init { b = BranchReturn() }
sample { out1 = b(in1) }
"#;

#[test]
fn proc_local_def_nested_return_infers_temp_type_from_nested_branch() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_NESTED_RETURN_F64_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.5_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

// ── proc-local defs: error cases ────────────────────────────────────────────

const PROC_LOCAL_DEF_CYCLE_ERROR_EXAMPLE: &str = r#"
proc Bad {
  outs 1

  init {
    x = 0.0
  }

  def a() {
    b()
  }

  def b() {
    a()
  }

  sample {
    a()
    out1 = x
  }
}

init { p = Bad() }
sample { out1 = p() }
"#;

#[test]
fn proc_local_def_cycle_detection_error() {
    let parsed = parse_program(PROC_LOCAL_DEF_CYCLE_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("recursive proc-local defs should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("recursive proc-local def cycle")),
        "expected cycle error, got {errs:?}"
    );
}

const PROC_LOCAL_DEF_DUPLICATE_ERROR_EXAMPLE: &str = r#"
proc Bad {
  outs 1

  init {
    x = 0.0
  }

  def foo() {
    x = 1.0
  }

  def foo() {
    x = 2.0
  }

  sample {
    foo()
    out1 = x
  }
}

init { p = Bad() }
sample { out1 = p() }
"#;

#[test]
fn proc_local_def_duplicate_name_error() {
    let parsed =
        parse_program(PROC_LOCAL_DEF_DUPLICATE_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("duplicate proc-local def should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("duplicate proc-local def")),
        "expected duplicate error, got {errs:?}"
    );
}

const PROC_LOCAL_DEF_BAD_CALL_SHAPES_ERROR_EXAMPLE: &str = r#"
proc BadCalls {
  outs 1

  def pair(x, y) {
    return x + y
  }

  sample {
    a = pair(1.0)
    b = pair(1.0, 2.0, 3.0)
    c = pair(x = 1.0, 2.0)
    d = pair(z = 1.0, y = 2.0)
    out1 = a + b + c + d
  }
}

init { p = BadCalls() }
sample { out1 = p() }
"#;

#[test]
fn proc_local_def_bad_call_shapes_error() {
    let parsed =
        parse_program(PROC_LOCAL_DEF_BAD_CALL_SHAPES_ERROR_EXAMPLE).expect("parse should succeed");
    let errs = analyze(parsed).expect_err("bad proc-local def calls should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("missing required argument 'y'")),
        "expected missing-arg error, got {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|d| d.message.contains("too many positional arguments")),
        "expected too-many-args error, got {errs:?}"
    );
    assert!(
        errs.iter().any(|d| d
            .message
            .contains("positional arguments must come before named arguments")),
        "expected positional-after-named error, got {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|d| d.message.contains("unknown named argument 'z'")),
        "expected unknown-named-arg error, got {errs:?}"
    );
}

// ── proc-local defs: additional coverage ────────────────────────────────────

const PROC_LOCAL_DEF_CALLED_FROM_INIT_EXAMPLE: &str = r#"
proc Initer {
  outs 1

  init {
    value = 0.0
    setup(10.0)
  }

  def setup(v) {
    value = v
  }

  sample {
    out1 = value
  }
}

init { p = Initer() }
sample { out1 = p() }
"#;

#[test]
fn proc_local_def_called_from_init() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_CALLED_FROM_INIT_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_TYPED_PARAMS_EXAMPLE: &str = r#"
proc Mixer {
  ins 1
  outs 1

  init {
    gain = 1.0
  }

  def apply_gain(x: f32, g: f32) {
    return x * g
  }

  events {
    set_gain(v: f32) {
      gain = v
    }
  }

  sample {
    out1 = apply_gain(in1, gain)
  }
}

init {
  m = Mixer()
  m.set_gain(0.25)
}
sample { out1 = m(in1) }
"#;

#[test]
fn proc_local_def_typed_params() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_TYPED_PARAMS_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![4.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_DEFAULT_PARAMS_EXAMPLE: &str = r#"
proc Scaler {
  outs 1

  init {
    value = 0.0
  }

  def set_scaled(base, factor = 2.0) {
    value = base * factor
  }

  events {
    apply_default(v) {
      set_scaled(v)
    }
    apply_custom(v, f) {
      set_scaled(v, f)
    }
  }

  sample {
    out1 = value
  }
}

outs 1
events {
  apply_default(v) { s.apply_default(v) }
  apply_custom(v, f) { s.apply_custom(v, f) }
}
init { s = Scaler() }
sample { out1 = s() }
"#;

#[test]
fn proc_local_def_default_params() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_DEFAULT_PARAMS_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    // Test default factor (2.0): 3.0 * 2.0 = 6.0
    let idx = instance
        .event_index("apply_default")
        .expect("event must exist");
    trigger_event_by_index(&mut instance, idx, &3.0_f32.to_le_bytes())
        .expect("trigger should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }

    // Test explicit factor (0.5): 4.0 * 0.5 = 2.0
    let idx2 = instance
        .event_index("apply_custom")
        .expect("event must exist");
    let mut payload = Vec::new();
    payload.extend_from_slice(&4.0_f32.to_le_bytes());
    payload.extend_from_slice(&0.5_f32.to_le_bytes());
    trigger_event_by_index(&mut instance, idx2, &payload).expect("trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_NESTED_PROC_EVENT_EXAMPLE: &str = r#"
proc Inner {
  outs 1

  init {
    val = 0.0
  }

  events {
    set_val(v) {
      val = v
    }
  }

  sample {
    out1 = val
  }
}

proc Outer {
  outs 1

  init {
    child = Inner()
  }

  def reset_child() {
    child.set_val(0.0)
  }

  events {
    load(v) {
      child.set_val(v)
    }
    clear() {
      reset_child()
    }
  }

  sample {
    out1 = child()
  }
}

outs 1
events {
  load(v) { o.load(v) }
  clear() { o.clear() }
}
init {
  o = Outer()
  o.load(7.0)
}
sample { out1 = o() }
"#;

#[test]
fn proc_local_def_nested_proc_event_call() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_NESTED_PROC_EVENT_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }

    let idx = instance
        .event_index("clear")
        .expect("clear event must exist");
    trigger_event_by_index(&mut instance, idx, &[]).expect("trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed after clear");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_GENERIC_PROC_EXAMPLE: &str = r#"
proc Accum<T> {
  ins<T> 1
  outs<T> 1

  init {
    total: T = 0.0
  }

  def add_scaled(x: T, factor: T) {
    total = total + x * factor
  }

  def do_reset() {
    total = T(0.0)
  }

  events {
    reset() {
      do_reset()
    }
  }

  sample {
    add_scaled(in1, T(0.5))
    out1 = total
  }
}

outs 1
events { reset() { a.reset() } }
init { a = Accum<f32>() }
sample { out1 = a(in1) }
"#;

#[test]
fn proc_local_def_generic_proc() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_GENERIC_PROC_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![2.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // Each sample: total += 2.0 * 0.5 = 1.0
    // Accumulated: 1.0, 2.0, 3.0, 4.0
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
    assert_near(output[3], 4.0, 1e-6);

    // Reset and verify
    let idx = instance
        .event_index("reset")
        .expect("reset event must exist");
    trigger_event_by_index(&mut instance, idx, &[]).expect("trigger should succeed");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed after reset");
    assert_near(output[0], 1.0, 1e-6);
}

const PROC_LOCAL_DEF_GENERIC_SCALAR_PARAM_EXAMPLE: &str = r#"
proc Scale<T> {
  ins<T> 1
  outs<T> 1

  def mul_add(x: T, gain: T, bias: T) {
    return x * gain + bias
  }

  sample {
    out1 = mul_add(in1, T(2.0), T(0.5))
  }
}

outs 1
init { s = Scale<f32>() }
sample { out1 = s(in1) }
"#;

#[test]
fn proc_local_def_owner_generic_scalar_param() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_GENERIC_SCALAR_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]
fn proc_local_def_untyped_numeric_calls_compile_and_run() {
    let src = r#"
proc Math {
  outs 1

  def mix(x, y) {
    return x * y + x
  }

  sample {
    a = mix(f32(1.5), f32(2.0))
    b = f32(mix(f64(1.25), f64(4.0)))
    out1 = a + b
  }
}

outs 1
init { m = Math() }
sample { out1 = m() }
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.75, 1e-6);
    }
}

const PROC_LOCAL_DEF_GENERIC_SLICE_PARAM_EXAMPLE: &str = r#"
proc Loader<T> {
  outs 1

  init {
    data: T[4] = [T(1.0), T(2.0), T(3.0), T(4.0)]
  }

  def sum_window(values: T[]) {
    return values[0] + values[1]
  }

  sample {
    out1 = f32(sum_window(data[1:-1]))
  }
}

init { l = Loader<f32>() }
sample { out1 = l() }
"#;

#[test]
fn proc_local_def_owner_generic_slice_param() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_GENERIC_SLICE_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_GENERIC_BUFFER_PARAM_EXAMPLE: &str = r#"
buffers { src: buffer[f32] }

proc Reader<T> {
  buffers { line: buffer[T] }
  outs 1

  def first_plus(buf: buffer[T], add: T) {
    return buf[0] + add
  }

  sample {
    out1 = f32(first_plus(line, T(0.25)))
  }
}

init { r = Reader<f32>(line = src) }
sample { out1 = r() }
"#;

#[test]
fn proc_local_def_owner_generic_buffer_param() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_GENERIC_BUFFER_PARAM_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut buf = vec![1.5_f32, 2.5, 3.5, 4.5];
    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

const PROC_LOCAL_DEF_MULTIPLE_INLINE_FORLOOP_EXAMPLE: &str = r#"
proc Summer {
  outs 1

  init {
    data: f32[4] = [1.0, 2.0, 3.0, 4.0]
  }

  def sum_range(start: i32, count: i32) {
    total = 0.0
    for i in start..(start + count) {
      total = total + data[i]
    }
    return total
  }

  sample {
    a = sum_range(0, 2)
    b = sum_range(2, 2)
    out1 = a + b
  }
}

init { s = Summer() }
sample { out1 = s() }
"#;

#[test]
fn proc_local_def_multiple_inline_sites_forloop_vars() {
    let frames = 1;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_MULTIPLE_INLINE_FORLOOP_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // sum_range(0,2) = 1+2 = 3, sum_range(2,2) = 3+4 = 7, total = 10
    assert_near(output[0], 10.0, 1e-6);
}

// ── proc-local defs: expression-position calls ──────────────────────────────

const PROC_LOCAL_DEF_EXPR_POSITION_BINARY_EXAMPLE: &str = r#"
proc Calc {
  ins 1
  outs 1

  init {
    offset = 10.0
  }

  def shifted(x) {
    return x + offset
  }

  sample {
    out1 = shifted(in1) * 2.0
  }
}

init { c = Calc() }
sample { out1 = c(in1) }
"#;

#[test]
fn proc_local_def_expr_position_binary() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_EXPR_POSITION_BINARY_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![5.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // shifted(5.0) = 5.0 + 10.0 = 15.0; * 2.0 = 30.0
    for sample in &output {
        assert_near(*sample, 30.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_EXPR_POSITION_BUILTIN_ARG_EXAMPLE: &str = r#"
proc AbsHelper {
  ins 1
  outs 1

  init {
    bias = -3.0
  }

  def biased(x) {
    return x + bias
  }

  sample {
    out1 = abs(biased(in1))
  }
}

init { a = AbsHelper() }
sample { out1 = a(in1) }
"#;

#[test]
fn proc_local_def_expr_position_builtin_arg() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_EXPR_POSITION_BUILTIN_ARG_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // biased(1.0) = 1.0 + (-3.0) = -2.0; abs(-2.0) = 2.0
    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_EXPR_POSITION_TWO_CALLS_EXAMPLE: &str = r#"
proc Dual {
  ins 1
  outs 1

  init {
    a_offset = 1.0
    b_offset = 2.0
  }

  def add_a(x) {
    return x + a_offset
  }

  def add_b(x) {
    return x + b_offset
  }

  sample {
    out1 = add_a(in1) + add_b(in1)
  }
}

init { d = Dual() }
sample { out1 = d(in1) }
"#;

#[test]
fn proc_local_def_expr_position_two_calls() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_EXPR_POSITION_TWO_CALLS_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![10.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // add_a(10) = 11, add_b(10) = 12, sum = 23
    for sample in &output {
        assert_near(*sample, 23.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_EXPR_POSITION_NESTED_CALLS_EXAMPLE: &str = r#"
proc Chain {
  ins 1
  outs 1

  init {
    scale = 2.0
    bias = 1.0
  }

  def amplify(x) {
    return x * scale
  }

  def shift(x) {
    return x + bias
  }

  sample {
    out1 = shift(amplify(in1))
  }
}

init { c = Chain() }
sample { out1 = c(in1) }
"#;

#[test]
fn proc_local_def_expr_position_nested_calls() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_EXPR_POSITION_NESTED_CALLS_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![3.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // amplify(3.0) = 6.0; shift(6.0) = 7.0
    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_EXPR_POSITION_IF_COND_EXAMPLE: &str = r#"
proc Gated {
  ins 1
  outs 1

  init {
    threshold = 0.5
  }

  def above_threshold(x) {
    if (x > threshold) {
      return 1.0
    }
    return 0.0
  }

  events {
    set_threshold(v) {
      threshold = v
    }
  }

  sample {
    if (above_threshold(in1) > 0.5) {
      out1 = in1
    } else {
      out1 = 0.0
    }
  }
}

init { g = Gated() }
sample { out1 = g(in1) }
"#;

#[test]
fn proc_local_def_expr_position_in_if_condition() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_EXPR_POSITION_IF_COND_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.3, 0.8, 0.1, 0.9];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // threshold=0.5: 0.3 -> 0, 0.8 -> 0.8, 0.1 -> 0, 0.9 -> 0.9
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.8, 1e-6);
    assert_near(output[2], 0.0, 1e-6);
    assert_near(output[3], 0.9, 1e-6);
}

// ── proc-local defs: additional gap coverage ────────────────────────────────

const PROC_LOCAL_DEF_ARRAY_PARAM_EXAMPLE: &str = r#"
proc ArrayHelper {
  outs 1

  init {
    data: f32[4] = [1.0, 2.0, 3.0, 4.0]
  }

  def sum_array(arr: f32[]) {
    total = 0.0
    for i in 0..(arr.len()) {
      total = total + arr[i]
    }
    return total
  }

  sample {
    out1 = sum_array(data)
  }
}

init { h = ArrayHelper() }
sample { out1 = h() }
"#;

#[test]
fn proc_local_def_array_param() {
    let frames = 1;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_ARRAY_PARAM_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 10.0, 1e-6);
}

const PROC_LOCAL_DEF_BLOCK_SCOPE_EXAMPLE: &str = r#"
proc BlockProc {
  ins 1
  outs 1

  init {
    total = 0.0
  }

  def add_bias(x) {
    return x + 100.0
  }

  block {
    total = 0.0
    sample {
      total = total + add_bias(in1)
      out1 = total
    }
  }
}

init { b = BlockProc() }
sample { out1 = b(in1) }
"#;

#[test]
fn proc_local_def_called_from_block_scope() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_BLOCK_SCOPE_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // block pre: total = 0.0
    // Each sample: total += 1.0 + 100.0 = 101.0
    // Accumulated per sample: 101, 202, 303, 404
    assert_near(output[0], 101.0, 1e-6);
    assert_near(output[1], 202.0, 1e-6);
    assert_near(output[2], 303.0, 1e-6);
    assert_near(output[3], 404.0, 1e-6);
}

const PROC_LOCAL_DEF_CALLS_NAMESPACE_DEF_EXAMPLE: &str = r#"
def double(x) {
  return x * 2.0
}

proc Doubler {
  ins 1
  outs 1

  init {
    offset = 5.0
  }

  def process_sample(x) {
    return double(x) + offset
  }

  sample {
    out1 = process_sample(in1)
  }
}

init { d = Doubler() }
sample { out1 = d(in1) }
"#;

#[test]
fn proc_local_def_calls_namespace_def() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_CALLS_NAMESPACE_DEF_EXAMPLE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![3.0_f32; frames];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    // double(3.0) = 6.0; + 5.0 = 11.0
    for sample in &output {
        assert_near(*sample, 11.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_ORDER_INDEPENDENT_EXAMPLE: &str = r#"
proc OrderTest {
  outs 1

  init {
    value = 0.0
  }

  events {
    set(v) {
      apply(v)
    }
  }

  sample {
    out1 = value
  }

  def apply(v) {
    value = v * 3.0
  }
}

outs 1
events { set(v) { p.set(v) } }
init {
  p = OrderTest()
  p.set(2.0)
}
sample { out1 = p() }
"#;

#[test]
fn proc_local_def_order_independent() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_ORDER_INDEPENDENT_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // 2.0 * 3.0 = 6.0
    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_NOT_CALLABLE_FROM_OUTSIDE_EXAMPLE: &str = r#"
proc LocalOnly {
  outs 1

  def helper(v) {
    return v * 2.0
  }

  sample {
    out1 = helper(1.0)
  }
}

init {
  p = LocalOnly()
}

sample {
  out1 = helper(2.0) + p()
}
"#;

#[test]
fn proc_local_def_not_callable_from_outside_owner_proc() {
    let parsed =
        parse_program(PROC_LOCAL_DEF_NOT_CALLABLE_FROM_OUTSIDE_EXAMPLE).expect("parse succeeds");
    let errs = analyze(parsed).expect_err("outside call to proc-local def should fail");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("unknown function 'helper'")),
        "expected unknown-function error, got {errs:?}"
    );
}

const PROC_LOCAL_DEF_SHADOWS_TOP_LEVEL_DEF_EXAMPLE: &str = r#"
outs 2

def mix(v) {
  return v + 100.0
}

proc UsesLocal {
  outs 1

  def mix(v) {
    return v + 1.0
  }

  sample {
    out1 = mix(1.0)
  }
}

init {
  p = UsesLocal()
}

sample {
  out1 = p()
  out2 = mix(1.0)
}
"#;

#[test]
fn proc_local_def_name_prefers_local_resolution_over_top_level_def() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_SHADOWS_TOP_LEVEL_DEF_EXAMPLE, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in output.chunks_exact(2) {
        assert_near(frame[0], 2.0, 1e-6);
        assert_near(frame[1], 101.0, 1e-6);
    }
}

const PROC_LOCAL_DEF_SAME_NAME_PARENT_CHILD_EXAMPLE: &str = r#"
proc Child {
  outs 1

  init {
    value = 1.0
  }

  def bump() {
    value = value + 1.0
  }

  sample {
    bump()
    out1 = value
  }
}

proc Parent {
  outs 1

  init {
    child = Child()
    value = 10.0
  }

  def bump() {
    value = value + 10.0
  }

  sample {
    bump()
    out1 = value + child()
  }
}

init {
  p = Parent()
}

sample {
  out1 = p()
}
"#;

#[test]
fn proc_local_def_same_name_in_parent_and_child_proc_remains_isolated() {
    let frames = 4;
    let (mut instance, _, out_channels) =
        compile_instance(PROC_LOCAL_DEF_SAME_NAME_PARENT_CHILD_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 22.0, 1e-6);
    assert_near(output[1], 33.0, 1e-6);
    assert_near(output[2], 44.0, 1e-6);
    assert_near(output[3], 55.0, 1e-6);
}

const PROC_LOCAL_DEF_DOES_NOT_PARTICIPATE_IN_TOP_LEVEL_OVERLOADS_EXAMPLE: &str = r#"
def helper(v) {
  return v + 100.0
}

proc HiddenOverload {
  outs 1

  def helper(a, b) {
    return a + b
  }

  sample {
    out1 = helper(1.0, 2.0)
  }
}

init {
  p = HiddenOverload()
}

sample {
  out1 = helper(1.0, 2.0) + p()
}
"#;

#[test]
fn proc_local_defs_do_not_participate_in_top_level_overload_resolution() {
    let parsed = parse_program(PROC_LOCAL_DEF_DOES_NOT_PARTICIPATE_IN_TOP_LEVEL_OVERLOADS_EXAMPLE)
        .expect("parse succeeds");
    let errs =
        analyze(parsed).expect_err("proc-local def should not leak into top-level overloads");
    assert!(
        errs.iter()
            .any(|d| d.message.contains("too many positional arguments")),
        "expected top-level-only binding error, got {errs:?}"
    );
}

#[test]
fn slice_full_read_write() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_FULL_READ_WRITE_EXAMPLE, frames);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // All 4 elements set to 10.0
    assert_near(output[0], 40.0, 1e-6);
}

#[test]
fn slice_start_only() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_START_ONLY_EXAMPLE, frames);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // tail = values[2:] => [3.0, 4.0, 5.0], len=3
    // 3.0 + 4.0 + 5.0 + 3.0 = 15.0
    assert_near(output[0], 15.0, 1e-6);
}

#[test]
fn slice_negative_start() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_NEGATIVE_START_EXAMPLE, frames);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // tail = values[-2:] => [4.0, 5.0], len=2
    // 4.0 + 5.0 + 2.0 = 11.0
    assert_near(output[0], 11.0, 1e-6);
}

#[test]
fn slice_reverse_overlap() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_REVERSE_OVERLAP_EXAMPLE, frames);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // values = [1,2,3,4,5], values[:-1] = values[1:] => shift left
    // values becomes [2,3,4,5,5]
    // out1 = 2+3+4+5 = 14.0
    assert_near(output[0], 14.0, 1e-6);
}

#[test]
fn slice_as_def_argument() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_AS_DEF_ARG_EXAMPLE, frames);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // values[1:-1] => [2.0, 3.0, 4.0, 5.0]
    // sum = 14.0
    assert_near(output[0], 14.0, 1e-6);
}

#[test]
fn slice_in_event_handler() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(SLICE_IN_EVENT_EXAMPLE, frames);
    assert_eq!(out_channels, 1);

    // Trigger fill event with [10.0, 20.0, 30.0, 40.0]
    let mut payload = Vec::new();
    payload.extend_from_slice(&(4_i32).to_ne_bytes());
    payload.extend_from_slice(&10.0_f32.to_ne_bytes());
    payload.extend_from_slice(&20.0_f32.to_ne_bytes());
    payload.extend_from_slice(&30.0_f32.to_ne_bytes());
    payload.extend_from_slice(&40.0_f32.to_ne_bytes());
    trigger_event_by_index(&mut instance, 0, &payload).expect("fill event should succeed");

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    // total = 10+20+30+40 = 100.0
    assert_near(output[0], 100.0, 1e-6);
}

#[test]
fn port_index_outs_write_and_ins_read() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(PORT_INDEX_OUTS_WRITE, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 2);

    let mut input = vec![0.0_f32; frames * 2];
    for f in 0..frames {
        input[f * 2] = (f + 1) as f32;      // ch0
        input[f * 2 + 1] = (f + 10) as f32; // ch1
    }
    let mut output = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for f in 0..frames {
        assert_near(output[f * 2], (f + 1) as f32 * 2.0, 1e-6);
        assert_near(output[f * 2 + 1], (f + 10) as f32 * 3.0, 1e-6);
    }
}

#[test]
fn port_index_ins_dynamic_read() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(PORT_INDEX_INS_READ, frames);
    assert_eq!(in_channels, 4);
    assert_eq!(out_channels, 1);

    // Set idx param to 2.0 to select channel 2
    set_param_f32(&mut instance, "idx", 2.0);
    let mut input = vec![0.0_f32; frames * 4];
    for f in 0..frames {
        input[f * 4 + 0] = 10.0; // ch0
        input[f * 4 + 1] = 20.0; // ch1
        input[f * 4 + 2] = 30.0; // ch2
        input[f * 4 + 3] = 40.0; // ch3
    }
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for f in 0..frames {
        assert_near(output[f], 30.0, 1e-6); // should read ch2
    }
}

#[test]
fn port_index_params_dynamic_read() {
    let frames = 4;
    let (mut instance, _in_channels, out_channels) =
        compile_instance(PORT_INDEX_PARAMS_READ, frames);
    assert_eq!(out_channels, 1);

    set_param_f32(&mut instance, "a", 10.0);
    set_param_f32(&mut instance, "b", 20.0);
    set_param_f32(&mut instance, "c", 30.0);
    set_param_f32(&mut instance, "d", 40.0);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");

    // Frame 0: sel=0 → params[0]=a=10.0, then sel=1
    // Frame 1: sel=1 → params[1]=b=20.0, then sel=2
    // Frame 2: sel=2 → params[2]=c=30.0, then sel=3
    // Frame 3: sel=3 → params[3]=d=40.0, then sel=4 (clamped to 3 next time)
    assert_near(output[0], 10.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
    assert_near(output[2], 30.0, 1e-6);
    assert_near(output[3], 40.0, 1e-6);
}

#[test]
fn port_index_outs_loop_passthrough() {
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(PORT_INDEX_OUTS_LOOP, frames);
    assert_eq!(in_channels, 4);
    assert_eq!(out_channels, 4);

    let mut input = vec![0.0_f32; frames * 4];
    for f in 0..frames {
        for ch in 0..4 {
            input[f * 4 + ch] = ((ch + 1) * 10 + f) as f32;
        }
    }
    let mut output = vec![0.0_f32; frames * 4];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for f in 0..frames {
        for ch in 0..4 {
            let expected = ((ch + 1) * 10 + f) as f32 * 0.5;
            assert_near(output[f * 4 + ch], expected, 1e-6);
        }
    }
}

#[test]
fn port_index_ins_clamping() {
    // Verify that out-of-range indices are clamped
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(PORT_INDEX_INS_READ, frames);
    assert_eq!(in_channels, 4);
    assert_eq!(out_channels, 1);

    // Set idx to 100 (way out of range, should clamp to 3)
    set_param_f32(&mut instance, "idx", 100.0);
    let input = vec![10.0, 20.0, 30.0, 40.0_f32]; // one frame, 4 channels
    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 40.0, 1e-6); // clamped to last channel

    // Set idx to -5 (should clamp to 0)
    set_param_f32(&mut instance, "idx", -5.0);
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 10.0, 1e-6); // clamped to first channel
}

#[test]
fn port_index_rejects_inferred_ports() {
    // ins/outs without explicit block declaration should fail
    let src = r#"
sample {
  out1 = ins[0]
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(result.is_err(), "should reject ins[i] without explicit ins block");
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|d| d.message.contains("ins[i]") && d.message.contains("explicit")),
        "error should mention explicit block requirement: {:?}",
        errors
    );
}

#[test]
fn section_count_const_outs_compiles_and_runs() {
    let src = r#"
const N = 2
ins N
outs N
sample {
  out1 = in1 * 2.0
  out2 = in2 * 3.0
}
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 2);
    assert_eq!(out_channels, 2);

    let input = vec![1.0_f32, 10.0, 2.0, 20.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 2.0, 1e-6);
    assert_near(output[1], 30.0, 1e-6);
    assert_near(output[2], 4.0, 1e-6);
    assert_near(output[3], 60.0, 1e-6);
}

#[test]
fn section_count_const_params_compiles_and_runs() {
    let src = r#"
const NUM_PARAMS = 2
outs 1
params NUM_PARAMS
sample {
  out1 = param1 + param2
}
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    set_param_f32(&mut instance, "param1", 3.0);
    set_param_f32(&mut instance, "param2", 7.0);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 10.0, 1e-6);
}

#[test]
fn section_count_expr_outs_compiles_and_runs() {
    let src = r#"
const N = 1
outs (N + 1)
sample {
  out1 = 5.0
  out2 = 10.0
}
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 5.0, 1e-6);
    assert_near(output[1], 10.0, 1e-6);
}

#[test]
fn section_count_namespace_generic_proc_outs_compiles_and_runs() {
    let src = r#"
namespace Synth<Num = 2>:
  proc Voice:
    ins Num
    outs Num
    sample:
      for i in 0..Num:
        outs[i] = ins[i] * 2.0

outs 2
init:
  v = Synth<2>::Voice()
sample:
  out1 = v(1.0, 10.0).out1
  out2 = v.out2
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 2.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
}

#[test]
fn section_count_namespace_generic_proc_default_param_compiles_and_runs() {
    let src = r#"
namespace FX<N = 4>:
  proc Mixer:
    ins N
    outs 1
    sample:
      sum = 0.0
      for i in 0..N:
        sum = sum + ins[i]
      out1 = sum

outs 1
init:
  m = FX<3>::Mixer()
sample:
  out1 = m(1.0, 2.0, 3.0).out1
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 6.0, 1e-6);
}

#[test]
fn section_count_const_with_default_type_compiles_and_runs() {
    let src = r#"
const N = 2
ins N
outs<f64> N
sample {
  for i in 0..N:
    outs[i] = ins[i] * 2.0
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let input = vec![1.5_f32, 2.5];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 3.0, 1e-6);
    assert_near(output[1], 5.0, 1e-6);
}

#[test]
fn proc_port_index_outs_i_in_sample_compiles_and_runs() {
    let src = r#"
proc Voice:
  ins 2
  outs 2
  sample:
    for i in 0..2:
      outs[i] = ins[i] * 2.0

outs 2
init:
  v = Voice()
sample:
  out1 = v(1.0, 10.0).out1
  out2 = v.out2
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 2.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
}

#[test]
fn proc_port_index_params_i_in_sample_compiles_and_runs() {
    let src = r#"
proc Gain:
  ins 2
  outs 2
  params 2
  sample:
    for i in 0..2:
      outs[i] = ins[i] * params[i]

outs 2
init:
  g = Gain()
  g.param1 = 3.0
  g.param2 = 5.0
sample:
  out1 = g(1.0, 10.0).out1
  out2 = g.out2
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 3.0, 1e-6);
    assert_near(output[1], 50.0, 1e-6);
}

#[test]
fn section_count_namespace_generic_proc_params_i_compiles_and_runs() {
    let src = r#"
namespace FX<N = 2>:
  proc WeightedSum:
    ins N
    outs 1
    params N
    sample:
      sum = 0.0
      for i in 0..N:
        sum = sum + ins[i] * params[i]
      out1 = sum

outs 1
init:
  w = FX<3>::WeightedSum()
  w.param1 = 1.0
  w.param2 = 2.0
  w.param3 = 3.0
sample:
  out1 = w(10.0, 20.0, 30.0).out1
"#;
    let frames = 2;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    // 10*1 + 20*2 + 30*3 = 10 + 40 + 90 = 140
    assert_near(output[0], 140.0, 1e-6);
}

#[test]
fn section_count_const_top_level_dynamic_ins_outs_compiles_and_runs() {
    let src = r#"
const N = 3
ins N
outs N
sample:
  for i in 0..N:
    outs[i] = ins[i] + 1.0
"#;
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 3);
    assert_eq!(out_channels, 3);

    let input = vec![10.0_f32, 20.0, 30.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 11.0, 1e-6);
    assert_near(output[1], 21.0, 1e-6);
    assert_near(output[2], 31.0, 1e-6);
}

#[test]
fn section_count_const_top_level_dynamic_params_compiles_and_runs() {
    let src = r#"
const N = 3
outs 1
params N
sample:
  sum = 0.0
  for i in 0..N:
    sum = sum + params[i]
  out1 = sum
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    set_param_f32(&mut instance, "param1", 5.0);
    set_param_f32(&mut instance, "param2", 15.0);
    set_param_f32(&mut instance, "param3", 25.0);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 45.0, 1e-6);
}

#[test]
fn struct_tuple_field_basic() {
    let src = r#"
outs { out1 }
struct Foo { pair: (f32, f32) = (0.25, 0.75) }
init {
  foo = Foo()
}
sample {
  out1 = foo.pair[0] + foo.pair[1]
}
"#;
    let frames = 4;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]
fn struct_tuple_field_write() {
    let src = r#"
outs { out1 }
struct Foo { pair: (f32, f32) = (0.0, 0.0) }
init {
  foo = Foo()
}
sample {
  foo.pair[0] = foo.pair[0] + 1.0
  out1 = foo.pair[0]
}
"#;
    let frames = 4;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
    assert_near(output[3], 4.0, 1e-6);
}

#[test]
fn struct_tuple_field_mixed_types() {
    let src = r#"
outs { out1 }
struct Foo { pair: (f32, i32) = (0.5, 3) }
init {
  foo = Foo()
}
sample {
  out1 = foo.pair[0] + f32(foo.pair[1])
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 3.5, 1e-6);
}
