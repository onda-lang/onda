use super::*;

fn run(source: &str, smoothing: f64) -> RunSession {
    let path = std::env::temp_dir().join("onda_param_array_test.onda");
    let mut analysis = AnalysisSession::default();
    analysis.open_document(&path, DocumentVersion(1), source.to_owned());
    RunSession::build(
        &analysis,
        &path,
        RunOptions {
            block_size: 48,
            float_param_smoothing_ms: smoothing,
            ..RunOptions::default()
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
        let mut run = run(&source, 0.0);
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
fn array_float_smoothing_keeps_siblings_and_restart_targets() {
    let mut run = run("params:\n  gains: f64[2] = [0.0, 0.5] {0, 1}\nsample:\n  out1 = f32(gains[0])\n  out2 = f32(gains[1])\n", 10.0);
    run.set_param_f64("gains[0]", 1.0).unwrap();
    let block = run.render_block().unwrap();
    assert!((block[0][0] - 0.1).abs() < 1e-6);
    assert_eq!(block[1][0], 0.5);
    run.restart().unwrap();
    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
}

#[test]
fn raw_array_writes_are_clamped_for_indexed_slice_and_event_reads() {
    let mut run = run(
        r#"
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
"#,
        0.0,
    );
    let bytes = [f32::NAN.to_ne_bytes(), 5.0_f32.to_ne_bytes()].concat();
    set_param_by_index(&mut run.instance, 0, &bytes).unwrap();
    run.trigger_event("capture", &[]).unwrap();
    let block = run.render_block().unwrap();
    assert_eq!(block[0][0], 1.0);
    assert_eq!(block[1][0], 1.0);
}
