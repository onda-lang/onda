use super::*;

fn render_channels(source: &str, expected: &[&[f32]]) {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let frames = expected[0].len();
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            frames,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: frames,
                fast_math: false,
                opt_level,
            },
        );
        let mut output = vec![0.0; frames * expected.len()];
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        for (channel, values) in expected.iter().enumerate() {
            let actual: Vec<_> = output
                .chunks_exact(expected.len())
                .map(|frame| frame[channel])
                .collect();
            assert_eq!(&actual, values, "{opt_level:?}, channel {channel}");
        }
    }
}

#[test]
fn forwarded_processor_views_keep_their_owner_callback_environment() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-environments.onda"
        ),
        &[
            &[6.0, 1408.0, 1414.0, 2816.0],
            &[704.0, 2804.0, 3508.0, 5608.0],
        ],
    );
}

#[test]
fn joined_children_of_different_owners_route_to_the_selected_owner() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(1)
    out1 = 0.0
proc First:
  init:
    voices: Voice[2] = Voice()
    count: i32 = 0
  when voices[0].fired(value):
    count += value
  when voices[1].fired(value):
    count += value * 10
  sample:
    out1 = f32(count)
proc Second:
  init:
    voices: Voice[2] = Voice()
    count: i32 = 0
  when voices[0].fired(value):
    count += value * 100
  when voices[1].fired(value):
    count += value * 1000
  sample:
    out1 = f32(count)
def step(voice) -> f32:
  return voice()
def exercise(first: First, second: Second, choose_first: bool):
  if choose_first:
    selected = first.voices[1]
  else:
    selected = second.voices[0]
  step(selected)
init:
  first = First()
  second = Second()
  choose_first = true
sample:
  exercise(first, second, choose_first)
  choose_first = !choose_first
  out1 = first()
  out2 = second()
"#;
    render_channels(
        source,
        &[&[10.0, 10.0, 20.0, 20.0], &[0.0, 100.0, 100.0, 200.0]],
    );
}

#[test]
fn joined_views_reconstruct_selected_owners_inside_the_selected_environment() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-owner-arrays.onda"
        ),
        &[&[10.0, 1010.0, 1020.0, 2020.0]],
    );
}

#[test]
fn joined_children_of_same_owner_type_keep_independent_owner_storage() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(1)
    out1 = 0.0
proc Bank:
  params:
    weight: i32 = 1
  init:
    voices: Voice[2] = Voice()
    total: i32 = 0
  when voices[0].fired(value):
    total += value * weight
  sample:
    out1 = f32(total)
def step(voice) -> f32:
  return voice()
def exercise(left: Bank, right: Bank, choose_left: bool):
  if choose_left:
    selected = left.voices[0]
  else:
    selected = right.voices[0]
  step(selected)
init:
  left = Bank(weight = 2)
  right = Bank(weight = 7)
  choose_left = true
sample:
  exercise(left, right, choose_left)
  choose_left = !choose_left
  out1 = left()
  out2 = right()
"#;
    render_channels(source, &[&[2.0, 2.0, 4.0, 4.0], &[0.0, 7.0, 7.0, 14.0]]);
}

#[test]
fn replacing_processor_contents_preserves_the_existing_alias_environment() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(1)
    out1 = 0.0
proc Bank:
  params:
    weight: i32 = 1
  init:
    voices: Voice[2] = Voice()
    total: i32 = 0
  when voices[0].fired(value):
    total += value * weight
  sample:
    out1 = f32(total)
def step(voice) -> f32:
  return voice()
def exercise(left: Bank, right: Bank, replace: bool):
  selected = left.voices[0]
  if replace:
    selected = right.voices[1]
  step(selected)
init:
  left = Bank(weight = 2)
  right = Bank(weight = 7)
  replace = true
sample:
  exercise(left, right, replace)
  replace = !replace
  out1 = left()
  out2 = right()
"#;
    render_channels(source, &[&[2.0, 4.0, 6.0, 8.0], &[0.0, 0.0, 0.0, 0.0]]);
}

#[test]
fn scalar_child_views_keep_their_owner_callback_environment() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(3)
    out1 = 0.0
def step(voice) -> f32:
  return voice()
def forward(voice) -> f32:
  return step(voice)
proc Bank:
  init:
    voice = Voice()
    total: i32 = 0
  when voice.fired(value):
    total += value
  sample:
    selected = voice
    forward(selected)
    out1 = f32(total)
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    render_channels(source, &[&[3.0, 6.0, 9.0, 12.0]]);
}

#[test]
fn nested_child_views_preserve_the_outer_owner_callback_chain() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(3)
    out1 = 0.0
def step(voice) -> f32:
  return voice()
proc Bank:
  delegate changed(value: i32)
  init:
    voices: Voice[2] = Voice()
    total: i32 = 0
  when voices[0].fired(value):
    total += value
    changed(value)
  sample:
    out1 = f32(total)
proc Rack:
  init:
    banks: Bank[2] = Bank()
    total: i32 = 0
    selector: i32 = 0
  when banks[0].changed(value):
    total += value
  when banks[1].changed(value):
    total += value * 10
  sample:
    bank = banks[selector]
    voice = bank.voices[0]
    step(voice)
    selector = 1 - selector
    out1 = f32(total)
init:
  rack = Rack()
sample:
  out1 = rack()
"#;
    render_channels(source, &[&[3.0, 33.0, 36.0, 66.0]]);
}

#[test]
fn persistent_processor_view_environments_reconstruct_after_snapshot_restore() {
    let source = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(3)
    out1 = 0.0
proc Bank:
  init:
    voices: Voice[2] = Voice()
    total: i32 = 0
  when voices[0].fired(value):
    total += value
  when voices[1].fired(value):
    total += value * 10
  sample:
    out1 = f32(total)
def step(voice) -> f32:
  return voice()
init:
  left = Bank()
  right = Bank()
  selector = 1
  selected = left.voices[selector]
  selector = 0
sample:
  step(selected)
  out1 = left() + right() * 100.0
"#;
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            2,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 2,
                fast_math: false,
                opt_level,
            },
        );
        let mut output = [0.0; 2];
        process_interleaved(&mut instance, &[], &mut output, 2).unwrap();
        assert_eq!(output, [30.0, 60.0], "{opt_level:?}");
        let snapshot = instance.snapshot_state_bytes().unwrap();
        process_interleaved(&mut instance, &[], &mut output, 2).unwrap();
        assert_eq!(output, [90.0, 120.0], "{opt_level:?}");
        instance.restore_state_bytes(&snapshot).unwrap();
        process_interleaved(&mut instance, &[], &mut output, 2).unwrap();
        assert_eq!(output, [90.0, 120.0], "{opt_level:?}");
    }
}

#[test]
fn forwarded_resource_maps_keep_bindings_and_observe_host_rebinding() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-resources.onda"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        let mut left = [2.0_f32];
        let mut right = [7.0_f32];
        let mut interleaved = [123.0_f64];
        let unused = instance.buffer_index("interleaved").unwrap();
        bind_buffer(
            &mut instance,
            unused,
            interleaved.as_mut_ptr().cast(),
            1,
            1,
            48_000.0,
            PrimitiveType::F64,
        )
        .unwrap();
        for (name, samples) in [("left", &mut left), ("right", &mut right)] {
            let index = instance.buffer_index(name).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0; 4];
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(output, [2.0, 7.0, 2.0, 7.0], "{opt_level:?}");
        let snapshot = instance.snapshot_state_bytes().unwrap();
        right[0] = 11.0;
        let right_index = instance.buffer_index("right").unwrap();
        bind_buffer(
            &mut instance,
            right_index,
            right.as_mut_ptr().cast(),
            1,
            1,
            48_000.0,
            PrimitiveType::F32,
        )
        .unwrap();
        instance.restore_state_bytes(&snapshot).unwrap();
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(output, [2.0, 11.0, 2.0, 11.0], "{opt_level:?}");
    }
}

#[test]
fn forwarded_resource_choices_keep_owner_callbacks_and_buffer_writes() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-resource-routing.onda"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        let mut left = [0.0_f32; 4];
        let mut right = [0.0_f32; 4];
        for (name, samples) in [("left", &mut left), ("right", &mut right)] {
            let index = instance.buffer_index(name).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                4,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0; 4];
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(
            output,
            [20200.0, 7020207.0, 7040207.0, 14040207.0],
            "{opt_level:?}"
        );
        assert_eq!(left[0], 2.0);
        assert_eq!(right[0], 7.0);
    }
}

#[test]
fn task_processor_views_resume_events_with_current_bindings_after_snapshot_restore() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-task.onda"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            1,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 1,
                fast_math: false,
                opt_level,
            },
        );
        let mut left = [2.0_f32];
        let mut right = [7.0_f32];
        for (index, samples) in [(0, &mut left), (1, &mut right)] {
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0];
        process_interleaved(&mut instance, &[], &mut output, 1).unwrap();
        assert_eq!(output, [0.0]);
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut rebound = [11.0_f32];
        for _ in 0..2 {
            instance.restore_state_bytes(&snapshot).unwrap();
            let event = instance.event_index("change").unwrap();
            trigger_event_by_index_checked(
                &mut instance,
                event,
                &[],
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap();
            rebound[0] = 11.0;
            let right = 1;
            bind_buffer(
                &mut instance,
                right,
                rebound.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            process_interleaved(&mut instance, &[], &mut output, 1).unwrap();
            assert_eq!(output, [120.0], "{opt_level:?}");
            assert_eq!(rebound, [12.0]);
            assert_eq!(left, [2.0]);
        }
    }
}

#[test]
fn forwarded_processor_buffer_collections_select_complete_spans() {
    let common = r#"
proc Voice:
  buffers:
    source: f32 {2}
  params:
    tag: i32 = 0
  sample:
    source[0][0] = f32(tag)
    source[1][0] = f32(tag * 10)
    out1 = source[0][0] + source[1][0]
def step_child(voice) -> f32:
  return voice()
def step_array(voices, index: i32) -> f32:
  return step_child(voices[index])
"#;
    let owned = r#"
proc Bank:
  buffers:
    left: f32 {2}
    right: f32 {2}
  init:
    voices: Voice[2] = [Voice(tag = 2, source = left), Voice(tag = 7, source = right)]
    index: i32 = 0
  sample:
    out1 = step_array(voices, index)
    index = 1 - index
buffers:
  left: f32 {2}
  right: f32 {2}
init:
  bank = Bank(left = left, right = right)
sample:
  out1 = bank()
"#;
    let global = r#"
buffers:
  left: f32 {2}
  right: f32 {2}
init:
  voices: Voice[2] = [Voice(tag = 2, source = left), Voice(tag = 7, source = right)]
  index: i32 = 0
sample:
  out1 = step_array(voices, index)
  index = 1 - index
"#;
    for body in [global, owned] {
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let (mut instance, _, _) = compile_instance_with_options(
                &format!("{common}{body}"),
                4,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 4,
                    fast_math: false,
                    opt_level,
                },
            );
            let mut buffers = [[0.0_f32; 4]; 4];
            for (name, samples) in ["left[0]", "left[1]", "right[0]", "right[1]"]
                .into_iter()
                .zip(&mut buffers)
            {
                let index = instance.buffer_index(name).unwrap();
                bind_buffer(
                    &mut instance,
                    index,
                    samples.as_mut_ptr().cast(),
                    4,
                    1,
                    48_000.0,
                    PrimitiveType::F32,
                )
                .unwrap();
            }
            let mut output = [0.0; 4];
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, [22.0, 77.0, 22.0, 77.0], "{opt_level:?}");
            assert_eq!(buffers.map(|samples| samples[0]), [2.0, 20.0, 7.0, 70.0]);
        }
    }
}

#[test]
fn joined_child_resources_keep_roots_coordinates_callbacks_and_rebinding() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-joined-resources.onda"
    );
    for (same_extent, different_owner) in [(false, false), (true, false), (false, true)] {
        let source = if same_extent {
            source.replace(
                "others: Bank[3] = [Bank(source = left), Bank(source = right), Bank(source = left)]",
                "others: Bank[2] = [Bank(source = left), Bank(source = right)]",
            )
        } else {
            source.to_owned()
        };
        let source = if different_owner {
            let owner = source
                .split("proc Bank:")
                .nth(1)
                .unwrap()
                .split("def step")
                .next()
                .unwrap();
            let other = format!(
                "proc OtherBank:{}",
                owner
                    .replace("    source: f32", "    source: f32\n    extra: f64")
                    .replace(
                        "count += value * 10",
                        "count += value * 10 + i32(extra[0] - 123.0)"
                    )
            );
            let source = source.replace("def step", &format!("{other}def step"));
            source.replace(
                "others: Bank[3] = [Bank(source = left), Bank(source = right), Bank(source = left)]",
                "others: OtherBank[3] = [OtherBank(source = left, extra = interleaved), OtherBank(source = right, extra = interleaved), OtherBank(source = left, extra = interleaved)]",
            ).replace(
                "    owner = others[index]\n    selected = owner.voices[0]",
                "    other = others[index]\n    selected = other.voices[0]",
            )
        } else {
            source
        };
        for callbacks in if different_owner {
            &[true][..]
        } else {
            &[false, true][..]
        } {
            let callbacks = *callbacks;
            let source = if callbacks {
                source.clone()
            } else {
                source.replace(
                    "  when voices[0].fired(value):\n    count += value * 10\n  when voices[1].fired(value):\n    count += value\n", "",
                )
            };
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
                let mut left = [2.0_f32];
                let mut right = [7.0_f32];
                let mut unused = [123.0_f64];
                for (name, ptr, ty) in [
                    ("left", left.as_mut_ptr().cast(), PrimitiveType::F32),
                    ("right", right.as_mut_ptr().cast(), PrimitiveType::F32),
                    (
                        "interleaved",
                        unused.as_mut_ptr().cast(),
                        PrimitiveType::F64,
                    ),
                ] {
                    let index = instance.buffer_index(name).unwrap();
                    bind_buffer(&mut instance, index, ptr, 1, 1, 48_000.0, ty).unwrap();
                }
                let mut output = [0.0; 8];
                process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
                let counts = if callbacks {
                    [1.0, 10001.0, 10002.0, 20002.0]
                } else {
                    [0.0; 4]
                };
                for (frame, (value, count)) in
                    [3.0, 8.0, 4.0, 9.0].into_iter().zip(counts).enumerate()
                {
                    assert_eq!(&output[frame * 2..frame * 2 + 2], &[value, count]);
                }
                assert_eq!(left, [4.0]);
                assert_eq!(right, [9.0]);
                assert_eq!(unused, [123.0]);
                let snapshot = instance.snapshot_state_bytes().unwrap();
                let mut rebound_left = [20.0_f32];
                let mut rebound_right = [70.0_f32];
                for (name, samples) in [("left", &mut rebound_left), ("right", &mut rebound_right)]
                {
                    let index = instance.buffer_index(name).unwrap();
                    bind_buffer(
                        &mut instance,
                        index,
                        samples.as_mut_ptr().cast(),
                        1,
                        1,
                        48_000.0,
                        PrimitiveType::F32,
                    )
                    .unwrap();
                }
                instance.restore_state_bytes(&snapshot).unwrap();
                process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
                assert_eq!(
                    output.into_iter().step_by(2).collect::<Vec<_>>(),
                    [21.0, 71.0, 22.0, 72.0]
                );
                assert_eq!(rebound_left, [22.0]);
                assert_eq!(rebound_right, [72.0]);
            }
        }
    }
}

#[test]
fn joined_deep_child_resources_preserve_collection_windows() {
    let source = r#"
proc Voice:
  buffers:
    source: f32
  sample:
    out1 = source[0]
proc Bank:
  params:
    pick: i32 = 0
  buffers:
    source: f32 {2}
  init:
    voices: Voice[2] = Voice(source = source[pick])
  sample:
    out1 = 0.0
proc Rack:
  buffers:
    source: f32 {4}
  init:
    banks: Bank[2] = [Bank(source = source[0:2], pick = 1), Bank(source = source[2:4], pick = 0)]
  sample:
    out1 = 0.0
def step(voice) -> f32:
  return voice()
def exercise(racks, first: bool) -> f32:
  if first:
    rack = racks[0]
    bank = rack.banks[0]
    selected = bank.voices[1]
  else:
    rack = racks[1]
    bank = rack.banks[1]
    selected = bank.voices[0]
  return step(selected)
buffers:
  clips: f32 {4}
init:
  racks: Rack[2] = Rack(source = clips)
  first = true
sample:
  out1 = exercise(racks, first)
  first = !first
"#;
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        let mut buffers = [[1.0_f32], [2.0], [3.0], [4.0]];
        for (index, samples) in buffers.iter_mut().enumerate() {
            let index = instance.buffer_index(&format!("clips[{index}]")).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0; 4];
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(output, [2.0, 3.0, 2.0, 3.0], "{opt_level:?}");
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut rebound = [[10.0_f32], [20.0], [30.0], [40.0]];
        for (index, samples) in rebound.iter_mut().enumerate() {
            let index = instance.buffer_index(&format!("clips[{index}]")).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        instance.restore_state_bytes(&snapshot).unwrap();
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(output, [20.0, 30.0, 20.0, 30.0], "{opt_level:?}");
    }
}

#[test]
fn nested_processor_joins_keep_storage_captures_initialized_on_every_path() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-nested-joins.onda"
        ),
        &[
            &[2.0, 2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0],
            &[0.0, 0.0, 7.0, 7.0, 7.0, 7.0, 14.0, 14.0],
            &[0.0, 100.0, 100.0, 200.0, 200.0, 300.0, 300.0, 400.0],
        ],
    );
}

#[test]
fn nested_processor_joins_preserve_selected_resource_captures() {
    let source = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-view-nested-joins.onda")
        .replace("proc Voice:", "proc Voice:\n  buffers:\n    source: f32")
        .replace("    fired(1)", "    source[0] += 1.0\n    fired(1)")
        .replace("proc Bank:", "proc Bank:\n  buffers:\n    source: f32")
        .replace("proc OtherBank:", "proc OtherBank:\n  buffers:\n    source: f32")
        .replace("Voice()", "Voice(source = source)")
        .replace("init:\n  left = Bank(weight = 2)", "buffers:\n  clip_left: f32\n  clip_right: f32\n  clip_third: f32\ninit:\n  left = Bank(weight = 2, source = clip_left)")
        .replace("right = Bank(weight = 7)", "right = Bank(weight = 7, source = clip_right)")
        .replace("other = OtherBank()", "other = OtherBank(source = clip_third)");
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            8,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 8,
                fast_math: false,
                opt_level,
            },
        );
        let mut buffers = [[0.0_f32]; 3];
        for (name, samples) in ["clip_left", "clip_right", "clip_third"]
            .into_iter()
            .zip(&mut buffers)
        {
            let index = instance.buffer_index(name).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0; 24];
        process_interleaved(&mut instance, &[], &mut output, 8).unwrap();
        assert_eq!(buffers, [[2.0], [2.0], [4.0]], "{opt_level:?}");
        assert_eq!(&output[21..], &[4.0, 14.0, 400.0], "{opt_level:?}");
    }
}
