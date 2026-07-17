use super::*;

struct PreparedCodegenLayouts {
    top_level_oversampling_layout: TopLevelOversamplingLayout,
    proc_slot_buffer_ref_layouts: Vec<ProcSlotBufferRefLayout>,
    state_total_size_bytes: usize,
}

fn const_array_info_map(typed: &TypedProgram) -> HashMap<String, TypedArrayInfo> {
    typed
        .const_arrays
        .iter()
        .map(|array| {
            (
                array.name.clone(),
                TypedArrayInfo {
                    elem_ty: array.elem_ty,
                    len: array.len,
                    offset: 0,
                },
            )
        })
        .collect()
}

unsafe fn llvm_const_for_typed_value(
    context: LLVMContextRef,
    value: TypedConstValue,
    ty: PrimitiveType,
) -> Result<LLVMValueRef, Diagnostic> {
    Ok(match (ty, value) {
        (PrimitiveType::F32, TypedConstValue::F32(v)) => {
            LLVMConstReal(llvm_ty_for_primitive(context, ty), v as f64)
        }
        (PrimitiveType::F64, TypedConstValue::F64(v)) => {
            LLVMConstReal(llvm_ty_for_primitive(context, ty), v)
        }
        (PrimitiveType::I32, TypedConstValue::I32(v)) => {
            LLVMConstInt(llvm_ty_for_primitive(context, ty), v as u64, 1)
        }
        (PrimitiveType::I64, TypedConstValue::I64(v)) => {
            LLVMConstInt(llvm_ty_for_primitive(context, ty), v as u64, 1)
        }
        (PrimitiveType::Bool, TypedConstValue::Bool(v)) => {
            LLVMConstInt(llvm_ty_for_primitive(context, ty), u64::from(v), 0)
        }
        (expected, actual) => {
            return Err(Diagnostic::internal(format!(
                "const array value type mismatch: expected {:?}, got {:?}",
                expected, actual
            )));
        }
    })
}

unsafe fn build_const_array_globals(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
) -> Result<HashMap<String, LLVMValueRef>, Diagnostic> {
    let mut out = HashMap::new();
    let i32_ty = LLVMInt32TypeInContext(context);
    for array in &typed.const_arrays {
        let elem_ty = llvm_ty_for_primitive(context, array.elem_ty);
        let array_ty = LLVMArrayType2(elem_ty, array.len as u64);
        let mut values = Vec::with_capacity(array.values.len());
        for value in &array.values {
            values.push(llvm_const_for_typed_value(context, *value, array.elem_ty)?);
        }
        let init = LLVMConstArray2(elem_ty, values.as_mut_ptr(), values.len() as u64);
        let llvm_name = CString::new(format!(
            "__onda_const_array_{}",
            sanitize_runtime_symbol_component(&array.name)
        ))
        .map_err(|_| Diagnostic::internal("invalid const array global name"))?;
        let global = LLVMAddGlobal(module, array_ty, llvm_name.as_ptr());
        LLVMSetInitializer(global, init);
        LLVMSetGlobalConstant(global, 1);
        LLVMSetLinkage(global, llvm_sys::LLVMLinkage::LLVMInternalLinkage);
        let mut indices = [LLVMConstInt(i32_ty, 0, 0), LLVMConstInt(i32_ty, 0, 0)];
        let base_ptr = LLVMConstInBoundsGEP2(array_ty, global, indices.as_mut_ptr(), 2);
        out.insert(array.name.clone(), base_ptr);
    }
    Ok(out)
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

#[cfg(test)]
pub(crate) fn emit_legacy_artifacts(
    typed: &TypedProgram,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    opt_level: crate::TargetOptLevel,
) -> Result<(String, Vec<u8>), Diagnostic> {
    initialize_native_target()?;
    let layouts = prepare_codegen_layouts(typed)?;
    unsafe {
        let target_machine = create_host_target_machine(opt_level)?;
        let result = (|| {
            let triple = target_machine_triple_string(target_machine)?;
            let data_layout = target_machine_data_layout_string(target_machine)?;
            let (module, context) = build_optimized_module(
                typed,
                sample_rate,
                block_size,
                fast_math,
                &layouts.top_level_oversampling_layout,
                &layouts.proc_slot_buffer_ref_layouts,
                &triple,
                &data_layout,
                target_machine,
                map_opt_level(opt_level),
            )?;
            let artifacts = (|| {
                let ir = llvm_module_to_string(module)?;
                let mut error_message = null_mut();
                let mut memory_buffer = null_mut();
                let failed = LLVMTargetMachineEmitToMemoryBuffer(
                    target_machine,
                    module,
                    LLVMCodeGenFileType::LLVMObjectFile,
                    &mut error_message,
                    &mut memory_buffer,
                );
                if failed != 0 || memory_buffer.is_null() {
                    let detail = if error_message.is_null() {
                        "unknown LLVM object emission error".to_owned()
                    } else {
                        let message = CStr::from_ptr(error_message).to_string_lossy().into_owned();
                        LLVMDisposeMessage(error_message);
                        message
                    };
                    return Err(Diagnostic::internal(format!(
                        "legacy LLVM object emission failed: {detail}"
                    )));
                }
                let start = LLVMGetBufferStart(memory_buffer).cast::<u8>();
                let size = LLVMGetBufferSize(memory_buffer);
                let object = if start.is_null() || size == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(start, size).to_vec()
                };
                LLVMDisposeMemoryBuffer(memory_buffer);
                if !error_message.is_null() {
                    LLVMDisposeMessage(error_message);
                }
                Ok((ir, object))
            })();
            LLVMDisposeModule(module);
            LLVMContextDispose(context);
            artifacts
        })();
        LLVMDisposeTargetMachine(target_machine);
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

    let ts_context = llvm_orc_create_new_thread_safe_context_from_llvm_context(context);
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
        let const_arrays = const_array_info_map(typed);
        let const_array_base_ptrs = build_const_array_globals(typed, module, context)?;
        let mut user_fns = build_user_functions_ir(
            typed,
            module,
            context,
            &const_array_base_ptrs,
            sample_rate,
            block_size,
            fast_math,
        )?;

        build_init_ir(
            typed,
            module,
            context,
            &mut user_fns,
            &const_arrays,
            &const_array_base_ptrs,
            sample_rate,
            block_size,
            fast_math,
        )?;
        build_process_ir(
            typed,
            module,
            context,
            &mut user_fns,
            &const_arrays,
            &const_array_base_ptrs,
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
            &const_arrays,
            &const_array_base_ptrs,
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
