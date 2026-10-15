use super::*;

fn source(operation: &str, width: &str, values: &[f64]) -> String {
    let n = values.len();
    let values = values
        .iter()
        .map(|value| format!("{value:.330}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("outs:\n  out1: f64\ninit:\n  values: {width}[{n}] = [{values}]\n  results: {width}[{n}]\n  index = 0\nblock:\n  for i in 0..{n}:\n    results[i] = {operation}(values[i])\n  sample:\n    out1 = f64(results[index])\n    index += 1\n    if index >= {n}:\n      index = 0\n")
}

fn inputs(operation: &str, width: &str) -> Vec<f64> {
    let mut seed = 0x123456789abcdef_u64;
    let mut values = (0..257)
        .map(|i| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if operation == "exp" {
                return (i as f64 - 128.0) * if width == "f32" { 0.625 } else { 5.5 };
            }
            let value = if width == "f32" {
                // Finite normal numbers distributed over the full exponent range.
                f32::from_bits((seed as u32 & 0x807fffff) | ((1 + (seed >> 32) as u32 % 254) << 23))
                    as f64
            } else {
                f64::from_bits((seed & 0x800fffffffffffff) | ((1 + (seed >> 52) % 2046) << 52))
            };
            if operation == "log" {
                value.abs()
            } else {
                value
            }
        })
        .collect::<Vec<_>>();
    let boundaries = match operation {
        "exp" if width == "f32" => vec![-87.3, -1.0, 0.0, 1.0, 88.72],
        "exp" => vec![-708.0, -1.0, 0.0, 1.0, 709.78],
        "log" if width == "f32" => vec![
            f32::MIN_POSITIVE as f64,
            0.5,
            1.0,
            1.0000001192092896,
            f32::MAX as f64,
        ],
        "log" => vec![f64::MIN_POSITIVE, 0.5, 1.0, 1.0000000000000002, f64::MAX],
        _ => vec![
            0.0,
            -0.0,
            std::f64::consts::PI,
            -std::f64::consts::PI,
            if width == "f32" {
                f32::MAX as f64
            } else {
                f64::MAX
            },
        ],
    };
    values[..boundaries.len()].copy_from_slice(&boundaries);
    values
}

fn render(source: &str, fast_math: bool) -> Vec<f64> {
    let (_, mir) = source_program(source, 257);
    let native = lower_mir_and_jit_with_options(
        mir,
        MirCompileOptions {
            fast_math,
            opt_level: TargetOptLevel::O3,
        },
    )
    .unwrap();
    let params = native.default_param_bytes();
    let mut state = native.initialize_state(&params).unwrap();
    let mut output = vec![0.0_f64; 257];
    native
        .test_process_checked(
            &mut state,
            &params,
            0,
            257,
            onda_mir::PROCESSOR_FULL_BLOCK as u32,
            &[],
            &[output.as_mut_ptr().cast()],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap();
    output
}

#[test]
fn bundled_vector_math_matches_scalar_math_over_wide_finite_ranges() {
    for width in ["f32", "f64"] {
        for operation in ["sin", "cos", "exp", "log"] {
            let source = source(operation, width, &inputs(operation, width));
            let (_, mir) = source_program(&source, 257);
            let ir = lower_mir_to_llvm_ir_with_options(
                &mir,
                MirCompileOptions {
                    fast_math: true,
                    opt_level: TargetOptLevel::O3,
                },
            )
            .unwrap();
            assert!(
                ir.lines()
                    .any(|line| line.contains("call") && line.contains("@onda_sleef_")),
                "missing {operation}/{width} vector helper"
            );
            let strict = render(&source, false);
            let relaxed = render(&source, true);
            for (i, (&expected, &actual)) in strict.iter().zip(&relaxed).enumerate() {
                assert!(
                    expected.is_finite() && actual.is_finite(),
                    "{operation}/{width}/{i}: {actual} vs {expected}"
                );
                if actual == expected {
                    continue;
                } // Fast math permits signed-zero changes.
                let ulps = if width == "f32" {
                    (actual as f32)
                        .to_bits()
                        .abs_diff((expected as f32).to_bits()) as u64
                } else {
                    actual.to_bits().abs_diff(expected.to_bits())
                };
                assert!(
                    ulps <= 4,
                    "{operation}/{width}/{i}: {actual} vs {expected}, {ulps} ULP"
                );
            }
        }
    }
}

#[test]
fn vector_helpers_are_internal_and_emit_on_all_release_targets() {
    for width in ["f32", "f64"] {
        for operation in ["sin", "cos", "exp", "log"] {
            let (_, mir) =
                source_program(&source(operation, width, &inputs(operation, width)), 257);
            let optimized = trusted_optimized(mir).unwrap();
            for (triple, magic) in [
                ("x86_64-unknown-linux-gnu", &b"\x7fELF"[..]),
                ("x86_64-pc-windows-msvc", &b"\x64\x86"[..]),
                ("aarch64-apple-darwin", &b"\xcf\xfa\xed\xfe"[..]),
            ] {
                let options = MirTargetOptions {
                    fast_math: true,
                    target: crate::TargetConfig::for_triple(triple),
                };
                let ir = lower_optimized_mir_to_target_llvm_ir(&optimized, &options).unwrap();
                assert!(
                    ir.lines()
                        .any(|line| line.contains("call") && line.contains("@onda_sleef_")),
                    "{triple}/{operation}/{width}"
                );
                assert!(!ir
                    .lines()
                    .any(|line| line.starts_with("declare") && line.contains("@onda_sleef_")));
                assert!(!ir.lines().any(|line| line.starts_with("define")
                    && line.contains("@onda_sleef_")
                    && !line.starts_with("define internal")));
                assert!(!ir.contains("_ZGV"));
                assert!(lower_optimized_mir_to_object(&optimized, &options)
                    .unwrap()
                    .starts_with(magic));
            }
            for opt_level in [TargetOptLevel::O0, TargetOptLevel::O1, TargetOptLevel::O3] {
                let ir = lower_optimized_mir_to_llvm_ir_with_options(
                    &optimized,
                    MirCompileOptions {
                        fast_math: opt_level != TargetOptLevel::O3,
                        opt_level,
                    },
                )
                .unwrap();
                assert!(!ir.contains("onda_sleef_"));
            }
        }
    }
}

#[test]
fn padded_and_split_helpers_execute_with_a_scalar_pointer_abi() {
    use crate::orc_backend::{jit_utils, vector_math};
    use llvm_sys::ir_reader::LLVMParseIRInContext;
    use llvm_sys::target::LLVMCopyStringRepOfTargetData;
    use llvm_sys::target_machine::{
        LLVMCreateTargetDataLayout, LLVMDisposeTargetMachine, LLVMGetTargetMachineTriple,
    };
    unsafe {
        initialize_native_llvm().unwrap();
        let target = jit_utils::create_host_target_machine(crate::TargetOptLevel::O3).unwrap();
        for bits in [32, 64] {
            for lanes in [2, 3, 16] {
                for operation in ["sin", "cos", "exp", "log"] {
                    let ty = if bits == 32 { "float" } else { "double" };
                    let ir = format!(
                        "define void @test(ptr %input, ptr %output) {{
  %x = load <{lanes} x {ty}>, ptr %input, align 1
  %y = call fast <{lanes} x {ty}> @llvm.{operation}.v{lanes}f{bits}(<{lanes} x {ty}> %x)
  store <{lanes} x {ty}> %y, ptr %output, align 1
  ret void
}}
declare <{lanes} x {ty}> @llvm.{operation}.v{lanes}f{bits}(<{lanes} x {ty}>)"
                    );
                    let context = LLVMContextCreate();
                    let buffer = LLVMCreateMemoryBufferWithMemoryRangeCopy(
                        ir.as_ptr().cast(),
                        ir.len(),
                        c"test".as_ptr(),
                    );
                    let mut module = null_mut();
                    let mut error = null_mut();
                    assert_eq!(
                        LLVMParseIRInContext(context, buffer, &mut module, &mut error),
                        0
                    );
                    let triple = LLVMGetTargetMachineTriple(target);
                    LLVMSetTarget(module, triple);
                    LLVMDisposeMessage(triple);
                    let layout = LLVMCreateTargetDataLayout(target);
                    let layout_string = LLVMCopyStringRepOfTargetData(layout);
                    LLVMSetDataLayout(module, layout_string);
                    LLVMDisposeMessage(layout_string);
                    LLVMDisposeTargetData(layout);
                    vector_math::lower(module, target).unwrap();
                    let ir = jit_utils::llvm_module_to_string(module).unwrap();
                    assert!(ir.contains("call fast"));
                    assert!(ir.contains("@onda_sleef_"));
                    let mut jit = null_mut();
                    assert!(LLVMOrcCreateLLJIT(&mut jit, null_mut()).is_null());
                    let thread_context = crate::orc_backend::llvm_helpers::llvm_orc_create_new_thread_safe_context_from_llvm_context(context);
                    let owned_module = LLVMOrcCreateNewThreadSafeModule(module, thread_context);
                    LLVMOrcDisposeThreadSafeContext(thread_context);
                    assert!(LLVMOrcLLJITAddLLVMIRModule(
                        jit,
                        LLVMOrcLLJITGetMainJITDylib(jit),
                        owned_module
                    )
                    .is_null());
                    let mut address = 0;
                    assert!(LLVMOrcLLJITLookup(jit, &mut address, c"test".as_ptr()).is_null());
                    let function: unsafe extern "C" fn(*const u8, *mut u8) =
                        std::mem::transmute(address as usize);
                    let inputs = (0..lanes)
                        .map(|i| 0.125 + i as f64 * 0.125)
                        .collect::<Vec<_>>();
                    let actual = if bits == 32 {
                        let inputs = inputs.iter().map(|x| *x as f32).collect::<Vec<_>>();
                        let mut output = vec![0.0_f32; lanes];
                        function(inputs.as_ptr().cast(), output.as_mut_ptr().cast());
                        output.iter().map(|x| *x as f64).collect::<Vec<_>>()
                    } else {
                        let mut output = vec![0.0_f64; lanes];
                        function(inputs.as_ptr().cast(), output.as_mut_ptr().cast());
                        output
                    };
                    assert!(LLVMOrcDisposeLLJIT(jit).is_null());
                    for (input, actual) in inputs.iter().zip(actual) {
                        let expected = match operation {
                            "sin" => input.sin(),
                            "cos" => input.cos(),
                            "exp" => input.exp(),
                            _ => input.ln(),
                        };
                        let tolerance = if bits == 32 { 2e-7 } else { 1e-15 };
                        assert!(
                            (actual - expected).abs() <= tolerance * expected.abs().max(1.0),
                            "{operation}/f{bits}/{lanes}: {actual} vs {expected}"
                        );
                    }
                }
            }
        }
        LLVMDisposeTargetMachine(target);
    }
}
