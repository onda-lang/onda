//! Portable SLEEF mappings with late linking of precompiled, freestanding helpers.
//! Helpers retain their numerical policy and are private to each generated module.
use super::*;
use llvm_sys::bit_reader::LLVMParseBitcodeInContext2;
use llvm_sys::linker::LLVMLinkModules2;
use llvm_sys::transforms::pass_builder::*;

#[path = "../../vector_math_spec.rs"]
mod spec;

#[derive(Clone, Copy, Default)]
pub(super) struct Library {
    variants: [Option<(&'static str, u32)>; 3],
}

impl Library {
    pub(super) unsafe fn for_target(machine: LLVMTargetMachineRef, triple: &str) -> Self {
        unsafe extern "C" {
            fn onda_vector_math_features(machine: LLVMTargetMachineRef) -> u32;
        }
        let windows = triple.contains("windows-msvc");
        let unix = triple.contains("linux") || triple.contains("apple");
        if !(unix || windows) || (windows && !triple.starts_with("x86_64-")) {
            return Self::default();
        }
        let features = onda_vector_math_features(machine);
        let mut variants = [None; 3];
        if features & 1 != 0 {
            variants[0] = Some((if features & 4 != 0 { "avx2128" } else { "sse2" }, 16));
        }
        if features & 2 != 0 {
            variants[1] = Some((if features & 4 != 0 { "avx2" } else { "avx" }, 32));
        }
        if features & 8 != 0 {
            variants[2] = Some(("avx512f", 64));
        }
        if features & 16 != 0 {
            variants[0] = Some(("advsimd", 16));
        }
        Self { variants }
    }

    pub(super) fn is_available(self) -> bool {
        self.variants.iter().any(Option::is_some)
    }
}

// VFABI mappings refer to declarations by name. Keep those declarations alive
// until the vectorizer uses them, as LLVM's InjectTLIMappings pass does.
pub(super) unsafe fn preserve_variants(context: LLVMContextRef, module: LLVMModuleRef) {
    let mut functions = Vec::new();
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        let mut len = 0;
        let name = LLVMGetValueName2(function, &mut len);
        if std::slice::from_raw_parts(name.cast::<u8>(), len).starts_with(b"Sleef_") {
            functions.push(function);
        }
        function = LLVMGetNextFunction(function);
    }
    if functions.is_empty() {
        return;
    }
    let ty = LLVMPointerTypeInContext(context, 0);
    let array = LLVMConstArray2(ty, functions.as_mut_ptr(), functions.len() as u64);
    let used = LLVMAddGlobal(module, LLVMTypeOf(array), c"llvm.compiler.used".as_ptr());
    LLVMSetInitializer(used, array);
    LLVMSetLinkage(used, LLVMLinkage::LLVMAppendingLinkage);
    LLVMSetSection(used, c"llvm.metadata".as_ptr());
}

pub(super) unsafe fn register_scalar_math(jit: LLVMOrcLLJITRef) -> Result<(), MirCodegenError> {
    use llvm_sys::orc2::{LLVMJITEvaluatedSymbol, LLVMJITSymbolFlags, LLVMJITSymbolGenericFlags};
    let mut symbols = scalar_symbols()
        .into_iter()
        .map(|(name, address)| LLVMOrcCSymbolMapPair {
            Name: LLVMOrcLLJITMangleAndIntern(jit, name.as_ptr()),
            Sym: LLVMJITEvaluatedSymbol {
                Address: address,
                Flags: LLVMJITSymbolFlags {
                    GenericFlags: LLVMJITSymbolGenericFlags::LLVMJITSymbolGenericFlagsExported
                        as u8
                        | LLVMJITSymbolGenericFlags::LLVMJITSymbolGenericFlagsCallable as u8,
                    TargetFlags: 0,
                },
            },
        })
        .collect::<Vec<_>>();
    let unit = LLVMOrcAbsoluteSymbols(symbols.as_mut_ptr(), symbols.len());
    let error = LLVMOrcJITDylibDefine(LLVMOrcLLJITGetMainJITDylib(jit), unit);
    if !error.is_null() {
        LLVMOrcDisposeMaterializationUnit(unit);
        return Err(codegen_diagnostic(
            super::super::jit_utils::llvm_error_to_diag(
                "failed to register scalar math for JIT tails",
                error,
            ),
        ));
    }
    Ok(())
}

include!(concat!(env!("OUT_DIR"), "/scalar_math_symbols.rs"));

macro_rules! kernel {
    ($isa:literal, $bits:literal) => {
        (
            $isa,
            $bits,
            include_bytes!(concat!("../../../math-kernel/", $isa, "-f", $bits, ".bc")) as &[u8],
        )
    };
}

const KERNELS: &[(&str, u32, &[u8])] = &[
    kernel!("sse2", 32),
    kernel!("sse2", 64),
    kernel!("avx", 32),
    kernel!("avx", 64),
    kernel!("avx2128", 32),
    kernel!("avx2128", 64),
    kernel!("avx2", 32),
    kernel!("avx2", 64),
    kernel!("avx512f", 32),
    kernel!("avx512f", 64),
    kernel!("advsimd", 32),
    kernel!("advsimd", 64),
];

unsafe fn function_name(function: LLVMValueRef) -> String {
    let mut len = 0;
    let name = LLVMGetValueName2(function, &mut len);
    String::from_utf8_lossy(std::slice::from_raw_parts(name.cast::<u8>(), len)).into_owned()
}

/// LLVM can introduce exp2 after VFABI mapping (notably for pow(2, x)).
/// Resolve only legal fixed-width variants after application optimization so
/// neither another intrinsic rewrite nor fast math can alter the helper body.
unsafe fn resolve_vector_exp2(module: LLVMModuleRef, target: LLVMTargetMachineRef) {
    let triple = CStr::from_ptr(LLVMGetTarget(module)).to_string_lossy();
    let library = Library::for_target(target, &triple);
    if !library.is_available() {
        return;
    }
    let (_, accuracy, _) = spec::FUNCTIONS
        .iter()
        .find(|entry| entry.0 == "exp2")
        .unwrap();
    let intrinsic = LLVMLookupIntrinsicID(c"llvm.exp2".as_ptr(), 9);
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        let next = LLVMGetNextFunction(function);
        if LLVMGetIntrinsicID(function) == intrinsic && !LLVMGetFirstUse(function).is_null() {
            let signature = LLVMGlobalGetValueType(function);
            let vector = LLVMGetReturnType(signature);
            if LLVMGetTypeKind(vector) == llvm_sys::LLVMTypeKind::LLVMVectorTypeKind {
                let scalar = match LLVMGetTypeKind(LLVMGetElementType(vector)) {
                    llvm_sys::LLVMTypeKind::LLVMFloatTypeKind => Some(('f', 4)),
                    llvm_sys::LLVMTypeKind::LLVMDoubleTypeKind => Some(('d', 8)),
                    _ => None,
                };
                if let Some((precision, bytes)) = scalar {
                    let lanes = LLVMGetVectorSize(vector);
                    if let Some(&(isa, _)) = library
                        .variants
                        .iter()
                        .flatten()
                        .find(|(_, width)| *width == lanes * bytes)
                    {
                        let name =
                            CString::new(format!("Sleef_exp2{precision}{lanes}_{accuracy}{isa}"))
                                .unwrap();
                        let mut helper = LLVMGetNamedFunction(module, name.as_ptr());
                        if helper.is_null() {
                            helper = LLVMAddFunction(module, name.as_ptr(), signature);
                        }
                        // Keep call-site FP flags and locations; link_helpers
                        // resolves the definition and sets its private ABI.
                        LLVMReplaceAllUsesWith(function, helper);
                    }
                }
            }
        }
        function = next;
    }
}

/// Link only ISA/precision modules actually used by the vectorizers. Application
/// optimization runs first so fast-math flags cannot reach SLEEF's arithmetic.
pub(super) unsafe fn link_helpers(
    module: LLVMModuleRef,
    target: LLVMTargetMachineRef,
) -> Result<(), MirCodegenError> {
    resolve_vector_exp2(module, target);
    let used = LLVMGetNamedGlobal(module, c"llvm.compiler.used".as_ptr());
    if !used.is_null() {
        LLVMDeleteGlobal(used);
    }
    let mut needed = std::collections::BTreeSet::new();
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        let name = function_name(function);
        if name.starts_with("Sleef_") && !LLVMGetFirstUse(function).is_null() {
            let ty = LLVMGetReturnType(LLVMGlobalGetValueType(function));
            let bits = match LLVMGetTypeKind(LLVMGetElementType(ty)) {
                llvm_sys::LLVMTypeKind::LLVMFloatTypeKind => 32,
                _ => 64,
            };
            let index = KERNELS
                .iter()
                .position(|(isa, precision, _)| name.ends_with(isa) && bits == *precision)
                .ok_or_else(|| {
                    MirCodegenError::llvm(format!("missing bundled math kernel: {name}"))
                })?;
            needed.insert(index);
        }
        // The mappings are no longer needed and may refer to discarded variants.
        let mut usage = LLVMGetFirstUse(function);
        while !usage.is_null() {
            let call = LLVMIsACallInst(LLVMGetUser(usage));
            if !call.is_null() {
                let key = "vector-function-abi-variant";
                LLVMRemoveCallSiteStringAttribute(
                    call,
                    llvm_sys::LLVMAttributeFunctionIndex,
                    key.as_ptr().cast(),
                    key.len() as u32,
                );
            }
            usage = LLVMGetNextUse(usage);
        }
        function = LLVMGetNextFunction(function);
    }
    for &index in &needed {
        let bytes = KERNELS[index].2;
        let buffer = LLVMCreateMemoryBufferWithMemoryRangeCopy(
            bytes.as_ptr().cast(),
            bytes.len(),
            c"onda-sleef".as_ptr(),
        );
        let mut kernel = null_mut();
        let status = LLVMParseBitcodeInContext2(LLVMGetModuleContext(module), buffer, &mut kernel);
        LLVMDisposeMemoryBuffer(buffer);
        if status != 0 {
            return Err(MirCodegenError::llvm(
                "failed to parse bundled SLEEF bitcode",
            ));
        }
        // Kernels contain only fixed-width vectors, constants and LLVM intrinsics.
        // Their private signatures use the destination ABI, never a host C ABI.
        LLVMSetTarget(kernel, LLVMGetTarget(module));
        LLVMSetDataLayout(kernel, LLVMGetDataLayoutStr(module));
        if LLVMLinkModules2(module, kernel) != 0 {
            return Err(MirCodegenError::llvm(
                "failed to link bundled SLEEF bitcode",
            ));
        }
    }
    let mut function = LLVMGetFirstFunction(module);
    while !function.is_null() {
        let next = LLVMGetNextFunction(function);
        if function_name(function).starts_with("Sleef_") {
            if LLVMIsDeclaration(function) != 0 {
                if !LLVMGetFirstUse(function).is_null() {
                    return Err(MirCodegenError::llvm("unresolved bundled SLEEF helper"));
                }
                LLVMDeleteFunction(function);
                function = next;
                continue;
            }
            LLVMSetLinkage(function, LLVMLinkage::LLVMInternalLinkage);
            let cc = llvm_sys::LLVMCallConv::LLVMFastCallConv as u32;
            LLVMSetFunctionCallConv(function, cc);
            let mut usage = LLVMGetFirstUse(function);
            while !usage.is_null() {
                let call = LLVMIsACallInst(LLVMGetUser(usage));
                if !call.is_null() {
                    LLVMSetInstructionCallConv(call, cc);
                }
                usage = LLVMGetNextUse(usage);
            }
        }
        function = next;
    }
    if !needed.is_empty() {
        let options = LLVMCreatePassBuilderOptions();
        let error = LLVMRunPasses(module, c"globaldce".as_ptr(), target, options);
        LLVMDisposePassBuilderOptions(options);
        if !error.is_null() {
            return Err(codegen_diagnostic(
                super::super::jit_utils::llvm_error_to_diag(
                    "failed to discard unused SLEEF helpers",
                    error,
                ),
            ));
        }
    }
    Ok(())
}

pub(super) unsafe fn emit_call(
    context: LLVMContextRef,
    module: LLVMModuleRef,
    builder: LLVMBuilderRef,
    library: Library,
    intrinsic: onda_mir::Intrinsic,
    scalar: ScalarType,
    args: &mut [LLVMValueRef],
) -> Result<Option<LLVMValueRef>, MirCodegenError> {
    let base = match intrinsic {
        onda_mir::Intrinsic::Sin => "sin",
        onda_mir::Intrinsic::Cos => "cos",
        onda_mir::Intrinsic::Tan => "tan",
        onda_mir::Intrinsic::Tanh => "tanh",
        onda_mir::Intrinsic::Atan => "atan",
        onda_mir::Intrinsic::Atan2 => "atan2",
        onda_mir::Intrinsic::Exp => "exp",
        onda_mir::Intrinsic::Log => "log",
        onda_mir::Intrinsic::Pow => "pow",
        _ => return Ok(None),
    };
    let name = if scalar == ScalarType::F32 {
        format!("{base}f")
    } else {
        base.to_owned()
    };
    let ty = llvm_scalar_type(context, scalar);
    let mut params = vec![ty; args.len()];
    let function_type = LLVMFunctionType(ty, params.as_mut_ptr(), params.len() as u32, 0);
    let function = ensure_named_function(module, &name, function_type)?;
    // Onda does not expose errno or floating exception flags. Pure math calls
    // can therefore be moved and vectorized without observable memory effects.
    for (attribute, value) in [("nounwind", 0), ("willreturn", 0), ("memory", 0)] {
        add_enum_attribute_at_index(
            context,
            function,
            llvm_sys::LLVMAttributeFunctionIndex,
            attribute,
            value,
        )?;
    }
    let call = LLVMBuildCall2(
        builder,
        function_type,
        function,
        args.as_mut_ptr(),
        args.len() as u32,
        c"math".as_ptr(),
    );
    let signature = "v".repeat(args.len());
    let (_, accuracy, _) = spec::FUNCTIONS
        .iter()
        .find(|entry| entry.0 == base)
        .unwrap();
    let mut mappings = Vec::new();
    for (isa, bytes) in library.variants.into_iter().flatten() {
        let lanes = bytes / if scalar == ScalarType::F32 { 4 } else { 8 };
        let precision = if scalar == ScalarType::F32 { 'f' } else { 'd' };
        let symbol = format!("Sleef_{base}{precision}{lanes}_{accuracy}{isa}");
        let vector_type = LLVMVectorType(ty, lanes);
        let mut params = vec![vector_type; args.len()];
        let function_type =
            LLVMFunctionType(vector_type, params.as_mut_ptr(), params.len() as u32, 0);
        let function = ensure_named_function(module, &symbol, function_type)?;
        for attribute in ["nounwind", "willreturn", "memory"] {
            add_enum_attribute_at_index(
                context,
                function,
                llvm_sys::LLVMAttributeFunctionIndex,
                attribute,
                0,
            )?;
        }
        mappings.push(format!("_ZGV_LLVM_N{lanes}{signature}_{name}({symbol})"));
    }
    let mappings = mappings.join(",");
    let key = "vector-function-abi-variant";
    let attribute = LLVMCreateStringAttribute(
        context,
        key.as_ptr().cast(),
        key.len() as u32,
        mappings.as_ptr().cast(),
        mappings.len() as u32,
    );
    LLVMAddCallSiteAttribute(call, llvm_sys::LLVMAttributeFunctionIndex, attribute);
    Ok(Some(call))
}
