use super::*;

#[test]
fn oscillators_emit_initialized_and_reset_phase_before_advancing() {
    let oscillators = [
        ("Phasor", 0.0, 0.25),
        ("Sine", 0.0, 1.0),
        ("Saw", 0.0, -0.5),
        ("SawDown", 0.0, 0.5),
        ("Pulse", 0.0, 1.0),
        ("Square", 0.0, 1.0),
        ("Triangle", -11.0 / 12.0, 0.0),
    ];
    for scalar in ["f32", "f64"] {
        let mut source = format!("import std/osc\nouts {}\ninit:\n", oscillators.len());
        for (index, (name, _, _)) in oscillators.iter().enumerate() {
            let processor = if *name == "Sine" {
                name.to_string()
            } else {
                format!("{name}<{scalar}>")
            };
            source.push_str(&format!(
                "  osc{index} = std::osc::{processor}(freq = SR * 0.0625)\n"
            ));
        }
        source.push_str("event reset():\n");
        for index in 0..oscillators.len() {
            source.push_str(&format!("  osc{index}.reset(0.25)\n"));
        }
        source.push_str("sample:\n");
        for index in 0..oscillators.len() {
            source.push_str(&format!("  out{} = f32(osc{index}())\n", index + 1));
        }

        let frames = 2;
        let (mut instance, _, channels) = compile_instance(&source, frames);
        let mut output = vec![0.0; frames * channels];
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        for (index, (_, initialized, _)) in oscillators.iter().enumerate() {
            assert_near(output[index], *initialized, 1e-6);
        }
        assert_near(output[channels], 0.0625, 1e-6);
        assert_near(
            output[channels + 1],
            (std::f32::consts::TAU / 16.0).sin(),
            1e-6,
        );

        let event = instance.event_index("reset").unwrap();
        trigger_event_by_index_checked(
            &mut instance,
            event,
            &[],
            onda_runtime::ExecutionOutput::none(),
        )
        .unwrap();
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        for (index, (_, _, reset)) in oscillators.iter().enumerate() {
            assert_near(output[index], *reset, 1e-6);
        }
        assert_near(output[channels], 0.3125, 1e-6);
        // The reset triangle evaluates phase 0.25 before advancing to 0.3125.
        assert_near(output[channels + 6], 0.25, 1e-6);
    }
}

#[test]
fn oscillator_phase_continues_across_segments_blocks_and_reset_events() {
    for scalar in ["f32", "f64"] {
        let source = format!(
            r#"import std/osc
init:
  phasor = std::osc::Phasor<{scalar}>(freq = {scalar}(SR) * 0.25)
  sine = std::osc::Sine(freq = SR * 0.25)
event reset():
  phasor.reset({scalar}(0.5))
  sine.reset(0.5)
event reverse():
  phasor.freq = -{scalar}(SR) * 0.25
  sine.freq = -SR * 0.25
sample:
  out1 = f32(phasor())
  out2 = f32(sine())
"#
        );
        let frames = 8;
        let (mut instance, _, _) = compile_instance(&source, frames);
        let mut phase = [0.0_f32; 8];
        let mut sine = [0.0_f32; 8];
        bind_output(
            &mut instance,
            0,
            phase.as_mut_ptr().cast(),
            std::mem::size_of_val(&phase),
        )
        .unwrap();
        bind_output(
            &mut instance,
            1,
            sine.as_mut_ptr().cast(),
            std::mem::size_of_val(&sine),
        )
        .unwrap();
        process_checked_segment(
            &mut instance,
            0,
            3,
            PROCESSOR_BEGIN_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .unwrap();
        for name in ["reset", "reverse"] {
            let event = instance.event_index(name).unwrap();
            trigger_event_by_index_checked(
                &mut instance,
                event,
                &[],
                onda_runtime::ExecutionOutput::none(),
            )
            .unwrap();
        }
        process_checked_segment(
            &mut instance,
            3,
            5,
            PROCESSOR_END_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(phase, [0.0, 0.25, 0.5, 0.5, 0.25, 0.0, 0.75, 0.5]);
        for (actual, expected) in sine.iter().zip([0.0, 1.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0]) {
            assert_near(*actual, expected, 1e-6);
        }
        process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none()).unwrap();
        assert_near(phase[0], 0.25, 1e-6);
        assert_near(sine[0], 1.0, 1e-6);
    }
}

#[test]
fn audio_and_control_sines_share_phase_at_block_starts_and_after_reset() {
    let source = r#"import std/osc
init:
  audio = std::osc::Sine(freq = SR / (BS * 4))
  control = std::osc::KSine(freq = SR / (BS * 4))
event reset():
  audio.reset(0.25)
  control.reset(0.25)
block:
  held = control()
  sample:
    out1 = audio()
    out2 = held
"#;
    let frames = 4;
    let (mut instance, _, channels) = compile_instance(source, frames);
    let mut output = vec![0.0; frames * channels];
    for expected in [0.0, 1.0, 0.0, -1.0, 0.0] {
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        assert_near(output[0], expected, 1e-6);
        assert_near(output[1], expected, 1e-6);
        for frame in output.chunks_exact(channels) {
            assert_near(frame[1], expected, 1e-6);
        }
    }
    let event = instance.event_index("reset").unwrap();
    trigger_event_by_index_checked(
        &mut instance,
        event,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .unwrap();
    process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 1.0, 1e-6);
}
