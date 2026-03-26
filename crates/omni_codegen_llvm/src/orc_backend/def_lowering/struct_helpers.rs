use super::*;

fn dot_slot_symbol_to_flat(symbol: &str) -> String {
    if let Some(idx_sep) = symbol.find("].") {
        let head = &symbol[..=idx_sep];
        let tail = &symbol[idx_sep + 2..];
        return format!("{head}__{}", tail.replace('.', "__"));
    }
    symbol.replace('.', "__")
}

fn def_symbol_lookup_candidates(symbol: &str) -> Vec<String> {
    let flat = dot_slot_symbol_to_flat(symbol);
    vec![
        symbol.to_owned(),
        format!("self.{symbol}"),
        flat.clone(),
        format!("self.{flat}"),
    ]
}

fn find_def_local_slot(ctx: &DefLoweringCtx<'_>, symbol: &str) -> Option<DefLocalSlot> {
    for candidate in def_symbol_lookup_candidates(symbol) {
        if let Some(slot) = ctx.local_slots.get(&candidate) {
            return Some(*slot);
        }
    }
    None
}

fn find_def_array_ptr(ctx: &DefLoweringCtx<'_>, symbol: &str) -> Option<LLVMValueRef> {
    for candidate in def_symbol_lookup_candidates(symbol) {
        if let Some(ptr) = ctx.array_ptrs.get(&candidate) {
            return Some(*ptr);
        }
    }
    None
}

fn parse_flat_slot_index_for_base(symbol: &str, base: &str) -> Option<usize> {
    let base_prefix = format!("{base}[");
    let self_prefix = format!("self.{base}[");
    let rest = if let Some(rest) = symbol.strip_prefix(base_prefix.as_str()) {
        rest
    } else {
        symbol.strip_prefix(self_prefix.as_str())?
    };
    let close_idx = rest.find(']')?;
    let idx = rest[..close_idx].parse::<usize>().ok()?;
    let after = &rest[close_idx + 1..];
    if !after.starts_with('.') && !after.starts_with("__") {
        return None;
    }
    Some(idx)
}

fn parse_flat_slot_field_for_base(symbol: &str, base: &str) -> Option<(usize, String)> {
    let base_prefix = format!("{base}[");
    let self_prefix = format!("self.{base}[");
    let rest = if let Some(rest) = symbol.strip_prefix(base_prefix.as_str()) {
        rest
    } else {
        symbol.strip_prefix(self_prefix.as_str())?
    };
    let close_idx = rest.find(']')?;
    let idx = rest[..close_idx].parse::<usize>().ok()?;
    let after = &rest[close_idx + 1..];
    if let Some(field) = after.strip_prefix('.') {
        return Some((idx, field.to_owned()));
    }
    if let Some(field) = after.strip_prefix("__") {
        return Some((idx, field.replace("__", ".")));
    }
    None
}

fn collect_def_flat_slot_indices(ctx: &DefLoweringCtx<'_>, base: &str) -> Vec<usize> {
    let mut out = HashSet::<usize>::new();
    for symbol in ctx.local_slots.keys() {
        if let Some(idx) = parse_flat_slot_index_for_base(symbol, base) {
            out.insert(idx);
        }
    }
    let mut ordered = out.into_iter().collect::<Vec<_>>();
    ordered.sort_unstable();
    ordered
}

pub(super) unsafe fn try_bind_struct_data_slot_aliases_in_def(
    alias_name: &str,
    base: &str,
    index: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    let mut resolved_base = base.to_owned();
    let mut slot_indices = collect_def_flat_slot_indices(ctx, resolved_base.as_str());
    if slot_indices.is_empty() {
        if let Some(stripped) = base.strip_prefix("self.") {
            let stripped_indices = collect_def_flat_slot_indices(ctx, stripped);
            if !stripped_indices.is_empty() {
                resolved_base = stripped.to_owned();
                slot_indices = stripped_indices;
            }
        } else {
            let self_base = format!("self.{base}");
            let self_indices = collect_def_flat_slot_indices(ctx, self_base.as_str());
            if !self_indices.is_empty() {
                resolved_base = self_base;
                slot_indices = self_indices;
            }
        }
    }
    if slot_indices.is_empty() {
        return Ok(false);
    }

    for (expected, actual) in slot_indices.iter().enumerate() {
        if *actual != expected {
            return Err(Diagnostic::internal(format!(
                "local alias binding '{alias_name} = {base}[...]' requires contiguous flattened slot indices in def lowering, got {:?}",
                slot_indices
            )));
        }
    }

    let mut field_names = HashSet::<String>::new();
    for symbol in ctx.local_slots.keys() {
        if let Some((slot_idx, field_name)) =
            parse_flat_slot_field_for_base(symbol, resolved_base.as_str())
        {
            if slot_idx == 0 {
                field_names.insert(field_name);
            }
        }
    }
    if field_names.is_empty() {
        return Ok(false);
    }

    let raw_index = lower_def_expr(index, ctx)?;
    let index_i32 = cast_def_value_to(
        ctx,
        raw_index,
        PrimitiveType::I32,
        b"def_data_alias_slot_idx_i32\0",
    );
    let clamped_index = clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, slot_indices.len())?;

    let mut ordered_fields = field_names.into_iter().collect::<Vec<_>>();
    ordered_fields.sort();
    for field_name in ordered_fields {
        let mut slot_ptrs = Vec::<LLVMValueRef>::with_capacity(slot_indices.len());
        let mut field_ty = None::<PrimitiveType>;
        for slot_idx in &slot_indices {
            let slot_symbol = format!("{}[{}].{}", resolved_base, slot_idx, field_name);
            let slot = find_def_local_slot(ctx, &slot_symbol).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "missing flattened slot state '{}' while creating alias '{}' in def lowering",
                    slot_symbol, alias_name
                ))
            })?;
            if let Some(expected_ty) = field_ty {
                if slot.ty != expected_ty {
                    return Err(Diagnostic::internal(format!(
                        "flattened slot field '{}' has inconsistent types while creating alias '{}' in def lowering",
                        field_name, alias_name
                    )));
                }
            } else {
                field_ty = Some(slot.ty);
            }
            slot_ptrs.push(slot.ptr);
        }
        let selected_ptr = select_def_value_by_slot_index(
            ctx,
            clamped_index,
            &slot_ptrs,
            b"def_alias_slot_ptr_sel\0",
            "def struct-alias slot dispatch",
        )?;
        ctx.local_slots.insert(
            format!("{alias_name}.{}", field_name),
            DefLocalSlot {
                ptr: selected_ptr,
                ty: field_ty.ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing flattened slot type for field '{}' while creating alias '{}' in def lowering",
                        field_name, alias_name
                    ))
                })?,
            },
        );
    }

    Ok(true)
}

pub(super) unsafe fn lower_struct_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
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
                "def call lowering",
                "def_struct",
                |flat| {
                    ctx.array_len.get(flat).copied().ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array[Struct] length metadata for '{flat}' while calling '{callee_name}'"
                        ))
                    })
                },
                |flat| {
                    let local = find_def_local_slot(ctx, flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def local slot for struct field '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    Ok((local.ptr, local.ty))
                },
                |symbol| {
                    find_def_array_ptr(ctx, symbol).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array pointer for struct field '{symbol}' while calling '{callee_name}'"
                        ))
                    })
                },
            )?;
        }
        Expr::Index { base, index, .. } => {
            let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
            let resolved = resolve_indexed_struct_arg_common(
                ctx.builder,
                &ctx.local_array_aliases,
                &ctx.array_struct_roots,
                &ctx.array_len,
                base,
                struct_name,
                callee_name,
                "def lowering",
                b"def_struct_idx_alias_global\0",
                |len| unsafe {
                    let raw_index = lower_def_expr(index, &mut *ctx_ptr)?;
                    let index_i32 = cast_def_value_to(
                        &*ctx_ptr,
                        raw_index,
                        PrimitiveType::I32,
                        b"def_struct_idx_i32\0",
                    );
                    clamp_data_index((&*ctx_ptr).builder, (&*ctx_ptr).i32_ty, index_i32, len)
                },
            )?;
            let (resolved_base, clamped_index) = if let Some(resolved) = resolved {
                resolved
            } else {
                let slot_indices = collect_def_flat_slot_indices(ctx, base);
                if slot_indices.is_empty() {
                    return Err(Diagnostic::internal(format!(
                        "function '{callee_name}' expects struct '{struct_name}' argument, but '{base}[...]' is not a struct-array root in def lowering"
                    )));
                }
                for (expected, actual) in slot_indices.iter().enumerate() {
                    if *actual != expected {
                        return Err(Diagnostic::internal(format!(
                            "function '{callee_name}' expects contiguous flattened slot indices for '{base}[...]' in def lowering, got {:?}",
                            slot_indices
                        )));
                    }
                }
                let raw_index = lower_def_expr(index, ctx)?;
                let index_i32 = cast_def_value_to(
                    ctx,
                    raw_index,
                    PrimitiveType::I32,
                    b"def_struct_idx_slot_i32\0",
                );
                let clamped_index =
                    clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, slot_indices.len())?;
                let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "unknown struct '{struct_name}' in def call lowering for function '{callee_name}'"
                    ))
                })?;
                for field in fields {
                    match field.ty {
                        TypedFieldType::Scalar(prim) => {
                            let mut slot_ptrs =
                                Vec::<LLVMValueRef>::with_capacity(slot_indices.len());
                            for slot_idx in &slot_indices {
                                let slot_name = format!("{base}[{slot_idx}].{}", field.name);
                                let slot = find_def_local_slot(ctx, &slot_name).ok_or_else(|| {
                                    Diagnostic::internal(format!(
                                        "missing flattened slot state '{}' while lowering struct argument for '{}'",
                                        slot_name, callee_name
                                    ))
                                })?;
                                slot_ptrs.push(slot.ptr);
                            }
                            let ptr = select_def_value_by_slot_index(
                                ctx,
                                clamped_index,
                                &slot_ptrs,
                                b"def_struct_idx_slot_ptr_sel\0",
                                "def struct scalar slot dispatch",
                            )?;
                            if by_ref {
                                out_args.push(ptr);
                            } else {
                                let loaded = LLVMBuildLoad2(
                                    ctx.builder,
                                    llvm_ty_for_primitive(ctx.context, prim),
                                    ptr,
                                    b"def_struct_idx_slot_load\0".as_ptr().cast(),
                                );
                                out_args.push(loaded);
                            }
                        }
                        TypedFieldType::Struct => {}
                        TypedFieldType::Tuple(ref elem_tys) => {
                            for (idx, prim) in elem_tys.iter().enumerate() {
                                let mut slot_ptrs =
                                    Vec::<LLVMValueRef>::with_capacity(slot_indices.len());
                                for slot_idx in &slot_indices {
                                    let slot_name =
                                        format!("{base}[{slot_idx}].{}.__{idx}", field.name);
                                    let slot =
                                        find_def_local_slot(ctx, &slot_name).ok_or_else(|| {
                                            Diagnostic::internal(format!(
                                                "missing flattened slot state '{}' while lowering struct tuple argument for '{}'",
                                                slot_name, callee_name
                                            ))
                                        })?;
                                    slot_ptrs.push(slot.ptr);
                                }
                                let ptr = select_def_value_by_slot_index(
                                    ctx,
                                    clamped_index,
                                    &slot_ptrs,
                                    b"def_struct_idx_tuple_ptr_sel\0",
                                    "def struct tuple slot dispatch",
                                )?;
                                if by_ref {
                                    out_args.push(ptr);
                                } else {
                                    let loaded = LLVMBuildLoad2(
                                        ctx.builder,
                                        llvm_ty_for_primitive(ctx.context, *prim),
                                        ptr,
                                        b"def_struct_idx_tuple_load\0".as_ptr().cast(),
                                    );
                                    out_args.push(loaded);
                                }
                            }
                        }
                        TypedFieldType::Array(_) => {
                            return Err(Diagnostic::internal(format!(
                                "function '{callee_name}' indexed struct argument '{base}[...]' requires struct-array root metadata for array field '{}'",
                                field.name
                            )));
                        }
                    }
                }
                return Ok(());
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
                "def call lowering",
                "def_struct",
                |flat, elem_ty, index, gep_name| unsafe {
                    load_def_array_ptr_at_index(&mut *ctx_ptr, flat, elem_ty, index, gep_name)
                },
            )?;
        }
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable or indexed struct-array element in def lowering"
            )));
        }
    }
    Ok(())
}

pub(super) unsafe fn lower_struct_array_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var { name: base, .. } = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument as a variable in def lowering"
        )));
    };
    let actual_struct = ctx.array_struct_roots.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument, but '{base}' is not a struct-array root in def lowering"
        ))
    })?;
    if actual_struct != struct_name {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' expects struct-array '{struct_name}[]' argument, but '{base}' has element type '{actual_struct}' in def lowering"
        )));
    }
    let len_val = ctx.array_len_values.get(base).copied().unwrap_or_else(|| {
        LLVMConstInt(
            ctx.i32_ty,
            ctx.array_len.get(base).copied().unwrap_or(1) as u64,
            0,
        )
    });
    out_args.push(len_val);
    for (leaf_name, _leaf_ty) in
        collect_struct_array_leaf_symbols(ctx.struct_fields, struct_name, base, 1)?
    {
        out_args.push(find_def_array_ptr(ctx, &leaf_name).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing def array pointer '{leaf_name}' while lowering struct-array argument for '{callee_name}'"
            ))
        })?);
    }
    Ok(())
}

unsafe fn load_def_array_ptr_at_index(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    elem_ty: PrimitiveType,
    index: LLVMValueRef,
    gep_name: &[u8],
) -> Result<LLVMValueRef, Diagnostic> {
    let array_base_ptr = *ctx.array_ptrs.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing def array pointer for symbol '{base}' while lowering indexed struct argument"
        ))
    })?;
    Ok(build_f32_ptr_offset(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, elem_ty),
        array_base_ptr,
        index,
        gep_name,
    ))
}

pub(super) unsafe fn bind_struct_data_element_aliases_in_def(
    alias_name: &str,
    struct_name: &str,
    root_base: &str,
    global_index: LLVMValueRef,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<(), Diagnostic> {
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while creating def array alias '{}'",
            struct_name, alias_name
        ))
    })?;
    for field in fields {
        let array_field_base = format!("{root_base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                let array_base_ptr = *ctx.array_ptrs.get(&array_field_base).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing def array pointer for symbol '{array_field_base}' while creating alias '{alias_name}'"
                    ))
                })?;
                let elem_ptr = build_f32_ptr_offset(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, prim),
                    array_base_ptr,
                    global_index,
                    b"def_struct_data_elem_ptr\0",
                );
                ctx.local_slots.insert(
                    format!("{alias_name}.{}", field.name),
                    DefLocalSlot {
                        ptr: elem_ptr,
                        ty: prim,
                    },
                );
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Tuple(ref elem_tys) => {
                for (idx, prim) in elem_tys.iter().enumerate() {
                    let elem_flat = format!("{array_field_base}.__{idx}");
                    let array_base_ptr = *ctx.array_ptrs.get(&elem_flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array pointer for symbol '{elem_flat}' while creating alias '{alias_name}'"
                        ))
                    })?;
                    let elem_ptr = build_f32_ptr_offset(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, *prim),
                        array_base_ptr,
                        global_index,
                        b"def_struct_tuple_elem_ptr\0",
                    );
                    ctx.local_slots.insert(
                        format!("{alias_name}.{}.__{idx}", field.name),
                        DefLocalSlot {
                            ptr: elem_ptr,
                            ty: *prim,
                        },
                    );
                }
            }
            TypedFieldType::Array(field_len) => {
                let start_idx = build_data_segment_start_index(
                    ctx.builder,
                    ctx.i32_ty,
                    global_index,
                    field_len,
                )?;
                if let Some(elem_struct) = &field.array_elem_struct {
                    ctx.local_array_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalArrayAlias::Struct {
                            root_base: array_field_base.clone(),
                            elem_struct: elem_struct.clone(),
                            len: field_len,
                            start_index: start_idx,
                        },
                    );
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    let array_base_ptr = *ctx.array_ptrs.get(&array_field_base).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array pointer for symbol '{array_field_base}' while creating alias '{alias_name}'"
                        ))
                    })?;
                    let seg_ptr = build_f32_ptr_offset(
                        ctx.builder,
                        ctx.float_ty,
                        array_base_ptr,
                        start_idx,
                        b"def_struct_data_seg_ptr\0",
                    );
                    ctx.local_array_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalArrayAlias::Primitive {
                            base_ptr: seg_ptr,
                            len: field_len,
                            elem_ty,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}
