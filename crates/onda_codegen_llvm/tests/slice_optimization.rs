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
fn mixed_state_and_local_origins_preserve_in_place_dependencies() {
    for scalar in ["f32", "f64"] {
        let source = include_str!(
            "../../../packages/onda_binaryen_web/test/fixtures/mixed-storage-origins.onda"
        )
        .replace("f32", scalar)
        .replace("out1 = f64(result)", "out1 = f32(result)");
        let program = compile(&source);
        for opt_level in [
            onda_codegen_llvm::TargetOptLevel::O0,
            onda_codegen_llvm::TargetOptLevel::O3,
        ] {
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
            let mut output = [f32::NAN; 48];
            unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    48,
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
            assert_eq!(output, [0.0; 48], "{scalar}, {opt_level:?}");
        }
    }
}

#[test]
fn mixed_state_and_external_origins_preserve_in_place_dependencies() {
    for scalar in ["f32", "f64"] {
        let source = format!(
            r#"
buffers:
  external: {scalar}
def shift(src: {scalar}[], dst: {scalar}[]):
  for i in 0..min(src.len(), dst.len()) - 1:
    dst[i + 1] = src[i]
init:
  initial: {scalar}[64]
  for i in 0..64:
    initial[i] = {scalar}(i + 1)
block:
  shift(initial[:], external[:])
  shift(external[:], external[:])
  sample:
    out1 = f32(external[32])
"#
        );
        let program = compile(&source);
        for opt_level in [
            onda_codegen_llvm::TargetOptLevel::O0,
            onda_codegen_llvm::TargetOptLevel::O3,
        ] {
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
            // Enough naturally aligned zero storage for either precision.
            let mut buffer = [0u64; 64];
            let mut output = [f32::NAN; 48];
            unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    48,
                    3,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[buffer.as_mut_ptr().cast()],
                    &[64],
                    &[1],
                    &[48000.0],
                    None,
                )
                .unwrap();
            }
            assert_eq!(output, [0.0; 48], "{scalar}, {opt_level:?}");
        }
    }
}

#[test]
fn overlapping_views_observe_mutation_across_calls_and_loops() {
    let program = compile(include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/memory-access-semantics.onda"
    ));
    let (index, _) = program
        .functions
        .iter()
        .enumerate()
        .find(|(_, f)| f.name == "loop_read" || f.name.starts_with("loop_read."))
        .unwrap();
    let function = onda_mir::FunctionId::new(index as u32);
    assert_eq!(
        program
            .memory()
            .parameter(function, onda_mir::ParameterId::new(0))
            .states()
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        program
            .memory()
            .parameter(function, onda_mir::ParameterId::new(1))
            .states()
            .unwrap()
            .count(),
        2
    );
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
fn derived_slice_indices_are_proven_without_changing_clamping() {
    let program = compile(include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/slice-derived-bounds.onda"
    ));
    for name in ["reverse", "shifted"] {
        let function = program
            .functions
            .iter()
            .find(|f| f.name == name || f.name.starts_with(&format!("{name}.")))
            .unwrap();
        let mut selected = program.as_program().clone();
        selected.functions = vec![function.clone()];
        let text = onda_mir::format_program(&selected);
        let accesses: Vec<_> = text
            .lines()
            .filter(|line| line.contains("load_slice") || line.contains("store_slice"))
            .collect();
        assert_eq!(accesses.len(), 2, "{text}");
        assert!(
            accesses.iter().all(|line| line.contains("unchecked")),
            "{text}"
        );
    }
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
        let mut outputs = [[f32::NAN; 48]; 2];
        let pointers = outputs
            .iter_mut()
            .map(|out| out.as_mut_ptr().cast())
            .collect::<Vec<_>>();
        for _ in 0..3 {
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
            assert_eq!(outputs, [[0.0; 48]; 2], "{opt_level:?}");
        }
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

#[test]
fn bounded_reverse_slices_expose_independent_array_accesses() {
    let program = compile(
        r#"
params:
  count = 509
  factor = 0.875
def reverse(src: f32[], dst: f32[], factor):
  for i in 0..min(src.len(), dst.len()):
    dst[i] = src[src.len() - 1 - i] * factor
init:
  values: f32[512]
  results: f32[512]
  for i in 0..512:
    values[i] = f32(i)
block:
  reverse(values[:count], results[:count], factor)
  sample:
    out1 = results[3]
"#,
    );
    let ir = lower_optimized_mir_to_llvm_ir_with_options(&program, MirCompileOptions::default())
        .unwrap();
    let process = ir
        .split("@onda_process(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    assert!(
        process.contains("load <"),
        "reverse slice loop stayed scalar: {process}"
    );
}

#[test]
fn short_slice_lengths_survive_forwarding_and_bound_native_loop_cost() {
    let program = compile(
        r#"
params:
  count = 3
def reverse(src: f32[], dst: f32[]):
  for i in 0..min(src.len(), dst.len()):
    dst[i] = src[src.len() - 1 - i]
def forward(src: f32[], dst: f32[]):
  reverse(src, dst)
init:
  values: f32[5] = [1.0, 2.0, 3.0, 4.0, 5.0]
  results: f32[5]
sample:
  forward(values[:count], results[:count])
  out1 = results[0]
"#,
    );
    let (index, function) = program
        .functions
        .iter()
        .enumerate()
        .find(|(_, function)| function.name == "reverse")
        .unwrap();
    let ranges = program
        .ranges()
        .function(onda_mir::FunctionId::new(index as u32))
        .unwrap();
    let mut lengths = 0;
    for statement in &function.body.statements {
        if let onda_mir::StatementKind::Assign {
            destination:
                onda_mir::Place {
                    base: onda_mir::PlaceBase::Local(local),
                    ..
                },
            value: onda_mir::Rvalue::SliceLen(_),
        } = &statement.kind
        {
            let range = ranges.local(*local).unwrap();
            assert_eq!((range.min(), range.max()), (0, 5));
            lengths += 1;
        }
    }
    assert_eq!(lengths, 2);
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("avx2") && !std::is_x86_feature_detected!("avx512f") {
        let ir = lower_optimized_mir_to_llvm_ir_with_options(
            &program,
            MirCompileOptions {
                opt_level: onda_codegen_llvm::TargetOptLevel::O0,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        assert!(
            ir.contains("!\"llvm.loop.vectorize.enable\", i1 false"),
            "{ir}"
        );
    }
}

#[test]
fn slice_length_caps_join_call_sites_and_keep_unknown_external_lengths() {
    let source = r#"
params:
  count = 3
def length(values: f32[]):
  return values.len()
def forward(values: f32[]):
  return length(values[1:])
init:
  small: f32[5]
  large: f32[11]
sample:
  out1 = f32(forward(small[:count]) + forward(large[:count]))
"#;
    for (source, maximum) in [
        (source.to_owned(), 11),
        (
            source
                .replace("params:\n", "buffers:\n  external: f32\nparams:\n")
                .replace("forward(large[:count])", "forward(external[:count])"),
            i32::MAX,
        ),
    ] {
        let mir = compile(&source);
        let (id, function) = mir
            .functions
            .iter()
            .enumerate()
            .find(|(_, function)| function.name == "length")
            .unwrap();
        let ranges = mir
            .ranges()
            .function(onda_mir::FunctionId::new(id as u32))
            .unwrap();
        let length = function
            .body
            .statements
            .iter()
            .find_map(|statement| match &statement.kind {
                onda_mir::StatementKind::Assign {
                    destination:
                        onda_mir::Place {
                            base: onda_mir::PlaceBase::Local(local),
                            ..
                        },
                    value: onda_mir::Rvalue::SliceLen(_),
                } => Some(*local),
                _ => None,
            })
            .unwrap();
        assert_eq!(ranges.local(length).unwrap().max(), i64::from(maximum));
    }
}

#[test]
fn reference_alignment_tracks_whole_arrays_and_offset_views() {
    for (storage, view, expected) in [
        (256, "values[:]", 64),
        (257, "values[1:]", 4),
        (260, "values[4:]", 16),
        (272, "values[16:]", 64),
    ] {
        let source = format!(
            r#"
def sum(values: f32[256]):
  total = 0.0
  for i in 0..256:
    total += values[i]
  return total
init:
  values: f32[{storage}]
  for i in 0..{storage}:
    values[i] = f32(i)
block:
  total = sum({view})
  sample:
    out1 = total
"#
        );
        let mut raw = compile(&source).as_program().clone();
        let helper = raw
            .functions
            .iter_mut()
            .find(|function| function.name == "sum" || function.name.starts_with("sum."))
            .unwrap();
        helper.attributes.inline = onda_mir::InlineHint::Never;
        // Only the inlining hint changed; the compiler's bounds/range proofs
        // remain valid.
        let validated = unsafe { onda_mir::validate_owned_with_producer_proofs(raw) }.unwrap();
        let program = onda_mir::optimize(validated).unwrap().0;
        let ir = lower_optimized_mir_to_llvm_ir_with_options(
            &program,
            MirCompileOptions {
                opt_level: onda_codegen_llvm::TargetOptLevel::O0,
                ..MirCompileOptions::default()
            },
        )
        .unwrap();
        let definition = ir
            .lines()
            .find(|line| line.starts_with("define internal") && line.contains("@__onda_mir_fn_"))
            .unwrap();
        assert!(
            definition.contains(&format!("align {expected} "))
                && definition.contains("dereferenceable(1024)"),
            "{definition}"
        );
        let jit =
            jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default())
                .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut output = [0.0_f32; 48];
        unsafe {
            jit.process_checked(
                &mut state,
                &params,
                0,
                48,
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
        let first = storage - 256;
        let expected: f32 = (first..storage).map(|value| value as f32).sum();
        assert_eq!(output, [expected; 48]);
    }
}

#[test]
fn shared_reference_alignment_is_valid_for_every_view() {
    for (offset, alignment) in [(80, 64), (20, 16), (17, 4)] {
        let mut raw = compile(&format!(
            r#"
def first(values: f32[64]):
  return values[0]
init:
  values: f32[512]
  for i in 0..512:
    values[i] = f32(i)
sample:
  out1 = first(values[16:80]) + first(values[{offset}:{}])
"#,
            offset + 64
        ))
        .into_program();
        raw.functions
            .iter_mut()
            .find(|f| f.name == "first")
            .unwrap()
            .attributes
            .inline = onda_mir::InlineHint::Never;
        let program = onda_mir::optimize(
            unsafe { onda_mir::validate_owned_with_producer_proofs(raw) }.unwrap(),
        )
        .unwrap()
        .0;
        let ir = lower_optimized_mir_to_llvm_ir_with_options(
            &program,
            MirCompileOptions {
                opt_level: onda_codegen_llvm::TargetOptLevel::O0,
                ..Default::default()
            },
        )
        .unwrap();
        let definition = ir
            .lines()
            .find(|line| line.starts_with("define internal") && line.contains("@__onda_mir_fn_"))
            .unwrap();
        assert!(
            definition.contains(&format!("align {alignment} ")),
            "{definition}"
        );
        let jit =
            jit_program_from_optimized_mir_with_options(program, MirCompileOptions::default())
                .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let mut output = [0.0_f32; 48];
        unsafe {
            jit.process_checked(
                &mut state,
                &params,
                0,
                48,
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
        assert_eq!(output, [(16 + offset) as f32; 48]);
    }
}

#[test]
fn descriptor_references_do_not_confuse_storage_with_payload_origins() {
    let mut raw = compile("def first(values: f32[]):\n  return values[0]\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\nsample:\n  out1 = first(values[:])\n").into_program();
    let helper = raw
        .functions
        .iter()
        .position(|function| function.name == "first" || function.name.starts_with("first."))
        .unwrap();
    raw.functions[helper].params[0].mode = onda_mir::PassingMode::ReadOnlyReference;
    fn references(block: &mut onda_mir::Block, helper: usize) {
        for statement in &mut block.statements {
            match &mut statement.kind {
                onda_mir::StatementKind::Call { function, args, .. }
                    if function.index() == helper =>
                {
                    let onda_mir::CallArgument::Value(onda_mir::Value::Local(local)) = args[0]
                    else {
                        panic!("expected a slice descriptor");
                    };
                    args[0] = onda_mir::CallArgument::Place(onda_mir::Place::local(local));
                }
                onda_mir::StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    references(then_block, helper);
                    references(else_block, helper);
                }
                onda_mir::StatementKind::Loop { body } => references(body, helper),
                _ => {}
            }
        }
    }
    for function in &mut raw.functions {
        references(&mut function.body, helper);
    }
    // Borrowing the same immutable descriptor preserves its existing proofs.
    let program =
        onda_mir::optimize(unsafe { onda_mir::validate_owned_with_producer_proofs(raw) }.unwrap())
            .unwrap()
            .0;
    assert!(program
        .memory()
        .parameter(
            onda_mir::FunctionId::new(helper as u32),
            onda_mir::ParameterId::new(0)
        )
        .states()
        .is_none());
}

#[test]
fn fixed_windows_remove_shape_checks_only_when_the_complete_view_fits() {
    use onda_mir::{Block, BoundsMode, CallArgument, Rvalue, ScalarValue, StatementKind, Value};
    fn check_windows(block: &mut Block, length: i32) {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Assign {
                    value: Rvalue::MakeSlice { len, bounds, .. },
                    ..
                } if *len == Value::Constant(ScalarValue::I32(8)) => {
                    *len = Value::Constant(ScalarValue::I32(length));
                    *bounds = BoundsMode::Checked;
                }
                StatementKind::Call { args, .. } => {
                    for arg in args {
                        if let CallArgument::SliceWindow { bounds, .. } = arg {
                            *bounds = BoundsMode::Checked;
                        }
                    }
                }
                StatementKind::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    check_windows(then_block, length);
                    check_windows(else_block, length);
                }
                StatementKind::Loop { body } => check_windows(body, length),
                _ => {}
            }
        }
    }
    for length in [8, 4, 0] {
        let mut raw = compile(
            r#"
def sum(values: f32[8]):
  total = 0.0
  for i in 0..8:
    total += values[i]
  return total
init:
  values: f32[16]
  for i in 0..16:
    values[i] = f32(i)
block:
  total = sum(values[4:12])
  sample:
    out1 = total
"#,
        )
        .into_program();
        check_windows(&mut raw.functions[1].body, length);
        // Existing element proofs remain valid: the explicitly checked call
        // must reject a short view before the fixed-array helper can run.
        let program = onda_mir::optimize(
            unsafe { onda_mir::validate_owned_with_producer_proofs(raw) }.unwrap(),
        )
        .unwrap()
        .0;
        let text = onda_mir::format_program(&program);
        let calls: Vec<_> = text
            .lines()
            .filter(|line| line.contains("slice_window "))
            .collect();
        assert_eq!(calls.len(), 1, "{text}");
        assert!(
            calls[0].contains(if length == 8 {
                " unchecked"
            } else {
                " checked"
            }),
            "{text}"
        );
        for opt_level in [
            onda_codegen_llvm::TargetOptLevel::O0,
            onda_codegen_llvm::TargetOptLevel::O3,
        ] {
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
            let mut output = [f32::NAN; 48];
            let result = unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    48,
                    3,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
            };
            if length == 8 {
                result.unwrap();
                assert_eq!(output, [60.0; 48]);
            } else {
                assert!(result.is_err(), "short view reached an unchecked callee");
            }
        }
    }
}
