use super::*;

pub(super) unsafe fn lower_data_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<ArrayElementPtr, Diagnostic> {
    lower_data_element_ptr_with_bounds_mode(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_array_aliases,
        true,
    )
}

pub(super) unsafe fn lower_data_element_ptr_unchecked(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<ArrayElementPtr, Diagnostic> {
    lower_data_element_ptr_with_bounds_mode(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_array_aliases,
        false,
    )
}

pub(super) unsafe fn lower_array_index_i32(
    ctx: &mut LoweringCtx<'_>,
    index_expr: &Expr,
    len: usize,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<LLVMValueRef, Diagnostic> {
    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"arr_idx_i32\0");
    if clamp_index {
        clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, len)
    } else {
        Ok(raw_index_i)
    }
}

pub(super) unsafe fn lower_input_array_index_read(
    ctx: &mut LoweringCtx<'_>,
    info: TypedArrayInfo,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    if info.offset > i32::MAX as usize {
        return Err(Diagnostic::internal(
            "input array offset exceeds supported i32 index range in ORC lowering",
        ));
    }
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_array_aliases,
        clamp_index,
    )?;
    let offset_v = LLVMConstInt(ctx.i32_ty, info.offset as u64, 0);
    let ch = LLVMBuildAdd(ctx.builder, offset_v, idx, b"in_arr_ch\0".as_ptr().cast());
    let in_ptr_ptr = build_ptr_offset(
        ctx.builder,
        ctx.float_ptr_ty,
        ctx.in_ptrs,
        ch,
        b"in_arr_ch_ptr_ptr\0",
    );
    let in_ch_ptr = LLVMBuildLoad2(
        ctx.builder,
        ctx.float_ptr_ty,
        in_ptr_ptr,
        b"in_arr_ch_ptr\0".as_ptr().cast(),
    );
    let in_ch_ptr_typed = LLVMBuildBitCast(
        ctx.builder,
        in_ch_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
        b"in_arr_ch_ptr_typed\0".as_ptr().cast(),
    );
    let ptr = build_f32_ptr_offset(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        in_ch_ptr_typed,
        ctx.frame_idx,
        b"in_arr_ptr\0",
    );
    let raw = LLVMBuildLoad2(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        ptr,
        b"in_arr_load\0".as_ptr().cast(),
    );
    if ctx.oversample_factor > 1 {
        if let Some(input_cache_ptr) = ctx.oversample_input_cache {
            let cached_ptr = build_f32_ptr_offset(
                ctx.builder,
                ctx.float_ty,
                input_cache_ptr,
                ch,
                b"os_cached_in_arr_ptr\0",
            );
            let cached_f32 = LLVMBuildLoad2(
                ctx.builder,
                ctx.float_ty,
                cached_ptr,
                b"os_cached_in_arr\0".as_ptr().cast(),
            );
            let value = cast_orc_value_to(
                ctx,
                OrcValue {
                    value: cached_f32,
                    ty: PrimitiveType::F32,
                },
                info.elem_ty,
                b"os_cached_in_arr_cast\0",
            );
            return Ok(OrcValue {
                value,
                ty: info.elem_ty,
            });
        }
    }
    if ctx.oversample_factor > 1 {
        if let (Some(alpha), Some(prev_inputs_ptr)) =
            (ctx.oversample_alpha, ctx.oversample_prev_inputs)
        {
            let prev_ptr = build_f32_ptr_offset(
                ctx.builder,
                ctx.float_ty,
                prev_inputs_ptr,
                ch,
                b"os_prev_in_arr_ptr\0",
            );
            let prev = LLVMBuildLoad2(
                ctx.builder,
                ctx.float_ty,
                prev_ptr,
                b"os_prev_in_arr\0".as_ptr().cast(),
            );
            let current_f32 = cast_orc_value_to(
                ctx,
                OrcValue {
                    value: raw,
                    ty: info.elem_ty,
                },
                PrimitiveType::F32,
                b"os_in_arr_curr_f32\0",
            );
            let diff = build_fsub_fast(
                ctx.builder,
                current_f32,
                prev,
                b"os_in_arr_diff\0",
                ctx.fast_math_flags,
            );
            let scaled = build_fmul_fast(
                ctx.builder,
                diff,
                alpha,
                b"os_in_arr_scaled\0",
                ctx.fast_math_flags,
            );
            let interp = build_fadd_fast(
                ctx.builder,
                prev,
                scaled,
                b"os_in_arr_interp\0",
                ctx.fast_math_flags,
            );
            let value = cast_orc_value_to(
                ctx,
                OrcValue {
                    value: interp,
                    ty: PrimitiveType::F32,
                },
                info.elem_ty,
                b"os_in_arr_interp_cast\0",
            );
            return Ok(OrcValue {
                value,
                ty: info.elem_ty,
            });
        }
    }
    Ok(OrcValue {
        value: raw,
        ty: info.elem_ty,
    })
}

pub(super) unsafe fn lower_param_array_index_read(
    ctx: &mut LoweringCtx<'_>,
    base_name: &str,
    info: TypedArrayInfo,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    let elem_bytes = primitive_type_bytes(info.elem_ty);
    let base_byte_offset = ctx
        .param_byte_offset
        .get(base_name)
        .copied()
        .ok_or_else(|| Diagnostic::internal(format!("unknown parameter array '{base_name}'")))?;
    if base_byte_offset > i32::MAX as usize {
        return Err(Diagnostic::internal(
            "parameter array offset exceeds supported i32 index range in ORC lowering",
        ));
    }
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_array_aliases,
        clamp_index,
    )?;
    let offset_v = LLVMConstInt(ctx.i32_ty, base_byte_offset as u64, 0);
    let elem_bytes_v = LLVMConstInt(ctx.i32_ty, elem_bytes as u64, 0);
    let scaled = LLVMBuildMul(
        ctx.builder,
        idx,
        elem_bytes_v,
        b"param_arr_scaled\0".as_ptr().cast(),
    );
    let byte_off = LLVMBuildAdd(
        ctx.builder,
        offset_v,
        scaled,
        b"param_arr_byte_off\0".as_ptr().cast(),
    );
    let ptr = build_typed_ptr_from_byte_offset(
        ctx.builder,
        ctx.context,
        ctx.params_ptr,
        byte_off,
        info.elem_ty,
        b"param_arr_ptr_i8\0",
        b"param_arr_ptr_typed\0",
    );
    let raw = LLVMBuildLoad2(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, info.elem_ty),
        ptr,
        b"param_arr_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value: raw,
        ty: info.elem_ty,
    })
}

pub(super) unsafe fn lower_output_array_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<ArrayElementPtr, Diagnostic> {
    let info = *ctx
        .output_arrays
        .get(base)
        .ok_or_else(|| Diagnostic::internal(format!("unknown output array '{base}'")))?;
    let base_ptr = *ctx.out_array_base_ptrs.get(base).ok_or_else(|| {
        Diagnostic::internal(format!("missing output array storage for '{base}'"))
    })?;
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        info.len,
        locals,
        local_aliases,
        local_array_aliases,
        clamp_index,
    )?;
    Ok(ArrayElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, info.elem_ty),
            base_ptr,
            idx,
            b"out_arr_elem_ptr\0",
        ),
        elem_ty: info.elem_ty,
    })
}

pub(super) unsafe fn load_orc_buffer_total_len_i32(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let frames_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_frames_ptr,
        idx,
        b"buf_frames_ptr\0",
    );
    let channels_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_channels_ptr,
        idx,
        b"buf_channels_ptr\0",
    );
    let frames = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        frames_ptr,
        b"buf_frames\0".as_ptr().cast(),
    );
    if ctx.buffer_mono.contains(base) {
        return Ok(frames);
    }
    let channels = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        channels_ptr,
        b"buf_channels\0".as_ptr().cast(),
    );
    Ok(LLVMBuildMul(
        ctx.builder,
        frames,
        channels,
        b"buf_total_len\0".as_ptr().cast(),
    ))
}

pub(super) unsafe fn lower_buffer_element_ptr(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<ArrayElementPtr, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing element type metadata for buffer '{base}' in ORC lowering"
        ))
    })?;

    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"buf_idx_i32\0");
    let total_len = load_orc_buffer_total_len_i32(ctx, base)?;
    let final_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_index_i, total_len)
    } else {
        raw_index_i
    };

    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"buf_ptr_ptr\0",
    );
    let raw_ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"buf_ptr\0".as_ptr().cast(),
    );
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        raw_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, elem_ty), 0),
        b"buf_ptr_typed\0".as_ptr().cast(),
    );
    Ok(ArrayElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            typed_ptr,
            final_index,
            b"buf_elem_ptr\0",
        ),
        elem_ty,
    })
}

pub(super) unsafe fn load_orc_buffer_channels_i32(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let channels_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_channels_ptr,
        idx,
        b"buf_channels_ptr\0",
    );
    Ok(LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        channels_ptr,
        b"buf_channels\0".as_ptr().cast(),
    ))
}

pub(super) unsafe fn lower_buffer_element_ptr_2d(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    channel_expr: &Expr,
    sample_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<ArrayElementPtr, Diagnostic> {
    let channel = lower_expr(
        channel_expr,
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
    )?;
    let sample = lower_expr(sample_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let channel_i = cast_orc_value_to(ctx, channel, PrimitiveType::I32, b"buf_ch_i32\0");
    let sample_i = cast_orc_value_to(ctx, sample, PrimitiveType::I32, b"buf_sample_i32\0");
    let channels = load_orc_buffer_channels_i32(ctx, base)?;
    let total_len = load_orc_buffer_total_len_i32(ctx, base)?;
    let sample_off = LLVMBuildMul(
        ctx.builder,
        sample_i,
        channels,
        b"buf_sample_off\0".as_ptr().cast(),
    );
    let raw_flat = LLVMBuildAdd(
        ctx.builder,
        sample_off,
        channel_i,
        b"buf_flat_idx\0".as_ptr().cast(),
    );
    let flat_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_flat, total_len)
    } else {
        raw_flat
    };
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing element type metadata for buffer '{base}' in ORC lowering"
        ))
    })?;
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"buf_ptr_ptr2\0",
    );
    let raw_ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"buf_ptr2\0".as_ptr().cast(),
    );
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        raw_ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, elem_ty), 0),
        b"buf_ptr_typed2\0".as_ptr().cast(),
    );
    Ok(ArrayElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            typed_ptr,
            flat_index,
            b"buf_elem_ptr2\0",
        ),
        elem_ty,
    })
}

pub(super) unsafe fn lower_data_element_ptr_with_bounds_mode(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<ArrayElementPtr, Diagnostic> {
    if ctx.array_struct_len.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "array symbol '{base}[...]' has struct elements; index it via an alias assignment first"
        )));
    }

    let (array_base_ptr, array_len, array_elem_ty) = if let Some(alias) =
        local_array_aliases.get(base)
    {
        match alias {
            LocalArrayAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => (*base_ptr, *len, *elem_ty),
            LocalArrayAlias::Struct { .. } => {
                return Err(Diagnostic::internal(format!(
                    "array alias '{base}[...]' has struct elements; index it via an alias assignment first"
                )));
            }
        }
    } else {
        let array_len = *ctx.array_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing array length for symbol '{base}' in ORC lowering"
            ))
        })?;
        let array_elem_ty = *ctx.array_elem_ty.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing array element type for symbol '{base}' in ORC lowering"
            ))
        })?;
        let array_base_ptr = load_data_base_ptr_for_symbol(ctx, base)?;
        (array_base_ptr, array_len, array_elem_ty)
    };

    if array_len == 0 {
        return Err(Diagnostic::internal(format!(
            "array symbol '{base}' has zero length in ORC lowering"
        )));
    }

    let final_index = if clamp_index {
        if let Some(const_idx) = try_constant_index_i64(index_expr) {
            LLVMConstInt(
                ctx.i32_ty,
                checked_constant_data_index_u64(
                    array_len,
                    const_idx,
                    &format!("array index in ORC lowering for '{base}'"),
                )?,
                0,
            )
        } else {
            let raw_index =
                lower_expr(index_expr, ctx, locals, local_aliases, local_array_aliases)?;
            let raw_index_i =
                cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0");
            clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, array_len)?
        }
    } else {
        let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_array_aliases)?;
        cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0")
    };
    Ok(ArrayElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, array_elem_ty),
            array_base_ptr,
            final_index,
            b"array_elem_ptr\0",
        ),
        elem_ty: array_elem_ty,
    })
}

pub(super) unsafe fn load_data_base_ptr_for_symbol(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    ctx.array_base_ptrs.get(base).copied().ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown array symbol '{base}' in ORC indexed lowering"
        ))
    })
}

pub(super) unsafe fn lower_clamped_data_index(
    ctx: &mut LoweringCtx<'_>,
    index_expr: &Expr,
    len: usize,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<LLVMValueRef, Diagnostic> {
    let raw_index = lower_expr(index_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let raw_index_i = cast_orc_value_to(ctx, raw_index, PrimitiveType::I32, b"data_idx_i32\0");
    clamp_data_index(ctx.builder, ctx.i32_ty, raw_index_i, len)
}

pub(super) unsafe fn build_data_segment_start_index(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    parent_index: LLVMValueRef,
    field_len: usize,
) -> Result<LLVMValueRef, Diagnostic> {
    if field_len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "nested array field length {field_len} exceeds supported i32 index range"
        )));
    }
    let stride = LLVMConstInt(i32_ty, field_len as u64, 0);
    Ok(LLVMBuildMul(
        builder,
        parent_index,
        stride,
        b"array_seg_start_idx\0".as_ptr().cast(),
    ))
}

pub(super) unsafe fn load_data_ptr_at_index(
    ctx: &mut LoweringCtx<'_>,
    symbol: &str,
    elem_ty: PrimitiveType,
    index: LLVMValueRef,
    gep_name: &[u8],
) -> Result<LLVMValueRef, Diagnostic> {
    let array_base_ptr = load_data_base_ptr_for_symbol(ctx, symbol)?;
    Ok(build_f32_ptr_offset(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, elem_ty),
        array_base_ptr,
        index,
        gep_name,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn bind_struct_data_element_aliases(
    alias_name: &str,
    struct_name: &str,
    root_base: &str,
    global_index: LLVMValueRef,
    ctx: &mut LoweringCtx<'_>,
    local_aliases: &mut HashMap<String, AliasSlot>,
    local_array_aliases: &mut HashMap<String, LocalArrayAlias>,
) -> Result<(), Diagnostic> {
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while creating array alias '{}'",
            struct_name, alias_name
        ))
    })?;
    for field in fields {
        let array_field_base = format!("{root_base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => {
                let array_ptr = load_data_ptr_at_index(
                    ctx,
                    &array_field_base,
                    prim,
                    global_index,
                    b"struct_data_elem_ptr\0",
                )?;
                local_aliases.insert(
                    format!("{alias_name}.{}", field.name),
                    AliasSlot {
                        ptr: array_ptr,
                        ty: prim,
                    },
                );
            }
            TypedFieldType::Struct => {}
            TypedFieldType::Array(field_len) => {
                let start_idx = build_data_segment_start_index(
                    ctx.builder,
                    ctx.i32_ty,
                    global_index,
                    field_len,
                )?;
                if let Some(elem_struct) = &field.array_elem_struct {
                    if !ctx.array_struct_len.contains_key(&array_field_base) {
                        return Err(Diagnostic::internal(format!(
                            "missing nested array[Struct] metadata for symbol '{array_field_base}'"
                        )));
                    }
                    local_array_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalArrayAlias::Struct {
                            root_base: array_field_base,
                            elem_struct: elem_struct.clone(),
                            len: field_len,
                            start_index: start_idx,
                        },
                    );
                } else {
                    let elem_ty = field.array_elem_ty.unwrap_or(PrimitiveType::F32);
                    let seg_base_ptr = load_data_ptr_at_index(
                        ctx,
                        &array_field_base,
                        elem_ty,
                        start_idx,
                        b"struct_data_seg_ptr\0",
                    )?;
                    local_array_aliases.insert(
                        format!("{alias_name}.{}", field.name),
                        LocalArrayAlias::Primitive {
                            base_ptr: seg_base_ptr,
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

pub(super) unsafe fn clamp_data_index(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    index: LLVMValueRef,
    len: usize,
) -> Result<LLVMValueRef, Diagnostic> {
    let max_index = len.saturating_sub(1);
    if max_index > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "array length {len} exceeds supported index range (i32)"
        )));
    }
    let zero = LLVMConstInt(i32_ty, 0, 0);
    let max_v = LLVMConstInt(i32_ty, max_index as u64, 0);

    let below_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSLT,
        index,
        zero,
        b"idx_below_zero\0".as_ptr().cast(),
    );
    let low_clamped = LLVMBuildSelect(
        builder,
        below_zero,
        zero,
        index,
        b"idx_low_clamped\0".as_ptr().cast(),
    );
    let above_max = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        low_clamped,
        max_v,
        b"idx_above_max\0".as_ptr().cast(),
    );
    Ok(LLVMBuildSelect(
        builder,
        above_max,
        max_v,
        low_clamped,
        b"idx_clamped\0".as_ptr().cast(),
    ))
}

pub(super) unsafe fn clamp_data_index_dynamic(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    index: LLVMValueRef,
    len: LLVMValueRef,
) -> LLVMValueRef {
    let zero = LLVMConstInt(i32_ty, 0, 0);
    let one = LLVMConstInt(i32_ty, 1, 0);
    let below_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSLT,
        index,
        zero,
        b"idx_dyn_below_zero\0".as_ptr().cast(),
    );
    let low_clamped = LLVMBuildSelect(
        builder,
        below_zero,
        zero,
        index,
        b"idx_dyn_low_clamped\0".as_ptr().cast(),
    );
    let len_gt_zero = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        len,
        zero,
        b"idx_dyn_len_gt_zero\0".as_ptr().cast(),
    );
    let len_safe = LLVMBuildSelect(
        builder,
        len_gt_zero,
        len,
        one,
        b"idx_dyn_len_safe\0".as_ptr().cast(),
    );
    let max_v = LLVMBuildSub(builder, len_safe, one, b"idx_dyn_max\0".as_ptr().cast());
    let above_max = LLVMBuildICmp(
        builder,
        LLVMIntPredicate::LLVMIntSGT,
        low_clamped,
        max_v,
        b"idx_dyn_above_max\0".as_ptr().cast(),
    );
    LLVMBuildSelect(
        builder,
        above_max,
        max_v,
        low_clamped,
        b"idx_dyn_clamped\0".as_ptr().cast(),
    )
}

pub(super) fn try_constant_index_i64(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Int(v) => Some(*v),
        Expr::Number(v) => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn checked_constant_data_index_u64(
    len: usize,
    idx: i64,
    context: &str,
) -> Result<u64, Diagnostic> {
    let max_index = len.saturating_sub(1);
    if max_index > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "array length {len} exceeds supported index range (i32)"
        )));
    }
    if idx < 0 || idx > max_index as i64 {
        return Err(Diagnostic::internal(format!(
            "{context}: constant index {idx} is out of range (expected 0..{max_index})"
        )));
    }
    Ok(idx as u64)
}

pub(super) unsafe fn const_i32(i32_ty: LLVMTypeRef, v: i32) -> LLVMValueRef {
    LLVMConstInt(i32_ty, v as i64 as u64, 1)
}
