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

