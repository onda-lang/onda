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
            sample_rate: 48_000.0,
            block_size: 48,
        },
    )
    .unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}

#[test]
fn overlapping_views_observe_mutation_across_calls_and_loops() {
    let program = compile(include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/memory-access-semantics.onda"
    ));
    for opt_level in [
        onda_codegen_llvm::TargetOptLevel::O0,
        onda_codegen_llvm::TargetOptLevel::O3,
    ] {
        let jit = jit_program_from_optimized_mir_with_options(
            program.clone(),
            MirCompileOptions {
                opt_level,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut outputs = [[f32::NAN; 48]; 4];
        let pointers = outputs
            .iter_mut()
            .map(|out| out.as_mut_ptr().cast())
            .collect::<Vec<_>>();
        unsafe {
            jit.process_checked(
                &mut state,
                &params,
                0,
                48,
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
        assert_eq!(outputs, [[0.0; 48]; 4], "optimization level {opt_level:?}");
    }
}

#[test]
fn mir_slice_proofs_preserve_empty_views_clamping_and_mutation() {
    let program = compile(include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/slice-bounds-proofs.onda"
    ));
    let transform = program
        .functions
        .iter()
        .find(|f| f.name == "transform" || f.name.starts_with("transform."))
        .unwrap();
    let mut selected = program.as_program().clone();
    selected.functions = vec![transform.clone()];
    let text = onda_mir::format_program(&selected);
    assert!(text.contains("load_slice") && text.contains("store_slice"));
    for access in text
        .lines()
        .filter(|line| line.contains("load_slice") || line.contains("store_slice"))
    {
        assert!(
            access.contains("unchecked"),
            "unproven loop access: {access}"
        );
    }
    let jit =
        jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default()).unwrap();
    let params = jit.default_param_bytes();
    let mut state = jit.initialize_state(&params).unwrap();
    let mut outputs = [[f32::NAN; 48]; 4];
    let pointers = outputs
        .iter_mut()
        .map(|out| out.as_mut_ptr().cast())
        .collect::<Vec<_>>();
    unsafe {
        jit.process_checked(
            &mut state,
            &params,
            0,
            48,
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
    for output in outputs {
        assert_eq!(output, [0.0; 48]);
    }
}

#[test]
fn state_alignment_reaches_vector_accesses_through_slices() {
    let program = compile(
        r#"
params:
  factor = 0.75

def transform(src: f32[], dst: f32[], scale):
  for i in 0..min(src.len(), dst.len()):
    dst[i] = src[i] * scale

init:
  src: f32[256]
  dst: f32[256]
  for i in 0..256:
    src[i] = f32(i)
block:
  transform(src[:], dst[:], factor)
  sample:
    out1 = dst[3]
"#,
    );
    let ir = lower_optimized_mir_to_llvm_ir_with_options(&program, MirCompileOptions::default())
        .unwrap();
    let process = ir
        .lines()
        .find(|line| line.starts_with("define") && line.contains("@onda_process("))
        .unwrap();
    let state = process
        .split("@onda_process(")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap();
    assert!(
        state.contains("align 64"),
        "state argument lost its alignment: {state}"
    );
    let vectors: Vec<_> = ir
        .lines()
        // This program's vector loads only read state. Audio output stores
        // retain the host's natural alignment and must not inherit this fact.
        .filter(|line| line.contains("load <"))
        .collect();
    assert!(!vectors.is_empty(), "slice loop did not vectorize: {ir}");
    for access in vectors {
        let alignment: u32 = access
            .split("align ")
            .nth(1)
            .unwrap()
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(alignment >= 16, "aligned state was weakened: {access}");
    }
}
