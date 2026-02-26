use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use omni_codegen_llvm::{CompileOptions, ExecutionBackend};
use omni_examples::{GAIN, ONE_POLE, SINE};
use omni_frontend::{parse_program, parse_program_file, Diagnostic, PrimitiveType};
use omni_runtime::{
    bind_buffer, bind_input, bind_output, create_instance, process_bound, process_unchecked,
    set_param_by_index, trigger_event_by_index, validate_bindings, validate_buffers,
    validate_outputs, InstanceConfig,
};
use omni_semantics::{analyze, analyze_with_options, AnalysisOptions};

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
  out1 = PI + TWO_PI + SAMPLE_RATE - SR
}
"#;

const BUILTIN_CONSTS_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + TWO_PI / SR
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_SAMPLERATE_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + TWO_PI / SAMPLERATE
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_LOWERCASE_ALIASES_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = pi + two_pi + twopi + samplerate - sample_rate + blocksize - block_size
}
"#;

const BUILTIN_CONSTS_LOWERCASE_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + twopi / samplerate
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
  incr = freq * TWO_PI / SR
  sample {
    phase = phase + incr
    if (phase > TWO_PI) { phase = phase - TWO_PI }
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
  out1 = id[f32](1.0)
}
"#;

const GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair[T] { a: T, b: T }
init {
  p = Pair[f64](f64(1.25), f64(0.5))
}
sample {
  out1 = f32(p.a + p.b)
}
"#;

const GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair[T] { a: T, b: T }
init {
  p = Pair(1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box[T] { v: T }
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
struct Bank[T] { taps: T[2] }
init {
  b = Bank()
}
sample {
  out1 = 0.0
}
"#;

const GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair[T] { a: T, b: T }
init {
  p = Pair[f32, f64](1.0, 2.0)
}
sample {
  out1 = 0.0
}
"#;

const NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair[f32](1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box[T] { v: T }
init {
  a = Box[f32](1.0)
  b = Box[f64](f64(0.25))
}
sample {
  out1 = a.v + f32(b.v)
}
"#;

const GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Bank[T] { taps: T[2] }
init {
  b = Bank[f64]()
  b.taps[0.0] = f64(1.5)
  b.taps[1.0] = f64(0.5)
}
sample {
  out1 = f32(b.taps[0.0] + b.taps[1.0])
}
"#;

const GENERIC_STRUCT_METHOD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair[T] {
  a: T
  b: T
  def sum(self) {
    return self.a + self.b
  }
}
init {
  p = Pair[f64](f64(1.25), f64(0.75))
}
sample {
  out1 = f32(p.sum())
}
"#;

const GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
proc Gain[T] {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain[f64](g = f64(0.5))
}
sample {
  out1 = f32(p(f64(2.0)))
}
"#;

const GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
proc Gain[T] {
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
proc Gain[T] {
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
proc Tap[T] {
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
proc Hold[T] {
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
struct Pair[T] { a: T, b: T }

proc Voice {
  outs { out1 }
  init {
    s = Pair[f64](f64(1.0), f64(2.0))
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
struct Pair[T] { a: T, b: T }

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
proc Gain[T, U] {
  ins { in1: T }
  outs { out1: T }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  p = Gain[f64]()
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
  p = Gain[f64]()
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
proc Gain[T] {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p1 = Gain[f32](g = 2.0)
  p2 = Gain[f64](g = f64(0.25))
}
sample {
  out1 = p1(1.0) + f32(p2(f64(2.0)))
}
"#;

const GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE: &str = r#"
proc Mix[T] {
  ins { in1: T[2] }
  outs { out1: T }
  params { gains: T[2] = [1.0, 0.5] }
  sample {
    out1 = in1[0] * gains[0] + in1[1] * gains[1]
  }
}
outs { out1 }
init {
  p = Mix[f64]()
}
sample {
  out1 = f32(p([f64(2.0), f64(4.0)]))
}
"#;

const GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE: &str = r#"
proc Sum2[T] {
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
  p = Sum2[f64]()
}
sample {
  out1 = f32(p())
}
"#;

const GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE: &str = r#"
buffers { buf1: buffer[f64] }
proc Tap[T] {
  buffers { line: buffer[T] }
  outs { out1: T }
  sample {
    out1 = line[0]
  }
}
outs { out1 }
init {
  p = Tap[f64](line = buf1)
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

const PROC_INSTANCE_ARRAY_INDEXED_CALL_NON_LITERAL_ERROR_EXAMPLE: &str = r#"
proc Voice {
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 1
}
sample {
  out1 = voices[idx](0.5)
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
    self.phase = self.phase + hz * TWO_PI / SR
    if (self.phase >= TWO_PI) { self.phase = self.phase - TWO_PI }
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
    phase = phase + (freq * TWO_PI / SR)
    if (phase >= TWO_PI) {
      phase = phase - TWO_PI
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
    phase = phase + (freq * TWO_PI / SR)
    if (phase >= TWO_PI) {
      phase = phase - TWO_PI
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

fn assert_near(a: f32, b: f32, eps: f32) {
    let delta = (a - b).abs();
    assert!(delta <= eps, "expected {a} ~= {b}, delta={delta}");
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
        result.is_err(),
        "semantic analysis should reject primitive Data alias binding via 'x = buf[i]'"
    );
}

#[test]
fn primitive_struct_field_data_local_alias_binding_is_rejected() {
    let parsed =
        parse_program(STRUCT_DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject primitive struct Data alias binding via 'x = v.delay[i]'"
    );
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
fn typed_data_primitive_elements_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    let mut out = vec![0.0_f32; out_channels];
    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");
    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "typed Data elements should preserve runtime values across primitive types"
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
        "semantic analysis should reject bool Data index"
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
        "codegen should reject out-of-range constant Data index"
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
    assert_eq!(mydef.return_ty, PrimitiveType::F64);
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
def bad[T](x) {
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
fn generic_struct_ctor_reports_unresolved_inference() {
    let parsed = parse_program(GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor calls when type inference cannot resolve all parameters"
    );
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
fn generic_proc_ctor_reports_unresolved_inference() {
    let parsed = parse_program(GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject generic proc ctor calls when type inference cannot resolve all parameters"
    );
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
        "codegen should succeed for generic proc buffer[T] specialization"
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
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 2.3968143, 1e-6);
    assert_near(output[1], 6.394737, 1e-6);
    assert_near(output[2], 10.394737, 1e-6);
    assert_near(output[3], 14.394737, 1e-6);
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
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_near(output[0], 0.75, 1e-6);
    assert_near(output[1], 2.4240382, 1e-6);
    assert_near(output[2], 4.3591337, 1e-6);
    assert_near(output[3], 6.3475933, 1e-6);
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
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![0.0_f32, 1.0, 2.0, 3.0];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.651605, 1e-6);
    assert_near(output[2], 1.6447935, 1e-6);
    assert_near(output[3], 2.6447372, 1e-6);
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
fn proc_instance_array_indexed_call_non_literal_index_is_rejected() {
    let parsed = parse_program(PROC_INSTANCE_ARRAY_INDEXED_CALL_NON_LITERAL_ERROR_EXAMPLE)
        .expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "semantic analysis should reject non-literal processor-array call indices"
    );
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
