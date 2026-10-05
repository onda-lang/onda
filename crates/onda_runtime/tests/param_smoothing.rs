use onda_codegen_llvm::{jit_program_from_optimized_mir_with_options, MirCompileOptions};
use onda_frontend::parse_program;
use onda_runtime::*;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

struct Processor {
    instance: Instance,
    outputs: Vec<Box<[f32]>>,
}

impl Processor {
    fn new(source: &str, block_size: usize, fast_math: bool) -> Self {
        let typed = analyze_with_options(
            parse_program(source).unwrap(),
            AnalysisOptions {
                sample_rate: 48_000.0,
                block_size,
                ..AnalysisOptions::default()
            },
        )
        .unwrap();
        let program = jit_program_from_optimized_mir_with_options(
            lower_program_to_optimized_mir(&typed).unwrap(),
            MirCompileOptions {
                fast_math,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        let channels = program.output_count();
        let mut instance = create_instance_initialized(
            program,
            InstanceConfig {
                sample_rate: 48_000.0,
                frames_per_block: block_size,
                in_channels: 0,
                out_channels: channels,
            },
        )
        .unwrap();
        let mut outputs = (0..channels)
            .map(|_| vec![0.0; block_size].into_boxed_slice())
            .collect::<Vec<_>>();
        for (index, output) in outputs.iter_mut().enumerate() {
            unsafe {
                bind_output(
                    &mut instance,
                    index,
                    output.as_mut_ptr().cast(),
                    output.len() * 4,
                )
                .unwrap();
            }
        }
        Self { instance, outputs }
    }

    fn target(&mut self, value: f64) {
        set_param_element_plain_f64(&mut self.instance, 0, 0, value).unwrap();
    }

    fn segment(&mut self, start: usize, frames: usize, flags: u32) {
        process_checked_segment(
            &mut self.instance,
            start,
            frames,
            flags,
            ExecutionOutput::none(),
        )
        .unwrap();
    }

    fn block(&mut self, frames: usize) -> f32 {
        self.segment(0, frames, PROCESSOR_FULL_BLOCK);
        self.outputs[0][0]
    }
}

fn source(ty: &str, array_len: Option<usize>, seconds: f64) -> String {
    let suffix = array_len.map(|len| format!("[{len}]")).unwrap_or_default();
    let (argument_ty, read) = if array_len.is_some() {
        (format!("{ty}[]"), "value[0]")
    } else {
        (ty.to_owned(), "value")
    };
    format!("params:\n  gain: {ty}{suffix} = 0.0 {{0, 1, smooth = {seconds:.18}}}\ndef read_gain(value: {argument_ty}):\n  return {read}\nblock:\n  cached = read_gain(gain)\n  sample:\n    out1 = f32(read_gain(gain))\n    out2 = f32(cached)\n")
}

fn assert_value(actual: f32, expected: f32) {
    assert!((actual - expected).abs() <= 2e-7, "{actual} != {expected}");
}

#[test]
fn linear_ramps_are_block_rate_and_finish_at_the_next_boundary() {
    for fast_math in [false, true] {
        for ty in ["f32", "f64"] {
            for len in [None, Some(1), Some(3)] {
                let mut dsp = Processor::new(&source(ty, len, 0.002), 64, fast_math);
                dsp.target(1.0);
                for expected in [0.0, 64.0 / 96.0, 1.0, 1.0] {
                    // Repeated writes must not restart an in-progress ramp.
                    dsp.target(1.0);
                    assert_value(dsp.block(64), expected);
                    for output in &dsp.outputs {
                        assert!(output.iter().all(|value| *value == dsp.outputs[0][0]));
                    }
                }
            }
        }
    }
}

#[test]
fn segments_17_and_47_match_full_blocks_exactly() {
    for ty in ["f32", "f64"] {
        for fast_math in [false, true] {
            let source = source(ty, Some(2), 0.02);
            let mut full = Processor::new(&source, 64, fast_math);
            let mut split = Processor::new(&source, 64, fast_math);
            full.target(1.0);
            split.target(1.0);
            for _ in 0..17 {
                full.block(64);
                split.segment(0, 17, PROCESSOR_BEGIN_BLOCK);
                split.segment(17, 0, 0);
                split.segment(17, 47, PROCESSOR_END_BLOCK);
                assert_eq!(full.outputs, split.outputs);
                assert_eq!(
                    full.instance.snapshot_state_bytes().unwrap(),
                    split.instance.snapshot_state_bytes().unwrap()
                );
            }
        }
    }
}

#[test]
fn partial_blocks_count_actual_samples_and_zero_frames_do_not_advance() {
    let mut dsp = Processor::new(&source("f32", None, 0.02), 64, false);
    dsp.target(1.0);
    dsp.block(0);
    assert_eq!(dsp.block(17), 0.0);
    assert_value(dsp.block(47), 17.0 / 960.0);
    let snapshot = dsp.instance.snapshot_state_bytes().unwrap();
    dsp.block(0); // Publish the value reached after 64 samples.
    let published = dsp.instance.snapshot_state_bytes().unwrap();
    assert_ne!(published, snapshot);
    dsp.block(0);
    assert_eq!(dsp.instance.snapshot_state_bytes().unwrap(), published);
    assert_value(dsp.block(1), 64.0 / 960.0);
}

#[test]
fn retargeting_uses_the_value_reached_at_the_boundary() {
    for ty in ["f32", "f64"] {
        let mut dsp = Processor::new(&source(ty, None, 0.002), 24, false);
        dsp.target(1.0);
        assert_eq!(dsp.block(24), 0.0);
        assert_eq!(dsp.block(24), 0.25);
        dsp.target(0.0);
        assert_eq!(dsp.block(24), 0.5);
        for expected in [0.375, 0.25, 0.125, 0.0] {
            assert_eq!(dsp.block(24), expected);
        }
    }
}

#[test]
fn target_changes_inside_a_block_wait_for_the_next_boundary() {
    let mut dsp = Processor::new(&source("f64", Some(2), 0.002), 64, false);
    dsp.target(1.0);
    assert_eq!(dsp.block(64), 0.0);
    dsp.segment(0, 17, PROCESSOR_BEGIN_BLOCK);
    let first = dsp.outputs[0][0];
    dsp.target(0.0);
    dsp.segment(17, 47, PROCESSOR_END_BLOCK);
    assert!(dsp
        .outputs
        .iter()
        .all(|output| output.iter().all(|value| *value == first)));
    assert_eq!(dsp.block(64), 1.0); // Complete the old ramp before retargeting.
    assert_value(dsp.block(64), 1.0 / 3.0);
    assert_eq!(dsp.block(64), 0.0);
}

#[test]
fn snapshots_preserve_pending_samples_and_ramp_progress() {
    // No authored block locals: this exercises only persistent smoothing state.
    let source = "params:\n  gain: f64[2] = 0.0 {0, 1, smooth = 0.02}\nsample:\n  out1 = f32(gain[0])\n  out2 = f32(gain[1])\n";
    let mut dsp = Processor::new(source, 64, false);
    dsp.target(1.0);
    dsp.segment(0, 17, PROCESSOR_BEGIN_BLOCK);
    let snapshot = dsp.instance.snapshot_state_bytes().unwrap();
    dsp.segment(17, 47, PROCESSOR_END_BLOCK);
    dsp.block(64);
    let expected_outputs = dsp.outputs.clone();
    let expected_snapshot = dsp.instance.snapshot_state_bytes().unwrap();
    for _ in 0..5 {
        dsp.block(64);
    }
    dsp.instance.restore_state_bytes(&snapshot).unwrap();
    dsp.segment(17, 47, PROCESSOR_END_BLOCK);
    dsp.block(64);
    assert_eq!(dsp.outputs, expected_outputs);
    assert_eq!(
        dsp.instance.snapshot_state_bytes().unwrap(),
        expected_snapshot
    );
    assert!(dsp.outputs[1].iter().all(|value| *value == 0.0));
    init_checked(&mut dsp.instance, InitMode::Full).unwrap();
    assert_eq!(dsp.block(64), 1.0);
}

#[test]
fn sub_sample_durations_take_one_sample_and_publish_at_a_boundary() {
    let mut dsp = Processor::new(&source("f32", None, 0.000001), 64, false);
    dsp.target(1.0);
    assert_eq!(dsp.block(1), 0.0);
    assert_eq!(dsp.block(1), 1.0);
}

#[test]
fn long_f32_ramps_do_not_accumulate_rounding_or_stall() {
    for fast_math in [false, true] {
        let source = "params:\n  gain: f32 = 1.0 {1.0, 2.0, smooth = 26.666666666666668}\nsample:\n  out1 = gain\n";
        let mut dsp = Processor::new(source, 64, fast_math);
        dsp.target(1.001);
        dsp.block(0);
        for _ in 0..10_000 {
            dsp.block(64);
        }
        // Publish at the midpoint, without processing more samples.
        dsp.block(0);
        assert_eq!(dsp.block(0), 1.0005_f32);
        for _ in 0..10_000 {
            dsp.block(64);
        }
        dsp.block(0);
        assert_eq!(dsp.block(0), 1.001_f32);
    }
}

#[test]
fn events_and_block_end_reads_share_the_published_value() {
    let source = "params:\n  gain: f32[2] = 0.0 {0, 1, smooth = 0.002}\ninit:\n  captured = gain[0]\n  ending = gain[0]\nevent capture():\n  captured = gain[0]\nblock:\n  cached = gain[0]\n  sample:\n    out1 = gain[0]\n    out2 = cached\n    out3 = captured\n    out4 = ending\n  ending = gain[0]\n";
    let mut dsp = Processor::new(source, 24, false);
    dsp.target(1.0);
    dsp.block(24);
    dsp.segment(0, 17, PROCESSOR_BEGIN_BLOCK);
    dsp.target(0.0);
    trigger_event_by_index_checked(&mut dsp.instance, 0, &[], ExecutionOutput::none()).unwrap();
    dsp.segment(17, 7, PROCESSOR_END_BLOCK);
    assert!(dsp.outputs[0].iter().all(|value| *value == 0.25));
    assert_eq!(dsp.outputs[0], dsp.outputs[1]);
    assert!(dsp.outputs[2][17..].iter().all(|value| *value == 0.25));
    assert_eq!(dsp.block(24), 0.5);
    assert!(dsp.outputs[2].iter().all(|value| *value == 0.25));
    assert!(dsp.outputs[3].iter().all(|value| *value == 0.25));
}

#[test]
fn very_long_durations_and_repeated_segments_keep_sample_counts_bounded() {
    let long_source = "params:\n  gain: f64 = 0.0 {0, 1, smooth = 100000000000000.0}\nsample:\n  out1 = f32(gain)\n";
    let mut dsp = Processor::new(long_source, 64, false);
    dsp.target(1.0);
    dsp.block(64);
    assert_eq!(dsp.block(64), (64.0 / 4_800_000_000_000_000_000.0) as f32);

    let mut short = Processor::new(&source("f32", None, 0.002), 64, false);
    short.target(1.0);
    short.block(64);
    for _ in 0..100 {
        short.segment(0, 64, 0);
    }
    // Accumulated time can saturate at the longest duration; more time does
    // not change the result or affect the effective value inside a block.
    assert_eq!(short.outputs[0][0], 0.0);
    assert_eq!(short.block(1), 1.0);
}

#[test]
fn parameters_and_array_elements_keep_independent_ramps_and_durations() {
    let source = "params:\n  gains: f32[2] = 0.0 {0, 1, smooth = 0.002}\n  slow: f64 = 0.0 {0, 1, smooth = 0.004}\nsample:\n  out1 = gains[0]\n  out2 = gains[1]\n  out3 = f32(slow)\n";
    let mut dsp = Processor::new(source, 48, false);
    dsp.target(1.0);
    set_param_plain_f64(&mut dsp.instance, 1, 1.0).unwrap();
    dsp.block(48);
    assert!(dsp
        .outputs
        .iter()
        .all(|output| output.iter().all(|value| *value == 0.0)));
    set_param_element_plain_f64(&mut dsp.instance, 0, 1, 0.5).unwrap();
    for expected in [
        [0.5, 0.0, 0.25],
        [1.0, 0.25, 0.5],
        [1.0, 0.5, 0.75],
        [1.0, 0.5, 1.0],
    ] {
        dsp.block(48);
        for (output, value) in dsp.outputs.iter().zip(expected) {
            assert!(output.iter().all(|sample| *sample == value));
        }
    }
}
