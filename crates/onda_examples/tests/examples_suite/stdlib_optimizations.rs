use super::*;
use onda_runtime::ExecutionOutput;

fn literal(values: &[f64]) -> String {
    values
        .iter()
        .map(|x| format!("{x:.25}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn output_buffers(instance: &mut Instance, frames: usize, channels: usize) -> Vec<Vec<f64>> {
    let mut outputs = vec![vec![0.0_f64; frames]; channels];
    for (channel, values) in outputs.iter_mut().enumerate() {
        bind_output(instance, channel, values.as_mut_ptr().cast(), frames * 8).unwrap();
    }
    outputs
}

fn close(actual: f64, expected: f64, tolerance: f64, label: &str) {
    if expected.is_nan() {
        assert!(actual.is_nan(), "{label}: {actual} should be NaN");
    } else if expected.is_infinite() {
        assert_eq!(actual, expected, "{label}");
    } else {
        assert!(
            actual.is_finite() && (actual - expected).abs() <= tolerance * expected.abs().max(1.0),
            "{label}: {actual} != {expected}"
        );
    }
}

#[test]
fn conversion_helpers_match_independent_formulas_and_edges() {
    let values = [
        -240.0, -96.0, -18.0, -6.0, -1.0, 0.0, 1.0, 6.0, 18.0, 48.0, 96.0, 240.0,
    ];
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/levels
import std/pitch
outs<f64> 8
init:
  values: {width}[12] = [{values}]
  index = 0
sample:
  x = values[index]
  out1 = f64(dbamp(x))
  out2 = f64(ampdb(x))
  out3 = f64(midicps(x))
  out4 = f64(cpsmidi(x))
  out5 = f64(midiratio(x))
  out6 = f64(ratiomidi(x))
  out7 = f64(std::levels::db_to_gain(x))
  out8 = f64(std::pitch::ratio_from_semitones(x))
  index += 1
"#,
            values = literal(&values)
        );
        let (mut instance, ins, outs) = compile_instance(&source, values.len());
        assert_eq!((ins, outs), (0, 8));
        let outputs = output_buffers(&mut instance, values.len(), outs);
        process_checked(&mut instance, values.len(), ExecutionOutput::none()).unwrap();
        let tolerance = if width == "f32" { 5e-6 } else { 5e-14 };
        for (i, &x) in values.iter().enumerate() {
            let expected = [
                10.0_f64.powf(x / 20.0),
                20.0 * x.abs().max(1e-12).log10(),
                440.0 * 2.0_f64.powf((x - 69.0) / 12.0),
                69.0 + 12.0 * (x / 440.0).log2(),
                2.0_f64.powf(x / 12.0),
                12.0 * x.log2(),
                10.0_f64.powf(x / 20.0),
                2.0_f64.powf(x / 12.0),
            ];
            for (channel, expected) in expected.into_iter().enumerate() {
                close(
                    outputs[channel][i],
                    expected,
                    tolerance,
                    &format!("{width}, helper {channel}, {x}"),
                );
            }
        }
    }
    // Integer calls must retain floating arithmetic rather than truncate the scale to zero.
    let (mut instance, _, _) = compile_instance(
        r#"
outs<f64> 6
sample:
  out1 = f64(dbamp(6))
  out2 = f64(dbamp(i64(6)))
  out3 = f64(midicps(69))
  out4 = f64(midicps(i64(69)))
  out5 = f64(midiratio(12))
  out6 = f64(midiratio(i64(12)))
"#,
        1,
    );
    let outputs = output_buffers(&mut instance, 1, 6);
    process_checked(&mut instance, 1, ExecutionOutput::none()).unwrap();
    for (output, expected) in outputs.iter().zip([
        10.0_f64.powf(0.3),
        10.0_f64.powf(0.3),
        440.0,
        440.0,
        2.0,
        2.0,
    ]) {
        close(output[0], expected, 3e-7, "integer conversion");
    }
}

// The independent reference uses base-10 dB/power formulas, not the stdlib's ln/exp path.
fn reduction(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let over = level - threshold;
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let half = knee.max(0.0) / 2.0;
    if over >= half {
        over * slope
    } else if over > -half && half > 0.0 {
        slope * (over + half).powi(2) / (4.0 * half)
    } else {
        0.0
    }
}

fn check_compressor(width: &str, configurations: &[[f64; 5]], attack: f64, release: f64) {
    let frames = 32;
    let levels = [
        0.0, 1e-15, 0.001, 0.01, 0.03, 0.07, 0.1, 0.1122, 0.1259, 0.1413, 0.2, 0.5, 1.0, 2.0,
        0.001, 0.0,
    ];
    let values: Vec<_> = levels.into_iter().cycle().take(frames).collect();
    let source = format!(
        r#"
import std/dynamics
params<{width}>:
  threshold_db = -18.0
  ratio = 4.0
  knee_db = 6.0
  makeup_db = 0.0
  amplitude = 1.0
outs<f64> 2
init:
  compressor = std::dynamics::Compressor<{width}>(
    threshold_db = threshold_db, ratio = ratio, knee_db = knee_db, makeup_db = makeup_db,
    attack_s = {attack:.25}, release_s = {release:.25})
  values: {width}[{frames}] = [{values}]
  index = 0
block:
  compressor.threshold_db = threshold_db
  compressor.ratio = ratio
  compressor.knee_db = knee_db
  compressor.makeup_db = makeup_db
  index = 0
  sample:
    x = values[index] * amplitude
    left, right = compressor(x, -x * 0.5)
    out1 = f64(left)
    out2 = f64(right)
    index += 1
"#,
        values = literal(&values)
    );
    let (mut instance, _, outs) = compile_instance(&source, frames);
    let outputs = output_buffers(&mut instance, frames, outs);
    let q = |x: f64| {
        if width == "f32" {
            f64::from(x as f32)
        } else {
            x
        }
    };
    let coefficient = |seconds: f64| {
        if seconds <= 0.0 {
            0.0
        } else {
            q(q(-1.0 / q(q(seconds) * 48000.0)).exp())
        }
    };
    let mut detector = 0.0;
    let tolerance = if width == "f32" { 3e-6 } else { 2e-13 };
    for (block, &config) in configurations.iter().enumerate() {
        for (index, value) in config.iter().enumerate() {
            if width == "f32" {
                set_param_by_index(&mut instance, index, &(*value as f32).to_ne_bytes()).unwrap();
            } else {
                set_param_by_index(&mut instance, index, &value.to_ne_bytes()).unwrap();
            }
        }
        process_checked(&mut instance, frames, ExecutionOutput::none()).unwrap();
        let [threshold, ratio, knee, makeup, amplitude] = config.map(q);
        for (i, &value) in values.iter().enumerate() {
            let left = q(q(value) * amplitude);
            let right = q(-left * 0.5);
            let peak = left.abs().max(right.abs());
            let coefficient = coefficient(if peak > detector { attack } else { release });
            detector = q(peak + q(q(detector - peak) * coefficient));
            let level = 20.0 * detector.max(q(1e-20)).log10();
            let gain = 10.0_f64.powf((makeup - reduction(level, threshold, ratio, knee)) / 20.0);
            for (channel, input) in [left, right].into_iter().enumerate() {
                close(
                    outputs[channel][i],
                    input * gain,
                    tolerance,
                    &format!("{width}, compressor block {block}, sample {i}, channel {channel}"),
                );
            }
        }
    }
}

#[test]
fn compressor_curve_and_parameter_updates_match_reference() {
    let configurations = [
        [-18.0, 4.0, 6.0, 0.0, 1.0],
        [-6.0, 2.0, 0.0, 6.0, 1.0],
        [-24.0, 1.0, 24.0, -6.0, 1.0],
        [-18.0, 0.5, -2.0, 0.0, 1.0],
        [-400.0, 4.0, 6.0, 0.0, 1.0],
        [12.0, 8.0, 18.0, 12.0, 1.0],
    ];
    for width in ["f32", "f64"] {
        check_compressor(width, &configurations, 0.0, 0.0);
    }
}

#[test]
fn compressor_detector_continues_below_knee_and_through_release() {
    let mut configurations = vec![[0.0, 4.0, 6.0, 0.0, 0.01]; 16];
    configurations.extend([[-48.0, 4.0, 6.0, 3.0, 0.01]; 16]);
    configurations.extend([[-18.0, 4.0, 6.0, 3.0, 1.0]; 16]);
    configurations.extend([[-18.0, 4.0, 6.0, 3.0, 0.001]; 64]);
    for width in ["f32", "f64"] {
        check_compressor(width, &configurations, 0.01, 0.05);
    }
}

#[test]
fn pitch_shift_complementary_windows_match_original_after_warmup() {
    // Retain the original two-cosine algorithm as a behavioral reference. Shared data/pitch
    // primitives stay current; this comparison isolates the complementary-window change.
    let reference = include_str!("stdlib_optimizations/pitch_shift_reference.onda");
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/pitch_shift
{reference}
outs<f64> 2
init:
  current = std::pitch_shift::DualWindow<{width}>()
  original = reference::DualWindow<{width}>()
  blocks = 0
  index = 0
block:
  semitones: {width} = 12.0
  if blocks >= 64:
    semitones = 0.0
  if blocks >= 128:
    semitones = -12.0
  if blocks >= 192:
    semitones = 7.0
  if blocks >= 256:
    semitones = -5.0
  current.semitones = semitones
  original.semitones = semitones
  blocks += 1
  sample:
    x = {width}(sin(f64(index) * 0.071) * 0.6 + cos(f64(index) * 0.017) * 0.2)
    out1 = f64(current(x))
    out2 = f64(original(x))
    index += 1
"#
        );
        let (mut instance, _, _) = compile_instance(&source, 128);
        let outputs = output_buffers(&mut instance, 128, 2);
        let mut nonzero = 0;
        for block in 0..320 {
            process_checked(&mut instance, 128, ExecutionOutput::none()).unwrap();
            for (i, (&current, &original)) in outputs[0].iter().zip(outputs[1].iter()).enumerate() {
                close(
                    current,
                    original,
                    if width == "f32" { 2e-6 } else { 2e-13 },
                    &format!("{width}, pitch block {block}, sample {i}"),
                );
                nonzero += usize::from(current.abs() > 0.01);
            }
        }
        assert!(
            nonzero > 10000,
            "pitch validation must cover active, warmed output"
        );
    }
}
