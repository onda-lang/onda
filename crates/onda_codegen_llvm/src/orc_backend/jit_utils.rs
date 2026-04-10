use super::*;

pub(super) struct ResolvedTargetMachineConfig {
    pub(super) normalized_triple: String,
    pub(super) cpu: String,
    pub(super) features: String,
    pub(super) opt_level: LLVMCodeGenOptLevel,
    pub(super) reloc_mode: LLVMRelocMode,
    pub(super) code_model: LLVMCodeModel,
    pub(super) abi_name: Option<String>,
}

pub(super) unsafe fn create_aggressive_jit_target_machine_builder(
    opt_level: crate::TargetOptLevel,
) -> Result<LLVMOrcJITTargetMachineBuilderRef, Diagnostic> {
    let tm = create_host_target_machine(opt_level)?;
    let jtmb = LLVMOrcJITTargetMachineBuilderCreateFromTargetMachine(tm);
    if jtmb.is_null() {
        return Err(Diagnostic::internal(
            "failed to create ORC JIT target machine builder from aggressive target machine",
        ));
    }
    Ok(jtmb)
}

pub(super) unsafe fn run_default_pass_pipeline(
    module: LLVMModuleRef,
    tm: LLVMTargetMachineRef,
    opt_level: LLVMCodeGenOptLevel,
) -> Result<(), Diagnostic> {
    let options = LLVMCreatePassBuilderOptions();
    if options.is_null() {
        return Err(Diagnostic::internal(
            "failed to create LLVM pass builder options",
        ));
    }

    let pipeline = match opt_level {
        LLVMCodeGenOptLevel::LLVMCodeGenLevelNone => "default<O0>",
        LLVMCodeGenOptLevel::LLVMCodeGenLevelLess => "default<O1>",
        LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault => "default<O2>",
        LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive => "default<O3>",
    };
    let pipeline =
        CString::new(pipeline).map_err(|_| Diagnostic::internal("invalid pass pipeline string"))?;
    let run_err = LLVMRunPasses(module, pipeline.as_ptr(), tm, options);
    LLVMDisposePassBuilderOptions(options);

    if !run_err.is_null() {
        return Err(llvm_error_to_diag(
            &format!(
                "failed to run LLVM pass pipeline {}",
                pipeline.to_string_lossy()
            ),
            run_err,
        ));
    }
    Ok(())
}

pub(super) unsafe fn llvm_module_to_string(module: LLVMModuleRef) -> Result<String, Diagnostic> {
    let ir_ptr = LLVMPrintModuleToString(module);
    if ir_ptr.is_null() {
        return Err(Diagnostic::internal(
            "failed to print LLVM module to string",
        ));
    }
    let ir = CStr::from_ptr(ir_ptr).to_string_lossy().to_string();
    LLVMDisposeMessage(ir_ptr);
    Ok(ir)
}

pub(super) unsafe fn create_host_target_machine(
    opt_level: crate::TargetOptLevel,
) -> Result<LLVMTargetMachineRef, Diagnostic> {
    let triple = LLVMGetDefaultTargetTriple();
    if triple.is_null() {
        return Err(Diagnostic::internal(
            "LLVMGetDefaultTargetTriple returned null",
        ));
    }
    let cpu = LLVMGetHostCPUName();
    if cpu.is_null() {
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal("LLVMGetHostCPUName returned null"));
    }
    let features = LLVMGetHostCPUFeatures();
    if features.is_null() {
        LLVMDisposeMessage(cpu);
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal("LLVMGetHostCPUFeatures returned null"));
    }

    let mut target: LLVMTargetRef = null_mut();
    let mut target_err: *mut i8 = null_mut();
    let target_status = LLVMGetTargetFromTriple(triple as *const i8, &mut target, &mut target_err);
    if target_status != 0 {
        let detail = if target_err.is_null() {
            "unknown target lookup error".to_owned()
        } else {
            let msg = CStr::from_ptr(target_err).to_string_lossy().to_string();
            LLVMDisposeMessage(target_err);
            msg
        };
        LLVMDisposeMessage(features);
        LLVMDisposeMessage(cpu);
        LLVMDisposeMessage(triple);
        return Err(Diagnostic::internal(format!(
            "LLVMGetTargetFromTriple failed: {detail}"
        )));
    }

    let tm = LLVMCreateTargetMachine(
        target,
        triple as *const i8,
        cpu as *const i8,
        features as *const i8,
        map_opt_level(opt_level),
        LLVMRelocMode::LLVMRelocDefault,
        LLVMCodeModel::LLVMCodeModelJITDefault,
    );
    LLVMDisposeMessage(features);
    LLVMDisposeMessage(cpu);
    LLVMDisposeMessage(triple);

    if tm.is_null() {
        return Err(Diagnostic::internal(
            "LLVMCreateTargetMachine returned null for aggressive host setup",
        ));
    }

    Ok(tm)
}

pub(super) unsafe fn resolve_target_machine_config(
    target: &crate::TargetConfig,
) -> Result<ResolvedTargetMachineConfig, Diagnostic> {
    let host_triple = normalized_default_target_triple()?;
    let normalized_triple = match &target.triple {
        Some(triple) => normalize_target_triple(triple)?,
        None => host_triple.clone(),
    };

    let is_host_triple = normalized_triple == host_triple;
    let cpu = match &target.cpu {
        crate::TargetCpu::Host => {
            if !is_host_triple {
                return Err(Diagnostic::internal(format!(
                    "target cpu 'host' is only valid for the host triple '{}'",
                    host_triple
                )));
            }
            llvm_owned_message_to_string("LLVMGetHostCPUName", LLVMGetHostCPUName())?
        }
        crate::TargetCpu::Explicit(cpu) => cpu.clone(),
    };

    let features = match (&target.features, &target.cpu) {
        (Some(features), _) => features.clone(),
        (None, crate::TargetCpu::Host) => {
            if !is_host_triple {
                return Err(Diagnostic::internal(format!(
                    "target features default to host features only for the host triple '{}'",
                    host_triple
                )));
            }
            llvm_owned_message_to_string("LLVMGetHostCPUFeatures", LLVMGetHostCPUFeatures())?
        }
        (None, crate::TargetCpu::Explicit(_)) => String::new(),
    };

    Ok(ResolvedTargetMachineConfig {
        normalized_triple,
        cpu,
        features,
        opt_level: map_opt_level(target.opt_level),
        reloc_mode: map_reloc_mode(target.reloc_model),
        code_model: map_code_model(target.code_model),
        abi_name: target.abi_name.clone(),
    })
}

pub(super) unsafe fn create_target_machine_from_config(
    config: &ResolvedTargetMachineConfig,
) -> Result<LLVMTargetMachineRef, Diagnostic> {
    let triple = CString::new(config.normalized_triple.as_str())
        .map_err(|_| Diagnostic::internal("invalid target triple"))?;
    let mut target: LLVMTargetRef = null_mut();
    let mut target_err: *mut i8 = null_mut();
    let target_status = LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut target_err);
    if target_status != 0 {
        let detail = if target_err.is_null() {
            "unknown target lookup error".to_owned()
        } else {
            let msg = CStr::from_ptr(target_err).to_string_lossy().to_string();
            LLVMDisposeMessage(target_err);
            msg
        };
        return Err(Diagnostic::internal(format!(
            "LLVMGetTargetFromTriple failed for '{}': {detail}",
            config.normalized_triple
        )));
    }

    let options = LLVMCreateTargetMachineOptions();
    if options.is_null() {
        return Err(Diagnostic::internal(
            "failed to create LLVM target machine options",
        ));
    }

    let cpu = CString::new(config.cpu.as_str())
        .map_err(|_| Diagnostic::internal("invalid target cpu"))?;
    let features = CString::new(config.features.as_str())
        .map_err(|_| Diagnostic::internal("invalid target feature string"))?;
    LLVMTargetMachineOptionsSetCPU(options, cpu.as_ptr());
    LLVMTargetMachineOptionsSetFeatures(options, features.as_ptr());
    LLVMTargetMachineOptionsSetCodeGenOptLevel(options, config.opt_level);
    LLVMTargetMachineOptionsSetRelocMode(options, config.reloc_mode);
    LLVMTargetMachineOptionsSetCodeModel(options, config.code_model);

    let abi_cstring = if let Some(abi_name) = &config.abi_name {
        Some(
            CString::new(abi_name.as_str())
                .map_err(|_| Diagnostic::internal("invalid target abi name"))?,
        )
    } else {
        None
    };
    if let Some(abi_name) = &abi_cstring {
        LLVMTargetMachineOptionsSetABI(options, abi_name.as_ptr());
    }

    let tm = LLVMCreateTargetMachineWithOptions(target, triple.as_ptr(), options);
    LLVMDisposeTargetMachineOptions(options);

    if tm.is_null() {
        return Err(Diagnostic::internal(format!(
            "LLVMCreateTargetMachineWithOptions returned null for target '{}'",
            config.normalized_triple
        )));
    }

    Ok(tm)
}

pub(super) unsafe fn target_machine_triple_string(
    tm: LLVMTargetMachineRef,
) -> Result<String, Diagnostic> {
    llvm_owned_message_to_string("LLVMGetTargetMachineTriple", LLVMGetTargetMachineTriple(tm))
}

pub(super) unsafe fn target_machine_data_layout_string(
    tm: LLVMTargetMachineRef,
) -> Result<String, Diagnostic> {
    let target_data = LLVMCreateTargetDataLayout(tm);
    if target_data.is_null() {
        return Err(Diagnostic::internal(
            "LLVMCreateTargetDataLayout returned null",
        ));
    }
    let data_layout = LLVMCopyStringRepOfTargetData(target_data);
    LLVMDisposeTargetData(target_data);
    llvm_owned_message_to_string("LLVMCopyStringRepOfTargetData", data_layout)
}

pub(super) unsafe fn llvm_error_to_diag(prefix: &str, err: LLVMErrorRef) -> Diagnostic {
    if err.is_null() {
        return Diagnostic::internal(prefix);
    }
    let msg_ptr = LLVMGetErrorMessage(err);
    if msg_ptr.is_null() {
        return Diagnostic::internal(prefix);
    }
    let msg = CStr::from_ptr(msg_ptr).to_string_lossy().to_string();
    LLVMDisposeErrorMessage(msg_ptr);
    Diagnostic::internal(format!("{prefix}: {msg}"))
}

pub(super) unsafe fn lookup_symbol(
    lljit: LLVMOrcLLJITRef,
    symbol: &str,
    what: &str,
) -> Result<LLVMOrcExecutorAddress, Diagnostic> {
    let symbol_name =
        CString::new(symbol).map_err(|_| Diagnostic::internal(format!("invalid {what} symbol")))?;
    let mut address: LLVMOrcExecutorAddress = 0;
    let lookup_err = LLVMOrcLLJITLookup(lljit, &mut address, symbol_name.as_ptr());
    if !lookup_err.is_null() {
        return Err(llvm_error_to_diag(
            &format!("failed to lookup compiled {what}"),
            lookup_err,
        ));
    }
    if address == 0 {
        return Err(Diagnostic::internal(format!(
            "LLJIT lookup returned null address for {what}"
        )));
    }
    Ok(address)
}

pub(super) unsafe fn dispose_lljit_quiet(lljit: LLVMOrcLLJITRef) {
    if lljit.is_null() {
        return;
    }
    let err = LLVMOrcDisposeLLJIT(lljit);
    if err.is_null() {
        return;
    }
    let msg_ptr = LLVMGetErrorMessage(err);
    if !msg_ptr.is_null() {
        LLVMDisposeErrorMessage(msg_ptr);
    }
}

unsafe fn normalized_default_target_triple() -> Result<String, Diagnostic> {
    let triple_ptr = LLVMGetDefaultTargetTriple();
    if triple_ptr.is_null() {
        return Err(Diagnostic::internal(
            "LLVMGetDefaultTargetTriple returned null",
        ));
    }
    let triple = CStr::from_ptr(triple_ptr).to_string_lossy().to_string();
    LLVMDisposeMessage(triple_ptr);
    normalize_target_triple(&triple)
}

unsafe fn normalize_target_triple(triple: &str) -> Result<String, Diagnostic> {
    let triple = CString::new(triple).map_err(|_| Diagnostic::internal("invalid target triple"))?;
    let normalized = LLVMNormalizeTargetTriple(triple.as_ptr());
    llvm_owned_message_to_string("LLVMNormalizeTargetTriple", normalized)
}

unsafe fn llvm_owned_message_to_string(what: &str, ptr: *mut i8) -> Result<String, Diagnostic> {
    if ptr.is_null() {
        return Err(Diagnostic::internal(format!("{what} returned null")));
    }
    let value = CStr::from_ptr(ptr).to_string_lossy().to_string();
    LLVMDisposeMessage(ptr);
    Ok(value)
}

pub(super) fn map_opt_level(level: crate::TargetOptLevel) -> LLVMCodeGenOptLevel {
    match level {
        crate::TargetOptLevel::O0 => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
        crate::TargetOptLevel::O1 => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
        crate::TargetOptLevel::O2 => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
        crate::TargetOptLevel::O3 => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
    }
}

fn map_reloc_mode(mode: crate::TargetRelocMode) -> LLVMRelocMode {
    match mode {
        crate::TargetRelocMode::Default => LLVMRelocMode::LLVMRelocDefault,
        crate::TargetRelocMode::Static => LLVMRelocMode::LLVMRelocStatic,
        crate::TargetRelocMode::Pic => LLVMRelocMode::LLVMRelocPIC,
        crate::TargetRelocMode::DynamicNoPic => LLVMRelocMode::LLVMRelocDynamicNoPic,
    }
}

fn map_code_model(model: crate::TargetCodeModel) -> LLVMCodeModel {
    match model {
        crate::TargetCodeModel::Default => LLVMCodeModel::LLVMCodeModelDefault,
        crate::TargetCodeModel::Small => LLVMCodeModel::LLVMCodeModelSmall,
        crate::TargetCodeModel::Kernel => LLVMCodeModel::LLVMCodeModelKernel,
        crate::TargetCodeModel::Medium => LLVMCodeModel::LLVMCodeModelMedium,
        crate::TargetCodeModel::Large => LLVMCodeModel::LLVMCodeModelLarge,
    }
}
