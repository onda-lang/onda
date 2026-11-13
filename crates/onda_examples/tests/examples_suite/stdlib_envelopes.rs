use super::*;

fn render_at_1khz(source: &str, frames: usize) -> Vec<f64> {
    render_f64(source, 1000.0, frames).remove(0)
}

fn render_at_host_rate(source: &str, frames: usize) -> Vec<Vec<f64>> {
    render_f64(source, 48_000.0, frames)
}

fn curve_progress(phase: f64, curve: f64) -> f64 {
    if curve == 0.0 || !curve.is_finite() {
        phase
    } else {
        (curve * phase).exp_m1() / curve.exp_m1()
    }
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

                for i in 0..5 {
                    let p = (i + 1) as f64 / 5.0;
                    let remaining = match shape {
                        "Linear" => 1.0 - p,
                        "SCurve" => 1.0 - p * p * (3.0 - 2.0 * p),
                        _ => 1.0 - (-4.0 * p).exp_m1() / (-4.0_f64).exp_m1(),
                    };
                    assert_close_f64(output[0][i], level * remaining);
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
fn gated_envelopes_apply_active_attack_and_release_time_changes() {
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

            assert_eq!(output[0], [0.25, 0.5, 0.625, 1.0, 0.75, 0., 0., 0., 0., 0.]);
            assert_eq!(output[1], [0., 0., 0., 0., 0., 1., 1., 1., 1., 1.]);
        }
    }
}

#[test]
fn gate_and_time_parameter_hooks_apply_in_either_argument_order() {
    for width in ["f32", "f64"] {
        for processor in ["ASR", "ADSR"] {
            let source = format!(
                r#"
import std/env
outs<f64> 3
init:
  first = std::env::{processor}<{width}>(attack_s = 0.1, sustain = 1.0)
  last = std::env::{processor}<{width}>(attack_s = 0.1, sustain = 1.0)
  initial = std::env::{processor}<{width}>(gate = 0.5, attack_s = 0.004, sustain = 1.0)
  frame = 0
sample:
  if frame == 0:
    # Time edits after a gate edge must update the already-started attack.
    first.gate = 1.0
    first.gate = 0.0
    last.gate = 1.0
    last.gate = 0.0
    out1 = f64(first(gate = 1.0, attack_s = 0.004))
    out2 = f64(last(attack_s = 0.004, gate = 1.0))
  else:
    out1 = f64(first(gate = 0.5))
    out2 = f64(last())
  out3 = f64(initial())
  frame += 1
"#
            );
            let output = render_f64(&source, 1000.0, 4);

            for channel in output {
                assert_eq!(channel, [0.25, 0.5, 0.75, 1.0]);
            }
        }
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
            [0.25, 0.5, 0.625, 0.75, 0.875, 1., 0., 0., 0., 0., 0., 0.]
        );
        assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 1., 1., 1., 1., 1., 1.]);
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
fn exponential_curve_edits_update_envelopes_and_wait_for_new_ramp_targets() {
    for width in ["f32", "f64"] {
        for curve in [-60.0, -4.0, -1e-6, 0.0, 1e-6, 4.0, 60.0] {
            let literal = format!("{curve:.8}");
            let output = render_f64(
                &format!(
                    r#"
import std/env
outs<f64> 3
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.004, curve = {literal})
  ramp = std::smoothing::Ramp<{width}>(duration_s = 0.004, shape = std::smoothing::RampShape::Exponential, curve = {literal})
  count = 0
  frame = 0
when env.finished():
  count += 1
sample:
  if frame == 0 || frame == 6:
    env.start(0.75)
  if frame == 1:
    env.curve = 4.0
    ramp.curve = 4.0
  target: {width} = 1.0
  if frame >= 6:
    target = -1.0
  out1 = f64(env())
  out2 = f64(ramp(target))
  out3 = f64(count)
  frame += 1
"#
                ),
                1000.0,
                12,
            );
            for i in 0..12 {
                let (phase, curvature) = if i < 6 {
                    (((i + 1) as f64 / 4.0).min(1.0), curve)
                } else {
                    (((i - 5) as f64 / 4.0).min(1.0), 4.0)
                };
                let progress = curve_progress(phase, curvature);
                let envelope = if curve == 4.0 {
                    0.75 * (1.0 - progress)
                } else if i == 0 {
                    0.75 * (1.0 - curve_progress(0.25, curve))
                } else if i < 6 {
                    let level = 0.75 * (1.0 - curve_progress(0.25, curve));
                    level * (1.0 - curve_progress((i as f64 / 3.0).min(1.0), 4.0))
                } else {
                    0.75 * (1.0 - progress)
                };
                assert_close_f64(output[0][i], envelope);
                assert_close_f64(
                    output[1][i],
                    if i < 6 {
                        progress
                    } else {
                        1.0 - 2.0 * progress
                    },
                );
            }
            assert_eq!(output[2], [0., 0., 0., 1., 1., 1., 1., 1., 1., 2., 2., 2.]);
            assert_eq!(&output[0][9..], &[0.; 3]);
            assert_eq!(&output[1][9..], &[-1.; 3]);
        }
    }
}

#[test]
fn envelope_curve_hooks_update_only_the_active_stage() {
    for processor in ["AR", "ASR", "ADSR"] {
        let decay = if processor == "ADSR" {
            "decay_s = 0.004, decay_shape = std::smoothing::RampShape::Exponential, decay_curve = 4.0,"
        } else {
            ""
        };
        let sustain = if processor == "AR" {
            ""
        } else {
            "sustain = 0.5,"
        };
        let changes = if processor == "ADSR" {
            "    env.decay_curve = -8.0"
        } else {
            ""
        };
        let release = if processor == "AR" {
            ""
        } else {
            "  if frame == 8:\n    env.release()"
        };
        let output = render_f64(
            &format!(
                r#"
import std/env
outs<f64> 1
init:
  env = std::env::{processor}<f64>({decay} {sustain} attack_s = 0.004, release_s = 0.004,
    attack_shape = std::smoothing::RampShape::Exponential, attack_curve = -4.0,
    release_shape = std::smoothing::RampShape::Exponential, release_curve = -2.0)
  frame = 0
sample:
  if frame == 0:
    env.start()
  if frame == 1:
    env.attack_curve = 8.0
  if frame == 5:
{changes}
    env.attack_curve = 0.0
{release}
  if frame == 9:
    env.release_curve = 8.0
  out1 = env()
  frame += 1
"#
            ),
            1000.0,
            14,
        );
        for i in 0..14 {
            let expected = if i < 4 {
                let target = if processor == "ASR" { 0.5 } else { 1.0 };
                let first = target * curve_progress(0.25, -4.0);
                if i == 0 {
                    first
                } else {
                    first + (target - first) * curve_progress(i as f64 / 3.0, 8.0)
                }
            } else if processor == "AR" {
                1.0 - curve_progress(((i - 3) as f64 / 4.0).min(1.0), -2.0)
            } else if i < 8 {
                if processor == "ADSR" {
                    let first = 1.0 - 0.5 * curve_progress(0.25, 4.0);
                    if i == 4 {
                        first
                    } else {
                        first + (0.5 - first) * curve_progress((i - 4) as f64 / 3.0, -8.0)
                    }
                } else {
                    0.5
                }
            } else {
                let first = 0.5 * (1.0 - curve_progress(0.25, -2.0));
                if i == 8 {
                    first
                } else {
                    first * (1.0 - curve_progress(((i - 8) as f64 / 3.0).min(1.0), 8.0))
                }
            };
            assert_close_f64(output[0][i], expected);
        }
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
                        (actual - expected).abs() < 1e-8 + expected * 1e-4,
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

        for i in 0..5 {
            let elapsed = if i == 0 {
                0.0
            } else {
                0.25 + (i - 1) as f64 / 8.0
            };
            assert_close_f64(output[0][i], 0.001_f64.powf(elapsed));
        }
        assert_eq!(&output[0][5..7], &[0., 0.]);
        assert_eq!(&output[0][7..], &[1.; 5]);
    }
}

#[test]
fn unchanged_or_irrelevant_envelope_controls_leave_the_running_curve_untouched() {
    // Exercise the shared shape guard in DecayEnv and each remaining processor's
    // parameter hooks without recompiling every processor/shape combination.
    for width in ["f32", "f64"] {
        for (processor, shape, curve) in [
            ("DecayEnv", "Linear", -4.0),
            ("DecayEnv", "SCurve", -4.0),
            ("DecayEnv", "Exponential", 0.0),
            ("DecayEnv", "Exponential", -4.0),
            ("DecayEnv", "Exponential", 4.0),
            ("AR", "Exponential", -4.0),
            ("ASR", "Exponential", -4.0),
            ("ADSR", "Exponential", -4.0),
        ] {
            let (time, prefix, extra) = match processor {
                "DecayEnv" => ("decay_s", "", ""),
                "ADSR" => ("decay_s", "decay_", "attack_s = 0.0, sustain = 0.25,"),
                _ => ("attack_s", "attack_", ""),
            };
            let edits = format!(
                "  env.{time} = 0.025\n  env.{prefix}shape = std::smoothing::RampShape::{shape}\n  env.{prefix}curve = {}",
                if shape == "Exponential" { curve.to_string() } else { format!("{width}(frame)") },
            );
            let source = |edits: &str| {
                format!(
                    r#"
import std/env
outs<f64> 2
init:
  env = std::env::{processor}<{width}>({extra} {time} = 0.025,
    {prefix}shape = std::smoothing::RampShape::{shape}, {prefix}curve = {curve})
  frame = 0
  count = 0
  env.start()
when env.finished():
  count += 1
sample:
{edits}
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
                )
            };
            assert_eq!(
                render_f64(&source(&edits), 1000.0, 32),
                render_f64(&source(""), 1000.0, 32),
                "{processor}<{width}>, {shape}, curve {curve}"
            );
        }
    }
}

#[test]
fn sustain_hooks_update_attack_decay_and_hold_without_changing_release() {
    for width in ["f32", "f64"] {
        for processor in ["ASR", "ADSR"] {
            let (extra, level, expected) = if processor == "ASR" {
                (
                    "attack_s = 0.004, sustain = 1.0,",
                    0.25,
                    [0.25, 0.25, 0.25, 0.25, 0.5, 0.25, 0.0, 0.0],
                )
            } else {
                (
                    "attack_s = 0.0, decay_s = 0.004, sustain = 0.0,",
                    0.9,
                    [0.75, 0.9, 0.9, 0.9, 0.5, 0.25, 0.0, 0.0],
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
  if frame == 4:
    env.sustain = 0.5
  if frame == 5:
    env.release()
  if frame == 6:
    env.sustain = 1.0
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
            );
            let output = render_f64(&source, 1000.0, 8);

            for (actual, expected) in output[0].iter().zip(expected) {
                assert_close_f64(*actual, expected);
            }
            assert_eq!(output[1], [0., 0., 0., 0., 0., 0., 1., 1.]);
        }
    }
}

#[test]
fn configuration_edits_preserve_exact_sample_deadlines() {
    for width in ["f32", "f64"] {
        for (edit, remaining) in [
            ("env.shape = std::smoothing::RampShape::SCurve", 7),
            ("env.decay_s = 0.05", 14),
        ] {
            let output = render_f64(
                &format!(
                    r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.025, shape = std::smoothing::RampShape::Linear)
  env.start()
  frame = 0
  count = 0
when env.finished():
  count += 1
sample:
  if frame == 18:
    {edit}
  out1 = f64(env())
  out2 = f64(count)
  frame += 1
"#
                ),
                1000.0,
                35,
            );
            for (frame, &actual) in output[0].iter().enumerate() {
                let expected = if frame < 18 {
                    1.0 - (frame + 1) as f64 / 25.0
                } else {
                    let phase = ((frame - 17) as f64 / remaining as f64).min(1.0);
                    let progress = if remaining == 7 {
                        phase * phase * (3.0 - 2.0 * phase)
                    } else {
                        phase
                    };
                    0.28 * (1.0 - progress)
                };
                assert_close_f64(actual, expected);
            }
            let terminal = 18 + remaining - 1;
            assert!(output[1][..terminal].iter().all(|&count| count == 0.0));
            assert!(output[1][terminal..].iter().all(|&count| count == 1.0));
        }
    }
}

#[test]
fn zero_duration_decay_completes_once_for_every_start_level() {
    for width in ["f32", "f64"] {
        for level in [1.0, 0.5, 0.25, 0.0] {
            let output = render_at_host_rate(
                &format!(
                    r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.0)
  completions = 0
  frame = 0
when env.finished():
  completions += 1
sample:
  if frame == 0 || frame == 4:
    env.start({width}({level}))
  out1 = f64(env())
  out2 = f64(completions)
  frame += 1
"#
                ),
                8,
            );
            assert!(output[0].iter().all(|&x| x == 0.0));
            assert_eq!(output[1], [1.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
        }
        let output = render_at_host_rate(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.0, trigger = 1.0)
  completions = 0
when env.finished():
  completions += 1
sample:
  out1 = f64(env())
  out2 = f64(completions)
"#
            ),
            4,
        );
        assert_eq!(output[0], [0.0; 4]);
        assert_eq!(output[1], [1.0; 4]);
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
fn asr_host_events_apply_sustain_and_release_time_changes() {
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
        // Seven of eight samples remain; halving the duration leaves four.
        frame(&mut instance, 0.328125, 0.0);
        event(&mut instance, "duration", Some(0.0));
        frame(&mut instance, 0.0, 1.0);
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
fn exponential_envelopes_preserve_representable_tails_until_the_duration_sample() {
    for width in ["f32", "f64"] {
        for curve in [-60.0_f64, -20.0, -4.0] {
            let output = render_at_1khz(
                &format!(
                    r#"
import std/env
outs<f64> 1
init:
  envelope = std::env::DecayEnv<{width}>(decay_s = 4.0 / SR, curve = {curve})
  envelope.start()
sample:
  out1 = f64(envelope())
"#
                ),
                5,
            );
            for (index, &actual) in output[..3].iter().enumerate() {
                let phase = (index + 1) as f64 / 4.0;
                // Evaluate the residual directly, without subtracting progress from one.
                let expected =
                    (curve * phase).exp() * -(curve * (1.0 - phase)).exp_m1() / -curve.exp_m1();
                let tolerance = if width == "f32" { 2e-6 } else { 2e-13 };
                assert!(
                    (actual - expected).abs() <= expected * tolerance,
                    "{width}, curve {curve}, sample {}: {actual} != {expected}",
                    index + 1
                );
            }
            assert_eq!(&output[3..], &[0.0, 0.0]);
        }
    }
}
