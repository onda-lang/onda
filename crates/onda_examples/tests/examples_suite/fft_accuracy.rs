use super::*;

fn literal(values: &[f64]) -> String {
    values
        .iter()
        .map(|x| format!("{x:.17}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn signal(n: usize, scalar: &str, seed: u32) -> Vec<f64> {
    let mut state = seed;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let value = f64::from(state >> 8) / 16777216.0 * 2.0 - 1.0;
            if scalar == "f32" {
                f64::from(value as f32)
            } else {
                value
            }
        })
        .collect()
}

// An independent O(N²) DFT avoids validating the FFT against itself.
fn dft(real: &[f64], imag: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = real.len();
    (0..n)
        .map(|k| {
            let mut re = 0.0;
            let mut im = 0.0;
            for j in 0..n {
                let angle = -std::f64::consts::TAU * (k * j) as f64 / n as f64;
                let (s, c) = angle.sin_cos();
                re += real[j] * c - imag[j] * s;
                im += real[j] * s + imag[j] * c;
            }
            (re, im)
        })
        .unzip()
}

fn run_f64_outputs(source: &str, frames: usize, channels: usize) -> Vec<Vec<f64>> {
    let (mut instance, ins, outs) = compile_instance(source, frames);
    assert_eq!((ins, outs), (0, channels));
    let mut output = vec![vec![0.0_f64; frames]; channels];
    for (i, values) in output.iter_mut().enumerate() {
        bind_output(&mut instance, i, values.as_mut_ptr().cast(), frames * 8).unwrap();
    }
    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none()).unwrap();
    output
}

fn close(actual: &[f64], expected: &[f64], tolerance: f64, label: &str) {
    let scale = expected.iter().fold(1.0_f64, |a, x| a.max(x.abs()));
    for (k, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            actual.is_finite() && (actual - expected).abs() <= tolerance * scale,
            "{label}, bin {k}: {actual} != {expected}, tolerance {}",
            tolerance * scale
        );
    }
}

#[test]
fn fft_radix4_matches_independent_dft_and_inverse() {
    for scalar in ["f32", "f64"] {
        for n in [2, 4, 8, 16, 32, 64, 128, 256, 1024] {
            let real = signal(n, scalar, 12345);
            let imag = signal(n, scalar, 67890);
            let source = format!(
                r#"
import std/fft
const N = {n}
outs<f64> 7
init:
  real: {scalar}[N] = [{real_literal}]
  imag: {scalar}[N] = [{imag_literal}]
  fft = std::fft<N>::FFT<{scalar}>()
  fft.forward_complex(real, imag)
  re: {scalar}[N]
  im: {scalar}[N]
  fft.store_real(re)
  fft.store_imag(im)
  fft.inverse()
  full = std::fft<N>::FFT<{scalar}>()
  full.forward_real(real)
  packed = std::fft<N>::RealTransform<{scalar}>()
  packed.forward(real)
  pr: {scalar}[N / 2 + 1]
  packed_imag: {scalar}[N / 2 + 1]
  for k in 0..(N / 2 + 1):
    pr[k] = packed.real(k)
    packed_imag[k] = packed.imag(k)
  restored: {scalar}[N]
  packed.inverse(pr, packed_imag, restored)
  index = 0
sample:
  out1 = f64(re[index])
  out2 = f64(im[index])
  out3 = f64(fft.real(index))
  out4 = f64(fft.imag(index))
  out5 = f64(full.real(index))
  out6 = f64(full.imag(index))
  out7 = f64(restored[index])
  index += 1
"#,
                real_literal = literal(&real),
                imag_literal = literal(&imag)
            );
            let output = run_f64_outputs(&source, n, 7);
            let (re, im) = dft(&real, &imag);
            let (rr, ri) = dft(&real, &vec![0.0; n]);
            let tolerance = if scalar == "f32" { 2e-6 } else { 2e-12 };
            let label = format!("{scalar}, N={n}");
            for (actual, expected) in output.iter().zip([&re, &im, &real, &imag, &rr, &ri, &real]) {
                close(actual, expected, tolerance, &label);
            }
        }
    }
}

#[test]
fn stft_windows_and_short_inputs_match_independent_dft() {
    let n = 32;
    for scalar in ["f32", "f64"] {
        for window in ["hann", "rectangular", "hamming", "blackman"] {
            for count in [1, 17, 32] {
                let input = signal(count, scalar, 12345);
                let mut windowed = vec![0.0; n];
                for (i, value) in input.iter().enumerate() {
                    let phase = std::f64::consts::TAU * i as f64 / (n - 1) as f64;
                    let w = match window {
                        "rectangular" => 1.0,
                        "hamming" => 0.54 - 0.46 * phase.cos(),
                        "blackman" => 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos(),
                        _ => 0.5 - 0.5 * phase.cos(),
                    };
                    windowed[i] = if scalar == "f32" {
                        f64::from((*value as f32) * (w as f32))
                    } else {
                        value * w
                    };
                }
                let source = format!(
                    r#"
import std/fft
outs<f64> 2
init:
  values: {scalar}[{count}] = [{values}]
  fft = std::fft<{n}>::STFT<{scalar}>()
  fft.set_{window}()
  fft.forward_real(values)
  index = 0
sample:
  out1 = f64(fft.real(index))
  out2 = f64(fft.imag(index))
  index += 1
"#,
                    values = literal(&input)
                );
                let output = run_f64_outputs(&source, n, 2);
                let (re, im) = dft(&windowed, &vec![0.0; n]);
                let tolerance = if scalar == "f32" { 2e-6 } else { 2e-12 };
                close(&output[0], &re, tolerance, window);
                close(&output[1], &im, tolerance, window);
            }
        }
    }
}
