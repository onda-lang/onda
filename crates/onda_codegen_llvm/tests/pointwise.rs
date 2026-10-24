#![cfg(feature = "llvm-orc")]
use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    MirCompileOptions,
};

fn compile(source: &str) -> onda_mir::OptimizedProgram {
    let parsed = onda_frontend::parse_program(source).unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48000.0,
            block_size: 17,
        },
    )
    .unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}

fn eligible_loops(program: &onda_mir::OptimizedProgram) -> usize {
    let function = &program.functions[program.entry_points.process.index()];
    let analysis = onda_mir::LoopAnalysis::new(
        program,
        program.effects(),
        program.memory(),
        program.ranges(),
        program.entry_points.process,
    );
    let mut blocks = vec![&function.body];
    let mut eligible = 0;
    while let Some(block) = blocks.pop() {
        for statement in &block.statements {
            match &statement.kind {
                onda_mir::StatementKind::Loop { body } => {
                    eligible += usize::from(analysis.pointwise_audio_loop(body).is_some());
                    blocks.push(body);
                }
                onda_mir::StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => blocks.extend([then_block, else_block]),
                _ => {}
            }
        }
    }
    eligible
}

#[test]
fn pointwise_audio_preserves_aliasing_tails_and_segments() {
    macro_rules! check {
        ($scalar:ty, $name:literal, $expression:literal) => {
            let program = compile(&format!(
                "def offset(x: {0}):\n  return x + 0.25\ndef scale(x: {0}, amount: {0}):\n  return offset(x * amount)\nins:\n  in1: {0}\nouts:\n  out1: {0}\nsample:\n  out1 = {1}\n",
                $name, $expression
            ));
            assert_eq!(eligible_loops(&program), 1);
            let ir =
                lower_optimized_mir_to_llvm_ir_with_options(&program, MirCompileOptions::default())
                    .unwrap();
            assert!(
                ir.contains("audio_equal"),
                "missing equal-pointer fast path: {ir}"
            );
            assert!(ir.contains("load <"), "no vector audio path: {ir}");
            assert!(!ir.contains("fadd reassoc") && !ir.contains("fmul contract"));
            let jit =
                jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default())
                    .unwrap();
            let params = jit.default_param_bytes();
            let mut state = jit.initialize_state(&params).unwrap();
            for output_base in [7, 8, 9, 40] {
                for segments in [
                    vec![(0, 17)],
                    vec![(0, 0), (0, 1), (1, 7), (8, 9)],
                    vec![(3, 8)],
                ] {
                    let mut values = [0.0 as $scalar; 64];
                    for (index, value) in values.iter_mut().enumerate() {
                        *value = (index as $scalar - 16.0) / 7.0;
                    }
                    values[8] = <$scalar>::NAN;
                    values[10] = <$scalar>::INFINITY;
                    values[11] = -0.0;
                    let mut expected = values;
                    for &(start, frames) in &segments {
                        for index in start..start + frames {
                            expected[output_base + index] = expected[8 + index] * 0.5 + 0.25;
                        }
                        unsafe {
                            jit.process_checked(
                                &mut state,
                                &params,
                                start,
                                frames,
                                3,
                                &[values.as_ptr().add(8).cast()],
                                &[values.as_mut_ptr().add(output_base).cast()],
                                &[],
                                &[],
                                &[],
                                &[],
                                None,
                            )
                            .unwrap();
                        }
                    }
                    for (actual, expected) in values.into_iter().zip(expected) {
                        assert!(
                            actual.to_bits() == expected.to_bits()
                                || actual.is_nan() && expected.is_nan(),
                            "{} output={output_base}, {segments:?}: {actual} != {expected}",
                            $name
                        );
                    }
                }
            }
        };
    }
    check!(f32, "f32", "in1 * 0.5 + 0.25");
    check!(f64, "f64", "in1 * 0.5 + 0.25");
    check!(f32, "f32", "scale(in1, 0.5)");
    check!(f64, "f64", "scale(in1, 0.5)");
}

#[test]
fn a_sample_recurrence_is_not_claimed_independent() {
    let program = compile("ins 1\ninit:\n  previous = 0.0\nsample:\n  previous = previous * 0.5 + in1\n  out1 = previous\n");
    assert_eq!(eligible_loops(&program), 0);
}

#[test]
fn helpers_with_failures_or_unproven_termination_are_not_independent() {
    for helper in [
        "def helper(x: f32):\n  return f32(12 / i32(x))\n",
        "def helper(x: f32):\n  result = x\n  while result > 1.0:\n    result *= 0.5\n  return result\n",
    ] {
        let program = compile(&format!("{helper}ins 1\nsample:\n  out1 = helper(in1)\n"));
        assert_eq!(eligible_loops(&program), 0, "{helper}");
    }
}

#[test]
fn helpers_with_memory_effects_are_not_claimed_independent() {
    for source in [
        "def helper(x: f32, weights: f32[]):\n  return x * weights[0]\ninit:\n  weights: f32[2] = [0.5, 0.25]\nsample:\n  out1 = helper(in1, weights)\n",
        "proc Feedback:\n  ins 1\n  init:\n    previous = 0.0\n  sample:\n    previous = previous * 0.5 + in1\n    out1 = previous\ninit:\n  feedback = Feedback()\nsample:\n  out1 = feedback(in1)\n",
    ] {
        assert_eq!(eligible_loops(&compile(source)), 0, "{source}");
    }
}

#[test]
fn pointwise_audio_checks_all_input_and_output_pairs() {
    let program = compile("ins 2\nouts 2\nsample:\n  first = in1 * 0.5 + 0.25\n  out1 = first\n  out2 = in2 * 0.75 + first\n");
    assert_eq!(eligible_loops(&program), 1);
    let jit =
        jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default()).unwrap();
    let params = jit.default_param_bytes();
    let mut state = jit.initialize_state(&params).unwrap();
    for (inputs, outputs) in [
        ([8, 80], [8, 80]),
        ([8, 80], [80, 8]),
        ([8, 80], [40, 40]),
        ([8, 80], [9, 40]),
        ([8, 80], [40, 41]),
        ([8, 8], [8, 40]),
    ] {
        for (start, frames) in [(0, 17), (3, 8)] {
            let mut values = std::array::from_fn::<_, 128, _>(|i| (i as f32 - 64.0) / 17.0);
            let mut expected = values;
            for frame in start..start + frames {
                let first = expected[inputs[0] + frame] * 0.5 + 0.25;
                let second = expected[inputs[1] + frame] * 0.75 + first;
                expected[outputs[0] + frame] = first;
                expected[outputs[1] + frame] = second;
            }
            unsafe {
                let input = inputs.map(|offset| values.as_ptr().add(offset).cast());
                let output = outputs.map(|offset| values.as_mut_ptr().add(offset).cast());
                jit.process_checked(
                    &mut state,
                    &params,
                    start,
                    frames,
                    3,
                    &input,
                    &output,
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
                .unwrap();
            }
            assert_eq!(
                values.map(f32::to_bits),
                expected.map(f32::to_bits),
                "{inputs:?} -> {outputs:?}, {start}+{frames}"
            );
        }
    }
}
