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
fn wide_i64_controls_preserve_defaults_updates_restart_and_reset() {
    let mut run = run(r#"
params:
  seeds: i64[2] = [9007199254740993, -9223372036854775808]
sample:
  out1 = f32(seeds[0] - 9007199254740992)
"#);
    let initial = crate::RunParamValue::I64(9_007_199_254_740_993);
    assert_eq!(run.param_info()[0].default, Some(initial));
    assert_eq!(run.param_info()[0].value, Some(initial));
    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
    let next = crate::RunParamValue::I64(9_007_199_254_740_995);
    run.set_param("seeds[0]", next).unwrap();
    assert_eq!(run.param_info()[0].value, Some(next));
    assert_eq!(run.render_block().unwrap()[0][0], 3.0);
    run.restart().unwrap();
    assert_eq!(run.render_block().unwrap()[0][0], 3.0);
    run.set_param("seeds[1]", crate::RunParamValue::I64(i64::MAX))
        .unwrap();
    assert_eq!(
        run.param_info()[1].value,
        Some(crate::RunParamValue::I64(i64::MAX))
    );
    run.reset_params().unwrap();
    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
    assert_eq!(
        run.param_info()[1].value,
        Some(crate::RunParamValue::I64(i64::MIN))
    );
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
        assert_eq!(
            info[1].default.map(crate::RunParamValue::as_f64),
            Some(if ty == "bool" { 0.0 } else { 4.0 })
        );
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
fn wide_floating_inputs_are_validated_against_the_parameter_type() {
    let mut run = run("params:\n  integer: i64 = 1\n  floating: f64 = 1\nsample:\n  out1 = f32(floating / 10000000000000000000.0)\n");
    let wide = crate::RunParamValue::F64(1e19);
    run.set_param("floating", wide).unwrap();
    assert_eq!(run.param_info()[1].value, Some(wide));
    assert_eq!(run.render_block().unwrap()[0][0], 1.0);
    for value in [1e19, -1e19, i64::MIN as f64, i64::MAX as f64, f64::NAN] {
        assert!(run.set_param_f64("integer", value).is_err());
    }
    assert_eq!(
        run.param_info()[0].value,
        Some(crate::RunParamValue::I64(1))
    );
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
