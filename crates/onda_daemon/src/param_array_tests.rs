use super::*;

fn run(source: &str) -> RunSession {
    run_with_options(source, RunOptions::default())
}

fn run_with_options(source: &str, options: RunOptions) -> RunSession {
    let path = std::env::temp_dir().join("onda_param_array_test.onda");
    let mut analysis = AnalysisSession::default();
    analysis.open_document(&path, DocumentVersion(1), source.to_owned());
    RunSession::build(
        &analysis,
        &path,
        RunOptions {
            block_size: 48,
            ..options
        },
    )
    .unwrap()
}

#[test]
fn primitive_arrays_have_independent_controls_defaults_and_reset() {
    for ty in ["f32", "f64", "i32", "i64", "bool"] {
        let (defaults, domain, output) = if ty == "bool" {
            (
                "[true, false]",
                "",
                "bool_value(values[0]) + 2.0 * bool_value(values[1])",
            )
        } else {
            ("[2, 4]", "{0, 10, step = 2}", "f32(values[0] + values[1])")
        };
        let source = format!("def bool_value(value: bool):\n  if value:\n    return 1.0\n  return 0.0\nparams:\n  values: {ty}[2] = {defaults} {domain}\nsample:\n  out1 = {output}\n");
        let mut run = run(&source);
        let info = run.param_info();
        assert_eq!(info.len(), 2, "{ty}");
        assert_eq!(info[0].name, "values[0]");
        assert_eq!(info[1].type_repr, ty);
        assert_eq!(info[1].array.as_ref().unwrap().length, 2);
        assert_eq!(info[1].array.as_ref().unwrap().index, 1);
        assert_eq!(info[1].default, Some(if ty == "bool" { 0.0 } else { 4.0 }));
        let initial = run.render_block().unwrap()[0][0];
        assert_eq!(initial, if ty == "bool" { 1.0 } else { 6.0 });
        run.set_param_f64("values[1]", 100.0).unwrap();
        assert_eq!(
            run.render_block().unwrap()[0][0],
            if ty == "bool" { 3.0 } else { 12.0 }
        );
        run.restart().unwrap();
        assert_eq!(
            run.render_block().unwrap()[0][0],
            if ty == "bool" { 3.0 } else { 12.0 }
        );
        assert!(run.set_param_f64("values", 0.0).is_err());
        assert!(run.set_param_f64("values[2]", 0.0).is_err());
        run.reset_params().unwrap();
        assert_eq!(run.render_block().unwrap()[0][0], initial);
    }
}

#[test]
fn onda_array_smoothing_keeps_siblings_and_restart_targets() {
    let mut run = run("params:\n  gains: f64[2] = [0.0, 0.5] {0, 1, smooth = 0.01}\nsample:\n  out1 = f32(gains[0])\n  out2 = f32(gains[1])\n");
    run.set_param_f64("gains[0]", 1.0).unwrap();
    assert!(run.render_block().unwrap()[0]
        .iter()
        .all(|&value| value == 0.0));
    let block = run.render_block().unwrap();
    let expected = 0.1;
    assert!((block[0][0] - expected).abs() < 1e-6);
    assert!(block[0].iter().all(|&sample| sample == block[0][0]));
    assert!(block[1].iter().all(|&sample| sample == 0.5));
    run.restart().unwrap();
    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
}

#[test]
fn one_element_parameter_arrays_are_smoothed_as_arrays() {
    let mut run =
        run("params:\n  gain: f32[1] = 0.0 {0, 1, smooth = 0.01}\nsample:\n  out1 = gain[0]\n");
    run.set_param_f64("gain[0]", 1.0).unwrap();

    assert!(run.render_block().unwrap()[0]
        .iter()
        .all(|&value| value == 0.0));
    let block = run.render_block().unwrap();
    let expected = 0.1;
    assert!((block[0][0] - expected).abs() < 1e-6);
    assert!(block[0].iter().all(|&sample| sample == block[0][0]));
}

#[test]
fn smoothed_array_targets_wait_for_the_next_logical_block() {
    let mut run =
        run("params:\n  gains: f32[2] = 0.0 {0, 1, smooth = 0.01}\nsample:\n  out1 = gains[0]\n");
    let rendered = vec![0.0; run.options.block_size];
    run.begin_block_render(&rendered).unwrap();
    run.process_segment_in_batch(0, 1, onda_runtime::PROCESSOR_BEGIN_BLOCK, 0)
        .unwrap();
    run.set_param_f64("gains[0]", 1.0).unwrap();
    run.process_segment_in_batch(1, 1, onda_runtime::PROCESSOR_END_BLOCK, 0)
        .unwrap();
    assert_eq!(run.output_buffers[0][0], 0.0);
    assert_eq!(run.output_buffers[0][1], 0.0);
    assert_eq!(run.render_block().unwrap()[0][0], 0.0);
    assert!(run.render_block().unwrap()[0][0] > 0.0);
}

#[test]
fn smoothing_between_opposite_finite_extremes_stays_finite() {
    for fast_math in [false, true] {
        for seconds in [0.01, 0.001 / std::f64::consts::LN_2, 0.0001, 0.000001] {
            for (ty, maximum) in [
                ("f32", 3.0e38),
                ("f64", 1.6e308),
                ("f32", f64::from(f32::MAX)),
                ("f64", f64::MAX),
            ] {
                let maximum_literal = format!("{maximum:.1}");
                let source = format!("params:\n  gain: {ty} = {maximum_literal} {{min = -{maximum_literal}, max = {maximum_literal}, smooth = {seconds:.18}}}\nsample:\n  out1 = f32(gain / {maximum_literal})\n");
                let mut run = run_with_options(
                    &source,
                    RunOptions {
                        fast_math,
                        ..RunOptions::default()
                    },
                );
                run.set_param_f64("gain", -maximum).unwrap();
                let limit = if ty == "f32" {
                    f64::from(maximum as f32)
                } else {
                    maximum
                };
                for _ in 0..3 {
                    run.render_block().unwrap();
                    let snapshot = run.snapshot_state_bytes().unwrap();
                    let value = if ty == "f32" {
                        f64::from(f32::from_le_bytes(snapshot[..4].try_into().unwrap()))
                    } else {
                        f64::from_le_bytes(snapshot[..8].try_into().unwrap())
                    };
                    assert!(
                        value.is_finite() && value.abs() <= limit,
                        "{ty}, {seconds}, fast_math={fast_math}: {value}"
                    );
                }
            }
        }
    }
}

#[test]
fn short_linear_ramps_reach_small_targets_exactly() {
    for fast_math in [false, true] {
        for ty in ["f32", "f64"] {
            for array in [false, true] {
                for (seconds, target) in [
                    (0.000001, 1e-9),
                    (0.000001, -1e-9),
                    (0.00003333333333333333, 1e-9),
                    (0.00003333333333333333, -1e-9),
                ] {
                    let suffix = if array { "[2]" } else { "" };
                    let read = if array { "gain[0]" } else { "gain" };
                    // Positive endpoints exercise cancellation without
                    // allowing a zero result to fall within the range.
                    let minimum = if target < 0.0 { "-1.0" } else { "0.000000001" };
                    let source = format!("params:\n  gain: {ty}{suffix} = 1.0 {{{minimum}, 1.0, smooth = {seconds:.18}}}\nsample:\n  out1 = f32({read})\n");
                    let mut run = run_with_options(
                        &source,
                        RunOptions {
                            fast_math,
                            ..RunOptions::default()
                        },
                    );
                    run.set_param_f64(read, target).unwrap();
                    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
                    let expected = target as f32;
                    let output = run.render_block().unwrap()[0][0];
                    assert!((output - expected).abs() <= expected.abs() * 2e-6, "{ty}{suffix}, {seconds}, target={target}, fast_math={fast_math}: {output} != {expected}");
                    assert!(run.output_buffers[0].iter().all(|value| *value == output));
                }
            }
        }
    }
}

#[test]
fn raw_array_writes_are_clamped_for_indexed_slice_and_event_reads() {
    let mut run = run(r#"
params:
  gains: f32[2] = 0.5 {0, 1}
def sum(values: f32[]):
  return values[0] + values[1]
init:
  captured = sum(gains)
event capture():
  captured = sum(gains)
sample:
  out1 = sum(gains)
  out2 = captured
"#);
    let bytes = [f32::NAN.to_ne_bytes(), 5.0_f32.to_ne_bytes()].concat();
    set_param_by_index(&mut run.instance, 0, &bytes).unwrap();
    run.trigger_event("capture", &[]).unwrap();
    let block = run.render_block().unwrap();
    assert_eq!(block[0][0], 1.0);
    assert_eq!(block[1][0], 1.0);
}
