use onda_daemon::{AnalysisSession, DocumentVersion, RunEventValue, RunOptions, RunSession};

fn build(source: &str) -> RunSession {
    let path = std::env::temp_dir().join("onda_typed_ports.onda");
    let mut analysis = AnalysisSession::default();
    analysis.open_document(&path, DocumentVersion(1), source.to_owned());
    RunSession::build(
        &analysis,
        path,
        RunOptions {
            block_size: 4,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn mixed_audio_ports_convert_values_and_preserve_channel_layout() {
    let mut run = build(
        r#"
ins:
  single: f32
  pair: f64[2]
  integer: i32
  wide: i64
  enabled: bool
outs:
  single_out: f32
  pair_out: f64[2]
  integer_out: i32
  wide_out: i64
  enabled_out: bool
sample:
  single_out = single
  pair_out[0] = pair[0]
  pair_out[1] = pair[1]
  integer_out = integer
  wide_out = wide
  enabled_out = enabled
"#,
    );
    run.set_input_block(
        &[
            -0.0,
            0.125,
            -0.25,
            7.9,
            -9.9,
            0.0,
            0.5,
            0.25,
            0.5,
            -7.9,
            9.9,
            -0.0,
            -0.5,
            0.5,
            0.75,
            42.0,
            -43.0,
            -1.0,
            1.0,
            -1.0,
            -2.0,
            1_065_353_216.0,
            16_777_216.0,
            1.0,
        ],
        6,
    );
    let rendered = run.render_block().unwrap();
    assert_eq!(rendered[0][0].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(
        rendered,
        vec![
            vec![-0.0, 0.5, -0.5, 1.0],
            vec![0.125, 0.25, 0.5, -1.0],
            vec![-0.25, 0.5, 0.75, -2.0],
            vec![7.0, -7.0, 42.0, 1_065_353_216.0],
            vec![-9.0, 9.0, -43.0, 16_777_216.0],
            vec![0.0, 0.0, 1.0, 1.0],
        ]
    );

    // A shorter input callback and fewer channels must not expose stale samples.
    run.set_input_block(&[0.25, -0.5, 0.75, 1.0, 99.0], 2);
    let rendered = run.render_block().unwrap();
    assert_eq!(rendered[0], [0.25, 0.75, 0.0, 0.0]);
    assert_eq!(rendered[1], [-0.5, 1.0, 0.0, 0.0]);
    assert!(rendered[2..].iter().flatten().all(|value| *value == 0.0));
    run.set_input_block(&[], 0);
    assert!(run
        .render_block()
        .unwrap()
        .iter()
        .flatten()
        .all(|value| *value == 0.0));
}

#[test]
fn integer_and_boolean_inputs_handle_nonfinite_values_and_saturation() {
    for ty in ["i32", "i64", "bool"] {
        let mut run = build(&format!(
            "ins:\n  value: {ty}\nouts:\n  result: {ty}\nsample:\n  result = value\n"
        ));
        run.set_input_block(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0], 1);
        let expected = match ty {
            "i32" => [0.0, i32::MAX as f32, i32::MIN as f32, 0.0],
            "i64" => [0.0, i64::MAX as f32, i64::MIN as f32, 0.0],
            _ => [1.0, 1.0, 1.0, 0.0],
        };
        assert_eq!(run.render_block().unwrap()[0], expected);
    }
}

#[test]
fn typed_outputs_and_events_retain_precision_until_the_audio_boundary() {
    let mut run = build(
        r#"
outs:
  single: f32
  double: f64
  integer: i32
  wide: i64
  enabled: bool
init:
  fraction: f64 = 1.0000000000000002
  id: i64 = 9007199254740993
delegate reported(fraction: f64, id: i64)
event report():
  reported(fraction, id)
sample:
  single = -0.0
  double = fraction
  integer = 1065353216
  wide = id
  enabled = true
"#,
    );
    let rendered = run.render_block_segments(&[(0, 1, 1), (1, 3, 2)]).unwrap();
    assert_eq!(rendered[0][0].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(rendered[1], [1.0; 4]);
    assert_eq!(rendered[2], [1_065_353_216.0; 4]);
    assert_eq!(rendered[3], [9_007_199_254_740_993_i64 as f32; 4]);
    assert_eq!(rendered[4], [1.0; 4]);
    run.set_delegate_collection_enabled(true);
    run.trigger_event("report", &[]).unwrap();
    let batch = run.take_delegate_batch().unwrap();
    assert_eq!(
        batch.occurrences[0].values[0].value,
        RunEventValue::Number(1.0000000000000002)
    );
    assert_eq!(
        batch.occurrences[0].values[1].value,
        RunEventValue::I64(9_007_199_254_740_993)
    );
    run.restart().unwrap();
    assert_eq!(run.render_block().unwrap()[2], [1_065_353_216.0; 4]);
}
