#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};

#[test]
fn guarded_arrays_preserve_empty_ranges_mutation_and_wrapping() {
    let parsed = onda_frontend::parse_program(include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/guarded-array-access.onda"
    ))
    .unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 32,
        },
    )
    .unwrap();
    let mir = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    let reverse = mir
        .functions
        .iter()
        .find(|f| f.name.starts_with("reverse"))
        .unwrap();
    let mut selected = mir.as_program().clone();
    selected.functions = vec![reverse.clone()];
    let text = onda_mir::format_program(&selected);
    let accesses: Vec<_> = text
        .lines()
        .filter(|line| line.contains("[%") && line.contains("@p"))
        .collect();
    assert_eq!(accesses.len(), 2, "{text}");
    assert!(
        accesses.iter().all(|line| line.contains("unchecked")),
        "{text}"
    );

    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            mir.clone(),
            MirCompileOptions {
                opt_level,
                ..Default::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut outputs = [[f32::NAN; 32]; 3];
        let pointers = outputs
            .iter_mut()
            .map(|output| output.as_mut_ptr().cast())
            .collect::<Vec<_>>();
        for _ in 0..8 {
            for (start, frames, flags) in [(0, 0, 1), (0, 13, 0), (13, 19, 2)] {
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
            assert_eq!(outputs, [[0.0; 32]; 3], "{opt_level:?}");
        }
    }
}
