use super::*;

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

const PROC_LOCAL_DEF_NESTED_PROC_CALL_EXAMPLE: &str = r#"

proc Child {

  outs 1



  init {

    value = 0.75

  }



  sample {

    out1 = value

  }

}



proc Parent {

  outs 1



  init {

    child = Child()

  }



  def run_child() {

    return child()

  }



  sample {

    out1 = run_child()

  }

}



init { p = Parent() }

sample { out1 = p() }

"#;

#[test]

fn proc_local_def_can_call_nested_proc_operator() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_LOCAL_DEF_NESTED_PROC_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

const PROC_LOCAL_DEF_NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOK_EXAMPLE: &str = r#"

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



  def run_selected() {

    voices[idx]()

    return 0.0

  }



  sample {

    x = run_selected()

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

#[test]

fn proc_local_def_nested_proc_array_dynamic_call_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_LOCAL_DEF_NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOK_EXAMPLE,
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
