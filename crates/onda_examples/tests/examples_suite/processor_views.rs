use super::*;

#[test]
fn block_storage_and_sample_owned_processors_compose() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-block-sample-owned-hooks.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("    unused: Voice = voice\n", ""),
    ] {
        render_segmented_lifecycle_outputs(&source, &[21.0, 22.0, 22.0, 23.0]);
    }
}

#[test]
fn forwarded_and_indexed_processor_aliases_compose() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-forwarded-alias-joins.onda"
        ),
        &[&[1.0, 1.0, 2.0, 2.0], &[1.0, 1.0, 3.0, 3.0], &[0.0, 1.0, 1.0, 3.0], &[1.0; 4]],
    );
}

#[test]
fn processor_array_parameter_slices_preserve_callback_owners() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-array-parameter-slices.onda"
    );
    for parameter in ["voices", "voices: Voice[3]", "voices: Voice[]"] {
        render_channels(
            &source.replace("step_tail(voices)", &format!("step_tail({parameter})")),
            &[&[1.0, 2.0, 3.0, 4.0]],
        );
    }
}

#[test]
fn joined_processor_owners_keep_nested_tensor_shapes_when_forwarded() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-joined-owner-forwarding.onda"
    );
    render_channels(source, &[&[1242.0, 2763.0, 14847.0, 2763.0], &[0.0; 4]]);
}

#[test]
fn peeled_callback_loops_preserve_cached_reads_segments_and_rebound_snapshots() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/first-iteration-callbacks.onda"
    )
    .replace("proc Voice:\n", "proc Voice:\n  buffers:\n    step: f32\n")
    .replace("proc Bank:\n", "proc Bank:\n  buffers:\n    step: f32\n")
    .replace("Voice()", "Voice(step = step)")
    .replace("phase += 0.125", "phase += step[0]")
    .replace("expected += 0.125", "expected += step[0]")
    .replace("expected - 0.125", "expected - step[0]")
    .replace(
        "init:\n  bank = Bank()",
        "buffers:\n  step: f32\ninit:\n  bank = Bank(step = step)",
    );
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        for mut step in [[0.125_f32], [0.25]] {
            instance.restore_state_bytes(&snapshot).unwrap();
            bind_buffer(
                &mut instance,
                0,
                step.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            let mut outputs = [[f32::NAN; 4]; 4];
            for (channel, output) in outputs.iter_mut().enumerate() {
                bind_output(
                    &mut instance,
                    channel,
                    output.as_mut_ptr().cast(),
                    std::mem::size_of_val(output),
                )
                .unwrap();
            }
            for (start, frames, flags) in [(0, 1, 1), (1, 3, 2)] {
                assert_eq!(
                    process_checked_segment(
                        &mut instance,
                        start,
                        frames,
                        flags,
                        onda_runtime::ExecutionOutput::none(),
                    )
                    .unwrap(),
                    onda_runtime::PROCESSOR_EXECUTION_OK
                );
            }
            assert_eq!(outputs, [[0.0; 4]; 4], "{opt_level:?}");
        }
    }
}

#[test]
fn processor_local_resource_arguments_preserve_names_defaults_and_order() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-local-resource-arguments.onda"
    );
    render_channels_with_buffers(
        source,
        &[
            &[24.0; 4],
            &[73.5; 4],
            &[73.25; 4],
            &[21.0; 4],
            &[3.0; 4],
            &[13.0; 4],
        ],
        &[0.0, 0.0],
    );
}

#[test]
fn processor_local_unused_resources_preserve_named_and_default_arguments() {
    let source = r#"
proc Voice:
  sample:
    out1 = 1.0
proc Bank:
  buffers:
    source: f32
  init:
    voice = Voice()
  def mix(a: f32, b: i32 = 7) -> f32:
    return a + f32(b) + voice()
  sample:
    out1 = mix(b = 7, a = 0.5)
    out2 = mix(0.5)
buffers:
  source: f32
init:
  bank = Bank(source = source)
sample:
  out1, out2 = bank()
"#;
    render_channels_with_buffers(source, &[&[8.5; 4], &[8.5; 4]], &[0.0]);
}

#[test]
fn reordered_constructor_resources_match_through_borrowed_views() {
    let source = r#"
proc Voice:
  params:
    gain: f32 = 2.0
    offsets: f32[2] = [3.0, 5.0]
  buffers:
    left: f32
    right: f32
  sample:
    out1 = gain * left[0] + right[0] + offsets[0] + offsets[1]
proc Bank:
  buffers:
    first: f32
    second: f32
  init:
    voice = Voice(right = second, left = first)
    voices: Voice[2] = [Voice(right = second, left = first), Voice(left = second, right = first, gain = 4.0)]
  sample:
    out1 = voice()
def step(voice) -> f32:
  return voice()
def forward(voice) -> f32:
  return step(voice)
buffers:
  a: f32
  b: f32
init:
  voice = Voice(right = b, left = a)
  voices: Voice[2] = [Voice(right = b, left = a), Voice(left = b, right = a, gain = 4.0)]
  bank = Bank(second = b, first = a)
sample:
  out1 = voice()
  out2 = forward(voice)
  out3 = bank()
  out4 = forward(bank.voice)
  out5 = forward(voices[0])
  out6 = forward(bank.voices[0])
  out7 = forward(voices[1])
  out8 = forward(bank.voices[1])
"#;
    render_channels_with_buffers(
        source,
        &[
            &[40.0; 4], &[40.0; 4], &[40.0; 4], &[40.0; 4], &[40.0; 4], &[40.0; 4], &[59.0; 4],
            &[59.0; 4],
        ],
        &[11.0, 10.0],
    );
}

#[test]
fn local_scalar_processor_copies_without_resources_activate_once() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-local-scalar-hooks.onda"
    );
    render_channel_blocks_with_buffers(
        source,
        &[&[2.0; 8], &[2.0; 8], &[2.0; 8], &[2.0; 8], &[0.0; 8]],
        &[],
        4,
    );
}

#[test]
fn forwarded_processor_hooks_ignore_definition_order() {
    for reverse in [false, true] {
        let mut source = String::from(
            "proc Voice:\n  init:\n    before: i32 = 0\n    after: i32 = 0\n  block:\n    before += 1\n    sample:\n      out1 = f32(before * 10 + after)\n    after += 1\n",
        );
        let mut indices = (0..128).collect::<Vec<_>>();
        if reverse {
            indices.reverse();
        }
        for index in indices {
            let call = if index == 127 {
                "voice()".to_owned()
            } else {
                format!("forward_{}(voice)", index + 1)
            };
            source.push_str(&format!(
                "def forward_{index}(voice: Voice) -> f32:\n  return {call}\n"
            ));
        }
        source.push_str("init:\n  voice = Voice()\nsample:\n  out1 = forward_0(voice)\n");
        render_channel_blocks_with_buffers(
            &source,
            &[&[10.0, 10.0, 10.0, 10.0, 21.0, 21.0, 21.0, 21.0]],
            &[],
            4,
        );
    }
}

#[test]
fn copied_processor_elements_flatten_without_resolving_resources() {
    let source = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-element-resources.onda");
    let mut chain = String::new();
    let mut previous = "copies".to_owned();
    for index in 0..12 {
        let next = format!("copy{index}");
        chain.push_str(&format!(
            "  {next}: Voice[2] = [{previous}[index], {previous}[1 - index]]\n"
        ));
        previous = next;
    }
    chain.push_str(&format!("  return {previous}[index]()"));
    let source = source.replace("  return copies[index]()", &chain);
    render_channel_blocks_with_buffers(&source, &[&[0.0; 8], &[1.0; 8]], &[0.0, 0.0], 4);

    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-array-resources.onda"
    );
    let source = source.replace("  copies: Voice[2] = bank.voices",
        "  originals: Voice[2] = [bank.voices[0], bank.voices[1]]\n  intermediates: Voice[2] = [originals[index], originals[1 - index]]\n  copies: Voice[2] = [intermediates[index], intermediates[1 - index]]");
    render_channel_blocks_with_buffers(
        &source,
        &[
            &[64.0, 119.0, 64.0, 119.0, 64.0, 119.0, 64.0, 119.0],
            &[0.0; 8],
            &[11.0; 8],
        ],
        &[0.0, 0.0],
        4,
    );
}

#[test]
fn local_processor_lifetimes_complete_on_returns_and_loop_exits() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-local-copy-lifetimes.onda"
    );
    render_channel_blocks_with_buffers(
        source,
        &[
            &[2.0; 8],
            &[10.0, 20.0, 50.0, 60.0, 70.0, 80.0, 110.0, 120.0],
            &[0.0; 8],
        ],
        &[0.0],
        4,
    );
}

#[test]
fn local_processor_lifetimes_complete_on_fallthrough_and_while_exits() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-local-copy-lifetimes.onda"
    );
    let start = source.find("def exercise(").unwrap();
    let end = source[start..].find("\nbuffers:").unwrap() + start;
    let mut source = source.to_owned();
    source.replace_range(
        start..end,
        r#"def exercise(source: Voice):
  i: i32 = 0
  while i < 3:
    local: Voice = source
    step(local)
    i += 1
    if i == 1:
      continue
    if i == 3:
      break
  copy: Voice = source
  step(copy)"#,
    );
    source = source.replace(
        "  out1 = exercise(voice, frame % 4)",
        "  exercise(voice)\n  out1 = result[0]",
    );
    let expected = [40.0, 80.0, 120.0, 160.0, 200.0, 240.0, 280.0, 320.0];
    render_channel_blocks_with_buffers(&source, &[&expected, &expected, &[0.0; 8]], &[0.0], 4);
}

#[test]
fn local_parent_copies_reset_and_complete_independently_borrowed_children() {
    let source = r#"
proc Voice:
  buffers:
    result: f32
  init:
    before: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before)
    result[0] += 10.0
proc Bank:
  buffers:
    result: f32
  init:
    voice = Voice(result = result)
def step(voice) -> f32:
  return voice()
def exercise(source: Bank) -> f32:
  copy: Bank = source
  return step(copy.voice)
buffers:
  result: f32
init:
  bank = Bank(result = result)
sample:
  step(bank.voice)
  out1 = exercise(bank)
  out2 = result[0]
"#;
    render_channel_blocks_with_buffers(
        source,
        &[
            &[2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 3.0, 3.0],
            &[10.0, 20.0, 30.0, 40.0, 60.0, 70.0, 80.0, 90.0],
        ],
        &[0.0],
        4,
    );
}

#[test]
fn sample_local_processor_copies_complete_before_the_next_sample() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-local-copy-lifetimes.onda"
    );
    let source = source.replace(
        "  out1 = exercise(voice, frame % 4)",
        "  copy: Voice = voice\n  out1 = step(copy)",
    );
    render_channel_blocks_with_buffers(
        &source,
        &[
            &[2.0; 8],
            &[0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0],
            &[0.0; 8],
        ],
        &[0.0],
        4,
    );
}

#[test]
fn joined_local_processor_copies_keep_one_completion_owner() {
    let source = r#"
proc Voice:
  buffers:
    result: f32
  init:
    before: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before)
    result[0] += 10.0
def step(voice) -> f32:
  return voice() + voice()
def exercise(source: Voice, mode: i32) -> f32:
  if mode == 0:
    copy: Voice = source
    selected = copy
  else:
    copies: Voice[2] = [source, source]
    selected = copies[mode - 1]
  return step(selected)
buffers:
  result: f32
init:
  voice = Voice(result = result)
  frame: i32 = 0
sample:
  out1 = exercise(voice, frame % 3)
  out2 = result[0]
  frame += 1
"#;
    render_channel_blocks_with_buffers(
        source,
        &[&[2.0; 8], &[10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]],
        &[0.0],
        4,
    );
}

#[test]
fn unreachable_processor_calls_do_not_schedule_hooks() {
    for body in [
        "  return 0.0\n  return voice()",
        "  if in_value > 0:\n    return 0.0\n  else:\n    return 0.0\n  return voice()",
        "  for i in 0..2:\n    break\n    voice()\n  return 0.0",
        "  for i in 0..2:\n    continue\n    voice()\n  return 0.0",
    ] {
        let source = format!(
            r#"
proc Voice:
  init:
    before: i32 = 0
    after: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before)
    after += 1
def step(voice: Voice, in_value: i32) -> f32:
{body}
init:
  voice = Voice()
sample:
  step(voice, 0)
  out1 = f32(voice.before + voice.after)
"#
        );
        render_channel_blocks_with_buffers(&source, &[&[0.0; 12]], &[], 4);
    }
}

#[test]
fn returning_hook_branches_do_not_contribute_aliases_to_the_join() {
    let source = r#"
proc Voice:
  init:
    before: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before)
def step(a: Voice, b: Voice, mode: i32) -> f32:
  if mode == 0:
    selected = a
    return 0.0
  else:
    selected = b
  return selected()
init:
  a = Voice()
  b = Voice()
sample:
  step(a, b, 1)
  out1 = f32(a.before)
  out2 = f32(b.before)
"#;
    render_channel_blocks_with_buffers(
        source,
        &[&[0.0; 8], &[1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]],
        &[],
        4,
    );
}

#[test]
fn helper_local_processor_copies_run_pre_and_post_hooks() {
    for (declaration, receiver) in [
        ("copy: Voice = source", "copy"),
        ("copy: Voice[1] = [source]", "copy[0]"),
    ] {
        let source = format!(
            r#"
proc Voice:
  buffers:
    result: f32
  init:
    before: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before)
    result[0] += 10.0
def exercise(source: Voice) -> f32:
  {declaration}
  return {receiver}() + {receiver}()
buffers:
  result: f32
init:
  voice = Voice(result = result)
sample:
  out1 = exercise(voice)
  out2 = result[0]
"#
        );
        render_channel_blocks_with_buffers(
            &source,
            &[
                &[2.0; 12],
                &[
                    10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
                ],
            ],
            &[0.0],
            4,
        );
    }
}

#[test]
fn owned_processor_slice_copies_rebase_lifecycle_slots() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-slice-hooks.onda"
    );
    render_channels(source, &[&[131.0; 4]]);
    render_channels(
        &source.replace("step(copies[1])", "copies[1]()"),
        &[&[131.0; 4]],
    );
}

#[test]
fn owned_processor_slice_copies_preserve_resource_rows() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-slice-resources.onda"
    );
    let expected = [121.0, 122.0, 123.0, 124.0, 122.0, 123.0, 124.0, 125.0];
    for initializer in [
        "bank.voices",
        "[bank.voices[0], bank.voices[1], bank.voices[2]]",
    ] {
        let copied = source.replace(
            "  copies: Voice[2] = bank.voices[1:3]",
            &format!("  original: Voice[3] = {initializer}\n  copies: Voice[2] = original[1:3]"),
        );
        render_channel_blocks_with_buffers(
            &copied,
            &[&expected, &[10.0; 8], &expected],
            &[1.0, 10.0, 20.0],
            4,
        );
    }
    render_channel_blocks_with_buffers(
        source,
        &[&expected, &[10.0; 8], &expected],
        &[1.0, 10.0, 20.0],
        4,
    );
}

#[test]
fn owned_processor_slices_keep_resources_when_restored_mid_block() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-slice-resources.onda"
    );
    let source = source.replace(
        "block:\n  copies: Voice[2] = bank.voices[1:3]",
        "block:\n  if clips[0][0] == 1.0:\n    copies: Voice[2] = bank.voices[1:3]\n  else:\n    copies: Voice[2] = bank.voices[:2]",
    );
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
        let mut outputs = [[0.0_f32; 4]; 3];
        for (channel, output) in outputs.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                channel,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
            )
            .unwrap();
        }
        let mut original = [[1.0_f32], [10.0], [20.0]];
        let mut rebound = [[7.0_f32], [30.0], [40.0]];
        let mut snapshot: Option<Vec<u8>> = None;
        for (buffers, start, flags) in [
            (&mut original, 0, PROCESSOR_BEGIN_BLOCK),
            (&mut rebound, 2, PROCESSOR_END_BLOCK),
        ] {
            if let Some(snapshot) = &snapshot {
                instance.restore_state_bytes(snapshot).unwrap();
            }
            for (index, samples) in buffers.iter_mut().enumerate() {
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
            assert_eq!(
                process_checked_segment(
                    &mut instance,
                    start,
                    2,
                    flags,
                    onda_runtime::ExecutionOutput::none(),
                )
                .unwrap(),
                onda_runtime::PROCESSOR_EXECUTION_OK
            );
            snapshot = Some(instance.snapshot_state_bytes().unwrap());
        }
        let expected = [121.0, 122.0, 143.0, 144.0];
        assert_eq!(
            outputs,
            [expected, [10.0, 10.0, 30.0, 30.0], expected],
            "{opt_level:?}"
        );
        assert_eq!(original, [[1.0], [10.0], [20.0]]);
        assert_eq!(rebound, [[7.0], [30.0], [41.0]]);
    }
}

#[test]
fn owned_processor_direct_calls_retain_post_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-direct-post.onda"
    );
    let expected = [[0.0; 4], [1.0; 4], [2.0; 4]].concat();
    render_channel_blocks_with_buffers(source, &[&expected], &[0.0], 4);
}

#[test]
fn owned_processor_joined_copies_select_post_hook_resources() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-joined-post.onda"
    );
    let expected = [[0.0; 4], [1.0; 4], [2.0; 4]].concat();
    for source in [
        source.to_owned(),
        source.replace("step(copies[0])", "copies[0]()"),
    ] {
        render_channel_blocks_with_buffers(
            &source,
            &[&expected, &[0.0; 12], &expected],
            &[0.0; 3],
            4,
        );
        render_channel_blocks_with_buffers(
            &source.replace("select: i32 = 1", "select: i32 = 0"),
            &[&expected, &expected, &[0.0; 12]],
            &[0.0; 3],
            4,
        );
    }
}

#[test]
fn sliced_processor_views_preserve_owner_callbacks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-sliced-views.onda"
    );
    let nested = source.replace(
        "  step(window[0])",
        "  inner = window[:1]\n  step(inner[0])",
    );
    let dynamic = source.replace(
        "window = bank.voices[1:3]",
        "start: i32 = 1\n  window = bank.voices[start:3]",
    );
    let forwarded = source
        .replace(
            "def exercise",
            "def forward(voices: Voice[]) -> f32:\n  return step(voices[0])\ndef exercise",
        )
        .replace("step(window[0])", "forward(window)");
    let sliced_argument = forwarded.replace("forward(window)", "forward(bank.voices[1:3])");
    let direct = source.replace("step(window[0])", "window[0]()");
    let forwarded_direct = forwarded.replace("step(voices[0])", "voices[0]()");
    let fixed_forwarded = forwarded_direct.replace("voices: Voice[]", "voices: Voice[2]");
    let joined = source
        .replace(
            "  window = bank.voices[1:3]",
            "  if choose:\n    window = bank.voices[1:3]\n  else:\n    window = bank.voices[:1]",
        )
        .replace("bank: Bank)", "bank: Bank, choose: bool)")
        .replace("  bank = Bank()", "  bank = Bank()\n  choose = true")
        .replace(
            "exercise(bank)",
            "exercise(bank, choose)\n  choose = !choose",
        );
    for source in [
        source,
        &nested,
        &dynamic,
        &forwarded,
        &sliced_argument,
        &direct,
        &forwarded_direct,
    ] {
        render_channels(source, &[&[1.0, 2.0, 3.0, 4.0]]);
    }
    render_channels(&joined, &[&[1.0, 1.0, 2.0, 2.0]]);
    let errors = analyze(parse_program(&fixed_forwarded).unwrap()).unwrap_err();
    assert!(
        errors.iter().any(|error| error.message.contains(
            "expects fixed array length 2, but the argument length is not statically known"
        )),
        "{errors:?}"
    );
}

#[test]
fn runtime_processor_aliases_keep_block_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-runtime-alias-hooks.onda"
    );
    let expected = [[10.0; 4], [21.0; 4], [32.0; 4]].concat();
    let block_alias = source.replace(
        "sample:\n  selected = bank.voices[1]\n  out1 = selected()",
        "block:\n  selected = bank.voices[1]\n  sample:\n    out1 = selected()",
    );
    let joined = source.replace(
        "  selected = bank.voices[1]",
        "  if true:\n    selected = bank.voices[1]\n  else:\n    selected = bank.voices[0]",
    );
    for source in [source, &block_alias, &joined] {
        render_channel_blocks_with_buffers(source, &[&expected], &[], 4);
        render_segmented_lifecycle(source);
    }
}

#[test]
fn block_processor_roots_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-block-root-hooks.onda"
    );
    render_segmented_lifecycle(source);
    render_segmented_lifecycle(&source.replace("step(selected)", "selected()"));
}

#[test]
fn plain_processor_parameters_keep_tuple_output_calls() {
    let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0
def sum_voice(voice) -> f32:
  (left, right) = voice()
  return left + right
init:
  voice = Stereo()
sample:
  out1 = sum_voice(voice)
"#;
    render_channels(source, &[&[3.0; 4]]);
}

#[test]
fn block_owned_processor_copies_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-block-owned-hooks.onda"
    );
    render_segmented_lifecycle_outputs(source, &[30.0; 4]);
    for initializer in [
        "bank.voices[1:3]",
        "[bank.voices[0], bank.voices[1]]",
        "bank.voices[0]",
    ] {
        let variant = source
            .replace("voices: Voice[2]", "voices: Voice[3]")
            .replace(
                "copies: Voice[2] = bank.voices",
                &format!("copies: Voice[2] = {initializer}"),
            );
        render_segmented_lifecycle_outputs(&variant, &[30.0; 4]);
        render_segmented_lifecycle_outputs(&variant.replace("copies[0]", "copies[1]"), &[30.0; 4]);
    }
    let sliced = source
        .replace("proc Voice:", "proc Voice:\n  params:\n    id: i32 = 0")
        .replace("before * 10 + after", "before * 10 + after + id")
        .replace(
            "voices: Voice[2] = Voice()",
            "voices: Voice[3] = Voice(id = [1, 2, 3])",
        )
        .replace(
            "copies: Voice[2] = bank.voices",
            "copies: Voice[2] = bank.voices[1:3]",
        );
    render_segmented_lifecycle_outputs(&sliced, &[34.0; 4]);
    render_segmented_lifecycle_outputs(&sliced.replace("copies[0]", "copies[1]"), &[36.0; 4]);
    render_segmented_lifecycle_outputs(
        &source
            .replace("step(scalar)", "scalar()")
            .replace("step(copies[0])", "copies[0]()"),
        &[30.0; 4],
    );
    let resources = source
        .replace("proc Voice:", "proc Voice:\n  buffers:\n    trace: f32")
        .replace("    after += 1", "    after += 1\n    trace[0] += 1.0")
        .replace("proc Bank:", "proc Bank:\n  buffers:\n    trace: f32")
        .replace("= Voice()", "= Voice(trace = trace)")
        .replace("init:\n  bank", "buffers:\n  trace: f32\ninit:\n  bank")
        .replace("bank = Bank()", "bank = Bank(trace = trace)")
        .replace("step(scalar) +", "trace[0] * 100.0 + step(scalar) +");
    let expected = [[30.0; 4], [230.0; 4], [430.0; 4]].concat();
    render_channel_blocks_with_buffers(&resources, &[&expected], &[0.0], 4);
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            &resources,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        let mut output = [0.0_f32; 4];
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .unwrap();
        let mut bindings = [[0.0_f32], [10.0], [20.0]];
        let mut snapshot: Option<Vec<u8>> = None;
        for (index, trace) in bindings.iter_mut().enumerate() {
            if index == 2 {
                instance
                    .restore_state_bytes(snapshot.as_ref().unwrap())
                    .unwrap();
            }
            bind_buffer(
                &mut instance,
                0,
                trace.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            let (start, flags) = if index == 0 {
                (0, PROCESSOR_BEGIN_BLOCK)
            } else {
                (2, PROCESSOR_END_BLOCK)
            };
            assert_eq!(
                process_checked_segment(
                    &mut instance,
                    start,
                    2,
                    flags,
                    onda_runtime::ExecutionOutput::none(),
                )
                .unwrap(),
                onda_runtime::PROCESSOR_EXECUTION_OK
            );
            if index == 0 {
                assert_eq!(&output[..2], &[30.0; 2]);
                snapshot = Some(instance.snapshot_state_bytes().unwrap());
            } else {
                assert_eq!(&output[2..], &[30.0 + index as f32 * 1000.0; 2]);
                assert_eq!(trace[0], index as f32 * 10.0 + 2.0);
            }
        }
        assert_eq!(
            bindings[0],
            [0.0],
            "post hooks must use the current host binding"
        );
    }
}

#[test]
fn sliced_processor_views_keep_resources_hooks_and_retained_offsets() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-sliced-resources.onda"
    );
    let expected = [112.0, 113.0, 114.0, 115.0, 226.0, 227.0, 228.0, 229.0];
    let callbacks = [11.0, 12.0, 13.0, 14.0, 125.0, 126.0, 127.0, 128.0];
    let dynamic = source.replace(
        "  window = bank.voices[start:3]\nsample:",
        "sample:\n  window = bank.voices[start:3]",
    );
    let nested = source.replace(
        "  selected = window[0]",
        "  inner = window[:1]\n  selected = inner[0]",
    );
    let direct = source.replace("step(selected)", "window[0]()");
    let forwarded = source
        .replace(
            "def step",
            "def forward(voices: Voice[]) -> f32:\n  return voices[0]()\ndef step",
        )
        .replace("step(selected)", "forward(window)");
    for source in [source, &dynamic, &nested, &direct, &forwarded] {
        render_channel_blocks_with_buffers(
            source,
            &[&expected, &callbacks, &expected],
            &[7.0, 11.0, 13.0],
            4,
        );
    }
    let events = source
        .replace(
            "  block:\n",
            "  event add(value: i32):\n    count += value\n    fired(value)\n  block:\n",
        )
        .replace(
            "  out1 = step(selected)",
            "  window[0].add(2)\n  out1 = step(selected)",
        );
    let event_output = [114.0, 117.0, 120.0, 123.0];
    render_channels_with_buffers(
        &events,
        &[&event_output, &[13.0, 16.0, 19.0, 22.0], &event_output],
        &[7.0, 11.0, 13.0],
    );
    render_channels_with_buffers(
        &events.replace("window[0].add(2)", "selected.add(2)"),
        &[&event_output, &[13.0, 16.0, 19.0, 22.0], &event_output],
        &[7.0, 11.0, 13.0],
    );
}

#[test]
fn processor_slice_selectors_run_once_and_empty_selection_fails() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-sliced-views.onda"
    );
    let source = source
        .replace(
            "def exercise",
            "def choose(calls: i32[1]) -> i32:\n  calls[0] += 1\n  return 1\ndef exercise",
        )
        .replace("bank: Bank)", "bank: Bank, calls: i32[1])")
        .replace("bank.voices[1:3]", "bank.voices[choose(calls):3]")
        .replace("  bank = Bank()", "  bank = Bank()\n  calls: i32[1] = [0]")
        .replace(
            "exercise(bank)",
            "exercise(bank, calls)\n  out2 = f32(calls[0])",
        );
    let expected: &[f32] = &[1.0, 2.0, 3.0, 4.0];
    render_channels(&source, &[expected; 2]);
    let empty = source.replace("bank.voices[choose(calls):3]", "bank.voices[1:1]");
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            &empty,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        assert!(process_interleaved(&mut instance, &[], &mut [0.0; 8], 4).is_err());
    }
}

#[test]
fn retained_processor_slices_resolve_rebound_buffers_after_restore() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-sliced-resources.onda"
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
        let mut outputs = [[0.0_f32; 4]; 3];
        for (channel, output) in outputs.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                channel,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
            )
            .unwrap();
        }
        let mut original = [[7.0_f32], [11.0], [13.0]];
        let mut rebound = [[70.0_f32], [17.0], [130.0]];
        let mut snapshot: Option<Vec<u8>> = None;
        for (buffers, expected, total) in [
            (
                &mut original,
                [112.0, 113.0, 114.0, 115.0],
                [11.0, 12.0, 13.0, 14.0],
            ),
            (
                &mut rebound,
                [232.0, 233.0, 234.0, 235.0],
                [125.0, 126.0, 127.0, 128.0],
            ),
        ] {
            if let Some(snapshot) = &snapshot {
                instance.restore_state_bytes(snapshot).unwrap();
            }
            for (index, samples) in buffers.iter_mut().enumerate() {
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
            for (start, flags) in [(0, PROCESSOR_BEGIN_BLOCK), (2, PROCESSOR_END_BLOCK)] {
                assert_eq!(
                    process_checked_segment(
                        &mut instance,
                        start,
                        2,
                        flags,
                        onda_runtime::ExecutionOutput::none(),
                    )
                    .unwrap(),
                    onda_runtime::PROCESSOR_EXECUTION_OK
                );
            }
            assert_eq!(outputs, [expected, total, expected], "{opt_level:?}");
            if snapshot.is_none() {
                snapshot = Some(instance.snapshot_state_bytes().unwrap());
                assert_eq!(
                    onda_runtime::process_checked(
                        &mut instance,
                        onda_runtime::ExecutionOutput::none()
                    )
                    .unwrap(),
                    onda_runtime::PROCESSOR_EXECUTION_OK
                );
            }
        }
    }
}

#[test]
fn deferred_resource_selectors_bind_generic_and_overloaded_helpers() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-resource-selector-calls.onda"
    );
    let expected: &[f32] = &[7.0; 4];
    render_channels_with_buffers(source, &[expected; 4], &[0.0, 0.0]);
}

#[test]
fn inherited_resource_selectors_use_the_operation_sample_rate() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-inherited-resource-context.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("step(bank.voice)", "bank.voice()"),
        source.replace("out2 = clips[choose()][0]", "out2 = 7.0"),
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
            let mut buffers = [[0.0_f32]; 2];
            for (index, samples) in buffers.iter_mut().enumerate() {
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
            for _ in 0..2 {
                let mut output = [0.0; 8];
                process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
                for frame in output.as_chunks::<2>().0 {
                    assert_eq!(frame[0], frame[1], "{opt_level:?}\n{source}");
                }
            }
        }
    }
}

#[test]
fn repeated_processor_joins_keep_selected_state_and_callbacks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-repeated-joins.onda"
    );
    render_channels(
        source,
        &[
            &[1.0, 1.0, 2.0, 2.0],
            &[1.0, 1.0, 3.0, 3.0],
            &[0.0, 1.0, 1.0, 3.0],
        ],
    );
    let three_owners = source.replace(
        "  return a8()",
        "  third = other.voices[0]\n  if choose:\n    selected = a8\n  else:\n    selected = third\n  return selected()",
    ).replace(
        "right: Bank, choose: bool", "right: Bank, other: Bank, choose: bool",
    ).replace("  choose = true", "  other = Bank()\n  choose = true")
        .replace("exercise(left, right, choose)", "exercise(left, right, other, choose)")
        .replace("f32(right.total)", "f32(other.total)");
    render_channels(
        &three_owners,
        &[
            &[1.0, 1.0, 2.0, 2.0],
            &[1.0, 1.0, 3.0, 3.0],
            &[0.0, 1.0, 1.0, 3.0],
        ],
    );
}

#[test]
fn inherited_processor_resources_keep_identity_and_evaluation_order() {
    let identity = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-inherited-resource-identity.onda");
    let order = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-inherited-resource-order.onda");
    for (source, value, calls) in [(identity, 77.0, 1.0), (order, 117.0, 2.0)] {
        for array in [false, true] {
            let source = if array {
                source
                    .replace("    voice = Voice(", "    voices: Voice[2] = Voice(")
                    .replace("out1 = voice()", "out1 = voices[1]()")
                    .replace("step(bank.voice)", "step(bank.voices[1])")
            } else {
                source.to_owned()
            };
            render_channels_with_buffers(
                &source,
                &[&[value; 4], &[calls; 4], &[value; 4], &[calls; 4]],
                &[0.0, 0.0],
            );
        }
    }
}

#[test]
fn runtime_owned_processor_copies_preserve_resources_through_forwarding() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-runtime-owned-resources.onda"
    );
    let inferred = source.replace("step(voice: Voice)", "step(voice)");
    let joined = source
        .replace("block:\n", "  other = Voice(clip = right)\n  choose = true\nblock:\n")
        .replace(
            "    sample_copy: Voice = voices[0]",
            "    if choose:\n      selected = voices[0]\n    else:\n      selected = other\n    sample_copy: Voice = selected\n    choose = !choose",
        );
    let looped = source.replace(
        "    sample_copy: Voice = voices[0]\n    sample_copy = voices[1]\n    out1 = forward(sample_copy)",
        "    for index in 0..2:\n      sample_copy: Voice = voices[index]\n      sample_copy = voices[1]\n      out1 = forward(sample_copy)",
    );
    let array = source.replace(
        "    sample_copy: Voice = voices[0]",
        "    sample_copies: Voice[2] = voices\n    sample_copy = sample_copies[0]",
    );
    for (source, expected) in [
        (source, [8.0; 8]),
        (&inferred, [8.0; 8]),
        (&joined, [8.0, 12.0, 8.0, 12.0, 8.0, 12.0, 8.0, 12.0]),
        (&looped, [12.0; 8]),
        (&array, [8.0; 8]),
    ] {
        render_channel_blocks_with_buffers(
            source,
            &[
                &expected,
                &[12.0, 13.0, 14.0, 15.0, 12.0, 13.0, 14.0, 15.0],
                &[0.0; 8],
            ],
            &[7.0, 11.0],
            4,
        );
    }
}

#[test]
fn runtime_owned_processor_copies_resolve_rebound_buffers_across_segments() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-runtime-owned-resources.onda"
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut outputs = [[0.0_f32; 4]; 3];
        for (index, output) in outputs.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                index,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
            )
            .unwrap();
        }
        for mut buffers in [[[7.0_f32], [11.0]], [[13.0], [17.0]]] {
            instance.restore_state_bytes(&snapshot).unwrap();
            for (index, samples) in buffers.iter_mut().enumerate() {
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
            assert_eq!(
                process_checked_segment(
                    &mut instance,
                    0,
                    2,
                    PROCESSOR_BEGIN_BLOCK,
                    onda_runtime::ExecutionOutput::none(),
                )
                .unwrap(),
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
                .unwrap(),
                onda_runtime::PROCESSOR_EXECUTION_OK
            );
            let [left, right] = buffers.map(|samples| samples[0]);
            assert_eq!(outputs[0], [left + 1.0; 4], "{opt_level:?}");
            assert_eq!(
                outputs[1],
                [right + 1.0, right + 2.0, right + 3.0, right + 4.0],
                "{opt_level:?}"
            );
            assert_eq!(outputs[2], [0.0; 4], "{opt_level:?}");
        }
    }
}

#[test]
fn owned_processor_arrays_keep_independent_state_and_selected_resources() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-array-resources.onda"
    );
    let parameter = source
        .replace("exercise(bank: Bank,", "exercise(voices: Voice[2],")
        .replace("= bank.voices", "= voices")
        .replace("exercise(bank, index)", "exercise(bank.voices, index)");
    for (source, expected) in [
        (source.to_owned(), [64.0, 119.0, 64.0, 119.0]),
        (
            source.replace("forward(copies, index)", "step(copies[index])"),
            [64.0, 119.0, 64.0, 119.0],
        ),
        (parameter, [64.0, 119.0, 64.0, 119.0]),
        (
            source.replace(
                "copies: Voice[2] = bank.voices",
                "copies: Voice[2] = [bank.voices[1], bank.voices[0]]",
            ),
            [119.0, 64.0, 119.0, 64.0],
        ),
        (
            source.replace(
                "copies: Voice[2] = bank.voices",
                "copies: Voice[2] = [bank.voices[0], bank.voices[0]]",
            ),
            [64.0; 4],
        ),
        (
            source.replace("= bank.voices\n", "= bank.voices[1]\n"),
            [119.0; 4],
        ),
    ] {
        render_channels_with_buffers(&source, &[&expected, &[0.0; 4], &[11.0; 4]], &[0.0, 0.0]);
    }
}

#[test]
fn processor_element_copies_preserve_resource_identity_after_rebinding() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-element-resources.onda"
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut bindings = [[[7.0_f32], [11.0]], [[11.0], [13.0]]];
        for (buffers, expected) in bindings.iter_mut().zip([77.0, 121.0]) {
            instance.restore_state_bytes(&snapshot).unwrap();
            for (index, samples) in buffers.iter_mut().enumerate() {
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
            let mut output = [0.0; 8];
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(
                output,
                [expected, 1.0, expected, 1.0, expected, 1.0, expected, 1.0]
            );
        }
    }
}

#[test]
fn owned_processor_arrays_use_current_buffers_after_snapshot_restore() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-array-resources.onda"
    )
    .replace("    clip[0] += 1.0\n", "")
    .replace("  left[0] = 2.0\n  right[0] = 7.0\n", "")
    .replace(
        "  out2 = f32(bank.voices[index].count)\n  out3 = left[0] + right[0]\n",
        "",
    );
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut bindings = [[[2.0_f32], [7.0]], [[11.0], [13.0]]];
        for (buffers, expected) in bindings
            .iter_mut()
            .zip([[43.0, 98.0, 43.0, 98.0], [142.0, 164.0, 142.0, 164.0]])
        {
            instance.restore_state_bytes(&snapshot).unwrap();
            for (name, samples) in ["left", "right"].into_iter().zip(buffers) {
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
            assert_eq!(output, expected, "{opt_level:?}");
        }
    }
}

#[test]
fn owned_processor_views_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-hooks.onda"
    );
    render_segmented_lifecycle(source);
    render_segmented_lifecycle(
        &source
            .replace("  bank = Bank()", "  banks: Bank[2] = Bank()")
            .replace("copy: Bank = bank", "copy: Bank = banks[1]"),
    );
}

#[test]
fn owned_local_processor_copies_activate_without_scheduling_source_hooks() {
    let source = r#"
proc Voice:
  init:
    before: i32 = 0
    after: i32 = 0
  block:
    before += 1
    sample:
      out1 = f32(before * 10 + after)
    after += 1
def exercise(source: Voice) -> f32:
  copy: Voice = source
  return copy()
init:
  voice = Voice()
sample:
  out3 = exercise(voice)
  out1 = f32(voice.before)
  out2 = f32(voice.after)
"#;
    let alias = source.replace("  return copy()", "  alias = copy\n  return alias()");
    let forwarded = source
        .replace(
            "def exercise(source: Voice)",
            "def step(voice) -> f32:\n  return voice()\ndef exercise(source: Voice)",
        )
        .replace("  return copy()", "  return step(copy)");
    let replaced = source.replace("  return copy()", "  copy = source\n  return copy()");
    for source in [source, &alias, &forwarded, &replaced] {
        render_channel_blocks_with_buffers(source, &[&[0.0; 12], &[0.0; 12], &[10.0; 12]], &[], 4);
    }
}

#[test]
fn owned_scalar_processor_copies_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-root-hooks.onda"
    );
    for owner in ["voice", "voices[0]"] {
        let source = source
            .replace(
                "  voice = Voice()",
                if owner == "voice" {
                    "  voice = Voice()"
                } else {
                    "  voices: Voice[2] = Voice()"
                },
            )
            .replace(
                "sample:\n  out1 = step(voice)",
                &format!("  copy: Voice = {owner}\nsample:\n  out1 = step(copy)"),
            );
        render_segmented_lifecycle(&source);
        render_segmented_lifecycle(&source.replace("step(copy)", "copy()"));
        render_segmented_lifecycle(&source.replace(
            "sample:\n  out1 = step(copy)",
            "  alias = copy\nsample:\n  out1 = step(alias)\n  copy()",
        ));
    }
}

#[test]
fn owned_scalar_processor_hooks_use_current_buffers_after_restore() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-root-hooks.onda"
    )
    .replace("proc Voice:", "proc Voice:\n  buffers:\n    source: f32")
    .replace("before += 1", "before += i32(source[0])")
    .replace(
        "  voice = Voice()",
        "  voices: Voice[2] = Voice(source = clip)\n  copy: Voice = voices[0]",
    )
    .replace(
        "init:\n  voices:",
        "buffers:\n  clip: f32\ninit:\n  voices:",
    )
    .replace("out1 = step(voice)", "out1 = step(copy)");
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut samples = [0.0_f32];
        for value in [7.0, 11.0] {
            instance.restore_state_bytes(&snapshot).unwrap();
            samples[0] = value;
            bind_buffer(
                &mut instance,
                0,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            let mut output = [0.0; 4];
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, [value * 10.0; 4], "{opt_level:?}");
        }
    }
}

#[test]
fn owned_processor_views_keep_resources_after_snapshot_and_rebinding() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-owned-resources.onda"
    );
    let source = source.replace(
        "  saved: Bank = bank",
        "  copy0: Bank = bank\n  copy1: Bank = copy0\n  copy2: Bank = copy1\n  saved: Bank = copy2",
    );
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
        let mut output = [0.0; 4];
        let mut samples = [7.0_f32];
        let snapshot = instance.snapshot_state_bytes().unwrap();
        for expected in [7.0, 11.0] {
            instance.restore_state_bytes(&snapshot).unwrap();
            samples[0] = expected;
            bind_buffer(
                &mut instance,
                0,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, [expected + 1.0; 4], "{opt_level:?}");
        }
    }
}

#[test]
fn forwarded_processor_views_do_not_evaluate_inactive_resources() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-inactive-resources.onda"
    );
    for (body, expected) in [("voice()", 7.0), ("voice.out1", 0.0), ("25.0", 25.0)] {
        let source = source.replace("return voice()", &format!("return {body}"));
        render_channels_with_buffers(&source, &[&[expected; 4]], &[7.0, 0.0]);
    }
    // The same selector must still fail when its environment is selected.
    let source = source.replace("first = false", "first = true");
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
        assert!(process_interleaved(&mut instance, &[], &mut [0.0; 4], 4).is_err());
    }
}

#[test]
fn forwarded_processor_resource_recipes_use_current_declaration_bindings() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-root-resource-order.onda"
    );
    render_channels_with_buffers(source, &[&[7.0; 4]], &[0.0, 0.0]);
    let alias = source
        .replace(
            "  voices: Voice[2]",
            "  original = selection\n  voices: Voice[2]",
        )
        .replace("clips[selection.index]", "clips[original.index]")
        .replace(
            "def use_voice(voice, selection: i32)",
            "def use_voice(voice, original: i32)",
        );
    render_channels_with_buffers(&alias, &[&[7.0; 4]], &[0.0, 0.0]);
}

#[test]
fn heterogeneous_root_resources_defer_only_the_selected_choices() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-root-resource-choices.onda"
    );
    render_channels_with_buffers(source, &[&[2431.0; 4]], &[7.0, 11.0, 13.0, 100.0, 200.0]);
    let static_choices = source.replace("clips[choose(selection)]", "clips[0]");
    render_channels_with_buffers(
        &static_choices,
        &[&[427.0; 4]],
        &[7.0, 11.0, 13.0, 100.0, 200.0],
    );
    let inactive = source
        .replace("Selection(0, 0)", "Selection(1, 0)")
        .replace("return selection.index", "return 1 / selection.index")
        .replace("  a = forward(selected, selection)", "  a = voices[1]()")
        .replace("  b = voices[0]()", "  b = voices[1]()");
    render_channels_with_buffers(&inactive, &[&[639.0; 4]], &[7.0, 11.0, 13.0, 100.0, 200.0]);
    let active = inactive.replace("  a = voices[1]()", "  a = selected()");
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            &active,
            4,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 4,
                fast_math: false,
                opt_level,
            },
        );
        assert!(process_interleaved(&mut instance, &[], &mut [0.0; 4], 4).is_err());
    }
}

#[test]
fn heterogeneous_root_resources_observe_rebinding_after_restore() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-root-resource-choices.onda"
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
        let snapshot = instance.snapshot_state_bytes().unwrap();
        let mut original = [7.0_f32, 11.0, 13.0, 100.0, 200.0];
        let mut replacement = [14.0_f32, 22.0, 26.0, 200.0, 400.0];
        for (samples, expected) in [(&mut original, 2431.0), (&mut replacement, 2862.0)] {
            instance.restore_state_bytes(&snapshot).unwrap();
            for (index, sample) in samples.iter_mut().enumerate() {
                bind_buffer(
                    &mut instance,
                    index,
                    std::ptr::from_mut(sample).cast(),
                    1,
                    1,
                    48_000.0,
                    PrimitiveType::F32,
                )
                .unwrap();
            }
            let mut output = [0.0; 4];
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, [expected; 4], "{opt_level:?}");
        }
    }
}

fn render_channels(source: &str, expected: &[&[f32]]) {
    render_channels_with_buffers(source, expected, &[]);
}

fn render_channels_with_buffers(source: &str, expected: &[&[f32]], buffers: &[f32]) {
    render_channel_blocks_with_buffers(source, expected, buffers, expected[0].len());
}

fn render_channel_blocks_with_buffers(
    source: &str,
    expected: &[&[f32]],
    buffers: &[f32],
    frames: usize,
) {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
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
        let mut buffers = buffers.iter().map(|value| [*value]).collect::<Vec<_>>();
        for (index, samples) in buffers.iter_mut().enumerate() {
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
        let mut output = vec![0.0; expected[0].len() * expected.len()];
        for block in output.chunks_exact_mut(frames * expected.len()) {
            process_interleaved(&mut instance, &[], block, frames).unwrap();
        }
        for (channel, values) in expected.iter().enumerate() {
            let actual: Vec<_> = output
                .chunks_exact(expected.len())
                .map(|frame| frame[channel])
                .collect();
            assert_eq!(&actual, values, "{opt_level:?}, channel {channel}");
        }
    }
}

fn render_segmented_lifecycle(source: &str) {
    render_segmented_lifecycle_outputs(source, &[10.0, 21.0, 21.0, 32.0]);
}

fn render_segmented_lifecycle_outputs(source: &str, expected: &[f32]) {
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
        let mut output = [0.0_f32; 4];
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .unwrap();
        let mut snapshot: Option<Vec<u8>> = None;
        for (block, expected) in expected.iter().copied().enumerate() {
            if block == 2 {
                instance
                    .restore_state_bytes(snapshot.as_ref().unwrap())
                    .unwrap();
            }
            for (start, flags) in [(0, PROCESSOR_BEGIN_BLOCK), (2, PROCESSOR_END_BLOCK)] {
                assert_eq!(
                    process_checked_segment(
                        &mut instance,
                        start,
                        2,
                        flags,
                        onda_runtime::ExecutionOutput::none(),
                    )
                    .unwrap(),
                    onda_runtime::PROCESSOR_EXECUTION_OK
                );
            }
            assert_eq!(output, [expected; 4], "{opt_level:?}\n{source}");
            if block == 0 {
                snapshot = Some(instance.snapshot_state_bytes().unwrap());
            }
        }
    }
}

#[test]
fn scalar_root_views_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-root-hooks.onda"
    );
    let alias = source
        .replace("  voice = Voice()", "  voice = Voice()\n  selected = voice")
        .replace("out1 = step(voice)", "out1 = step(selected)");
    let direct = source.replace("out1 = step(voice)", "out1 = voice()");
    let both = source.replace("out1 = step(voice)", "out1 = step(voice)\n  voice()");
    let joined = source
        .replace("init:\n  voice", "def choose(left, right, first: bool) -> f32:\n  if first:\n    selected = left\n  else:\n    selected = right\n  return step(selected)\ninit:\n  voice")
        .replace("  voice = Voice()", "  voice = Voice()\n  other = Voice()\n  first = true")
        .replace("out1 = step(voice)", "out1 = choose(voice, other, first)\n  first = !first");
    let alias_and_direct =
        alias.replace("out1 = step(selected)", "out1 = step(selected)\n  voice()");
    let mixed = source
        .replace(
            "init:\n  voice",
            "proc Rack:\n  init:\n    voice = Voice()\n  sample:\n    out1 = 0.0\ninit:\n  voice",
        )
        .replace(
            "  voice = Voice()\nsample:",
            "  voice = Voice()\n  rack = Rack()\nsample:",
        )
        .replace(
            "out1 = step(voice)",
            "out1 = step(voice) + 0.0 * step(rack.voice)",
        );
    for source in [
        source,
        &alias,
        &direct,
        &both,
        &joined,
        &alias_and_direct,
        &mixed,
    ] {
        render_segmented_lifecycle(source);
    }
}

#[test]
fn scalar_root_hooks_follow_explicit_process_flags() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-root-hooks.onda"
    );
    let direct = source.replace("out1 = step(voice)", "out1 = voice()");
    for source in [source, &direct] {
        let (mut instance, _, _) = compile_instance(source, 4);
        let mut output = [0.0_f32; 4];
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            std::mem::size_of_val(&output),
        )
        .unwrap();
        assert_eq!(
            process_checked_segment(
                &mut instance,
                0,
                4,
                0,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap(),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(output, [0.0; 4]);
        assert_eq!(
            process_checked_segment(
                &mut instance,
                0,
                0,
                PROCESSOR_BEGIN_BLOCK,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap(),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(
            process_checked_segment(
                &mut instance,
                0,
                4,
                0,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap(),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(output, [10.0; 4]);
        assert_eq!(
            process_checked_segment(
                &mut instance,
                4,
                0,
                PROCESSOR_END_BLOCK,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap(),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(
            process_checked_segment(
                &mut instance,
                0,
                4,
                0,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap(),
            onda_runtime::PROCESSOR_EXECUTION_OK
        );
        assert_eq!(output, [11.0; 4]);
    }
}

#[test]
fn processor_hook_arguments_execute_before_activation() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-hook-arguments.onda"
    );
    let dynamic = source
        .replace(
            "  direct_count: i32 = 0",
            "  direct_count: i32 = 0\n  index: i32 = 0",
        )
        .replace("\n  out1 = voices[0](", "\n  out1 = voices[index](");
    let aliased = source.replace(
        "  out1 = voices[0](f32(direct_count))",
        "  selected = voices[0]\n  out1 = selected(f32(direct_count))",
    );
    for source in [source, &dynamic, &aliased] {
        let expected = [0.0, 10.0, 10.0, 10.0];
        render_channels(source, &[&expected, &expected, &expected]);
    }
}

#[test]
fn nested_processor_calls_activate_hooks_without_callbacks() {
    let source = r#"
proc Voice:
  init:
    count: i32 = 0
  block:
    count += 1
    sample:
      out1 = f32(count)
proc Bank:
  init:
    voices: Voice[2] = Voice()
  sample:
    out1 = voices[0]()
init:
  bank = Bank()
sample:
  out1 = bank()
"#;
    render_channels(source, &[&[1.0; 4]]);
}

#[test]
fn processor_hook_arguments_fail_before_callbacks() {
    let source = r#"
proc Voice:
  ins:
    value: f32
  delegate fired()
  block:
    fired()
    sample:
      out1 = value
def step(voice, value: f32) -> f32:
  return voice(value)
buffers:
  marks: f32
init:
  voices: Voice[2] = Voice()
  zero: i32 = 0
when voices[0].fired():
  marks[0] += 1.0
sample:
  out1 = voices[0](f32(1 / zero))
"#;
    for call in [
        "voices[0](f32(1 / zero))",
        "voices[zero](f32(1 / zero))",
        "step(voices[0], f32(1 / zero))",
    ] {
        let source = source.replace("voices[0](f32(1 / zero))", call);
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
            let mut marks = [0.0_f32];
            let index = instance.buffer_index("marks").unwrap();
            bind_buffer(
                &mut instance,
                index,
                marks.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            assert!(process_interleaved(&mut instance, &[], &mut [0.0; 4], 4).is_err());
            assert_eq!(marks, [0.0], "{call}, {opt_level:?}");
        }
    }
}

#[test]
fn joined_processor_array_owners_preserve_lifecycle_references() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-joined-owner-hooks.onda"
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
        let mut outputs = [[0.0_f32; 4]; 3];
        for (channel, output) in outputs.iter_mut().enumerate() {
            bind_output(
                &mut instance,
                channel,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(output),
            )
            .unwrap();
        }
        let mut snapshot: Option<Vec<u8>> = None;
        for (block, expected) in [
            [1.0, 100.0, 100.0],
            [12.0, 1200.0, 1200.0],
            [12.0, 1200.0, 1200.0],
        ]
        .into_iter()
        .enumerate()
        {
            if block == 2 {
                instance
                    .restore_state_bytes(snapshot.as_ref().unwrap())
                    .unwrap();
            }
            for (start, flags) in [(0, PROCESSOR_BEGIN_BLOCK), (2, PROCESSOR_END_BLOCK)] {
                assert_eq!(
                    process_checked_segment(
                        &mut instance,
                        start,
                        2,
                        flags,
                        onda_runtime::ExecutionOutput::none(),
                    )
                    .unwrap(),
                    onda_runtime::PROCESSOR_EXECUTION_OK
                );
            }
            for (actual, value) in outputs.iter().zip(expected) {
                assert_eq!(*actual, [value; 4], "block {block}, {opt_level:?}");
            }
            if block == 0 {
                snapshot = Some(instance.snapshot_state_bytes().unwrap());
            }
        }
    }
}

#[test]
fn scalar_child_views_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-hooks.onda"
    );
    let direct = source.replace("step(bank.voice)", "bank.voice()");
    let retained = direct
        .replace(
            "  bank = Bank()",
            "  bank = Bank()\n  selected = bank.voice",
        )
        .replace("  out1 = bank.voice()", "  out1 = selected()");
    let parent = source
        .replace("  out1 = step(bank.voice)", "  out1 = bank()")
        .replace("  out1 = voice()", "  out1 = step(voice)");
    let both = source.replace(
        "  out1 = step(bank.voice)",
        "  out1 = step(bank.voice)\n  bank()",
    );
    let selected = source
        .replace(
            "  bank = Bank()",
            "  banks: Bank[2] = Bank()\n  selector: i32 = 1",
        )
        .replace(
            "  out1 = step(bank.voice)",
            "  owner = banks[selector]\n  selector = 1 - selector\n  out1 = step(owner.voice)",
        );
    let selected_parent = selected.replace(
        "  out1 = step(owner.voice)",
        "  out1 = step(owner.voice)\n  owner()",
    );
    let nested = format!("{}\nproc Rack:\n  init:\n    banks: Bank[2] = Bank()\n  sample:\n    out1 = 0.0\ninit:\n  rack = Rack()\n  selector: i32 = 1\nsample:\n  owner = rack.banks[selector]\n  selector = 1 - selector\n  out1 = step(owner.voice)\n", source.split("\ninit:\n  bank = Bank()").next().unwrap());
    for source in [
        source,
        &direct,
        &retained,
        &parent,
        &both,
        &selected,
        &selected_parent,
        &nested,
    ] {
        render_segmented_lifecycle(source);
    }
}

#[test]
fn scalar_child_post_callbacks_run_before_the_parent_post_hook() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-hook-order.onda"
    );
    render_segmented_lifecycle_outputs(source, &[10.0, 121.0, 121.0, 232.0]);
}

#[test]
fn array_child_post_callbacks_run_before_the_parent_post_hook() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-array-hook-order.onda"
    );
    let nested = source
        .replace("  bank = Bank()", "  banks: Bank[2] = Bank()")
        .replace("  out1 = bank()", "  out1 = banks[0]()");
    for source in [source, &nested] {
        render_segmented_lifecycle_outputs(source, &[10.0, 121.0, 121.0, 232.0]);
    }
}

#[test]
fn borrowed_child_post_hooks_survive_parent_task_suspension() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-suspended-parent-hooks.onda"
    );
    let array = source
        .replace("voice = Voice()", "voices: Voice[2] = Voice()")
        .replace("step(bank.voice)", "step(bank.voices[0])");
    let nested = source
        .replace("init:\n  bank = Bank()", "proc Rack:\n  init:\n    bank = Bank()\n  sample:\n    out1 = bank()\ninit:\n  rack = Rack()")
        .replace("step(bank.voice)", "step(rack.bank.voice)")
        .replace("\n  bank()", "\n  rack()");
    let inactive_owner = nested.replace("    out1 = bank()", "    out1 = 0.0");
    for source in [source, &array, &nested, &inactive_owner] {
        render_segmented_lifecycle(source);
    }
}

#[test]
fn borrowed_descendant_post_callbacks_precede_the_active_owner_post_hook() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-borrowed-descendant-hook-order.onda"
    );
    render_segmented_lifecycle_outputs(source, &[10.0, 121.0, 121.0, 232.0]);
}

#[test]
fn child_completion_reuses_the_parent_operations_prepared_resources() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-hook-resource-order.onda"
    );
    render_channel_blocks_with_buffers(
        source,
        &[&[3.0, 4.0, 5.0, 6.0, 9.0, 10.0, 11.0, 12.0]],
        &[0.0, 0.0],
        4,
    );
}

#[test]
fn scalar_child_init_aliases_remain_callable() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-init-alias.onda"
        ),
        &[&[1.0, 2.0, 3.0, 4.0]],
    );
}

#[test]
fn scalar_children_of_selected_owners_keep_their_nominal_type() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-selected-owner-child.onda"
    );
    let forwarded = source.replace("owner.voice()", "step(owner.voice)");
    let alias = source.replace(
        "out1 = owner.voice()",
        "selected = owner.voice\n  out1 = selected()",
    );
    let nested = source.replace("init:\n  banks: Bank[2] = Bank()", "proc Rack:\n  init:\n    banks: Bank[2] = Bank()\n  sample:\n    out1 = 0.0\ninit:\n  rack = Rack()")
        .replace("owner = banks[selector]", "owner = rack.banks[selector]");
    let nested_alias = nested.replace(
        "out1 = owner.voice()",
        "selected = owner.voice\n  out1 = selected()",
    );
    for source in [source, &forwarded, &alias, &nested, &nested_alias] {
        render_channels(source, &[&[1.0, 1.0, 2.0, 2.0], &[1.0, 1.0, 2.0, 2.0]]);
    }
}

#[test]
fn selected_parent_resources_compose_with_child_collection_windows() {
    let source = include_str!("../../../../packages/onda_binaryen_web/test/fixtures/processor-selected-resource-windows.onda");
    let direct = source.replace("step(owner.voice)", "owner.voice()");
    for source in [source, &direct] {
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
            let mut output = [0.0_f32; 4];
            let mut snapshot: Option<Vec<u8>> = None;
            let mut original = std::array::from_fn::<_, 8, _>(|index| [(index + 1) as f32]);
            let mut rebound = original.map(|samples| [samples[0] * 10.0]);
            for (buffers, expected) in [
                (&mut original, [6.0, 14.0, 7.0, 15.0]),
                (&mut rebound, [51.0, 131.0, 52.0, 132.0]),
            ] {
                for (index, samples) in buffers.iter_mut().enumerate() {
                    let buffer = instance.buffer_index(&format!("clips[{index}]")).unwrap();
                    bind_buffer(
                        &mut instance,
                        buffer,
                        samples.as_mut_ptr().cast(),
                        1,
                        1,
                        48_000.0,
                        PrimitiveType::F32,
                    )
                    .unwrap();
                }
                if let Some(snapshot) = &snapshot {
                    instance.restore_state_bytes(snapshot).unwrap();
                }
                process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
                assert_eq!(output, expected, "{opt_level:?}");
                if snapshot.is_none() {
                    snapshot = Some(instance.snapshot_state_bytes().unwrap());
                }
            }
            assert_eq!(
                original.map(|samples| samples[0]),
                [1.0, 4.0, 3.0, 4.0, 5.0, 8.0, 7.0, 8.0]
            );
        }
    }
}

#[test]
fn selected_processor_views_keep_segmented_lifecycle_hooks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-selected-hooks.onda"
    );
    let top_level = format!("{}\ninit:\n  voices: Voice[2] = Voice()\n  selector: i32 = 0\nsample:\n  out1 = forward(voices, selector)\n  selector = 1 - selector\n", source.split("proc Bank:").next().unwrap());
    let borrowed_child = source.replace("  out1 = bank()", "  out1 = forward(bank.voices, 0)");
    for source in [source, &top_level, &borrowed_child] {
        render_segmented_lifecycle(source);
    }
}

#[test]
fn nested_processor_array_aliases_and_joined_direct_calls_execute() {
    let arrays = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-array-aliases.onda"
    );
    let selected = arrays
        .replace("selected = left.voices", "selected = left.voices[0]")
        .replace("selected = right.voices", "selected = right.voices[1]")
        .replace("return step(selected, 1)", "return selected()");
    let copied = selected.replace("return selected()", "copy = selected\n  return copy()");
    for source in [arrays, &selected, &copied] {
        render_channels(source, &[&[1.0, 1.0, 2.0, 2.0]]);
    }
}

#[test]
fn direct_and_forwarded_processor_calls_resolve_resources_after_arguments() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-resource-order.onda"
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
        let mut buffers = [[0.0_f32]; 2];
        for (index, samples) in buffers.iter_mut().enumerate() {
            let buffer = instance.buffer_index(&format!("clips[{index}]")).unwrap();
            bind_buffer(
                &mut instance,
                buffer,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = [0.0; 8];
        process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
        assert_eq!(output, [11.0; 8], "{opt_level:?}");
    }
}

#[test]
fn joined_processor_views_keep_branch_local_owned_storage_and_callbacks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-local-owners.onda"
    );
    for source in [
        source.to_owned(),
        source
            .replace("b: Bank = right", "a: Bank = right")
            .replace("b.voices", "a.voices")
            .replace("owner = b", "owner = a"),
    ] {
        render_channels(&source, &[&[1242.0, 14847.0, 1242.0, 14847.0], &[0.0; 4]]);
    }
}

#[test]
fn joined_distinct_local_owners_preserve_selected_state_and_callbacks() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-distinct-local-owners.onda"
    );
    let arrays = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-distinct-local-array-owners.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("step(selected)", "selected()"),
        arrays.to_owned(),
    ] {
        render_channels(&source, &[&[42.0, 147.0, 42.0, 147.0], &[0.0; 4]]);
    }
}

#[test]
fn top_level_scalar_child_arguments_keep_state_callbacks_and_retained_aliases() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-arguments.onda"
        ),
        &[
            &[1.0, 3.0, 5.0, 7.0],
            &[2.0, 4.0, 6.0, 8.0],
            &[1.0, 2.0, 3.0, 4.0],
            &[3.0, 10.0, 21.0, 36.0],
        ],
    );
}

#[test]
fn top_level_scalar_children_support_direct_calls_and_cached_reads() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-arguments.onda"
    ).replace(
        "  out1 = step(bank.voice)\n  out2 = step(selected)\n  out3 = step(rack.bank.voice)\n  out4 = bank()",
        "  out1 = bank.voice()\n  out2 = bank.voice.out1\n  out3 = rack.bank.voice()\n  out4 = bank()",
    );
    render_channels(
        &source,
        &[
            &[1.0, 2.0, 3.0, 4.0],
            &[1.0, 2.0, 3.0, 4.0],
            &[1.0, 2.0, 3.0, 4.0],
            &[1.0, 3.0, 6.0, 10.0],
        ],
    );
}

#[test]
fn borrowed_scalar_children_step_and_read_caches_through_nested_owners() {
    render_channels(
        include_str!(
            "../../../../packages/onda_binaryen_web/test/fixtures/processor-scalar-projections.onda"
        ),
        &[
            &[1.0, 2.0, 3.0, 4.0],
            &[2.0, 4.0, 6.0, 8.0],
            &[1.0, 2.0, 3.0, 4.0],
            &[11.0, 22.0, 33.0, 44.0],
        ],
    );
}

#[test]
fn direct_indexed_buffer_bindings_preserve_events_caches_and_rebinding() {
    let direct = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-indexed-buffer-bindings.onda"
    );
    let routed = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-indexed-buffer-callbacks.onda"
    );
    for (source, expected, rebound_expected) in [
        (
            direct,
            [
                5.0, 16.0, 21.0, 6.0, 6.0, 22.0, 5.0, 16.0, 21.0, 6.0, 6.0, 22.0,
            ],
            [
                25.0, 16.0, 41.0, 26.0, 26.0, 42.0, 25.0, 16.0, 41.0, 26.0, 26.0, 42.0,
            ],
        ),
        (
            routed,
            [
                5.0, 16.0, 165.0, 6.0, 6.0, 177.0, 5.0, 16.0, 342.0, 6.0, 6.0, 354.0,
            ],
            [
                25.0, 16.0, 539.0, 26.0, 26.0, 591.0, 25.0, 16.0, 776.0, 26.0, 26.0, 828.0,
            ],
        ),
    ] {
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
            let mut clips = [[1.0_f32], [2.0], [3.0], [7.0]];
            for (index, samples) in clips.iter_mut().enumerate() {
                let buffer = instance.buffer_index(&format!("clips[{index}]")).unwrap();
                bind_buffer(
                    &mut instance,
                    buffer,
                    samples.as_mut_ptr().cast(),
                    1,
                    1,
                    48_000.0,
                    PrimitiveType::F32,
                )
                .unwrap();
            }
            let mut output = [0.0; 12];
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, expected, "{opt_level:?}");
            let snapshot = instance.snapshot_state_bytes().unwrap();
            let mut rebound = [12.0_f32];
            let buffer = instance.buffer_index("clips[1]").unwrap();
            bind_buffer(
                &mut instance,
                buffer,
                rebound.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
            instance.restore_state_bytes(&snapshot).unwrap();
            process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
            assert_eq!(output, rebound_expected, "{opt_level:?}");
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
            assert_eq!(
                trigger_event_by_index_checked(
                    &mut instance,
                    event,
                    &[],
                    onda_runtime::ExecutionOutput::none(),
                )
                .unwrap(),
                onda_runtime::PROCESSOR_EXECUTION_OK
            );
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
    )
    // The parity daemon accepts f32 assets; native coverage also interleaves a
    // different element type between the two mapped f32 resources.
    .replace("  interleaved: f32", "  interleaved: f64");
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
    picks: i32[1] = [pick]
    voices: Voice[2] = Voice(source = source[picks[0]])
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
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-view-nested-joins.onda"
    );
    let direct = source.replace("  step(selected)", "  selected()");
    for source in [source, &direct] {
        render_channels(
            source,
            &[
                &[2.0, 2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0],
                &[0.0, 0.0, 7.0, 7.0, 7.0, 7.0, 14.0, 14.0],
                &[0.0, 100.0, 100.0, 200.0, 200.0, 300.0, 300.0, 400.0],
            ],
        );
    }
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
