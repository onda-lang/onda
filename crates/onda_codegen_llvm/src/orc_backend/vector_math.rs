//! Late lowering to bundled SLEEF bitcode. Helpers retain their own floating-point
//! rules and are internal to the emitted module, including standalone objects.

use super::*;
use llvm_sys::bit_reader::LLVMParseBitcodeInContext2;
use llvm_sys::linker::LLVMLinkModules2;
use llvm_sys::target_machine::LLVMGetTargetMachineFeatureString;
use llvm_sys::{LLVMLinkage, LLVMOpcode, LLVMTypeKind};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Isa {
    Sse2,
    Avx2,
    AdvSimd,
}

impl Isa {
    fn name(self) -> &'static str {
        match self {
            Self::Sse2 => "sse2",
            Self::Avx2 => "avx2",
            Self::AdvSimd => "advsimd",
        }
    }

    fn bitcode(self, bits: u32) -> &'static [u8] {
        macro_rules! kernel {
            ($file:literal) => {
                include_bytes!(concat!("../../math-kernel/", $file))
            };
        }
        match (self, bits) {
            (Self::Sse2, 32) => kernel!("sse2-f32.bc"),
            (Self::Sse2, 64) => kernel!("sse2-f64.bc"),
            (Self::Avx2, 32) => kernel!("avx2-f32.bc"),
            (Self::Avx2, 64) => kernel!("avx2-f64.bc"),
            (Self::AdvSimd, 32) => kernel!("advsimd-f32.bc"),
            (Self::AdvSimd, 64) => kernel!("advsimd-f64.bc"),
            _ => unreachable!("only f32/f64 vector math is lowered"),
        }
    }
}

fn select_isa(triple: &str, features: &str, vector_bits: u32) -> Option<Isa> {
    if vector_bits == 0 {
        return None;
    }
    let feature = |name: &str| {
        features.split(',').rev().find_map(|token| {
            if token.strip_prefix('+') == Some(name) {
                Some(true)
            } else if token.strip_prefix('-') == Some(name) {
                Some(false)
            } else {
                None
            }
        })
    };
    if feature("soft-float") == Some(true) {
        return None;
    }
    if triple.starts_with("x86_64-") {
        if ["sse", "sse2"]
            .iter()
            .any(|name| feature(name) == Some(false))
        {
            return None;
        }
        match vector_bits {
            width
                if width >= 256
                    && ["avx", "avx2", "fma"]
                        .iter()
                        .all(|name| feature(name) == Some(true))
                    && ["sse3", "ssse3", "sse4.1", "sse4.2"]
                        .iter()
                        .all(|name| feature(name) != Some(false)) =>
            {
                Some(Isa::Avx2)
            }
            _ => Some(Isa::Sse2),
        }
    } else if triple.starts_with("aarch64-")
        && feature("neon") != Some(false)
        && feature("fp-armv8") != Some(false)
    {
        Some(Isa::AdvSimd)
    } else {
        None
    }
}

unsafe fn name(value: LLVMValueRef) -> String {
    let mut len = 0;
    let bytes = LLVMGetValueName2(value, &mut len);
    String::from_utf8_lossy(std::slice::from_raw_parts(bytes.cast::<u8>(), len)).into_owned()
}

unsafe fn link_kernel(module: LLVMModuleRef, isa: Isa, bits: u32) -> Result<(), Diagnostic> {
    let bytes = isa.bitcode(bits);
    let buffer = LLVMCreateMemoryBufferWithMemoryRangeCopy(
        bytes.as_ptr().cast(),
        bytes.len(),
        c"onda-sleef".as_ptr(),
    );
    let mut kernel = null_mut();
    let status = LLVMParseBitcodeInContext2(LLVMGetModuleContext(module), buffer, &mut kernel);
    LLVMDisposeMemoryBuffer(buffer);
    if status != 0 {
        return Err(Diagnostic::internal(
            "failed to parse bundled SLEEF bitcode",
        ));
    }
    LLVMSetTarget(kernel, LLVMGetTarget(module));
    LLVMSetDataLayout(kernel, LLVMGetDataLayoutStr(module));
    // The linker consumes kernel, even on failure.
    if LLVMLinkModules2(module, kernel) != 0 {
        return Err(Diagnostic::internal("failed to link bundled SLEEF bitcode"));
    }
    Ok(())
}

pub(super) unsafe fn lower(
    module: LLVMModuleRef,
    target: LLVMTargetMachineRef,
) -> Result<(), Diagnostic> {
    let triple = super::jit_utils::target_machine_triple_string(target)?;
    let raw_features = LLVMGetTargetMachineFeatureString(target);
    let features = CStr::from_ptr(raw_features).to_string_lossy().into_owned();
    LLVMDisposeMessage(raw_features);
    let mut calls = Vec::new();
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        let mut block = LLVMGetFirstBasicBlock(function);
        while !block.is_null() {
            let mut instruction = LLVMGetFirstInstruction(block);
            while !instruction.is_null() {
                if LLVMGetInstructionOpcode(instruction) == LLVMOpcode::LLVMCall {
                    let ty = LLVMTypeOf(instruction);
                    if LLVMGetTypeKind(ty) == LLVMTypeKind::LLVMVectorTypeKind {
                        let intrinsic = name(LLVMGetCalledValue(instruction));
                        let operation = intrinsic
                            .strip_prefix("llvm.")
                            .and_then(|n| n.split('.').next());
                        let bits = match LLVMGetTypeKind(LLVMGetElementType(ty)) {
                            LLVMTypeKind::LLVMFloatTypeKind => 32,
                            LLVMTypeKind::LLVMDoubleTypeKind => 64,
                            _ => 0,
                        };
                        if let Some(operation @ ("sin" | "cos" | "exp" | "log")) = operation {
                            if let Some(isa) =
                                select_isa(&triple, &features, LLVMGetVectorSize(ty) * bits)
                            {
                                calls.push((instruction, isa, bits, operation.to_owned()));
                            }
                        }
                    }
                }
                instruction = LLVMGetNextInstruction(instruction);
            }
            block = LLVMGetNextBasicBlock(block);
        }
        function = LLVMGetNextFunction(function);
    }
    if calls.is_empty() {
        return Ok(());
    }
    let mut linked = HashSet::new();
    for &(_, isa, bits, _) in &calls {
        if linked.insert((isa, bits)) {
            link_kernel(module, isa, bits)?;
        }
    }
    let builder = LLVMCreateBuilderInContext(LLVMGetModuleContext(module));
    for (call, isa, bits, operation) in calls {
        let symbol =
            CString::new(format!("onda_sleef_{}_{operation}_f{bits}", isa.name())).unwrap();
        let helper = LLVMGetNamedFunction(module, symbol.as_ptr());
        let ty = LLVMTypeOf(call);
        let lanes = LLVMGetVectorSize(ty);
        let helper_lanes = if isa == Isa::Avx2 { 256 } else { 128 } / bits;
        let helper_ty = LLVMVectorType(LLVMGetElementType(ty), helper_lanes);
        let fn_ty = LLVMFunctionType(helper_ty, [helper_ty].as_mut_ptr(), 1, 0);
        if helper.is_null() || LLVMGlobalGetValueType(helper) != fn_ty {
            LLVMDisposeBuilder(builder);
            return Err(Diagnostic::internal(
                "bundled SLEEF helper signature mismatch",
            ));
        }
        LLVMPositionBuilderBefore(builder, call);
        let argument = LLVMGetArgOperand(call, 0);
        let index_ty = LLVMInt32TypeInContext(LLVMGetModuleContext(module));
        let index = |i: u32| LLVMConstInt(index_ty, i.into(), 0);
        let one = LLVMConstReal(LLVMGetElementType(ty), 1.0);
        let padding = LLVMConstVector(vec![one; lanes as usize].as_mut_ptr(), lanes);
        let mut replacement = LLVMGetPoison(ty);
        for start in (0..lanes).step_by(helper_lanes as usize) {
            // Small SLP vectors use neutral padding; wider vectors are split.
            // This also covers AVX2 targets without FMA using SSE2 helpers.
            let chunk = if lanes == helper_lanes {
                argument
            } else {
                let mut mask = (start..start + helper_lanes)
                    .map(|i| index(i.min(lanes)))
                    .collect::<Vec<_>>();
                LLVMBuildShuffleVector(
                    builder,
                    argument,
                    padding,
                    LLVMConstVector(mask.as_mut_ptr(), helper_lanes),
                    c"math_chunk".as_ptr(),
                )
            };
            let result = LLVMBuildCall2(
                builder,
                fn_ty,
                helper,
                [chunk].as_mut_ptr(),
                1,
                c"vector_math".as_ptr(),
            );
            LLVMSetFastMathFlags(result, LLVMGetFastMathFlags(call));
            if lanes == helper_lanes {
                LLVMSetTailCall(result, LLVMIsTailCall(call));
                replacement = result;
            } else {
                for lane in 0..helper_lanes.min(lanes - start) {
                    let value = LLVMBuildExtractElement(
                        builder,
                        result,
                        index(lane),
                        c"math_lane".as_ptr(),
                    );
                    replacement = LLVMBuildInsertElement(
                        builder,
                        replacement,
                        value,
                        index(start + lane),
                        c"math_result".as_ptr(),
                    );
                }
            }
        }
        LLVMReplaceAllUsesWith(call, replacement);
        LLVMInstructionEraseFromParent(call);
    }
    LLVMDisposeBuilder(builder);
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        if name(function).starts_with("onda_sleef_") && LLVMIsDeclaration(function) == 0 {
            LLVMSetLinkage(function, LLVMLinkage::LLVMInternalLinkage);
        }
        function = LLVMGetNextFunction(function);
    }
    // Only discard unused helpers. Re-running O3 on the application here can
    // change loop/tail vectorization and contaminate the helper's FP policy.
    let options = LLVMCreatePassBuilderOptions();
    let error = LLVMRunPasses(module, c"globaldce".as_ptr(), target, options);
    LLVMDisposePassBuilderOptions(options);
    if !error.is_null() {
        return Err(super::jit_utils::llvm_error_to_diag(
            "failed to discard unused SLEEF helpers",
            error,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{select_isa, Isa};

    #[test]
    fn helpers_follow_the_target_isa_and_explicit_feature_disables() {
        for os in [
            "unknown-linux-gnu",
            "unknown-linux-musl",
            "pc-windows-msvc",
            "apple-darwin",
        ] {
            let triple = format!("x86_64-{os}");
            assert_eq!(select_isa(&triple, "", 64), Some(Isa::Sse2));
            assert_eq!(select_isa(&triple, "+avx,+avx2", 256), Some(Isa::Sse2));
            assert_eq!(select_isa(&triple, "+avx,+avx2,+fma", 512), Some(Isa::Avx2));
            assert_eq!(
                select_isa(&triple, "+avx,+avx2,+fma,-fma", 256),
                Some(Isa::Sse2)
            );
            assert_eq!(select_isa(&triple, "-sse2,+avx,+avx2,+fma", 256), None);
        }
        assert_eq!(
            select_isa("aarch64-apple-darwin", "", 64),
            Some(Isa::AdvSimd)
        );
        assert_eq!(select_isa("aarch64-apple-darwin", "-neon", 128), None);
        assert_eq!(select_isa("aarch64_be-unknown-linux-gnu", "", 128), None);
        assert_eq!(select_isa("wasm32-unknown-unknown", "", 128), None);
    }
}
