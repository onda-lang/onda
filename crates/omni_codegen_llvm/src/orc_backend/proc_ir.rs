use super::*;

fn oversample_iir_coeff(factor: usize) -> f64 {
    if factor <= 1 {
        return 1.0;
    }
    0.95
}

pub(super) unsafe fn build_process_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i32_ptr_ty = LLVMPointerType(i32_ty, 0);
    let i8_ptr_ptr_ty = LLVMPointerType(i8_ptr_ty, 0);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = i8_ptr_ptr_ty;
    let fast_math_flags = fast_math_flags(fast_math);

    let mut arg_types = [
        float_ptr_ptr_ty,
        float_ptr_ptr_ty,
        i32_ty,
        i8_ptr_ty,
        i8_ptr_ty,
        i8_ptr_ptr_ty,
        i32_ptr_ty,
        i32_ptr_ty,
    ];

    let fn_name = CString::new("omni_process")
        .map_err(|_| Diagnostic::internal("invalid process function name"))?;
    let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
    let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);
    add_enum_param_attribute(fn_ref, context, 1, b"noalias")?; // in_ptrs
    add_enum_param_attribute(fn_ref, context, 2, b"noalias")?; // out_ptrs
    add_enum_param_attribute(fn_ref, context, 5, b"noalias")?; // state_ptr

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());
    let cond_name =
        CString::new("loop_cond").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let body_name =
        CString::new("loop_body").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let exit_name =
        CString::new("loop_exit").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let loop_cond = LLVMAppendBasicBlockInContext(context, fn_ref, cond_name.as_ptr());
    let loop_body = LLVMAppendBasicBlockInContext(context, fn_ref, body_name.as_ptr());
    let loop_exit = LLVMAppendBasicBlockInContext(context, fn_ref, exit_name.as_ptr());

    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }
    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);

        let in_ptrs = LLVMGetParam(fn_ref, 0);
        let out_ptrs = LLVMGetParam(fn_ref, 1);
        let frames = LLVMGetParam(fn_ref, 2);
        let params_ptr = LLVMGetParam(fn_ref, 3);
        let state_ptr = LLVMGetParam(fn_ref, 4);
        let buffer_ptrs = LLVMGetParam(fn_ref, 5);
        let buffer_frames_ptr = LLVMGetParam(fn_ref, 6);
        let buffer_channels_ptr = LLVMGetParam(fn_ref, 7);

        let zero_i32 = LLVMConstInt(i32_ty, 0, 0);
        let one_i32 = LLVMConstInt(i32_ty, 1, 0);

        let frame_idx_name =
            CString::new("frame_idx").map_err(|_| Diagnostic::internal("invalid local name"))?;
        let frame_idx = LLVMBuildAlloca(builder, i32_ty, frame_idx_name.as_ptr());
        LLVMBuildStore(builder, zero_i32, frame_idx);

        let mut input_index = HashMap::new();
        for (idx, name) in typed.ins.iter().enumerate() {
            input_index.insert(name.clone(), idx as u32);
        }
        let mut input_types = HashMap::new();
        for name in &typed.ins {
            input_types.insert(
                name.clone(),
                *typed.in_types.get(name).unwrap_or(&PrimitiveType::F32),
            );
        }
        let mut buffer_index = HashMap::new();
        let mut buffer_elem_types = HashMap::new();
        let mut buffer_channels = HashMap::new();
        let mut buffer_mono = HashSet::new();
        for (idx, decl) in typed.buffers.iter().enumerate() {
            buffer_index.insert(decl.name.clone(), idx as u32);
            buffer_elem_types.insert(decl.name.clone(), decl.elem_ty);
            buffer_channels.insert(decl.name.clone(), decl.channels.clone());
            let mono = match decl.channels {
                TypedBufferChannels::Mono => true,
                TypedBufferChannels::Static(ch) => ch <= 1,
                TypedBufferChannels::Dynamic => false,
            };
            if mono {
                buffer_mono.insert(decl.name.clone());
            }
        }
        let arrays_by_offset = typed
            .param_arrays
            .iter()
            .map(|(name, info)| (info.offset, name.as_str()))
            .collect::<HashMap<_, _>>();
        let mut param_byte_offset = HashMap::new();
        let mut running_param_bytes = 0usize;
        for (slot_idx, param) in typed.params.iter().enumerate() {
            if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
                param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
            }
            param_byte_offset.insert(param.name.clone(), running_param_bytes);
            running_param_bytes =
                running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
        }
        let mut param_types = HashMap::new();
        for param in &typed.params {
            param_types.insert(param.name.clone(), param.ty);
        }
        let state_layout_entries = compute_state_layout(typed)?;
        let state_layout = state_layout_map(&state_layout_entries);
        let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
        let jit_layout = arrays_layout_map(&array_layout_entries);

        let mut array_base_ptrs = HashMap::new();
        let mut array_len = HashMap::new();
        let mut array_elem_ty = HashMap::new();
        for array_var in &typed.array_vars {
            let (_, offset) = *jit_layout.get(&array_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}'",
                    array_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                array_var.elem_ty,
                b"arr_state_ptr\0",
                b"arr_state_ptr_cast\0",
            );
            array_base_ptrs.insert(array_var.name.clone(), ptr);
            array_len.insert(array_var.name.clone(), array_var.len);
            array_elem_ty.insert(array_var.name.clone(), array_var.elem_ty);
        }
        let mut array_struct_roots = HashMap::new();
        let mut array_struct_len = HashMap::new();
        for root in &typed.array_struct_roots {
            array_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            array_struct_len.insert(root.name.clone(), root.len);
        }
        let struct_fields = typed
            .structs
            .iter()
            .map(|s| (s.name.clone(), s.fields.clone()))
            .collect::<HashMap<_, _>>();
        let mut state_slots = HashMap::new();
        for (idx, name) in typed.state_vars.iter().enumerate() {
            let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let slot_name = CString::new(format!("state_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
            let slot = LLVMBuildAlloca(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                slot_name.as_ptr(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                state_ty,
                b"state_ptr\0",
                b"state_ptr_cast\0",
            );
            let state_load = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                state_ptr_elt,
                b"state_load\0".as_ptr().cast(),
            );
            LLVMBuildStore(builder, state_load, slot);
            state_slots.insert(
                name.clone(),
                StateSlot {
                    ptr: slot,
                    ty: state_ty,
                },
            );
        }

        let mut out_slots = HashMap::new();
        let mut out_array_base_ptrs = HashMap::new();
        let mut out_array_names = typed.out_arrays.keys().cloned().collect::<Vec<_>>();
        out_array_names.sort();
        for array_name in out_array_names {
            let array_info = typed
                .out_arrays
                .get(&array_name)
                .ok_or_else(|| Diagnostic::internal("missing output array metadata"))?;
            let base_ptr = build_local_array_slot(
                builder,
                llvm_ty_for_primitive(context, array_info.elem_ty),
                array_info.len,
                &format!("out_arr_{array_name}"),
            )?;
            out_array_base_ptrs.insert(array_name.clone(), base_ptr);
            for idx in 0..array_info.len {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let elem_ptr = build_f32_ptr_offset(
                    builder,
                    llvm_ty_for_primitive(context, array_info.elem_ty),
                    base_ptr,
                    idx_v,
                    b"out_arr_elem_ptr\0",
                );
                out_slots.insert(
                    format!("{array_name}[{idx}]"),
                    OutSlot {
                        ptr: elem_ptr,
                        ty: array_info.elem_ty,
                    },
                );
            }
        }
        for (idx, name) in typed.outs.iter().enumerate() {
            if out_slots.contains_key(name) {
                continue;
            }
            let slot_name = CString::new(format!("out_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid output slot name"))?;
            let out_ty = *typed.out_types.get(name).unwrap_or(&PrimitiveType::F32);
            let out_llvm_ty = llvm_ty_for_primitive(context, out_ty);
            let slot = LLVMBuildAlloca(builder, out_llvm_ty, slot_name.as_ptr());
            LLVMBuildStore(builder, llvm_zero_for_primitive(context, out_ty), slot);
            out_slots.insert(
                name.clone(),
                OutSlot {
                    ptr: slot,
                    ty: out_ty,
                },
            );
        }
        let sample_oversample_factor = typed.sample_oversample_factor.max(1);
        let oversampled_sample_rate = sample_rate * sample_oversample_factor as f32;
        let oversample_iir = oversample_iir_coeff(sample_oversample_factor);
        let oversample_up_iir_coeff = if sample_oversample_factor > 1 {
            Some(LLVMConstReal(float_ty, oversample_iir))
        } else {
            None
        };
        let oversample_prev_inputs = if sample_oversample_factor > 1 && !typed.ins.is_empty() {
            let prev =
                build_local_array_slot(builder, float_ty, typed.ins.len(), "os_prev_inputs")?;
            for idx in 0..typed.ins.len() {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let prev_ptr =
                    build_f32_ptr_offset(builder, float_ty, prev, idx_v, b"os_prev_input_ptr\0");
                LLVMBuildStore(builder, LLVMConstReal(float_ty, 0.0), prev_ptr);
            }
            Some(prev)
        } else {
            None
        };
        let oversample_input_cache = if sample_oversample_factor > 1 && !typed.ins.is_empty() {
            let cache =
                build_local_array_slot(builder, float_ty, typed.ins.len(), "os_input_cache")?;
            for idx in 0..typed.ins.len() {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let ptr = build_f32_ptr_offset(builder, float_ty, cache, idx_v, b"os_cache_ptr\0");
                LLVMBuildStore(builder, LLVMConstReal(float_ty, 0.0), ptr);
            }
            Some(cache)
        } else {
            None
        };
        let oversample_up_iir_stage1 = if sample_oversample_factor > 1 && !typed.ins.is_empty() {
            let stage = build_local_array_slot(builder, float_ty, typed.ins.len(), "os_up_iir1")?;
            for idx in 0..typed.ins.len() {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let ptr =
                    build_f32_ptr_offset(builder, float_ty, stage, idx_v, b"os_up_iir1_ptr\0");
                LLVMBuildStore(builder, LLVMConstReal(float_ty, 0.0), ptr);
            }
            Some(stage)
        } else {
            None
        };
        let oversample_up_iir_stage2 = if sample_oversample_factor > 1 && !typed.ins.is_empty() {
            let stage = build_local_array_slot(builder, float_ty, typed.ins.len(), "os_up_iir2")?;
            for idx in 0..typed.ins.len() {
                let idx_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let ptr =
                    build_f32_ptr_offset(builder, float_ty, stage, idx_v, b"os_up_iir2_ptr\0");
                LLVMBuildStore(builder, LLVMConstReal(float_ty, 0.0), ptr);
            }
            Some(stage)
        } else {
            None
        };
        let mut oversample_accum_slots = HashMap::<String, LLVMValueRef>::new();
        let mut oversample_down_iir_stage1 = HashMap::<String, LLVMValueRef>::new();
        let mut oversample_down_iir_stage2 = HashMap::<String, LLVMValueRef>::new();
        if sample_oversample_factor > 1 {
            let mut out_names = out_slots.keys().cloned().collect::<Vec<_>>();
            out_names.sort();
            for name in out_names {
                let Some(slot) = out_slots.get(&name).copied() else {
                    continue;
                };
                let safe_name = name.replace(['[', ']', '.'], "_");
                let acc = build_local_slot(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    &format!("os_acc_{safe_name}"),
                )?;
                oversample_accum_slots.insert(name.clone(), acc);
                if matches!(slot.ty, PrimitiveType::F32 | PrimitiveType::F64) {
                    let stage1 = build_local_slot(
                        builder,
                        llvm_ty_for_primitive(context, slot.ty),
                        &format!("os_down_iir1_{safe_name}"),
                    )?;
                    let stage2 = build_local_slot(
                        builder,
                        llvm_ty_for_primitive(context, slot.ty),
                        &format!("os_down_iir2_{safe_name}"),
                    )?;
                    LLVMBuildStore(
                        builder,
                        LLVMConstReal(llvm_ty_for_primitive(context, slot.ty), 0.0),
                        stage1,
                    );
                    LLVMBuildStore(
                        builder,
                        LLVMConstReal(llvm_ty_for_primitive(context, slot.ty), 0.0),
                        stage2,
                    );
                    oversample_down_iir_stage1.insert(name.clone(), stage1);
                    oversample_down_iir_stage2.insert(name, stage2);
                }
            }
        }

        // Run optional block-level code once per process callback before per-sample loop.
        if !typed.block_pre.is_empty() {
            let mut block_ctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate: oversampled_sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                array_base_ptrs: &array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                array_len: &array_len,
                array_elem_ty: &array_elem_ty,
                array_struct_roots: &array_struct_roots,
                array_struct_len: &array_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_factor: 1,
                oversample_alpha: None,
                oversample_prev_inputs: None,
                oversample_input_cache: None,
                loop_stack: Vec::new(),
            };
            let mut block_locals = HashMap::new();
            let mut block_aliases = HashMap::new();
            let mut block_data_aliases = HashMap::new();
            for stmt in &typed.block_pre {
                lower_stmt(
                    stmt,
                    &mut block_ctx,
                    &mut block_locals,
                    &mut block_aliases,
                    &mut block_data_aliases,
                )?;
            }
        }

        LLVMBuildBr(builder, loop_cond);

        LLVMPositionBuilderAtEnd(builder, loop_cond);
        let frame_cur = LLVMBuildLoad2(builder, i32_ty, frame_idx, b"frame_cur\0".as_ptr().cast());
        let loop_cmp = LLVMBuildICmp(
            builder,
            LLVMIntPredicate::LLVMIntULT,
            frame_cur,
            frames,
            b"loop_cmp\0".as_ptr().cast(),
        );
        LLVMBuildCondBr(builder, loop_cmp, loop_body, loop_exit);

        LLVMPositionBuilderAtEnd(builder, loop_body);
        let frame_in_body =
            LLVMBuildLoad2(builder, i32_ty, frame_idx, b"frame_body\0".as_ptr().cast());
        if sample_oversample_factor <= 1 {
            for slot in out_slots.values() {
                LLVMBuildStore(builder, llvm_zero_for_primitive(context, slot.ty), slot.ptr);
            }
            let mut lctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: frame_in_body,
                state_slots: &state_slots,
                array_base_ptrs: &array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                array_len: &array_len,
                array_elem_ty: &array_elem_ty,
                array_struct_roots: &array_struct_roots,
                array_struct_len: &array_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_factor: 1,
                oversample_alpha: None,
                oversample_prev_inputs: None,
                oversample_input_cache: None,
                loop_stack: Vec::new(),
            };

            let mut locals = HashMap::new();
            let mut local_aliases = HashMap::new();
            let mut local_array_aliases = HashMap::new();
            for stmt in &typed.sample {
                lower_stmt(
                    stmt,
                    &mut lctx,
                    &mut locals,
                    &mut local_aliases,
                    &mut local_array_aliases,
                )?;
            }
        } else {
            for (name, slot) in &out_slots {
                let Some(acc_ptr) = oversample_accum_slots.get(name).copied() else {
                    continue;
                };
                LLVMBuildStore(builder, llvm_zero_for_primitive(context, slot.ty), acc_ptr);
            }

            let sub_preheader = LLVMGetInsertBlock(builder);
            if sub_preheader.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get current block for oversample lowering",
                ));
            }
            let sub_cond =
                LLVMAppendBasicBlockInContext(context, fn_ref, b"os_sub_cond\0".as_ptr().cast());
            let sub_body =
                LLVMAppendBasicBlockInContext(context, fn_ref, b"os_sub_body\0".as_ptr().cast());
            let sub_latch =
                LLVMAppendBasicBlockInContext(context, fn_ref, b"os_sub_latch\0".as_ptr().cast());
            let sub_end =
                LLVMAppendBasicBlockInContext(context, fn_ref, b"os_sub_end\0".as_ptr().cast());
            LLVMBuildBr(builder, sub_cond);

            LLVMPositionBuilderAtEnd(builder, sub_cond);
            let sub_i = LLVMBuildPhi(builder, i32_ty, b"os_sub_i\0".as_ptr().cast());
            let mut sub_incoming_vals = [zero_i32];
            let mut sub_incoming_blocks = [sub_preheader];
            LLVMAddIncoming(
                sub_i,
                sub_incoming_vals.as_mut_ptr(),
                sub_incoming_blocks.as_mut_ptr(),
                1,
            );
            let sub_factor_const = LLVMConstInt(i32_ty, sample_oversample_factor as u64, 0);
            let sub_cmp = LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntULT,
                sub_i,
                sub_factor_const,
                b"os_sub_cmp\0".as_ptr().cast(),
            );
            LLVMBuildCondBr(builder, sub_cmp, sub_body, sub_end);

            LLVMPositionBuilderAtEnd(builder, sub_body);
            for slot in out_slots.values() {
                LLVMBuildStore(builder, llvm_zero_for_primitive(context, slot.ty), slot.ptr);
            }
            let sub_i_plus_one =
                LLVMBuildAdd(builder, sub_i, one_i32, b"os_sub_i1\0".as_ptr().cast());
            let sub_i_plus_one_f = LLVMBuildSIToFP(
                builder,
                sub_i_plus_one,
                float_ty,
                b"os_sub_i1_f\0".as_ptr().cast(),
            );
            let sub_factor_f = LLVMConstReal(float_ty, sample_oversample_factor as f64);
            let sub_alpha = build_fdiv_fast(
                builder,
                sub_i_plus_one_f,
                sub_factor_f,
                b"os_sub_alpha\0",
                fast_math_flags,
            );
            if let (Some(prev_inputs_ptr), Some(input_cache_ptr)) =
                (oversample_prev_inputs, oversample_input_cache)
            {
                for (idx, name) in typed.ins.iter().enumerate() {
                    let in_ty = *input_types.get(name).unwrap_or(&PrimitiveType::F32);
                    let ch_v = LLVMConstInt(i32_ty, idx as u64, 0);
                    let in_ptr_ptr = build_ptr_offset(
                        builder,
                        float_ptr_ty,
                        in_ptrs,
                        ch_v,
                        b"os_sub_in_ch_ptr_ptr\0",
                    );
                    let in_ch_ptr = LLVMBuildLoad2(
                        builder,
                        float_ptr_ty,
                        in_ptr_ptr,
                        b"os_sub_in_ch_ptr\0".as_ptr().cast(),
                    );
                    let in_ch_ptr_typed = LLVMBuildBitCast(
                        builder,
                        in_ch_ptr,
                        LLVMPointerType(llvm_ty_for_primitive(context, in_ty), 0),
                        b"os_sub_in_ch_ptr_typed\0".as_ptr().cast(),
                    );
                    let in_sample_ptr = build_f32_ptr_offset(
                        builder,
                        llvm_ty_for_primitive(context, in_ty),
                        in_ch_ptr_typed,
                        frame_in_body,
                        b"os_sub_in_sample_ptr\0",
                    );
                    let in_raw = LLVMBuildLoad2(
                        builder,
                        llvm_ty_for_primitive(context, in_ty),
                        in_sample_ptr,
                        b"os_sub_in_raw\0".as_ptr().cast(),
                    );
                    let in_f32 = match in_ty {
                        PrimitiveType::F32 => in_raw,
                        PrimitiveType::F64 => LLVMBuildFPTrunc(
                            builder,
                            in_raw,
                            float_ty,
                            b"os_sub_in_f32\0".as_ptr().cast(),
                        ),
                        PrimitiveType::I32 | PrimitiveType::I64 => LLVMBuildSIToFP(
                            builder,
                            in_raw,
                            float_ty,
                            b"os_sub_in_f32\0".as_ptr().cast(),
                        ),
                        PrimitiveType::Bool => LLVMBuildUIToFP(
                            builder,
                            in_raw,
                            float_ty,
                            b"os_sub_in_f32\0".as_ptr().cast(),
                        ),
                    };
                    let prev_ptr = build_f32_ptr_offset(
                        builder,
                        float_ty,
                        prev_inputs_ptr,
                        ch_v,
                        b"os_sub_prev_ptr\0",
                    );
                    let prev = LLVMBuildLoad2(
                        builder,
                        float_ty,
                        prev_ptr,
                        b"os_sub_prev\0".as_ptr().cast(),
                    );
                    let diff = build_fsub_fast(
                        builder,
                        in_f32,
                        prev,
                        b"os_sub_in_diff\0",
                        fast_math_flags,
                    );
                    let scaled = build_fmul_fast(
                        builder,
                        diff,
                        sub_alpha,
                        b"os_sub_in_scaled\0",
                        fast_math_flags,
                    );
                    let mut filtered =
                        build_fadd_fast(builder, prev, scaled, b"os_sub_interp\0", fast_math_flags);
                    if let (Some(up_coeff), Some(up_iir1), Some(up_iir2)) = (
                        oversample_up_iir_coeff,
                        oversample_up_iir_stage1,
                        oversample_up_iir_stage2,
                    ) {
                        let up1_ptr = build_f32_ptr_offset(
                            builder,
                            float_ty,
                            up_iir1,
                            ch_v,
                            b"os_sub_up1_ptr\0",
                        );
                        let up1_old = LLVMBuildLoad2(
                            builder,
                            float_ty,
                            up1_ptr,
                            b"os_sub_up1_old\0".as_ptr().cast(),
                        );
                        let up1_delta = build_fsub_fast(
                            builder,
                            filtered,
                            up1_old,
                            b"os_sub_up1_delta\0",
                            fast_math_flags,
                        );
                        let up1_step = build_fmul_fast(
                            builder,
                            up1_delta,
                            up_coeff,
                            b"os_sub_up1_scaled\0",
                            fast_math_flags,
                        );
                        let up1_new = build_fadd_fast(
                            builder,
                            up1_old,
                            up1_step,
                            b"os_sub_up1_new\0",
                            fast_math_flags,
                        );
                        LLVMBuildStore(builder, up1_new, up1_ptr);

                        let up2_ptr = build_f32_ptr_offset(
                            builder,
                            float_ty,
                            up_iir2,
                            ch_v,
                            b"os_sub_up2_ptr\0",
                        );
                        let up2_old = LLVMBuildLoad2(
                            builder,
                            float_ty,
                            up2_ptr,
                            b"os_sub_up2_old\0".as_ptr().cast(),
                        );
                        let up2_delta = build_fsub_fast(
                            builder,
                            up1_new,
                            up2_old,
                            b"os_sub_up2_delta\0",
                            fast_math_flags,
                        );
                        let up2_step = build_fmul_fast(
                            builder,
                            up2_delta,
                            up_coeff,
                            b"os_sub_up2_scaled\0",
                            fast_math_flags,
                        );
                        let up2_new = build_fadd_fast(
                            builder,
                            up2_old,
                            up2_step,
                            b"os_sub_up2_new\0",
                            fast_math_flags,
                        );
                        LLVMBuildStore(builder, up2_new, up2_ptr);
                        filtered = up2_new;
                    }
                    let cache_ptr = build_f32_ptr_offset(
                        builder,
                        float_ty,
                        input_cache_ptr,
                        ch_v,
                        b"os_sub_cache_ptr\0",
                    );
                    LLVMBuildStore(builder, filtered, cache_ptr);
                }
            }
            let mut lctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate: oversampled_sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: frame_in_body,
                state_slots: &state_slots,
                array_base_ptrs: &array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                array_len: &array_len,
                array_elem_ty: &array_elem_ty,
                array_struct_roots: &array_struct_roots,
                array_struct_len: &array_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_factor: sample_oversample_factor,
                oversample_alpha: Some(sub_alpha),
                oversample_prev_inputs,
                oversample_input_cache,
                loop_stack: Vec::new(),
            };
            let mut locals = HashMap::new();
            let mut local_aliases = HashMap::new();
            let mut local_array_aliases = HashMap::new();
            for stmt in &typed.sample {
                lower_stmt(
                    stmt,
                    &mut lctx,
                    &mut locals,
                    &mut local_aliases,
                    &mut local_array_aliases,
                )?;
            }
            for (name, slot) in &out_slots {
                let Some(acc_ptr) = oversample_accum_slots.get(name).copied() else {
                    continue;
                };
                let cur_raw = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    slot.ptr,
                    b"os_cur_out\0".as_ptr().cast(),
                );
                let cur = match slot.ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        if let (Some(stage1_ptr), Some(stage2_ptr)) = (
                            oversample_down_iir_stage1.get(name).copied(),
                            oversample_down_iir_stage2.get(name).copied(),
                        ) {
                            let stage_ty = llvm_ty_for_primitive(context, slot.ty);
                            let coeff = LLVMConstReal(stage_ty, oversample_iir);
                            let stage1_old = LLVMBuildLoad2(
                                builder,
                                stage_ty,
                                stage1_ptr,
                                b"os_down_iir1_old\0".as_ptr().cast(),
                            );
                            let stage1_delta = build_fsub_fast(
                                builder,
                                cur_raw,
                                stage1_old,
                                b"os_down_iir1_delta\0",
                                fast_math_flags,
                            );
                            let stage1_step = build_fmul_fast(
                                builder,
                                stage1_delta,
                                coeff,
                                b"os_down_iir1_scaled\0",
                                fast_math_flags,
                            );
                            let stage1_new = build_fadd_fast(
                                builder,
                                stage1_old,
                                stage1_step,
                                b"os_down_iir1_new\0",
                                fast_math_flags,
                            );
                            LLVMBuildStore(builder, stage1_new, stage1_ptr);
                            let stage2_old = LLVMBuildLoad2(
                                builder,
                                stage_ty,
                                stage2_ptr,
                                b"os_down_iir2_old\0".as_ptr().cast(),
                            );
                            let stage2_delta = build_fsub_fast(
                                builder,
                                stage1_new,
                                stage2_old,
                                b"os_down_iir2_delta\0",
                                fast_math_flags,
                            );
                            let stage2_step = build_fmul_fast(
                                builder,
                                stage2_delta,
                                coeff,
                                b"os_down_iir2_scaled\0",
                                fast_math_flags,
                            );
                            let stage2_new = build_fadd_fast(
                                builder,
                                stage2_old,
                                stage2_step,
                                b"os_down_iir2_new\0",
                                fast_math_flags,
                            );
                            LLVMBuildStore(builder, stage2_new, stage2_ptr);
                            stage2_new
                        } else {
                            cur_raw
                        }
                    }
                    _ => cur_raw,
                };
                let acc_old = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    acc_ptr,
                    b"os_acc_old\0".as_ptr().cast(),
                );
                let acc_new = match slot.ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        build_fadd_fast(builder, acc_old, cur, b"os_acc_add\0", fast_math_flags)
                    }
                    PrimitiveType::I32 | PrimitiveType::I64 => {
                        LLVMBuildAdd(builder, acc_old, cur, b"os_acc_add_i\0".as_ptr().cast())
                    }
                    PrimitiveType::Bool => cur,
                };
                LLVMBuildStore(builder, acc_new, acc_ptr);
            }
            if !current_block_terminated(builder) {
                LLVMBuildBr(builder, sub_latch);
            }

            LLVMPositionBuilderAtEnd(builder, sub_latch);
            let sub_latch_block = LLVMGetInsertBlock(builder);
            if sub_latch_block.is_null() {
                return Err(Diagnostic::internal(
                    "failed to get oversample latch block in ORC lowering",
                ));
            }
            let sub_next = LLVMBuildAdd(builder, sub_i, one_i32, b"os_sub_next\0".as_ptr().cast());
            LLVMBuildBr(builder, sub_cond);
            let mut sub_back_vals = [sub_next];
            let mut sub_back_blocks = [sub_latch_block];
            LLVMAddIncoming(
                sub_i,
                sub_back_vals.as_mut_ptr(),
                sub_back_blocks.as_mut_ptr(),
                1,
            );

            LLVMPositionBuilderAtEnd(builder, sub_end);
            for (name, slot) in &out_slots {
                let Some(acc_ptr) = oversample_accum_slots.get(name).copied() else {
                    continue;
                };
                let acc = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    acc_ptr,
                    b"os_acc_load\0".as_ptr().cast(),
                );
                let decimated = match slot.ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        let denom = LLVMConstReal(
                            llvm_ty_for_primitive(context, slot.ty),
                            sample_oversample_factor as f64,
                        );
                        build_fdiv_fast(builder, acc, denom, b"os_decim\0", fast_math_flags)
                    }
                    PrimitiveType::I32 | PrimitiveType::I64 => {
                        let denom = LLVMConstInt(
                            llvm_ty_for_primitive(context, slot.ty),
                            sample_oversample_factor as u64,
                            0,
                        );
                        LLVMBuildSDiv(builder, acc, denom, b"os_decim_i\0".as_ptr().cast())
                    }
                    PrimitiveType::Bool => acc,
                };
                LLVMBuildStore(builder, decimated, slot.ptr);
            }
        }

        let io_cast_ctx = LoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            float_ptr_ty,
            i32_ty,
            fast_math_flags,
            sample_rate,
            block_size: block_size as f32,
            in_ptrs,
            params_ptr,
            buffer_ptrs,
            buffer_frames_ptr,
            buffer_channels_ptr,
            frame_idx: frame_in_body,
            state_slots: &state_slots,
            array_base_ptrs: &array_base_ptrs,
            out_slots: &out_slots,
            out_array_base_ptrs: &out_array_base_ptrs,
            input_index: &input_index,
            input_types: &input_types,
            input_arrays: &typed.in_arrays,
            buffer_index: &buffer_index,
            buffer_elem_types: &buffer_elem_types,
            buffer_channels: &buffer_channels,
            buffer_mono: &buffer_mono,
            param_byte_offset: &param_byte_offset,
            param_types: &param_types,
            param_arrays: &typed.param_arrays,
            output_arrays: &typed.out_arrays,
            array_len: &array_len,
            array_elem_ty: &array_elem_ty,
            array_struct_roots: &array_struct_roots,
            array_struct_len: &array_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: false,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
            oversample_factor: sample_oversample_factor,
            oversample_alpha: None,
            oversample_prev_inputs,
            oversample_input_cache: None,
            loop_stack: Vec::new(),
        };

        for (ch, name) in typed.outs.iter().enumerate() {
            let slot = out_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing output slot for '{name}' in ORC lowering"))
            })?;
            let out_ty = *typed.out_types.get(name).unwrap_or(&PrimitiveType::F32);
            let raw_out_value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"out_value_raw\0".as_ptr().cast(),
            );
            let out_value = if slot.ty == out_ty {
                raw_out_value
            } else {
                cast_orc_value_to(
                    &io_cast_ctx,
                    OrcValue {
                        value: raw_out_value,
                        ty: slot.ty,
                    },
                    out_ty,
                    b"out_value_cast\0",
                )
            };
            let ch_idx = LLVMConstInt(i32_ty, ch as u64, 0);
            let out_ptr_ptr =
                build_ptr_offset(builder, float_ptr_ty, out_ptrs, ch_idx, b"out_ch_ptr_ptr\0");
            let out_ch_ptr_raw = LLVMBuildLoad2(
                builder,
                float_ptr_ty,
                out_ptr_ptr,
                b"out_ch_ptr\0".as_ptr().cast(),
            );
            let out_ch_ptr = LLVMBuildBitCast(
                builder,
                out_ch_ptr_raw,
                LLVMPointerType(llvm_ty_for_primitive(context, out_ty), 0),
                b"out_ch_ptr_typed\0".as_ptr().cast(),
            );
            let out_ptr_elt = build_f32_ptr_offset(
                builder,
                llvm_ty_for_primitive(context, out_ty),
                out_ch_ptr,
                frame_in_body,
                b"out_ptr\0",
            );
            LLVMBuildStore(builder, out_value, out_ptr_elt);
        }
        if let Some(prev_inputs_ptr) = oversample_prev_inputs {
            for (idx, name) in typed.ins.iter().enumerate() {
                let in_ty = *input_types.get(name).unwrap_or(&PrimitiveType::F32);
                let ch_v = LLVMConstInt(i32_ty, idx as u64, 0);
                let in_ptr_ptr = build_ptr_offset(
                    builder,
                    float_ptr_ty,
                    in_ptrs,
                    ch_v,
                    b"os_prev_in_ch_ptr_ptr\0",
                );
                let in_ch_ptr = LLVMBuildLoad2(
                    builder,
                    float_ptr_ty,
                    in_ptr_ptr,
                    b"os_prev_in_ch_ptr\0".as_ptr().cast(),
                );
                let in_ch_ptr_typed = LLVMBuildBitCast(
                    builder,
                    in_ch_ptr,
                    LLVMPointerType(llvm_ty_for_primitive(context, in_ty), 0),
                    b"os_prev_in_ch_ptr_typed\0".as_ptr().cast(),
                );
                let in_sample_ptr = build_f32_ptr_offset(
                    builder,
                    llvm_ty_for_primitive(context, in_ty),
                    in_ch_ptr_typed,
                    frame_in_body,
                    b"os_prev_in_sample_ptr\0",
                );
                let in_raw = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, in_ty),
                    in_sample_ptr,
                    b"os_prev_in_raw\0".as_ptr().cast(),
                );
                let in_f32 = cast_orc_value_to(
                    &io_cast_ctx,
                    OrcValue {
                        value: in_raw,
                        ty: in_ty,
                    },
                    PrimitiveType::F32,
                    b"os_prev_in_cast\0",
                );
                let prev_ptr = build_f32_ptr_offset(
                    builder,
                    float_ty,
                    prev_inputs_ptr,
                    ch_v,
                    b"os_prev_in_ptr\0",
                );
                LLVMBuildStore(builder, in_f32, prev_ptr);
            }
        }

        let next_frame = LLVMBuildAdd(
            builder,
            frame_in_body,
            one_i32,
            b"next_frame\0".as_ptr().cast(),
        );
        LLVMBuildStore(builder, next_frame, frame_idx);
        LLVMBuildBr(builder, loop_cond);

        LLVMPositionBuilderAtEnd(builder, loop_exit);

        // Run optional post-sample block-level code once per process callback.
        if !typed.block_post.is_empty() {
            let mut block_ctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                array_base_ptrs: &array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &typed.in_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &typed.out_arrays,
                array_len: &array_len,
                array_elem_ty: &array_elem_ty,
                array_struct_roots: &array_struct_roots,
                array_struct_len: &array_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_factor: 1,
                oversample_alpha: None,
                oversample_prev_inputs: None,
                oversample_input_cache: None,
                loop_stack: Vec::new(),
            };
            let mut block_locals = HashMap::new();
            let mut block_aliases = HashMap::new();
            let mut block_data_aliases = HashMap::new();
            for stmt in &typed.block_post {
                lower_stmt(
                    stmt,
                    &mut block_ctx,
                    &mut block_locals,
                    &mut block_aliases,
                    &mut block_data_aliases,
                )?;
            }
        }
        for (_idx, name) in typed.state_vars.iter().enumerate() {
            let slot = state_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state slot for '{name}' in ORC lowering"))
            })?;
            let (_state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"state_out\0".as_ptr().cast(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                slot.ty,
                b"state_out_ptr\0",
                b"state_out_ptr_cast\0",
            );
            LLVMBuildStore(builder, value, state_ptr_elt);
        }
        LLVMBuildRetVoid(builder);

        Ok(())
    })();
    LLVMDisposeBuilder(builder);
    result
}

pub(super) unsafe fn build_init_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = LLVMPointerType(float_ptr_ty, 0);
    let fast_math_flags = fast_math_flags(fast_math);

    let mut arg_types = [i8_ptr_ty, i8_ptr_ty];

    let fn_name = CString::new("omni_init")
        .map_err(|_| Diagnostic::internal("invalid init function name"))?;
    let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
    let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());

    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }
    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);

        let params_ptr = LLVMGetParam(fn_ref, 0);
        let state_ptr = LLVMGetParam(fn_ref, 1);

        let arrays_by_offset = typed
            .param_arrays
            .iter()
            .map(|(name, info)| (info.offset, name.as_str()))
            .collect::<HashMap<_, _>>();
        let mut param_byte_offset = HashMap::new();
        let mut running_param_bytes = 0usize;
        for (slot_idx, param) in typed.params.iter().enumerate() {
            if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
                param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
            }
            param_byte_offset.insert(param.name.clone(), running_param_bytes);
            running_param_bytes =
                running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
        }
        let mut param_types = HashMap::new();
        for param in &typed.params {
            param_types.insert(param.name.clone(), param.ty);
        }
        let state_layout_entries = compute_state_layout(typed)?;
        let state_layout = state_layout_map(&state_layout_entries);
        let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
        let jit_layout = arrays_layout_map(&array_layout_entries);

        let mut array_base_ptrs = HashMap::new();
        let mut array_len = HashMap::new();
        let mut array_elem_ty = HashMap::new();
        for array_var in &typed.array_vars {
            let (_, offset) = *jit_layout.get(&array_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}' in ORC init lowering",
                    array_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                array_var.elem_ty,
                b"arr_init_state_ptr\0",
                b"arr_init_state_ptr_cast\0",
            );
            array_base_ptrs.insert(array_var.name.clone(), ptr);
            array_len.insert(array_var.name.clone(), array_var.len);
            array_elem_ty.insert(array_var.name.clone(), array_var.elem_ty);
        }
        let mut array_struct_roots = HashMap::new();
        let mut array_struct_len = HashMap::new();
        for root in &typed.array_struct_roots {
            array_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            array_struct_len.insert(root.name.clone(), root.len);
        }
        let struct_fields = typed
            .structs
            .iter()
            .map(|s| (s.name.clone(), s.fields.clone()))
            .collect::<HashMap<_, _>>();

        let mut state_slots = HashMap::new();
        for (idx, name) in typed.state_vars.iter().enumerate() {
            let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let slot_name = CString::new(format!("init_state_{}", idx))
                .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
            let slot = LLVMBuildAlloca(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                slot_name.as_ptr(),
            );
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                state_ty,
                b"init_state_ptr\0",
                b"init_state_ptr_cast\0",
            );
            let state_load = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, state_ty),
                state_ptr_elt,
                b"init_state_load\0".as_ptr().cast(),
            );
            LLVMBuildStore(builder, state_load, slot);
            state_slots.insert(
                name.clone(),
                StateSlot {
                    ptr: slot,
                    ty: state_ty,
                },
            );
        }

        let out_slots = HashMap::<String, OutSlot>::new();
        let out_array_base_ptrs = HashMap::<String, LLVMValueRef>::new();
        let input_index = HashMap::new();
        let input_types = HashMap::new();
        let input_arrays = HashMap::<String, TypedArrayInfo>::new();
        let buffer_index = HashMap::new();
        let buffer_elem_types = HashMap::new();
        let buffer_channels = HashMap::new();
        let buffer_mono = HashSet::<String>::new();
        let output_arrays = HashMap::<String, TypedArrayInfo>::new();

        let mut lctx = LoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            float_ptr_ty,
            i32_ty,
            fast_math_flags,
            sample_rate,
            block_size: block_size as f32,
            in_ptrs: LLVMConstPointerNull(float_ptr_ptr_ty),
            params_ptr,
            buffer_ptrs: LLVMConstPointerNull(LLVMPointerType(i8_ptr_ty, 0)),
            buffer_frames_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            buffer_channels_ptr: LLVMConstPointerNull(LLVMPointerType(i32_ty, 0)),
            frame_idx: LLVMConstInt(i32_ty, 0, 0),
            state_slots: &state_slots,
            array_base_ptrs: &array_base_ptrs,
            out_slots: &out_slots,
            out_array_base_ptrs: &out_array_base_ptrs,
            input_index: &input_index,
            input_types: &input_types,
            input_arrays: &input_arrays,
            buffer_index: &buffer_index,
            buffer_elem_types: &buffer_elem_types,
            buffer_channels: &buffer_channels,
            buffer_mono: &buffer_mono,
            param_byte_offset: &param_byte_offset,
            param_types: &param_types,
            param_arrays: &typed.param_arrays,
            output_arrays: &output_arrays,
            array_len: &array_len,
            array_elem_ty: &array_elem_ty,
            array_struct_roots: &array_struct_roots,
            array_struct_len: &array_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: true,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
            oversample_factor: 1,
            oversample_alpha: None,
            oversample_prev_inputs: None,
            oversample_input_cache: None,
            loop_stack: Vec::new(),
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_array_aliases = HashMap::new();
        for stmt in &typed.init {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_array_aliases,
            )?;
        }

        for (_idx, name) in typed.state_vars.iter().enumerate() {
            let slot = state_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing state slot for '{name}' in ORC init lowering"
                ))
            })?;
            let value = LLVMBuildLoad2(
                builder,
                llvm_ty_for_primitive(context, slot.ty),
                slot.ptr,
                b"init_state_out\0".as_ptr().cast(),
            );
            let (_state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
            })?;
            let state_ptr_elt = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                state_offset,
                slot.ty,
                b"init_state_out_ptr\0",
                b"init_state_out_ptr_cast\0",
            );
            LLVMBuildStore(builder, value, state_ptr_elt);
        }
        LLVMBuildRetVoid(builder);

        Ok(())
    })();
    LLVMDisposeBuilder(builder);
    result
}

pub(super) unsafe fn build_event_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    user_fns: &mut UserFnRegistry,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(LLVMInt8TypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i32_ptr_ty = LLVMPointerType(i32_ty, 0);
    let void_ty = LLVMVoidTypeInContext(context);
    let float_ptr_ty = i8_ptr_ty;
    let float_ptr_ptr_ty = LLVMPointerType(float_ptr_ty, 0);
    let i8_ptr_ptr_ty = LLVMPointerType(i8_ptr_ty, 0);
    let fast_math_flags = fast_math_flags(fast_math);

    if typed.events.is_empty() {
        return Ok(());
    }

    let arrays_by_offset = typed
        .param_arrays
        .iter()
        .map(|(name, info)| (info.offset, name.as_str()))
        .collect::<HashMap<_, _>>();
    let mut param_byte_offset = HashMap::new();
    let mut running_param_bytes = 0usize;
    for (slot_idx, param) in typed.params.iter().enumerate() {
        if let Some(base_name) = arrays_by_offset.get(&slot_idx) {
            param_byte_offset.insert((*base_name).to_owned(), running_param_bytes);
        }
        param_byte_offset.insert(param.name.clone(), running_param_bytes);
        running_param_bytes = running_param_bytes.saturating_add(primitive_type_bytes(param.ty));
    }
    let mut param_types = HashMap::new();
    for param in &typed.params {
        param_types.insert(param.name.clone(), param.ty);
    }
    let state_layout_entries = compute_state_layout(typed)?;
    let state_layout = state_layout_map(&state_layout_entries);
    let array_layout_entries = compute_arrays_layout(typed, &state_layout_entries)?;
    let jit_layout = arrays_layout_map(&array_layout_entries);
    let struct_fields = typed
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.fields.clone()))
        .collect::<HashMap<_, _>>();
    let mut buffer_index = HashMap::new();
    let mut buffer_elem_types = HashMap::new();
    let mut buffer_channels = HashMap::new();
    let mut buffer_mono = HashSet::new();
    for (idx, decl) in typed.buffers.iter().enumerate() {
        buffer_index.insert(decl.name.clone(), idx as u32);
        buffer_elem_types.insert(decl.name.clone(), decl.elem_ty);
        buffer_channels.insert(decl.name.clone(), decl.channels.clone());
        let mono = match decl.channels {
            TypedBufferChannels::Mono => true,
            TypedBufferChannels::Static(ch) => ch <= 1,
            TypedBufferChannels::Dynamic => false,
        };
        if mono {
            buffer_mono.insert(decl.name.clone());
        }
    }

    for (event_idx, event) in typed.events.iter().enumerate() {
        let mut arg_types = [
            i8_ptr_ty,
            i8_ptr_ty,
            i8_ptr_ty,
            i8_ptr_ptr_ty,
            i32_ptr_ty,
            i32_ptr_ty,
        ];
        let fn_name = CString::new(format!("omni_event_{event_idx}"))
            .map_err(|_| Diagnostic::internal("invalid event function name"))?;
        let fn_ty = LLVMFunctionType(void_ty, arg_types.as_mut_ptr(), arg_types.len() as u32, 0);
        let fn_ref = LLVMAddFunction(module, fn_name.as_ptr(), fn_ty);

        let entry_name =
            CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
        let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());

        let builder = LLVMCreateBuilderInContext(context);
        if builder.is_null() {
            return Err(Diagnostic::internal("failed to create LLVM builder"));
        }
        let result = (|| -> Result<(), Diagnostic> {
            LLVMPositionBuilderAtEnd(builder, entry);

            let payload_ptr = LLVMGetParam(fn_ref, 0);
            let params_ptr = LLVMGetParam(fn_ref, 1);
            let state_ptr = LLVMGetParam(fn_ref, 2);
            let buffer_ptrs = LLVMGetParam(fn_ref, 3);
            let buffer_frames_ptr = LLVMGetParam(fn_ref, 4);
            let buffer_channels_ptr = LLVMGetParam(fn_ref, 5);

            let mut array_base_ptrs = HashMap::new();
            let mut array_len = HashMap::new();
            let mut array_elem_ty = HashMap::new();
            for array_var in &typed.array_vars {
                let (_, offset) = *jit_layout.get(&array_var.name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing array layout metadata for '{}' in ORC event lowering",
                        array_var.name
                    ))
                })?;
                let ptr = build_typed_state_ptr(
                    builder,
                    context,
                    state_ptr,
                    offset,
                    array_var.elem_ty,
                    b"evt_arr_state_ptr\0",
                    b"evt_arr_state_ptr_cast\0",
                );
                array_base_ptrs.insert(array_var.name.clone(), ptr);
                array_len.insert(array_var.name.clone(), array_var.len);
                array_elem_ty.insert(array_var.name.clone(), array_var.elem_ty);
            }
            let mut array_struct_roots = HashMap::new();
            let mut array_struct_len = HashMap::new();
            for root in &typed.array_struct_roots {
                array_struct_roots.insert(root.name.clone(), root.struct_name.clone());
                array_struct_len.insert(root.name.clone(), root.len);
            }

            let mut state_slots = HashMap::new();
            for (idx, name) in typed.state_vars.iter().enumerate() {
                let (state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                    Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
                })?;
                let slot_name = CString::new(format!("evt_state_{idx}"))
                    .map_err(|_| Diagnostic::internal("invalid state slot name"))?;
                let slot = LLVMBuildAlloca(
                    builder,
                    llvm_ty_for_primitive(context, state_ty),
                    slot_name.as_ptr(),
                );
                let state_ptr_elt = build_typed_state_ptr(
                    builder,
                    context,
                    state_ptr,
                    state_offset,
                    state_ty,
                    b"evt_state_ptr\0",
                    b"evt_state_ptr_cast\0",
                );
                let state_load = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, state_ty),
                    state_ptr_elt,
                    b"evt_state_load\0".as_ptr().cast(),
                );
                LLVMBuildStore(builder, state_load, slot);
                state_slots.insert(
                    name.clone(),
                    StateSlot {
                        ptr: slot,
                        ty: state_ty,
                    },
                );
            }

            let out_slots = HashMap::<String, OutSlot>::new();
            let out_array_base_ptrs = HashMap::<String, LLVMValueRef>::new();
            let input_index = HashMap::new();
            let input_types = HashMap::new();
            let input_arrays = HashMap::<String, TypedArrayInfo>::new();
            let output_arrays = HashMap::<String, TypedArrayInfo>::new();

            let mut lctx = LoweringCtx {
                builder,
                context,
                module,
                fn_ref,
                float_ty,
                float_ptr_ty,
                i32_ty,
                fast_math_flags,
                sample_rate,
                block_size: block_size as f32,
                in_ptrs: LLVMConstPointerNull(float_ptr_ptr_ty),
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                array_base_ptrs: &array_base_ptrs,
                out_slots: &out_slots,
                out_array_base_ptrs: &out_array_base_ptrs,
                input_index: &input_index,
                input_types: &input_types,
                input_arrays: &input_arrays,
                buffer_index: &buffer_index,
                buffer_elem_types: &buffer_elem_types,
                buffer_channels: &buffer_channels,
                buffer_mono: &buffer_mono,
                param_byte_offset: &param_byte_offset,
                param_types: &param_types,
                param_arrays: &typed.param_arrays,
                output_arrays: &output_arrays,
                array_len: &array_len,
                array_elem_ty: &array_elem_ty,
                array_struct_roots: &array_struct_roots,
                array_struct_len: &array_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
                oversample_factor: 1,
                oversample_alpha: None,
                oversample_prev_inputs: None,
                oversample_input_cache: None,
                loop_stack: Vec::new(),
            };

            let mut locals = HashMap::<String, OrcValue>::new();
            let mut local_aliases = HashMap::new();
            let mut local_array_aliases = HashMap::new();
            let mut payload_offset = 0usize;
            for param in &event.params {
                match param.ty {
                    TypedEventParamType::Scalar(ty) => {
                        let param_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            const_i32(i32_ty, payload_offset as i32),
                            ty,
                            b"evt_param_ptr_i8\0",
                            b"evt_param_ptr_typed\0",
                        );
                        let loaded = LLVMBuildLoad2(
                            builder,
                            llvm_ty_for_primitive(context, ty),
                            param_ptr,
                            b"evt_param_load\0".as_ptr().cast(),
                        );
                        locals.insert(param.name.clone(), OrcValue { value: loaded, ty });
                        payload_offset = payload_offset.saturating_add(primitive_type_bytes(ty));
                    }
                    TypedEventParamType::Array { elem, len } => {
                        let base_ptr = build_typed_ptr_from_byte_offset(
                            builder,
                            context,
                            payload_ptr,
                            const_i32(i32_ty, payload_offset as i32),
                            elem,
                            b"evt_arr_ptr_i8\0",
                            b"evt_arr_ptr_typed\0",
                        );
                        local_array_aliases.insert(
                            param.name.clone(),
                            LocalArrayAlias::Primitive {
                                base_ptr,
                                len,
                                elem_ty: elem,
                            },
                        );
                        payload_offset = payload_offset
                            .saturating_add(primitive_type_bytes(elem).saturating_mul(len));
                    }
                }
            }

            for stmt in &event.body {
                lower_stmt(
                    stmt,
                    &mut lctx,
                    &mut locals,
                    &mut local_aliases,
                    &mut local_array_aliases,
                )?;
            }

            for name in &typed.state_vars {
                let slot = state_slots.get(name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing state slot for '{name}' in ORC event lowering"
                    ))
                })?;
                let value = LLVMBuildLoad2(
                    builder,
                    llvm_ty_for_primitive(context, slot.ty),
                    slot.ptr,
                    b"evt_state_out\0".as_ptr().cast(),
                );
                let (_state_ty, state_offset) = *state_layout.get(name).ok_or_else(|| {
                    Diagnostic::internal(format!("missing state layout metadata for '{name}'"))
                })?;
                let state_ptr_elt = build_typed_state_ptr(
                    builder,
                    context,
                    state_ptr,
                    state_offset,
                    slot.ty,
                    b"evt_state_out_ptr\0",
                    b"evt_state_out_ptr_cast\0",
                );
                LLVMBuildStore(builder, value, state_ptr_elt);
            }

            LLVMBuildRetVoid(builder);
            Ok(())
        })();
        LLVMDisposeBuilder(builder);
        result?;
    }

    Ok(())
}
