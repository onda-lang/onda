#![cfg(feature = "llvm-orc")]
use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    FloatMathPolicy, MirCompileOptions,
};

fn is_vector_call(line: &str) -> bool {
    line.contains("call ") && line.contains("@Sleef_")
}

fn compile(source: &str) -> onda_mir::OptimizedProgram {
    let parsed = onda_frontend::parse_program(source).unwrap();
    let typed = onda_semantics::analyze_with_options(
        parsed,
        onda_semantics::AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 128,
        },
    )
    .unwrap();
    onda_semantics::lower_program_to_optimized_mir(&typed).unwrap()
}

#[test]
fn permissions_are_independent_and_strict_by_default() {
    let mir = compile("ins 1\nsample:\n  out1 = in1 * 0.7 + 0.3\n");
    let strict =
        lower_optimized_mir_to_llvm_ir_with_options(&mir, MirCompileOptions::default()).unwrap();
    assert!(!strict.contains("fmul contract") && !strict.contains("fadd reassoc"));
    let contract = lower_optimized_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            math: FloatMathPolicy {
                contract: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert!(contract.contains("contract"));
    assert!(
        !contract.contains("reassoc") && !contract.contains("nnan") && !contract.contains("nsz")
    );
    let reorder = lower_optimized_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            math: FloatMathPolicy {
                reassociate: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert!(reorder.contains("reassoc"));
    assert!(!reorder.contains("contract") && !reorder.contains("nnan") && !reorder.contains("nsz"));
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn native_vector_math_calls_resolve_and_preserve_exceptional_values() {
    macro_rules! check {
        ($scalar:ty, $name:literal) => {
            for operation in ["sin", "cos", "tan", "tanh", "atan", "atan2", "exp", "log", "pow"] {
                let binary = matches!(operation, "atan2" | "pow");
                let argument = if binary { "in1, in2" } else { "in1" };
                let mir = compile(&format!("ins:\n  in1: {}\n  in2: {}\nouts:\n  out1: {}\nsample:\n  out1 = {operation}({argument})\n", $name, $name, $name));
                let options = MirCompileOptions { math: FloatMathPolicy::default(), ..Default::default() };
                let ir = lower_optimized_mir_to_llvm_ir_with_options(&mir, options).unwrap();
                assert!(ir.lines().any(is_vector_call), "{operation} did not use vector math:\n{ir}");
                assert!(ir.contains("!DILocation("), "IR must retain source locations for remarks");
                let jit = jit_program_from_optimized_mir_with_options(mir, options).unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                let mut input = [0.0 as $scalar; 128];
                for (index, value) in input.iter_mut().enumerate() { *value = (index as $scalar - 64.0) / 8.0; }
                input[..10].copy_from_slice(&[<$scalar>::NAN, <$scalar>::INFINITY, <$scalar>::NEG_INFINITY, 0.0, -0.0, 1e20, 1e-20, <$scalar>::MAX, -<$scalar>::MAX, <$scalar>::MIN_POSITIVE]);
                let mut second = [0.5 as $scalar; 128];
                if binary {
                    let special = [<$scalar>::NAN, <$scalar>::INFINITY, <$scalar>::NEG_INFINITY, 0.0, -0.0, 1.0, -1.0, 2.0, -2.0, 0.5];
                    for (i, (&left, &right)) in special.iter().flat_map(|left| special.iter().map(move |right| (left, right))).enumerate() {
                        input[i] = left;
                        second[i] = right;
                    }
                }
                let mut output = [0.0 as $scalar; 128];
                unsafe { jit.process_checked(&mut state, &params, 0, 128, 3, &[input.as_ptr().cast(), second.as_ptr().cast()], &[output.as_mut_ptr().cast()], &[], &[], &[], &[], None).unwrap(); }
                for ((&input, &second), &actual) in input.iter().zip(&second).zip(&output) {
                    let expected = match operation {
                        "sin" => input.sin(), "cos" => input.cos(), "tan" => input.tan(),
                        "tanh" => input.tanh(), "atan" => input.atan(), "atan2" => input.atan2(second),
                        "exp" => input.exp(), "log" => input.ln(), "pow" => input.powf(second), _ => unreachable!(),
                    };
                    if expected.is_nan() { assert!(actual.is_nan(), "{operation}({input}) = {actual}"); }
                    else if !expected.is_finite() || expected == 0.0 { assert_eq!(actual.to_bits(), expected.to_bits(), "{operation}({input})"); }
                    else { assert!((actual - expected).abs() <= 5.0 * <$scalar>::EPSILON * expected.abs(), "{operation}({input}): {actual} vs {expected}"); }
                }
            }
        }
    }
    check!(f32, "f32");
    check!(f64, "f64");
}

fn math_source(precision: &str) -> String {
    let mut source = format!("ins:\n  in1: {precision}\n  in2: {precision}\nouts:\n");
    for i in 1..=10 {
        source.push_str(&format!("  out{i}: {precision}\n"));
    }
    source.push_str("sample:\n");
    for (index, expression) in [
        "sin(in1)",
        "cos(in1)",
        "tan(in1)",
        "tanh(in1)",
        "atan(in1)",
        "atan2(in1, in2)",
        "exp(in1)",
        "log(in1)",
        "pow(in1, in2)",
        "pow(2.0, in1)",
    ]
    .iter()
    .enumerate()
    {
        source.push_str(&format!("  out{} = {expression}\n", index + 1));
    }
    source
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn power_of_two_uses_vector_exp2_with_scalar_tails_and_overlap_fallbacks() {
    macro_rules! check {
        ($scalar:ty, $name:literal) => {
            for (base, log2_base) in [(2.0 as $scalar, 1.0), (4.0, 2.0), (0.5, -1.0)] {
                let mir = compile(&format!(
                    "ins:\n  in1: {0}\nouts:\n  out1: {0}\nsample:\n  out1 = pow({base:?}, in1)\n", $name
                ));
                let ir = lower_optimized_mir_to_llvm_ir_with_options(&mir, MirCompileOptions::default()).unwrap();
                assert!(ir.lines().any(|line| is_vector_call(line) && line.contains("@Sleef_exp2")), "{base}: no vector exp2");
                assert!(!ir.lines().any(|line| line.contains("call ") && line.contains("@llvm.exp2.v")), "unresolved vector exp2");
                assert!(ir.contains("audio_alias_guard") && ir.contains("audio_overlap"),
                    "{base}: missing alias guard or scalar overlap fallback");
                let jit = jit_program_from_optimized_mir_with_options(mir, MirCompileOptions::default()).unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                for output_base in [7, 8, 9, 256] {
                    for segments in [vec![(0, 127)], vec![(3, 125)], vec![(0, 1), (1, 65), (66, 62)]] {
                        let mut values = [0.0 as $scalar; 512];
                        for (index, value) in values[8..136].iter_mut().enumerate() {
                            *value = (index as $scalar - 64.0) / 8.0;
                        }
                        let overflow = <$scalar>::MAX_EXP as $scalar;
                        let underflow = (<$scalar>::MIN_EXP - <$scalar>::MANTISSA_DIGITS as i32) as $scalar;
                        values[8..22].copy_from_slice(&[
                            <$scalar>::NAN, <$scalar>::INFINITY, <$scalar>::NEG_INFINITY,
                            0.0, -0.0, overflow, overflow - 0.5, overflow + 0.5,
                            underflow, underflow - 1.0, underflow + 0.5, underflow - 0.5,
                            <$scalar>::MAX, -<$scalar>::MAX,
                        ]);
                        let mut expected = values;
                        for &(start, frames) in &segments {
                            for frame in start..start + frames {
                                let input = expected[8 + frame];
                                let exponent = input * log2_base;
                                // Let m be the smallest positive subnormal. At or below m/2,
                                // ties-to-even gives zero; between m/2 and m it gives m.
                                // Derive these answers directly, without platform powf underflow.
                                expected[output_base + frame] = if exponent <= underflow - 1.0 {
                                    0.0
                                } else if exponent <= underflow {
                                    <$scalar>::from_bits(1)
                                } else {
                                    base.powf(input)
                                };
                            }
                            unsafe {
                                jit.process_checked(&mut state, &params, start, frames, 3,
                                    &[values.as_ptr().add(8).cast()], &[values.as_mut_ptr().add(output_base).cast()],
                                    &[], &[], &[], &[], None).unwrap();
                            }
                        }
                        for (index, (&actual, &expected)) in values.iter().zip(&expected).enumerate() {
                            let written = segments.iter().any(|&(start, frames)|
                                (output_base + start..output_base + start + frames).contains(&index));
                            if !written {
                                assert_eq!(actual.to_bits(), expected.to_bits(),
                                    "{} modified untouched index {index}, output {output_base}, {segments:?}", $name);
                            } else if expected.is_nan() {
                                assert!(actual.is_nan());
                            } else if !expected.is_finite() || expected == 0.0 {
                                assert_eq!(actual.to_bits(), expected.to_bits(),
                                    "{} pow({base}, x), index {index}, output {output_base}, {segments:?}", $name);
                            } else {
                                // Differential integration check against platform libm, not
                                // a proof of SLEEF's error bound against the exact real result.
                                assert!(actual.is_finite() && actual.to_bits().abs_diff(expected.to_bits()) <= 2,
                                    "{} pow({base}, x): {actual} vs {expected}, index {index}, output {output_base}, {segments:?}", $name);
                            }
                        }
                    }
                }
            }
        };
    }
    check!(f32, "f32");
    check!(f64, "f64");
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn power_of_two_underflow_obeys_the_sleef_accuracy_contract() {
    macro_rules! check {
        ($scalar:ty, $name:literal) => {
            // m = 2^(MIN_EXP - MANTISSA_DIGITS) is the smallest positive subnormal.
            // These intervals follow from |actual - 2^offset * m| <= m (one ULP),
            // requiring exact zero when the correctly rounded result is zero, as
            // SLEEF's src/common/testerutil.c countULPsp/countULPdp do.
            let cases = [
                (-2.0, 0, 0),
                (-1.5, 0, 0),
                (-1.0, 0, 0),
                (-0.5, 0, 1),
                (0.0, 0, 2),
                (0.5, 1, 2),
                (1.0, 1, 3),
                (2.0, 3, 5),
            ];
            let underflow = (<$scalar>::MIN_EXP - <$scalar>::MANTISSA_DIGITS as i32) as $scalar;
            for (base, log2_base) in [(2.0 as $scalar, 1.0), (4.0, 2.0), (0.5, -1.0)] {
                let mir = compile(&format!(
                    "ins:\n  in1: {0}\nouts:\n  out1: {0}\nsample:\n  out1 = pow({base:?}, in1)\n", $name
                ));
                let jit = jit_program_from_optimized_mir_with_options(mir, MirCompileOptions::default()).unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                for (offset, low, high) in cases {
                    let input = [(underflow + offset) / log2_base; 128];
                    for (start, frames) in [(0, 1), (3, 7), (0, 127), (0, 128)] {
                        let sentinel = -12345.0 as $scalar;
                        let mut output = [sentinel; 128];
                        unsafe {
                            jit.process_checked(&mut state, &params, start, frames, 3,
                                &[input.as_ptr().cast()], &[output.as_mut_ptr().cast()],
                                &[], &[], &[], &[], None).unwrap();
                        }
                        for (index, actual) in output.iter().enumerate() {
                            if (start..start + frames).contains(&index) {
                                assert!((low..=high).contains(&actual.to_bits()),
                                    "{} pow({base}, {}): bits {}, expected {low}..={high}, frame {index}, start {start}, frames {frames}",
                                    $name, input[index], actual.to_bits());
                            } else {
                                assert_eq!(actual.to_bits(), sentinel.to_bits(), "untouched frame {index}");
                            }
                        }
                    }
                }
            }
        };
    }
    check!(f32, "f32");
    check!(f64, "f64");
}

#[test]
fn vector_helpers_are_embedded_for_native_and_cross_targets() {
    use onda_codegen_llvm::{
        lower_optimized_mir_to_object_artifact, lower_optimized_mir_to_target_llvm_ir,
        MirTargetOptions, TargetConfig,
    };
    for precision in ["f32", "f64"] {
        let mir = compile(&math_source(precision));
        for triple in [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
        ] {
            let options = MirTargetOptions {
                target: TargetConfig::for_triple(triple),
                ..Default::default()
            };
            let ir = lower_optimized_mir_to_target_llvm_ir(&mir, &options).unwrap();
            assert!(ir.lines().any(is_vector_call), "{triple}/{precision}");
            assert!(
                ir.contains("@Sleef_exp2"),
                "{triple}/{precision}: missing exp2"
            );
            for line in ir.lines().filter(|line| line.contains("@Sleef_")) {
                assert!(!line.starts_with("declare"), "{triple}: {line}");
                if line.starts_with("define") {
                    assert!(
                        line.starts_with("define internal fastcc"),
                        "{triple}: {line}"
                    );
                }
            }
            assert!(!ir.contains("vector-function-abi-variant"));
            let artifact = lower_optimized_mir_to_object_artifact(&mir, &options).unwrap();
            assert_eq!(artifact.metadata.compile.floating_point, None);
            assert!(artifact.metadata.integration.link_libraries.is_empty());
        }
    }
}

#[test]
fn vector_variants_follow_cpu_features_and_platform_abi() {
    use onda_codegen_llvm::{
        lower_optimized_mir_to_target_llvm_ir, MirTargetOptions, TargetConfig, TargetCpu,
    };
    let mir = compile("ins 1\nouts 2\nsample:\n  out1 = atan2(in1, 0.5)\n  out2 = pow(2.0, in1)\n");
    for (triple, cpu, features, expected, forbidden) in [
        (
            "x86_64-unknown-linux-gnu",
            "generic",
            "",
            "u35sse2",
            "u35avx",
        ),
        (
            "x86_64-unknown-linux-gnu",
            "sandybridge",
            "",
            "u35avx",
            "u35avx2",
        ),
        (
            "x86_64-unknown-linux-gnu",
            "haswell",
            "",
            "u35avx2",
            "u35avx512",
        ),
        (
            "x86_64-unknown-linux-gnu",
            "haswell",
            "-fma",
            "u35avx",
            "u35avx2",
        ),
        (
            "x86_64-unknown-linux-gnu",
            "knl",
            "",
            "u35avx512f",
            "u35advsimd",
        ),
        (
            "x86_64-unknown-linux-musl",
            "generic",
            "",
            "u35sse2",
            "u35avx",
        ),
        (
            "x86_64-unknown-linux-musl",
            "sandybridge",
            "",
            "u35avx",
            "u35avx2",
        ),
        (
            "x86_64-unknown-linux-musl",
            "haswell",
            "",
            "u35avx2",
            "u35avx512",
        ),
        (
            "x86_64-unknown-linux-musl",
            "haswell",
            "-fma",
            "u35avx",
            "u35avx2",
        ),
        (
            "x86_64-unknown-linux-musl",
            "knl",
            "",
            "u35avx512f",
            "u35advsimd",
        ),
        ("x86_64-pc-windows-msvc", "generic", "", "u35sse2", "u35avx"),
        (
            "x86_64-pc-windows-msvc",
            "haswell",
            "",
            "u35avx2",
            "u35avx512",
        ),
        ("x86_64-apple-darwin", "generic", "", "u35sse2", "u35avx"),
        (
            "aarch64-apple-darwin",
            "generic",
            "",
            "u35advsimd",
            "u35avx",
        ),
        (
            "aarch64-unknown-linux-gnu",
            "generic",
            "",
            "u35advsimd",
            "u35avx",
        ),
    ] {
        let options = MirTargetOptions {
            fast_math: false,
            math: FloatMathPolicy::default(),
            target: TargetConfig {
                cpu: TargetCpu::Explicit(cpu.to_owned()),
                features: Some(features.to_owned()),
                ..TargetConfig::for_triple(triple)
            },
        };
        let ir = lower_optimized_mir_to_target_llvm_ir(&mir, &options).unwrap();
        assert!(
            ir.contains("@Sleef_exp2"),
            "{triple}/{cpu}/{features}: missing exp2"
        );
        let calls = ir
            .lines()
            .filter(|line| is_vector_call(line))
            .collect::<Vec<_>>();
        assert!(
            !calls.is_empty(),
            "no vector calls for {triple}/{cpu}/{features}"
        );
        assert!(
            ir.contains(expected),
            "missing {expected} for {triple}/{cpu}/{features}"
        );
        assert!(
            !ir.contains(forbidden),
            "illegal {forbidden} for {triple}/{cpu}/{features}"
        );
        for call in calls {
            assert!(call.contains("fastcc"), "{call}");
        }
        // Exercise machine lowering for every selected ISA, even when the
        // current host cannot execute it (AVX-512 and cross-target code).
        onda_codegen_llvm::lower_optimized_mir_to_object_artifact(&mir, &options).unwrap();
    }
}

#[test]
fn unused_vector_helpers_are_discarded() {
    use onda_codegen_llvm::{
        lower_optimized_mir_to_object_artifact, MirTargetOptions, TargetConfig,
    };
    let mir = compile("ins 1\nsample:\n  out1 = in1 * 0.5\n");
    let artifact = lower_optimized_mir_to_object_artifact(
        &mir,
        &MirTargetOptions {
            fast_math: false,
            math: FloatMathPolicy::default(),
            target: TargetConfig::for_triple("x86_64-unknown-linux-gnu"),
        },
    )
    .unwrap();
    assert!(artifact.metadata.integration.link_libraries.is_empty());
    for opt_level in [
        onda_codegen_llvm::TargetOptLevel::O0,
        onda_codegen_llvm::TargetOptLevel::O3,
    ] {
        let ir = lower_optimized_mir_to_llvm_ir_with_options(
            &mir,
            MirCompileOptions {
                opt_level,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!ir.contains("Sleef_"));
    }
    let scalar_math = compile("ins 1\nsample:\n  out1 = sin(in1)\n");
    let ir = lower_optimized_mir_to_llvm_ir_with_options(
        &scalar_math,
        MirCompileOptions {
            opt_level: onda_codegen_llvm::TargetOptLevel::O0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!ir.contains("Sleef_"));
}

#[test]
fn application_fast_math_does_not_relax_helper_arithmetic() {
    let mir = compile(&math_source("f64"));
    let ir = lower_optimized_mir_to_llvm_ir_with_options(
        &mir,
        MirCompileOptions {
            fast_math: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut helper = false;
    let mut arithmetic = 0;
    for line in ir.lines() {
        if line.starts_with("define") {
            helper = line.contains("@Sleef_");
        } else if helper
            && ["fadd ", "fsub ", "fmul ", "fdiv ", "fcmp "]
                .iter()
                .any(|opcode| line.contains(opcode))
        {
            arithmetic += 1;
            for flag in [
                "fast", "reassoc", "nnan", "ninf", "nsz", "arcp", "contract", "afn",
            ] {
                assert!(!line.split_whitespace().any(|word| word == flag), "{line}");
            }
        }
    }
    assert!(arithmetic > 0, "no helper arithmetic inspected");
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn slp_math_calls_use_internal_helpers() {
    use onda_codegen_llvm::{
        lower_optimized_mir_to_target_llvm_ir, MirTargetOptions, TargetConfig,
    };
    // Carried feedback blocks loop vectorization, so these four adjacent state
    // updates must use SLP and share the loop vectorizer's private helper ABI.
    let mir = compile(include_str!("fixtures/vector_math_feedback.onda"));
    let math = FloatMathPolicy::default();
    for triple in [
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ] {
        let ir = lower_optimized_mir_to_target_llvm_ir(
            &mir,
            &MirTargetOptions {
                fast_math: false,
                math,
                target: TargetConfig::for_triple(triple),
            },
        )
        .unwrap();
        let calls = ir.lines().filter(|line| is_vector_call(line));
        let mut count = 0;
        for call in calls {
            count += 1;
            assert!(call.contains("fastcc"), "{triple}: {call}");
        }
        assert!(count > 0, "SLP did not vectorize for {triple}");
    }
    let input = std::array::from_fn::<f32, 128, _>(|i| ((i as f32) * 0.1).sin() * 0.05);
    let mut results = [[0.0_f32; 128]; 2];
    for (opt_level, output) in [
        onda_codegen_llvm::TargetOptLevel::O0,
        onda_codegen_llvm::TargetOptLevel::O3,
    ]
    .into_iter()
    .zip(&mut results)
    {
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
        unsafe {
            jit.process_checked(
                &mut state,
                &params,
                0,
                128,
                3,
                &[input.as_ptr().cast()],
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
    for (&strict, &vector) in results[0].iter().zip(&results[1]) {
        assert!((strict - vector).abs() < 2e-5, "{strict} vs {vector}");
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn native_vector_math_objects_link_without_the_compiler_runtime() {
    use onda_codegen_llvm::{
        lower_optimized_mir_to_object_artifact, MirTargetOptions, TargetConfig, TargetCpu,
        TargetRelocMode,
    };
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let windows = cfg!(target_os = "windows");
    let targets = vec![
        ("generic", TargetCpu::Explicit("generic".to_owned()), None),
        ("host", TargetCpu::Host, None),
    ];
    #[cfg(target_arch = "x86_64")]
    let targets = {
        let mut targets = targets;
        for (name, features, available) in [
            ("avx", "+avx", std::arch::is_x86_feature_detected!("avx")),
            (
                "avx2-no-fma",
                "+avx2,-fma",
                std::arch::is_x86_feature_detected!("avx2"),
            ),
            (
                "avx2-fma",
                "+avx2,+fma",
                std::arch::is_x86_feature_detected!("avx2")
                    && std::arch::is_x86_feature_detected!("fma"),
            ),
            (
                "avx512-fma",
                "+avx512f,+fma",
                std::arch::is_x86_feature_detected!("avx512f")
                    && std::arch::is_x86_feature_detected!("fma"),
            ),
        ] {
            if available {
                targets.push((
                    name,
                    TargetCpu::Explicit("generic".to_owned()),
                    Some(features.to_owned()),
                ));
            }
        }
        targets
    };
    for precision in ["f32", "f64"] {
        let mir = compile(&math_source(precision));
        // Execute every available vector ISA, not just compile foreign targets.
        for (name, cpu, features) in &targets {
            let artifact = lower_optimized_mir_to_object_artifact(
                &mir,
                &MirTargetOptions {
                    fast_math: false,
                    math: FloatMathPolicy::default(),
                    target: TargetConfig {
                        cpu: cpu.clone(),
                        features: features.clone(),
                        reloc_model: TargetRelocMode::Pic,
                        ..TargetConfig::host()
                    },
                },
            )
            .unwrap();
            assert_eq!(artifact.metadata.runtime.state_size_bytes, 0);
            assert_eq!(artifact.metadata.runtime.param_size_bytes, 0);
            assert!(artifact.metadata.integration.link_libraries.is_empty());
            let object = dir.path().join(if windows {
                "processor.obj"
            } else {
                "processor.o"
            });
            let executable = dir.path().join(if windows { "host.exe" } else { "host" });
            std::fs::write(&object, artifact.object_bytes).unwrap();
            let mut command = Command::new(
                std::env::var_os("CC").unwrap_or_else(|| if windows { "cl" } else { "cc" }.into()),
            );
            command.current_dir(dir.path());
            if windows {
                command
                    .args(["/nologo", "/O2", "/MT", "/std:c11"])
                    .arg(format!("/I{}", root.join("../../include").display()))
                    .arg(format!("/Fe{}", executable.display()));
                if precision == "f64" {
                    command.arg("/DTEST_F64");
                }
            } else {
                command
                    .args(["-O2", "-std=c11", "-I"])
                    .arg(root.join("../../include"))
                    .arg("-o")
                    .arg(&executable);
                if precision == "f64" {
                    command.arg("-DTEST_F64");
                }
            }
            command
                .arg(root.join("tests/fixtures/vector_math_host.c"))
                .arg(&object);
            if !windows {
                command.arg("-lm");
            }
            let output = command
                .output()
                .expect("the native C compiler must be available");
            assert!(
                output.status.success(),
                "{command:?}\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let output = Command::new(&executable)
                .arg(root.join("tests/fixtures/vector_math_reference.txt"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{precision}/{name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
