#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    MirCompileOptions, TargetOptLevel,
};

const SOURCE: &str =
    include_str!("../../../packages/onda_binaryen_web/test/fixtures/affine-array-pairs.onda");
const INTERLEAVED: &str =
    include_str!("../../../packages/onda_binaryen_web/test/fixtures/interleaved-state-access.onda");

fn compile(source: &str) -> onda_mir::OptimizedProgram {
    let parsed = onda_frontend::parse_program(source).unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48000.0,
            block_size: 32,
        },
    )
    .unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}

fn proven(mir: &onda_mir::OptimizedProgram) -> bool {
    proven_in(mir, "pairs")
}

fn proven_in(mir: &onda_mir::OptimizedProgram, name: &str) -> bool {
    let (id, function) = mir
        .functions
        .iter()
        .enumerate()
        .find(|(_, function)| function.name == name)
        .unwrap();
    let body = function
        .body
        .statements
        .iter()
        .find_map(|statement| {
            if let onda_mir::StatementKind::Loop { body } = &statement.kind {
                Some(body)
            } else {
                None
            }
        })
        .unwrap();
    onda_mir::LoopAnalysis::new(
        mir,
        mir.effects(),
        mir.memory(),
        mir.ranges(),
        onda_mir::FunctionId::new(id as u32),
    )
    .independent_memory_loop(body)
}

#[test]
fn dependent_wrapping_and_unknown_addresses_keep_scalar_dependences() {
    for expression in [
        "i + 1",
        "64 - i",
        "0",
        "(2147483647 + i) - 2147483647",
        "i32(read(values, i))",
    ] {
        let mir = compile(&SOURCE.replace("128 - i\n", &format!("{expression}\n")));
        assert!(!proven(&mir), "unexpected proof for {expression}");
    }
    // The dependency can be hidden behind a helper, even when its caller's
    // arguments look like disjoint mirrored ranges.
    let mir = compile(&SOURCE.replace(
        "values.write_unsafe(i, value)",
        "values.write_unsafe(i % 7, value)",
    ));
    assert!(!proven(&mir));
}

#[test]
fn counted_for_and_while_loops_share_the_proof() {
    let mir = compile(
        &SOURCE
            .replace("i = 1\n  while i < 64:", "for i in 1..64:")
            .replace("    i += 1\n", ""),
    );
    assert!(proven(&mir));
}

#[test]
fn interleaved_accesses_and_multiple_state_origins_share_the_proof() {
    let mir = compile(INTERLEAVED);
    assert!(proven(&mir));
    let (id, _) = mir
        .functions
        .iter()
        .enumerate()
        .find(|(_, f)| f.name == "pairs")
        .unwrap();
    assert_eq!(
        mir.memory()
            .parameter(
                onda_mir::FunctionId::new(id as u32),
                onda_mir::ParameterId::new(0)
            )
            .only_states()
            .unwrap()
            .count(),
        2
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(rendered(&mir, opt_level), [[0.0; 32]; 3]);
    }
    let overlapping = compile(
        &INTERLEAVED
            .replace("0..64:", "0..63:")
            .replace("right = left + 1", "right = left + 2"),
    );
    assert!(!proven(&overlapping));
    assert_eq!(
        rendered(&overlapping, TargetOptLevel::O0),
        rendered(&overlapping, TargetOptLevel::O3)
    );
}

#[test]
fn overlapping_parameter_origin_sets_do_not_imply_disjoint_arrays() {
    let mir = compile(
        r#"
def pairs(destination: f32[128], source: f32[128]):
  for i in 1..64:
    destination[i] = source[i - 1]
init:
  first: f32[128]
  second: f32[128]
  for i in 0..128:
    first[i] = f32(i)
    second[i] = f32(i + 128)
block:
  pairs(first, second)
  pairs(first, first)
  sample:
    out1 = first[63] + second[63]
"#,
    );
    assert!(!proven(&mir));
    assert_eq!(
        rendered(&mir, TargetOptLevel::O0),
        rendered(&mir, TargetOptLevel::O3)
    );
}

#[test]
fn disjoint_state_windows_survive_forwarding_without_inlining_mir() {
    let source =
        include_str!("../../../packages/onda_binaryen_web/test/fixtures/state-window-access.onda");
    let mir = compile(source);
    assert!(proven(&mir));
    let (id, _) = mir
        .functions
        .iter()
        .enumerate()
        .find(|(_, f)| f.name == "pairs")
        .unwrap();
    for (parameter, start) in [(0, 136), (1, 16)] {
        let regions = mir
            .memory()
            .parameter(
                onda_mir::FunctionId::new(id as u32),
                onda_mir::ParameterId::new(parameter),
            )
            .state_regions()
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(regions.len(), 1);
        assert_eq!((regions[0].start, regions[0].len), (start, 64));
    }
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(rendered(&mir, opt_level), [[0.0; 32]; 3]);
    }

    // The shifted destination overlaps the source across iterations. Equal
    // parameter-relative indices do not imply equal backing-array addresses.
    let overlapping = compile(
        &source
            .replace("values[128:224]", "values[9:105]")
            .replace("source[63 - i] * 0.5 + source[i]", "source[i]"),
    );
    assert!(!proven(&overlapping));
    assert_eq!(
        rendered(&overlapping, TargetOptLevel::O0),
        rendered(&overlapping, TargetOptLevel::O3)
    );
}

#[test]
fn known_view_shapes_remove_repeated_descriptor_work_in_mir() {
    let source =
        include_str!("../../../packages/onda_binaryen_web/test/fixtures/state-view-shapes.onda");
    let mir = compile(source);
    assert!(proven_in(&mir, "transform"));
    for name in ["transform", "forward"] {
        let function = mir
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap();
        let mut selected = mir.as_program().clone();
        selected.functions = vec![function.clone()];
        let text = onda_mir::format_program(&selected);
        assert!(!text.contains("slice_len"), "{text}");
        for access in text.lines().filter(|line| {
            line.contains("load_slice")
                || line.contains("store_slice")
                || line.contains("make_slice")
        }) {
            assert!(access.contains("unchecked"), "{access}");
        }
    }
    let mut selected = mir.as_program().clone();
    selected.functions = vec![mir
        .functions
        .iter()
        .find(|f| f.name == "length")
        .unwrap()
        .clone()];
    assert!(
        onda_mir::format_program(&selected).contains("slice_len"),
        "conflicting views must retain their actual lengths"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(rendered(&mir, opt_level), [[0.0; 32]; 3]);
    }
    let overlapping = compile(&source.replace("values[144:240]", "values[13:109]"));
    assert!(!proven_in(&overlapping, "transform"));
    assert_eq!(
        rendered(&overlapping, TargetOptLevel::O0),
        rendered(&overlapping, TargetOptLevel::O3)
    );
}

#[test]
fn shared_helpers_preserve_alternative_views_and_common_lengths() {
    let source =
        include_str!("../../../packages/onda_binaryen_web/test/fixtures/shared-view-ranges.onda");
    for source in [
        source.to_owned(),
        source
            .replace(
                "def transform(source: f32[], destination: f32[]):",
                "def transform(source: f32[64], destination: f32[64]):",
            )
            .replace(
                "def forward(source: f32[], destination: f32[]):",
                "def forward(source: f32[96], destination: f32[96]):",
            ),
    ] {
        let mir = compile(&source);
        assert!(proven_in(&mir, "transform"));
        let (id, function) = mir
            .functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == "transform")
            .unwrap();
        for (parameter, starts) in [(0, [16, 112]), (1, [256, 352])] {
            let regions = mir
                .memory()
                .parameter(
                    onda_mir::FunctionId::new(id as u32),
                    onda_mir::ParameterId::new(parameter),
                )
                .state_regions()
                .unwrap()
                .map(|region| (region.start, region.len))
                .collect::<Vec<_>>();
            assert_eq!(regions, starts.map(|start| (start, 64)));
        }
        let mut selected = mir.as_program().clone();
        selected.functions = vec![function.clone()];
        assert!(!onda_mir::format_program(&selected).contains("slice_len"));
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            assert_eq!(rendered(&mir, opt_level), [[0.0; 32]; 3]);
        }
        let overlapping = compile(
            &source
                .replace("values[108:204]", "values[12:108]")
                .replace("values[344:440]", "values[9:105]"),
        );
        assert!(!proven_in(&overlapping, "transform"));
        assert_eq!(
            rendered(&overlapping, TargetOptLevel::O0),
            rendered(&overlapping, TargetOptLevel::O3)
        );
    }
}

#[test]
fn bounded_runtime_counts_share_the_fixed_loop_dependence_proof() {
    let source =
        include_str!("../../../packages/onda_binaryen_web/test/fixtures/bounded-state-loops.onda");
    let mir = compile(source);
    assert!(
        proven_in(&mir, "transform"),
        "{}\n{:?}",
        onda_mir::format_program(&mir),
        mir.ranges()
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(rendered_blocks::<8>(&mir, opt_level), [[0.0; 32]; 8]);
    }
    for changed in [
        source.replace("min(max(count, 0), 64):", "count:"),
        source.replace("min(max(count, 0), 64):", "min(max(count, 0), 65):"),
        source.replace("values[144:208], count", "values[17:81], count"),
        source.replace("source[63 - i]", "source[(2147483647 + i) - 2147483647]"),
    ] {
        let mir = compile(&changed);
        assert!(!proven_in(&mir, "transform"));
        assert_eq!(
            rendered_blocks::<8>(&mir, TargetOptLevel::O0),
            rendered_blocks::<8>(&mir, TargetOptLevel::O3)
        );
    }
}

#[test]
fn scalar_reference_aliasing_the_written_array_is_not_invariant() {
    use onda_mir::*;
    let source = SOURCE
        .replace(
            "def pairs(values: f32[128]):",
            "def pairs(values: f32[128], anchor: f32):",
        )
        .replace("a + b)", "a + b + anchor)")
        .replace("pairs(values)\n", "pairs(values, 0.0)\n");
    let mut raw = compile(&source).into_program();
    let helper = raw
        .functions
        .iter()
        .position(|function| function.name == "pairs")
        .unwrap();
    raw.functions[helper].params[1].mode = PassingMode::ReadOnlyReference;
    // Read the borrow on every iteration, rather than capturing its entry
    // value once as an ordinary source-language scalar parameter would do.
    let function = &mut raw.functions[helper];
    let load = function.body.statements.iter().position(|statement| matches!(&statement.kind,
        StatementKind::Assign { value: Rvalue::Load(Place { base: PlaceBase::Parameter(parameter), .. }), .. } if parameter.index() == 1)).unwrap();
    let load = function.body.statements.remove(load);
    let body = function
        .body
        .statements
        .iter_mut()
        .find_map(|statement| match &mut statement.kind {
            StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .unwrap();
    let StatementKind::If { then_block, .. } = &mut body.statements[1].kind else {
        panic!("while guard")
    };
    then_block.statements.insert(0, load);
    fn borrow(block: &mut Block, helper: usize) {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Call { function, args, .. } if function.index() == helper => {
                    let CallArgument::SliceWindow { slice, .. } = args[0] else {
                        panic!("array view")
                    };
                    args[1] = CallArgument::SliceElement {
                        slice,
                        index: Value::Constant(ScalarValue::I32(127)),
                        bounds: BoundsMode::Unchecked,
                    };
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    borrow(then_block, helper);
                    borrow(else_block, helper);
                }
                StatementKind::Loop { body } => borrow(body, helper),
                _ => {}
            }
        }
    }
    for function in &mut raw.functions {
        borrow(&mut function.body, helper);
    }
    let mir = optimize(unsafe { validate_owned_with_producer_proofs(raw) }.unwrap())
        .unwrap()
        .0;
    assert!(
        !proven(&mir),
        "a scalar borrow can observe an earlier iteration's write"
    );
    assert_eq!(
        rendered(&mir, TargetOptLevel::O0),
        rendered(&mir, TargetOptLevel::O3)
    );
}

#[test]
fn mirrored_accesses_through_helpers_have_a_shared_dependence_proof() {
    let mir = compile(SOURCE);
    assert!(proven(&mir));
    let ir = lower_optimized_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            opt_level: TargetOptLevel::O0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        ir.lines().any(
            |line| (line.contains("load float") || line.contains("store float"))
                && line.contains("!llvm.access.group")
        ),
        "inlined helper accesses must retain the MIR dependence proof"
    );
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        assert_eq!(rendered(&mir, opt_level), [[0.0; 32]; 3], "{opt_level:?}");
    }
}

fn rendered(mir: &onda_mir::OptimizedProgram, opt_level: TargetOptLevel) -> [[f32; 32]; 3] {
    rendered_blocks(mir, opt_level)
}

fn rendered_blocks<const N: usize>(
    mir: &onda_mir::OptimizedProgram,
    opt_level: TargetOptLevel,
) -> [[f32; 32]; N] {
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
    let mut outputs = [[f32::NAN; 32]; N];
    for output in &mut outputs {
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
    }
    outputs
}
