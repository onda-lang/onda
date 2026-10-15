use super::*;

#[test]
fn oversampling_scratch_preserves_split_blocks_and_unwritten_outputs() {
    let source = r#"
init:
  counter: i32 = 0
sample 4:
  counter += 1
  if counter % 3 == 0:
    out1 = f32(counter) * 0.001
"#;
    let frames = 8;
    let (mut whole, _, _) = compile_instance(source, frames);
    let (mut split, _, _) = compile_instance(source, frames);
    let mut expected = vec![0.0_f32; frames];
    let mut actual = vec![0.0_f32; frames];
    bind_output(&mut whole, 0, expected.as_mut_ptr().cast(), frames * 4).unwrap();
    bind_output(&mut split, 0, actual.as_mut_ptr().cast(), frames * 4).unwrap();
    for _ in 0..3 {
        process_checked_segment(
            &mut whole,
            0,
            frames,
            PROCESSOR_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .unwrap();
        for (start, count, flags) in [
            (0, 0, PROCESSOR_BEGIN_BLOCK),
            (0, 1, 0),
            (1, 3, 0),
            (4, 0, 0),
            (4, 4, PROCESSOR_END_BLOCK),
        ] {
            process_checked_segment(
                &mut split,
                start,
                count,
                flags,
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap();
        }
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|value| *value != 0.0));
        assert!(actual.iter().all(|value| value.is_finite()));
    }
}
