#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    lower_optimized_mir_to_llvm_ir, lower_optimized_mir_to_target_llvm_ir, MirTargetOptions,
};
use std::sync::Barrier;

// Keep this in its own test executable: LLVM's process-global registry must be
// uninitialized when native and explicit-target compilation first race.
#[test]
fn concurrent_native_and_explicit_target_initialization() {
    let parsed = onda_frontend::parse_program("ins 1\nsample:\n  out1 = in1 * 0.7\n").unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 128,
        },
    )
    .unwrap();
    let mir = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    let start = Barrier::new(8);
    std::thread::scope(|threads| {
        for index in 0..8 {
            let (mir, start) = (&mir, &start);
            threads.spawn(move || {
                start.wait();
                let ir = if index % 2 == 0 {
                    lower_optimized_mir_to_llvm_ir(mir)
                } else {
                    lower_optimized_mir_to_target_llvm_ir(mir, &MirTargetOptions::default())
                }
                .unwrap();
                assert!(ir.contains("define "));
            });
        }
    });
}
