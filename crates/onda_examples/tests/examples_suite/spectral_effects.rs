use super::*;

const FREEZE: &str = include_str!("../../../../examples/spectral/processors/spectral_freeze.onda");
const DELAY: &str = include_str!("../../../../examples/spectral/processors/spectral_delay.onda");
const RATE: usize = 48_000;

fn render(engine: &str, body: &str, seconds: f32) -> Vec<[f32; 2]> {
    let source = format!("import std/osc\n{engine}\n{body}");
    let frames = (seconds * RATE as f32) as usize;
    let (mut instance, inputs, outputs) = compile_instance(&source, frames);
    assert_eq!((inputs, outputs), (0, 2));
    let mut output = vec![0.0; frames * 2];
    process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
    output.chunks_exact(2).map(|s| [s[0], s[1]]).collect()
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
    effect.recapture()
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
        assert!(wanted > 0.001 && wanted > unwanted * 100.0,
            "{start}..{end}: expected {held} Hz, power {wanted}, unwanted {other} Hz power {unwanted}");
    }
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
            assert!(shifted > 0.0001 && shifted > unshifted * 100.0,
                "{semitones} semitones: expected {} Hz, power {shifted}, original power {unshifted}", original * ratio);
        }
    }
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
