use super::*;
use onda_runtime::ExecutionOutput;

fn render(source: &str, frames: usize) -> Vec<Vec<f64>> {
    let (mut instance, inputs, channels) = compile_instance(source, frames);
    assert_eq!(inputs, 0);
    let mut output = vec![vec![0.0_f64; frames]; channels];
    for (channel, samples) in output.iter_mut().enumerate() {
        bind_output(
            &mut instance,
            channel,
            samples.as_mut_ptr().cast(),
            frames * 8,
        )
        .unwrap();
    }
    process_checked(&mut instance, frames, ExecutionOutput::none()).unwrap();
    output
}

fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn integer_and_mixed_mapping_arguments_preserve_fractional_results() {
    let output = render(
        r#"
outs<f64> 10
sample:
  out1 = f64(inverse_lerp(0, 10, 5))
  out2 = f64(smoothstep(0, 10, 5))
  out3 = f64(map(0, 10, 0, 100, 5))
  out4 = f64(linlin(5, 0, 10, 0, 100))
  out5 = f64(linexp(5, 0, 10, 1, 100))
  out6 = f64(linexp(0.5, 0.0, 1.0, 2, 9))
  out7 = f64(explin(3, 2, 8, 0, 1))
  out8 = f64(expexp(3, 2, 8, 1, 100))
  out9 = f64(inverse_lerp(i64(0), i64(10), i64(5)))
  out10 = f64(smoothstep(i64(0), i64(10), i64(5)))
"#,
        1,
    );
    let exponential_position = 1.5_f64.ln() / 4.0_f64.ln();
    for (channel, expected) in output.iter().zip([
        0.5,
        0.5,
        50.0,
        50.0,
        10.0,
        18.0_f64.sqrt(),
        exponential_position,
        100.0_f64.powf(exponential_position),
        0.5,
        0.5,
    ]) {
        near(channel[0], expected, 2e-6);
    }
}

#[test]
fn mappings_preserve_f64_precision_and_degenerate_ranges() {
    let output = render(
        r#"
outs<f64> 7
sample:
  out1 = inverse_lerp(f64(1.0), f64(1.0000000002), f64(1.0000000001))
  out2 = smoothstep(f64(1.0), f64(1.0000000002), f64(1.0000000001))
  out3 = f64(inverse_lerp(2, 2, 2))
  out4 = f64(smoothstep(2, 2, 3))
  out5 = f64(linexp(3, 2, 2, 4, 8))
  out6 = f64(explin(3, 2, 2, 4, 8))
  out7 = f64(expexp(3, 2, 2, 4, 8))
"#,
        1,
    );
    for (channel, expected) in output.iter().zip([0.5, 0.5, 0.0, 0.0, 4.0, 4.0, 4.0]) {
        near(channel[0], expected, 1e-12);
    }
}

#[test]
fn panning_accepts_integer_positions_and_preserves_gain_laws() {
    for width in ["i32", "i64", "f32", "f64"] {
        let output = render(
            &format!(
                r#"
import std/levels
outs<f64> 4
init:
  position: {width}[5] = [-2, -1, 0, 1, 2]
  index = 0
sample:
  left, right = std::levels::pan_linear(position[index])
  equal_left, equal_right = std::levels::pan_3db(position[index])
  out1 = f64(left)
  out2 = f64(right)
  out3 = f64(equal_left)
  out4 = f64(equal_right)
  index += 1
"#
            ),
            5,
        );
        for (i, t) in [0.0_f64, 0.0, 0.5, 1.0, 1.0].into_iter().enumerate() {
            near(output[0][i], 1.0 - t, 1e-7);
            near(output[1][i], t, 1e-7);
            near(output[2][i], (t * std::f64::consts::FRAC_PI_2).cos(), 1e-7);
            near(output[3][i], (t * std::f64::consts::FRAC_PI_2).sin(), 1e-7);
            near(output[2][i].powi(2) + output[3][i].powi(2), 1.0, 2e-7);
        }
    }
}

#[test]
fn decay_completes_once_even_when_started_at_or_below_its_threshold() {
    for width in ["f32", "f64"] {
        for (threshold, level) in [(1.0, 1.0), (0.5, 0.5), (0.5, 0.25), (0.5, 0.0)] {
            let output = render(
                &format!(
                    r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(decay_s = 0.1, end_level = {threshold})
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
        let output = render(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::DecayEnv<{width}>(end_level = 1.0, trigger = 1.0)
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
                let output = render(
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
                assert!(
                    elapsed.abs_diff(RELEASE) <= 1,
                    "{width}: released in {elapsed} samples"
                );
                let level = output[0][NOTE_OFF - 1];
                near(output[0][NOTE_OFF + RELEASE / 2 - 1], level * 0.5, 5e-5);
                assert_eq!(*output[1].last().unwrap(), 1.0);
                assert_eq!(*output[0].last().unwrap(), 0.0);
            }
        }
    }
}

#[test]
fn adsr_release_time_automation_preserves_the_captured_level() {
    for width in ["f32", "f64"] {
        let output = render(
            &format!(
                r#"
import std/env
outs<f64> 2
init:
  env = std::env::ADSR<{width}>(attack_s = 0.0, decay_s = 0.0,
    sustain = 0.5, release_s = 8.0 / SR)
  completions = 0
  frame = 0
when env.finished():
  completions += 1
sample:
  if frame == 0:
    env.start()
  if frame == 2:
    env.release()
  if frame == 4:
    env.release_s = 4.0 / SR
    env.sustain = 0.0
  out1 = f64(env())
  out2 = f64(completions)
  frame += 1
"#
            ),
            9,
        );
        assert_eq!(
            output[0],
            [1.0, 0.5, 0.4375, 0.375, 0.25, 0.125, 0.0, 0.0, 0.0]
        );
        assert_eq!(output[1], [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    }
}

#[test]
fn triangles_remain_centered_and_symmetric_after_reset_and_frequency_changes() {
    for width in ["f32", "f64"] {
        for phase in [0.0, 0.13, 0.25, 0.5, 0.75] {
            for frequency in [4800.0, -4800.0] {
                let reverse_frequency = -frequency;
                let output = render(
                    &format!(
                        r#"
import std/osc
outs<f64> 1
init:
  osc = std::osc::Triangle<{width}>(freq = {frequency}, amp = 0.25)
  frame = 0
sample:
  if frame == 0:
    osc.reset({width}({phase}))
  if frame == 1000:
    osc.freq = {width}({reverse_frequency})
    osc.reset({width}({phase}))
  if frame == 2000:
    osc.freq = 6000.0
  out1 = f64(osc())
  frame += 1
"#
                    ),
                    3000,
                );
                let samples = &output[0];
                for (section, period) in [(&samples[..2000], 10), (&samples[2000..], 8)] {
                    for cycle in section.chunks_exact(period) {
                        near(cycle.iter().sum::<f64>() / period as f64, 0.0, 1e-5);
                        for i in 0..period / 2 {
                            near(cycle[i], -cycle[i + period / 2], 2e-5);
                        }
                    }
                }
                assert!(samples.iter().all(|x| x.abs() <= 0.25));
            }
        }
    }
}

#[test]
fn negative_frequency_oscillators_reverse_the_corrected_waveforms() {
    for width in ["f32", "f64"] {
        let output = render(
            &format!(
                r#"
import std/osc
outs<f64> 10
init:
  saw = std::osc::Saw<{width}>(freq = 4800.0)
  reverse_saw = std::osc::Saw<{width}>(freq = -4800.0)
  down = std::osc::SawDown<{width}>(freq = 4800.0)
  reverse_down = std::osc::SawDown<{width}>(freq = -4800.0)
  pulse = std::osc::Pulse<{width}>(freq = 4800.0, width = 0.3)
  reverse_pulse = std::osc::Pulse<{width}>(freq = -4800.0, width = 0.3)
  square = std::osc::Square<{width}>(freq = 4800.0)
  reverse_square = std::osc::Square<{width}>(freq = -4800.0)
sample:
  out1 = f64(saw())
  out2 = f64(reverse_saw())
  out3 = f64(down())
  out4 = f64(reverse_down())
  out5 = f64(pulse())
  out6 = f64(reverse_pulse())
  out7 = f64(square())
  out8 = f64(reverse_square())
  out9 = f64(std::osc::poly_blep<{width}>({width}(0.025), {width}(-0.1)))
  out10 = f64(std::osc::poly_blep<{width}>({width}(0.025), {width}(0.0)))
"#
            ),
            100,
        );
        for pair in output[..8].as_chunks::<2>().0 {
            for i in 0..10 {
                near(pair[0][i], pair[1][(10 - i) % 10], 5e-6);
            }
        }
        for channel in [0, 1, 2, 3, 6, 7] {
            near(output[channel].iter().sum::<f64>() / 100.0, 0.0, 1e-6);
        }
        near(output[8][0], -0.5625, 1e-7);
        assert_eq!(output[9], [0.0; 100]);
    }
}

#[test]
fn biquad_low_cutoffs_remain_stable_and_pass_dc_correctly() {
    for width in ["f32", "f64"] {
        let output = render(
            &format!(
                r#"
import std/filter
outs<f64> 3
init:
  low = std::filter::Biquad<{width}>()
  high = std::filter::Biquad<{width}>()
  low_coeffs = std::filter::design_lowpass<{width}>(1.0, 0.707107)
  high_coeffs = std::filter::design_highpass<{width}>(1.0, 0.707107)
  impulse: {width} = 1.0
  frame = 0
sample:
  if frame == 0:
    low.set_coefficients(low_coeffs)
    high.set_coefficients(high_coeffs)
  out1 = f64(low({width}(1.0)))
  out2 = f64(high(impulse))
  out3 = f64(low_coeffs.b0)
  impulse = 0.0
  frame += 1
"#
            ),
            96_000,
        );
        assert!(
            output[2][0] > 0.0,
            "{width}: vanished feedforward coefficients"
        );
        assert!(output[0]
            .iter()
            .all(|&x| x.is_finite() && (0.0..1.1).contains(&x)));
        near(*output[0].last().unwrap(), 1.0, 1e-4);
        assert!(output[1].iter().all(|&x| x.is_finite() && x.abs() <= 1.0));
        near(*output[1].last().unwrap(), 0.0, 1e-6);
    }
}
