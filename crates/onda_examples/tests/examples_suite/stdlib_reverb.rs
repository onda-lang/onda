use super::*;

#[test]
fn schroeder_ring_capacity_does_not_change_the_response_when_tunings_fit() {
    for width in ["f32", "f64"] {
        let source = format!(
            r#"
import std/reverb
outs 2
init:
  standard = std::reverb::Schroeder::Reverb<{width}>()
  compact = std::reverb::Schroeder<2049, 769>::Reverb<{width}>()
  impulse: {width} = 1.0
sample:
  left, right = standard(impulse, impulse)
  compact_left, compact_right = compact(impulse, impulse)
  out1 = f32(left - compact_left)
  out2 = f32(right - compact_right)
  impulse = 0.0
"#
        );
        let frames = 10_000;
        let (mut instance, _, _) = compile_instance(&source, frames);
        let mut output = vec![0.0_f32; frames * 2];
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        assert!(output.iter().all(|&value| value == 0.0), "{width}");
    }
}

#[test]
fn schroeder_wet_impulse_has_normalized_energy_after_construction_and_reinit() {
    let frames = 48_000;
    for width in ["f32", "f64"] {
        for decay in [0.0, 0.5] {
            let source = format!(
                r#"
import std/reverb
outs 2
init:
  reverb = std::reverb::Schroeder::Reverb<{width}>(decay_60db_s = {decay}, damping = 0.0, width = 1.0)
  impulse: {width} = 1.0
event restart():
  reverb.init(decay_60db_s = {decay}, damping = 0.0, width = 1.0)
  impulse = 1.0
sample:
  left, right = reverb(impulse, impulse, decay_60db_s = {decay})
  out1 = f32(left)
  out2 = f32(right)
  impulse = 0.0
"#
            );
            let (mut instance, _, _) = compile_instance(&source, frames);
            let mut output = vec![0.0_f32; frames * 2];
            for render in 0..2 {
                if render == 1 {
                    let restart = instance.event_index("restart").unwrap();
                    trigger_event_by_index_checked(
                        &mut instance,
                        restart,
                        &[],
                        onda_runtime::ExecutionOutput::none(),
                    )
                    .unwrap();
                }
                process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
                for channel in 0..2 {
                    let energy: f32 = output.iter().skip(channel).step_by(2).map(|x| x * x).sum();
                    assert!(
                        (0.9..1.1).contains(&energy),
                        "{width}, decay {decay}, render {render}, channel {channel}: wet energy {energy}"
                    );
                }
                assert!(output.iter().all(|x| x.is_finite()));
            }
        }
    }
}

#[test]
fn reverb_nominal_decay_is_calibrated_in_seconds_at_different_sample_rates() {
    for width in ["f32", "f64"] {
        for rate in [1000.0, 2000.0] {
            let period = (rate * 0.098) as usize;
            // Capacity clips all combs to seven samples. The one-sample allpasses settle
            // much faster than the combs, so the late waveform repeats 60 dB lower.
            let output = render_f64(
                &format!(
                    r#"
import std/reverb
outs<f64> 1
init:
  reverb = std::reverb::Schroeder<8, 2>::Reverb<{width}>(decay_60db_s = 0.098, damping = 0.0, width = 1.0)
  impulse: {width} = 1.0
sample:
  left, right = reverb(impulse, impulse)
  out1 = f64(left)
  impulse = 0.0
"#
                ),
                rate,
                4 * period,
            );
            let a = output[0][2 * period];
            let b = output[0][3 * period];
            assert!(a.abs() > 1e-8);
            assert!((b / a - 0.001).abs() < 1e-7, "{width}, {rate}: {}", b / a);
        }
    }
}
