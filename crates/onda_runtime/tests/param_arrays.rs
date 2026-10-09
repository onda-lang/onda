use onda_codegen_llvm::jit_program_from_optimized_mir;
use onda_frontend::parse_program;
use onda_runtime::*;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

fn instance(source: &str) -> Instance {
    let typed = analyze_with_options(
        parse_program(source).unwrap(),
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 4,
        },
    )
    .unwrap();
    let program =
        jit_program_from_optimized_mir(lower_program_to_optimized_mir(&typed).unwrap()).unwrap();
    create_instance_initialized(
        program,
        InstanceConfig {
            sample_rate: 48_000.0,
            frames_per_block: 4,
            in_channels: 0,
            out_channels: 1,
        },
    )
    .unwrap()
}

#[test]
fn array_elements_support_raw_plain_and_normalized_host_writes() {
    let mut instance = instance(
        r#"
params:
  gains: f32[2] = [1.0, 1.0] {0.0, 4.0}
sample:
  out1 = gains[0] + 10.0 * gains[1]
"#,
    );
    let mut output = [0.0_f32; 4];
    unsafe {
        bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 16).unwrap();
    }

    set_param_element_plain_f64(&mut instance, 0, 0, 9.0).unwrap();
    set_param_element_normalized(&mut instance, 0, 1, 0.5).unwrap();
    process_checked(&mut instance, 4, ExecutionOutput::none()).unwrap();
    assert_eq!(output, [24.0; 4]);

    set_param_element_by_index(&mut instance, 0, 0, &3.0_f32.to_ne_bytes()).unwrap();
    process_checked(&mut instance, 4, ExecutionOutput::none()).unwrap();
    assert_eq!(output, [23.0; 4]);
    assert!(set_param_element_plain_f64(&mut instance, 0, 2, 1.0).is_err());
    assert!(set_param_element_normalized(&mut instance, 0, 2, 1.0).is_err());
}

#[test]
fn event_array_reads_preserve_process_snapshot_across_segments() {
    for read in [
        "gains[0]",
        "first(gains)",
        "first(gains[0:2])",
        "forward(gains)",
    ] {
        let mut instance = instance(&format!("params:\n  gains: f32[2] = 0.25 {{0, 1}}\ndef first(values: f32[]):\n  return values[0]\ndef forward(values: f32[]):\n  return first(values)\ninit:\n  captured = 0.0\nevent capture():\n  captured = {read}\nsample:\n  out1 = {read} + captured\n"));
        let mut output = [0.0_f32; 4];
        unsafe {
            bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 16).unwrap();
        }
        process_checked_segment(
            &mut instance,
            0,
            2,
            PROCESSOR_BEGIN_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        set_param_element_by_index(&mut instance, 0, 0, &2.0_f32.to_ne_bytes()).unwrap();
        trigger_event_by_index_checked(&mut instance, 0, &[], ExecutionOutput::none()).unwrap();
        process_checked_segment(
            &mut instance,
            2,
            2,
            PROCESSOR_END_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(output, [0.25, 0.25, 1.25, 1.25], "{read}");
        process_checked_segment(
            &mut instance,
            0,
            4,
            PROCESSOR_BEGIN_BLOCK | PROCESSOR_END_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(output, [2.0; 4], "{read}");
    }
}

#[test]
fn proc_array_ranges_clamp_defaults_and_writes() {
    for ty in ["f32", "f64", "i32", "i64"] {
        let mut instance = instance(&format!(
            r#"
proc Voice:
  params:
    gains: {ty}[2] = [-2, 8] {{0, 4}}
  event change():
    gains[0] = {ty}(9)
    gains[1] = {ty}(-1)
  sample:
    out1 = f32(gains[0]) + 10.0 * f32(gains[1])
init:
  voice = Voice()
event change():
  voice.change()
sample:
  out1 = voice()
"#
        ));
        let mut output = [0.0_f32; 4];
        unsafe {
            bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 16).unwrap();
        }
        process_checked_segment(
            &mut instance,
            0,
            4,
            PROCESSOR_BEGIN_BLOCK | PROCESSOR_END_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(output, [40.0; 4], "{ty} defaults");
        trigger_event_by_index_checked(&mut instance, 0, &[], ExecutionOutput::none()).unwrap();
        process_checked_segment(
            &mut instance,
            0,
            4,
            PROCESSOR_BEGIN_BLOCK | PROCESSOR_END_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(output, [4.0; 4], "{ty} writes");
    }
}

#[test]
fn generic_proc_array_ranges_clamp_constructor_call_and_dynamic_writes() {
    let mut instance = instance(
        r#"
proc Voice<T>:
  params:
    gains: T[2] = 8 {0, 4}
  event change(index: i32, value: T):
    gains[index] = value
  sample:
    out1 = f32(gains[0]) + 10.0 * f32(gains[1])
init:
  voice = Voice<f64>(gains = [-5.0, 9.0])
  mode: i32 = 0
event change():
  voice.change(1, -8.0)
event named():
  mode = 1
sample:
  if mode == 0:
    out1 = voice()
  else:
    out1 = voice(gains = [9.0, -2.0])
"#,
    );
    let mut output = [0.0_f32; 4];
    unsafe {
        bind_output(&mut instance, 0, output.as_mut_ptr().cast(), 16).unwrap();
    }
    for (event, expected) in [(None, 40.0), (Some(0), 0.0), (Some(1), 4.0)] {
        if let Some(event) = event {
            trigger_event_by_index_checked(&mut instance, event, &[], ExecutionOutput::none())
                .unwrap();
        }
        process_checked_segment(
            &mut instance,
            0,
            4,
            PROCESSOR_BEGIN_BLOCK | PROCESSOR_END_BLOCK,
            ExecutionOutput::none(),
        )
        .unwrap();
        assert_eq!(output, [expected; 4]);
    }
}
