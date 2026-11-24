use super::*;

const FREEZE: &str = include_str!("../../../../examples/spectral/processors/spectral_freeze.onda");
const DELAY: &str = include_str!("../../../../examples/spectral/processors/spectral_delay.onda");
const RATE: usize = 48_000;
const BLOCK_SIZE: usize = 512;

fn render(engine: &str, body: &str, seconds: f32) -> Vec<[f32; 2]> {
    let source = format!("import std/osc\n{engine}\n{body}");
    let frames = (seconds * RATE as f32) as usize;
    let (mut instance, inputs, outputs) = compile_instance(&source, BLOCK_SIZE);
    assert_eq!((inputs, outputs), (0, 2));
    let mut output = vec![[0.0; 2]; frames.next_multiple_of(BLOCK_SIZE)];
    for block in output.chunks_mut(BLOCK_SIZE) {
        process_interleaved(&mut instance, &[], block.as_flattened_mut(), BLOCK_SIZE).unwrap();
    }
    output.truncate(frames);
    output
}

fn tone_power(signal: &[[f32; 2]], start: f32, end: f32, frequency: f64) -> f64 {
    let samples = &signal[(start * RATE as f32) as usize..(end * RATE as f32) as usize];
    let (mut re, mut im) = (0.0, 0.0);
    for (i, sample) in samples.iter().enumerate() {
        let phase = std::f64::consts::TAU * frequency * i as f64 / RATE as f64;
        re += f64::from(sample[0]) * phase.cos();
        im += f64::from(sample[0]) * phase.sin();
    }
    (re * re + im * im) / (samples.len() as f64).powi(2)
}

#[test]
fn spectral_freeze_smear_preserves_signed_dc_and_nyquist() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<64>::Engine(smear = 1.0)
  nyquist = -0.2
sample:
  out1, out2 = effect(0.3 + nyquist)
  nyquist = -nyquist
"#,
        2.1,
    );
    // Align to the 63-sample analysis latency before measuring Nyquist polarity.
    let (windows, _) = output[RATE / 2 + 63..].as_chunks::<{ RATE / 2 }>();
    for (window, frames) in windows.iter().enumerate() {
        for channel in 0..2 {
            let mut dc = 0.0;
            let mut nyquist = 0.0;
            for [even, odd] in frames.as_chunks::<2>().0 {
                dc += f64::from(even[channel]) + f64::from(odd[channel]);
                nyquist += f64::from(even[channel]) - f64::from(odd[channel]);
            }
            dc /= frames.len() as f64;
            nyquist /= frames.len() as f64;
            assert!(
                (0.15..0.35).contains(&dc),
                "window {window}, channel {channel}, DC {dc}"
            );
            assert!(
                (-0.25..-0.1).contains(&nyquist),
                "window {window}, channel {channel}, Nyquist {nyquist}"
            );
        }
    }
}

#[test]
fn spectral_freeze_holds_recaptures_and_releases_the_frame() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine()
  oscillator = std::osc::Sine()
  frame = 0
sample:
  frequency = 375.0
  if frame >= i32(SR * 0.4):
    frequency = 750.0
  if frame >= i32(SR * 0.8):
    frequency = 975.0
  if frame >= i32(SR * 1.2):
    frequency = 1125.0
  if frame == i32(SR * 0.8):
    effect.capture_after_source_change()
  source = oscillator(freq = frequency) * 0.2
  out1, out2 = effect(source, freeze = frame >= i32(SR * 0.2) && frame < i32(SR * 1.8), smear = 0.0)
  frame += 1
"#,
        2.4,
    );
    for (start, end, held, other) in [
        (0.5, 0.7, 375.0, 750.0),
        (1.4, 1.6, 975.0, 1125.0),
        (2.1, 2.3, 1125.0, 975.0),
    ] {
        let wanted = tone_power(&output, start, end, held);
        let unwanted = tone_power(&output, start, end, other);
        assert!(
            wanted > 0.001 && wanted > unwanted * 100.0,
            "{start}..{end}: expected {held} Hz, power {wanted}, unwanted {other} Hz power {unwanted}"
        );
    }
}

#[test]
fn spectral_freeze_recaptures_a_live_tone_just_before_it_stops() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(smear = 0.0)
  oscillator = std::osc::Sine()
  frame = 0
sample:
  frequency = 375.0
  if frame >= i32(SR * 0.2):
    frequency = 440.0
  if frame == i32(SR * 0.4):
    effect.recapture()
  source = oscillator(freq = frequency) * 0.2
  if frame >= i32(SR * 0.41):
    source = 0.0
  out1, out2 = effect(source)
  frame += 1
"#,
        1.0,
    );
    let wanted = tone_power(&output, 0.6, 0.9, 440.0);
    let old = tone_power(&output, 0.6, 0.9, 375.0);
    assert!(wanted > 0.001 && wanted > old * 100.0, "{wanted} vs {old}");
}

#[test]
fn spectral_freeze_explicit_recapture_holds_the_first_available_frame() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(smear = 0.0)
  oscillator = std::osc::Sine(freq = 750.0)
  frame = 0
sample:
  source = 0.0
  if frame < 512:
    source = oscillator() * 0.5
  if frame == 256:
    effect.recapture()
  out1, out2 = effect(source)
  frame += 1
"#,
        0.3,
    );
    // The second FFT contains only silence. The first frame must still be audible.
    let power = tone_power(&output, 0.1, 0.3, 750.0);
    assert!(power > 0.0001, "held tone power {power}");
}

#[test]
fn spectral_freeze_recaptures_an_off_bin_note_after_silence() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(smear = 0.0)
  oscillator = std::osc::Sine(freq = 440.0)
  frame = 0
sample:
  source = 0.0
  if frame >= 22016 && frame < 23040:
    source = oscillator() * 0.2
  if frame == 22272:
    effect.recapture()
  out1, out2 = effect(source)
  frame += 1
"#,
        1.3,
    );
    let (peak, power) = (345..=490)
        .map(|frequency| {
            (
                frequency,
                tone_power(&output, 0.8, 1.3, f64::from(frequency)),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let cents = 1200.0 * (f64::from(peak) / 440.0).log2();
    assert!(
        cents.abs() <= 20.0 && power > 0.001,
        "peak {peak} Hz, power {power}"
    );
}

#[test]
fn spectral_freeze_unfrozen_reconstructs_clean_audio() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine()
  oscillator = std::osc::Sine(freq = 440.0)
sample:
  source = oscillator() * 0.2
  left, right = effect(source, freeze = false, smear = 0.0)
  out1 = left
  out2 = source
"#,
        0.6,
    );
    let error = (RATE / 3..output.len())
        .map(|i| f64::from(output[i][0] - output[i - 1023][1]).powi(2))
        .sum::<f64>()
        / (output.len() - RATE / 3) as f64;
    assert!(
        error < 1e-7,
        "unfrozen reconstruction mean squared error {error}"
    );
}

#[test]
fn spectral_freeze_neutral_live_path_reconstructs_an_impulse_after_silence() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(freeze = false, smear = 0.0)
  frame = 0
sample:
  source = 0.0
  if frame == 6603:
    source = 0.5
  out1, out2 = effect(source)
  frame += 1
"#,
        0.25,
    );
    for (i, channels) in output.iter().enumerate() {
        let expected = if i == 6603 + 1023 { 0.5 } else { 0.0 };
        assert!(
            channels.iter().all(|x| (x - expected).abs() < 1e-5),
            "frame {i}: {channels:?}, expected {expected}"
        );
    }
}

#[test]
fn spectral_freeze_keeps_an_off_bin_pitch_after_the_input_stops() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine()
  oscillator = std::osc::Sine(freq = 440.0)
  frame = 0
sample:
  source = oscillator() * 0.2
  if frame >= i32(SR * 0.4):
    source = 0.0
  out1, out2 = effect(source, freeze = frame >= i32(SR * 0.2), smear = 0.0)
  frame += 1
"#,
        2.0,
    );
    let wanted = tone_power(&output, 1.0, 2.0, 440.0);
    let lower = tone_power(&output, 1.0, 2.0, 421.875);
    let upper = tone_power(&output, 1.0, 2.0, 468.75);
    assert!(
        wanted > 0.001 && wanted > (lower + upper) * 100.0,
        "held pitch power {wanted}, neighbouring FFT bins {lower} and {upper}"
    );
}

#[test]
fn spectral_freeze_captures_at_startup_and_transposes_held_partials() {
    for semitones in [-12, 12] {
        let output = render(
            FREEZE,
            &format!(
                r#"
init:
  effect = SpectralFreeze<1024>::Engine(transpose = {semitones}.0, smear = 0.0)
  low = std::osc::Sine(freq = 440.0)
  high = std::osc::Sine(freq = 660.0)
  frame = 0
sample:
  source = (low() + high()) * 0.15
  if frame >= i32(SR * 0.2):
    source = 0.0
  out1, out2 = effect(source)
  frame += 1
"#
            ),
            2.0,
        );
        let ratio = 2.0_f64.powf(semitones as f64 / 12.0);
        for original in [440.0, 660.0] {
            let shifted = tone_power(&output, 1.0, 2.0, original * ratio);
            let unshifted = tone_power(&output, 1.0, 2.0, original);
            assert!(
                shifted > 0.0001 && shifted > unshifted * 100.0,
                "{semitones} semitones: expected {} Hz, power {shifted}, original power {unshifted}",
                original * ratio
            );
        }
    }
}

#[test]
fn spectral_freeze_does_not_turn_transposed_partials_into_dc() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(transpose = -24.0, smear = 0.0)
  frame = 0
sample:
  source = 0.0
  if frame < i32(SR * 0.2):
    source = 0.2 * cos(TWO_PI * 93.75 * f32(frame) / SR)
  out1, out2 = effect(source)
  frame += 1
"#,
        2.0,
    );
    let held = &output[RATE..];
    for channel in 0..2 {
        let dc = held
            .iter()
            .map(|frame| f64::from(frame[channel]))
            .sum::<f64>()
            / held.len() as f64;
        assert!(dc.abs() < 0.002, "channel {channel}, DC {dc}");
    }
    let power = tone_power(&output, 1.0, 2.0, 23.4375);
    assert!(power > 0.0004, "held pitch power {power}");
}

#[test]
fn spectral_freeze_retunes_a_held_frame_without_recapturing() {
    let output = render(
        FREEZE,
        r#"
init:
  effect = SpectralFreeze<1024>::Engine(smear = 0.0)
  oscillator = std::osc::Sine(freq = 440.0)
  frame = 0
sample:
  source = oscillator() * 0.2
  if frame >= i32(SR * 0.2):
    source = 0.0
  if frame == i32(SR):
    effect.transpose = 12.0
  out1, out2 = effect(source)
  frame += 1
"#,
        2.0,
    );
    let initial = tone_power(&output, 0.5, 0.9, 440.0);
    let retained = tone_power(&output, 1.5, 2.0, 440.0);
    let shifted = tone_power(&output, 1.5, 2.0, 880.0);
    assert!(initial > 0.001, "initial pitch power {initial}");
    assert!(
        shifted > 0.0001 && shifted > retained * 100.0,
        "shifted pitch power {shifted}, previous pitch power {retained}"
    );
}

#[test]
fn spectral_delay_uniform_taps_match_a_time_delay() {
    let output = render(
        DELAY,
        r#"
init:
  effect = SpectralDelay<1024>::Engine(delay_s = 0.064, slope = 0.0, feedback = 0.0, stereo = 0.0)
  oscillator = std::osc::Sine(freq = 440.0)
  frame = 0
sample:
  source = oscillator() * 0.2 + 0.05
  if (frame & 1) == 0:
    source += 0.03
  else:
    source -= 0.03
  left, right = effect(source)
  out1 = left
  out2 = source
  frame += 1
"#,
        0.6,
    );
    let latency = 1023 + 6 * 512;
    let error = (RATE / 3..output.len())
        .map(|i| f64::from(output[i][0] - output[i - latency][1]).powi(2))
        .sum::<f64>()
        / (output.len() - RATE / 3) as f64;
    assert!(
        error < 1e-10,
        "uniform spectral delay mean squared error {error}"
    );
}

#[test]
fn spectral_delay_preserves_full_stereo_spread_at_maximum_delay() {
    let output = render(
        DELAY,
        r#"
init:
  effect = SpectralDelay<1024>::Engine(delay_s = 1.0, slope = 0.0, feedback = 0.0, stereo = 1.0)
  oscillator = std::osc::Sine(freq = 22500.0)
  frame = 0
sample:
  source = oscillator() * 0.2
  if frame >= i32(SR * 0.1):
    source = 0.0
  out1, out2 = effect(source)
  frame += 1
"#,
        1.5,
    );
    let centroid = |channel: usize| {
        let (weighted, energy) =
            output
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(weighted, energy), (i, s)| {
                    let power = f64::from(s[channel]).powi(2);
                    (weighted + i as f64 * power, energy + power)
                });
        weighted / energy
    };
    let spread = centroid(1) - centroid(0);
    assert!(tone_power(&output, 1.04, 1.08, 22500.0) > 0.001);
    assert!(
        (spread - (7 * 512) as f64).abs() < 8.0,
        "maximum-delay stereo spread {spread} samples"
    );
}

#[test]
fn spectral_delay_feedback_repeats_decay_across_ring_wraps() {
    let output = render(
        DELAY,
        r#"
init:
  effect = SpectralDelay<1024, 16>::Engine(delay_s = 0.064, slope = 0.0, feedback = 0.7, stereo = 0.0)
  frame = 0
sample:
  source = 0.0
  if frame == 8192:
    source = 0.2
  out1, out2 = effect(source)
  frame += 1
"#,
        1.5,
    );
    let first = 8192 + 1023 + 6 * 512;
    for repeat in 0..15 {
        let index = first + repeat * 6 * 512;
        let expected = 0.2 * 0.7_f32.powi(repeat as i32);
        assert!(
            (output[index][0] - expected).abs() < 1e-5,
            "echo {repeat}: expected {expected}, got {}",
            output[index][0]
        );
        assert_eq!(output[index][0], output[index][1]);
    }
    assert!(output
        .iter()
        .flatten()
        .all(|sample| sample.is_finite() && sample.abs() <= 0.201));
}

#[test]
fn spectral_delay_slope_separates_low_and_high_frequency_echoes() {
    let output = render(
        DELAY,
        r#"
init:
  effect = SpectralDelay<1024>::Engine(delay_s = 0.341333333, slope = 1.0, feedback = 0.0, stereo = 0.0)
  low = std::osc::Sine(freq = 375.0)
  high = std::osc::Sine(freq = 18000.0)
  frame = 0
sample:
  source = (low() + high()) * 0.15
  if frame < i32(SR * 0.1) || frame >= i32(SR * 0.15):
    source = 0.0
  out1, out2 = effect(source)
  frame += 1
"#,
        0.6,
    );
    let low_early = tone_power(&output, 0.14, 0.17, 375.0);
    let high_early = tone_power(&output, 0.14, 0.17, 18000.0);
    let low_late = tone_power(&output, 0.39, 0.42, 375.0);
    let high_late = tone_power(&output, 0.39, 0.42, 18000.0);
    assert!(
        low_early > 0.001 && low_early > high_early * 100.0,
        "early echo: low power {low_early}, high power {high_early}"
    );
    assert!(
        high_late > 0.001 && high_late > low_late * 100.0,
        "late echo: low power {low_late}, high power {high_late}"
    );
}
