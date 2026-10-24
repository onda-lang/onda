#![cfg(all(
    feature = "llvm-orc",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, lower_optimized_mir_to_llvm_ir_with_options,
    MirCompileOptions,
};

#[path = "support/vector_math.rs"]
mod reference;

fn check_references(audio_mode: bool) {
    let cases = reference::cases();
    macro_rules! check {
        ($scalar:ty, $precision:literal, $width:literal) => {
            for operation in reference::operations() {
                let selected: Vec<_> = cases.iter().copied().filter(|case|
                    case.precision == $width && case.operation == operation
                    && (!audio_mode || case.safe_in_audio_mode())).collect();
                assert!(!selected.is_empty(), "no {operation}/{} reference cases", $precision);
                let mir = reference::compile(operation, $precision);
                let ir = lower_optimized_mir_to_llvm_ir_with_options(&mir, MirCompileOptions::default()).unwrap();
                assert!(ir.lines().any(|line| line.contains("call ") && line.contains("@Sleef_")),
                    "{operation}/{} did not vectorize", $precision);
                let jit = jit_program_from_optimized_mir_with_options(mir, MirCompileOptions::default()).unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                for chunk in selected.chunks(128) {
                    let left = std::array::from_fn::<_, 128, _>(|i|
                        <$scalar>::from_bits(chunk[i % chunk.len()].left as _));
                    let right = std::array::from_fn::<_, 128, _>(|i|
                        <$scalar>::from_bits(chunk[i % chunk.len()].right as _));
                    for (start, frames) in [(0, 128), (3, 125), (0, 1), (0, 7), (0, 0)] {
                        let sentinel = -12345.0 as $scalar;
                        let mut output = [sentinel; 128];
                        unsafe {
                            jit.process_checked(&mut state, &params, start, frames, 3,
                                &[left.as_ptr().cast(), right.as_ptr().cast()], &[output.as_mut_ptr().cast()],
                                &[], &[], &[], &[], None).unwrap();
                        }
                        for (i, actual) in output.iter().enumerate() {
                            if (start..start + frames).contains(&i) {
                                let case = chunk[i % chunk.len()];
                                let accepted = if start == 0 && frames == 128 {
                                    case.accepts_vector(actual.to_bits() as u64)
                                } else {
                                    case.accepts_scalar_or_vector(actual.to_bits() as u64)
                                };
                                assert!(accepted,
                                    "{case:?}: actual bits {:x}, frame {i}, start {start}, frames {frames}, audio {audio_mode}", actual.to_bits());
                            } else {
                                assert_eq!(actual.to_bits(), sentinel.to_bits(), "untouched frame {i}");
                            }
                        }
                    }
                }
            }
        };
    }
    check!(f32, "f32", 32);
    check!(f64, "f64", 64);
}

#[test]
fn vector_math_matches_independent_mpfr_intervals() {
    check_references(false);
}

#[test]
fn scalar_underflow_uses_the_mpfr_error_budget() {
    let cases = reference::cases();
    for (precision, two) in [(32, u64::from(2.0_f32.to_bits())), (64, 2.0_f64.to_bits())] {
        let sign = 1 << (precision - 1);
        for negative in [0, sign] {
            let case = cases
                .iter()
                .copied()
                .find(|case| {
                    case.operation == "atan2"
                        && case.precision == precision
                        && case.left == negative | 1
                        && case.right == two
                })
                .unwrap();
            // For m = the smallest subnormal, 0 < atan(m/2) < m/2.
            // Thus 3m is within 3.5 ULP, but 4m exceeds it. The negative
            // case is symmetric. Windows CRT returned m in the positive f64 case.
            for magnitude in 0..=4 {
                let raw = negative | magnitude;
                assert_eq!(
                    case.accepts_scalar_or_vector(raw),
                    magnitude <= 3,
                    "{case:?}, bits {raw:x}"
                );
                assert_eq!(
                    case.accepts_vector(raw),
                    magnitude == 0,
                    "{case:?}, bits {raw:x}"
                );
            }
            assert!(
                !case.accepts_scalar_or_vector(negative ^ sign),
                "underflow must retain its sign"
            );
            let exact_zero = cases
                .iter()
                .copied()
                .find(|case| {
                    case.operation == "atan2"
                        && case.precision == precision
                        && case.left == negative
                        && case.right == two
                })
                .unwrap();
            assert!(exact_zero.accepts_scalar_or_vector(negative));
            assert!(
                !exact_zero.accepts_scalar_or_vector(negative | 1),
                "exact zeros are not underflow approximations"
            );
            assert!(
                !exact_zero.accepts_scalar_or_vector(negative ^ sign),
                "exact zero must retain its sign"
            );
        }
    }
}

#[test]
fn reference_intervals_match_analytic_powers_of_two() {
    let cases = reference::cases();
    for (precision, minimum_exponent, negative_maximum) in [
        (32, -149, u64::from((-f32::MAX).to_bits())),
        (64, -1074, (-f64::MAX).to_bits()),
    ] {
        let input_bits = |value: f64| {
            if precision == 32 {
                u64::from((value as f32).to_bits())
            } else {
                value.to_bits()
            }
        };
        // In units of the smallest subnormal m, exp2(minimum_exponent + d)
        // equals 2^d * m. These integer ranges follow analytically from the
        // one-ULP budget, with SLEEF's additional rounded-zero rule.
        for (offset, vector_low, vector_high, scalar_low, scalar_high) in [
            (-2.0, 0, 0, 0, 1),
            (-1.5, 0, 0, 0, 1),
            (-1.0, 0, 0, 0, 1),
            (-0.5, 0, 1, 0, 1),
            (0.0, 0, 2, 0, 2),
            (0.5, 1, 2, 1, 2),
            (1.0, 1, 3, 1, 3),
            (2.0, 3, 5, 3, 5),
        ] {
            let case = cases
                .iter()
                .copied()
                .find(|case| {
                    case.operation == "exp2"
                        && case.precision == precision
                        && case.left == input_bits(f64::from(minimum_exponent) + offset)
                })
                .unwrap();
            for raw in 0..=6 {
                assert_eq!(
                    case.accepts_vector(raw),
                    (vector_low..=vector_high).contains(&raw),
                    "{case:?}, bits {raw}"
                );
                assert_eq!(
                    case.accepts_scalar_or_vector(raw),
                    (scalar_low..=scalar_high).contains(&raw),
                    "{case:?}, bits {raw}"
                );
            }
        }
        for operation in ["exp", "exp2"] {
            // Both functions are strictly positive for finite inputs, even when
            // MPFR's own exponent range cannot represent the real result.
            let case = cases
                .iter()
                .copied()
                .find(|case| {
                    case.operation == operation
                        && case.precision == precision
                        && case.left == negative_maximum
                })
                .unwrap();
            assert!(case.accepts_vector(0));
            assert!(!case.accepts_vector(1));
            assert!(
                case.accepts_scalar_or_vector(1),
                "{case:?}: positive underflow is within one ULP of m"
            );
            assert!(!case.accepts_scalar_or_vector(2));
        }
    }
}

#[test]
fn all_vector_operations_preserve_aliases_tails_and_memory_guards() {
    let cases = reference::cases();
    macro_rules! check {
        ($scalar:ty, $precision:literal, $width:literal) => {
            for operation in reference::operations() {
                let selected: Vec<_> = cases.iter().copied().filter(|case|
                    case.precision == $width && case.operation == operation).collect();
                let mir = reference::compile(operation, $precision);
                let jit = jit_program_from_optimized_mir_with_options(mir, MirCompileOptions::default()).unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                let mut output_bases = vec![15, 16, 17, 512];
                if matches!(operation, "atan2" | "pow") {
                    output_bases.extend([255, 256, 257]);
                }
                for chunk in selected.chunks(128) {
                    for &output_base in &output_bases {
                        // Only writes ahead of an input affect future reads. A write
                        // behind an input may legally use vector math, so validate it
                        // against MPFR rather than demanding scalar-libm bit equality.
                        let forward_overlap = matches!(output_base, 17 | 257);
                        for segments in [vec![(0, 128)], vec![(3, 125)], vec![(0, 0), (0, 1), (1, 7), (8, 120)]] {
                            let mut memory = [-12345.0 as $scalar; 768];
                            for i in 0..128 {
                                memory[16 + i] = <$scalar>::from_bits(chunk[i % chunk.len()].left as _);
                                memory[256 + i] = <$scalar>::from_bits(chunk[i % chunk.len()].right as _);
                            }
                            let mut sequential = memory;
                            for &(start, frames) in &segments {
                                // One-frame calls cannot take a vector path. Replaying writes
                                // immediately is the reference for observable overlap ordering;
                                // independent MPFR cases separately validate the math itself.
                                if forward_overlap {
                                    for frame in start..start + frames {
                                        unsafe {
                                            jit.process_checked(&mut state, &params, frame, 1, 3,
                                                &[sequential.as_ptr().add(16).cast(), sequential.as_ptr().add(256).cast()],
                                                &[sequential.as_mut_ptr().add(output_base).cast()],
                                                &[], &[], &[], &[], None).unwrap();
                                        }
                                    }
                                }
                                unsafe {
                                    jit.process_checked(&mut state, &params, start, frames, 3,
                                        &[memory.as_ptr().add(16).cast(), memory.as_ptr().add(256).cast()],
                                        &[memory.as_mut_ptr().add(output_base).cast()],
                                        &[], &[], &[], &[], None).unwrap();
                                }
                            }
                            for (i, &actual) in memory.iter().enumerate() {
                                let written = segments.iter().any(|&(start, frames)|
                                    (output_base + start..output_base + start + frames).contains(&i));
                                if !written || forward_overlap {
                                    let expected = sequential[i];
                                    assert!(actual.to_bits() == expected.to_bits()
                                        || written && actual.is_nan() && expected.is_nan(),
                                        "{operation}/{}: index {i}, output {output_base}, {segments:?}: {actual} vs {expected}", $precision);
                                } else {
                                    let case = chunk[(i - output_base) % chunk.len()];
                                    assert!(case.accepts_scalar_or_vector(actual.to_bits() as u64),
                                        "{case:?}: index {i}, output {output_base}, {segments:?}, actual {actual}");
                                }
                            }
                        }
                    }
                }
            }
        };
    }
    check!(f32, "f32", 32);
    check!(f64, "f64", 64);
}

#[cfg(target_arch = "x86_64")]
fn fp_control() -> u64 {
    let mut value = 0_u32;
    unsafe {
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut value, options(nostack, preserves_flags));
    }
    u64::from(value & !0x3f) // Ignore sticky exception status, retain all control bits.
}

#[cfg(target_arch = "aarch64")]
fn fp_control() -> u64 {
    let value: u64;
    unsafe {
        std::arch::asm!("mrs {}, fpcr", out(reg) value, options(nostack, preserves_flags));
    }
    value
}

fn check_audio_underflow(flush: bool) {
    macro_rules! check {
        ($scalar:ty, $precision:literal) => {
            for (expression, input, expected) in [
                (
                    "pow(2.0, in1) * 1.25",
                    (<$scalar>::MIN_EXP - 3) as $scalar,
                    <$scalar>::MIN_POSITIVE.to_bits() / 16 * 5,
                ),
                (
                    "sin(in1) * 1.25",
                    <$scalar>::from_bits(<$scalar>::MIN_POSITIVE.to_bits() / 2),
                    <$scalar>::MIN_POSITIVE.to_bits() / 8 * 5,
                ),
            ] {
                let mir = reference::compile_expression(expression, $precision);
                let jit =
                    jit_program_from_optimized_mir_with_options(mir, MirCompileOptions::default())
                        .unwrap();
                let params = jit.default_param_bytes();
                let mut state = jit.initialize_state(&params).unwrap();
                let left = [input; 128];
                let right = [0.0 as $scalar; 128];
                for frames in [1, 127, 128] {
                    let mut output = [-12345.0 as $scalar; 128];
                    unsafe {
                        jit.process_checked(
                            &mut state,
                            &params,
                            0,
                            frames,
                            3,
                            &[left.as_ptr().cast(), right.as_ptr().cast()],
                            &[output.as_mut_ptr().cast()],
                            &[],
                            &[],
                            &[],
                            &[],
                            None,
                        )
                        .unwrap();
                    }
                    for actual in &output[..frames] {
                        assert_eq!(
                            actual.to_bits(),
                            if flush { 0 } else { expected },
                            "{expression}/{}: input {input}, flush {flush}, frames {frames}",
                            $precision
                        );
                    }
                }
            }
        };
    }
    check!(f32, "f32");
    check!(f64, "f64");
}

#[test]
fn audio_fp_mode_preserves_normal_math_and_controls_subnormal_arithmetic() {
    let parent_control = fp_control();
    // Production configuration is thread-local and once-only. A dedicated thread
    // exercises the real API without leaking its mode into other tests.
    std::thread::spawn(|| {
        let gradual = fp_control();
        #[cfg(target_arch = "x86_64")]
        assert_eq!(gradual & ((1 << 6) | (1 << 15)), 0);
        check_audio_underflow(false);
        onda_realtime::configure_current_thread_audio_fp_mode();
        let audio = fp_control();
        #[cfg(target_arch = "x86_64")]
        assert_eq!(audio & ((1 << 6) | (1 << 15)), (1 << 6) | (1 << 15));
        #[cfg(target_arch = "aarch64")]
        assert_eq!(audio, gradual);
        check_references(true);
        check_audio_underflow(cfg!(target_arch = "x86_64"));
        assert_eq!(
            fp_control(),
            audio,
            "generated math changed FP control state"
        );
    })
    .join()
    .unwrap();
    assert_eq!(
        fp_control(),
        parent_control,
        "audio mode escaped its thread"
    );
}
