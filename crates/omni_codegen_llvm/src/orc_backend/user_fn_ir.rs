use super::*;

pub(super) unsafe fn build_user_functions_ir(
    typed: &TypedProgram,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
) -> Result<UserFnRegistry, Diagnostic> {
    let float_ptr_ty = LLVMPointerType(LLVMFloatTypeInContext(context), 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let struct_fields = typed
        .structs
        .iter()
        .map(|s| (s.name.clone(), s.fields.clone()))
        .collect::<HashMap<_, _>>();
    let mut defs = HashMap::new();
    let mut refs = HashMap::new();
    let mut tys = HashMap::new();
    let mut base_return_tys = HashMap::new();
    let mut param_names = HashMap::new();
    let mut param_defaults = HashMap::new();
    let mut param_kinds = HashMap::new();
    let mut param_by_ref = HashMap::new();

    for def in &typed.defs {
        defs.insert(def.name.clone(), def.clone());
        base_return_tys.insert(def.name.clone(), def.return_ty);
        let mut arg_tys = Vec::new();
        let mut by_ref_flags = vec![false; def.param_kinds.len()];
        if def.method_of.is_some() && !def.params.is_empty() && def.params[0] == "self" {
            by_ref_flags[0] = true;
        }
        for (idx, kind) in def.param_kinds.iter().enumerate() {
            match kind {
                TypedFnParam::Scalar => arg_tys.push(LLVMFloatTypeInContext(context)),
                TypedFnParam::Struct { struct_name } => {
                    // Phase 2: all struct parameters are passed by reference.
                    by_ref_flags[idx] = true;
                    let fields = struct_fields.get(struct_name).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "unknown struct '{}' in function '{}' parameter lowering",
                            struct_name, def.name
                        ))
                    })?;
                    for field in fields {
                        match field.ty {
                            TypedFieldType::Scalar(prim) => {
                                if by_ref_flags[idx] {
                                    arg_tys.push(float_ptr_ty);
                                } else {
                                    arg_tys.push(llvm_ty_for_primitive(context, prim));
                                }
                            }
                            TypedFieldType::Data(len) => {
                                if let Some(elem_struct) = &field.data_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_data_struct_bindings(
                                        &struct_fields,
                                        elem_struct,
                                        &format!("{}.{}", def.params[idx], field.name),
                                        len,
                                        &mut roots,
                                        &mut leaves,
                                        &mut Vec::new(),
                                    )?;
                                    for _ in &leaves {
                                        arg_tys.push(float_ptr_ty);
                                    }
                                } else {
                                    arg_tys.push(float_ptr_ty);
                                }
                            }
                        }
                    }
                }
                TypedFnParam::Buffer { .. } => {
                    arg_tys.push(i8_ptr_ty);
                    arg_tys.push(i32_ty);
                    arg_tys.push(i32_ty);
                }
            }
        }
        let ret_ty = llvm_ty_for_primitive(context, def.return_ty);
        let fn_ty = LLVMFunctionType(ret_ty, arg_tys.as_mut_ptr(), arg_tys.len() as u32, 0);
        let symbol = mangle_user_fn_symbol(&def.name)?;
        let fn_ref = LLVMAddFunction(module, symbol.as_ptr(), fn_ty);
        if fn_ref.is_null() {
            return Err(Diagnostic::internal(format!(
                "failed to add user function '{}'",
                def.name
            )));
        }
        if is_proc_glue_function_name(&def.name) {
            set_internal_alwaysinline(fn_ref, context)?;
        }
        refs.insert(def.name.clone(), fn_ref);
        tys.insert(def.name.clone(), fn_ty);
        param_names.insert(def.name.clone(), def.params.clone());
        param_defaults.insert(def.name.clone(), def.param_defaults.clone());
        param_kinds.insert(def.name.clone(), def.param_kinds.clone());
        param_by_ref.insert(def.name.clone(), by_ref_flags);
    }

    let mut registry = UserFnRegistry {
        defs,
        refs,
        tys,
        base_return_tys,
        mono_refs: HashMap::new(),
        mono_tys: HashMap::new(),
        mono_return_tys: HashMap::new(),
        param_names,
        param_defaults,
        param_kinds,
        param_by_ref,
        in_progress: HashSet::new(),
        return_in_progress: HashSet::new(),
    };
    for def in &typed.defs {
        let fn_ref = *registry.refs.get(&def.name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing base LLVM function reference for '{}'",
                def.name
            ))
        })?;
        let scalar_sig = default_scalar_signature(def);
        let buffer_sig = default_buffer_signature(def);
        lower_user_function_body(
            def,
            module,
            context,
            &mut registry,
            &struct_fields,
            sample_rate,
            block_size,
            fast_math,
            fn_ref,
            def.return_ty,
            &scalar_sig,
            &buffer_sig,
        )?;
    }

    Ok(registry)
}

pub(super) unsafe fn ensure_user_fn_specialization(
    module: LLVMModuleRef,
    context: LLVMContextRef,
    registry: &mut UserFnRegistry,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> Result<(LLVMValueRef, LLVMTypeRef, PrimitiveType), Diagnostic> {
    let def = registry
        .defs
        .get(name)
        .ok_or_else(|| Diagnostic::internal(format!("unknown function '{}'", name)))?
        .clone();

    let scalar_count = def
        .param_kinds
        .iter()
        .filter(|k| matches!(k, TypedFnParam::Scalar))
        .count();
    if scalar_types.len() != scalar_count {
        return Err(Diagnostic::internal(format!(
            "function '{}' scalar signature length mismatch: expected {}, got {}",
            name,
            scalar_count,
            scalar_types.len()
        )));
    }
    let buffer_count = def
        .param_kinds
        .iter()
        .filter(|k| matches!(k, TypedFnParam::Buffer { .. }))
        .count();
    if buffer_types.len() != buffer_count {
        return Err(Diagnostic::internal(format!(
            "function '{}' buffer signature length mismatch: expected {}, got {}",
            name,
            buffer_count,
            buffer_types.len()
        )));
    }

    let default_scalar_sig = default_scalar_signature(&def);
    let default_buffer_sig = default_buffer_signature(&def);
    if generic_type_args.is_empty()
        && scalar_types == default_scalar_sig.as_slice()
        && buffer_types == default_buffer_sig.as_slice()
    {
        let fn_ref = *registry.refs.get(name).ok_or_else(|| {
            Diagnostic::internal(format!("missing base function ref for '{}'", name))
        })?;
        let fn_ty = *registry.tys.get(name).ok_or_else(|| {
            Diagnostic::internal(format!("missing base function type for '{}'", name))
        })?;
        let ret_ty = registry
            .base_return_tys
            .get(name)
            .copied()
            .unwrap_or(PrimitiveType::F32);
        return Ok((fn_ref, fn_ty, ret_ty));
    }

    let key = user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args);
    if let (Some(fn_ref), Some(fn_ty), Some(ret_ty)) = (
        registry.mono_refs.get(&key),
        registry.mono_tys.get(&key),
        registry.mono_return_tys.get(&key),
    ) {
        return Ok((*fn_ref, *fn_ty, *ret_ty));
    }

    let ret_ty = infer_specialized_def_return_type(
        name,
        scalar_types,
        buffer_types,
        generic_type_args,
        registry,
    )?;
    let float_ty = LLVMFloatTypeInContext(context);
    let float_ptr_ty = LLVMPointerType(float_ty, 0);
    let i32_ty = LLVMInt32TypeInContext(context);
    let i8_ty = LLVMInt8TypeInContext(context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let by_ref_flags = registry.param_by_ref.get(name).ok_or_else(|| {
        Diagnostic::internal(format!("missing by-ref metadata for function '{}'", name))
    })?;

    let mut arg_tys = Vec::new();
    let mut scalar_idx = 0usize;
    for (param_idx, kind) in def.param_kinds.iter().enumerate() {
        match kind {
            TypedFnParam::Scalar => {
                let param_ty = scalar_types[scalar_idx];
                scalar_idx += 1;
                arg_tys.push(llvm_ty_for_primitive(context, param_ty));
            }
            TypedFnParam::Struct { struct_name } => {
                let fields = struct_fields.get(struct_name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "unknown struct '{}' in function '{}' parameter lowering",
                        struct_name, def.name
                    ))
                })?;
                for field in fields {
                    match field.ty {
                        TypedFieldType::Scalar(prim) => {
                            if by_ref_flags[param_idx] {
                                arg_tys.push(float_ptr_ty);
                            } else {
                                arg_tys.push(llvm_ty_for_primitive(context, prim));
                            }
                        }
                        TypedFieldType::Data(len) => {
                            if let Some(elem_struct) = &field.data_elem_struct {
                                let mut roots = Vec::new();
                                let mut leaves = Vec::new();
                                collect_data_struct_bindings(
                                    struct_fields,
                                    elem_struct,
                                    &format!("{}.{}", def.params[param_idx], field.name),
                                    len,
                                    &mut roots,
                                    &mut leaves,
                                    &mut Vec::new(),
                                )?;
                                for _ in &leaves {
                                    arg_tys.push(float_ptr_ty);
                                }
                            } else {
                                arg_tys.push(float_ptr_ty);
                            }
                        }
                    }
                }
            }
            TypedFnParam::Buffer { .. } => {
                arg_tys.push(i8_ptr_ty);
                arg_tys.push(i32_ty);
                arg_tys.push(i32_ty);
            }
        }
    }

    let ret_llvm_ty = llvm_ty_for_primitive(context, ret_ty);
    let fn_ty = LLVMFunctionType(ret_llvm_ty, arg_tys.as_mut_ptr(), arg_tys.len() as u32, 0);
    let symbol = mangle_user_fn_symbol_mono(name, scalar_types, buffer_types, generic_type_args)?;
    let fn_ref = LLVMAddFunction(module, symbol.as_ptr(), fn_ty);
    if fn_ref.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to add monomorphized function '{}'",
            key
        )));
    }
    if is_proc_glue_function_name(name) {
        set_internal_alwaysinline(fn_ref, context)?;
    }

    registry.mono_refs.insert(key.clone(), fn_ref);
    registry.mono_tys.insert(key.clone(), fn_ty);
    registry.mono_return_tys.insert(key.clone(), ret_ty);
    if registry.in_progress.insert(key.clone()) {
        lower_user_function_body(
            &def,
            module,
            context,
            registry,
            struct_fields,
            sample_rate,
            block_size,
            fast_math,
            fn_ref,
            ret_ty,
            scalar_types,
            buffer_types,
        )?;
        registry.in_progress.remove(&key);
    }

    Ok((fn_ref, fn_ty, ret_ty))
}

pub(super) unsafe fn lower_user_function_body(
    def: &TypedFunction,
    module: LLVMModuleRef,
    context: LLVMContextRef,
    registry: &mut UserFnRegistry,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    sample_rate: f32,
    block_size: usize,
    fast_math: bool,
    fn_ref: LLVMValueRef,
    return_ty: PrimitiveType,
    scalar_param_types: &[PrimitiveType],
    buffer_param_types: &[(PrimitiveType, TypedBufferChannels)],
) -> Result<(), Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let i32_ty = LLVMInt32TypeInContext(context);
    let return_llvm_ty = llvm_ty_for_primitive(context, return_ty);

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
        let zero_ret = llvm_zero_for_primitive(context, return_ty);

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
            local_data_aliases: HashMap::new(),
            buffer_params: HashMap::new(),
            data_ptrs: HashMap::new(),
            data_len: HashMap::new(),
            data_elem_ty: HashMap::new(),
            data_struct_roots: HashMap::new(),
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
        let expected_scalar_count = def
            .param_kinds
            .iter()
            .filter(|k| matches!(k, TypedFnParam::Scalar))
            .count();
        if scalar_param_types.len() != expected_scalar_count {
            return Err(Diagnostic::internal(format!(
                "function '{}' scalar parameter type mismatch: expected {}, got {}",
                def.name,
                expected_scalar_count,
                scalar_param_types.len()
            )));
        }
        let expected_buffer_count = def
            .param_kinds
            .iter()
            .filter(|k| matches!(k, TypedFnParam::Buffer { .. }))
            .count();
        if buffer_param_types.len() != expected_buffer_count {
            return Err(Diagnostic::internal(format!(
                "function '{}' buffer parameter type mismatch: expected {}, got {}",
                def.name,
                expected_buffer_count,
                buffer_param_types.len()
            )));
        }

        let mut llvm_param_idx: u32 = 0;
        let mut scalar_param_idx: usize = 0;
        let mut buffer_param_idx: usize = 0;
        for (param_idx, (param_name, kind)) in
            def.params.iter().zip(def.param_kinds.iter()).enumerate()
        {
            match kind {
                TypedFnParam::Scalar => {
                    let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    if param_val.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM param {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    let param_ty = scalar_param_types[scalar_param_idx];
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
                        let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                        if param_val.is_null() {
                            return Err(Diagnostic::internal(format!(
                                "missing LLVM param {} for function '{}'",
                                llvm_param_idx, def.name
                            )));
                        }
                        let flat = format!("{param_name}.{}", field.name);
                        match field.ty {
                            TypedFieldType::Scalar(prim) => {
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
                            }
                            TypedFieldType::Data(len) => {
                                if let Some(elem_struct) = &field.data_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_data_struct_bindings(
                                        ctx.struct_fields,
                                        elem_struct,
                                        &flat,
                                        len,
                                        &mut roots,
                                        &mut leaves,
                                        &mut Vec::new(),
                                    )?;
                                    for (root_name, root_struct, root_len) in roots {
                                        ctx.data_struct_roots
                                            .insert(root_name.clone(), root_struct);
                                        ctx.data_len.entry(root_name).or_insert(root_len);
                                    }
                                    let mut leaf_iter = leaves.into_iter();
                                    let first = leaf_iter.next().ok_or_else(|| {
                                        Diagnostic::internal(format!(
                                            "Data[Struct] field '{flat}' produced no leaf bindings in def lowering"
                                        ))
                                    })?;
                                    ctx.data_ptrs.insert(first.0.clone(), param_val);
                                    ctx.data_len.insert(first.0.clone(), first.1);
                                    ctx.data_elem_ty.insert(first.0, first.2);
                                    for (leaf_name, leaf_len, leaf_ty) in leaf_iter {
                                        llvm_param_idx += 1;
                                        let leaf_param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                                        if leaf_param_val.is_null() {
                                            return Err(Diagnostic::internal(format!(
                                                "missing LLVM param {} for function '{}'",
                                                llvm_param_idx, def.name
                                            )));
                                        }
                                        ctx.data_ptrs.insert(leaf_name.clone(), leaf_param_val);
                                        ctx.data_len.insert(leaf_name.clone(), leaf_len);
                                        ctx.data_elem_ty.insert(leaf_name, leaf_ty);
                                    }
                                } else {
                                    ctx.data_ptrs.insert(flat.clone(), param_val);
                                    ctx.data_len.insert(flat.clone(), len);
                                    ctx.data_elem_ty.insert(
                                        flat,
                                        field.data_elem_ty.unwrap_or(PrimitiveType::F32),
                                    );
                                }
                            }
                        }
                        llvm_param_idx += 1;
                    }
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
                    if ptr_val.is_null() || frames_val.is_null() || channels_val.is_null() {
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
                            elem_ty,
                            declared_channels: channels,
                        },
                    );
                    llvm_param_idx += 3;
                }
            }
        }

        let mut terminated = false;
        for stmt in &def.body {
            if lower_def_stmt(stmt, &mut ctx)? {
                terminated = true;
                break;
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
