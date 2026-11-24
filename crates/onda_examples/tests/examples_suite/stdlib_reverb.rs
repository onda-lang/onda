use super::*;

#[test]
fn short_comb_retains_wet_output_and_decay_at_high_effective_rates() {
    let rate = 48_000.0 * 128.0;
    let first = 256;
    let distance = 32_768;
    let output = render_f64(
        r#"
import std/reverb
outs<f64> 2
init:
  narrow = std::reverb::Schroeder<2, 2>::Reverb<f32>(decay_60db_s = 60.0, damping = 0.0, width = 1.0)
  wide = std::reverb::Schroeder<2, 2>::Reverb<f64>(decay_60db_s = 60.0, damping = 0.0, width = 1.0)
  impulse = 1.0
sample:
  narrow_l, _ = narrow(impulse, impulse)
  wide_l, _ = wide(f64(impulse), f64(impulse))
  impulse = 0.0
  out1 = f64(narrow_l)
  out2 = wide_l
"#,
        rate,
        first + distance + 1,
    );
    // Once the allpass transients settle, the wet signal follows the comb.
    let wide_pole = 0.001_f64.powf(1.0 / (f64::from(rate) * 60.0));
    // The requested pole rounds to unity in f32. Apply the closest decaying pole
    // to the normalized excitation, rounding each feedback pass as the comb does.
    let narrow_pole = 1.0_f32.next_down();
    let excitation = ((1.0 - narrow_pole) * (1.0 + narrow_pole) * 0.25).sqrt();
    let narrow_history = (0..first).fold(excitation, |v, _| v * narrow_pole);
    let narrow_tail = (0..distance).fold(narrow_history, |v, _| v * narrow_pole);
    for (samples, expected) in output.iter().zip([
        f64::from(narrow_tail / narrow_history),
        wide_pole.powi(distance as i32),
    ]) {
        assert!(samples[first].abs() > 1e-5);
        let ratio = samples[first + distance] / samples[first];
        assert!((ratio - expected).abs() < 1e-6, "{ratio} vs {expected}");
    }
}

#[test]
fn changing_decay_preserves_reflections_already_in_the_comb() {
    for width in ["f32", "f64"] {
        let output = render_f64(
            &format!(
                r#"
import std/reverb
outs<f64> 2
init:
  reference = std::reverb::Schroeder::Reverb<{width}>(decay_60db_s = 60.0, damping = 0.0, width = 1.0)
  changed = std::reverb::Schroeder::Reverb<{width}>(decay_60db_s = 60.0, damping = 0.0, width = 1.0)
  frame = 0
  impulse: {width} = 1.0
sample:
  if frame == 1116:
    changed.decay_60db_s = 0.0
  reference_l, _ = reference(impulse, impulse)
  changed_l, _ = changed(impulse, impulse)
  out1 = f64(reference_l)
  out2 = f64(changed_l)
  impulse = 0.0
  frame += 1
"#
            ),
            48_000.0,
            2233,
        );
        // The first comb returns its stored impulse at 1116 and its feedback at 2232.
        assert!(output[0][1116].abs() > 0.001);
        assert_eq!(output[0][..2232], output[1][..2232], "{width}");
        assert!((output[0][2232] - output[1][2232]).abs() > 0.001, "{width}");
    }
}

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
