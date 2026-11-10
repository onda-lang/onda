use super::*;

fn render(instance: &mut Instance, frames: usize) -> Vec<f32> {
    let mut output = vec![0.0; frames];
    process_interleaved(instance, &[], &mut output, frames).expect("processor should render");
    output
}

fn send_integers(instance: &mut Instance, name: &str, values: &[i32]) {
    let payload = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect::<Vec<_>>();
    let event = instance.event_index(name).expect("event should exist");
    assert_eq!(
        trigger_event_by_index_checked(
            instance,
            event,
            &payload,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("processor event should run"),
        onda_runtime::PROCESSOR_EXECUTION_OK
    );
}

#[test]
fn broadcast_array_parameters_initialize_all_top_level_and_nested_slots() {
    let source = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-whole-array-constructors.onda");
    for (source, expected) in [
        (source.to_owned(), 18.0),
        (
            source
                .replace("f32[2]", "f32[1]")
                .replace("[0.0, 0.0]", "[0.0]")
                .replace("[0.25, 0.75]", "[0.25]")
                .replace("gains[0] + gains[1]", "gains[0]"),
            4.5,
        ),
    ] {
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                4,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 4,
                    fast_math: false,
                    opt_level,
                },
            );
            assert_eq!(render(&mut instance, 4), [expected; 4], "{opt_level:?}");
        }
    }
}

#[test]
fn nested_large_banks_keep_slot_history_and_events_independent() {
    let source =
        include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-storage.onda");
    let (mut instance, _, _) = compile_instance(source, 4);
    for values in [[0, 127, 7], [0, 0, 2], [1, 127, 11], [1, 0, 5]] {
        send_integers(&mut instance, "Seed", &values);
    }
    assert_eq!(render(&mut instance, 4), [6127.0, 2222.0, 3333.0, 7238.0]);
    let snapshot = instance.snapshot_state_bytes().unwrap();
    assert_eq!(render(&mut instance, 4), [3333.0, 4444.0, 8349.0, 4444.0]);
    instance.restore_state_bytes(&snapshot).unwrap();
    assert_eq!(render(&mut instance, 4), [3333.0, 4444.0, 8349.0, 4444.0]);
}

#[test]
fn captured_aliases_keep_the_selected_voice_and_share_mutations() {
    let source = r#"
proc Voice:
  init:
    value: i32 = 0
  event add(amount: i32):
    value = value + amount
  sample:
    out1 = f32(value)
    value = value + 1
proc Bank:
  init:
    voices: Voice[4] = Voice()
    selector: i32 = 1
  sample:
    first = voices[selector]
    second = voices[selector]
    selector = 2
    first.add(10)
    second.add(1)
    a = first()
    b = second()
    other = voices[selector]()
    out1 = a * 1000.0 + b * 10.0 + other
    selector = 1
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 3);
    assert_eq!(render(&mut instance, 3), [11120.0, 24251.0, 37382.0]);
}

#[test]
fn indexed_alias_captures_selector_before_mutation() {
    let source = r#"
proc Voice:
  init:
    value: i32 = 0
  event add(amount: i32):
    value = value + amount
  sample:
    out1 = f32(value)
proc Bank:
  init:
    voices: Voice[3] = Voice()
    selector: i32 = 1
  sample:
    voice = voices[selector]
    selector = 2
    voice.add(10)
    out1 = voices[1]() * 100.0 + voices[2]()
    selector = 1
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    assert_eq!(render(&mut instance, 2), [1000.0, 2000.0]);
}

#[test]
fn indexed_alias_evaluates_a_side_effecting_selector_once() {
    let source = r#"
proc Voice:
  init:
    value: i32 = 0
  event add(amount: i32):
    value = value + amount
  sample:
    out1 = f32(value)
proc Bank:
  init:
    voices: Voice[3] = Voice()
    selections: i32 = 0
  def choose() -> i32:
    selections = selections + 1
    return 1
  sample:
    voice = voices[choose()]
    voice.add(10)
    out1 = voice() * 100.0 + voice() * 10.0 + f32(selections)
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 2);
    assert_eq!(render(&mut instance, 2), [1101.0, 2202.0]);
}

#[test]
fn branch_selected_aliases_preserve_the_chosen_voice() {
    let source = r#"
proc Voice:
  init:
    value: i32 = 0
  event add(amount: i32):
    value = value + amount
  sample:
    out1 = f32(value)
proc Bank:
  init:
    voices: Voice[3] = Voice()
    choose_second = false
    selector: i32 = 0
  sample:
    if choose_second:
      selector = 1
      second = voices[selector]
      selector = 2
      second.add(10)
    else:
      selector = 0
      first = voices[selector]
      selector = 2
      first.add(10)
    out1 = voices[0]() * 100.0 + voices[1]()
    selector = 0
    choose_second = !choose_second
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 3);
    assert_eq!(render(&mut instance, 3), [1000.0, 1010.0, 2010.0]);
}

#[test]
fn dynamic_nested_parameter_hooks_use_the_same_clamped_slot_as_the_write() {
    let source = r#"
proc Voice:
  params:
    gain: i32 = 0 => update
  init:
    cached: i32 = 0
  def update():
    cached = gain * 2
  sample:
    out1 = f32(cached)
proc Bank:
  init:
    voices: Voice[128] = Voice()
    selected: i32 = 0
    gain_value: i32 = 0
  event configure(index: i32, value: i32):
    selected = index
    gain_value = value
  sample:
    voices[selected].gain = gain_value
    out1 = voices[0]() * 100.0 + voices[127]()
init:
  bank = Bank()
event Configure(index: i32, value: i32):
  bank.configure(index, value)
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 1);
    for (index, value, expected) in [(127, 7, 14.0), (-1, 3, 614.0), (500, 9, 618.0)] {
        send_integers(&mut instance, "Configure", &[index, value]);
        assert_eq!(render(&mut instance, 1), [expected]);
    }
}

#[test]
fn nested_proc_arrays_forward_through_multiple_helpers() {
    let source = r#"
proc Voice:
  init:
    count: i32 = 0
  sample:
    count = count + 1
    out1 = f32(count)
def step(voices, index: i32) -> f32:
  voice = voices[index]
  return voice()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
proc Bank:
  init:
    voices: Voice[128] = Voice()
    index: i32 = 127
  sample:
    out1 = forward(voices, index) * 100.0 + forward(voices, 0)
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 3);
    assert_eq!(render(&mut instance, 3), [101.0, 202.0, 303.0]);
}

#[test]
fn nested_voice_block_hooks_forward_through_helpers_across_segments() {
    let source = r#"
proc Voice:
  init:
    before: i32 = 0
    after: i32 = 0
  block:
    before = before + 1
    sample:
      out1 = f32(before * 10 + after)
    after = after + 1
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
proc Bank:
  init:
    voices: Voice[4] = Voice()
    selector: i32 = 0
  sample:
    out1 = forward(voices, selector)
    selector = 1 - selector
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let frames = 4;
    let (mut instance, _, _) = compile_instance(source, frames);
    let mut output = vec![0.0_f32; frames];
    bind_output(
        &mut instance,
        0,
        output.as_mut_ptr().cast(),
        std::mem::size_of_val(output.as_slice()),
    )
    .unwrap();
    for expected in [10.0, 21.0] {
        assert_eq!(
            process_checked_segment(
                &mut instance,
                0,
                2,
                PROCESSOR_BEGIN_BLOCK,
                onda_runtime::ExecutionOutput::none(),
            )
            .expect("begin segment should run"),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(
            process_checked_segment(
                &mut instance,
                2,
                2,
                PROCESSOR_END_BLOCK,
                onda_runtime::ExecutionOutput::none(),
            )
            .expect("end segment should run"),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(output, [expected; 4]);
    }
    assert_eq!(render(&mut instance, frames), [32.0; 4]);
}

#[test]
fn proc_array_pinned_fields_reset_and_restore_independently() {
    let source = r#"
proc Voice:
  init:
    pin held: i32 = 10
    resettable: i32 = 1
  event add(amount: i32):
    held = held + amount
    resettable = resettable + amount
  sample:
    out1 = f32(held * 100 + resettable)
proc Bank:
  init:
    voices: Voice[2] = Voice()
  event add(index: i32, amount: i32):
    voices[index].add(amount)
  sample:
    out1 = voices[0]() * 10.0 + voices[1]()
init:
  bank = Bank()
event Add(index: i32, amount: i32):
  bank.add(index, amount)
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 1);
    assert_eq!(render(&mut instance, 1), [11011.0]);
    send_integers(&mut instance, "Add", &[0, 2]);
    send_integers(&mut instance, "Add", &[1, 5]);
    assert_eq!(render(&mut instance, 1), [13536.0]);
    let snapshot = instance
        .snapshot_state_bytes()
        .expect("snapshot should save");
    assert_eq!(
        init_checked(
            &mut instance,
            InitMode::PreservePinned,
            onda_runtime::ExecutionOutput::none()
        )
        .expect("pinned reset should run"),
        onda_runtime::PROCESSOR_EXECUTION_OK
    );
    assert_eq!(render(&mut instance, 1), [13511.0]);
    instance
        .restore_state_bytes(&snapshot)
        .expect("snapshot should restore");
    assert_eq!(render(&mut instance, 1), [13536.0]);
    assert_eq!(
        init_checked(
            &mut instance,
            InitMode::Full,
            onda_runtime::ExecutionOutput::none()
        )
        .expect("full reset should run"),
        onda_runtime::PROCESSOR_EXECUTION_OK
    );
    assert_eq!(render(&mut instance, 1), [11011.0]);
}

#[test]
fn nested_heterogeneous_buffer_slots_observe_current_host_bindings() {
    let source = r#"
proc Voice:
  buffers:
    source: f32
  init:
    held = 0.0
  event capture():
    held = source[0]
  sample:
    out1 = source[0] + held * 10.0
proc Bank:
  buffers:
    left: f32
    right: f32
  init:
    voices: Voice[2] = [Voice(source = left), Voice(source = right)]
    selector: i32 = 0
  event capture(index: i32):
    voices[index].capture()
  sample:
    out1 = voices[selector]()
    selector = 1 - selector
buffers:
  left: f32
  right: f32
init:
  bank = Bank(left = left, right = right)
event Capture(index: i32):
  bank.capture(index)
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    let mut left = [2.0_f32];
    let mut right = [7.0_f32];
    for (name, samples) in [("left", &mut left), ("right", &mut right)] {
        let index = instance.buffer_index(name).unwrap();
        bind_buffer(
            &mut instance,
            index,
            samples.as_mut_ptr().cast(),
            samples.len(),
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .unwrap();
    }
    assert_eq!(render(&mut instance, 4), [2.0, 7.0, 2.0, 7.0]);
    let mut replacement = [11.0_f32];
    let right_index = instance.buffer_index("right").unwrap();
    bind_buffer(
        &mut instance,
        right_index,
        replacement.as_mut_ptr().cast(),
        replacement.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .unwrap();
    assert_eq!(render(&mut instance, 4), [2.0, 11.0, 2.0, 11.0]);
    send_integers(&mut instance, "Capture", &[1]);
    assert_eq!(render(&mut instance, 4), [2.0, 121.0, 2.0, 121.0]);
}

#[test]
fn indexed_nested_delegates_route_to_their_slot_and_owning_instance() {
    let source = r#"
proc Voice:
  params:
    tag: i32 = 0
  delegate fired(value: i32)
  sample:
    fired(tag)
    out1 = 0.0
proc Bank:
  delegate changed(value: i32)
  init:
    voices: Voice[2] = [Voice(tag = 2), Voice(tag = 7)]
    selector: i32 = 0
  when voices[0].fired(value):
    changed(value)
  when voices[1].fired(value):
    changed(value * 100)
  sample:
    voices[selector]()
    selector = 1 - selector
    out1 = 0.0
init:
  bank = Bank()
  total: i32 = 0
when bank.changed(value):
  total = total + value
sample:
  bank()
  out1 = f32(total)
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    assert_eq!(render(&mut instance, 4), [2.0, 702.0, 704.0, 1404.0]);
}

#[test]
fn forwarded_top_level_proc_arrays_keep_global_delegate_routes() {
    let source = r#"
proc Voice:
  params:
    tag: i32 = 0
  delegate fired(value: i32)
  sample:
    fired(tag)
    out1 = 0.0
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
init:
  voices: Voice[2] = [Voice(tag = 2), Voice(tag = 7)]
  selector: i32 = 0
  total: i32 = 0
when voices[0].fired(value):
  total += value
when voices[1].fired(value):
  total += value * 100
sample:
  forward(voices, selector)
  selector = 1 - selector
  out1 = f32(total)
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    assert_eq!(render(&mut instance, 4), [2.0, 702.0, 704.0, 1404.0]);
}

#[test]
fn routed_buffer_calls_evaluate_selector_before_inputs_once() {
    let source = r#"
proc Voice:
  ins:
    in1
  buffers:
    source: f32
  delegate fired(value: i32)
  sample:
    fired(i32(in1))
    out1 = source[0] + in1
proc Bank:
  buffers:
    left: f32
    right: f32
  init:
    voices: Voice[2] = [Voice(source = left), Voice(source = right)]
    selections: i32 = 0
    seen: i32 = 0
  def choose() -> i32:
    selections = selections + 1
    return 1
  when voices[0].fired(value):
    seen = value
  when voices[1].fired(value):
    seen = value * 100
  sample:
    out1 = voices[choose()](f32(selections)) + f32(seen)
buffers:
  left: f32
  right: f32
init:
  bank = Bank(left = left, right = right)
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    let mut left = [2.0_f32];
    let mut right = [7.0_f32];
    for (name, samples) in [("left", &mut left), ("right", &mut right)] {
        let index = instance.buffer_index(name).unwrap();
        bind_buffer(
            &mut instance,
            index,
            samples.as_mut_ptr().cast(),
            samples.len(),
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .unwrap();
    }
    assert_eq!(render(&mut instance, 4), [108.0, 209.0, 310.0, 411.0]);
    let mut replacement = [11.0_f32];
    let right_index = instance.buffer_index("right").unwrap();
    bind_buffer(
        &mut instance,
        right_index,
        replacement.as_mut_ptr().cast(),
        replacement.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .unwrap();
    assert_eq!(render(&mut instance, 4), [516.0, 617.0, 718.0, 819.0]);
}

#[test]
fn oversampled_nested_delegates_keep_slot_state_and_selector_order() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  init:
    ticks: i32 = 0
  sample 2:
    ticks = ticks + 1
    fired(1)
    out1 = 0.0
proc Bank:
  init:
    voices: Voice[2] = Voice()
    selections: i32 = 0
    seen: i32 = 0
  def choose() -> i32:
    selections = selections + 1
    return 1
  when voices[0].fired(value):
    seen = seen + value
  when voices[1].fired(value):
    seen = seen + value * 100
  sample:
    voices[choose()]()
    out1 = f32(seen + selections)
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 4);
    assert_eq!(render(&mut instance, 4), [201.0, 402.0, 603.0, 804.0]);
}

#[test]
fn unsafe_voice_aliases_preserve_multiple_outputs_and_cached_fields() {
    let source = r#"
proc Voice:
  outs:
    main
    aux
  init:
    count: i32 = 0
  sample:
    count = count + 1
    main = f32(count)
    aux = f32(count * 10)
proc Bank:
  outs:
    out1
  init:
    voices: Voice[4] = Voice()
    selector: i32 = 3
  sample:
    voice = read_unsafe(voices, selector)
    a, b = voice()
    out1 = a + b + voices[selector].aux
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    let (mut instance, _, _) = compile_instance(source, 3);
    assert_eq!(render(&mut instance, 3), [21.0, 42.0, 63.0]);
}

#[test]
fn indexed_and_aliased_fields_distinguish_cached_reads_from_calls() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-cached-fields.onda"
    );
    let (mut instance, _, _) = compile_instance(source, 3);
    assert_eq!(render(&mut instance, 3), [81.0, 234.0, 387.0]);
}
