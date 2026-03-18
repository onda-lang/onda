use super::super::*;

pub(in crate::orc_backend) unsafe fn lower_user_function_body(
    def: &TypedFunction,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    registry: &mut UserFnRegistry,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    fn_ref: LLVMValueRef,
    return_ty: ReturnType,
    scalar_param_types: &[PrimitiveType],
    array_param_types: &[(PrimitiveType, usize)],
    buffer_param_types: &[(PrimitiveType, TypedBufferChannels)],
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i32_ty = LLVMInt32TypeInContext(context);
    let return_llvm_ty = llvm_ty_for_return_type(context, &return_ty);

    let entry_name =
        CString::new("entry").map_err(|_| Diagnostic::internal("invalid block name"))?;
    let entry = LLVMAppendBasicBlockInContext(context, fn_ref, entry_name.as_ptr());
    let ret_block = LLVMAppendBasicBlockInContext(context, fn_ref, b"def_ret\0".as_ptr().cast());
    let builder = LLVMCreateBuilderInContext(context);
    if builder.is_null() {
        return Err(Diagnostic::internal("failed to create LLVM builder"));
    }

    let result = (|| -> Result<(), Diagnostic> {
        LLVMPositionBuilderAtEnd(builder, entry);
        let zero_ret = llvm_zero_for_return_type(context, &return_ty);

        let ret_name =
            CString::new("ret").map_err(|_| Diagnostic::internal("invalid local variable name"))?;
        let return_slot = LLVMBuildAlloca(builder, return_llvm_ty, ret_name.as_ptr());
        LLVMBuildStore(builder, zero_ret, return_slot);

        let mut ctx = DefLoweringCtx {
            builder,
            context,
            module,
            fn_ref,
            float_ty,
            i32_ty,
            fast_math_flags: fast_math_flags(fast_math),
            sample_rate,
            block_size: block_size as f32,
            return_ty,
            return_slot,
            return_block: ret_block,
            local_slots: HashMap::new(),
            tuple_slots: HashMap::new(),
            local_array_aliases: HashMap::new(),
            buffer_params: HashMap::new(),
            array_ptrs: HashMap::new(),
            array_len: HashMap::new(),
            array_len_values: HashMap::new(),
            array_elem_ty: HashMap::new(),
            array_struct_roots: HashMap::new(),
            struct_fields,
            user_fn_param_names: &registry.param_names,
            user_fn_param_defaults: &registry.param_defaults,
            user_fn_param_kinds: &registry.param_kinds,
            user_fn_param_by_ref: &registry.param_by_ref,
            user_registry: registry as *const UserFnRegistry,
            loop_stack: Vec::new(),
        };

        let by_ref_flags = registry.param_by_ref.get(&def.name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing by-ref metadata for user function '{}'",
                def.name
            ))
        })?;
        validate_param_signatures(
            &def.name,
            &def.param_kinds,
            scalar_param_types,
            array_param_types,
            buffer_param_types,
            "function body lowering",
        )?;
        let oversample_factor = registry
            .sample_oversample_factors
            .get(&def.name)
            .copied()
            .unwrap_or(1)
            .max(1);
        let proc_step_os_meta = registry.proc_step_oversample_meta.get(&def.name).cloned();

        let mut llvm_param_idx: u32 = 0;
        let mut scalar_param_idx: usize = 0;
        let mut array_param_idx: usize = 0;
        let mut buffer_param_idx: usize = 0;
        for (param_idx, (param_name, kind)) in
            def.params.iter().zip(def.param_kinds.iter()).enumerate()
        {
            match kind {
                TypedFnParam::Scalar { ty } => {
                    let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    if param_val.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM param {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    let param_ty =
                        resolve_scalar_param_type(*ty, scalar_param_types[scalar_param_idx]);
                    scalar_param_idx += 1;
                    let slot = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, param_ty),
                        &format!("p_{param_name}"),
                    )?;
                    LLVMBuildStore(ctx.builder, param_val, slot);
                    ctx.local_slots.insert(
                        param_name.clone(),
                        DefLocalSlot {
                            ptr: slot,
                            ty: param_ty,
                        },
                    );
                    llvm_param_idx += 1;
                }
                TypedFnParam::Struct { struct_name } => {
                    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "unknown struct '{}' used by function '{}'",
                            struct_name, def.name
                        ))
                    })?;
                    for field in fields {
                        let flat = format!("{param_name}.{}", field.name);
                        match field.ty {
                            TypedFieldType::Scalar(prim) => {
                                let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                if param_val.is_null() {
                                    return Err(Diagnostic::internal(format!(
                                        "missing LLVM param {} for function '{}'",
                                        llvm_param_idx, def.name
                                    )));
                                }
                                if by_ref_flags[param_idx] {
                                    ctx.local_slots.insert(
                                        flat,
                                        DefLocalSlot {
                                            ptr: param_val,
                                            ty: prim,
                                        },
                                    );
                                } else {
                                    let slot = build_local_slot(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, prim),
                                        &format!("p_{flat}"),
                                    )?;
                                    LLVMBuildStore(ctx.builder, param_val, slot);
                                    ctx.local_slots.insert(
                                        flat,
                                        DefLocalSlot {
                                            ptr: slot,
                                            ty: prim,
                                        },
                                    );
                                }
                                llvm_param_idx += 1;
                            }
                            TypedFieldType::Struct => {}
                            TypedFieldType::Tuple(ref elem_tys) => {
                                for (idx, prim) in elem_tys.iter().enumerate() {
                                    let elem_flat = format!("{flat}.__{idx}");
                                    let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                    if param_val.is_null() {
                                        return Err(Diagnostic::internal(format!(
                                            "missing LLVM param {} for function '{}'",
                                            llvm_param_idx, def.name
                                        )));
                                    }
                                    if by_ref_flags[param_idx] {
                                        ctx.local_slots.insert(
                                            elem_flat,
                                            DefLocalSlot {
                                                ptr: param_val,
                                                ty: *prim,
                                            },
                                        );
                                    } else {
                                        let slot = build_local_slot(
                                            ctx.builder,
                                            llvm_ty_for_primitive(ctx.context, *prim),
                                            &format!("p_{elem_flat}"),
                                        )?;
                                        LLVMBuildStore(ctx.builder, param_val, slot);
                                        ctx.local_slots.insert(
                                            elem_flat,
                                            DefLocalSlot {
                                                ptr: slot,
                                                ty: *prim,
                                            },
                                        );
                                    }
                                    llvm_param_idx += 1;
                                }
                            }
                            TypedFieldType::Array(len) => {
                                let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                if param_val.is_null() {
                                    return Err(Diagnostic::internal(format!(
                                        "missing LLVM param {} for function '{}'",
                                        llvm_param_idx, def.name
                                    )));
                                }
                                if let Some(elem_struct) = &field.array_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_array_struct_bindings(
                                        ctx.struct_fields,
                                        elem_struct,
                                        &flat,
                                        len,
                                        &mut roots,
                                        &mut leaves,
                                        &mut Vec::new(),
                                    )?;
                                    for (root_name, root_struct, root_len) in roots {
                                        ctx.array_struct_roots
                                            .insert(root_name.clone(), root_struct);
                                        ctx.array_len.entry(root_name).or_insert(root_len);
                                    }
                                    let mut leaf_iter = leaves.into_iter();
                                    let first = leaf_iter.next().ok_or_else(|| {
                                        Diagnostic::internal(format!(
                                            "array[Struct] field '{flat}' produced no leaf bindings in def lowering"
                                        ))
                                    })?;
                                    ctx.array_ptrs.insert(first.0.clone(), param_val);
                                    ctx.array_len.insert(first.0.clone(), first.1);
                                    ctx.array_elem_ty.insert(first.0, first.2);
                                    for (leaf_name, leaf_len, leaf_ty) in leaf_iter {
                                        llvm_param_idx += 1;
                                        let leaf_param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                        if leaf_param_val.is_null() {
                                            return Err(Diagnostic::internal(format!(
                                                "missing LLVM param {} for function '{}'",
                                                llvm_param_idx, def.name
                                            )));
                                        }
                                        ctx.array_ptrs.insert(leaf_name.clone(), leaf_param_val);
                                        ctx.array_len.insert(leaf_name.clone(), leaf_len);
                                        ctx.array_elem_ty.insert(leaf_name, leaf_ty);
                                    }
                                } else {
                                    ctx.array_ptrs.insert(flat.clone(), param_val);
                                    ctx.array_len.insert(flat.clone(), len);
                                    ctx.array_elem_ty.insert(
                                        flat,
                                        field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                                    );
                                }
                                llvm_param_idx += 1;
                            }
                        }
                    }
                }
                TypedFnParam::Array { .. } => {
                    let (elem_ty, len) = array_param_types
                        .get(array_param_idx)
                        .copied()
                        .ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "missing array signature for '{}' parameter '{}' at index {}",
                                def.name, param_name, array_param_idx
                            ))
                        })?;
                    array_param_idx += 1;
                    let param_ptr = LLVMGetParam(fn_ref, llvm_param_idx);
                    let param_len = LLVMGetParam(fn_ref, llvm_param_idx + 1);
                    if param_ptr.is_null() || param_len.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM array param {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    ctx.array_ptrs.insert(param_name.clone(), param_ptr);
                    ctx.array_len.insert(param_name.clone(), len);
                    ctx.array_len_values.insert(param_name.clone(), param_len);
                    ctx.array_elem_ty.insert(param_name.clone(), elem_ty);
                    llvm_param_idx += 2;
                }
                TypedFnParam::Buffer { .. } => {
                    let (elem_ty, channels) = buffer_param_types
                        .get(buffer_param_idx)
                        .cloned()
                        .ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "missing buffer signature for '{}' parameter '{}' at index {}",
                                def.name, param_name, buffer_param_idx
                            ))
                        })?;
                    buffer_param_idx += 1;
                    let ptr_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    let frames_val = LLVMGetParam(fn_ref, llvm_param_idx + 1);
                    let channels_val = LLVMGetParam(fn_ref, llvm_param_idx + 2);
                    let sample_rate_val = LLVMGetParam(fn_ref, llvm_param_idx + 3);
                    if ptr_val.is_null()
                        || frames_val.is_null()
                        || channels_val.is_null()
                        || sample_rate_val.is_null()
                    {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM buffer param tuple at {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    ctx.buffer_params.insert(
                        param_name.clone(),
                        DefBufferParamInfo {
                            ptr: ptr_val,
                            frames: frames_val,
                            channels: channels_val,
                            sample_rate: sample_rate_val,
                            elem_ty,
                            declared_channels: channels,
                        },
                    );
                    llvm_param_idx += 4;
                }
                TypedFnParam::Tuple { elem_tys } => {
                    // Tuple params are expanded to N individual LLVM params.
                    // Allocate a tuple slot (LLVM struct alloca) and store each element.
                    let mut llvm_elem_tys: Vec<LLVMTypeRef> = elem_tys
                        .iter()
                        .map(|t| llvm_ty_for_primitive(ctx.context, *t))
                        .collect();
                    let struct_ty = LLVMStructTypeInContext(
                        ctx.context,
                        llvm_elem_tys.as_mut_ptr(),
                        llvm_elem_tys.len() as u32,
                        0,
                    );
                    let slot =
                        build_local_slot(ctx.builder, struct_ty, &format!("p_{param_name}"))?;
                    // Store each expanded param into the struct slot
                    for (i, _ty) in elem_tys.iter().enumerate() {
                        let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                        if param_val.is_null() {
                            return Err(Diagnostic::internal(format!(
                                "missing LLVM tuple param {} for function '{}'",
                                llvm_param_idx, def.name
                            )));
                        }
                        let gep = LLVMBuildStructGEP2(
                            ctx.builder,
                            struct_ty,
                            slot,
                            i as u32,
                            b"tup_p_gep\0".as_ptr().cast(),
                        );
                        LLVMBuildStore(ctx.builder, param_val, gep);
                        llvm_param_idx += 1;
                    }
                    ctx.tuple_slots.insert(
                        param_name.clone(),
                        DefTupleSlot {
                            ptr: slot,
                            elem_tys: elem_tys.clone(),
                        },
                    );
                }
            }
        }

        let mut terminated = false;
        if oversample_factor > 1 {
            if let Some(meta) = proc_step_os_meta {
                let zero_i32 = LLVMConstInt(ctx.i32_ty, 0, 0);
                let one_i32 = LLVMConstInt(ctx.i32_ty, 1, 0);
                let sub_factor_const = LLVMConstInt(ctx.i32_ty, oversample_factor as u64, 0);
                let mut input_runtime =
                    Vec::<(DefLocalSlot, LLVMValueRef, Option<LLVMValueRef>)>::new();
                for (param_name, state_fields) in &meta.input_state_fields {
                    let param_slot = ctx.local_slots.get(param_name).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample input param slot '{}' for '{}'",
                            param_name, def.name
                        ))
                    })?;
                    let raw = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, param_slot.ty),
                        param_slot.ptr,
                        b"os_param_raw\0".as_ptr().cast(),
                    );
                    let value_array =
                        if matches!(param_slot.ty, PrimitiveType::F32 | PrimitiveType::F64)
                            && !state_fields.up_stages.is_empty()
                        {
                            let value_ptr = build_local_array_slot(
                                ctx.builder,
                                llvm_ty_for_primitive(ctx.context, param_slot.ty),
                                oversample_factor,
                                &oversample_local_name("os_in_values", param_name),
                            )?;
                            let stage_slots = state_fields
                                .up_stages
                                .iter()
                                .map(|stage| proc_sinc_stage_slots(&ctx, &def.name, stage))
                                .collect::<Result<Vec<_>, _>>()?;
                            let mut step = oversample_factor / 2;
                            for (stage_index, stage_slot_set) in stage_slots.iter().enumerate() {
                                let tap_ptrs = proc_sinc_stage_slot_ptrs(stage_slot_set);
                                let mut frame = 0usize;
                                while frame < oversample_factor {
                                    let input_value = if stage_index == 0 {
                                        raw
                                    } else {
                                        let source_ptr = build_ptr_offset(
                                            ctx.builder,
                                            llvm_ty_for_primitive(ctx.context, param_slot.ty),
                                            value_ptr,
                                            LLVMConstInt(ctx.i32_ty, frame as u64, 0),
                                            b"os_in_stage_src_ptr\0",
                                        );
                                        LLVMBuildLoad2(
                                            ctx.builder,
                                            llvm_ty_for_primitive(ctx.context, param_slot.ty),
                                            source_ptr,
                                            b"os_in_stage_src\0".as_ptr().cast(),
                                        )
                                    };
                                    let (out1, out2) = build_sinc_interpolate(
                                        ctx.builder,
                                        ctx.context,
                                        param_slot.ty,
                                        &tap_ptrs,
                                        input_value,
                                        ctx.fast_math_flags,
                                    );
                                    let out1_ptr = build_ptr_offset(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, param_slot.ty),
                                        value_ptr,
                                        LLVMConstInt(ctx.i32_ty, frame as u64, 0),
                                        b"os_in_stage_out1_ptr\0",
                                    );
                                    let out2_ptr = build_ptr_offset(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, param_slot.ty),
                                        value_ptr,
                                        LLVMConstInt(ctx.i32_ty, (frame + step) as u64, 0),
                                        b"os_in_stage_out2_ptr\0",
                                    );
                                    LLVMBuildStore(ctx.builder, out1, out1_ptr);
                                    LLVMBuildStore(ctx.builder, out2, out2_ptr);
                                    frame += step * 2;
                                }
                                step /= 2;
                            }
                            Some(value_ptr)
                        } else {
                            None
                        };
                    input_runtime.push((param_slot, raw, value_array));
                }

                let mut output_runtime =
                    Vec::<(DefLocalSlot, LLVMValueRef, Vec<[DefLocalSlot; 8]>)>::new();
                for (output_field, state_fields) in &meta.output_state_fields {
                    let output_key = format!("self.{output_field}");
                    let out_slot = ctx.local_slots.get(&output_key).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample output slot '{}' for '{}'",
                            output_key, def.name
                        ))
                    })?;
                    let value_ptr = build_local_array_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                        oversample_factor,
                        &oversample_local_name("os_out_values", output_field),
                    )?;
                    let down_stage_slots =
                        if matches!(out_slot.ty, PrimitiveType::F32 | PrimitiveType::F64)
                            && !state_fields.down_stages.is_empty()
                        {
                            state_fields
                                .down_stages
                                .iter()
                                .map(|stage| proc_sinc_stage_slots(&ctx, &def.name, stage))
                                .collect::<Result<Vec<_>, _>>()?
                        } else {
                            Vec::new()
                        };
                    output_runtime.push((out_slot, value_ptr, down_stage_slots));
                }

                let sub_preheader = LLVMGetInsertBlock(ctx.builder);
                if sub_preheader.is_null() {
                    return Err(Diagnostic::internal(
                        "failed to get proc oversample preheader block in def lowering",
                    ));
                }
                let sub_cond = LLVMAppendBasicBlockInContext(
                    ctx.context,
                    ctx.fn_ref,
                    b"def_os_sub_cond\0".as_ptr().cast(),
                );
                let sub_body = LLVMAppendBasicBlockInContext(
                    ctx.context,
                    ctx.fn_ref,
                    b"def_os_sub_body\0".as_ptr().cast(),
                );
                let sub_latch = LLVMAppendBasicBlockInContext(
                    ctx.context,
                    ctx.fn_ref,
                    b"def_os_sub_latch\0".as_ptr().cast(),
                );
                let sub_end = LLVMAppendBasicBlockInContext(
                    ctx.context,
                    ctx.fn_ref,
                    b"def_os_sub_end\0".as_ptr().cast(),
                );
                LLVMBuildBr(ctx.builder, sub_cond);

                LLVMPositionBuilderAtEnd(ctx.builder, sub_cond);
                let sub_i =
                    LLVMBuildPhi(ctx.builder, ctx.i32_ty, b"def_os_sub_i\0".as_ptr().cast());
                let mut sub_incoming_vals = [zero_i32];
                let mut sub_incoming_blocks = [sub_preheader];
                LLVMAddIncoming(
                    sub_i,
                    sub_incoming_vals.as_mut_ptr(),
                    sub_incoming_blocks.as_mut_ptr(),
                    1,
                );
                let sub_cmp = LLVMBuildICmp(
                    ctx.builder,
                    LLVMIntPredicate::LLVMIntULT,
                    sub_i,
                    sub_factor_const,
                    b"def_os_sub_cmp\0".as_ptr().cast(),
                );
                LLVMBuildCondBr(ctx.builder, sub_cmp, sub_body, sub_end);

                LLVMPositionBuilderAtEnd(ctx.builder, sub_body);
                for (param_slot, raw, value_array) in &input_runtime {
                    let sub_value = if let Some(value_ptr) = value_array {
                        let value_elem_ptr = build_ptr_offset(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, param_slot.ty),
                            *value_ptr,
                            sub_i,
                            b"def_os_param_value_ptr\0",
                        );
                        LLVMBuildLoad2(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, param_slot.ty),
                            value_elem_ptr,
                            b"def_os_param_value\0".as_ptr().cast(),
                        )
                    } else {
                        *raw
                    };
                    LLVMBuildStore(ctx.builder, sub_value, param_slot.ptr);
                }

                for stmt in &def.body {
                    if lower_def_stmt(stmt, &mut ctx)? {
                        terminated = true;
                        break;
                    }
                }

                if !terminated && !current_block_terminated(ctx.builder) {
                    for (out_slot, value_ptr, _) in &output_runtime {
                        let cur = LLVMBuildLoad2(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, out_slot.ty),
                            out_slot.ptr,
                            b"def_os_out_cur\0".as_ptr().cast(),
                        );
                        let value_elem_ptr = build_ptr_offset(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, out_slot.ty),
                            *value_ptr,
                            sub_i,
                            b"def_os_out_value_ptr\0",
                        );
                        LLVMBuildStore(ctx.builder, cur, value_elem_ptr);
                    }
                    LLVMBuildBr(ctx.builder, sub_latch);
                }

                LLVMPositionBuilderAtEnd(ctx.builder, sub_latch);
                let sub_latch_block = LLVMGetInsertBlock(ctx.builder);
                if sub_latch_block.is_null() {
                    return Err(Diagnostic::internal(
                        "failed to get proc oversample latch block in def lowering",
                    ));
                }
                let sub_next = LLVMBuildAdd(
                    ctx.builder,
                    sub_i,
                    one_i32,
                    b"def_os_sub_next\0".as_ptr().cast(),
                );
                LLVMBuildBr(ctx.builder, sub_cond);
                let mut sub_back_vals = [sub_next];
                let mut sub_back_blocks = [sub_latch_block];
                LLVMAddIncoming(
                    sub_i,
                    sub_back_vals.as_mut_ptr(),
                    sub_back_blocks.as_mut_ptr(),
                    1,
                );

                LLVMPositionBuilderAtEnd(ctx.builder, sub_end);
                let last_sub_index = LLVMConstInt(ctx.i32_ty, (oversample_factor - 1) as u64, 0);
                for (out_slot, value_ptr, down_stage_slots) in &output_runtime {
                    let decimated =
                        if matches!(out_slot.ty, PrimitiveType::F32 | PrimitiveType::F64)
                            && !down_stage_slots.is_empty()
                        {
                            let mut reduced = llvm_zero_for_primitive(ctx.context, out_slot.ty);
                            let mut step = 1usize;
                            for (stage_index, stage_slot_set) in down_stage_slots.iter().enumerate()
                            {
                                let tap_ptrs = proc_sinc_stage_slot_ptrs(stage_slot_set);
                                let mut frame = 0usize;
                                while frame < oversample_factor {
                                    let src1_ptr = build_ptr_offset(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                        *value_ptr,
                                        LLVMConstInt(ctx.i32_ty, frame as u64, 0),
                                        b"def_os_dec_src1_ptr\0",
                                    );
                                    let src2_ptr = build_ptr_offset(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                        *value_ptr,
                                        LLVMConstInt(ctx.i32_ty, (frame + step) as u64, 0),
                                        b"def_os_dec_src2_ptr\0",
                                    );
                                    let src1 = LLVMBuildLoad2(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                        src1_ptr,
                                        b"def_os_dec_src1\0".as_ptr().cast(),
                                    );
                                    let src2 = LLVMBuildLoad2(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                        src2_ptr,
                                        b"def_os_dec_src2\0".as_ptr().cast(),
                                    );
                                    let value = build_sinc_decimate(
                                        ctx.builder,
                                        ctx.context,
                                        out_slot.ty,
                                        &tap_ptrs,
                                        src1,
                                        src2,
                                        ctx.fast_math_flags,
                                    );
                                    if stage_index + 1 == down_stage_slots.len() {
                                        reduced = value;
                                    } else {
                                        LLVMBuildStore(ctx.builder, value, src1_ptr);
                                    }
                                    frame += step * 2;
                                }
                                step *= 2;
                            }
                            reduced
                        } else {
                            let last_ptr = build_ptr_offset(
                                ctx.builder,
                                llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                *value_ptr,
                                last_sub_index,
                                b"def_os_last_ptr\0",
                            );
                            LLVMBuildLoad2(
                                ctx.builder,
                                llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                last_ptr,
                                b"def_os_last\0".as_ptr().cast(),
                            )
                        };
                    LLVMBuildStore(ctx.builder, decimated, out_slot.ptr);
                }
            } else {
                for stmt in &def.body {
                    if lower_def_stmt(stmt, &mut ctx)? {
                        terminated = true;
                        break;
                    }
                }
            }
        } else {
            for stmt in &def.body {
                if lower_def_stmt(stmt, &mut ctx)? {
                    terminated = true;
                    break;
                }
            }
        }

        if !terminated && !current_block_terminated(ctx.builder) {
            LLVMBuildBr(ctx.builder, ctx.return_block);
        }

        LLVMPositionBuilderAtEnd(ctx.builder, ctx.return_block);
        let ret_value = LLVMBuildLoad2(
            ctx.builder,
            return_llvm_ty,
            ctx.return_slot,
            b"ret_v\0".as_ptr().cast(),
        );
        LLVMBuildRet(ctx.builder, ret_value);
        Ok(())
    })();

    LLVMDisposeBuilder(builder);
    result
}

fn oversample_local_name(prefix: &str, name: &str) -> String {
    format!("{prefix}_{}", name.replace(['[', ']', '.', ':'], "_"))
}

fn proc_sinc_stage_slot_keys(stage: &ProcSincStageStateFields) -> [&str; 8] {
    [
        &stage.a0, &stage.a1, &stage.a2, &stage.a3, &stage.b0, &stage.b1, &stage.b2, &stage.b3,
    ]
}

fn proc_sinc_stage_slot_ptrs(stage_slots: &[DefLocalSlot; 8]) -> [LLVMValueRef; 8] {
    [
        stage_slots[0].ptr,
        stage_slots[1].ptr,
        stage_slots[2].ptr,
        stage_slots[3].ptr,
        stage_slots[4].ptr,
        stage_slots[5].ptr,
        stage_slots[6].ptr,
        stage_slots[7].ptr,
    ]
}

fn proc_sinc_stage_slots(
    ctx: &DefLoweringCtx<'_>,
    def_name: &str,
    stage: &ProcSincStageStateFields,
) -> Result<[DefLocalSlot; 8], Diagnostic> {
    let keys = proc_sinc_stage_slot_keys(stage);
    let mut slots = [DefLocalSlot {
        ptr: std::ptr::null_mut(),
        ty: PrimitiveType::F32,
    }; 8];
    for (index, key) in keys.into_iter().enumerate() {
        let full_key = format!("self.{key}");
        slots[index] = ctx.local_slots.get(&full_key).copied().ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing proc oversample sinc state '{}' for '{}'",
                full_key, def_name
            ))
        })?;
    }
    Ok(slots)
}
