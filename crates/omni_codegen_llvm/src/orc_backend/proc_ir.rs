use super::*;

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
        let data_layout = arrays_layout_map(&array_layout_entries);

        let mut data_base_ptrs = HashMap::new();
        let mut data_len = HashMap::new();
        let mut data_elem_ty = HashMap::new();
        for data_var in &typed.data_vars {
            let (_, offset) = *data_layout.get(&data_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}'",
                    data_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                data_var.elem_ty,
                b"arr_state_ptr\0",
                b"arr_state_ptr_cast\0",
            );
            data_base_ptrs.insert(data_var.name.clone(), ptr);
            data_len.insert(data_var.name.clone(), data_var.len);
            data_elem_ty.insert(data_var.name.clone(), data_var.elem_ty);
        }
        let mut data_struct_roots = HashMap::new();
        let mut data_struct_len = HashMap::new();
        for root in &typed.data_struct_roots {
            data_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            data_struct_len.insert(root.name.clone(), root.len);
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
                sample_rate,
                block_size: block_size as f32,
                in_ptrs,
                params_ptr,
                buffer_ptrs,
                buffer_frames_ptr,
                buffer_channels_ptr,
                frame_idx: LLVMConstInt(i32_ty, 0, 0),
                state_slots: &state_slots,
                data_base_ptrs: &data_base_ptrs,
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
                data_len: &data_len,
                data_elem_ty: &data_elem_ty,
                data_struct_roots: &data_struct_roots,
                data_struct_len: &data_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
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
        for slot in out_slots.values() {
            LLVMBuildStore(builder, llvm_zero_for_primitive(context, slot.ty), slot.ptr);
        }

        let frame_in_body =
            LLVMBuildLoad2(builder, i32_ty, frame_idx, b"frame_body\0".as_ptr().cast());
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
            data_base_ptrs: &data_base_ptrs,
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
            data_len: &data_len,
            data_elem_ty: &data_elem_ty,
            data_struct_roots: &data_struct_roots,
            data_struct_len: &data_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: false,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
            loop_stack: Vec::new(),
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_data_aliases = HashMap::new();
        for stmt in &typed.sample {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_data_aliases,
            )?;
        }

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
                    &lctx,
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
                data_base_ptrs: &data_base_ptrs,
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
                data_len: &data_len,
                data_elem_ty: &data_elem_ty,
                data_struct_roots: &data_struct_roots,
                data_struct_len: &data_struct_len,
                struct_fields: &struct_fields,
                allow_struct_ctor: false,
                user_fn_param_names: &user_fns.param_names,
                user_fn_param_defaults: &user_fns.param_defaults,
                user_fn_param_kinds: &user_fns.param_kinds,
                user_fn_param_by_ref: &user_fns.param_by_ref,
                user_registry: user_fns as *const UserFnRegistry,
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
        let data_layout = arrays_layout_map(&array_layout_entries);

        let mut data_base_ptrs = HashMap::new();
        let mut data_len = HashMap::new();
        let mut data_elem_ty = HashMap::new();
        for data_var in &typed.data_vars {
            let (_, offset) = *data_layout.get(&data_var.name).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing array layout metadata for '{}' in ORC init lowering",
                    data_var.name
                ))
            })?;
            let ptr = build_typed_state_ptr(
                builder,
                context,
                state_ptr,
                offset,
                data_var.elem_ty,
                b"arr_init_state_ptr\0",
                b"arr_init_state_ptr_cast\0",
            );
            data_base_ptrs.insert(data_var.name.clone(), ptr);
            data_len.insert(data_var.name.clone(), data_var.len);
            data_elem_ty.insert(data_var.name.clone(), data_var.elem_ty);
        }
        let mut data_struct_roots = HashMap::new();
        let mut data_struct_len = HashMap::new();
        for root in &typed.data_struct_roots {
            data_struct_roots.insert(root.name.clone(), root.struct_name.clone());
            data_struct_len.insert(root.name.clone(), root.len);
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
            data_base_ptrs: &data_base_ptrs,
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
            data_len: &data_len,
            data_elem_ty: &data_elem_ty,
            data_struct_roots: &data_struct_roots,
            data_struct_len: &data_struct_len,
            struct_fields: &struct_fields,
            allow_struct_ctor: true,
            user_fn_param_names: &user_fns.param_names,
            user_fn_param_defaults: &user_fns.param_defaults,
            user_fn_param_kinds: &user_fns.param_kinds,
            user_fn_param_by_ref: &user_fns.param_by_ref,
            user_registry: user_fns as *const UserFnRegistry,
            loop_stack: Vec::new(),
        };

        let mut locals = HashMap::new();
        let mut local_aliases = HashMap::new();
        let mut local_data_aliases = HashMap::new();
        for stmt in &typed.init {
            lower_stmt(
                stmt,
                &mut lctx,
                &mut locals,
                &mut local_aliases,
                &mut local_data_aliases,
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
