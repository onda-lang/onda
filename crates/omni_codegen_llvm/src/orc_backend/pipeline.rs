use super::*;

pub(crate) fn compile_orc(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<OrcProcess, Diagnostic> {
    initialize_native_target()?;
    let state_layout = compute_state_layout(typed)?;
    let arrays_layout = compute_arrays_layout(typed, &state_layout)?;
    let base_state_size_bytes = state_total_size_bytes(&state_layout, &arrays_layout);
    let top_level_oversampling_layout =
        compute_top_level_oversampling_layout(typed, base_state_size_bytes)?;
    let (proc_slot_buffer_ref_layouts, state_total_size_bytes) =
        compute_proc_slot_buffer_ref_layouts(
            typed,
            top_level_oversampling_layout.state_size_bytes,
        )?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }

        let jtmb = match create_aggressive_jit_target_machine_builder() {
            Ok(v) => v,
            Err(diag) => {
                LLVMOrcDisposeLLJITBuilder(builder);
                return Err(diag);
            }
        };
        if jtmb.is_null() {
            LLVMOrcDisposeLLJITBuilder(builder);
            return Err(Diagnostic::internal(
                "failed to create aggressive JIT target machine builder",
            ));
        }
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder, jtmb);

        let mut lljit: LLVMOrcLLJITRef = null_mut();
        let lljit_err = LLVMOrcCreateLLJIT(&mut lljit, builder);
        if !lljit_err.is_null() {
            return Err(llvm_error_to_diag(
                "failed to create LLJIT instance",
                lljit_err,
            ));
        }
        if lljit.is_null() {
            return Err(Diagnostic::internal(
                "LLJIT creation returned null without error",
            ));
        }

        let (process_fn, init_fn, event_fns) = match compile_module_into_jit(
            lljit,
            typed,
            sample_rate,
            block_size,
            fast_math,
            &top_level_oversampling_layout,
            &proc_slot_buffer_ref_layouts,
        ) {
            Ok(fns) => fns,
            Err(diag) => {
                dispose_lljit_quiet(lljit);
                return Err(diag);
            }
        };

        Ok(OrcProcess::new(
            lljit,
            process_fn,
            init_fn,
            event_fns,
            typed
                .params
                .iter()
                .map(|param| primitive_type_bytes(param.ty))
                .sum(),
            typed.ins.len(),
            typed.outs.len(),
            typed.buffers.len(),
            state_total_size_bytes,
            proc_slot_buffer_ref_layouts,
        ))
    }
}

pub(crate) fn emit_optimized_ir(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<String, Diagnostic> {
    initialize_native_target()?;
    let state_layout = compute_state_layout(typed)?;
    let arrays_layout = compute_arrays_layout(typed, &state_layout)?;
    let base_state_size_bytes = state_total_size_bytes(&state_layout, &arrays_layout);
    let top_level_oversampling_layout =
        compute_top_level_oversampling_layout(typed, base_state_size_bytes)?;
    let (proc_slot_buffer_ref_layouts, _state_total_size_bytes) =
        compute_proc_slot_buffer_ref_layouts(
            typed,
            top_level_oversampling_layout.state_size_bytes,
        )?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }
        let jtmb = match create_aggressive_jit_target_machine_builder() {
            Ok(v) => v,
            Err(diag) => {
                LLVMOrcDisposeLLJITBuilder(builder);
                return Err(diag);
            }
        };
        if jtmb.is_null() {
            LLVMOrcDisposeLLJITBuilder(builder);
            return Err(Diagnostic::internal(
                "failed to create aggressive JIT target machine builder",
            ));
        }
        LLVMOrcLLJITBuilderSetJITTargetMachineBuilder(builder, jtmb);

        let mut lljit: LLVMOrcLLJITRef = null_mut();
        let lljit_err = LLVMOrcCreateLLJIT(&mut lljit, builder);
        if !lljit_err.is_null() {
            return Err(llvm_error_to_diag(
                "failed to create LLJIT instance",
                lljit_err,
            ));
        }
        if lljit.is_null() {
            return Err(Diagnostic::internal(
                "LLJIT creation returned null without error",
            ));
        }

        let result = (|| {
            let (module, ts_context) = build_optimized_module_for_jit(
                lljit,
                typed,
                sample_rate,
                block_size,
                fast_math,
                &top_level_oversampling_layout,
                &proc_slot_buffer_ref_layouts,
            )?;
            let ir = llvm_module_to_string(module)?;
            LLVMDisposeModule(module);
            LLVMOrcDisposeThreadSafeContext(ts_context);
            Ok(ir)
        })();

        dispose_lljit_quiet(lljit);
        result
    }
}

fn initialize_native_target() -> Result<(), Diagnostic> {
    let init_result = NATIVE_INIT_ERR.get_or_init(|| unsafe {
        if LLVM_InitializeNativeTarget() != 0 {
            return Some("LLVM_InitializeNativeTarget failed".to_owned());
        }
        if LLVM_InitializeNativeAsmPrinter() != 0 {
            return Some("LLVM_InitializeNativeAsmPrinter failed".to_owned());
        }
        if LLVM_InitializeNativeAsmParser() != 0 {
            return Some("LLVM_InitializeNativeAsmParser failed".to_owned());
        }
        None
    });

    match init_result {
        Some(msg) => Err(Diagnostic::internal(msg.clone())),
        None => Ok(()),
    }
}

unsafe fn compile_module_into_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    top_level_oversampling_layout: &TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(OrcProcessFn, OrcInitFn, Vec<OrcEventFn>), Diagnostic> {
    let (module, ts_context) = build_optimized_module_for_jit(
        lljit,
        typed,
        sample_rate,
        block_size,
        fast_math,
        top_level_oversampling_layout,
        proc_slot_buffer_ref_layouts,
    )?;

    let thread_safe_module = LLVMOrcCreateNewThreadSafeModule(module, ts_context);
    if thread_safe_module.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal(
            "failed to create ORC thread-safe module",
        ));
    }

    let add_err = LLVMOrcLLJITAddLLVMIRModule(
        lljit,
        LLVMOrcLLJITGetMainJITDylib(lljit),
        thread_safe_module,
    );
    if !add_err.is_null() {
        LLVMOrcDisposeThreadSafeModule(thread_safe_module);
        return Err(llvm_error_to_diag(
            "failed to add LLVM IR module to LLJIT",
            add_err,
        ));
    }

    let process_addr = lookup_symbol(lljit, "omni_process", "process function")?;
    let init_addr = lookup_symbol(lljit, "omni_init", "init function")?;
    let mut event_fns = Vec::<OrcEventFn>::new();
    for event_idx in 0..typed.events.len() {
        let symbol_name = format!("omni_event_{event_idx}");
        let event_addr = lookup_symbol(lljit, &symbol_name, "event function")?;
        event_fns.push(std::mem::transmute::<usize, OrcEventFn>(
            event_addr as usize,
        ));
    }
    Ok((
        std::mem::transmute::<usize, OrcProcessFn>(process_addr as usize),
        std::mem::transmute::<usize, OrcInitFn>(init_addr as usize),
        event_fns,
    ))
}

unsafe fn build_optimized_module_for_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    top_level_oversampling_layout: &TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(LLVMModuleRef, LLVMOrcThreadSafeContextRef), Diagnostic> {
    let context = LLVMContextCreate();
    if context.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM context"));
    }
    let ts_context = LLVMOrcCreateNewThreadSafeContextFromLLVMContext(context);
    if ts_context.is_null() {
        LLVMContextDispose(context);
        return Err(Diagnostic::internal(
            "failed to create LLVM ORC thread-safe context",
        ));
    }

    let module_name =
        CString::new("omni_module").map_err(|_| Diagnostic::internal("invalid module name"))?;
    let module = LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
    if module.is_null() {
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to create LLVM module"));
    }

    let triple = LLVMOrcLLJITGetTripleString(lljit);
    if triple.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to get JIT target triple"));
    }
    LLVMSetTarget(module, triple);

    let datalayout = LLVMOrcLLJITGetDataLayoutStr(lljit);
    if datalayout.is_null() {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal("failed to get JIT data layout"));
    }
    LLVMSetDataLayout(module, datalayout);

    let mut user_fns =
        match build_user_functions_ir(typed, module, context, sample_rate, block_size, fast_math) {
            Ok(v) => v,
            Err(diag) => {
                LLVMDisposeModule(module);
                LLVMOrcDisposeThreadSafeContext(ts_context);
                return Err(diag);
            }
        };

    if let Err(diag) = build_init_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = build_process_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
        top_level_oversampling_layout,
        proc_slot_buffer_ref_layouts,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = build_event_ir(
        typed,
        module,
        context,
        &mut user_fns,
        sample_rate,
        block_size,
        fast_math,
    ) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }
    if let Err(diag) = run_default_o3_pipeline(module) {
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(diag);
    }

    let mut verify_message: *mut i8 = null_mut();
    let verify_failed = LLVMVerifyModule(
        module,
        LLVMVerifierFailureAction::LLVMReturnStatusAction,
        &mut verify_message,
    );
    if verify_failed != 0 {
        let detail = if verify_message.is_null() {
            "unknown LLVM verification error".to_owned()
        } else {
            let msg = CStr::from_ptr(verify_message).to_string_lossy().to_string();
            LLVMDisposeMessage(verify_message);
            msg
        };
        LLVMDisposeModule(module);
        LLVMOrcDisposeThreadSafeContext(ts_context);
        return Err(Diagnostic::internal(format!(
            "LLVM module verification failed: {detail}"
        )));
    }
    if !verify_message.is_null() {
        LLVMDisposeMessage(verify_message);
    }

    Ok((module, ts_context))
}
