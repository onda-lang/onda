use super::*;
use onda_runtime::ExecutionOutput;

fn source(width: &str, shape: &str, body: &str) -> String {
    format!(
        r#"
import std/smoothing
outs<f64> 1
init:
  ramp = std::smoothing::Ramp<{width}>(
    time_s = {width}(4.0) / {width}(SR),
    shape = std::smoothing::shape::{shape}
  )
  frame = 0
sample:
{body}
  frame += 1
"#
    )
}

fn render(source: &str, frames: usize) -> Vec<f64> {
    render_schedule(source, frames, &[(0, frames, PROCESSOR_FULL_BLOCK)], 1)
}

fn render_schedule(
    source: &str,
    frames: usize,
    segments: &[(usize, usize, u32)],
    blocks: usize,
) -> Vec<f64> {
    let (mut instance, inputs, channels) = compile_instance_with_options(
        source,
        frames,
        CompileOptions {
            sample_rate: 1000.0,
            block_size: frames,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    assert_eq!((inputs, channels), (0, 1));
    let mut output = vec![0.0_f64; frames];
    bind_output(&mut instance, 0, output.as_mut_ptr().cast(), frames * 8).unwrap();
    let mut rendered = Vec::new();
    for _ in 0..blocks {
        for &(start, count, flags) in segments {
            process_checked_segment(&mut instance, start, count, flags, ExecutionOutput::none())
                .unwrap();
        }
        rendered.extend_from_slice(&output);
    }
    rendered
}

#[test]
fn ramps_reach_each_target_on_the_duration_sample_and_remain_there() {
    for width in ["f32", "f64"] {
        for (shape, rise) in [
            ("LINEAR", [0.25, 0.5, 0.75, 1.0]),
            ("S_CURVE", [0.15625, 0.5, 0.84375, 1.0]),
        ] {
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        "  target: {width} = 1.0\n  if frame >= 6:\n    target = 0.75\n  out1 = f64(ramp(target))"
                    ),
                ),
                12,
            );
            assert_eq!(&output[..4], &rise);
            assert_eq!(&output[4..6], &[1.0, 1.0]);
            let fall = rise.map(|weight| 1.0 - 0.25 * weight);
            assert_eq!(&output[6..10], &fall);
            assert_eq!(&output[10..], &[0.75, 0.75]);
        }
    }
}

#[test]
fn retargeting_starts_from_the_current_output_with_a_fresh_duration() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("LINEAR", [0.25, 0.5, 0.75, 1.0]),
            ("S_CURVE", [0.15625, 0.5, 0.84375, 1.0]),
        ] {
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        "  target: {width} = 1.0\n  if frame >= 2:\n    target = 0.0\n  out1 = f64(ramp(target))"
                    ),
                ),
                8,
            );
            assert_eq!(&output[..2], &weights[..2]);
            assert_eq!(&output[2..6], &weights.map(|weight| 0.5 * (1.0 - weight)));
            assert_eq!(&output[6..], &[0.0, 0.0]);
        }
    }
}

#[test]
fn durations_round_up_to_samples_and_short_or_invalid_durations_update_directly() {
    for width in ["f32", "f64"] {
        for shape in ["LINEAR", "S_CURVE"] {
            for seconds in ["-1.0", "0.0", "0.00025", "0.001", "0.0 / 0.0", "1.0 / 0.0"] {
                let output = render(
                    &source(
                        width,
                        shape,
                        &format!(
                            "  ramp.time_s = {width}({seconds})\n  out1 = f64(ramp({width}(1.0)))"
                        ),
                    ),
                    3,
                );
                assert_eq!(output, [1.0, 1.0, 1.0], "{width}, {shape}, {seconds}");
            }
            let output = render(
                &source(
                    width,
                    shape,
                    &format!("  ramp.time_s = {width}(0.00425)\n  out1 = f64(ramp({width}(1.0)))"),
                ),
                7,
            );
            let expected = if shape == "LINEAR" {
                [0.2, 0.4, 0.6, 0.8, 1.0]
            } else {
                [0.104, 0.352, 0.648, 0.896, 1.0]
            };
            for (&actual, expected) in output.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "{width}, {shape}: {output:?}"
                );
            }
            assert_eq!(&output[4..], &[1.0, 1.0, 1.0]);
        }
    }
}

#[test]
fn duration_and_shape_changes_apply_to_the_next_target() {
    for width in ["f32", "f64"] {
        let output = render(
            &source(
                width,
                "LINEAR",
                &format!(
                    r#"  if frame == 1:
    ramp.time_s = {width}(8.0) / {width}(SR)
    ramp.shape = std::smoothing::shape::S_CURVE
  target: {width} = 1.0
  if frame >= 5:
    target = 0.0
  out1 = f64(ramp(target))"#
                ),
            ),
            15,
        );
        assert_eq!(&output[..5], &[0.25, 0.5, 0.75, 1.0, 1.0]);
        assert_eq!(
            &output[5..13],
            &[0.95703125, 0.84375, 0.68359375, 0.5, 0.31640625, 0.15625, 0.04296875, 0.0]
        );
        assert_eq!(&output[13..], &[0.0, 0.0]);
    }
}

#[test]
fn reset_sets_the_current_value_immediately_and_cancels_the_transition() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("LINEAR", [0.25, 0.5, 0.75, 1.0]),
            ("S_CURVE", [0.15625, 0.5, 0.84375, 1.0]),
        ] {
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        r#"  target: {width} = 1.0
  if frame == 2:
    ramp.reset({width}(0.75))
    target = 0.75
  if frame >= 7:
    if frame == 7:
      ramp.reset()
    target = 0.0
  out1 = f64(ramp(target))"#
                    ),
                ),
                9,
            );
            assert_eq!(&output[..2], &weights[..2]);
            assert_eq!(output[2], 0.75);
            assert_eq!(&output[3..7], &weights.map(|weight| 0.75 + 0.25 * weight));
            assert_eq!(&output[7..], &[0.0, 0.0]);
        }
    }
}

#[test]
fn finite_extremes_stay_finite_and_tiny_targets_finish_exactly() {
    for (width, maximum) in [("f32", f64::from(f32::MAX)), ("f64", f64::MAX)] {
        for shape in ["LINEAR", "S_CURVE"] {
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        r#"  target: {width} = {maximum:.1}
  if frame == 0:
    ramp.reset(target)
  if frame >= 1:
    target = -target
  out1 = f64(ramp(target) / {width}({maximum:.1}))"#
                    ),
                ),
                6,
            );
            let expected = if shape == "LINEAR" {
                [1.0, 0.5, 0.0, -0.5, -1.0, -1.0]
            } else {
                [1.0, 0.6875, 0.0, -0.6875, -1.0, -1.0]
            };
            for (&actual, expected) in output.iter().zip(expected) {
                assert!(
                    actual.is_finite() && (actual - expected).abs() < 1e-7,
                    "{width}, {shape}: {output:?}"
                );
            }
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        r#"  if frame == 0:
    ramp.reset({width}(1.0))
  out1 = f64(ramp({width}(0.00000000000000000001)))"#
                    ),
                ),
                6,
            );
            let target = if width == "f32" {
                f64::from(1e-20_f32)
            } else {
                1e-20
            };
            assert_eq!(&output[3..], &[target, target, target]);
        }
    }
}

#[test]
fn non_finite_targets_update_directly_and_finite_targets_recover() {
    for width in ["f32", "f64"] {
        for shape in ["LINEAR", "S_CURVE"] {
            let output = render(
                &source(
                    width,
                    shape,
                    &format!(
                        r#"  target: {width} = 1.0
  if frame == 1:
    target = {width}(1.0) / {width}(0.0)
  elif frame == 2:
    target = {width}(0.0) / {width}(0.0)
  elif frame >= 3:
    target = 0.5
  out1 = f64(ramp(target))"#
                    ),
                ),
                6,
            );
            assert_eq!(output[1], f64::INFINITY);
            assert!(output[2].is_nan());
            assert_eq!(&output[3..], &[0.5, 0.5, 0.5]);
        }
    }
}

#[test]
fn block_boundaries_and_zero_frame_segments_preserve_sample_deadlines() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("LINEAR", [0.25, 0.5, 0.75, 1.0]),
            ("S_CURVE", [0.15625, 0.5, 0.84375, 1.0]),
        ] {
            let output = render_schedule(
                &source(
                    width,
                    shape,
                    &format!(
                        "  target: {width} = 1.0\n  if frame >= 6:\n    target = 0.0\n  out1 = f64(ramp(target))"
                    ),
                ),
                3,
                &[
                    (0, 0, PROCESSOR_BEGIN_BLOCK),
                    (0, 1, 0),
                    (1, 0, 0),
                    (1, 2, 0),
                    (3, 0, PROCESSOR_END_BLOCK),
                ],
                4,
            );
            assert_eq!(&output[..4], &weights);
            assert_eq!(&output[4..6], &[1.0, 1.0]);
            assert_eq!(&output[6..10], &weights.map(|weight| 1.0 - weight));
            assert_eq!(&output[10..], &[0.0, 0.0]);
        }
    }
}

#[test]
fn oversampled_ramps_keep_the_duration_in_seconds() {
    for width in ["f32", "f64"] {
        for (shape, expected) in [
            ("LINEAR", [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]),
            ("S_CURVE", [0.15625, 0.5, 0.84375, 1.0, 1.0, 1.0]),
        ] {
            let output = render(
                &format!(
                    r#"
import std/smoothing
proc Voice:
  delegate progress(value: f64)
  init:
    ramp = std::smoothing::Ramp<{width}>(time_s = 0.004, shape = std::smoothing::shape::{shape})
  sample 2:
    progress(f64(ramp({width}(1.0))))
    out1 = 0.0
outs<f64> 1
init:
  voice = Voice()
  current: f64 = 0.0
when voice.progress(value):
  current = value
sample:
  voice()
  out1 = current
"#
                ),
                6,
            );
            assert_eq!(output, expected, "{width}, {shape}");
        }
    }
}

#[test]
fn amplitude_ramps_start_at_the_reset_gain_and_smooth_host_parameter_changes() {
    let (mut instance, inputs, outputs) = compile_instance(
        r#"
import std/smoothing
params:
  amplitude = 0.5 {0.0, 1.0}
init:
  amplitude_ramp = std::smoothing::Ramp(time_s = 0.02, shape = std::smoothing::shape::S_CURVE)
  amplitude_ramp.reset(amplitude)
sample:
  out1 = in1 * amplitude_ramp(amplitude)
"#,
        64,
    );
    assert_eq!((inputs, outputs), (1, 1));
    let input = vec![1.0_f32; 64];
    let mut output = vec![0.0_f32; 64];
    process_interleaved(&mut instance, &input, &mut output, 64).unwrap();
    assert_eq!(output, vec![0.5_f32; 64]);
    set_param_by_index(&mut instance, 0, &1.0_f32.to_ne_bytes()).unwrap();
    for block in 0..15 {
        process_interleaved(&mut instance, &input, &mut output, 64).unwrap();
        assert!(output.iter().all(|&value| (0.5..=1.0).contains(&value)));
        if block < 14 {
            assert!(output[63] < 1.0);
        } else {
            assert_eq!(output[63], 1.0);
        }
    }
    process_interleaved(&mut instance, &input, &mut output, 64).unwrap();
    assert_eq!(output, vec![1.0_f32; 64]);
}
