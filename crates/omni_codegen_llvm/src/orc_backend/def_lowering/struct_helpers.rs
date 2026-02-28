use super::*;

pub(super) unsafe fn lower_struct_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
) -> Result<(), Diagnostic> {
    let base = match arg_expr {
        Expr::Var(name) => name,
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable reference in def lowering"
            )));
        }
    };
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in def call lowering for function '{callee_name}'"
        ))
    })?;
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(_) => {
                let local = ctx.local_slots.get(&flat).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing def local slot for struct field '{flat}' while calling '{callee_name}'"
                    ))
                })?;
                if by_ref {
                    out_args.push(local.ptr);
                } else {
                    let loaded = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, local.ty),
                        local.ptr,
                        b"def_struct_arg_load\0".as_ptr().cast(),
                    );
                    out_args.push(loaded);
                }
            }
            TypedFieldType::Array(_) => {
                if let Some(elem_struct) = &field.array_elem_struct {
                    let root_len = *ctx.array_len.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array[Struct] length metadata for '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    let mut roots = Vec::new();
                    let mut leaves = Vec::new();
                    collect_array_struct_bindings(
                        ctx.struct_fields,
                        elem_struct,
                        &flat,
                        root_len,
                        &mut roots,
                        &mut leaves,
                        &mut Vec::new(),
                    )?;
                    for (leaf_name, _, _) in leaves {
                        let ptr = *ctx.array_ptrs.get(&leaf_name).ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "missing def array pointer for struct field '{leaf_name}' while calling '{callee_name}'"
                            ))
                        })?;
                        out_args.push(ptr);
                    }
                } else {
                    let ptr = *ctx.array_ptrs.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing def array pointer for struct field '{flat}' while calling '{callee_name}'"
                        ))
                    })?;
                    out_args.push(ptr);
                }
            }
        }
    }
    Ok(())
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
