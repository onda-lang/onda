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
                            TypedFieldType::Array(len) => {
                                if let Some(elem_struct) = &field.array_elem_struct {
                                    let mut roots = Vec::new();
                                    let mut leaves = Vec::new();
                                    collect_array_struct_bindings(
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
                TypedFnParam::Array { elem_ty } => {
                    arg_tys.push(LLVMPointerType(llvm_ty_for_primitive(context, *elem_ty), 0));
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
        param_names.insert(def.name.clone(), def.params.clone());
        param_defaults.insert(def.name.clone(), def.param_defaults.clone());
        param_kinds.insert(def.name.clone(), def.param_kinds.clone());
        param_by_ref.insert(def.name.clone(), by_ref_flags);
    }

    let mut registry = UserFnRegistry {
        defs,
        sample_oversample_factors: typed.def_sample_oversample_factors.clone(),
        proc_step_oversample_meta: typed.proc_step_oversample_meta.clone(),
        refs,
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
        let array_sig = default_array_signature(def);
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
            &array_sig,
            &buffer_sig,
        )?;
    }

    Ok(registry)
}

fn effective_callee_sample_rate(registry: &UserFnRegistry, name: &str, sample_rate: f32) -> f32 {
    let oversample_factor = registry
        .sample_oversample_factors
        .get(name)
        .copied()
        .unwrap_or(1)
        .max(1) as f32;
    sample_rate * oversample_factor
}

fn oversample_iir_coeff(factor: usize) -> f64 {
    match factor {
        1 => 1.0,
        2 => 0.633_974_596_2,
        4 => 0.456_786_383_1,
        8 => 0.245_237_275_3,
        16 => 0.127_739_580_9,
        32 => 0.064_405_735_1,
        64 => 0.032_210_026_3,
        _ => (0.5_f64).min(1.0 / factor.max(1) as f64),
    }
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
    array_types: &[(PrimitiveType, usize)],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> Result<(LLVMValueRef, LLVMTypeRef, PrimitiveType), Diagnostic> {
    let effective_sample_rate = effective_callee_sample_rate(registry, name, sample_rate);
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
    let array_count = def
        .param_kinds
        .iter()
        .filter(|k| matches!(k, TypedFnParam::Array { .. }))
        .count();
    if array_types.len() != array_count {
        return Err(Diagnostic::internal(format!(
            "function '{}' array signature length mismatch: expected {}, got {}",
            name,
            array_count,
            array_types.len()
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

    let base_key = user_fn_mono_key(
        name,
        scalar_types,
        array_types,
        buffer_types,
        generic_type_args,
    );
    let context_suffix = format!(
        "__sr_{:08x}__bs_{:08x}",
        effective_sample_rate.to_bits(),
        (block_size as u32)
    );
    let key = format!("{base_key}{context_suffix}");
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
        array_types,
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
    let mut array_idx = 0usize;
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
                        TypedFieldType::Array(len) => {
                            if let Some(elem_struct) = &field.array_elem_struct {
                                let mut roots = Vec::new();
                                let mut leaves = Vec::new();
                                collect_array_struct_bindings(
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
            TypedFnParam::Array { .. } => {
                let (elem_ty, _len) = array_types.get(array_idx).copied().ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing array signature for '{}' at array index {}",
                        name, array_idx
                    ))
                })?;
                array_idx += 1;
                arg_tys.push(LLVMPointerType(llvm_ty_for_primitive(context, elem_ty), 0));
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
    let symbol = mangle_user_fn_symbol_mono(
        name,
        scalar_types,
        array_types,
        buffer_types,
        generic_type_args,
        effective_sample_rate,
        block_size,
    )?;
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
            effective_sample_rate,
            block_size,
            fast_math,
            fn_ref,
            ret_ty,
            scalar_types,
            array_types,
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
    array_param_types: &[(PrimitiveType, usize)],
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
            local_array_aliases: HashMap::new(),
            buffer_params: HashMap::new(),
            array_ptrs: HashMap::new(),
            array_len: HashMap::new(),
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
        let expected_array_count = def
            .param_kinds
            .iter()
            .filter(|k| matches!(k, TypedFnParam::Array { .. }))
            .count();
        if array_param_types.len() != expected_array_count {
            return Err(Diagnostic::internal(format!(
                "function '{}' array parameter type mismatch: expected {}, got {}",
                def.name,
                expected_array_count,
                array_param_types.len()
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
                            TypedFieldType::Array(len) => {
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
                            }
                        }
                        llvm_param_idx += 1;
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
                    let param_val = LLVMGetParam(fn_ref, llvm_param_idx);
                    if param_val.is_null() {
                        return Err(Diagnostic::internal(format!(
                            "missing LLVM array param {} for function '{}'",
                            llvm_param_idx, def.name
                        )));
                    }
                    ctx.array_ptrs.insert(param_name.clone(), param_val);
                    ctx.array_len.insert(param_name.clone(), len);
                    ctx.array_elem_ty.insert(param_name.clone(), elem_ty);
                    llvm_param_idx += 1;
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
        if oversample_factor > 1 {
            if let Some(meta) = proc_step_os_meta {
                let zero_i32 = LLVMConstInt(ctx.i32_ty, 0, 0);
                let one_i32 = LLVMConstInt(ctx.i32_ty, 1, 0);
                let sub_factor_const = LLVMConstInt(ctx.i32_ty, oversample_factor as u64, 0);
                let sub_factor_f = LLVMConstReal(ctx.float_ty, oversample_factor as f64);
                let up_down_coeff_f32 =
                    LLVMConstReal(ctx.float_ty, oversample_iir_coeff(oversample_factor));

                let mut input_runtime = Vec::<(
                    String,
                    DefLocalSlot,
                    DefLocalSlot,
                    DefLocalSlot,
                    DefLocalSlot,
                    LLVMValueRef,
                    LLVMValueRef,
                )>::new();
                for (param_name, state_fields) in &meta.input_state_fields {
                    let param_slot = ctx.local_slots.get(param_name).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample input param slot '{}' for '{}'",
                            param_name, def.name
                        ))
                    })?;
                    let prev_key = format!("self.{}", state_fields.prev);
                    let up1_key = format!("self.{}", state_fields.up1);
                    let up2_key = format!("self.{}", state_fields.up2);
                    let prev_slot = ctx.local_slots.get(&prev_key).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample prev state '{}' for '{}'",
                            prev_key, def.name
                        ))
                    })?;
                    let up1_slot = ctx.local_slots.get(&up1_key).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample upsample state '{}' for '{}'",
                            up1_key, def.name
                        ))
                    })?;
                    let up2_slot = ctx.local_slots.get(&up2_key).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample upsample state '{}' for '{}'",
                            up2_key, def.name
                        ))
                    })?;
                    let raw = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, param_slot.ty),
                        param_slot.ptr,
                        b"os_param_raw\0".as_ptr().cast(),
                    );
                    let raw_f32 = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: raw,
                            ty: param_slot.ty,
                        },
                        PrimitiveType::F32,
                        b"os_param_raw_f32\0",
                    );
                    let prev_raw = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, prev_slot.ty),
                        prev_slot.ptr,
                        b"os_prev_raw\0".as_ptr().cast(),
                    );
                    let prev_f32 = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: prev_raw,
                            ty: prev_slot.ty,
                        },
                        PrimitiveType::F32,
                        b"os_prev_f32\0",
                    );
                    input_runtime.push((
                        param_name.clone(),
                        param_slot,
                        prev_slot,
                        up1_slot,
                        up2_slot,
                        raw_f32,
                        prev_f32,
                    ));
                }

                let mut output_runtime = Vec::<(
                    String,
                    DefLocalSlot,
                    LLVMValueRef,
                    Option<DefLocalSlot>,
                    Option<DefLocalSlot>,
                )>::new();
                for (output_field, state_fields) in &meta.output_state_fields {
                    let output_key = format!("self.{output_field}");
                    let out_slot = ctx.local_slots.get(&output_key).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing proc oversample output slot '{}' for '{}'",
                            output_key, def.name
                        ))
                    })?;
                    let acc_name = format!(
                        "os_acc_{}",
                        output_field.replace('.', "_").replace(':', "_")
                    );
                    let acc_ptr = build_local_slot(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                        &acc_name,
                    )?;
                    LLVMBuildStore(
                        ctx.builder,
                        llvm_zero_for_primitive(ctx.context, out_slot.ty),
                        acc_ptr,
                    );
                    let down1_slot = state_fields
                        .down1
                        .as_ref()
                        .map(|name| {
                            let key = format!("self.{name}");
                            ctx.local_slots.get(&key).copied().ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing proc oversample downsample state '{}' for '{}'",
                                    key, def.name
                                ))
                            })
                        })
                        .transpose()?;
                    let down2_slot = state_fields
                        .down2
                        .as_ref()
                        .map(|name| {
                            let key = format!("self.{name}");
                            ctx.local_slots.get(&key).copied().ok_or_else(|| {
                                Diagnostic::internal(format!(
                                    "missing proc oversample downsample state '{}' for '{}'",
                                    key, def.name
                                ))
                            })
                        })
                        .transpose()?;
                    output_runtime.push((
                        output_field.clone(),
                        out_slot,
                        acc_ptr,
                        down1_slot,
                        down2_slot,
                    ));
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
                let sub_i_plus_one = LLVMBuildAdd(
                    ctx.builder,
                    sub_i,
                    one_i32,
                    b"def_os_sub_i1\0".as_ptr().cast(),
                );
                let sub_i_plus_one_f = LLVMBuildSIToFP(
                    ctx.builder,
                    sub_i_plus_one,
                    ctx.float_ty,
                    b"def_os_sub_i1_f\0".as_ptr().cast(),
                );
                let sub_alpha = build_fdiv_fast(
                    ctx.builder,
                    sub_i_plus_one_f,
                    sub_factor_f,
                    b"def_os_sub_alpha\0",
                    ctx.fast_math_flags,
                );

                for (_, param_slot, _prev_slot, up1_slot, up2_slot, raw_f32, prev_f32) in
                    &input_runtime
                {
                    let diff = build_fsub_fast(
                        ctx.builder,
                        *raw_f32,
                        *prev_f32,
                        b"def_os_in_diff\0",
                        ctx.fast_math_flags,
                    );
                    let scaled = build_fmul_fast(
                        ctx.builder,
                        diff,
                        sub_alpha,
                        b"def_os_in_scaled\0",
                        ctx.fast_math_flags,
                    );
                    let interp = build_fadd_fast(
                        ctx.builder,
                        *prev_f32,
                        scaled,
                        b"def_os_in_interp\0",
                        ctx.fast_math_flags,
                    );

                    let up1_old = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, up1_slot.ty),
                        up1_slot.ptr,
                        b"def_os_up1_old\0".as_ptr().cast(),
                    );
                    let up1_old_f32 = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: up1_old,
                            ty: up1_slot.ty,
                        },
                        PrimitiveType::F32,
                        b"def_os_up1_old_f32\0",
                    );
                    let up1_delta = build_fsub_fast(
                        ctx.builder,
                        interp,
                        up1_old_f32,
                        b"def_os_up1_delta\0",
                        ctx.fast_math_flags,
                    );
                    let up1_step = build_fmul_fast(
                        ctx.builder,
                        up1_delta,
                        up_down_coeff_f32,
                        b"def_os_up1_step\0",
                        ctx.fast_math_flags,
                    );
                    let up1_new = build_fadd_fast(
                        ctx.builder,
                        up1_old_f32,
                        up1_step,
                        b"def_os_up1_new\0",
                        ctx.fast_math_flags,
                    );
                    let up1_store = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: up1_new,
                            ty: PrimitiveType::F32,
                        },
                        up1_slot.ty,
                        b"def_os_up1_store\0",
                    );
                    LLVMBuildStore(ctx.builder, up1_store, up1_slot.ptr);

                    let up2_old = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, up2_slot.ty),
                        up2_slot.ptr,
                        b"def_os_up2_old\0".as_ptr().cast(),
                    );
                    let up2_old_f32 = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: up2_old,
                            ty: up2_slot.ty,
                        },
                        PrimitiveType::F32,
                        b"def_os_up2_old_f32\0",
                    );
                    let up2_delta = build_fsub_fast(
                        ctx.builder,
                        up1_new,
                        up2_old_f32,
                        b"def_os_up2_delta\0",
                        ctx.fast_math_flags,
                    );
                    let up2_step = build_fmul_fast(
                        ctx.builder,
                        up2_delta,
                        up_down_coeff_f32,
                        b"def_os_up2_step\0",
                        ctx.fast_math_flags,
                    );
                    let up2_new = build_fadd_fast(
                        ctx.builder,
                        up2_old_f32,
                        up2_step,
                        b"def_os_up2_new\0",
                        ctx.fast_math_flags,
                    );
                    let up2_store = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: up2_new,
                            ty: PrimitiveType::F32,
                        },
                        up2_slot.ty,
                        b"def_os_up2_store\0",
                    );
                    LLVMBuildStore(ctx.builder, up2_store, up2_slot.ptr);

                    let interp_cast = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: up2_new,
                            ty: PrimitiveType::F32,
                        },
                        param_slot.ty,
                        b"def_os_param_cast\0",
                    );
                    LLVMBuildStore(ctx.builder, interp_cast, param_slot.ptr);
                }

                for stmt in &def.body {
                    if lower_def_stmt(stmt, &mut ctx)? {
                        terminated = true;
                        break;
                    }
                }

                if !terminated && !current_block_terminated(ctx.builder) {
                    for (_, out_slot, acc_ptr, down1_slot, down2_slot) in &output_runtime {
                        let cur_raw = LLVMBuildLoad2(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, out_slot.ty),
                            out_slot.ptr,
                            b"def_os_out_cur\0".as_ptr().cast(),
                        );
                        let cur = match out_slot.ty {
                            PrimitiveType::F32 | PrimitiveType::F64 => {
                                if let (Some(down1), Some(down2)) = (down1_slot, down2_slot) {
                                    let stage_ty = llvm_ty_for_primitive(ctx.context, out_slot.ty);
                                    let coeff = LLVMConstReal(
                                        stage_ty,
                                        oversample_iir_coeff(oversample_factor),
                                    );
                                    let stage1_old = LLVMBuildLoad2(
                                        ctx.builder,
                                        stage_ty,
                                        down1.ptr,
                                        b"def_os_down1_old\0".as_ptr().cast(),
                                    );
                                    let stage1_delta = build_fsub_fast(
                                        ctx.builder,
                                        cur_raw,
                                        stage1_old,
                                        b"def_os_down1_delta\0",
                                        ctx.fast_math_flags,
                                    );
                                    let stage1_step = build_fmul_fast(
                                        ctx.builder,
                                        stage1_delta,
                                        coeff,
                                        b"def_os_down1_step\0",
                                        ctx.fast_math_flags,
                                    );
                                    let stage1_new = build_fadd_fast(
                                        ctx.builder,
                                        stage1_old,
                                        stage1_step,
                                        b"def_os_down1_new\0",
                                        ctx.fast_math_flags,
                                    );
                                    LLVMBuildStore(ctx.builder, stage1_new, down1.ptr);
                                    let stage2_old = LLVMBuildLoad2(
                                        ctx.builder,
                                        stage_ty,
                                        down2.ptr,
                                        b"def_os_down2_old\0".as_ptr().cast(),
                                    );
                                    let stage2_delta = build_fsub_fast(
                                        ctx.builder,
                                        stage1_new,
                                        stage2_old,
                                        b"def_os_down2_delta\0",
                                        ctx.fast_math_flags,
                                    );
                                    let stage2_step = build_fmul_fast(
                                        ctx.builder,
                                        stage2_delta,
                                        coeff,
                                        b"def_os_down2_step\0",
                                        ctx.fast_math_flags,
                                    );
                                    let stage2_new = build_fadd_fast(
                                        ctx.builder,
                                        stage2_old,
                                        stage2_step,
                                        b"def_os_down2_new\0",
                                        ctx.fast_math_flags,
                                    );
                                    LLVMBuildStore(ctx.builder, stage2_new, down2.ptr);
                                    stage2_new
                                } else {
                                    cur_raw
                                }
                            }
                            _ => cur_raw,
                        };
                        let acc_old = LLVMBuildLoad2(
                            ctx.builder,
                            llvm_ty_for_primitive(ctx.context, out_slot.ty),
                            *acc_ptr,
                            b"def_os_acc_old\0".as_ptr().cast(),
                        );
                        let acc_new = match out_slot.ty {
                            PrimitiveType::F32 | PrimitiveType::F64 => build_fadd_fast(
                                ctx.builder,
                                acc_old,
                                cur,
                                b"def_os_acc_add\0",
                                ctx.fast_math_flags,
                            ),
                            PrimitiveType::I32 | PrimitiveType::I64 => LLVMBuildAdd(
                                ctx.builder,
                                acc_old,
                                cur,
                                b"def_os_acc_add_i\0".as_ptr().cast(),
                            ),
                            PrimitiveType::Bool => cur,
                        };
                        LLVMBuildStore(ctx.builder, acc_new, *acc_ptr);
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
                for (_, out_slot, acc_ptr, _, _) in &output_runtime {
                    let acc = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, out_slot.ty),
                        *acc_ptr,
                        b"def_os_acc_load\0".as_ptr().cast(),
                    );
                    let decimated = match out_slot.ty {
                        PrimitiveType::F32 | PrimitiveType::F64 => {
                            let denom = LLVMConstReal(
                                llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                oversample_factor as f64,
                            );
                            build_fdiv_fast(
                                ctx.builder,
                                acc,
                                denom,
                                b"def_os_decim\0",
                                ctx.fast_math_flags,
                            )
                        }
                        PrimitiveType::I32 | PrimitiveType::I64 => {
                            let denom = LLVMConstInt(
                                llvm_ty_for_primitive(ctx.context, out_slot.ty),
                                oversample_factor as u64,
                                0,
                            );
                            LLVMBuildSDiv(
                                ctx.builder,
                                acc,
                                denom,
                                b"def_os_decim_i\0".as_ptr().cast(),
                            )
                        }
                        PrimitiveType::Bool => acc,
                    };
                    LLVMBuildStore(ctx.builder, decimated, out_slot.ptr);
                }
                for (_, _param_slot, prev_slot, _up1_slot, _up2_slot, raw_f32, _) in &input_runtime
                {
                    let prev_store = cast_def_value_to(
                        &ctx,
                        OrcValue {
                            value: *raw_f32,
                            ty: PrimitiveType::F32,
                        },
                        prev_slot.ty,
                        b"def_os_prev_store\0",
                    );
                    LLVMBuildStore(ctx.builder, prev_store, prev_slot.ptr);
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
