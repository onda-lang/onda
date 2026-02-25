use super::*;

pub(super) unsafe fn create_aggressive_jit_target_machine_builder(
) -> Result<LLVMOrcJITTargetMachineBuilderRef, Diagnostic> {
    let tm = create_host_target_machine_aggressive()?;
    let jtmb = LLVMOrcJITTargetMachineBuilderCreateFromTargetMachine(tm);
    if jtmb.is_null() {
        return Err(Diagnostic::internal(
            "failed to create ORC JIT target machine builder from aggressive target machine",
        ));
    }
    Ok(jtmb)
}

pub(super) unsafe fn run_default_o3_pipeline(module: LLVMModuleRef) -> Result<(), Diagnostic> {
    let options = LLVMCreatePassBuilderOptions();
    if options.is_null() {
        return Err(Diagnostic::internal(
            "failed to create LLVM pass builder options",
        ));
    }

    let tm = match create_host_target_machine_aggressive() {
        Ok(v) => v,
        Err(diag) => {
            LLVMDisposePassBuilderOptions(options);
            return Err(diag);
        }
    };

    let pipeline = CString::new("default<O3>")
        .map_err(|_| Diagnostic::internal("invalid pass pipeline string"))?;
    let run_err = LLVMRunPasses(module, pipeline.as_ptr(), tm, options);
    LLVMDisposeTargetMachine(tm);
    LLVMDisposePassBuilderOptions(options);

    if !run_err.is_null() {
        return Err(llvm_error_to_diag(
            "failed to run LLVM pass pipeline default<O3>",
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

pub(super) unsafe fn create_host_target_machine_aggressive(
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
        LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
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
