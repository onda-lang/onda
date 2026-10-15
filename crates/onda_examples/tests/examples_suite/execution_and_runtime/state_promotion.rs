use super::*;

#[test]
fn partial_state_promotion_preserves_segments_block_hooks_and_events() {
    // More independent scalar slots than the portable promotion budget. Both
    // the cached prefix and the remaining state must be visible between calls.
    let mut source = String::from("init:\n");
    for index in 0..68 {
        source.push_str(&format!("  counter_{index:02} = {index}\n"));
    }
    source.push_str("event reset():\n  counter_00 = 1000\n  counter_67 = 2000\n");
    source.push_str("block:\n  counter_00 += 100\n  sample:\n");
    for index in 0..68 {
        source.push_str(&format!("    counter_{index:02} += {}\n", index + 1));
    }
    let sum = (0..68)
        .map(|index| format!("counter_{index:02}"))
        .collect::<Vec<_>>()
        .join(" + ");
    source.push_str(&format!("    out1 = f32({sum})\n"));

    let frames = 8;
    let (mut instance, _, _) = compile_instance(&source, frames);
    let mut output = vec![0.0_f32; frames];
    bind_output(&mut instance, 0, output.as_mut_ptr().cast(), frames * 4).expect("bind output");
    let event = instance.event_index("reset").expect("reset event");
    let mut counters = (0..68).collect::<Vec<i32>>();
    for block in 0..2 {
        // A zero-frame BEGIN still executes the block hook and commits state.
        process_checked_segment(
            &mut instance,
            0,
            0,
            PROCESSOR_BEGIN_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("zero-frame begin");
        counters[0] += 100;
        for (start, count, flags) in [(0, 3, 0), (3, 5, PROCESSOR_END_BLOCK)] {
            if block == 0 && start == 3 {
                trigger_event_by_index_checked(
                    &mut instance,
                    event,
                    &[],
                    onda_runtime::ExecutionOutput::none(),
                )
                .expect("event between segments");
                counters[0] = 1000;
                counters[67] = 2000;
            }
            process_checked_segment(
                &mut instance,
                start,
                count,
                flags,
                onda_runtime::ExecutionOutput::none(),
            )
            .expect("process segment");
            for actual in &output[start..start + count] {
                for (index, counter) in counters.iter_mut().enumerate() {
                    *counter += index as i32 + 1;
                }
                assert_eq!(*actual, counters.iter().sum::<i32>() as f32);
            }
        }
    }
}
