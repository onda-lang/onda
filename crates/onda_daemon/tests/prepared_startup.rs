use onda_daemon::{AnalysisSession, DocumentVersion, RunOptions, RunSession};
use onda_project::{BufferAsset, BufferSamples};

fn prepare(source: &str) -> RunSession {
    let path = std::env::temp_dir().join("onda_prepared_startup.onda");
    let mut analysis = AnalysisSession::default();
    analysis.open_document(&path, DocumentVersion(1), source.to_owned());
    RunSession::prepare_with_inputs_and_initial_buffers(
        &analysis,
        path,
        RunOptions {
            block_size: 4,
            param_smoothing_seconds: 10.0,
            ..Default::default()
        },
        &Default::default(),
        [],
    )
    .unwrap()
}

#[test]
fn prepared_runs_restore_controls_before_initialization_and_reject_early_execution() {
    let mut run = prepare(
        r#"params:
  divisor: i32 = 0
  gain: f64 = 0.0
init:
  value = f64(i32(1) / divisor) * gain
event inspect():
  value += 1.0
sample:
  out1 = f32(value)
  out2 = f32(gain)
"#,
    );
    assert_eq!(run.param_info().len(), 2);
    assert_eq!(run.event_info().len(), 1);
    assert!(run.render_block().is_err());
    assert!(run.trigger_event("inspect", &[]).is_err());
    // A failed initialization remains recoverable without recompiling.
    assert!(run.restart().is_err());
    assert!(run.set_param_f64("unknown", 1.0).is_err());
    run.set_param_f64("divisor", 1.0).unwrap();
    run.set_param_f64("gain", 0.75).unwrap();
    run.restart().unwrap();
    assert_eq!(run.render_block().unwrap(), vec![vec![0.75; 4]; 2]);
}

#[test]
fn restoring_multiple_buffers_never_initializes_partial_bindings() {
    let mut run = prepare(
        r#"buffers:
  first: buffer<i32>
  second: buffer<i32>
init:
  print("initialized")
  value = i32(6) / first[0] + i32(6) / second[0]
sample:
  out1 = f32(value)
"#,
    );
    for (name, value) in [("first", 2), ("second", 3)] {
        let asset = BufferAsset::new(1, 1, 48_000.0, BufferSamples::I32(vec![value])).unwrap();
        run.bind_buffer_asset(name, asset).unwrap();
        assert!(run.take_print_batch().unwrap().entries.is_empty());
        assert!(run.render_block().is_err());
    }
    // Clearing and rebinding during preparation must also avoid executing init.
    run.clear_buffer("first").unwrap();
    assert!(run.restart().is_err());
    run.bind_buffer_asset(
        "first",
        BufferAsset::new(1, 1, 48_000.0, BufferSamples::I32(vec![2])).unwrap(),
    )
    .unwrap();
    run.restart().unwrap();
    assert_eq!(run.take_print_batch().unwrap().entries.len(), 1);
    assert_eq!(run.render_block().unwrap()[0], vec![5.0; 4]);
}

#[test]
fn restoring_a_snapshot_completes_preparation() {
    let source = "buffers:\n  clip: buffer<i32>\ninit:\n  value = 1\nsample:\n  out1 = f32(value + clip[0])\n";
    let mut original = prepare(source);
    original.restart().unwrap();
    let snapshot = original.snapshot_state_bytes().unwrap();
    let mut restored = prepare(source);
    restored.restore_state_bytes(&snapshot).unwrap();
    restored
        .bind_buffer_asset(
            "clip",
            BufferAsset::new(1, 1, 48_000.0, BufferSamples::I32(vec![2])).unwrap(),
        )
        .unwrap();
    assert_eq!(restored.render_block().unwrap()[0], vec![3.0; 4]);
}
