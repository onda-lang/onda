use super::*;

fn render_at_1khz(source: &str, frames: usize) -> Vec<f64> {
    render_f64(source, 1000.0, frames).remove(0)
}

fn render_at_host_rate(source: &str, frames: usize) -> Vec<Vec<f64>> {
    render_f64(source, 48_000.0, frames)
}

fn exponential_progress(phase: f64, curve: f64) -> f64 {
    (curve * phase).exp_m1() / curve.exp_m1()
}

#[test]
fn a_positive_attack_uses_at_least_one_sample() {
    let output = render_f64(
        r#"
import std/env
outs<f64> 1
init:
  env = std::env::AR<f32>(attack_s = 0.000000000000000000000000000000000000000000001, release_s = 2.0)
  env.start()
sample:
  out1 = f64(env())
"#,
        0.5,
        2,
    );
    assert_eq!(output[0], [1.0, 0.0]);
}

#[test]
fn decay_duration_is_independent_of_level_and_shape() {
    for width in ["f32", "f64"] {
        for shape in ["Linear", "SCurve", "Exponential"] {
            for level in [0.0, 0.25, 1.0, 2.0] {
                let source = format!(
                    r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.00425, shape = std::smoothing::RampShape::{shape})
  count = 0
  frame = 0
when env.finished():
  count += 1
sample:
  if frame == 0 || frame == 8:
    env.start({level})
  if frame == 7:
    env.decay_s = 0.002
    env.shape = std::smoothing::RampShape::Linear
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
                );
                let output = render_f64(&source, 1000.0, 12);

                for (i, &actual) in output[0][..5].iter().enumerate() {
                    let p = (i + 1) as f64 / 5.0;
                    let remaining = match shape {
                        "Linear" => 1.0 - p,
                        "SCurve" => 1.0 - p * p * (3.0 - 2.0 * p),
                        _ => 1.0 - exponential_progress(p, -4.0),
                    };
                    assert_close_f64(actual, level * remaining);
                }
                assert_eq!(&output[0][4..8], &[0.0; 4]);
                assert_close_f64(output[0][8], level * 0.5);
                assert_eq!(&output[0][9..], &[0.0; 3]);
                assert_eq!(output[1], [0., 0., 0., 0., 1., 1., 1., 1., 1., 2., 2., 2.]);
            }
        }
    }
}

#[test]
fn gated_envelopes_capture_duration_on_stage_entry() {
    for width in ["f32", "f64"] {
        for processor in ["ASR", "ADSR"] {
            let source = format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}<{width}>(attack_s = 0.004, sustain = 1.0, release_s = 0.008)
  count = 0
  frame = 0
when env.finished():
  count += 1
sample:
  if frame == 0 || frame == 2:
    env.start()
  if frame == 3:
    env.attack_s = 0.001
    env.release_s = 0.004
  if frame == 4:
    env.release()
  if frame == 5:
    env.release_s = 0.001
    env.release_shape = std::smoothing::RampShape::Exponential
    env.sustain = 0.0
  if frame == 6:
    env.release()
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
            );
            let output = render_f64(&source, 1000.0, 10);

            assert_eq!(
                output[0],
                [0.25, 0.5, 0.625, 0.75, 0.5625, 0.375, 0.1875, 0., 0., 0.]
            );
            assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 0., 1., 1., 1.]);
        }
    }
}

#[test]
fn gate_writes_capture_the_settings_in_effect_at_the_edge() {
    for processor in ["ASR", "ADSR"] {
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 3
init:
  first = std::env::{processor}<f64>(attack_s = 0.008, sustain = 1.0, release_s = 0.008)
  last = std::env::{processor}<f64>(attack_s = 0.008, sustain = 1.0, release_s = 0.008)
  initial = std::env::{processor}<f64>(gate = 0.5, attack_s = 0.004, sustain = 1.0)
  frame = 0
sample:
  if frame == 0:
    first.gate = 1.0
    first.attack_s = 0.004
    last.attack_s = 0.004
    last.gate = 1.0
  if frame == 4:
    first.gate = 0.0
    first.release_s = 0.004
    last.release_s = 0.004
    last.gate = 0.0
  out1 = first()
  out2 = last()
  out3 = initial()
  frame += 1
"#
            ),
            1000.0,
            8,
        );

        assert_eq!(
            output[0],
            [0.125, 0.25, 0.375, 0.5, 0.4375, 0.375, 0.3125, 0.25]
        );
        assert_eq!(output[1], [0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25, 0.0]);
        assert_eq!(output[2], [0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0]);
    }
}

#[test]
fn held_envelopes_track_sustain_and_preserve_gate_edges_and_release_levels() {
    for width in ["f32", "f64"] {
        for processor in ["ASR", "ADSR"] {
            let decay = if processor == "ADSR" {
                ", decay_s = 0.0"
            } else {
                ""
            };
            let source = format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}<{width}>(attack_s = 0.0, release_s = 0.002{decay})
  levels: {width}[10] = [0.25, 0.5, 0.75, 0.75, 0.125, 0.9, 0.9, 0.9, 0.4, 0.4]
  count = 0
  frame = 0
when env.finished():
  count += 1
sample:
  gate: {width} = 0.0
  if frame == 0 || frame == 1 || frame == 3 || frame == 4 || frame == 8:
    gate = 1.0
  if frame == 9:
    env.reset()
  out1 = f64(env(gate = gate, sustain = levels[frame]))
  out2 = f64(count)
  frame += 1
"#
            );
            let output = render_f64(&source, 1000.0, 10);

            for (&actual, expected) in output[0]
                .iter()
                .zip([0.25, 0.5, 0.25, 0.75, 0.125, 0.0625, 0., 0., 0.4, 0.])
            {
                assert_close_f64(actual, expected);
            }
            assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 1., 1., 1., 1.]);
        }
    }
}

#[test]
fn unchanged_sustain_preserves_the_release_level_after_a_descending_asr_retrigger() {
    let output = render_at_1khz(
        r#"
import std/env
outs<f64> 1
init:
  env = std::env::ASR(attack_s = 0.004, release_s = 0.008)
  env.start()
  frame = 0
sample:
  if frame == 4:
    env.release()
    env.sustain = 0.25
  if frame == 6:
    env.start()
  if frame == 7:
    env.sustain = 0.25
    env.release()
  out1 = f64(env())
  frame += 1
"#,
        8,
    );
    assert_eq!(&output[6..], &[0.625, 0.546875]);
}

#[test]
fn ar_retrigger_takes_a_full_attack_and_zero_stages_consume_no_extra_samples() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 4
init:
  env = std::env::AR<{width}>(attack_s = 0.004, release_s = 0.002)
  zero = std::env::ADSR<{width}>(attack_s = 0.0, decay_s = 0.0, sustain = 0.5, release_s = 0.0)
  count = 0
  zero_count = 0
  frame = 0
when env.finished():
  count += 1
when zero.finished():
  zero_count += 1
sample:
  if frame == 0 || frame == 2:
    env.start()
  if frame == 0:
    zero.start()
  if frame == 1:
    env.release_s = 0.004
  if frame == 4:
    zero.release()
  if frame == 6:
    env.release_s = 0.0
  out1 = f64(env())
  out2 = f64(count)
  out3 = f64(zero())
  out4 = f64(zero_count)
  frame += 1
"#
            ),
            1000.0,
            12,
        );
        assert_eq!(
            output[0],
            [0.25, 0.5, 0.625, 0.75, 0.875, 1., 0.75, 0.5, 0.25, 0., 0., 0.]
        );
        assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 0., 0., 0., 1., 1., 1.]);
        assert_eq!(
            output[2],
            [0.5, 0.5, 0.5, 0.5, 0., 0., 0., 0., 0., 0., 0., 0.]
        );
        assert_eq!(output[3], [0., 0., 0., 0., 1., 1., 1., 1., 1., 1., 1., 1.]);
    }
}

#[test]
fn zero_level_envelopes_still_finish_on_time_and_reset_cancels_silently() {
    for processor in ["DecayEnv", "AR", "ASR", "ADSR"] {
        let parameters = match processor {
            "DecayEnv" => "decay_s = 0.004",
            "AR" => "attack_s = 0.002, release_s = 0.002",
            "ASR" => "attack_s = 0.0, sustain = 0.0, release_s = 0.004",
            _ => "attack_s = 0.0, decay_s = 0.0, sustain = 0.0, release_s = 0.004",
        };
        let start = if processor == "DecayEnv" {
            "env.start(0.0)"
        } else {
            "env.start()"
        };
        let release = if matches!(processor, "ASR" | "ADSR") {
            "env.release()"
        } else {
            ""
        };
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 1
init:
  env = std::env::{processor}({parameters})
  count = 0
  frame = 0
when env.finished():
  count += 1
sample:
  if frame == 0 || frame == 6:
    {start}
    {release}
  if frame == 2:
    env.reset()
  value = env()
  out1 = f64(count)
  frame += 1
"#
            ),
            1000.0,
            12,
        );
        assert_eq!(
            output[0],
            [0., 0., 0., 0., 0., 0., 0., 0., 0., 1., 1., 1.],
            "{processor}"
        );
    }
}

#[test]
fn completion_callbacks_can_retrigger_without_replacing_the_terminal_sample() {
    for processor in ["DecayEnv", "AR", "ASR", "ADSR"] {
        let parameters = match processor {
            "DecayEnv" => "decay_s = 0.004",
            "AR" => "attack_s = 0.0, release_s = 0.004",
            "ASR" => "attack_s = 0.0, release_s = 0.004",
            _ => "attack_s = 0.0, decay_s = 0.0, sustain = 1.0, release_s = 0.004",
        };
        let release = if matches!(processor, "ASR" | "ADSR") {
            "env.release()"
        } else {
            ""
        };
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}({parameters})
  count = 0
  frame = 0
when env.finished():
  count += 1
  env.start()
  {release}
sample:
  if frame == 0:
    env.start()
    {release}
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
            ),
            1000.0,
            12,
        );
        for frame in [3, 7, 11] {
            assert_eq!(output[0][frame], 0.0, "{processor}");
        }
        assert_eq!(
            output[1],
            [0., 0., 0., 1., 1., 1., 1., 2., 2., 2., 2., 3.],
            "{processor}"
        );
    }
}

#[test]
fn envelope_events_capture_the_effective_oversampled_rate() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/env
proc Voice<T>:
  outs<T> 1
  delegate completed(samples: i32)
  init:
    env = std::env::DecayEnv<T>(decay_s = 0.004)
    ticks = 0
  event start():
    env.start()
  when env.finished():
    completed(ticks)
  sample 4:
    ticks += 1
    out1 = env()
outs<f64> 1
init:
  voice = Voice<{width}>()
  completed_at = 0
  frame = 0
when voice.completed(samples):
  completed_at = samples
sample:
  if frame == 0:
    voice.start()
  value = voice()
  out1 = f64(completed_at)
  frame += 1
"#
            ),
            1000.0,
            8,
        );
        assert_eq!(output[0], [0., 0., 0., 16., 16., 16., 16., 16.]);
    }
}

#[test]
fn envelope_shapes_and_curvature_are_captured_for_each_stage() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  decay = std::env::DecayEnv<{width}>(decay_s = 4.0 / SR, curve = -4.0)
  gated = std::env::ADSR<{width}>(attack_s = 4.0 / SR, decay_s = 4.0 / SR,
    sustain = 0.5, release_s = 4.0 / SR,
    attack_shape = std::smoothing::RampShape::Exponential, attack_curve = 4.0)
  frame = 0
sample:
  if frame == 0 || frame == 4:
    decay.start()
  if frame == 0:
    gated.start()
  if frame == 1:
    decay.curve = 4.0
    decay.decay_s = 2.0 / SR
    gated.attack_shape = std::smoothing::RampShape::Linear
    gated.decay_shape = std::smoothing::RampShape::SCurve
  if frame == 5:
    gated.decay_shape = std::smoothing::RampShape::Linear
  if frame == 8:
    gated.release()
  if frame == 9:
    gated.release_curve = -8.0
    gated.release_shape = std::smoothing::RampShape::Exponential
  if frame == 12:
    gated.start()
  out1 = f64(decay())
  out2 = f64(gated())
  frame += 1
"#
            ),
            1000.0,
            16,
        );
        for i in 0..4 {
            let phase = (i + 1) as f64 / 4.0;
            let s_curve = phase * phase * (3.0 - 2.0 * phase);
            assert_close_f64(output[0][i], 1.0 - exponential_progress(phase, -4.0));
            assert_close_f64(output[1][i], exponential_progress(phase, 4.0));
            assert_close_f64(output[1][i + 4], 1.0 - 0.5 * s_curve);
            assert_close_f64(output[1][i + 8], 0.5 * (1.0 - phase));
            assert_close_f64(output[1][i + 12], phase);
        }
        assert_close_f64(output[0][4], 1.0 - exponential_progress(0.5, 4.0));
        assert_eq!(&output[0][5..], &[0.0; 11]);
    }
}

#[test]
fn impulse_decay_uses_t60_at_each_rate_and_adds_overlapping_tails() {
    for width in ["f32", "f64"] {
        for rate in [1000.0, 48000.0] {
            for factor in [1, 4] {
                let period = (rate * 0.01) as usize;
                let output = render_f64(
                    &format!(
                        r#"
import std/env
proc Voice:
  delegate observed(value: f64)
  init:
    decay = std::env::Decay<{width}>(decay_60db_s = 0.01)
    frame = 0
  sample {factor}:
    impulse: {width} = 0.0
    if frame == 0:
      impulse = 1.0
    if frame == i32(SR * 0.01):
      impulse = 2.0
    observed(f64(decay(impulse)))
    out1 = 0.0
    frame += 1
outs<f64> 1
init:
  voice = Voice()
  current: f64 = 0.0
when voice.observed(value):
  current = value
sample:
  voice()
  out1 = current
"#
                    ),
                    rate,
                    period * 3,
                );
                // Each feedback pass multiplies by a pole rounded to the processor width.
                let relative_tolerance = if width == "f32" { 1e-4 } else { 1e-12 };
                for (i, &actual) in output[0].iter().enumerate() {
                    let tick = (i + 1) * factor - 1;
                    let elapsed = tick as f64 / (period * factor) as f64;
                    let expected = 0.001_f64.powf(elapsed)
                        + if i >= period {
                            2.0 * 0.001_f64.powf(elapsed - 1.0)
                        } else {
                            0.0
                        };
                    assert!(
                        (actual - expected).abs() <= expected * relative_tolerance,
                        "{width}, {rate}, {factor}x, frame {i}: {actual} != {expected}"
                    );
                }
                assert!(
                    output[0][period * 2] > 0.0,
                    "T60 is not a completion deadline"
                );
            }
        }
    }
}

#[test]
fn impulse_decay_time_changes_preserve_state_and_reset_clears_it() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/env
outs<f64> 1
init:
  decay = std::env::Decay<{width}>(decay_60db_s = 0.004)
  frame = 0
sample:
  impulse: {width} = 0.0
  if frame == 0 || frame >= 7:
    impulse = 1.0
  if frame == 2:
    decay.decay_60db_s = 0.008
  if frame == 5:
    decay.reset()
  if frame == 8:
    decay.decay_60db_s = 0.0
  if frame == 9:
    decay.decay_60db_s = -1.0
  if frame == 10:
    decay.decay_60db_s = {width}(1.0) / {width}(0.0)
  if frame == 11:
    decay.decay_60db_s = {width}(0.0) / {width}(0.0)
  out1 = f64(decay(impulse))
  frame += 1
"#
        );
        let output = render_f64(&source, 1000.0, 12);

        for (i, &actual) in output[0][..5].iter().enumerate() {
            let elapsed = if i == 0 {
                0.0
            } else {
                0.25 + (i - 1) as f64 / 8.0
            };
            assert_close_f64(actual, 0.001_f64.powf(elapsed));
        }
        assert_eq!(&output[0][5..7], &[0., 0.]);
        assert_eq!(&output[0][7..], &[1.; 5]);
    }
}

#[test]
fn disabling_impulse_decay_recovers_from_an_overflowed_tail() {
    let output = render_at_1khz(
        r#"
import std/env
const Large: f64 = pow(f64(2.0), 1023.0) * 1.8
outs<f64> 1
init:
  decay = std::env::Decay<f64>(decay_60db_s = 2.0)
  frame = 0
sample:
  if frame == 2:
    decay.decay_60db_s = 0.0
  if frame < 2:
    out1 = decay(Large)
  else:
    out1 = decay(0.5)
  frame += 1
"#,
        5,
    );
    assert!(output[0].is_finite());
    assert_eq!(output[1], f64::INFINITY);
    assert_eq!(&output[2..], &[0.5; 3]);
}

#[test]
fn sustain_is_captured_during_a_move_and_editable_while_held() {
    for width in ["f32", "f64"] {
        for processor in ["ASR", "ADSR"] {
            let (extra, level, expected) = if processor == "ASR" {
                (
                    "attack_s = 0.004, sustain = 1.0,",
                    0.25,
                    [0.25, 0.5, 0.75, 1.0, 0.25, 0.5, 0.25, 0.0, 0.0],
                )
            } else {
                (
                    "attack_s = 0.0, decay_s = 0.004, sustain = 0.0,",
                    0.875,
                    [0.75, 0.5, 0.25, 0.0, 0.875, 0.5, 0.25, 0.0, 0.0],
                )
            };
            let source = format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}<{width}>({extra} release_s = 0.002)
  frame = 0
  count = 0
  env.start()
when env.finished():
  count += 1
sample:
  if frame == 1:
    env.sustain = {level}
  if frame == 5:
    env.sustain = 0.5
  if frame == 6:
    env.release()
  if frame == 7:
    env.sustain = 1.0
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
            );
            let output = render_f64(&source, 1000.0, 9);

            for (actual, expected) in output[0].iter().zip(expected) {
                assert_close_f64(*actual, expected);
            }
            assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 0., 1., 1.]);
        }
    }
}

#[test]
fn a_pending_sustain_edit_can_be_replaced_or_released_before_the_first_held_sample() {
    for envelope in [
        "ASR<f64>(attack_s = 0.002, sustain = 0.5, release_s = 0.002)",
        "ADSR<f64>(attack_s = 0.0, decay_s = 0.002, sustain = 0.5, release_s = 0.002)",
    ] {
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  released = std::env::{envelope}
  held = std::env::{envelope}
  released.start()
  held.start()
  frame = 0
sample:
  if frame == 1:
    released.sustain = 0.125
    held.sustain = 0.125
  if frame == 2:
    released.release()
    held.sustain = 0.875
  out1 = released()
  out2 = held()
  frame += 1
"#
            ),
            1000.0,
            4,
        );
        assert_eq!(&output[0][1..], &[0.5, 0.25, 0.0]);
        assert_eq!(&output[1][1..], &[0.5, 0.875, 0.875]);
    }
}

#[test]
fn coincident_adsr_events_release_from_the_settled_level() {
    let output = render_at_1khz(
        r#"
import std/env
outs<f64> 1
init:
  env = std::env::ADSR<f64>(attack_s = 0.0, decay_s = 0.0, sustain = 0.25, release_s = 0.002)
  env.start()
  env.release()
sample:
  out1 = env()
"#,
        2,
    );
    assert_eq!(output, [0.125, 0.0]);
}

#[test]
fn coincident_gated_retriggers_settle_and_report_zero_length_releases() {
    for processor in ["ASR", "ADSR"] {
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}<f64>(attack_s = 0.004, sustain = 1.0, release_s = 0.0)
  frame = 0
  completions = 0
  env.start()
when env.finished():
  completions += 1
sample:
  if frame == 2:
    env.release()
    env.start()
    env.release()
    env.start()
  out1 = env()
  out2 = f64(completions)
  frame += 1
"#,
            ),
            1_000.0,
            4,
        );
        assert_eq!(output[0], [0.25, 0.5, 0.25, 0.5], "{processor}");
        assert_eq!(output[1], [0.0, 0.0, 2.0, 2.0], "{processor}");
    }
}

#[test]
fn zero_duration_decay_completes_once_for_every_start_level() {
    for width in ["f32", "f64"] {
        let output = render_at_host_rate(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.0, trigger = 1.0)
  levels: {width}[4] = [1.0, 0.5, 0.25, 0.0]
  completions = 0
  frame = 0
when env.finished():
  completions += 1
sample:
  if frame >= 1 && frame <= 4:
    env.start(levels[frame - 1])
  out1 = f64(env())
  out2 = f64(completions)
  frame += 1
"#
            ),
            6,
        );
        assert_eq!(output[0], [0.0; 6]);
        assert_eq!(output[1], [1.0, 2.0, 3.0, 4.0, 5.0, 5.0]);
    }
}

#[test]
fn adsr_release_duration_is_independent_of_level_and_sustain_automation() {
    const RELEASE: usize = 4800;
    const NOTE_OFF: usize = 480;
    for width in ["f32", "f64"] {
        for (sustain, attack) in [(1.0, 0.0), (0.5, 0.0), (0.25, 0.0), (0.5, 2000.0)] {
            for note_off in ["env.release()", "env.gate = 0.0"] {
                let output = render_at_host_rate(
                    &format!(
                        r#"
import std/env
outs<f64> 2
init:
  env = std::env::ADSR<{width}>(attack_s = {attack} / SR, decay_s = 0.0,
    sustain = {sustain}, release_s = 0.1)
  completions = 0
  frame = 0
when env.finished():
  completions += 1
sample:
  if frame == 0:
    env.start()
  if frame == {NOTE_OFF}:
    {note_off}
  if frame == {NOTE_OFF} + 2:
    env.sustain = 1.0
  if frame == {NOTE_OFF} + 120:
    env.release()
  out1 = f64(env())
  out2 = f64(completions)
  frame += 1
"#
                    ),
                    NOTE_OFF + RELEASE + 16,
                );
                let completion = output[1].iter().position(|&x| x == 1.0).unwrap();
                let elapsed = completion + 1 - NOTE_OFF;
                assert_eq!(elapsed, RELEASE, "{width}: release duration");
                let level = output[0][NOTE_OFF - 1];
                assert_close_f64(output[0][NOTE_OFF + RELEASE / 2 - 1], level * 0.5);
                assert_eq!(*output[1].last().unwrap(), 1.0);
                assert_eq!(*output[0].last().unwrap(), 0.0);
            }
        }
    }
}

#[test]
fn asr_host_events_capture_release_settings_and_the_note_off_level() {
    fn event(instance: &mut onda_runtime::Instance, name: &str, value: Option<f32>) {
        let index = instance.event_index(name).expect("event exists");
        let payload = value.map(f32::to_le_bytes);
        trigger_event_by_index_checked(
            instance,
            index,
            payload.as_ref().map_or(&[], |bytes| bytes.as_slice()),
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("event should succeed");
    }
    fn frame(instance: &mut onda_runtime::Instance, level: f32, completions: f32) {
        let mut output = [0.0_f32; 2];
        process_interleaved(instance, &[], &mut output, 1).expect("render envelope");
        assert_near(output[0], level, 1e-6);
        assert_eq!(output[1], completions);
    }
    let source = r#"
import std/env
init:
  env = std::env::ASR<$T>(attack_s = 0.0, sustain = 0.5, release_s = 8.0 / SR)
  completions = 0
event start():
  env.start()
event release():
  env.release()
event gate(value: f32):
  env.gate = $T(value)
event sustain(value: f32):
  env.sustain = $T(value)
event duration(samples: f32):
  env.release_s = $T(samples) / SR
event attack(samples: f32):
  env.attack_s = $T(samples) / SR
when env.finished():
  completions += 1
sample:
  out1 = f32(env())
  out2 = f32(completions)
"#;
    for scalar in ["f32", "f64"] {
        // Change sustain at note-off and during release, using both event and
        // parameter gates. Increasing sustain must not reshape the tail either.
        for (change_after, sustain, parameter_gate) in
            [(0, 0.0, false), (2, 0.0, true), (2, 1.0, false)]
        {
            let (mut instance, _, _) = compile_instance(&source.replace("$T", scalar), 1);
            event(&mut instance, "start", None);
            frame(&mut instance, 0.5, 0.0);
            if parameter_gate {
                event(&mut instance, "gate", Some(0.0));
            } else {
                event(&mut instance, "release", None);
            }
            for sample in 0..8 {
                if sample == change_after {
                    event(&mut instance, "sustain", Some(sustain));
                }
                frame(
                    &mut instance,
                    0.5 - (sample + 1) as f32 / 16.0,
                    if sample == 7 { 1.0 } else { 0.0 },
                );
            }
            frame(&mut instance, 0.0, 1.0);
        }
        let (mut instance, _, _) = compile_instance(&source.replace("$T", scalar), 1);
        event(&mut instance, "start", None);
        frame(&mut instance, 0.5, 0.0);
        event(&mut instance, "release", None);
        frame(&mut instance, 0.4375, 0.0);
        event(&mut instance, "duration", Some(4.0));
        frame(&mut instance, 0.375, 0.0);
        event(&mut instance, "duration", Some(0.0));
        for sample in 3..=8 {
            frame(
                &mut instance,
                0.5 - sample as f32 / 16.0,
                if sample == 8 { 1.0 } else { 0.0 },
            );
        }
        frame(&mut instance, 0.0, 1.0);
        event(&mut instance, "start", None);
        frame(&mut instance, 0.5, 1.0);
        event(&mut instance, "release", None);
        frame(&mut instance, 0.0, 2.0);

        // Releasing during attack takes the requested time from the current
        // level, rather than using the still-unreached sustain target.
        event(&mut instance, "attack", Some(8.0));
        event(&mut instance, "duration", Some(8.0));
        event(&mut instance, "start", None);
        frame(&mut instance, 0.0625, 2.0);
        event(&mut instance, "release", None);
        event(&mut instance, "sustain", Some(0.0));
        for sample in 0..8 {
            frame(
                &mut instance,
                0.0625 - (sample + 1) as f32 / 128.0,
                if sample == 7 { 3.0 } else { 2.0 },
            );
        }
        frame(&mut instance, 0.0, 3.0);
    }
}

#[test]
fn exponential_envelopes_approach_linear_at_tiny_curvature() {
    for curve in [-f64::MIN_POSITIVE, f64::MIN_POSITIVE] {
        let output = render_at_1khz(
            &format!(
                r#"
import std/env
outs<f64> 1
init:
  envelope = std::env::DecayEnv<f64>(decay_s = 0.004, curve = {curve:.324})
  envelope.start()
sample:
  out1 = envelope()
"#
            ),
            4,
        );
        assert_eq!(output, [0.75, 0.5, 0.25, 0.0]);
    }
}
