use super::*;
use onda_runtime::ExecutionOutput;

fn render(source: &str, frames: usize) -> Vec<Vec<f64>> {
    render_f64(source, 48_000.0, frames)
}

fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn allpass_delay_clamps_its_tap_and_reset_clears_the_feedback_tail() {
    for width in ["f32", "f64"] {
        for capacity in [2, 8, 32] {
            for feedback in [-0.5_f64, 0.0, 0.5] {
                let delay = capacity - 1;
                let output = render(
                    &format!(
                        r#"
import std/delay
outs<f64> 1
init:
  filter = std::delay<{capacity}>::Allpass<{width}>(delay_samples = 1000, feedback = {feedback})
  frame = 0
sample:
  input: {width} = 0.0
  if frame == 0:
    input = 1.0
  if frame == {reset_at}:
    filter.reset()
  out1 = f64(filter(input))
  frame += 1
"#,
                        reset_at = 3 * delay,
                    ),
                    6 * delay,
                );
                for (frame, &actual) in output[0].iter().enumerate() {
                    let expected = if frame == 0 {
                        -feedback
                    } else if frame == delay {
                        1.0 - feedback * feedback
                    } else if frame == 2 * delay {
                        feedback * (1.0 - feedback * feedback)
                    } else {
                        0.0
                    };
                    near(actual, expected, 1e-7);
                }
            }
        }
    }
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

#[test]
fn stdlib_selection_domains_are_nominal_enums() {
    let source = r#"
import std/filter
import std/smoothing
params:
  mode: std::filter::SvfMode = std::filter::SvfMode::BandPass
  shape: std::smoothing::RampShape = std::smoothing::RampShape::SCurve
init:
  filter = std::filter::Svf(mode = mode)
  ramp = std::smoothing::Ramp(shape = shape)
sample:
  out1 = ramp(filter(0.0))
"#;
    let typed = analyze(parse_program(source).unwrap()).unwrap();
    let mir = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    let modes = mir.interface.params[0].enumeration.as_ref().unwrap();
    assert_eq!(modes.members.len(), 6);
    assert_eq!(modes.members[2].label, "Band Pass");
    let shapes = mir.interface.params[1].enumeration.as_ref().unwrap();
    assert_eq!(shapes.members[1].label, "S-curve");

    let invalid = "import std/filter\ninit:\n  filter = std::filter::Svf(mode = std::filter::OnePoleMode::LowPass)\nsample:\n  out1 = filter(0.0)\n";
    assert!(analyze(parse_program(invalid).unwrap()).is_err());
}

#[test]
fn long_lags_and_decay_keep_moving_in_both_precisions() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/smoothing
import std/dynamics
import std/env
outs<f64> 5
init:
  lag = std::smoothing::Lag<{width}>(lag_s = 1000.0)
  asymmetric = std::smoothing::LagUD<{width}>(rise_lag_s = 1000.0)
  peak = std::dynamics::PeakFollower<{width}>(attack_lag_s = 1000.0)
  rms = std::dynamics::RmsFollower<{width}>(attack_lag_s = 1000.0)
  decay = std::env::Decay<{width}>(decay_60db_s = 10000.0)
  frame = 0
sample:
  impulse: {width} = 0.0
  if frame == 0:
    impulse = 1.0
  out1 = f64(lag({width}(1.0)))
  out2 = f64(asymmetric({width}(1.0)))
  out3 = f64(peak({width}(1.0)))
  out4 = f64(rms({width}(1.0)))
  out5 = f64(decay(impulse))
  frame += 1
"#
        );
        for rate in [48000.0, 192000.0] {
            let output = render_f64(&source, rate, 480);

            for i in [0, 47, 479] {
                let expected = -(-((i + 1) as f64) / (rate as f64 * 1000.0)).exp_m1();
                for channel in 0..4 {
                    let target = if channel == 3 {
                        expected.sqrt()
                    } else {
                        expected
                    };
                    assert!(
                        (output[channel][i] - target).abs() < target * 2e-6,
                        "{width}, {rate}, channel {channel}, frame {i}: {} != {target}",
                        output[channel][i]
                    );
                }
                let tail = 0.001_f64.powf(i as f64 / (rate as f64 * 10000.0));
                assert!((output[4][i] - tail).abs() < 4e-8);
            }
        }
    }
}

#[test]
fn slow_releases_and_delay_changes_accumulate_below_output_precision() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/smoothing
import std/dynamics
import std/delay
outs<f64> 9
init:
  lag = std::smoothing::Lag<{width}>(lag_s = 1000.0)
  lag.reset(1.0)
  asymmetric = std::smoothing::LagUD<{width}>(fall_lag_s = 1000.0)
  asymmetric.reset(1.0)
  peak = std::dynamics::PeakFollower<{width}>(attack_lag_s = 0.0, release_lag_s = 1000.0)
  rms = std::dynamics::RmsFollower<{width}>(attack_lag_s = 0.0, release_lag_s = 1000.0)
  gate = std::dynamics::Gate<{width}>(attack_lag_s = 0.0, release_lag_s = 1000.0)
  limiter = std::dynamics::Limiter<{width}>(ceiling_db = 0.0, release_lag_s = 1000.0)
  compressor = std::dynamics::Compressor<{width}>(threshold_db = 0.0, ratio = 2.0,
    knee_db = 0.0, attack_lag_s = 0.0, release_lag_s = 1000.0)
  smooth = std::delay<64>::Smooth<{width}>(delay_samples = 1.0, lag_s = 1000.0)
  delay = std::delay<64>::Delay<{width}>(delay_s = 1.0 / SR, lag_s = 1000.0)
  frame = 0
sample:
  input: {width} = 0.0
  level: {width} = 1.0
  if frame == 0:
    input = 1.0
    level = 2.0
  else:
    gate.threshold_db = 6.0
    smooth.delay_samples = 2.0
    delay.delay_s = 2.0 / SR
  out1 = f64(lag({width}(0.0)))
  out2 = f64(asymmetric({width}(0.0)))
  out3 = f64(peak(input))
  out4 = f64(rms(input))
  left, right = gate({width}(1.0), {width}(1.0))
  out5 = f64(left)
  left, right = limiter(level, level)
  out6 = f64(left)
  left, right = compressor(level, level)
  out7 = f64(left)
  ramp: {width} = {width}(frame % 16) / 16.0
  out8 = f64(smooth(ramp))
  out9 = f64(delay(ramp))
  frame += 1
"#
        );
        let output = render_f64(&source, 48000.0, 4800);
        for i in [479, 4799] {
            let remaining = (-(i as f64) / 48000000.0).exp();
            let expected = [
                (-((i + 1) as f64) / 48000000.0).exp(),
                (-((i + 1) as f64) / 48000000.0).exp(),
                remaining,
                remaining.sqrt(),
                remaining,
                1.0 - 0.5 * remaining,
                1.0 / (1.0 + remaining).sqrt(),
                ((i % 16) as f64 - (2.0 - remaining)) / 16.0,
                ((i % 16) as f64 - (2.0 - remaining)) / 16.0,
            ];
            for (channel, target) in expected.iter().enumerate() {
                assert!(
                    (output[channel][i] - target).abs() < 3e-7,
                    "{width}, channel {channel}, frame {i}: {} != {target}",
                    output[channel][i]
                );
            }
        }
    }
}

#[test]
fn dc_block_decay_tracks_elapsed_seconds_at_host_and_oversampled_rates() {
    for width in ["f32", "f64"] {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for factor in [1, 4] {
                let frames = (rate * 0.05) as usize + 1;
                let output = render_f64(
                    &format!(
                        r#"
import std/filter
proc Voice:
  delegate observed(value: f64)
  init:
    filter = std::filter::DCBlock<{width}>()
  sample {factor}:
    observed(f64(filter({width}(1.0))))
    out1 = 0.0
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
                    frames,
                );
                for millis in [0, 10, 20, 50] {
                    let frame = (rate as f64 * millis as f64 / 1000.0).round() as usize;
                    let seconds = (frame as f64 + 1.0 - 1.0 / factor as f64) / rate as f64;
                    let expected = 0.995_f64.powf(seconds * 48_000.0);
                    let actual = output[0][frame];
                    assert!(
                        (actual - expected).abs() < 2e-6 + expected * 1e-4,
                        "{width}, {rate} Hz, {factor}x, {millis} ms: {actual} != {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn lags_and_dynamics_measure_response_in_their_documented_domain() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/smoothing
import std/dynamics
import std/gain
outs<f64> 5
init:
  lag = std::smoothing::Lag<{width}>(lag_s = 0.004)
  asymmetric = std::smoothing::LagUD<{width}>(rise_lag_s = 0.004, fall_lag_s = 0.002)
  peak = std::dynamics::PeakFollower<{width}>(attack_lag_s = 0.004, release_lag_s = 0.002)
  rms = std::dynamics::RmsFollower<{width}>(attack_lag_s = 0.004, release_lag_s = 0.002)
  gain = std::gain::SmoothedDb<{width}>(db = 0.0, lag_s = 0.004)
  frame = 0
sample:
  input: {width} = 1.0
  if frame >= 4:
    input = 0.0
    lag.lag_s = 0.002
  out1 = f64(lag(input))
  out2 = f64(asymmetric(input))
  out3 = f64(peak(input))
  out4 = f64(rms(input))
  out5 = f64(gain({width}(1.0)))
  frame += 1
"#
            ),
            1000.0,
            8,
        );
        for i in 0..8 {
            let expected = if i < 4 {
                1.0 - (-((i + 1) as f64) / 4.0).exp()
            } else {
                (1.0 - (-1.0_f64).exp()) * (-((i - 3) as f64) / 2.0).exp()
            };
            for channel in 0..3 {
                assert_close_f64(output[channel][i], expected);
            }
            assert_close_f64(output[3][i], expected.sqrt());
            assert_close_f64(output[4][i], 1.0);
        }
    }
}

#[test]
fn crossfades_capture_duration_and_accept_only_the_latest_waiting_target() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/delay
outs<f64> 2
init:
  tap = std::delay<32>::Crossfade<{width}>(delay_samples = 2.0, fade_s = 0.004)
  effect = std::delay<32>::CrossfadeDelay<{width}>(delay_s = 0.002, fade_s = 0.004)
  frame = 0
sample:
  delay: {width} = 2.0
  if frame >= 6:
    delay = 4.0
  if frame >= 7:
    delay = 6.0
    tap.fade_s = 0.002
    effect.fade_s = 0.002
  if frame >= 8:
    delay = 8.0
  out1 = f64(tap({width}(frame + 1), delay_samples = delay))
  out2 = f64(effect({width}(frame + 1), delay_s = delay / SR))
  frame += 1
"#
            ),
            1000.0,
            14,
        );
        for channel in output {
            for (&actual, expected) in channel
                .iter()
                .zip([0., 0., 1., 2., 3., 4., 4.5, 5., 5.5, 6., 5., 4., 5., 6.])
            {
                assert_close_f64(actual, expected);
            }
        }
    }
}

#[test]
fn player_speed_updates_within_blocks_and_tracks_rebound_sample_rates() {
    let speeds = [1.0, 2.0, 0.0, -1.0, -2.0, 0.5, 1.0, 0.0];
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/sample
buffers:
  clip: {width}
outs<f64> 1
init:
  player = std::sample<1>::Player<{width}>(clip = clip)
  speeds: {width}[8] = [1.0, 2.0, 0.0, -1.0, -2.0, 0.5, 1.0, 0.0]
  frame = 0
event play():
  player.play(8.0)
sample:
  out1 = f64(player(speed = speeds[frame % 8]))
  frame += 1
"#
        );
        let (mut instance, _, _) = compile_instance(&source, 8);
        let mut output = [0.0_f64; 8];
        bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 64).unwrap();
        let mut position = 8.0;
        for (block, rate) in [48_000.0, 24_000.0, 96_000.0].into_iter().enumerate() {
            let mut clip64: Vec<f64> = (0..64).map(|i| (i + block * 100) as f64).collect();
            let mut clip32: Vec<f32> = clip64.iter().map(|&x| x as f32).collect();
            let (pointer, ty) = if width == "f32" {
                (clip32.as_mut_ptr().cast(), PrimitiveType::F32)
            } else {
                (clip64.as_mut_ptr().cast(), PrimitiveType::F64)
            };
            bind_buffer(&mut instance, 0, pointer, 64, 1, rate, ty).unwrap();
            if block == 0 {
                let play = instance.event_index("play").unwrap();
                trigger_event_by_index_checked(&mut instance, play, &[], ExecutionOutput::none())
                    .unwrap();
            }
            for (start, frames, flags) in
                [(0, 3, PROCESSOR_BEGIN_BLOCK), (3, 5, PROCESSOR_END_BLOCK)]
            {
                process_checked_segment(
                    &mut instance,
                    start,
                    frames,
                    flags,
                    ExecutionOutput::none(),
                )
                .unwrap();
            }
            for (frame, speed) in speeds.iter().enumerate() {
                let expected = position + (block * 100) as f64;
                assert!(
                    (output[frame] - expected).abs() < 1e-6,
                    "{width}, block {block}, sample {frame}: {} != {expected}",
                    output[frame]
                );
                position += speed * f64::from(rate) / 48_000.0;
            }
        }
    }
}

fn render_updates(block_size: usize) -> Vec<Vec<f64>> {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/stdlib-parameter-updates.onda"
    );
    let (mut instance, _, channels) = compile_instance_with_options(
        source,
        block_size,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let block = bind_f64_outputs(&mut instance, block_size);
    let mut output = vec![Vec::new(); channels];
    for _ in 0..128 / block_size {
        process_checked_segment(
            &mut instance,
            0,
            block_size,
            PROCESSOR_FULL_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        for (result, values) in output.iter_mut().zip(&block) {
            result.extend(values);
        }
    }
    output
}

#[test]
fn pitch_and_reverb_parameter_updates_are_independent_of_block_size() {
    let reference = render_updates(1);
    for block_size in [4, 32, 128] {
        let output = render_updates(block_size);
        for (channel, (actual, expected)) in output.iter().zip(&reference).enumerate() {
            assert!(
                expected.iter().any(|x| x.abs() > 0.01),
                "silent channel {channel}"
            );
            for (frame, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "block size {block_size}, channel {channel}, sample {frame}: {actual} != {expected}"
                );
            }
        }
    }
}

#[test]
fn delay_allpass_preserves_impulse_energy() {
    let frames = 512;
    let source = r#"
import std/delay
init:
  line = std::delay<32>::Allpass<f32>(delay_samples = 7)
  impulse = 1.0
sample:
  out1 = line(impulse)
  impulse = 0.0
"#;
    let (mut instance, _, _) = compile_instance(source, frames);
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
    let energy: f32 = output.iter().map(|x| x * x).sum();
    assert_near(energy, 1.0, 1e-6);
    assert_near(output[0], -0.5, 1e-6);
    assert_near(output[7], 0.75, 1e-6);
}
