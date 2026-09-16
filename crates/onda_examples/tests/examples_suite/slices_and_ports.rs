use super::*;

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

    trigger_event_by_index(
        &mut instance,
        0,
        &payload,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("fill event should succeed");

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
        input[f * 2] = (f + 1) as f32; // ch0

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
        input[f * 4] = 10.0; // ch0

        input[f * 4 + 1] = 20.0; // ch1

        input[f * 4 + 2] = 30.0; // ch2

        input[f * 4 + 3] = 40.0; // ch3
    }

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for sample in output.iter().take(frames) {
        assert_near(*sample, 30.0, 1e-6); // should read ch2
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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    assert!(
        result.is_err(),
        "should reject ins[i] without explicit ins block"
    );

    let errors = result.unwrap_err();

    assert!(
        errors
            .iter()
            .any(|d| d.message.contains("ins[i]") && d.message.contains("explicit")),
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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

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

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 3.5, 1e-6);
}

#[test]
fn deep_compound_assignment_places_support_every_operator_and_single_evaluation() {
    let src = r#"
struct Leaf:
  scalar: i32
  values: i32[10]

struct Mid:
  leaf: Leaf
  leaves: Leaf[2]

struct State:
  phase: Mid[2]
  direct: Leaf[1]

proc Worker:
  outs 1

  init:
    state: State
    selections = 0
    initial = state.phase[0]
    initial.leaf.scalar = 3
    initial.leaf.values[0] = 10
    initial.leaf.values[1] = 10
    initial.leaf.values[2] = 6
    initial.leaf.values[3] = 20
    initial.leaf.values[4] = 20
    initial.leaf.values[5] = 6
    initial.leaf.values[6] = 4
    initial.leaf.values[7] = 7
    initial.leaf.values[8] = 1
    initial.leaf.values[9] = 32
    nested = initial.leaves[1]
    nested.scalar = 2
    nested.values[0] = 1
    direct = state.direct[0]
    direct.scalar = 5

  def select():
    selections += 1
    return 0

  def select_leaf():
    selections += 1
    return 1

  sample:
    state.phase[select()].leaf.values[0] += 5
    state.phase[0].leaf.values[1] -= 3
    state.phase[0].leaf.values[2] *= 7
    state.phase[0].leaf.values[3] /= 4
    state.phase[0].leaf.values[4] %= 6
    state.phase[0].leaf.values[5] &= 3
    state.phase[0].leaf.values[6] |= 1
    state.phase[0].leaf.values[7] ^= 3
    state.phase[0].leaf.values[8] <<= 4
    state.phase[0].leaf.values[9] >>= 3
    state.phase[0].leaf.scalar += 4
    state.phase[0].leaves[1].scalar *= 3
    state.phase[0].leaves[select_leaf()].values[0] += 10
    state.direct[0].scalar += 2

    current = state.phase[0]
    current_nested = current.leaves[1]
    current_direct = state.direct[0]
    total = selections
    for i in 0..10:
      total += current.leaf.values[i]
    total += current.leaf.scalar + current_nested.scalar + current_nested.values[0]
    total += current_direct.scalar
    out1 = f32(total)

init:
  worker = Worker()

sample:
  out1 = worker()
"#;

    let (mut instance, _, _) = compile_instance(src, 1);
    let mut output = [0.0_f32];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 135.0, 1e-6);
}

#[test]
fn deep_plain_assignment_places_evaluate_selectors_before_the_value() {
    let src = r#"
struct Counter:
  value: i32 = 0

struct Leaf:
  gain: f32 = 0.0

struct Middle:
  leaves: Leaf[2]

struct Root:
  middles: Middle[2]

def next(counter: Counter) -> i32:
  result = counter.value
  counter.value += 1
  return result

sample:
  counter = Counter()
  roots: Root[2]
  roots[next(counter)].middles[next(counter)].leaves[next(counter)].gain = f32(next(counter) + 10)
  root = roots[0]
  middle = root.middles[1]
  leaf = middle.leaves[1]
  out1 = leaf.gain + f32(counter.value)
"#;

    let (mut instance, _, _) = compile_instance(src, 1);
    let mut output = [0.0_f32];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 17.0, 1e-6);
}

#[test]
fn indexed_proc_fields_and_mutating_rhs_preserve_compound_place_selection() {
    let src = r#"
proc Voice:
  params:
    gain = 1.0

  outs 1

  sample:
    out1 = gain

proc Bank:
  outs 1

  init:
    values: i32[2] = [10, 20]
    cursor = 0

  def move_cursor():
    cursor = 1
    return 5

  sample:
    values[cursor] += move_cursor()
    out1 = f32(values[0] + values[1])

init:
  voices: Voice[2] = Voice()
  selector = 0
  bank = Bank()

sample:
  voices[selector].gain += 2.0
  out1 = voices[0]() + bank()
"#;

    let (mut instance, _, _) = compile_instance(src, 1);
    let mut output = [0.0_f32];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 38.0, 1e-6);
}

#[test]
fn nested_proc_array_fields_support_compound_assignment() {
    let src = r#"
proc Voice:
  params:
    gain = 1.0

  outs 1

  sample:
    out1 = gain

proc Bank:
  outs 1

  init:
    voices: Voice[2] = Voice()
    selector = 0

  sample:
    voices[selector].gain += 2.0
    out1 = voices[0]()

init:
  bank = Bank()

sample:
  out1 = bank()
"#;

    let (mut instance, _, _) = compile_instance(src, 1);
    let mut output = [0.0_f32];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 3.0, 1e-6);
}
