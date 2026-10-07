use super::*;

#[test]
fn formant_percussion_has_centered_audio_output() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/instruments/formant_percussion.onda");
    let source = fs::read_to_string(path).unwrap();
    let frames = 480;
    let (mut instance, inputs, channels) = compile_instance(&source, frames);
    assert_eq!((inputs, channels), (0, 2));
    let mut output = vec![0.0_f32; frames * channels];
    let mut sums = [0.0_f64; 2];
    let mut squared_sums = [0.0_f64; 2];

    // Render four seconds, measuring after the first second of startup.
    for block in 0..400 {
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        if block >= 100 {
            for frame in output.chunks_exact(channels) {
                for (channel, &sample) in frame.iter().enumerate() {
                    let sample = f64::from(sample);
                    sums[channel] += sample;
                    squared_sums[channel] += sample * sample;
                }
            }
        }
    }

    let measured_frames = (300 * frames) as f64;
    for channel in 0..channels {
        let mean = sums[channel] / measured_frames;
        let ac_rms = (squared_sums[channel] / measured_frames - mean * mean).sqrt();
        assert!(mean.abs() < 0.005, "channel {channel}: DC mean {mean}");
        // Removing DC after heavy clipping can leave a centered but quiet signal.
        assert!(ac_rms > 0.1, "channel {channel}: AC RMS {ac_rms}");
    }
}
