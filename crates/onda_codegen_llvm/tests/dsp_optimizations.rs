#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};

fn optimize(source: &str) -> (onda_mir::OptimizedProgram, onda_mir::PassStats) {
    let parsed = onda_frontend::parse_program(source).unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 32,
        },
    )
    .unwrap();
    onda_mir::optimize(onda_semantics::lower_program_to_validated_mir(&typed).unwrap()).unwrap()
}

#[test]
fn distribution_preserves_ordered_mixing_and_avoids_coupled_or_small_loops() {
    let source = include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/ordered-voice-rendering.onda"
    );
    let (mir, stats) = optimize(source);
    assert_eq!(
        optimize(&source.replace("Voices = 256", "Voices = 32"))
            .1
            .distributed_loops,
        1
    );
    assert_eq!(
        stats.distributed_loops,
        1,
        "{}",
        onda_mir::format_program(&mir)
    );
    assert_eq!(
        optimize(&source.replace("Voices = 256", "Voices = 16"))
            .1
            .distributed_loops,
        0
    );
    assert_eq!(
        optimize(&source.replace(
            "tone = sin(phase * TWO_PI)",
            "tone = sin(phase * TWO_PI) + mixed"
        ))
        .1
        .distributed_loops,
        0
    );
    assert_eq!(
        optimize(
            &source
                .replace("tone = sin(phase * TWO_PI)", "tone = helper(phase)")
                .replace(
                    "const Voices",
                    "def helper(phase):\n  return sin(phase * TWO_PI)\nconst Voices"
                )
        )
        .1
        .distributed_loops,
        0
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            mir.clone(),
            MirCompileOptions {
                opt_level,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut output = [f32::NAN; 32];
        for _ in 0..16 {
            unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    32,
                    3,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
                .unwrap();
            }
            assert_eq!(output, [0.0; 32], "{opt_level:?}");
        }
    }
}

#[test]
fn repeated_coefficients_preserve_sample_accurate_changes() {
    check_coefficient_fixture("coefficient-updates.onda");
}

#[test]
fn cached_panning_preserves_default_and_sample_accurate_changes() {
    check_coefficient_fixture("panning-updates.onda");
}

fn check_coefficient_fixture(name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/onda_binaryen_web/test/fixtures")
        .join(name);
    let loaded = onda_frontend::load_program_file(&path).unwrap();
    let typed = onda_semantics::analyze_with_options(
        loaded.program,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 32,
        },
    )
    .unwrap();
    let mir = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            mir.clone(),
            MirCompileOptions {
                opt_level,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut outputs = [[f32::NAN; 32]; 4];
        let pointers = outputs
            .iter_mut()
            .map(|channel| channel.as_mut_ptr().cast())
            .collect::<Vec<_>>();
        for _ in 0..16 {
            for (start, frames, flags) in [(0, 13, 1), (13, 19, 2)] {
                unsafe {
                    jit.process_checked(
                        &mut state,
                        &params,
                        start,
                        frames,
                        flags,
                        &[],
                        &pointers,
                        &[],
                        &[],
                        &[],
                        &[],
                        None,
                    )
                    .unwrap();
                }
            }
            assert_eq!(outputs, [[0.0; 32]; 4], "{opt_level:?}");
        }
    }
}
