use super::super::*;

fn struct_llvm_name(prefix: &str, suffix: &str) -> Vec<u8> {
    let mut name = format!("{prefix}_{suffix}").into_bytes();
    name.push(0);
    name
}

fn sanitize_runtime_symbol_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
}

fn runtime_proc_array_active_symbol(array_base: &str) -> String {
    format!(
        "__omni_proc_block_active_{}",
        sanitize_runtime_symbol_component(array_base)
    )
}

fn resolve_runtime_proc_array_active_base<'a, T>(
    bases: &'a HashMap<String, T>,
    base: &str,
    context_name: &str,
) -> Result<Option<String>, Diagnostic> {
    let mut candidates = Vec::<String>::new();
    candidates.push(runtime_proc_array_active_symbol(base));
    if let Some(stripped) = base.strip_prefix("self.") {
        candidates.push(runtime_proc_array_active_symbol(stripped));
    } else {
        candidates.push(format!("self.{}", runtime_proc_array_active_symbol(base)));
    }

    for candidate in &candidates {
        if bases.contains_key(candidate) {
            return Ok(Some(candidate.clone()));
        }
    }

    let suffix = format!(".{}", runtime_proc_array_active_symbol(base));
    let matches = bases
        .keys()
        .filter(|key| key.ends_with(&suffix))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(Diagnostic::internal(format!(
            "ambiguous proc-array active-state base for '{base}' in {context_name}"
        )));
    }
    Ok(matches.into_iter().next())
}

pub(in crate::orc_backend) fn collect_struct_array_leaf_symbols(
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    elem_struct: &str,
    flat: &str,
    len: usize,
) -> Result<Vec<(String, PrimitiveType)>, Diagnostic> {
    let mut roots = Vec::new();
    let mut leaves = Vec::new();
    collect_array_struct_bindings(
        struct_fields,
        elem_struct,
        flat,
        len,
        &mut roots,
        &mut leaves,
        &mut Vec::new(),
    )?;
    Ok(leaves
        .into_iter()
        .map(|(name, _, leaf_ty)| (name, leaf_ty))
        .collect())
}

pub(in crate::orc_backend) unsafe fn lower_struct_array_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var { name: base, .. } = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument as a variable in ORC lowering"
        )));
    };
    let actual_struct = ctx.array_struct_roots.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument, but '{base}' is not a struct-array root in ORC lowering"
        ))
    })?;
    if actual_struct != struct_name {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument, but '{base}' has element type '{actual_struct}' in ORC lowering"
        )));
    }
    let len_val = ctx.array_len_values.get(base).copied().unwrap_or_else(|| {
        LLVMConstInt(
            ctx.i32_ty,
            ctx.array_struct_len.get(base).copied().unwrap_or(1) as u64,
            0,
        )
    });
    out_args.push(len_val);
    for (leaf_name, _leaf_ty) in
        collect_struct_array_leaf_symbols(ctx.struct_fields, struct_name, base, 1)?
    {
        out_args.push(ctx.array_base_ptrs.get(&leaf_name).copied().ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing ORC array symbol '{leaf_name}' while lowering struct-array argument for '{callee_name}'"
            ))
        })?);
    }
    Ok(())
}

pub(in crate::orc_backend) unsafe fn lower_proc_array_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var { name: base, .. } = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects proc-array '{struct_name}[]' argument as a variable in ORC lowering"
        )));
    };
    let actual_struct = ctx.array_struct_roots.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "function '{callee_name}' expects proc-array '{struct_name}[]' argument, but '{base}' is not a proc-array root in ORC lowering"
        ))
    })?;
    if actual_struct != struct_name {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects proc-array '{struct_name}[]' argument, but '{base}' has element type '{actual_struct}' in ORC lowering"
        )));
    }
    let len_val = ctx.array_len_values.get(base).copied().unwrap_or_else(|| {
        LLVMConstInt(
            ctx.i32_ty,
            ctx.array_struct_len.get(base).copied().unwrap_or(1) as u64,
            0,
        )
    });
    out_args.push(len_val);
    let bool_ptr_ty = LLVMPointerType(llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool), 0);
    let active_ptr = if let Some(active_base) = resolve_runtime_proc_array_active_base(
        ctx.array_base_ptrs,
        base,
        "ORC proc-array call lowering",
    )? {
        ctx.array_base_ptrs.get(&active_base).copied().ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing ORC proc-array active-state symbol '{active_base}' while lowering '{callee_name}'"
            ))
        })?
    } else {
        LLVMConstPointerNull(bool_ptr_ty)
    };
    out_args.push(active_ptr);
    for (leaf_name, _leaf_ty) in
        collect_struct_array_leaf_symbols(ctx.struct_fields, struct_name, base, 1)?
    {
        out_args.push(ctx.array_base_ptrs.get(&leaf_name).copied().ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing ORC array symbol '{leaf_name}' while lowering proc-array argument for '{callee_name}'"
            ))
        })?);
    }
    Ok(())
}

unsafe fn push_scalar_struct_arg(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    out_args: &mut Vec<LLVMValueRef>,
    ptr: LLVMValueRef,
    ty: PrimitiveType,
    by_ref: bool,
    load_name: &[u8],
) {
    if by_ref {
        out_args.push(ptr);
    } else {
        let value = LLVMBuildLoad2(
            builder,
            llvm_ty_for_primitive(context, ty),
            ptr,
            load_name.as_ptr().cast(),
        );
        out_args.push(value);
    }
}

fn find_struct_array_alias_by_base<'a>(
    local_array_aliases: &'a HashMap<String, LocalArrayAlias>,
    base: &str,
    context_name: &str,
) -> Result<Option<&'a LocalArrayAlias>, Diagnostic> {
    if let Some(alias) = local_array_aliases.get(base) {
        return Ok(Some(alias));
    }
    if let Some(stripped) = base.strip_prefix("self.") {
        if let Some(alias) = local_array_aliases.get(stripped) {
            return Ok(Some(alias));
        }
    } else {
        let self_base = format!("self.{base}");
        if let Some(alias) = local_array_aliases.get(&self_base) {
            return Ok(Some(alias));
        }
    }

    let suffix = format!(".{base}");
    let matches = local_array_aliases
        .iter()
        .filter_map(|(key, alias)| {
            if key.ends_with(&suffix) && matches!(alias, LocalArrayAlias::Struct { .. }) {
                Some(alias)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(Diagnostic::internal(format!(
            "ambiguous struct-array alias base '{base}[...]' in {context_name}"
        )));
    }
    Ok(matches.first().copied())
}

fn resolve_struct_array_root_base(
    array_struct_roots: &HashMap<String, String>,
    base: &str,
    context_name: &str,
) -> Result<Option<String>, Diagnostic> {
    let self_base = format!("self.{base}");
    let suffix = format!(".{base}");
    let suffix_match =
        if !array_struct_roots.contains_key(base) && !array_struct_roots.contains_key(&self_base) {
            let matches = array_struct_roots
                .keys()
                .filter(|key| key.ends_with(&suffix))
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                return Err(Diagnostic::internal(format!(
                    "ambiguous struct-array root base '{base}[...]' in {context_name}"
                )));
            }
            matches.into_iter().next().cloned()
        } else {
            None
        };

    Ok(if array_struct_roots.contains_key(base) {
        Some(base.to_owned())
    } else if array_struct_roots.contains_key(&self_base) {
        Some(self_base)
    } else if let Some(stripped) = base.strip_prefix("self.") {
        if array_struct_roots.contains_key(stripped) {
            Some(stripped.to_owned())
        } else {
            suffix_match
        }
    } else {
        suffix_match
    })
}

pub(in crate::orc_backend) unsafe fn resolve_indexed_struct_arg_common<FLowerClampedIndex>(
    builder: LLVMBuilderRef,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    array_struct_roots: &HashMap<String, String>,
    array_struct_len: &HashMap<String, usize>,
    array_struct_len_values: Option<&HashMap<String, LLVMValueRef>>,
    base: &str,
    struct_name: &str,
    callee_name: &str,
    context_name: &str,
    alias_global_name: &[u8],
    mut lower_clamped_index: FLowerClampedIndex,
) -> Result<Option<(String, LLVMValueRef)>, Diagnostic>
where
    FLowerClampedIndex: FnMut(usize, Option<LLVMValueRef>) -> Result<LLVMValueRef, Diagnostic>,
{
    if let Some(alias) = find_struct_array_alias_by_base(local_array_aliases, base, context_name)? {
        return match alias {
            LocalArrayAlias::Primitive { .. } => Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' resolves to a primitive array alias in {context_name}"
            ))),
            LocalArrayAlias::Struct {
                root_base,
                elem_struct,
                len,
                start_index,
            } => {
                if elem_struct != struct_name {
                    return Err(Diagnostic::internal(format!(
                        "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' alias element type is '{elem_struct}' in {context_name}"
                    )));
                }
                if *len == 0 {
                    return Err(Diagnostic::internal(format!(
                        "struct-array alias argument '{base}' has zero length in {context_name} for function '{callee_name}'"
                    )));
                }
                let clamped_local_idx = lower_clamped_index(*len, None)?;
                let global_index = LLVMBuildAdd(
                    builder,
                    *start_index,
                    clamped_local_idx,
                    alias_global_name.as_ptr().cast(),
                );
                Ok(Some((root_base.clone(), global_index)))
            }
        };
    }

    let Some(resolved_base) =
        resolve_struct_array_root_base(array_struct_roots, base, context_name)?
    else {
        return Ok(None);
    };
    let indexed_struct = array_struct_roots.get(&resolved_base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing struct-array root metadata for '{resolved_base}[...]' in {context_name}"
        ))
    })?;
    if indexed_struct != struct_name {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' has element type '{indexed_struct}' in {context_name}"
        )));
    }
    let len = array_struct_len.get(&resolved_base).copied().ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing struct-array length metadata for '{resolved_base}[...]' in {context_name} for function '{callee_name}'"
        ))
    })?;
    if len == 0 {
        return Err(Diagnostic::internal(format!(
            "struct-array argument '{resolved_base}' has zero length in {context_name} for function '{callee_name}'"
        )));
    }
    let runtime_len = array_struct_len_values.and_then(|lens| lens.get(&resolved_base).copied());
    Ok(Some((
        resolved_base,
        lower_clamped_index(len, runtime_len)?,
    )))
}

pub(in crate::orc_backend) unsafe fn lower_struct_call_args_for_base_common<
    FLookupStructArrayLen,
    FLookupScalarPtr,
    FLookupArrayPtr,
>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    out_args: &mut Vec<LLVMValueRef>,
    base: &str,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
    context_name: &str,
    llvm_prefix: &str,
    mut lookup_struct_array_len: FLookupStructArrayLen,
    mut lookup_scalar_ptr: FLookupScalarPtr,
    mut lookup_array_ptr: FLookupArrayPtr,
) -> Result<(), Diagnostic>
where
    FLookupStructArrayLen: FnMut(&str) -> Result<usize, Diagnostic>,
    FLookupScalarPtr: FnMut(&str) -> Result<(LLVMValueRef, PrimitiveType), Diagnostic>,
    FLookupArrayPtr: FnMut(&str) -> Result<LLVMValueRef, Diagnostic>,
{
    let fields = struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in {context_name} for function '{callee_name}'"
        ))
    })?;
    let load_name = struct_llvm_name(llvm_prefix, "arg_load");
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(_) => {
                let (ptr, ty) = lookup_scalar_ptr(&flat)?;
                push_scalar_struct_arg(
                    builder,
                    context,
                    out_args,
                    ptr,
                    ty,
                    by_ref,
                    load_name.as_slice(),
                );
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, _prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{flat}.__{idx}");
                    let (ptr, ty) = lookup_scalar_ptr(&elem_flat)?;
                    push_scalar_struct_arg(
                        builder,
                        context,
                        out_args,
                        ptr,
                        ty,
                        by_ref,
                        load_name.as_slice(),
                    );
                }
            }
            TypedFieldType::Array(_) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let root_len = lookup_struct_array_len(&flat)?;
                    for (leaf_name, _) in collect_struct_array_leaf_symbols(
                        struct_fields,
                        elem_struct,
                        &flat,
                        root_len,
                    )? {
                        out_args.push(lookup_array_ptr(&leaf_name)?);
                    }
                } else {
                    out_args.push(lookup_array_ptr(&flat)?);
                }
            }
        }
    }
    Ok(())
}

pub(in crate::orc_backend) unsafe fn lower_struct_call_args_for_indexed_base_common<FLoadPtr>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    i32_ty: LLVMTypeRef,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    out_args: &mut Vec<LLVMValueRef>,
    resolved_base: &str,
    clamped_index: LLVMValueRef,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
    context_name: &str,
    llvm_prefix: &str,
    mut load_ptr_at_index: FLoadPtr,
) -> Result<(), Diagnostic>
where
    FLoadPtr: FnMut(&str, PrimitiveType, LLVMValueRef, &[u8]) -> Result<LLVMValueRef, Diagnostic>,
{
    let fields = struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in {context_name} for function '{callee_name}'"
        ))
    })?;
    let scalar_ptr_name = struct_llvm_name(llvm_prefix, "idx_arg_ptr");
    let scalar_load_name = struct_llvm_name(llvm_prefix, "idx_arg_load");
    let leaf_ptr_name = struct_llvm_name(llvm_prefix, "idx_leaf_ptr");
    let array_ptr_name = struct_llvm_name(llvm_prefix, "idx_arr_ptr");
    for field in fields {
        let flat = format!("{resolved_base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                let ptr =
                    load_ptr_at_index(&flat, prim, clamped_index, scalar_ptr_name.as_slice())?;
                push_scalar_struct_arg(
                    builder,
                    context,
                    out_args,
                    ptr,
                    prim,
                    by_ref,
                    scalar_load_name.as_slice(),
                );
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{flat}.__{idx}");
                    let ptr = load_ptr_at_index(
                        &elem_flat,
                        *prim,
                        clamped_index,
                        scalar_ptr_name.as_slice(),
                    )?;
                    push_scalar_struct_arg(
                        builder,
                        context,
                        out_args,
                        ptr,
                        *prim,
                        by_ref,
                        scalar_load_name.as_slice(),
                    );
                }
            }
            TypedFieldType::Array(field_len) => {
                let start_idx =
                    build_data_segment_start_index(builder, i32_ty, clamped_index, field_len)?;
                if let Some(elem_struct) = &field.array_elem_struct {
                    for (leaf_name, leaf_ty) in collect_struct_array_leaf_symbols(
                        struct_fields,
                        elem_struct,
                        &flat,
                        field_len,
                    )? {
                        out_args.push(load_ptr_at_index(
                            &leaf_name,
                            leaf_ty,
                            start_idx,
                            leaf_ptr_name.as_slice(),
                        )?);
                    }
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    out_args.push(load_ptr_at_index(
                        &flat,
                        elem_ty,
                        start_idx,
                        array_ptr_name.as_slice(),
                    )?);
                }
            }
        }
    }
    Ok(())
}

pub(in crate::orc_backend) unsafe fn lower_struct_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<(), Diagnostic> {
    match arg_expr {
        Expr::Var { name: base, .. } => {
            lower_struct_call_args_for_base_common(
                ctx.builder,
                ctx.context,
                ctx.struct_fields,
                out_args,
                base,
                struct_name,
                callee_name,
                by_ref,
                "ORC call lowering",
                "struct",
                |flat| {
                    ctx.array_struct_len.get(flat).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing array[Struct] length metadata for '{flat}' while lowering struct argument for '{callee_name}'"
                        ))
                    })
                },
                |flat| {
                    let slot = ctx.state_slots.get(flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing state slot for struct field '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    Ok((slot.ptr, slot.ty))
                },
                |symbol| {
                    ctx.array_base_ptrs.get(symbol).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing array symbol '{symbol}' while lowering struct argument for '{callee_name}'"
                        ))
                    })
                },
            )?;
        }
        Expr::Index { base, index, .. } => {
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            let Some((resolved_base, clamped_index)) = resolve_indexed_struct_arg_common(
                ctx.builder,
                local_array_aliases,
                ctx.array_struct_roots,
                ctx.array_struct_len,
                None,
                base,
                struct_name,
                callee_name,
                "ORC lowering",
                b"struct_idx_alias_global\0",
                |len, _runtime_len| unsafe {
                    lower_clamped_data_index(
                        &mut *ctx_ptr,
                        index,
                        len,
                        locals,
                        local_aliases,
                        local_array_aliases,
                        local_tuples,
                    )
                },
            )?
            else {
                return Err(Diagnostic::internal(format!(
                    "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' is not a struct-array root in ORC lowering"
                )));
            };

            lower_struct_call_args_for_indexed_base_common(
                ctx.builder,
                ctx.context,
                ctx.i32_ty,
                ctx.struct_fields,
                out_args,
                &resolved_base,
                clamped_index,
                struct_name,
                callee_name,
                by_ref,
                "ORC call lowering",
                "struct",
                |flat, elem_ty, index, gep_name| unsafe {
                    load_data_ptr_at_index(&mut *ctx_ptr, flat, elem_ty, index, gep_name)
                },
            )?;
        }
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable or indexed struct-array element in ORC lowering"
            )));
        }
    }
    Ok(())
}
