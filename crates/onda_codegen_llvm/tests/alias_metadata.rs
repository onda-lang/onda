#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    lower_optimized_mir_to_llvm_ir_with_options, MirCompileOptions, TargetOptLevel,
};

fn scalar_states(count: usize) -> onda_mir::OptimizedProgram {
    use std::fmt::Write;

    let mut source = String::from("ins 1\ninit:\n");
    for index in 0..count {
        writeln!(source, "  x{index} = 0.0").unwrap();
    }
    source.push_str("sample:\n");
    for index in 0..count {
        writeln!(source, "  x{index} = x{index} * 0.99 + in1").unwrap();
    }
    source.push_str("  out1 = ");
    for index in 0..count {
        if index > 0 {
            source.push_str(" + ");
        }
        write!(source, "x{index}").unwrap();
    }
    source.push('\n');
    let typed = onda_semantics::analyze(onda_frontend::parse_program(&source).unwrap()).unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}

#[test]
fn state_alias_metadata_does_not_grow_quadratically() {
    let operands = |count| {
        let ir = lower_optimized_mir_to_llvm_ir_with_options(
            &scalar_states(count),
            MirCompileOptions {
                opt_level: TargetOptLevel::O0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(ir.contains("!alias.scope !") && ir.contains("!noalias !"));
        ir.lines()
            .filter(|line| line.starts_with('!'))
            .map(|line| line.matches('!').count())
            .sum::<usize>()
    };
    let small = operands(128);
    let large = operands(256);
    assert!(large < 3 * small, "metadata operands: {small} -> {large}");
}
