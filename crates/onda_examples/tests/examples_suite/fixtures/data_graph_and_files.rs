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
  left = 0.25
  out1[0] = left
  out1[1] = left + 0.5
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
    include_str!("../../../../../examples/buffers-fft-convolution/multitap_feedback_struct_data.onda");
const PROC_GAIN_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/processors-and-graphs/proc_gain_graph.onda");
const PROC_SPLIT_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/processors-and-graphs/proc_split_graph.onda");
const PROC_ARRAY_STEREO_SINE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/processors-and-graphs/proc_array_stereo_sine_graph.onda");
const FEEDBACK_SATURATOR_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/processors-and-graphs/feedback_saturator_graph.onda");
const STD_ONE_POLE_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/standard-library/std_one_pole.onda");
const STD_ONE_POLE_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/standard-library/std_one_pole_graph.onda");
const STDLIB_F32_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/standard-library/std_f32.onda");
const STDLIB_F32_GRAPH_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/standard-library/std_f32_graph.onda");
const WASM_PLAYGROUND_FILE_EXAMPLE: &str =
    include_str!("../../../../../examples/web/onda_wasm_playground/default.onda");
