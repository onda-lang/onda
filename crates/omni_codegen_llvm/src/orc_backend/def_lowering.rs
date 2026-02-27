use super::*;

mod expr_lowering;
mod stmt_lowering;
mod struct_helpers;
use struct_helpers::{bind_struct_data_element_aliases_in_def, lower_struct_call_args_in_def};

pub(super) unsafe fn lower_def_stmt(
    stmt: &Stmt,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<bool, Diagnostic> {
    stmt_lowering::lower_def_stmt(stmt, ctx)
}

pub(super) unsafe fn cast_def_value_to(
    ctx: &DefLoweringCtx<'_>,
    value: OrcValue,
    to: PrimitiveType,
    name: &[u8],
) -> LLVMValueRef {
    cast_value_to_common(
        value,
        to,
        ctx.builder,
        ctx.context,
        ctx.i32_ty,
        ctx.float_ty,
        ctx.fast_math_flags,
        name,
    )
}

pub(super) unsafe fn lower_def_condition(
    ctx: &DefLoweringCtx<'_>,
    value: OrcValue,
    name: &[u8],
) -> LLVMValueRef {
    cast_def_value_to(ctx, value, PrimitiveType::Bool, name)
}

pub(super) unsafe fn lower_def_logical_expr(
    op: LogicalOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    let lhs_value = lower_def_expr(lhs, ctx)?;
    let lhs_bool = lower_def_condition(ctx, lhs_value, b"def_logical_lhs\0");
    let builder = ctx.builder;
    let context = ctx.context;
    let fn_ref = ctx.fn_ref;
    lower_logical_short_circuit_common(
        op,
        builder,
        context,
        fn_ref,
        lhs_bool,
        b"def_logical_rhs\0",
        b"def_logical_merge\0",
        b"def_logical_phi\0",
        "def logical expression",
        || {
            let rhs_value = lower_def_expr(rhs, ctx)?;
            Ok(lower_def_condition(
                ctx,
                rhs_value,
                b"def_logical_rhs_bool\0",
            ))
        },
    )
}

pub(super) unsafe fn llvm_ty_for_primitive(
    context: LLVMContextRef,
    ty: PrimitiveType,
) -> LLVMTypeRef {
    match ty {
        PrimitiveType::F32 => LLVMFloatTypeInContext(context),
        PrimitiveType::F64 => LLVMDoubleTypeInContext(context),
        PrimitiveType::I32 => LLVMInt32TypeInContext(context),
        PrimitiveType::I64 => LLVMInt64TypeInContext(context),
        PrimitiveType::Bool => LLVMInt1TypeInContext(context),
    }
}

pub(super) unsafe fn llvm_zero_for_primitive(
    context: LLVMContextRef,
    ty: PrimitiveType,
) -> LLVMValueRef {
    match ty {
        PrimitiveType::F32 => LLVMConstReal(LLVMFloatTypeInContext(context), 0.0),
        PrimitiveType::F64 => LLVMConstReal(LLVMDoubleTypeInContext(context), 0.0),
        PrimitiveType::I32 => LLVMConstInt(LLVMInt32TypeInContext(context), 0, 0),
        PrimitiveType::I64 => LLVMConstInt(LLVMInt64TypeInContext(context), 0, 0),
        PrimitiveType::Bool => LLVMConstInt(LLVMInt1TypeInContext(context), 0, 0),
    }
}

pub(super) unsafe fn merge_numeric_primitive(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (F64, I32)
        | (I32, F64)
        | (F64, I64)
        | (I64, F64)
        | (F64, F32)
        | (F32, F64)
        | (F64, F64) => Some(F64),
        (F32, I32) | (I32, F32) | (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

pub(super) unsafe fn cast_orc_value_to(
    ctx: &LoweringCtx<'_>,
    value: OrcValue,
    to: PrimitiveType,
    name: &[u8],
) -> LLVMValueRef {
    cast_value_to_common(
        value,
        to,
        ctx.builder,
        ctx.context,
        ctx.i32_ty,
        ctx.float_ty,
        ctx.fast_math_flags,
        name,
    )
}

pub(super) unsafe fn lower_orc_condition(
    ctx: &LoweringCtx<'_>,
    value: OrcValue,
    name: &[u8],
) -> LLVMValueRef {
    cast_orc_value_to(ctx, value, PrimitiveType::Bool, name)
}

pub(super) unsafe fn lower_orc_logical_expr(
    op: LogicalOp,
    lhs: &Expr,
    rhs: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    let lhs_value = lower_expr(lhs, ctx, locals, local_aliases, local_data_aliases)?;
    let lhs_bool = lower_orc_condition(ctx, lhs_value, b"logical_lhs\0");
    let builder = ctx.builder;
    let context = ctx.context;
    let fn_ref = ctx.fn_ref;
    lower_logical_short_circuit_common(
        op,
        builder,
        context,
        fn_ref,
        lhs_bool,
        b"logical_rhs\0",
        b"logical_merge\0",
        b"logical_phi\0",
        "ORC logical expression",
        || {
            let rhs_value = lower_expr(rhs, ctx, locals, local_aliases, local_data_aliases)?;
            Ok(lower_orc_condition(ctx, rhs_value, b"logical_rhs_bool\0"))
        },
    )
}

pub(super) unsafe fn lower_def_expr(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    expr_lowering::lower_def_expr(expr, ctx)
}

pub(super) unsafe fn lower_def_data_element_ptr(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    index_expr: &Expr,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        let total_len = load_def_buffer_total_len_i32(ctx, base, &info)?;
        let final_index = if clamp_index {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            let index_i32 =
                cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_buf_idx_i32\0");
            clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, index_i32, total_len)
        } else {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_buf_idx_i32\0")
        };
        let typed_ptr = LLVMBuildBitCast(
            ctx.builder,
            info.ptr,
            LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
            b"def_buf_ptr_typed\0".as_ptr().cast(),
        );
        return Ok(DataElementPtr {
            ptr: build_f32_ptr_offset(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, info.elem_ty),
                typed_ptr,
                final_index,
                b"def_buf_elem_ptr\0",
            ),
            elem_ty: info.elem_ty,
        });
    }

    let (base_ptr, len, elem_ty) = if let Some(alias) = ctx.local_data_aliases.get(base) {
        match alias {
            LocalDataAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => (*base_ptr, *len, *elem_ty),
            LocalDataAlias::Struct { .. } => {
                return Err(Diagnostic::internal(format!(
                    "Data symbol '{base}[...]' has struct elements in def lowering; index it via an alias assignment first"
                )));
            }
        }
    } else {
        if ctx.data_struct_roots.contains_key(base) {
            return Err(Diagnostic::internal(format!(
                "Data symbol '{base}[...]' has struct elements in def lowering; index it via an alias assignment first"
            )));
        }
        let base_ptr = *ctx.data_ptrs.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "unknown Data symbol '{base}' in def indexed expression lowering"
            ))
        })?;
        let len = *ctx.data_len.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data length for '{base}' in def indexed expression lowering"
            ))
        })?;
        let elem_ty = *ctx.data_elem_ty.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing Data element type for '{base}' in def indexed expression lowering"
            ))
        })?;
        (base_ptr, len, elem_ty)
    };

    if len == 0 {
        return Err(Diagnostic::internal(format!(
            "Data symbol '{base}' has zero length in def lowering"
        )));
    }

    let final_index = if clamp_index {
        if let Some(const_idx) = try_constant_index_i64(index_expr) {
            LLVMConstInt(
                ctx.i32_ty,
                checked_constant_data_index_u64(
                    len,
                    const_idx,
                    &format!("Data index in def lowering for '{base}'"),
                )?,
                0,
            )
        } else {
            let raw_index = lower_def_expr(index_expr, ctx)?;
            let index_i32 =
                cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_data_idx_i32\0");
            clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, len)?
        }
    } else {
        let raw_index = lower_def_expr(index_expr, ctx)?;
        cast_def_value_to(ctx, raw_index, PrimitiveType::I32, b"def_data_idx_i32\0")
    };
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, elem_ty),
            base_ptr,
            final_index,
            b"def_data_elem_ptr\0",
        ),
        elem_ty,
    })
}

pub(super) unsafe fn load_def_buffer_channels_i32(
    ctx: &mut DefLoweringCtx<'_>,
    info: &DefBufferParamInfo,
) -> LLVMValueRef {
    match info.declared_channels {
        TypedBufferChannels::Mono => LLVMConstInt(ctx.i32_ty, 1, 0),
        TypedBufferChannels::Static(ch) => LLVMConstInt(ctx.i32_ty, ch as u64, 0),
        TypedBufferChannels::Dynamic => info.channels,
    }
}

pub(super) unsafe fn load_def_buffer_total_len_i32(
    ctx: &mut DefLoweringCtx<'_>,
    _base: &str,
    info: &DefBufferParamInfo,
) -> Result<LLVMValueRef, Diagnostic> {
    let frames = info.frames;
    match info.declared_channels {
        TypedBufferChannels::Mono => Ok(frames),
        TypedBufferChannels::Static(ch) => {
            if ch <= 1 {
                Ok(frames)
            } else {
                let channels = LLVMConstInt(ctx.i32_ty, ch as u64, 0);
                Ok(LLVMBuildMul(
                    ctx.builder,
                    frames,
                    channels,
                    b"def_buf_total_len\0".as_ptr().cast(),
                ))
            }
        }
        TypedBufferChannels::Dynamic => Ok(LLVMBuildMul(
            ctx.builder,
            frames,
            info.channels,
            b"def_buf_total_len\0".as_ptr().cast(),
        )),
    }
}

pub(super) unsafe fn lower_def_buffer_chans_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    let info = ctx.buffer_params.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "builtin method '{method_name}' requires a buffer symbol receiver in def lowering, got '{base}'"
        ))
    })?;
    Ok(OrcValue {
        value: load_def_buffer_channels_i32(ctx, &info),
        ty: PrimitiveType::I32,
    })
}

pub(super) unsafe fn lower_def_buffer_element_ptr_2d(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    channel_expr: &Expr,
    sample_expr: &Expr,
    clamp_index: bool,
) -> Result<DataElementPtr, Diagnostic> {
    let info = ctx.buffer_params.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def lowering two-dimensional index"
        ))
    })?;
    let channel = lower_def_expr(channel_expr, ctx)?;
    let sample = lower_def_expr(sample_expr, ctx)?;
    let channel_i = cast_def_value_to(ctx, channel, PrimitiveType::I32, b"def_buf_ch_i32\0");
    let sample_i = cast_def_value_to(ctx, sample, PrimitiveType::I32, b"def_buf_sample_i32\0");
    let channels = load_def_buffer_channels_i32(ctx, &info);
    let total_len = load_def_buffer_total_len_i32(ctx, base, &info)?;
    let sample_off = LLVMBuildMul(
        ctx.builder,
        sample_i,
        channels,
        b"def_buf_sample_off\0".as_ptr().cast(),
    );
    let raw_flat = LLVMBuildAdd(
        ctx.builder,
        sample_off,
        channel_i,
        b"def_buf_flat_idx\0".as_ptr().cast(),
    );
    let flat_index = if clamp_index {
        clamp_data_index_dynamic(ctx.builder, ctx.i32_ty, raw_flat, total_len)
    } else {
        raw_flat
    };
    let typed_ptr = LLVMBuildBitCast(
        ctx.builder,
        info.ptr,
        LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
        b"def_buf_ptr_typed2\0".as_ptr().cast(),
    );
    Ok(DataElementPtr {
        ptr: build_f32_ptr_offset(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, info.elem_ty),
            typed_ptr,
            flat_index,
            b"def_buf_elem_ptr2\0",
        ),
        elem_ty: info.elem_ty,
    })
}

pub(super) unsafe fn lower_def_buffer_read2_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_read2",
        3,
        "def lowering",
    )?;
    let base = builtin_data_call_base_symbol(args, "__omni_buffer_read2", "def lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let data = lower_def_buffer_element_ptr_2d(ctx, base, ch_expr, sample_expr, clamp_index)?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"def_buf2_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

pub(super) unsafe fn lower_def_buffer_write2_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_write2",
        4,
        "def lowering",
    )?;
    let base = builtin_data_call_base_symbol(args, "__omni_buffer_write2", "def lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let value_expr = &args[3].expr;
    let data = lower_def_buffer_element_ptr_2d(ctx, base, ch_expr, sample_expr, clamp_index)?;
    let value = lower_def_expr(value_expr, ctx)?;
    let casted = cast_def_value_to(ctx, value, data.elem_ty, b"def_buf2_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}
