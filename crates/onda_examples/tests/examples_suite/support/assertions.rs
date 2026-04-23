fn assert_near(a: f32, b: f32, eps: f32) {
    let delta = (a - b).abs();
    assert!(delta <= eps, "expected {a} ~= {b}, delta={delta}");
}

fn rms_after_skip(samples: &[f32], skip: usize) -> f32 {
    let tail = if skip < samples.len() {
        &samples[skip..]
    } else {
        samples
    };
    let energy = tail.iter().map(|sample| sample * sample).sum::<f32>();
    (energy / tail.len().max(1) as f32).sqrt()
}

fn max_abs(samples: &[f32]) -> f32 {
    samples
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0.0_f32, f32::max)
}

fn assert_non_silent(samples: &[f32], context: &str) {
    let peak = max_abs(samples);
    let rms = rms_after_skip(samples, 0);
    assert!(
        peak > 1e-3 && rms > 1e-4,
        "expected non-silent output for {context}, peak={peak}, rms={rms}"
    );
}
