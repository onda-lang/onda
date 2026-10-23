#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};

#[test]
fn tuple_stores_preserve_ranges_hooks_precision_and_rhs_evaluation() {
    let source =
        include_str!("../../../packages/onda_binaryen_web/test/fixtures/tuple-stores.onda");
    let typed = onda_semantics::analyze_with_options(
        onda_frontend::parse_program(source).unwrap(),
        onda_semantics::AnalysisOptions {
            sample_rate: 48000.0,
            block_size: 32,
        },
    )
    .unwrap();
    let program = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    // Catch a broken producer invariant before executing any unchecked access.
    let text = onda_mir::format_program(&program);
    assert!(
        !text
            .lines()
            .any(|line| line.trim_start().starts_with('@') && line.contains("= i32(1000)")),
        "{text}"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            program.clone(),
            MirCompileOptions {
                opt_level,
                ..Default::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut outputs = [[f32::NAN; 32]; 4];
        let pointers = outputs
            .iter_mut()
            .map(|output| output.as_mut_ptr().cast())
            .collect::<Vec<_>>();
        for _ in 0..3 {
            unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    32,
                    3,
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
            assert_eq!(outputs, [[0.0; 32]; 4], "{opt_level:?}");
        }
    }
}
