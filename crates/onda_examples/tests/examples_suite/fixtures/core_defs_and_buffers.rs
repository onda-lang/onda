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
  acc = 0.0;
  for i in 0..4 { acc = acc + i / 10.0 }
  out1 = acc
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
  acc = 0.0
  loop 4 {
    acc = acc + 1.0
  }
  out1 = acc
}
"#;

const FOR_VAR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 4
}
sample {
  acc = 0.0
  for i in 0..n {
    acc = acc + 1.0
  }
  out1 = acc
}
"#;

const FOR_PAREN_EXPR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 5
}
sample {
  acc = 0.0
  for i in 0..(n - 1) {
    acc = acc + 1.0
  }
  out1 = acc
}
"#;

const FOR_DESCENDING_STEP_EXAMPLE: &str = r#"
outs { out1 }
sample {
  acc = 0.0
  for i @ -1 in 3..=1 {
    acc = acc + i / 10.0
  }
  out1 = acc
}
"#;

const LOOP_VAR_BOUND_EXAMPLE: &str = r#"
outs { out1 }
init {
  n: i32 = 4
}
sample {
  acc = 0.0
  loop n {
    acc = acc + 1.0
  }
  out1 = acc
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

const BUFFER_STEREO_UNSAFE_2D_RW_EXAMPLE: &str = r#"
buffers {
  buf1: buffer[f32[2]]
}
outs {
  out1
}
init {
  idx: i32 = 1
}
sample {
  unsafe_write2(buf1, 1, idx, 11.0)
  buf1.unsafe_write2(1, idx, unsafe_read2(buf1, 1, idx) + 2.0)
  out1 = buf1.unsafe_read2(1, idx)
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
