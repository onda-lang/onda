use super::*;

fn build(source: &str, block_size: usize, seconds: f64) -> Result<RunSession, RunBuildError> {
    let path = std::env::temp_dir().join("onda_host_param_smoothing_test.onda");
    let mut analysis = AnalysisSession::default();
    analysis.open_document(&path, DocumentVersion(1), source.to_owned());
    RunSession::build(
        &analysis,
        &path,
        RunOptions {
            block_size,
            param_smoothing_seconds: seconds,
            ..RunOptions::default()
        },
    )
}

fn control(ty: &str, suffix: &str) -> String {
    let read = if suffix.is_empty() { "gain" } else { "gain[0]" };
    format!("params:\n  gain: {ty}{suffix} = 0.0 {{0, 1}}\ninit:\n  cached: {ty} = 0.0\nblock:\n  cached = {read}\n  sample:\n    out1 = f32({read})\n    out2 = f32(cached)\n")
}

fn assert_block(run: &mut RunSession, expected: f32) {
    for channel in run.render_block().unwrap() {
        for value in channel {
            assert!((value - expected).abs() < 1e-6, "{value} != {expected}");
        }
    }
}

#[test]
fn host_ramps_are_finite_and_do_not_add_processor_state() {
    for ty in ["f32", "f64"] {
        for suffix in ["", "[1]", "[2]"] {
            let mut run = build(&control(ty, suffix), 48, 0.003).unwrap();
            let direct = build(&control(ty, suffix), 48, 0.0).unwrap();
            assert_eq!(
                run.snapshot_state_bytes().unwrap(),
                direct.snapshot_state_bytes().unwrap()
            );
            assert_block(&mut run, 0.0);
            let name = if suffix.is_empty() { "gain" } else { "gain[0]" };
            run.set_param_f64(name, 1.0).unwrap();
            assert_eq!(run.param_info()[0].value, Some(1.0));
            assert_block(&mut run, 1.0 / 3.0);
            // Repeated target writes must not restart the deadline.
            run.set_param_f64(name, 1.0).unwrap();
            assert_block(&mut run, 2.0 / 3.0);
            assert_block(&mut run, 1.0);
            assert_block(&mut run, 1.0);
            if suffix == "[2]" {
                assert_eq!(run.param_info()[1].value, Some(0.0));
            }
        }
    }
}

#[test]
fn startup_restart_and_reset_settle_targets_immediately() {
    let mut run = build(&control("f64", "[2]"), 48, 0.003).unwrap();
    run.set_param_f64("gain[0]", 0.5).unwrap();
    assert_block(&mut run, 0.5);
    run.set_param_f64("gain[0]", 1.0).unwrap();
    assert_block(&mut run, 2.0 / 3.0);
    run.restart().unwrap();
    assert_block(&mut run, 1.0);
    run.set_param_f64("gain[0]", 0.0).unwrap();
    assert_block(&mut run, 2.0 / 3.0);
    run.reset_params().unwrap();
    assert_block(&mut run, 0.0);
    assert_block(&mut run, 0.0);
}

#[test]
fn retargeting_uses_the_published_value_and_reaches_small_targets_exactly() {
    for ty in ["f32", "f64"] {
        let mut run = build(&control(ty, ""), 48, 0.003).unwrap();
        assert_block(&mut run, 0.0);
        run.set_param_f64("gain", 1.0).unwrap();
        assert_block(&mut run, 1.0 / 3.0);
        run.set_param_f64("gain", 0.0).unwrap();
        assert_block(&mut run, 2.0 / 9.0);
        assert_block(&mut run, 1.0 / 9.0);
        assert_block(&mut run, 0.0);
        run.set_param_f64("gain", 1e-9).unwrap();
        for _ in 0..3 {
            run.render_block().unwrap();
        }
        assert_eq!(run.render_block().unwrap()[0][0], 1e-9_f32);
    }
}

#[test]
fn events_and_process_segments_share_one_published_value() {
    let source = "params:\n  gain = 0.0 {0, 1}\ninit:\n  captured = gain\nevent capture():\n  captured = gain\nblock:\n  cached = gain\n  sample:\n    out1 = gain\n    out2 = cached\n    out3 = captured\n";
    let mut run = build(source, 48, 0.003).unwrap();
    assert_block(&mut run, 0.0);
    run.set_param_f64("gain", 1.0).unwrap();
    let mut output = vec![0.0; 48 * 3];
    run.render_block_events_interleaved(
        &mut output,
        &[
            RunScheduledEvent {
                frame: 0,
                name: "capture",
                values: &[],
            },
            RunScheduledEvent {
                frame: 24,
                name: "capture",
                values: &[],
            },
        ],
    )
    .unwrap();
    assert!(output.iter().all(|value| (*value - 1.0 / 3.0).abs() < 1e-6));
    // A target written between segments is applied at the following block.
    run.begin_block_render(&output, 48).unwrap();
    run.process_segment_in_batch(0, 24, onda_runtime::PROCESSOR_BEGIN_BLOCK, 0)
        .unwrap();
    run.set_param_f64("gain", 0.0).unwrap();
    run.process_segment_in_batch(24, 24, onda_runtime::PROCESSOR_END_BLOCK, 0)
        .unwrap();
    assert!(run.output_buffers[0]
        .iter()
        .all(|value| (*value - 2.0 / 3.0).abs() < 1e-6));
    assert!((run.render_block().unwrap()[0][0] - 4.0 / 9.0).abs() < 1e-6);
}

#[test]
fn empty_and_zero_frame_schedules_preserve_startup_and_ramp_time() {
    let boundaries = [
        (0, 0, onda_runtime::PROCESSOR_BEGIN_BLOCK),
        (48, 0, onda_runtime::PROCESSOR_END_BLOCK),
    ];
    for ty in ["f32", "f64"] {
        for suffix in ["", "[2]"] {
            for segments in [&[][..], &boundaries[..]] {
                let mut run = build(&control(ty, suffix), 48, 0.003).unwrap();
                let name = if suffix.is_empty() { "gain" } else { "gain[0]" };
                for _ in 0..2 {
                    run.render_block_segments(segments).unwrap();
                }
                run.set_param_f64(name, 0.5).unwrap();
                assert_block(&mut run, 0.5);
                run.set_param_f64(name, 1.0).unwrap();
                for _ in 0..2 {
                    run.render_block_segments(segments).unwrap();
                }
                assert_block(&mut run, 2.0 / 3.0);
                assert_block(&mut run, 5.0 / 6.0);
                assert_block(&mut run, 1.0);
            }
        }
    }
}

#[test]
fn invalid_segment_schedules_preserve_ramp_and_processor_state() {
    for invalid in [
        (0, 49, onda_runtime::PROCESSOR_FULL_BLOCK),
        (49, 0, onda_runtime::PROCESSOR_FULL_BLOCK),
        (usize::MAX, 1, onda_runtime::PROCESSOR_FULL_BLOCK),
        (0, 48, 4),
    ] {
        let mut run = build(&control("f32", ""), 48, 0.003).unwrap();
        assert_block(&mut run, 0.0);
        run.set_param_f64("gain", 1.0).unwrap();
        let snapshot = run.snapshot_state_bytes().unwrap();
        for segments in [
            &[invalid][..],
            &[(0, 24, onda_runtime::PROCESSOR_BEGIN_BLOCK), invalid][..],
        ] {
            assert!(run.render_block_segments(segments).is_err());
            assert_eq!(run.snapshot_state_bytes().unwrap(), snapshot);
        }
        assert_block(&mut run, 1.0 / 3.0);
        assert_block(&mut run, 2.0 / 3.0);
        assert_block(&mut run, 1.0);
    }
}

#[test]
fn rejected_event_schedules_preserve_ramp_and_processor_state() {
    let valid_values = [RunEventValue::Array(vec![
        RunEventValue::Number(0.25),
        RunEventValue::Number(0.75),
    ])];
    for (name, values) in [
        ("missing", vec![]),
        ("capture", vec![]),
        ("capture", vec![RunEventValue::Number(0.25)]),
        (
            "capture",
            vec![RunEventValue::Array(vec![RunEventValue::Number(0.25)])],
        ),
        (
            "capture",
            vec![RunEventValue::Array(vec![
                RunEventValue::Bool(false),
                RunEventValue::Bool(true),
            ])],
        ),
    ] {
        for after_valid_event in [false, true] {
            let source = format!(
                "{}event capture(values: f32[2]):\n  cached = values[0]\n",
                control("f32", "")
            );
            let mut run = build(&source, 48, 0.003).unwrap();
            assert_block(&mut run, 0.0);
            run.set_param_f64("gain", 1.0).unwrap();
            let snapshot = run.snapshot_state_bytes().unwrap();
            let valid = RunScheduledEvent {
                frame: 0,
                name: "capture",
                values: &valid_values,
            };
            let invalid = RunScheduledEvent {
                frame: if after_valid_event { 24 } else { 0 },
                name,
                values: &values,
            };
            let events = if after_valid_event {
                vec![valid, invalid]
            } else {
                vec![invalid]
            };
            let mut output = vec![-1.0; 48 * 2];
            for _ in 0..2 {
                assert!(run
                    .render_block_events_interleaved(&mut output, &events)
                    .is_err());
                assert_eq!(run.snapshot_state_bytes().unwrap(), snapshot);
                assert!(output.iter().all(|value| *value == -1.0));
            }
            assert_block(&mut run, 1.0 / 3.0);
            assert_block(&mut run, 2.0 / 3.0);
            assert_block(&mut run, 1.0);
        }
    }
}

#[test]
fn zero_frame_boundaries_do_not_double_count_segmented_audio() {
    let mut run = build(&control("f32", "[2]"), 48, 0.003).unwrap();
    assert_block(&mut run, 0.0);
    run.set_param_f64("gain[0]", 1.0).unwrap();
    let channels = run
        .render_block_segments(&[
            (0, 0, onda_runtime::PROCESSOR_BEGIN_BLOCK),
            (0, 17, 0),
            (17, 31, 0),
            (48, 0, onda_runtime::PROCESSOR_END_BLOCK),
        ])
        .unwrap();
    assert!(channels
        .iter()
        .flatten()
        .all(|value| (*value - 1.0 / 3.0).abs() < 1e-6));
    assert_block(&mut run, 2.0 / 3.0);
    assert_block(&mut run, 1.0);
}

#[test]
fn partial_segment_schedules_advance_by_their_audio_frame_count() {
    let mut run = build(&control("f64", ""), 48, 0.003).unwrap();
    assert_block(&mut run, 0.0);
    run.set_param_f64("gain", 1.0).unwrap();
    let channels = run
        .render_block_segments(&[(0, 12, onda_runtime::PROCESSOR_FULL_BLOCK)])
        .unwrap();
    for channel in channels {
        assert!(channel[..12]
            .iter()
            .all(|value| (*value - 1.0 / 12.0).abs() < 1e-6));
        assert!(channel[12..].iter().all(|value| *value == 0.0));
    }
    assert_block(&mut run, 5.0 / 12.0);
    assert_block(&mut run, 0.75);
    assert_block(&mut run, 1.0);
}

#[test]
fn disabled_and_at_most_one_block_durations_write_directly() {
    for (block, seconds) in [(48, 0.0), (48, 0.001), (1024, 0.02), (2048, 0.03)] {
        let mut run = build(&control("f32", ""), block, seconds).unwrap();
        assert_block(&mut run, 0.0);
        run.set_param_f64("gain", 1.0).unwrap();
        assert_block(&mut run, 1.0);
    }
    let mut run = build(&control("f32", ""), 1024, 0.03).unwrap();
    assert_block(&mut run, 0.0);
    run.set_param_f64("gain", 1.0).unwrap();
    assert_block(&mut run, 1024.0 / 1440.0);
    assert_block(&mut run, 1.0);
}

#[test]
fn integer_boolean_and_stepped_float_controls_bypass_ramps() {
    for ty in ["i32", "i64", "f32", "f64", "bool"] {
        let source = if ty == "bool" {
            "params:\n  value = false\nsample:\n  if value:\n    out1 = 1.0\n  else:\n    out1 = 0.0\n".to_owned()
        } else {
            format!(
                "params:\n  value: {ty} = 0 {{0, 10, step = 2}}\nsample:\n  out1 = f32(value)\n"
            )
        };
        let mut run = build(&source, 48, 0.03).unwrap();
        assert_block(&mut run, 0.0);
        run.set_param_f64("value", 100.0).unwrap();
        assert_block(&mut run, if ty == "bool" { 1.0 } else { 10.0 });
    }
}

#[test]
fn finite_extremes_do_not_overflow_during_interpolation() {
    for (ty, maximum) in [("f32", f64::from(f32::MAX)), ("f64", f64::MAX)] {
        let source = format!(
            "params:\n  gain: {ty} = {maximum:.1}\nsample:\n  out1 = f32(gain / {maximum:.1})\n"
        );
        let mut run = build(&source, 48, 0.003).unwrap();
        assert_block(&mut run, 1.0);
        run.set_param_f64("gain", -maximum).unwrap();
        for expected in [1.0 / 3.0, -1.0 / 3.0, -1.0] {
            assert_block(&mut run, expected);
        }
    }
}

#[test]
fn invalid_host_durations_are_rejected() {
    for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(matches!(
            build(&control("f32", ""), 48, seconds),
            Err(RunBuildError::Runtime(_))
        ));
    }
}

#[test]
fn smoothing_duration_changes_live_and_can_be_enabled_after_starting_disabled() {
    for ty in ["f32", "f64"] {
        for suffix in ["", "[2]"] {
            let mut run = build(&control(ty, suffix), 48, 0.0).unwrap();
            let name = if suffix.is_empty() { "gain" } else { "gain[0]" };
            assert_block(&mut run, 0.0);
            run.set_param_f64(name, 0.25).unwrap();
            assert_block(&mut run, 0.25);
            run.set_param_smoothing_seconds(0.003).unwrap();
            run.set_param_f64(name, 1.0).unwrap();
            assert_block(&mut run, 0.5);
            run.set_param_smoothing_seconds(0.002).unwrap();
            assert_block(&mut run, 0.75);
            // Invalid changes leave both the option and active ramp intact.
            for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
                assert!(run.set_param_smoothing_seconds(seconds).is_err());
                assert_eq!(run.options().param_smoothing_seconds, 0.002);
            }
            assert_block(&mut run, 1.0);
            run.set_param_f64(name, 0.0).unwrap();
            assert_block(&mut run, 0.5);
            run.set_param_smoothing_seconds(0.0).unwrap();
            assert_block(&mut run, 0.0);
            run.set_param_f64(name, 1.0).unwrap();
            assert_block(&mut run, 1.0);
            run.set_param_smoothing_seconds(0.003).unwrap();
            run.set_param_f64(name, 0.0).unwrap();
            assert_block(&mut run, 2.0 / 3.0);
            run.restart().unwrap();
            assert_block(&mut run, 0.0);
            run.set_param_f64(name, 1.0).unwrap();
            assert_block(&mut run, 1.0 / 3.0);
        }
    }
}

#[test]
fn duration_changes_do_not_restart_cancelled_nonfinite_transitions() {
    for ty in ["f32", "f64"] {
        for target in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let source = control(ty, "").replace(" {0, 1}", "");
            let mut run = build(&source, 48, 0.003).unwrap();
            assert_block(&mut run, 0.0);
            run.set_param_f64("gain", 1.0).unwrap();
            assert_block(&mut run, 1.0 / 3.0);
            run.set_param_f64("gain", target).unwrap();
            run.set_param_smoothing_seconds(0.006).unwrap();
            assert!(run
                .render_block()
                .unwrap()
                .iter()
                .flatten()
                .all(|value| if target.is_nan() {
                    value.is_nan()
                } else {
                    *value == target as f32
                }));
        }
    }
}
