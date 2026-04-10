use super::*;

struct PreparedCodegenLayouts {
    top_level_oversampling_layout: TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: Vec<ProcSlotBufferRefLayout>,
    state_total_size_bytes: usize,
}

pub(crate) fn compile_orc(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: crate::TargetOptLevel,
) -> Result<OrcProcess, Diagnostic> {
    initialize_native_target()?;
    let layouts = prepare_codegen_layouts(typed)?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }

        let jtmb = match create_aggressive_jit_target_machine_builder(opt_level) {
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
            opt_level,
            &layouts.top_level_oversampling_layout,
            &layouts.proc_slot_buffer_ref_layouts,
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
            layouts.state_total_size_bytes,
            layouts.proc_slot_buffer_ref_layouts,
        ))
    }
}

pub(crate) fn emit_optimized_ir(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: crate::TargetOptLevel,
) -> Result<String, Diagnostic> {
    initialize_native_target()?;
    let layouts = prepare_codegen_layouts(typed)?;

    unsafe {
        let builder = LLVMOrcCreateLLJITBuilder();
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLJIT builder"));
        }
        let jtmb = match create_aggressive_jit_target_machine_builder(opt_level) {
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
            let (module, context) = build_optimized_module_for_jit(
                lljit,
                typed,
                sample_rate,
                block_size,
                fast_math,
                opt_level,
                &layouts.top_level_oversampling_layout,
                &layouts.proc_slot_buffer_ref_layouts,
            )?;
            let ir = llvm_module_to_string(module)?;
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            Ok(ir)
        })();

        dispose_lljit_quiet(lljit);
        result
    }
}

pub(crate) fn emit_targeted_ir(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: &crate::TargetConfig,
) -> Result<String, Diagnostic> {
    initialize_codegen_targets()?;
    let layouts = prepare_codegen_layouts(typed)?;

    unsafe {
        let resolved = resolve_target_machine_config(target)?;
        let tm = create_target_machine_from_config(&resolved)?;
        let result = (|| {
            let target_triple = target_machine_triple_string(tm)?;
            let data_layout = target_machine_data_layout_string(tm)?;
            let (module, context) = build_optimized_module(
                typed,
                sample_rate,
                block_size,
                fast_math,
                &layouts.top_level_oversampling_layout,
                &layouts.proc_slot_buffer_ref_layouts,
                &target_triple,
                &data_layout,
                tm,
                resolved.opt_level,
            )?;
            let ir = llvm_module_to_string(module)?;
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            Ok(ir)
        })();
        LLVMDisposeTargetMachine(tm);
        result
    }
}

pub(crate) fn emit_targeted_object(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    target: &crate::TargetConfig,
) -> Result<crate::AotObjectArtifact, Diagnostic> {
    initialize_codegen_targets()?;
    let layouts = prepare_codegen_layouts(typed)?;

    unsafe {
        let resolved = resolve_target_machine_config(target)?;
        let tm = create_target_machine_from_config(&resolved)?;
        let result = (|| {
            let target_triple = target_machine_triple_string(tm)?;
            let data_layout = target_machine_data_layout_string(tm)?;
            let (module, context) = build_optimized_module(
                typed,
                sample_rate,
                block_size,
                fast_math,
                &layouts.top_level_oversampling_layout,
                &layouts.proc_slot_buffer_ref_layouts,
                &target_triple,
                &data_layout,
                tm,
                resolved.opt_level,
            )?;
            let object_bytes = emit_object_to_memory_buffer(tm, module)?;
            let metadata = crate::aot_artifact::build_aot_metadata(
                typed,
                sample_rate,
                block_size,
                fast_math,
                target,
                resolved.normalized_triple.clone(),
                resolved.cpu.clone(),
                resolved.features.clone(),
                layouts.state_total_size_bytes,
            );
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            Ok(crate::AotObjectArtifact {
                object_bytes,
                metadata,
            })
        })();
        LLVMDisposeTargetMachine(tm);
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

fn initialize_codegen_targets() -> Result<(), Diagnostic> {
    CODEGEN_TARGETS_INIT.get_or_init(|| unsafe {
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmPrinters();
        LLVM_InitializeAllAsmParsers();
    });
    Ok(())
}

fn prepare_codegen_layouts(typed: &TypedProgram) -> Result<PreparedCodegenLayouts, Diagnostic> {
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

    Ok(PreparedCodegenLayouts {
        top_level_oversampling_layout,
        proc_slot_buffer_ref_layouts,
        state_total_size_bytes,
    })
}

unsafe fn compile_module_into_jit(
    lljit: LLVMOrcLLJITRef,
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: crate::TargetOptLevel,
    top_level_oversampling_layout: &TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(OrcProcessFn, OrcInitFn, Vec<OrcEventFn>), Diagnostic> {
    let (module, context) = build_optimized_module_for_jit(
        lljit,
        typed,
        sample_rate,
        block_size,
        fast_math,
        opt_level,
        top_level_oversampling_layout,
        proc_slot_buffer_ref_layouts,
    )?;

    let ts_context = LLVMOrcCreateNewThreadSafeContextFromLLVMContext(context);
    if ts_context.is_null() {
        LLVMDisposeModule(module);
        LLVMContextDispose(context);
        return Err(Diagnostic::internal(
            "failed to create LLVM ORC thread-safe context",
        ));
    }

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

    let process_addr = lookup_symbol(lljit, "onda_process", "process function")?;
    let init_addr = lookup_symbol(lljit, "onda_init", "init function")?;
    let mut event_fns = Vec::<OrcEventFn>::new();
    for event_idx in 0..typed.events.len() {
        let symbol_name = format!("onda_event_{event_idx}");
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
    opt_level: crate::TargetOptLevel,
    top_level_oversampling_layout: &TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
) -> Result<(LLVMModuleRef, LLVMContextRef), Diagnostic> {
    let triple = LLVMOrcLLJITGetTripleString(lljit);
    if triple.is_null() {
        return Err(Diagnostic::internal("failed to get JIT target triple"));
    }
    let target_triple = CStr::from_ptr(triple).to_string_lossy().to_string();

    let datalayout = LLVMOrcLLJITGetDataLayoutStr(lljit);
    if datalayout.is_null() {
        return Err(Diagnostic::internal("failed to get JIT data layout"));
    }
    let data_layout = CStr::from_ptr(datalayout).to_string_lossy().to_string();

    let opt_tm = create_host_target_machine(opt_level)?;
    let result = build_optimized_module(
        typed,
        sample_rate,
        block_size,
        fast_math,
        top_level_oversampling_layout,
        proc_slot_buffer_ref_layouts,
        &target_triple,
        &data_layout,
        opt_tm,
        map_opt_level(opt_level),
    );
    LLVMDisposeTargetMachine(opt_tm);
    result
}

unsafe fn build_optimized_module(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    top_level_oversampling_layout: &TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: &[ProcSlotBufferRefLayout],
    target_triple: &str,
    data_layout: &str,
    opt_tm: LLVMTargetMachineRef,
    opt_level: LLVMCodeGenOptLevel,
) -> Result<(LLVMModuleRef, LLVMContextRef), Diagnostic> {
    let context = LLVMContextCreate();
    if context.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM context"));
    }

    let module_name =
        CString::new("onda_module").map_err(|_| Diagnostic::internal("invalid module name"))?;
    let module = LLVMModuleCreateWithNameInContext(module_name.as_ptr(), context);
    if module.is_null() {
        LLVMContextDispose(context);
        return Err(Diagnostic::internal("failed to create LLVM module"));
    }

    let target_triple =
        CString::new(target_triple).map_err(|_| Diagnostic::internal("invalid target triple"))?;
    LLVMSetTarget(module, target_triple.as_ptr());

    let data_layout =
        CString::new(data_layout).map_err(|_| Diagnostic::internal("invalid data layout"))?;
    LLVMSetDataLayout(module, data_layout.as_ptr());

    let result = (|| {
        let mut user_fns =
            build_user_functions_ir(typed, module, context, sample_rate, block_size, fast_math)?;

        build_init_ir(
            typed,
            module,
            context,
            &mut user_fns,
            sample_rate,
            block_size,
            fast_math,
        )?;
        build_process_ir(
            typed,
            module,
            context,
            &mut user_fns,
            sample_rate,
            block_size,
            fast_math,
            top_level_oversampling_layout,
            proc_slot_buffer_ref_layouts,
        )?;
        build_event_ir(
            typed,
            module,
            context,
            &mut user_fns,
            sample_rate,
            block_size,
            fast_math,
        )?;
        run_default_pass_pipeline(module, opt_tm, opt_level)?;
        verify_module(module)?;
        Ok(())
    })();

    if let Err(diag) = result {
        LLVMDisposeModule(module);
        LLVMContextDispose(context);
        return Err(diag);
    }

    Ok((module, context))
}

unsafe fn verify_module(module: LLVMModuleRef) -> Result<(), Diagnostic> {
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
        return Err(Diagnostic::internal(format!(
            "LLVM module verification failed: {detail}"
        )));
    }
    if !verify_message.is_null() {
        LLVMDisposeMessage(verify_message);
    }
    Ok(())
}

unsafe fn emit_object_to_memory_buffer(
    tm: LLVMTargetMachineRef,
    module: LLVMModuleRef,
) -> Result<Vec<u8>, Diagnostic> {
    let mut error_message: *mut i8 = null_mut();
    let mut out_mem_buf: LLVMMemoryBufferRef = null_mut();
    let emit_status = LLVMTargetMachineEmitToMemoryBuffer(
        tm,
        module,
        LLVMCodeGenFileType::LLVMObjectFile,
        &mut error_message,
        &mut out_mem_buf,
    );
    if emit_status != 0 {
        let detail = if error_message.is_null() {
            "unknown object emission error".to_owned()
        } else {
            let msg = CStr::from_ptr(error_message).to_string_lossy().to_string();
            LLVMDisposeMessage(error_message);
            msg
        };
        return Err(Diagnostic::internal(format!(
            "LLVM target machine object emission failed: {detail}"
        )));
    }
    if out_mem_buf.is_null() {
        if !error_message.is_null() {
            LLVMDisposeMessage(error_message);
        }
        return Err(Diagnostic::internal(
            "LLVM target machine object emission returned a null memory buffer",
        ));
    }

    let start = LLVMGetBufferStart(out_mem_buf) as *const u8;
    let size = LLVMGetBufferSize(out_mem_buf);
    let bytes = if start.is_null() || size == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(start, size).to_vec()
    };
    LLVMDisposeMemoryBuffer(out_mem_buf);
    if !error_message.is_null() {
        LLVMDisposeMessage(error_message);
    }
    Ok(bytes)
}
