use onda_codegen_llvm::jit_program_from_optimized_mir;
use onda_frontend::parse_program;
use onda_runtime::*;
use onda_semantics::{analyze_with_options, lower_program_to_optimized_mir, AnalysisOptions};

const FRAMES: usize = 8;

fn instance(source: &str, _inputs: usize) -> Instance {
    let typed = analyze_with_options(
        parse_program(source).unwrap(),
        AnalysisOptions {
            sample_rate: 1000.0,
            block_size: FRAMES,
        },
    )
    .unwrap_or_else(|errors| panic!("{source}\n{errors:?}"));
    let mir = lower_program_to_optimized_mir(&typed).unwrap();
    let program = jit_program_from_optimized_mir(mir).unwrap();
    (|| -> Result<_, onda_frontend::Diagnostic> {
        let mut instance = onda_runtime::create_instance(program, None)?;
        onda_codegen_llvm::check_execution_status(onda_runtime::init_checked(
            &mut instance,
            onda_runtime::InitMode::Full,
            onda_runtime::ExecutionOutput::none(),
        )?)?;
        Ok(instance)
    })()
    .unwrap()
}

fn render(source: &str, input: &[f32; FRAMES]) -> [f32; FRAMES] {
    let mut instance = instance(source, 1);
    let mut output = [0.0; FRAMES];
    unsafe {
        bind_input(&mut instance, 0, input.as_ptr().cast(), size_of_val(input)).unwrap();
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            size_of_val(&output),
        )
        .unwrap();
    }
    onda_runtime::process_checked_segment(
        &mut instance,
        0,
        FRAMES,
        onda_runtime::PROCESSOR_FULL_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    output
}

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-5,
            "sample {index}: {actual} != {expected}"
        );
    }
}

#[test]
fn ramps_advance_once_before_reads_at_each_owner_boundary() {
    for source in [
        "ins:\n  x {ramp = 0.004}\nsample:\n  out1 = x + x + ins[0]\n",
        "proc Main:\n  ins:\n    x {ramp = 0.004}\n  sample:\n    out1 = x + x + ins[0]\n",
        "proc Voice<T>:\n  ins<T>:\n    x {ramp = 0.004}\n  sample:\n    out1 = f32(x + x + ins[0])\nproc Parent:\n  ins 1\n  init:\n    voice = Voice<f64>()\n  sample:\n    out1 = voice(in1)\ninit:\n  parent = Parent()\nsample:\n  out1 = parent(in1)\n",
    ] {
        assert_close(&render(source, &[1.0; FRAMES]), &[0.75, 1.5, 2.25, 3.0, 3.0, 3.0, 3.0, 3.0]);
    }
}

#[test]
fn lag_matches_its_time_constant_and_clamps_targets_first() {
    for source in [
        "ins:\n  x {0, 1, lag = 0.001}\nsample:\n  out1 = x\n",
        "proc Main:\n  ins:\n    x {0, 1, lag = 0.001}\n  sample:\n    out1 = x\n",
    ] {
        let expected: Vec<_> = (1..=FRAMES).map(|n| 1.0 - (-(n as f32)).exp()).collect();
        assert_close(&render(source, &[2.0; FRAMES]), &expected);
    }
}

#[test]
fn defaults_seed_smoothers_without_a_startup_fade() {
    for attribute in ["ramp", "lag"] {
        for source in [
            format!("ins:\n  x = 0.5 {{{attribute} = 0.004}}\nsample:\n  out1 = x\n"),
            format!("proc Main:\n  ins:\n    x = 0.5 {{{attribute} = 0.004}}\n  sample:\n    out1 = x\n"),
        ] {
            assert_close(&render(&source, &[0.5; FRAMES]), &[0.5; FRAMES]);
        }
    }
}

#[test]
fn array_elements_have_independent_state_and_all_reads_use_the_cache() {
    let source = "ins:\n  x: f64[2] = [0.0, 1.0] {ramp = 0.004}\nsample:\n  out1 = f32(x[0] + 10.0 * x[1] + 100.0 * (ins[0] - x.read_unsafe(0)) + 100.0 * (ins[1] - x[1]))\n";
    let mut instance = instance(source, 2);
    let mut input = vec![0.0_f64; 2 * FRAMES];
    input[..FRAMES].fill(1.0);
    let mut output = [0.0; FRAMES];
    unsafe {
        bind_input(
            &mut instance,
            0,
            input.as_ptr().cast(),
            size_of_val(input.as_slice()),
        )
        .unwrap();
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            size_of_val(&output),
        )
        .unwrap();
    }
    onda_runtime::process_checked_segment(
        &mut instance,
        0,
        FRAMES,
        onda_runtime::PROCESSOR_FULL_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    assert_close(&output, &[7.75, 5.5, 3.25, 1.0, 1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn segments_snapshots_and_reinitialization_preserve_the_smoothing_lifecycle() {
    let mut instance = instance("ins:\n  x {ramp = 0.004}\nsample:\n  out1 = x\n", 1);
    let input = [1.0_f32; FRAMES];
    let mut output = [0.0; FRAMES];
    unsafe {
        bind_input(&mut instance, 0, input.as_ptr().cast(), size_of_val(&input)).unwrap();
        bind_output(
            &mut instance,
            0,
            output.as_mut_ptr().cast(),
            size_of_val(&output),
        )
        .unwrap();
    }
    process_checked_segment(
        &mut instance,
        0,
        0,
        PROCESSOR_BEGIN_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    process_checked_segment(&mut instance, 0, 2, 0, ExecutionOutput::none()).unwrap();
    let snapshot = instance.snapshot_state_bytes().unwrap();
    process_checked_segment(
        &mut instance,
        2,
        6,
        PROCESSOR_END_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    assert_close(&output, &[0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0]);
    instance.restore_state_bytes(&snapshot).unwrap();
    process_checked_segment(
        &mut instance,
        2,
        6,
        PROCESSOR_END_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    assert_close(&output[2..], &[0.75, 1.0, 1.0, 1.0, 1.0, 1.0]);
    init_checked(
        &mut instance,
        InitMode::Full,
        onda_runtime::ExecutionOutput::none(),
    )
    .unwrap();
    onda_runtime::process_checked_segment(
        &mut instance,
        0,
        FRAMES,
        onda_runtime::PROCESSOR_FULL_BLOCK,
        ExecutionOutput::none(),
    )
    .unwrap();
    assert_close(&output, &[0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn oversampling_matches_an_explicit_stdlib_smoother() {
    for (attribute, processor) in [("ramp", "Ramp"), ("lag", "Lag")] {
        let implicit = format!("ins:\n  x = 0.5 {{{attribute} = 0.004}}\nsample 4:\n  out1 = x\n");
        let explicit = format!("import std/smoothing\nins:\n  x = 0.5\ninit:\n  smoother = std::smoothing::{processor}(time_s = 0.004)\n  smoother.reset(0.5)\nsample 4:\n  out1 = smoother(x)\n");
        let input = [0.5, 1.0, 1.0, 0.0, 0.0, 0.0, 0.25, 0.25];
        assert_close(&render(&implicit, &input), &render(&explicit, &input));
    }
}

#[test]
fn zero_smoothing_creates_no_additional_state() {
    for proc in [false, true] {
        let source = |attribute: &str| {
            if proc {
                format!("proc Main:\n  ins:\n    x {attribute}\n  sample:\n    out1 = x\n")
            } else {
                format!("ins:\n  x {attribute}\nsample:\n  out1 = x\n")
            }
        };
        let plain = instance(&source(""), 1).snapshot_state_bytes().unwrap();
        for attribute in ["{ramp = 0}", "{lag = 0}"] {
            assert_eq!(
                instance(&source(attribute), 1)
                    .snapshot_state_bytes()
                    .unwrap(),
                plain
            );
        }
    }
}

#[test]
fn proc_arrays_have_independent_smoothers_and_each_call_advances_once() {
    let source = "proc Voice:\n  ins:\n    x {ramp = 0.004}\n  sample:\n    out1 = x\ninit:\n  voices: Voice[2]\nsample:\n  out1 = voices[0](in1) + voices[0](in1) + 10.0 * voices[1](in1 * 0.5)\n";
    assert_close(
        &render(source, &[1.0; FRAMES]),
        &[2.0, 4.25, 5.75, 7.0, 7.0, 7.0, 7.0, 7.0],
    );
}
