use super::*;
use onda_runtime::ExecutionOutput;

fn bind_outputs(instance: &mut Instance, count: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut outputs = vec![vec![0.0_f32; frames]; count];
    for (index, output) in outputs.iter_mut().enumerate() {
        bind_output(
            instance,
            index,
            output.as_mut_ptr().cast(),
            output.len() * 4,
        )
        .expect("bind output");
    }
    outputs
}

#[test]
fn main_input_and_param_names_follow_ordinary_proc_calls() {
    let body = r#"
  ins:
    gain = 0.2
  params:
    gain = 0.5
  sample:
    out1 = ins[0] * gain
"#;
    let connections = r#"
ins:
  signal
params:
  gain = 0.5
init:
  voice = Voice(gain = gain)
sample:
  out1 = voice(signal)
"#;
    let input = [-1.0_f32, 0.0, 0.5, 2.0];
    for (name, connections) in [("Voice", connections), ("Main", "")] {
        let (mut instance, _, _) =
            compile_instance(&format!("proc {name}:\n{body}\n{connections}"), 4);
        assert_eq!(instance.param_name(0), Some("gain"));
        bind_input(&mut instance, 0, input.as_ptr().cast(), 16).expect("input");
        let outputs = bind_outputs(&mut instance, 1, 4);
        process_checked(&mut instance, 4, ExecutionOutput::none()).expect("process");
        assert_eq!(outputs[0], [-0.5, 0.0, 0.25, 1.0], "{name}");
    }
}

#[test]
fn main_input_ranges_follow_ordinary_proc_calls() {
    let connections = r#"
ins:
  signal = 0.5
  values: f32[2] = [0.25, 0.75]
  single: f32[1] = 0.5
init:
  voice = Voice()
sample:
  out1, out2, out3 = voice(signal, values, single)
"#;
    let signal = [-2.0_f32, -0.5, 0.5, f32::NAN];
    let values = [
        -2.0_f32,
        -0.5,
        0.5,
        f32::NAN,
        2.0,
        f32::NEG_INFINITY,
        f32::INFINITY,
        0.25,
    ];
    let single = [-0.5_f32, 0.25, 2.0, f32::NAN];
    for factor in [1, 4] {
        let body = format!(
            r#"
  ins:
    signal = 0.5 {{-1.0, 1.0}}
    values: f32[2] = [0.25, 0.75] {{-1.0, 1.0}}
    single: f32[1] = 0.5 {{0.0, 1.0}}
  sample {factor}:
    out1 = signal
    out2 = values[0] + values[1]
    out3 = single[0]
"#
        );
        let mut rendered = Vec::new();
        for (name, connections) in [("Voice", connections), ("Main", "")] {
            let (mut instance, _, _) =
                compile_instance(&format!("proc {name}:\n{body}\n{connections}"), 4);
            assert_eq!(instance.input_name(1), Some("values"));
            assert_eq!(instance.input_type(1).as_deref(), Some("f32[2]"));
            assert_eq!(instance.input_type(2).as_deref(), Some("f32[1]"));
            for (index, input) in [&signal[..], &values[..], &single[..]]
                .into_iter()
                .enumerate()
            {
                bind_input(
                    &mut instance,
                    index,
                    input.as_ptr().cast(),
                    std::mem::size_of_val(input),
                )
                .expect("input");
            }
            let outputs = bind_outputs(&mut instance, 3, 4);
            process_checked(&mut instance, 4, ExecutionOutput::none()).expect("process");
            rendered.push(outputs);
        }
        if factor == 1 {
            assert_eq!(rendered[0][0], [-1.0, -0.5, 0.5, -1.0]);
            assert_eq!(rendered[0][1], [0.0, -1.5, 1.5, -0.75]);
            assert_eq!(rendered[0][2], [0.0, 0.25, 1.0, 0.0]);
        }
        assert_eq!(rendered[0], rendered[1], "oversampling factor {factor}");
    }
}

#[test]
fn main_refreshes_before_block_pre_and_events_without_refreshing_segments() {
    let source = r#"
proc Main:
  params:
    gain = 0.25 {0.0, 1.0} => changed
  outs 4
  init:
    calls: i32 = 0
    recorded = 0.0
  def changed():
    calls = calls + 1
  def current_gain():
    return gain
  event record(gain: f32 = 10.0):
    recorded = current_gain() + gain
  block:
    seen = gain
    sample:
      out1 = seen
      out2 = gain
      out3 = calls
      out4 = recorded
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    let mut outputs = bind_outputs(&mut instance, 4, 4);
    set_param_f32(&mut instance, "gain", 0.5);
    process_checked_segment(
        &mut instance,
        0,
        2,
        PROCESSOR_BEGIN_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("begin");
    set_param_f32(&mut instance, "gain", 0.75);
    process_checked_segment(
        &mut instance,
        2,
        2,
        PROCESSOR_END_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("end");
    assert_eq!(outputs[0], [0.5; 4]);
    assert_eq!(outputs[1], [0.5; 4]);
    assert_eq!(outputs[2], [2.0; 4]);
    let event = instance.event_index("record").expect("event");
    trigger_event_by_index_checked(
        &mut instance,
        event,
        &10.0_f32.to_ne_bytes(),
        ExecutionOutput::none(),
    )
    .expect("event dispatch");
    process_checked(&mut instance, 4, ExecutionOutput::none()).expect("next block");
    assert_eq!(outputs[0], [0.75; 4]);
    assert_eq!(outputs[2], [4.0; 4]);
    assert_eq!(outputs[3], [10.75; 4]);
    outputs.iter_mut().for_each(|output| output.fill(0.0));
    process_checked(&mut instance, 4, ExecutionOutput::none()).expect("equal control refresh");
    assert_eq!(outputs[2], [5.0; 4]);
}

#[test]
fn main_array_controls_audio_ports_and_helpers_use_ordinary_proc_semantics() {
    let source = r#"
proc Main:
  ins:
    input: f32[2]
  outs:
    output: f32[2]
  params:
    gains: f32[2] = [0.25, 0.75] {0.0, 1.0, unit = "x"}
    private offset = 1.0
  init:
    count = 0.0
  def advance():
    count = count + offset
    return count
  sample:
    value = advance()
    output[0] = input[0] * gains[0] + value
    output[1] = input[1] * gains[1] + value
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    let input = [2.0_f32, 2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0];
    bind_input(&mut instance, 0, input.as_ptr().cast(), 32).expect("input array");
    let mut output = [0.0_f32; 8];
    bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 32).expect("output array");
    assert!(instance.param_index("offset").is_none());
    set_param_f32_array(&mut instance, "gains", &[0.5, 2.0]);
    process_checked(&mut instance, 4, ExecutionOutput::none()).expect("process");
    assert_eq!(output, [2.0, 3.0, 4.0, 5.0, 5.0, 6.0, 7.0, 8.0]);
}

#[test]
fn main_preserves_pinned_state_and_constructs_event_only_processors() {
    let source = r#"
proc Main:
  init:
    pin saved = 0.0
    ordinary = 0.0
  def advance():
    saved = saved + 1.0
    ordinary = ordinary + 1.0
    return saved + ordinary
  sample:
    out1 = advance()
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 1, 2);
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("first");
    assert_eq!(outputs[0], [2.0, 4.0]);
    init_checked(&mut instance, InitMode::PreservePinned).expect("preserve");
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("preserved");
    assert_eq!(outputs[0], [4.0, 6.0]);
    init_checked(&mut instance, InitMode::Full).expect("full");
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("reset");
    assert_eq!(outputs[0], [2.0, 4.0]);

    let (mut event_only, _, _) =
        compile_instance("proc Main:\n  outs 1\n  event reset():\n    print(1)\n", 2);
    let outputs = bind_outputs(&mut event_only, 1, 2);
    process_checked(&mut event_only, 2, ExecutionOutput::none()).expect("neutral process");
    assert_eq!(outputs[0], [0.0; 2]);
}

#[test]
fn main_forwards_delegates_and_live_buffer_bindings_hygienically() {
    let source = r#"
def changed(value):
  return value * 2.0
proc Main:
  buffers:
    data: f32
  init:
    value = data[0]
  delegate changed(value: f32)
  event read():
    value = data[0]
    changed(value)
  sample:
    out1 = value
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 1, 2);
    let mut first = [0.25_f32];
    let mut second = [0.75_f32];
    bind_buffer(
        &mut instance,
        0,
        first.as_mut_ptr().cast(),
        1,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("first buffer");
    init_checked(&mut instance, InitMode::Full).expect("initialize against bound buffer");
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("first");
    assert_eq!(outputs[0], [0.25; 2]);
    bind_buffer(
        &mut instance,
        0,
        second.as_mut_ptr().cast(),
        1,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("replacement buffer");
    let mut storage = [0_u8; 64];
    let mut batch = onda_runtime::DelegateBatch::from_storage(&mut storage);
    let read = instance.event_index("read").expect("event");
    trigger_event_by_index_checked(
        &mut instance,
        read,
        &[],
        ExecutionOutput {
            delegate_batch: Some(&mut batch),
            print_batch: None,
        },
    )
    .expect("read and publish");
    assert_eq!(instance.delegate_name(0), Some("changed"));
    assert_eq!(batch.record_count, 1);
    assert_eq!(
        batch
            .occurrence(0)
            .expect("valid batch")
            .expect("occurrence")
            .payload,
        0.75_f32.to_le_bytes()
    );
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("rebound");
    assert_eq!(outputs[0], [0.75; 2]);
}

#[test]
fn main_tasks_keep_ordinary_continuations_and_neutral_output() {
    let source = r#"
proc Main:
  params:
    gain = 0.25
  init:
    prepared = 0.0
  task prepare():
    prepared = gain
    yield
    prepared = prepared + gain
  block:
    await prepare()
    sample:
      out1 = prepared
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 1, 2);
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("yield");
    assert_eq!(outputs[0], [0.0; 2]);
    set_param_f32(&mut instance, "gain", 0.75);
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("resume");
    assert_eq!(outputs[0], [1.0; 2]);
}

#[test]
fn main_control_processors_step_once_per_logical_block() {
    let source = r#"
proc Main:
  params:
    step = 0.25
  init:
    count = 0.0
  block:
    count = count + step
    kout1 = count
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    process_checked_segment(
        &mut instance,
        0,
        2,
        PROCESSOR_BEGIN_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("begin");
    process_checked_segment(
        &mut instance,
        2,
        2,
        PROCESSOR_END_BLOCK,
        ExecutionOutput::none(),
    )
    .expect("end");
    let mut bytes = [0_u8; 4];
    onda_runtime::read_control_output_bytes(&instance, 0, &mut bytes).expect("control output");
    assert_eq!(f32::from_ne_bytes(bytes), 0.25);
    set_param_f32(&mut instance, "step", 0.5);
    process_checked(&mut instance, 4, ExecutionOutput::none()).expect("next");
    onda_runtime::read_control_output_bytes(&instance, 0, &mut bytes).expect("control output");
    assert_eq!(f32::from_ne_bytes(bytes), 0.75);
}

#[test]
fn main_nested_event_arguments_shadow_the_childs_parameters() {
    let source = r#"
proc Child:
  params:
    gain = 0.25
  init:
    value = 0.0
  event set(gain: f32):
    value = gain
  sample:
    out1 = value
proc Main:
  init:
    child = Child()
  event set(gain: f32):
    child.set(gain)
  sample:
    out1 = child()
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 1, 2);
    let event = instance.event_index("set").expect("event");
    trigger_event_by_index_checked(
        &mut instance,
        event,
        &2.0_f32.to_ne_bytes(),
        ExecutionOutput::none(),
    )
    .expect("nested event");
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("process");
    assert_eq!(outputs[0], [2.0; 2]);
}

#[test]
fn main_hooks_execute_in_the_processors_oversampled_context() {
    let source = r#"
proc Main:
  params:
    gain = 0.25 => changed
  init:
    rate = 0.0
  def changed():
    rate = SR
  delegate measured(rate: f32)
  event measure():
    measured(rate)
  sample 4:
    out1 = 0.0
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let mut storage = [0_u8; 64];
    let mut batch = onda_runtime::DelegateBatch::from_storage(&mut storage);
    let event = instance.event_index("measure").expect("event");
    trigger_event_by_index_checked(
        &mut instance,
        event,
        &[],
        ExecutionOutput {
            delegate_batch: Some(&mut batch),
            print_batch: None,
        },
    )
    .expect("measure");
    let occurrence = batch
        .occurrence(0)
        .expect("valid batch")
        .expect("measurement");
    assert_eq!(occurrence.payload, 192_000.0_f32.to_le_bytes());
}

#[test]
fn main_interface_bindings_preserve_module_names_and_public_descriptors() {
    let source = r#"
def gain(value):
  return value * 2.0
def input(value):
  return gain(value)
def output(value):
  return input(value)
def data(value):
  return output(value)
def apply(value):
  return data(value)
proc Main:
  ins:
    input: f32[1]
  outs:
    output: f32[1]
  params:
    gain: f32[1] = [0.5]
  buffers:
    data: f32 {1}
  sample:
    output[0] = apply(input[0] * gain[0] + data[0][0])
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let input = [2.0_f32; 2];
    bind_input(&mut instance, 0, input.as_ptr().cast(), 8).expect("input array");
    let outputs = bind_outputs(&mut instance, 1, 2);
    let mut data = [0.25_f32];
    bind_buffer(
        &mut instance,
        0,
        data.as_mut_ptr().cast(),
        1,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("buffer array");
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("process");
    assert_eq!(outputs[0], [2.5; 2]);
    set_param_f32_array(&mut instance, "gain", &[0.75]);
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("refresh");
    assert_eq!(outputs[0], [3.5; 2]);
}

#[test]
fn main_typed_parameter_arrays_preserve_constructor_and_refresh_values() {
    let source = r#"
enum Mode:
  Off
  On
proc Main:
  params:
    integers: i32[2] = [16777217, 16777219]
    wide: i64[2] = [9007199254740993, 9007199254740995]
    flags: bool[2] = [true, false]
    modes: Mode[2] = [Mode::Off, Mode::On]
    single: f64[1] = [0.5]
  init:
    initial = f32(integers[0] - 16777216) + f32(wide[0] - 9007199254740992) + f32(flags[0]) + f32(i32(modes[1])) + f32(single[0])
  sample:
    out1 = f32(integers[0] - 16777216) + f32(integers[1] - 16777216)
    out2 = f32(wide[0] - 9007199254740992) + f32(wide[1] - 9007199254740992)
    out3 = f32(flags[0]) + 2.0 * f32(flags[1])
    out4 = f32(i32(modes[0])) + 2.0 * f32(i32(modes[1]))
    out5 = f32(single[0])
    out6 = initial
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 6, 2);
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("defaults");
    for (output, expected) in outputs.iter().zip([4.0, 4.0, 1.0, 2.0, 0.5, 4.5]) {
        assert_eq!(output, &[expected; 2]);
    }
    for (name, bytes) in [
        (
            "integers",
            [16777221_i32, 16777223]
                .into_iter()
                .flat_map(i32::to_ne_bytes)
                .collect::<Vec<_>>(),
        ),
        (
            "wide",
            [9007199254740997_i64, 9007199254740999]
                .into_iter()
                .flat_map(i64::to_ne_bytes)
                .collect(),
        ),
        ("flags", vec![0, 1]),
        (
            "modes",
            [1_i32, 0].into_iter().flat_map(i32::to_ne_bytes).collect(),
        ),
        ("single", 0.75_f64.to_ne_bytes().to_vec()),
    ] {
        let index = instance.param_index(name).expect("public parameter");
        set_param_by_index(&mut instance, index, &bytes).expect("set array");
    }
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("refresh");
    for (output, expected) in outputs.iter().zip([12.0, 12.0, 2.0, 1.0, 0.75, 4.5]) {
        assert_eq!(output, &[expected; 2]);
    }
}

#[test]
fn main_array_message_defaults_support_constant_calls_and_publication() {
    let source = r#"
const Values: i64[2] = [9007199254740993, 9007199254740995]
const def initial() -> i64[2]:
  return Values
proc Main:
  init:
    value = 0.0
  delegate changed(values: i64[2] = initial())
  event set(values: i64[2] = Values):
    value = f32(values[0] - 9007199254740992)
    changed()
  sample:
    out1 = value
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    let outputs = bind_outputs(&mut instance, 1, 2);
    let mut storage = [0_u8; 64];
    let mut batch = onda_runtime::DelegateBatch::from_storage(&mut storage);
    let event = instance.event_index("set").expect("event");
    let payload: Vec<_> = [9007199254740999_i64, 9007199254740997]
        .into_iter()
        .flat_map(i64::to_le_bytes)
        .collect();
    trigger_event_by_index_checked(
        &mut instance,
        event,
        &payload,
        ExecutionOutput {
            delegate_batch: Some(&mut batch),
            print_batch: None,
        },
    )
    .expect("dispatch");
    let expected: Vec<_> = [9007199254740993_i64, 9007199254740995]
        .into_iter()
        .flat_map(i64::to_le_bytes)
        .collect();
    assert_eq!(
        batch
            .occurrence(0)
            .expect("valid batch")
            .expect("publication")
            .payload,
        expected
    );
    process_checked(&mut instance, 2, ExecutionOutput::none()).expect("process");
    assert_eq!(outputs[0], [7.0; 2]);
}
