#![cfg(feature = "llvm-orc")]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    lower_optimized_mir_to_object, MirCompileOptions, MirTargetOptions, TargetOptLevel,
    PROCESSOR_EXECUTION_OK, PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
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
fn portable_optimization_preserves_the_fused_rendering_schedule() {
    let source = include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/ordered-voice-rendering.onda"
    );
    for source in [
        source.to_owned(),
        source
            .replace("f32", "f64")
            .replace("mixed = 0.0", "mixed: f64 = 0.0")
            .replace("expected = 0.0", "expected: f64 = 0.0")
            .replace("out1 = mixed - expected", "out1 = f32(mixed - expected)"),
    ] {
        let (mir, stats) = optimize(&source);
        assert_eq!(stats.distributed_loops, 0);
        assert!(mir.functions.iter().all(|function| function
            .locals
            .iter()
            .all(|local| local.name.as_deref() != Some("$distributed.term"))));
        let original = onda_mir::to_messagepack_optimized(&mir).unwrap();
        let (scheduled, stats) = onda_mir::schedule_vector_loops(&mir).unwrap();
        assert_eq!(stats.distributed_loops, 1);
        assert_eq!(original, onda_mir::to_messagepack_optimized(&mir).unwrap());
        assert_eq!(scheduled.effects(), &onda_mir::analyze_effects(&scheduled));
        assert_eq!(
            scheduled.ranges(),
            &onda_mir::analyze_program_integer_ranges_with_effects(&scheduled, scheduled.effects())
        );
        assert_eq!(scheduled.memory(), &onda_mir::analyze_memory(&scheduled));
        let (again, stats) = onda_mir::schedule_vector_loops(&scheduled).unwrap();
        assert_eq!(stats.distributed_loops, 0);
        assert!(matches!(again, std::borrow::Cow::Borrowed(_)));
    }
}

#[test]
fn native_codegen_selects_distribution_without_changing_portable_mir() {
    let (mir, _) = optimize(
        "const Voices = 64\ninit:\n  phases: f32[Voices]\n  rendered: f32[Voices]\nsample:\n  mixed = 0.0\n  for voice in 0..Voices:\n    phase = phases[voice] + (110.0 + f32(voice) * 0.37) / SR\n    phases[voice] = phase\n    tone = sin(phase * TWO_PI)\n    rendered[voice] = tone\n    mixed += tone\n  out1 = mixed\n",
    );
    let original = onda_mir::to_messagepack_optimized(&mir).unwrap();
    let (scheduled, stats) = onda_mir::schedule_vector_loops(&mir).unwrap();
    assert_eq!(stats.distributed_loops, 1);
    let options = MirCompileOptions::default();
    assert_eq!(
        lower_optimized_mir_to_llvm_ir_with_options(&mir, options).unwrap(),
        lower_optimized_mir_to_llvm_ir_with_options(&scheduled, options).unwrap()
    );
    let target = MirTargetOptions::default();
    assert_eq!(
        lower_optimized_mir_to_object(&mir, &target).unwrap(),
        lower_optimized_mir_to_object(&scheduled, &target).unwrap()
    );
    assert_eq!(original, onda_mir::to_messagepack_optimized(&mir).unwrap());
}

#[test]
fn distribution_preserves_ordered_mixing_and_avoids_coupled_or_small_loops() {
    let distribute = |source: &str| {
        let (mir, _) = optimize(source);
        let (scheduled, stats) = onda_mir::schedule_vector_loops(&mir).unwrap();
        if stats.distributed_loops == 0 {
            assert!(matches!(scheduled, std::borrow::Cow::Borrowed(_)));
        }
        (scheduled.into_owned(), stats)
    };
    let source = include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/ordered-voice-rendering.onda"
    );
    let (mir, stats) = distribute(source);
    assert_eq!(
        distribute(&source.replace("Voices = 256", "Voices = 32"))
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
        distribute(&source.replace("Voices = 256", "Voices = 16"))
            .1
            .distributed_loops,
        0
    );
    assert_eq!(
        distribute(&source.replace(
            "tone = sin(phase * TWO_PI)",
            "tone = sin(phase * TWO_PI) + mixed"
        ))
        .1
        .distributed_loops,
        0
    );
    assert_eq!(
        distribute(
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
                assert_eq!(
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
                    .unwrap(),
                    PROCESSOR_EXECUTION_OK
                );
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

#[test]
fn first_iteration_branch_through_helpers_is_peeled_before_vectorization() {
    let source = r#"
def record(index: i32, values: f32[1024], observed: f32[1]):
  if index == 0:
    observed[0] = values[index]
def forward(index: i32, values: f32[1024], observed: f32[1]):
  record(index, values, observed)
init:
  values: f32[1024]
  observed: f32[1]
sample:
  for index in 0..1024:
    values[index] += 1.0
    forward(index, values, observed)
  out1 = observed[0] + values[1023]
"#;
    let (mir, _) = optimize(source);
    let original = onda_mir::to_messagepack_optimized(&mir).unwrap();
    let (scheduled, stats) = onda_mir::schedule_vector_loops(&mir).unwrap();
    assert_eq!(stats.peeled_loops, 1);
    assert!(
        !matches!(scheduled, std::borrow::Cow::Borrowed(_)),
        "the first iteration should be separated from the uniform remainder"
    );
    assert_eq!(original, onda_mir::to_messagepack_optimized(&mir).unwrap());
    assert_eq!(scheduled.effects(), &onda_mir::analyze_effects(&scheduled));
    assert_eq!(scheduled.memory(), &onda_mir::analyze_memory(&scheduled));
    let (again, stats) = onda_mir::schedule_vector_loops(&scheduled).unwrap();
    assert_eq!(stats.peeled_loops, 0);
    assert!(matches!(again, std::borrow::Cow::Borrowed(_)));

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
        let mut output = [0.0f32; 32];
        for block in 0..3 {
            for (start, frames, flags) in [(0, 13, 1), (13, 19, 2)] {
                unsafe {
                    assert_eq!(
                        jit.process_checked(
                            &mut state,
                            &params,
                            start,
                            frames,
                            flags,
                            &[],
                            &[output.as_mut_ptr().cast()],
                            &[],
                            &[],
                            &[],
                            &[],
                            None,
                        )
                        .unwrap(),
                        PROCESSOR_EXECUTION_OK
                    );
                }
            }
            let expected = std::array::from_fn::<_, 32, _>(|i| 2.0 * (block * 32 + i + 1) as f32);
            assert_eq!(output, expected, "{opt_level:?}");
        }
    }

    for source in [
        source.replace("index == 0", "index != 0"),
        source
            .replace("index == 0", "index == 8")
            .replace("0..1024", "8..1024"),
    ] {
        let (mir, _) = optimize(&source);
        assert_eq!(
            onda_mir::schedule_vector_loops(&mir)
                .unwrap()
                .1
                .peeled_loops,
            1
        );
    }

    for source in [
        source.replace("index == 0", "index == 1"),
        source.replace("0..1024", "0..16"),
        source.replace("0..1024", "0..0"),
        source.replace(
            "    values[index] += 1.0",
            "    if index == 4:\n      continue\n    values[index] += 1.0",
        ),
        source.replace(
            "    values[index] += 1.0",
            "    if index == 4:\n      break\n    values[index] += 1.0",
        ),
    ] {
        let (mir, _) = optimize(&source);
        assert_eq!(
            onda_mir::schedule_vector_loops(&mir)
                .unwrap()
                .1
                .peeled_loops,
            0
        );
    }
}

#[test]
fn peeling_preserves_writes_through_branch_selected_local_aliases() {
    let fixture = include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/peeled-loop-aliased-copy.onda"
    );
    for (choose, expected) in [(true, 32.0f32), (false, 1.0)] {
        let (mir, _) =
            optimize(&fixture.replace("choose: bool = true", &format!("choose: bool = {choose}")));
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
            let mut output = [f32::NAN; 32];
            unsafe {
                assert_eq!(
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
                    .unwrap(),
                    PROCESSOR_EXECUTION_OK
                );
            }
            assert_eq!(output, [expected; 32], "choose={choose}, {opt_level:?}");
        }
    }
}

#[test]
fn peeling_preserves_overlapping_views_and_first_iteration_effects() {
    for (src, dst) in [("values[:64]", "values[1:]"), ("values[1:]", "values[:64]")] {
        let source = format!(
            r#"
def visit(index: i32, src: f32[], dst: f32[], seen: f32[1]):
  if index == 0:
    seen[0] += 1.0
  dst[index] = src[index] + seen[0]
def run(src: f32[], dst: f32[], seen: f32[1]):
  for index in 0..64:
    visit(index, src, dst, seen)
init:
  values: f32[65]
  seen: f32[1]
sample:
  run({src}, {dst}, seen)
  out1 = values[0] + values[64]
"#
        );
        let (mir, _) = optimize(&source);
        assert_eq!(
            onda_mir::schedule_vector_loops(&mir)
                .unwrap()
                .1
                .peeled_loops,
            1
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
            unsafe {
                assert_eq!(
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
                    .unwrap(),
                    PROCESSOR_EXECUTION_OK
                );
            }
            let mut values = [0.0f32; 65];
            for (frame, actual) in output.into_iter().enumerate() {
                let (read, write) = if src == "values[:64]" { (0, 1) } else { (1, 0) };
                for i in 0..64 {
                    values[i + write] = values[i + read] + (frame + 1) as f32;
                }
                assert_eq!(
                    actual,
                    values[0] + values[64],
                    "{src}, {frame}, {opt_level:?}"
                );
            }
        }
    }
}

#[test]
fn peeling_preserves_checked_failure_and_preceding_effects() {
    for fail_at in [0, 7, 63] {
        let source = format!(
            r#"
params:
  fail_at: i32 = {fail_at}
init:
  seen: f32[2]
sample:
  for index in 0..64:
    if index == 0:
      seen[0] += 1.0
    seen[1] = f32(index)
    out1 = seen[0] + seen[1]
    out2 = f32(12 / (index - fail_at))
"#
        );
        let (mir, _) = optimize(&source);
        assert_eq!(
            onda_mir::schedule_vector_loops(&mir)
                .unwrap()
                .1
                .peeled_loops,
            1
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
            for invocation in 1..=2 {
                let mut output = [-12345.0f32; 2];
                let result = unsafe {
                    jit.process_checked(
                        &mut state,
                        &params,
                        0,
                        1,
                        3,
                        &[],
                        &[
                            output.as_mut_ptr().cast(),
                            output.as_mut_ptr().add(1).cast(),
                        ],
                        &[],
                        &[],
                        &[],
                        &[],
                        None,
                    )
                };
                assert_eq!(
                    result.unwrap(),
                    PROCESSOR_EXECUTION_RUNTIME_SAFETY_FAILURE,
                    "{fail_at}, {opt_level:?}"
                );
                // Audio outputs commit only after successful sample completion.
                assert_eq!(output, [-12345.0; 2]);
                let entry = jit
                    .state_entries()
                    .iter()
                    .find(|entry| entry.name() == "seen")
                    .unwrap();
                let offset = entry.storage_byte_offset();
                let seen =
                    f32::from_ne_bytes(state.bytes()[offset..offset + 4].try_into().unwrap());
                let index =
                    f32::from_ne_bytes(state.bytes()[offset + 4..offset + 8].try_into().unwrap());
                assert_eq!(seen, invocation as f32);
                assert_eq!(index, fail_at as f32);
            }
        }
    }
}

#[test]
fn peeled_remainder_folds_first_index_tests_without_reordering_state_updates() {
    let fixture = include_str!(
        "../../../packages/onda_binaryen_web/test/fixtures/peeled-loop-predicates.onda"
    );
    for (start, unequal) in [(0, false), (0, true), (8, false)] {
        let source = fixture
            .replace(
                "sample:\n  for i in 0..N:",
                &format!("sample:\n  for i in {start}..N:"),
            )
            .replace(
                "if i == 0:",
                &format!("if i {} {start}:", if unequal { "!=" } else { "==" }),
            );
        let (mir, _) = optimize(&source);
        let (scheduled, stats) = onda_mir::schedule_vector_loops(&mir).unwrap();
        assert_eq!(stats.peeled_loops, 1);
        assert_eq!(
            stats.folded_rvalues, 1,
            "fold the first-index comparison only in the remainder"
        );
        assert_eq!(scheduled.effects(), &onda_mir::analyze_effects(&scheduled));
        assert_eq!(scheduled.memory(), &onda_mir::analyze_memory(&scheduled));
        assert_eq!(
            scheduled.ranges(),
            &onda_mir::analyze_program_integer_ranges_with_effects(&scheduled, scheduled.effects())
        );
        let (again, stats) = onda_mir::schedule_vector_loops(&scheduled).unwrap();
        assert!(matches!(again, std::borrow::Cow::Borrowed(_)));
        assert_eq!(stats.folded_rvalues, 0);
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
            let mut phases = [0.0f32; 256];
            for _ in 0..3 {
                let mut output = [f32::NAN; 32];
                for (offset, frames, flags) in [(0, 13, 1), (13, 19, 2)] {
                    unsafe {
                        assert_eq!(
                            jit.process_checked(
                                &mut state,
                                &params,
                                offset,
                                frames,
                                flags,
                                &[],
                                &[output.as_mut_ptr().cast()],
                                &[],
                                &[],
                                &[],
                                &[],
                                None,
                            )
                            .unwrap(),
                            PROCESSOR_EXECUTION_OK
                        );
                    }
                }
                for actual in output {
                    let mut observed = 0.0;
                    for (index, phase) in phases.iter_mut().enumerate().skip(start) {
                        *phase += (110.0 + index as f32) / 48_000.0;
                        if *phase >= 1.0 {
                            *phase -= 1.0;
                        }
                        if (index == start) != unequal {
                            observed = *phase;
                        }
                    }
                    let total = phases.iter().fold(0.0f32, |sum, phase| sum + phase);
                    assert_eq!(
                        actual.to_bits(),
                        (total / 256.0 + observed).to_bits(),
                        "{start}, {unequal}, {opt_level:?}"
                    );
                }
            }
        }
    }
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
                    assert_eq!(
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
                        .unwrap(),
                        PROCESSOR_EXECUTION_OK
                    );
                }
            }
            assert_eq!(outputs, [[0.0; 32]; 4], "{opt_level:?}");
        }
    }
}
