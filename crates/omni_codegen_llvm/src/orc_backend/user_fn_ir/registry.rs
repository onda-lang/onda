use super::super::*;
use super::*;

pub(in crate::orc_backend) unsafe fn build_user_functions_ir(
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
                TypedFnParam::Scalar { ty } => arg_tys.push(llvm_ty_for_primitive(
                    context,
                    resolve_scalar_param_type(*ty, PrimitiveType::F32),
                )),
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
                            TypedFieldType::Struct => {}
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
                    arg_tys.push(i32_ty);
                }
                TypedFnParam::Buffer { .. } => {
                    arg_tys.push(i8_ptr_ty);
                    arg_tys.push(i32_ty);
                    arg_tys.push(i32_ty);
                    arg_tys.push(LLVMFloatTypeInContext(context));
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

pub(in crate::orc_backend) unsafe fn ensure_user_fn_specialization(
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

    validate_param_signatures(
        name,
        &def.param_kinds,
        scalar_types,
        array_types,
        buffer_types,
        "specialization",
    )?;

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
            TypedFnParam::Scalar { ty } => {
                let param_ty = scalar_types[scalar_idx];
                let param_ty = resolve_scalar_param_type(*ty, param_ty);
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
                        TypedFieldType::Struct => {}
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
                arg_tys.push(i32_ty);
            }
            TypedFnParam::Buffer { .. } => {
                arg_tys.push(i8_ptr_ty);
                arg_tys.push(i32_ty);
                arg_tys.push(i32_ty);
                arg_tys.push(LLVMFloatTypeInContext(context));
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
