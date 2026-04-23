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
