use super::*;
use onda_runtime::ExecutionOutput;

fn source(width: &str, shape: &str, body: &str) -> String {
    format!(
        r#"
import std/smoothing
outs<f64> 1
init:
  ramp = std::smoothing::Ramp<{width}>(
    duration_s = {width}(4.0) / {width}(SR),
    shape = std::smoothing::RampShape::{shape}
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
fn lag_ud_and_slew_reset_the_value_without_changing_the_response_rates() {
    for width in ["f32", "f64"] {
        for (processor, params, rise, fall) in [
            (
                "LagUD",
                "rise_lag_s = 0.004, fall_lag_s = 0.002",
                [
                    1.0 - 0.25 * (-0.25_f64).exp(),
                    1.0 - 0.25 * (-0.5_f64).exp(),
                ],
                [-1.0 + 0.5 * (-0.5_f64).exp(), -1.0 + 0.5 * (-1.0_f64).exp()],
            ),
            (
                "Slew",
                "rise_per_s = 125.0, fall_per_s = 250.0",
                [0.875, 1.0],
                [-0.75, -1.0],
            ),
        ] {
            let output = render(
                &format!(
                    r#"
import std/smoothing
outs<f64> 1
init:
  smooth = std::smoothing::{processor}<{width}>({params})
  smooth.reset(0.75)
  frame = 0
sample:
  target: {width} = 0.75
  if frame >= 2:
    target = 1.0
  if frame == 4:
    smooth.reset(-0.5)
  if frame >= 4:
    target = -0.5
  if frame >= 6:
    target = -1.0
  if frame == 8:
    smooth.reset()
  if frame >= 8:
    target = 0.0
  out1 = f64(smooth(target))
  frame += 1
"#
                ),
                10,
            );
            let expected = [
                0.75, 0.75, rise[0], rise[1], -0.5, -0.5, fall[0], fall[1], 0.0, 0.0,
            ];
            for (actual, expected) in output.into_iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "{processor}<{width}>: {actual} != {expected}"
                );
            }
        }
    }
}

#[test]
fn ramps_reach_each_target_on_the_duration_sample_and_remain_there() {
    for width in ["f32", "f64"] {
        for (shape, rise) in [
            ("Linear", [0.25, 0.5, 0.75, 1.0]),
            ("SCurve", [0.15625, 0.5, 0.84375, 1.0]),
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
fn input_ramps_move_monotonically_between_adjacent_values() {
    for (origin, target) in [(1.5_f64, 1.5_f64.next_up()), (1.5_f64.next_up(), 1.5)] {
        let output = render(
            &format!(
                r#"
outs<f64> 1
proc SmoothedInput:
  ins:
    x: f64 = {origin} {{ramp = 17.0 / SR}}
  outs<f64> 1
  sample:
    out1 = x
init:
  smoother = SmoothedInput()
sample:
  out1 = smoother(f64({target}))
"#
            ),
            19,
        );
        assert_eq!(&output[..8], &[origin; 8]);
        assert_eq!(&output[8..], &[target; 11]);
    }
}

#[test]
fn retargeting_starts_from_the_current_output_with_a_fresh_duration() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("Linear", [0.25, 0.5, 0.75, 1.0]),
            ("SCurve", [0.15625, 0.5, 0.84375, 1.0]),
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
        for shape in ["Linear", "SCurve"] {
            for seconds in ["-1.0", "0.0", "0.00025", "0.001", "0.0 / 0.0", "1.0 / 0.0"] {
                let output = render(
                    &source(
                        width,
                        shape,
                        &format!(
                            "  ramp.duration_s = {width}({seconds})\n  out1 = f64(ramp({width}(1.0)))"
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
                    &format!(
                        "  ramp.duration_s = {width}(0.00425)\n  out1 = f64(ramp({width}(1.0)))"
                    ),
                ),
                7,
            );
            let expected = if shape == "Linear" {
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
                "Linear",
                &format!(
                    r#"  if frame == 1:
    ramp.duration_s = {width}(8.0) / {width}(SR)
    ramp.shape = std::smoothing::RampShape::SCurve
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
fn exponential_ramps_capture_curvature_on_each_new_target() {
    for width in ["f32", "f64"] {
        let output = render(
            &source(
                width,
                "Exponential",
                &format!(
                    r#"  if frame == 0 || frame == 10:
    ramp.curve = 0.0
  if frame == 1:
    ramp.curve = 4.0
  if frame == 2:
    ramp.duration_s = {width}(8.0) / {width}(SR)
  if frame == 11:
    ramp.duration_s = {width}(4.0) / {width}(SR)
  target: {width} = 1.0
  if frame >= 5:
    target = 0.0
  if frame >= 14:
    target = -1.0
  out1 = f64(ramp(target))"#
                ),
            ),
            19,
        );
        assert_eq!(&output[..5], &[0.25, 0.5, 0.75, 1.0, 1.0]);
        for (step, &actual) in output[5..12].iter().enumerate() {
            let expected = 1.0 - ((step + 1) as f64 * 0.5).exp_m1() / 4.0_f64.exp_m1();
            assert!((actual - expected).abs() < 1e-7, "{width}, {actual}");
        }
        assert_eq!(&output[12..14], &[0.0, 0.0]);
        assert_eq!(&output[14..], &[-0.25, -0.5, -0.75, -1.0, -1.0]);
    }
}

#[test]
fn reset_sets_the_current_value_immediately_and_cancels_the_transition() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("Linear", [0.25, 0.5, 0.75, 1.0]),
            ("SCurve", [0.15625, 0.5, 0.84375, 1.0]),
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
        for shape in ["Linear", "SCurve"] {
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
            let expected = if shape == "Linear" {
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
        for shape in ["Linear", "SCurve"] {
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
fn s_curve_between_adjacent_finite_extremes_stays_in_range() {
    let upper = f64::MAX;
    let lower = upper.next_down();
    for frames in [5, 7] {
        let samples = render_f64(
            &format!(
                r#"
import std/smoothing
outs<f64> 1
init:
  ramp = std::smoothing::Ramp<f64>(duration_s = {frames}.0 / SR, shape = std::smoothing::RampShape::SCurve)
  ramp.reset({lower:.1})
sample:
  out1 = ramp({upper:.1})
"#
            ),
            1024.0,
            frames,
        );
        assert!(samples[0]
            .iter()
            .all(|value| (lower..=upper).contains(value)));
        assert_eq!(samples[0][frames - 1], upper);
    }
}

#[test]
fn block_boundaries_and_zero_frame_segments_preserve_sample_deadlines() {
    for width in ["f32", "f64"] {
        for (shape, weights) in [
            ("Linear", [0.25, 0.5, 0.75, 1.0]),
            ("SCurve", [0.15625, 0.5, 0.84375, 1.0]),
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
            ("Linear", [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]),
            ("SCurve", [0.15625, 0.5, 0.84375, 1.0, 1.0, 1.0]),
        ] {
            let output = render(
                &format!(
                    r#"
import std/smoothing
proc Voice:
  delegate progress(value: f64)
  init:
    ramp = std::smoothing::Ramp<{width}>(duration_s = 0.004, shape = std::smoothing::RampShape::{shape})
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
  amplitude_ramp = std::smoothing::Ramp(duration_s = 0.02, shape = std::smoothing::RampShape::SCurve)
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

#[test]
fn smoothed_gains_start_at_requested_gain_then_smooth_changes_in_amplitude() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/gain
outs<f64> 2
init:
  linear = std::gain::Smoothed<{width}>(gain = 0.25, lag_s = 0.004)
  decibels = std::gain::SmoothedDb<{width}>(db = -6.0, lag_s = 0.004)
  frame = 0
sample:
  if frame == 2:
    linear.gain = 0.75
    decibels.db = 0.0
  out1 = f64(linear({width}(1.0)))
  out2 = f64(decibels({width}(1.0)))
  frame += 1
"#
        );
        let output = render_f64(&source, 1000.0, 10);

        for (i, (&linear, &decibels)) in output[0].iter().zip(&output[1]).enumerate() {
            let remaining = if i < 2 {
                1.0
            } else {
                (-((i - 1) as f64) / 4.0).exp()
            };
            assert_close_f64(linear, 0.75 + (0.25 - 0.75) * remaining);
            assert_close_f64(
                decibels,
                1.0 + (10.0_f64.powf(-6.0 / 20.0) - 1.0) * remaining,
            );
        }
    }
}

#[test]
fn lag_time_updates_handle_instant_and_fast_responses_after_reset() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/smoothing
outs<f64> 2
init:
  lag = std::smoothing::Lag<{width}>()
  asymmetric = std::smoothing::LagUD<{width}>()
  times: {width}[6] = [0.0, -1.0, sqrt({width}(-1.0)), {width}(1.0) / {width}(0.0), 0.000001, 0.00002]
  frame = 0
sample:
  start: {width} = 100000000000000000000.0
  target: {width} = 0.00000000000000000001
  if frame >= 6:
    start = -start
    target = -target
  lag.lag_s = times[frame % 6]
  asymmetric.rise_lag_s = times[frame % 6]
  asymmetric.fall_lag_s = times[frame % 6]
  lag.reset(start)
  asymmetric.reset(start)
  out1 = f64(lag(target))
  out2 = f64(asymmetric(target))
  frame += 1
"#
        );
        let output = render_f64(&source, 1000.0, 12);
        let adapt = |value: f64| {
            if width == "f32" {
                f64::from(value as f32)
            } else {
                value
            }
        };
        for frame in 0..12 {
            let sign = if frame >= 6 { -1.0 } else { 1.0 };
            let target = sign * adapt(1e-20);
            for channel in &output {
                if frame % 6 < 5 {
                    assert_eq!(channel[frame], target, "{width}, frame {frame}");
                } else {
                    let pole = (-1.0 / (1000.0 * adapt(0.00002))).exp();
                    let expected = adapt(target + (sign * adapt(1e20) - target) * pole);
                    assert!((channel[frame] - expected).abs() < expected.abs() * 2e-6);
                }
            }
        }
    }
}

#[test]
fn lags_blend_opposite_finite_extremes_without_overflow() {
    let maximum = f64::MAX;
    let source = format!(
        r#"
import std/smoothing
outs<f64> 2
init:
  lag = std::smoothing::Lag<f64>()
  asymmetric = std::smoothing::LagUD<f64>()
  frame = 0
sample:
  previous: f64 = {maximum:.1}
  if (frame & 1) == 1:
    previous = -previous
  target = -previous
  duration: f64 = 0.0
  if frame >= 2:
    duration = 0.002
  if frame >= 4:
    target = previous
  lag.lag_s = duration
  asymmetric.rise_lag_s = duration
  asymmetric.fall_lag_s = duration
  lag.reset(previous)
  asymmetric.reset(previous)
  out1 = lag(target)
  out2 = asymmetric(target)
  frame += 1
"#,
    );
    let pole = (-0.5_f64).exp();
    let falling = maximum * pole - maximum * (1.0 - pole);
    let output = render_f64(&source, 1000.0, 6);
    for channel in &output {
        assert_eq!(&channel[..2], &[-maximum, maximum]);
        for (actual, expected) in channel[2..]
            .iter()
            .zip([falling, -falling, maximum, -maximum])
        {
            assert!(actual.is_finite());
            assert!((actual / maximum - expected / maximum).abs() < 1e-15);
        }
    }
}

#[test]
fn long_ramps_follow_each_curve_and_finish_exactly() {
    let samples = 32_000;
    for width in ["f32", "f64"] {
        for (shape, curve, initial, target) in [
            ("Linear", 0.0, 1.0, 0.0),
            ("SCurve", 0.0, 1.0, 0.0),
            ("Exponential", -60.0, 1.0, 0.0),
            ("Exponential", 60.0, 0.0, 1.0),
        ] {
            let output = render(
                &format!(
                    r#"
import std/smoothing
outs<f64> 1
init:
  ramp = std::smoothing::Ramp<{width}>(duration_s = 32.0,
    shape = std::smoothing::RampShape::{shape}, curve = {curve})
  ramp.reset({initial})
sample:
  out1 = f64(ramp({width}({target})))
"#
                ),
                samples + 2,
            );
            // Only the exponential curve accumulates roundoff over all 32,000 updates.
            let tolerance = match (width, shape) {
                ("f32", "Exponential") => 2e-5,
                ("f32", _) => 2e-7,
                _ => 2e-9,
            };
            for (index, &actual) in output[..samples - 1].iter().enumerate() {
                let phase = (index + 1) as f64 / samples as f64;
                let remaining = (samples - index - 1) as f64 / samples as f64;
                let expected = match shape {
                    "Linear" => remaining,
                    "SCurve" => remaining * remaining * (3.0 - 2.0 * remaining),
                    _ if curve < 0.0 => {
                        (curve * phase).exp() * -(curve * remaining).exp_m1() / -curve.exp_m1()
                    }
                    _ => (curve * phase).exp_m1() / curve.exp_m1(),
                };
                assert!(
                    (actual - expected).abs() <= tolerance,
                    "{width}, {shape}, curve {curve}, sample {index}: {actual} != {expected}"
                );
            }
            assert!(output.windows(2).all(|pair| if initial > target {
                pair[0] >= pair[1]
            } else {
                pair[0] <= pair[1]
            }));
            assert_eq!(&output[samples - 1..], &[target, target, target]);
        }
    }
}

#[test]
fn ramps_preserve_initial_progress_when_the_duration_exceeds_f64_integer_precision() {
    let samples = 2.0_f64.powi(54);
    for (shape, curve) in [
        ("Linear", 0.0_f64),
        ("SCurve", 0.0),
        ("Exponential", -4.0),
        ("Exponential", 4.0),
    ] {
        let output = render_f64(
            &format!(
                r#"
import std/smoothing
outs<f64> 1
init:
  ramp = std::smoothing::Ramp<f64>(duration_s = {samples}.0 / SR,
    shape = std::smoothing::RampShape::{shape}, curve = {curve})
sample:
  out1 = ramp(f64(1.0))
"#
            ),
            1024.0,
            4,
        );
        for (i, actual) in output[0].iter().enumerate() {
            let phase = (i + 1) as f64 / samples;
            let expected = match shape {
                "SCurve" => phase * phase * (3.0 - 2.0 * phase),
                "Exponential" => (curve * phase).exp_m1() / curve.exp_m1(),
                _ => phase,
            };
            assert!(
                (actual / expected - 1.0).abs() < 1e-12,
                "{shape}, frame {i}: {actual} vs {expected}"
            );
        }
    }
}
