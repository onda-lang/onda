use super::super::*;

fn struct_llvm_name(prefix: &str, suffix: &str) -> Vec<u8> {
    let mut name = format!("{prefix}_{suffix}").into_bytes();
    name.push(0);
    name
}

fn collect_struct_array_leaf_symbols(
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
) -> Result<(), Diagnostic> {
    match arg_expr {
        Expr::Var(base) => {
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
        Expr::Index { base, index } => {
            let local_alias = if let Some(alias) = local_array_aliases.get(base) {
                Some(alias)
            } else if let Some(stripped) = base.strip_prefix("self.") {
                local_array_aliases.get(stripped)
            } else {
                let self_base = format!("self.{base}");
                if let Some(alias) = local_array_aliases.get(&self_base) {
                    Some(alias)
                } else {
                    let suffix = format!(".{base}");
                    let matches = local_array_aliases
                        .iter()
                        .filter_map(|(k, v)| {
                            if k.ends_with(&suffix) && matches!(v, LocalArrayAlias::Struct { .. }) {
                                Some(v)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    if matches.len() > 1 {
                        return Err(Diagnostic::internal(format!(
                            "ambiguous struct-array alias base '{base}[...]' in ORC lowering"
                        )));
                    }
                    matches.first().copied()
                }
            };
            let (resolved_base, clamped_index) = if let Some(alias) = local_alias {
                match alias {
                    LocalArrayAlias::Primitive { .. } => {
                        return Err(Diagnostic::internal(format!(
                            "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' resolves to a primitive array alias in ORC lowering"
                        )));
                    }
                    LocalArrayAlias::Struct {
                        root_base,
                        elem_struct,
                        len,
                        start_index,
                    } => {
                        if elem_struct != struct_name {
                            return Err(Diagnostic::internal(format!(
                                "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' alias element type is '{elem_struct}' in ORC lowering"
                            )));
                        }
                        if *len == 0 {
                            return Err(Diagnostic::internal(format!(
                                "struct-array alias argument '{base}' has zero length in ORC lowering for function '{callee_name}'"
                            )));
                        }
                        let raw_index =
                            lower_expr(index, ctx, locals, local_aliases, local_array_aliases)?;
                        let index_i32 = cast_orc_value_to(
                            ctx,
                            raw_index,
                            PrimitiveType::I32,
                            b"struct_idx_alias_i32\0",
                        );
                        let clamped_local_idx =
                            clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, *len)?;
                        let global_index = LLVMBuildAdd(
                            ctx.builder,
                            *start_index,
                            clamped_local_idx,
                            b"struct_idx_alias_global\0".as_ptr().cast(),
                        );
                        (root_base.clone(), global_index)
                    }
                }
            } else {
                let self_base = format!("self.{base}");
                let suffix = format!(".{base}");
                let suffix_match = if !ctx.array_struct_roots.contains_key(base)
                    && !ctx.array_struct_roots.contains_key(&self_base)
                {
                    let matches = ctx
                        .array_struct_roots
                        .keys()
                        .filter(|k| k.ends_with(&suffix))
                        .collect::<Vec<_>>();
                    if matches.len() > 1 {
                        return Err(Diagnostic::internal(format!(
                            "ambiguous struct-array root base '{base}[...]' in ORC lowering"
                        )));
                    }
                    matches.into_iter().next().map(|k| k.as_str())
                } else {
                    None
                };
                let resolved_base = if ctx.array_struct_roots.contains_key(base) {
                    base.as_str()
                } else if ctx.array_struct_roots.contains_key(&self_base) {
                    self_base.as_str()
                } else if let Some(stripped) = base.strip_prefix("self.") {
                    if ctx.array_struct_roots.contains_key(stripped) {
                        stripped
                    } else {
                        suffix_match.unwrap_or(base.as_str())
                    }
                } else {
                    suffix_match.unwrap_or(base.as_str())
                };
                let resolved_base = resolved_base.to_owned();
                let Some(indexed_struct) =
                    ctx.array_struct_roots.get(resolved_base.as_str()).cloned()
                else {
                    return Err(Diagnostic::internal(format!(
                        "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' is not a struct-array root in ORC lowering"
                    )));
                };
                if indexed_struct != struct_name {
                    return Err(Diagnostic::internal(format!(
                        "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' has element type '{indexed_struct}' in ORC lowering"
                    )));
                }
                let len = ctx
                    .array_struct_len
                    .get(resolved_base.as_str())
                    .copied()
                    .ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing struct-array length metadata for '{resolved_base}[...]' in ORC lowering for function '{callee_name}'"
                        ))
                    })?;
                if len == 0 {
                    return Err(Diagnostic::internal(format!(
                        "struct-array argument '{resolved_base}' has zero length in ORC lowering for function '{callee_name}'"
                    )));
                }
                let clamped_index = lower_clamped_data_index(
                    ctx,
                    index,
                    len,
                    locals,
                    local_aliases,
                    local_array_aliases,
                )?;
                (resolved_base, clamped_index)
            };

            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
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
