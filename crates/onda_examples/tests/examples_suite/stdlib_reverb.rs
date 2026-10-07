use super::*;

#[test]
fn schroeder_allpass_preserves_impulse_energy() {
    let frames = 512;
    let source = r#"
import std/reverb
init:
  line = std::reverb::Schroeder<64, 32>::AllpassLine<f32>(length = 7)
  impulse = 1.0
sample:
  out1 = line.process(impulse)
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

#[test]
fn schroeder_wet_impulse_has_normalized_energy() {
    let frames = 48_000;
    let source = r#"
import std/reverb
outs 2
init:
  reverb = std::reverb::Schroeder::Reverb(room_size = 0.0, damping = 0.0, width = 1.0)
  impulse = 1.0
sample:
  out1, out2 = reverb(impulse, impulse)
  impulse = 0.0
"#;
    let (mut instance, _, _) = compile_instance(source, frames);
    let mut output = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
    for channel in 0..2 {
        let energy: f32 = output.iter().skip(channel).step_by(2).map(|x| x * x).sum();
        assert!(
            (0.9..1.1).contains(&energy),
            "channel {channel}: wet energy {energy}"
        );
    }
    assert!(output.iter().all(|x| x.is_finite()));
}
