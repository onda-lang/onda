use super::super::*;
use super::*;

pub(in crate::orc_backend) unsafe fn lower_orc_buffer_read2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_read2",
        3,
        "ORC expression lowering",
    )?;
    let base =
        builtin_data_call_base_symbol(args, "__omni_buffer_read2", "ORC expression lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let data = lower_buffer_element_ptr_2d(
        ctx,
        base,
        ch_expr,
        sample_expr,
        locals,
        local_aliases,
        local_array_aliases,
        clamp_index,
    )?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"buf2_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn lower_orc_buffer_write2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    clamp_index: bool,
) -> Result<OrcValue, Diagnostic> {
    ensure_internal_buffer_2d_call_positional_arity(
        args,
        "__omni_buffer_write2",
        4,
        "ORC expression lowering",
    )?;
    let base =
        builtin_data_call_base_symbol(args, "__omni_buffer_write2", "ORC expression lowering")?;
    let ch_expr = &args[1].expr;
    let sample_expr = &args[2].expr;
    let value_expr = &args[3].expr;
    let data = lower_buffer_element_ptr_2d(
        ctx,
        base,
        ch_expr,
        sample_expr,
        locals,
        local_aliases,
        local_array_aliases,
        clamp_index,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"buf2_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn lower_orc_unsafe_data_read_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_read", 2, "ORC expression lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_read", "ORC expression lowering")?;
    let index_expr = &args[1].expr;
    if let Some(info) = ctx.input_arrays.get(base).copied() {
        return lower_input_array_index_read(
            ctx,
            info,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        );
    }
    if let Some(info) = ctx.param_arrays.get(base).copied() {
        return lower_param_array_index_read(
            ctx,
            base,
            info,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        );
    }
    if ctx.output_arrays.contains_key(base) {
        let data = lower_output_array_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        )?;
        return Ok(OrcValue {
            value: LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, data.elem_ty),
                data.ptr,
                b"unsafe_out_arr_read\0".as_ptr().cast(),
            ),
            ty: data.elem_ty,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        let data = lower_buffer_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        )?;
        return Ok(OrcValue {
            value: LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, data.elem_ty),
                data.ptr,
                b"unsafe_buf_read\0".as_ptr().cast(),
            ),
            ty: data.elem_ty,
        });
    }
    let data = lower_data_element_ptr_unchecked(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_array_aliases,
    )?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"unsafe_data_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn lower_orc_unsafe_data_write_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_write", 3, "ORC expression lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_write", "ORC expression lowering")?;
    let index_expr = &args[1].expr;
    let value_expr = &args[2].expr;
    if ctx.input_arrays.contains_key(base) || ctx.param_arrays.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "unsafe_write cannot target immutable top-level array '{base}' in ORC lowering"
        )));
    }
    if ctx.output_arrays.contains_key(base) {
        let data = lower_output_array_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_array_aliases)?;
        let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_out_arr_write_cast\0");
        LLVMBuildStore(ctx.builder, casted, data.ptr);
        return Ok(OrcValue {
            value: casted,
            ty: data.elem_ty,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        let data = lower_buffer_element_ptr(
            ctx,
            base,
            index_expr,
            locals,
            local_aliases,
            local_array_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_array_aliases)?;
        let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_buf_write_cast\0");
        LLVMBuildStore(ctx.builder, casted, data.ptr);
        return Ok(OrcValue {
            value: casted,
            ty: data.elem_ty,
        });
    }
    let data = lower_data_element_ptr_unchecked(
        ctx,
        base,
        index_expr,
        locals,
        local_aliases,
        local_array_aliases,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_array_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_data_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn lower_def_unsafe_data_read_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_read", 2, "def lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_read", "def lowering")?;
    let data = lower_def_data_element_ptr(ctx, base, &args[1].expr, false)?;
    Ok(OrcValue {
        value: LLVMBuildLoad2(
            ctx.builder,
            llvm_ty_for_primitive(ctx.context, data.elem_ty),
            data.ptr,
            b"def_unsafe_data_read\0".as_ptr().cast(),
        ),
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn lower_def_unsafe_data_write_call(
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_data_call_positional_arity(args, "unsafe_write", 3, "def lowering")?;
    let base = builtin_data_call_base_symbol(args, "unsafe_write", "def lowering")?;
    let data = lower_def_data_element_ptr(ctx, base, &args[1].expr, false)?;
    let value = lower_def_expr(&args[2].expr, ctx)?;
    let casted = cast_def_value_to(ctx, value, data.elem_ty, b"def_unsafe_data_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

pub(in crate::orc_backend) unsafe fn load_orc_buffer_binding_tuple(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<(LLVMValueRef, LLVMValueRef, LLVMValueRef, LLVMValueRef), Diagnostic> {
    let Some(buf_idx) = ctx.buffer_index.get(base).copied() else {
        return Err(Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC buffer call argument lowering"
        )));
    };
    let idx = LLVMConstInt(ctx.i32_ty, buf_idx as u64, 0);
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        ctx.buffer_ptrs,
        idx,
        b"call_buf_ptr_ptr\0",
    );
    let ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"call_buf_ptr\0".as_ptr().cast(),
    );
    let frames_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        ctx.buffer_frames_ptr,
        idx,
        b"call_buf_frames_ptr\0",
    );
    let frames = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        frames_ptr,
        b"call_buf_frames\0".as_ptr().cast(),
    );
    let channels = if ctx.buffer_mono.contains(base) {
        LLVMConstInt(ctx.i32_ty, 1, 0)
    } else {
        let channels_ptr = build_ptr_offset(
            ctx.builder,
            ctx.i32_ty,
            ctx.buffer_channels_ptr,
            idx,
            b"call_buf_channels_ptr\0",
        );
        LLVMBuildLoad2(
            ctx.builder,
            ctx.i32_ty,
            channels_ptr,
            b"call_buf_channels\0".as_ptr().cast(),
        )
    };
    let samplerate_ptr = build_ptr_offset(
        ctx.builder,
        ctx.float_ty,
        ctx.buffer_samplerates_ptr,
        idx,
        b"call_buf_samplerate_ptr\0",
    );
    let sample_rate = LLVMBuildLoad2(
        ctx.builder,
        ctx.float_ty,
        samplerate_ptr,
        b"call_buf_samplerate\0".as_ptr().cast(),
    );
    Ok((ptr, frames, channels, sample_rate))
}

pub(in crate::orc_backend) unsafe fn lower_buffer_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
) -> Result<(), Diagnostic> {
    if let Expr::Var { name: base, .. } = arg_expr {
        if let Ok((ptr, frames, channels, sample_rate)) = load_orc_buffer_binding_tuple(ctx, base) {
            push_buffer_tuple(out_args, ptr, frames, channels, sample_rate);
            return Ok(());
        }

        // Allow untyped indexable params to accept primitive arrays by adapting
        // them to a mono buffer tuple: (ptr, frames=len, channels=1).
        let (_elem_ty, len) =
            infer_array_arg_signature_in_orc(ctx, local_array_aliases, arg_expr, callee_name)?;
        return lower_array_as_mono_buffer_tuple(
            out_args,
            ctx.i32_ty,
            ctx.float_ty,
            base,
            len,
            "ORC expression lowering",
            |ptr_out| {
                lower_array_call_args_in_orc(
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    ptr_out,
                    arg_expr,
                    callee_name,
                )
            },
        );
    }

    let (index_expr, slot_exprs) =
        parse_proc_index_buffer_selector_args(arg_expr, callee_name, "ORC expression lowering")?;
    let clamped_idx = lower_clamped_data_index(
        ctx,
        index_expr,
        slot_exprs.len(),
        locals,
        local_aliases,
        local_array_aliases,
    )?;
    let slot_buffer_indices = slot_exprs
        .iter()
        .map(|slot_expr| match slot_expr {
            Expr::Var { name: base, .. } => ctx.buffer_index.get(base).copied(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            Diagnostic::internal(format!(
                "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' slot arguments must resolve to declared runtime buffers in ORC expression lowering"
            ))
        })?;

    let proc_slot_refs = ctx
        .proc_slot_buffer_refs
        .get(&slot_buffer_indices)
        .ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing proc-slot buffer-ref metadata for selector signature {:?} in ORC expression lowering",
                slot_buffer_indices
            ))
        })?;
    if proc_slot_refs.len != slot_buffer_indices.len() {
        return Err(Diagnostic::internal(
            "proc-slot buffer-ref metadata length mismatch in ORC expression lowering",
        ));
    }
    let i8_ty = LLVMInt8TypeInContext(ctx.context);
    let i8_ptr_ty = LLVMPointerType(i8_ty, 0);
    let ptr_ptr = build_ptr_offset(
        ctx.builder,
        i8_ptr_ty,
        proc_slot_refs.ptrs_base,
        clamped_idx,
        b"proc_buf_ref_ptr_ptr\0",
    );
    let ptr = LLVMBuildLoad2(
        ctx.builder,
        i8_ptr_ty,
        ptr_ptr,
        b"proc_buf_ref_ptr\0".as_ptr().cast(),
    );
    let frames_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        proc_slot_refs.frames_base,
        clamped_idx,
        b"proc_buf_ref_frames_ptr\0",
    );
    let frames = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        frames_ptr,
        b"proc_buf_ref_frames\0".as_ptr().cast(),
    );
    let channels_ptr = build_ptr_offset(
        ctx.builder,
        ctx.i32_ty,
        proc_slot_refs.channels_base,
        clamped_idx,
        b"proc_buf_ref_chans_ptr\0",
    );
    let channels = LLVMBuildLoad2(
        ctx.builder,
        ctx.i32_ty,
        channels_ptr,
        b"proc_buf_ref_chans\0".as_ptr().cast(),
    );
    let samplerate_ptr = build_ptr_offset(
        ctx.builder,
        ctx.float_ty,
        proc_slot_refs.samplerates_base,
        clamped_idx,
        b"proc_buf_ref_sr_ptr\0",
    );
    let sample_rate = LLVMBuildLoad2(
        ctx.builder,
        ctx.float_ty,
        samplerate_ptr,
        b"proc_buf_ref_sr\0".as_ptr().cast(),
    );
    push_buffer_tuple(out_args, ptr, frames, channels, sample_rate);
    Ok(())
}

pub(in crate::orc_backend) fn infer_buffer_arg_signature_in_orc(
    ctx: &LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    if let Expr::Var { name: base, .. } = arg_expr {
        if let (Some(elem_ty), Some(channels)) = (
            ctx.buffer_elem_types.get(base).copied(),
            ctx.buffer_channels.get(base).cloned(),
        ) {
            return Ok((elem_ty, channels));
        }
        let (elem_ty, _len) =
            infer_array_arg_signature_in_orc(ctx, local_array_aliases, arg_expr, callee_name)?;
        return Ok((elem_ty, TypedBufferChannels::Mono));
    }
    infer_buffer_selector_signature_common(
        arg_expr,
        callee_name,
        "ORC expression lowering",
        |slot_expr, callee_name| {
            infer_buffer_arg_signature_in_orc(ctx, local_array_aliases, slot_expr, callee_name)
        },
    )
}

fn infer_static_slice_len_hint_codegen(
    total_len: Option<usize>,
    start: Option<&Expr>,
    end: Option<&Expr>,
) -> usize {
    let Some(total_len) = total_len else {
        return 1;
    };
    let start = normalize_static_slice_bound_codegen(start, total_len, false);
    let end = normalize_static_slice_bound_codegen(end, total_len, true);
    end.saturating_sub(start).max(1)
}

fn normalize_static_slice_bound_codegen(
    expr: Option<&Expr>,
    total_len: usize,
    default_to_len: bool,
) -> usize {
    let Some(expr) = expr else {
        return if default_to_len { total_len } else { 0 };
    };
    let raw = match expr {
        Expr::Int { value: v, .. } => Some(*v),
        Expr::Number { value: v, .. } => {
            let truncated = v.trunc();
            if (v - truncated).abs() <= 1e-6 {
                Some(truncated as i64)
            } else {
                None
            }
        }
        _ => None,
    }
    .unwrap_or(if default_to_len { total_len as i64 } else { 0 });
    let adjusted = if raw < 0 { total_len as i64 + raw } else { raw };
    adjusted.clamp(0, total_len as i64) as usize
}

fn slice_llvm_name(prefix: &str, suffix: &str) -> Vec<u8> {
    let mut name = format!("{prefix}_{suffix}").into_bytes();
    name.push(0);
    name
}

#[derive(Clone, Copy)]
struct CodegenArrayViewSig {
    elem_ty: PrimitiveType,
    len_hint: usize,
}

#[derive(Clone, Copy)]
pub(in crate::orc_backend) struct CodegenArrayView {
    pub(in crate::orc_backend) base_ptr: LLVMValueRef,
    pub(in crate::orc_backend) len_val: LLVMValueRef,
    pub(in crate::orc_backend) elem_ty: PrimitiveType,
    pub(in crate::orc_backend) len_hint: usize,
}

fn infer_array_view_signature_common<FInferBase>(
    arg_expr: &Expr,
    callee_name: &str,
    context: &str,
    mut infer_base_signature: FInferBase,
) -> Result<(PrimitiveType, usize), Diagnostic>
where
    FInferBase: FnMut(&str, &str) -> Result<CodegenArrayViewSig, Diagnostic>,
{
    match arg_expr {
        Expr::Var { name: base, .. } => {
            let sig = infer_base_signature(base, callee_name)?;
            Ok((sig.elem_ty, sig.len_hint))
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            let sig = infer_base_signature(base, callee_name)?;
            Ok((
                sig.elem_ty,
                infer_static_slice_len_hint_codegen(
                    Some(sig.len_hint),
                    start.as_deref(),
                    end.as_deref(),
                ),
            ))
        }
        _ => Err(Diagnostic::internal(format!(
            "function '{callee_name}' array argument must be an array symbol or slice in {context}"
        ))),
    }
}

fn infer_buffer_selector_signature_common<FInferSlot>(
    arg_expr: &Expr,
    callee_name: &str,
    context: &str,
    mut infer_slot_signature: FInferSlot,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic>
where
    FInferSlot: FnMut(&Expr, &str) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic>,
{
    let (_index_expr, slot_exprs) =
        parse_proc_index_buffer_selector_args(arg_expr, callee_name, context)?;
    let mut inferred = None::<(PrimitiveType, TypedBufferChannels)>;
    for slot_expr in slot_exprs {
        let sig = infer_slot_signature(slot_expr, callee_name)?;
        if let Some(prev) = &inferred {
            if prev != &sig {
                return Err(Diagnostic::internal(format!(
                    "function '{callee_name}' buffer selector argument mixes incompatible slot buffer signatures in {context}"
                )));
            }
        } else {
            inferred = Some(sig);
        }
    }
    inferred.ok_or_else(|| {
        Diagnostic::internal(format!(
            "function '{callee_name}' buffer selector argument has no slots in {context}"
        ))
    })
}

unsafe fn lower_slice_bound_common<FLowerI32>(
    builder: LLVMBuilderRef,
    i32_ty: LLVMTypeRef,
    len_val: LLVMValueRef,
    expr: Option<&Expr>,
    default_to_len: bool,
    llvm_prefix: &str,
    mut lower_i32_expr: FLowerI32,
) -> Result<LLVMValueRef, Diagnostic>
where
    FLowerI32: FnMut(&Expr) -> Result<LLVMValueRef, Diagnostic>,
{
    let raw = if let Some(expr) = expr {
        lower_i32_expr(expr)?
    } else if default_to_len {
        len_val
    } else {
        const_i32(i32_ty, 0)
    };
    let zero = const_i32(i32_ty, 0);
    let neg_name = slice_llvm_name(llvm_prefix, "bound_neg");
    let add_len_name = slice_llvm_name(llvm_prefix, "bound_add_len");
    let adj_name = slice_llvm_name(llvm_prefix, "bound_adj");
    let lt_zero_name = slice_llvm_name(llvm_prefix, "bound_lt_zero");
    let clamped_low_name = slice_llvm_name(llvm_prefix, "bound_clamped_low");
    let gt_len_name = slice_llvm_name(llvm_prefix, "bound_gt_len");
    let clamped_name = slice_llvm_name(llvm_prefix, "bound_clamped");
    let is_neg = LLVMBuildICmp(
        builder,
        llvm_sys::LLVMIntPredicate::LLVMIntSLT,
        raw,
        zero,
        neg_name.as_ptr().cast(),
    );
    let adjusted = LLVMBuildSelect(
        builder,
        is_neg,
        LLVMBuildAdd(builder, len_val, raw, add_len_name.as_ptr().cast()),
        raw,
        adj_name.as_ptr().cast(),
    );
    let clamped_low = LLVMBuildSelect(
        builder,
        LLVMBuildICmp(
            builder,
            llvm_sys::LLVMIntPredicate::LLVMIntSLT,
            adjusted,
            zero,
            lt_zero_name.as_ptr().cast(),
        ),
        zero,
        adjusted,
        clamped_low_name.as_ptr().cast(),
    );
    Ok(LLVMBuildSelect(
        builder,
        LLVMBuildICmp(
            builder,
            llvm_sys::LLVMIntPredicate::LLVMIntSGT,
            clamped_low,
            len_val,
            gt_len_name.as_ptr().cast(),
        ),
        len_val,
        clamped_low,
        clamped_name.as_ptr().cast(),
    ))
}

unsafe fn lower_array_view_common<FLowerBase, FLowerBound>(
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    i32_ty: LLVMTypeRef,
    arg_expr: &Expr,
    callee_name: &str,
    context_name: &str,
    llvm_prefix: &str,
    mut lower_base_view: FLowerBase,
    mut lower_slice_bound: FLowerBound,
) -> Result<CodegenArrayView, Diagnostic>
where
    FLowerBase: FnMut(&str, &str) -> Result<CodegenArrayView, Diagnostic>,
    FLowerBound: FnMut(Option<&Expr>, LLVMValueRef, bool) -> Result<LLVMValueRef, Diagnostic>,
{
    match arg_expr {
        Expr::Var { name: base, .. } => lower_base_view(base, callee_name),
        Expr::Slice {
            base,
            start,
            end,
            ..
        } => {
            let base_view = lower_base_view(base, callee_name)?;
            let start_idx = lower_slice_bound(start.as_deref(), base_view.len_val, false)?;
            let end_idx = lower_slice_bound(end.as_deref(), base_view.len_val, true)?;
            let end_before_start_name = slice_llvm_name(llvm_prefix, "end_before_start");
            let len_diff_name = slice_llvm_name(llvm_prefix, "len_diff");
            let len_name = slice_llvm_name(llvm_prefix, "len");
            let base_ptr_name = slice_llvm_name(llvm_prefix, "base_ptr");
            let end_before_start = LLVMBuildICmp(
                builder,
                llvm_sys::LLVMIntPredicate::LLVMIntSLT,
                end_idx,
                start_idx,
                end_before_start_name.as_ptr().cast(),
            );
            let diff = LLVMBuildSub(
                builder,
                end_idx,
                start_idx,
                len_diff_name.as_ptr().cast(),
            );
            let slice_len = LLVMBuildSelect(
                builder,
                end_before_start,
                const_i32(i32_ty, 0),
                diff,
                len_name.as_ptr().cast(),
            );
            Ok(CodegenArrayView {
                base_ptr: build_f32_ptr_offset(
                    builder,
                    llvm_ty_for_primitive(context, base_view.elem_ty),
                    base_view.base_ptr,
                    start_idx,
                    base_ptr_name.as_slice(),
                ),
                len_val: slice_len,
                elem_ty: base_view.elem_ty,
                len_hint: infer_static_slice_len_hint_codegen(
                    Some(base_view.len_hint),
                    start.as_deref(),
                    end.as_deref(),
                ),
            })
        }
        _ => Err(Diagnostic::internal(format!(
            "function '{callee_name}' array argument must be an array symbol or slice in {context_name}"
        ))),
    }
}

fn infer_orc_array_base_signature(
    ctx: &LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    base: &str,
    callee_name: &str,
) -> Result<CodegenArrayViewSig, Diagnostic> {
    if let Some(alias) = local_array_aliases.get(base) {
        return match alias {
            LocalArrayAlias::Primitive { elem_ty, len, .. } => Ok(CodegenArrayViewSig {
                elem_ty: *elem_ty,
                len_hint: *len,
            }),
            LocalArrayAlias::Struct { .. } => Err(Diagnostic::internal(format!(
                "function '{callee_name}' array argument '{base}' must have primitive elements in ORC expression lowering"
            ))),
        };
    }
    if ctx.input_arrays.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' cannot pass input array '{base}' by reference in ORC expression lowering"
        )));
    }
    if let Some(info) = ctx.param_arrays.get(base).copied() {
        return Ok(CodegenArrayViewSig {
            elem_ty: info.elem_ty,
            len_hint: info.len,
        });
    }
    if let Some(info) = ctx.output_arrays.get(base).copied() {
        return Ok(CodegenArrayViewSig {
            elem_ty: info.elem_ty,
            len_hint: info.len,
        });
    }
    if let Some(len) = ctx.array_len.get(base).copied() {
        let elem_ty = *ctx.array_elem_ty.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing array element type metadata for '{base}' in ORC array signature inference"
            ))
        })?;
        return Ok(CodegenArrayViewSig {
            elem_ty,
            len_hint: len,
        });
    }
    if let Some(elem_ty) = ctx.buffer_elem_types.get(base).copied() {
        return Ok(CodegenArrayViewSig {
            elem_ty,
            len_hint: 1,
        });
    }
    Err(Diagnostic::internal(format!(
        "unknown array symbol '{base}' in ORC array signature inference for function '{callee_name}'"
    )))
}

fn infer_orc_array_view_signature(
    ctx: &LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, usize), Diagnostic> {
    infer_array_view_signature_common(
        arg_expr,
        callee_name,
        "ORC expression lowering",
        |base, callee_name| {
            infer_orc_array_base_signature(ctx, local_array_aliases, base, callee_name)
        },
    )
}

unsafe fn lower_orc_array_base_view(
    ctx: &mut LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    base: &str,
    callee_name: &str,
) -> Result<CodegenArrayView, Diagnostic> {
    if let Some(alias) = local_array_aliases.get(base) {
        return match alias {
            LocalArrayAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => Ok(CodegenArrayView {
                base_ptr: *base_ptr,
                len_val: ctx
                    .array_len_values
                    .get(base)
                    .copied()
                    .unwrap_or_else(|| LLVMConstInt(ctx.i32_ty, *len as u64, 0)),
                elem_ty: *elem_ty,
                len_hint: *len,
            }),
            LocalArrayAlias::Struct { .. } => Err(Diagnostic::internal(format!(
                "function '{callee_name}' array argument '{base}' must have primitive elements in ORC expression lowering"
            ))),
        };
    }
    if ctx.input_arrays.contains_key(base) {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' cannot pass input array '{base}' by reference in ORC expression lowering"
        )));
    }
    if let Some(info) = ctx.param_arrays.get(base).copied() {
        let base_byte_offset = ctx
            .param_byte_offset
            .get(base)
            .copied()
            .ok_or_else(|| Diagnostic::internal(format!("unknown parameter array '{base}'")))?;
        if base_byte_offset > i32::MAX as usize {
            return Err(Diagnostic::internal(
                "parameter array offset exceeds supported i32 index range in ORC lowering",
            ));
        }
        return Ok(CodegenArrayView {
            base_ptr: build_typed_ptr_from_byte_offset(
                ctx.builder,
                ctx.context,
                ctx.params_ptr,
                LLVMConstInt(ctx.i32_ty, base_byte_offset as u64, 0),
                info.elem_ty,
                b"param_arr_ref_ptr_i8\0",
                b"param_arr_ref_ptr_typed\0",
            ),
            len_val: LLVMConstInt(ctx.i32_ty, info.len as u64, 0),
            elem_ty: info.elem_ty,
            len_hint: info.len,
        });
    }
    if let Some(info) = ctx.output_arrays.get(base).copied() {
        let ptr = *ctx.out_array_base_ptrs.get(base).ok_or_else(|| {
            Diagnostic::internal(format!("missing output array storage for '{base}'"))
        })?;
        return Ok(CodegenArrayView {
            base_ptr: ptr,
            len_val: LLVMConstInt(ctx.i32_ty, info.len as u64, 0),
            elem_ty: info.elem_ty,
            len_hint: info.len,
        });
    }
    if let Some(ptr) = ctx.array_base_ptrs.get(base).copied() {
        if !ctx.array_elem_ty.contains_key(base) {
            return Err(Diagnostic::internal(format!(
                "array argument '{base}' is not primitive in ORC expression lowering"
            )));
        }
        let len = ctx.array_len.get(base).copied().unwrap_or(1);
        return Ok(CodegenArrayView {
            base_ptr: ptr,
            len_val: ctx
                .array_len_values
                .get(base)
                .copied()
                .unwrap_or_else(|| LLVMConstInt(ctx.i32_ty, len as u64, 0)),
            elem_ty: *ctx.array_elem_ty.get(base).unwrap_or(&PrimitiveType::F32),
            len_hint: len,
        });
    }
    if let Some(buf_idx) = ctx.buffer_index.get(base).copied() {
        let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
            Diagnostic::internal(format!(
                "missing element type metadata for buffer '{base}' in ORC expression lowering"
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
            b"buf_slice_ptr_ptr\0",
        );
        let raw_ptr = LLVMBuildLoad2(
            ctx.builder,
            i8_ptr_ty,
            ptr_ptr,
            b"buf_slice_ptr\0".as_ptr().cast(),
        );
        let typed_ptr = LLVMBuildBitCast(
            ctx.builder,
            raw_ptr,
            LLVMPointerType(llvm_ty_for_primitive(ctx.context, elem_ty), 0),
            b"buf_slice_ptr_typed\0".as_ptr().cast(),
        );
        return Ok(CodegenArrayView {
            base_ptr: typed_ptr,
            len_val: load_orc_buffer_frames_i32(ctx, base)?,
            elem_ty,
            len_hint: 1,
        });
    }
    Err(Diagnostic::internal(format!(
        "unknown array symbol '{base}' in ORC array call argument lowering for function '{callee_name}'"
    )))
}

unsafe fn lower_orc_slice_bound(
    ctx: &mut LoweringCtx<'_>,
    expr: Option<&Expr>,
    len_val: LLVMValueRef,
    default_to_len: bool,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<LLVMValueRef, Diagnostic> {
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    lower_slice_bound_common(
        ctx.builder,
        ctx.i32_ty,
        len_val,
        expr,
        default_to_len,
        "slice",
        |expr| unsafe {
            let lowered = lower_expr(
                expr,
                &mut *ctx_ptr,
                locals,
                local_aliases,
                local_array_aliases,
            )?;
            Ok(cast_orc_value_to(
                &*ctx_ptr,
                lowered,
                PrimitiveType::I32,
                b"slice_bound_i32\0",
            ))
        },
    )
}

pub(in crate::orc_backend) unsafe fn lower_orc_array_view(
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<CodegenArrayView, Diagnostic> {
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    lower_array_view_common(
        ctx.builder,
        ctx.context,
        ctx.i32_ty,
        arg_expr,
        callee_name,
        "ORC expression lowering",
        "slice",
        |base, callee_name| unsafe {
            lower_orc_array_base_view(&mut *ctx_ptr, local_array_aliases, base, callee_name)
        },
        |expr, len_val, default_to_len| unsafe {
            lower_orc_slice_bound(
                &mut *ctx_ptr,
                expr,
                len_val,
                default_to_len,
                locals,
                local_aliases,
                local_array_aliases,
            )
        },
    )
}

pub(in crate::orc_backend) fn infer_array_arg_signature_in_orc(
    ctx: &LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, usize), Diagnostic> {
    infer_orc_array_view_signature(ctx, local_array_aliases, arg_expr, callee_name)
}

pub(in crate::orc_backend) unsafe fn lower_array_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let view = lower_orc_array_view(
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        arg_expr,
        callee_name,
    )?;
    out_args.push(view.base_ptr);
    out_args.push(view.len_val);
    Ok(())
}

pub(in crate::orc_backend) unsafe fn lower_buffer_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    if let Expr::Var { name: base, .. } = arg_expr {
        if let Some(info) = ctx.buffer_params.get(base) {
            push_buffer_tuple(out_args, info.ptr, info.frames, info.channels, info.sample_rate);
            return Ok(());
        }

        // Allow untyped indexable params to accept primitive arrays by adapting
        // them to a mono buffer tuple: (ptr, frames=len, channels=1).
        let (_elem_ty, len) = infer_array_arg_signature_in_def(ctx, arg_expr, callee_name)?;
        // Check for runtime length value from array param
        let runtime_len = ctx.array_len_values.get(base).copied();
        return lower_array_as_mono_buffer_tuple_ext(
            out_args,
            ctx.i32_ty,
            ctx.float_ty,
            base,
            len,
            runtime_len,
            "def lowering",
            |ptr_out| {
                // Only push the pointer, not the length (buffer tuple has its own format)
                let Expr::Var { name: b, .. } = arg_expr else {
                    unreachable!()
                };
                if let Some(alias) = ctx.local_array_aliases.get(b) {
                    return match alias {
                        LocalArrayAlias::Primitive { base_ptr, .. } => {
                            ptr_out.push(*base_ptr);
                            Ok(())
                        }
                        LocalArrayAlias::Struct { .. } => Err(Diagnostic::internal(format!(
                            "function '{callee_name}' array argument '{b}' must have primitive elements in def lowering"
                        ))),
                    };
                }
                let ptr = *ctx.array_ptrs.get(b).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "unknown array symbol '{b}' in def array call argument lowering"
                    ))
                })?;
                ptr_out.push(ptr);
                Ok(())
            },
        );
    }

    let (index_expr, slot_exprs) =
        parse_proc_index_buffer_selector_args(arg_expr, callee_name, "def lowering")?;
    let raw_index = lower_def_expr(index_expr, ctx)?;
    let index_i32 = cast_def_value_to(
        ctx,
        raw_index,
        PrimitiveType::I32,
        b"def_proc_buf_sel_idx\0",
    );
    let clamped_idx = clamp_data_index(ctx.builder, ctx.i32_ty, index_i32, slot_exprs.len())?;
    let mut slot_tuples = Vec::<(LLVMValueRef, LLVMValueRef, LLVMValueRef, LLVMValueRef)>::new();
    for slot_expr in slot_exprs.iter() {
        let mut tuple_args = Vec::<LLVMValueRef>::new();
        lower_buffer_call_args_in_def(ctx, &mut tuple_args, slot_expr, callee_name)?;
        if tuple_args.len() != 4 {
            return Err(Diagnostic::internal(format!(
                "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' produced invalid buffer tuple width in def lowering"
            )));
        }
        slot_tuples.push((tuple_args[0], tuple_args[1], tuple_args[2], tuple_args[3]));
    }
    let Some(first_tuple) = slot_tuples.first().copied() else {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' has no selectable slot buffers in def lowering"
        )));
    };
    let ptr_ty = LLVMTypeOf(first_tuple.0);
    let frames_ty = LLVMTypeOf(first_tuple.1);
    let channels_ty = LLVMTypeOf(first_tuple.2);
    let sr_ty = LLVMTypeOf(first_tuple.3);
    if slot_tuples.iter().any(|tuple| {
        LLVMTypeOf(tuple.0) != ptr_ty
            || LLVMTypeOf(tuple.1) != frames_ty
            || LLVMTypeOf(tuple.2) != channels_ty
            || LLVMTypeOf(tuple.3) != sr_ty
    }) {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' mixes incompatible slot buffer tuple LLVM types in def lowering"
        )));
    }
    let ptr_candidates = slot_tuples.iter().map(|t| t.0).collect::<Vec<_>>();
    let frames_candidates = slot_tuples.iter().map(|t| t.1).collect::<Vec<_>>();
    let channels_candidates = slot_tuples.iter().map(|t| t.2).collect::<Vec<_>>();
    let sr_candidates = slot_tuples.iter().map(|t| t.3).collect::<Vec<_>>();
    let ptr = select_def_value_by_slot_index(
        ctx,
        clamped_idx,
        &ptr_candidates,
        b"def_proc_buf_sel_ptr\0",
        "def buffer tuple selector pointer dispatch",
    )?;
    let frames = select_def_value_by_slot_index(
        ctx,
        clamped_idx,
        &frames_candidates,
        b"def_proc_buf_sel_frames\0",
        "def buffer tuple selector frames dispatch",
    )?;
    let channels = select_def_value_by_slot_index(
        ctx,
        clamped_idx,
        &channels_candidates,
        b"def_proc_buf_sel_chans\0",
        "def buffer tuple selector channels dispatch",
    )?;
    let sample_rate = select_def_value_by_slot_index(
        ctx,
        clamped_idx,
        &sr_candidates,
        b"def_proc_buf_sel_sr\0",
        "def buffer tuple selector sample-rate dispatch",
    )?;
    push_buffer_tuple(out_args, ptr, frames, channels, sample_rate);
    Ok(())
}

pub(in crate::orc_backend) fn infer_buffer_arg_signature_in_def(
    ctx: &DefLoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    if let Expr::Var { name: base, .. } = arg_expr {
        if let Some(info) = ctx.buffer_params.get(base) {
            return Ok((info.elem_ty, info.declared_channels.clone()));
        }
        let (elem_ty, _len) = infer_array_arg_signature_in_def(ctx, arg_expr, callee_name)?;
        return Ok((elem_ty, TypedBufferChannels::Mono));
    }
    infer_buffer_selector_signature_common(
        arg_expr,
        callee_name,
        "def lowering",
        |slot_expr, callee_name| infer_buffer_arg_signature_in_def(ctx, slot_expr, callee_name),
    )
}

fn infer_def_array_base_signature(
    ctx: &DefLoweringCtx<'_>,
    base: &str,
    callee_name: &str,
) -> Result<CodegenArrayViewSig, Diagnostic> {
    if let Some(alias) = ctx.local_array_aliases.get(base) {
        return match alias {
            LocalArrayAlias::Primitive { elem_ty, len, .. } => Ok(CodegenArrayViewSig {
                elem_ty: *elem_ty,
                len_hint: *len,
            }),
            LocalArrayAlias::Struct { .. } => Err(Diagnostic::internal(format!(
                "function '{callee_name}' array argument '{base}' must have primitive elements in def lowering"
            ))),
        };
    }
    if let Some(info) = ctx.buffer_params.get(base) {
        return Ok(CodegenArrayViewSig {
            elem_ty: info.elem_ty,
            len_hint: 1,
        });
    }
    let len = ctx.array_len.get(base).copied().ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown array symbol '{base}' in def array signature inference"
        ))
    })?;
    let elem_ty = *ctx.array_elem_ty.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing array element type metadata for '{base}' in def array signature inference"
        ))
    })?;
    Ok(CodegenArrayViewSig {
        elem_ty,
        len_hint: len,
    })
}

unsafe fn lower_def_array_base_view(
    ctx: &mut DefLoweringCtx<'_>,
    base: &str,
    callee_name: &str,
) -> Result<CodegenArrayView, Diagnostic> {
    if let Some(alias) = ctx.local_array_aliases.get(base) {
        return match alias {
            LocalArrayAlias::Primitive {
                base_ptr,
                len,
                elem_ty,
            } => Ok(CodegenArrayView {
                base_ptr: *base_ptr,
                len_val: ctx
                    .array_len_values
                    .get(base)
                    .copied()
                    .unwrap_or_else(|| LLVMConstInt(ctx.i32_ty, *len as u64, 0)),
                elem_ty: *elem_ty,
                len_hint: *len,
            }),
            LocalArrayAlias::Struct { .. } => Err(Diagnostic::internal(format!(
                "function '{callee_name}' array argument '{base}' must have primitive elements in def lowering"
            ))),
        };
    }
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        let typed_ptr = LLVMBuildBitCast(
            ctx.builder,
            info.ptr,
            LLVMPointerType(llvm_ty_for_primitive(ctx.context, info.elem_ty), 0),
            b"def_buf_slice_ptr_typed\0".as_ptr().cast(),
        );
        return Ok(CodegenArrayView {
            base_ptr: typed_ptr,
            len_val: load_def_buffer_frames_i32(ctx, base, &info)?,
            elem_ty: info.elem_ty,
            len_hint: 1,
        });
    }
    let ptr = *ctx.array_ptrs.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown array symbol '{base}' in def array call argument lowering"
        ))
    })?;
    let len = ctx.array_len.get(base).copied().unwrap_or(1);
    let elem_ty = *ctx.array_elem_ty.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "array argument '{base}' is not primitive in def lowering"
        ))
    })?;
    Ok(CodegenArrayView {
        base_ptr: ptr,
        len_val: ctx
            .array_len_values
            .get(base)
            .copied()
            .unwrap_or_else(|| LLVMConstInt(ctx.i32_ty, len as u64, 0)),
        elem_ty,
        len_hint: len,
    })
}

unsafe fn lower_def_slice_bound(
    ctx: &mut DefLoweringCtx<'_>,
    expr: Option<&Expr>,
    len_val: LLVMValueRef,
    default_to_len: bool,
) -> Result<LLVMValueRef, Diagnostic> {
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    lower_slice_bound_common(
        ctx.builder,
        ctx.i32_ty,
        len_val,
        expr,
        default_to_len,
        "def_slice",
        |expr| unsafe {
            let lowered = lower_def_expr(expr, &mut *ctx_ptr)?;
            Ok(cast_def_value_to(
                &*ctx_ptr,
                lowered,
                PrimitiveType::I32,
                b"def_slice_bound_i32\0",
            ))
        },
    )
}

pub(in crate::orc_backend) unsafe fn lower_def_array_view(
    ctx: &mut DefLoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<CodegenArrayView, Diagnostic> {
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    lower_array_view_common(
        ctx.builder,
        ctx.context,
        ctx.i32_ty,
        arg_expr,
        callee_name,
        "def lowering",
        "def_slice",
        |base, callee_name| unsafe { lower_def_array_base_view(&mut *ctx_ptr, base, callee_name) },
        |expr, len_val, default_to_len| unsafe {
            lower_def_slice_bound(&mut *ctx_ptr, expr, len_val, default_to_len)
        },
    )
}

pub(in crate::orc_backend) fn infer_array_arg_signature_in_def(
    ctx: &DefLoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, usize), Diagnostic> {
    infer_array_view_signature_common(
        arg_expr,
        callee_name,
        "def lowering",
        |base, callee_name| infer_def_array_base_signature(ctx, base, callee_name),
    )
}

pub(in crate::orc_backend) unsafe fn lower_array_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let view = lower_def_array_view(ctx, arg_expr, callee_name)?;
    out_args.push(view.base_ptr);
    out_args.push(view.len_val);
    Ok(())
}

fn push_buffer_tuple(
    out_args: &mut Vec<LLVMValueRef>,
    ptr: LLVMValueRef,
    frames: LLVMValueRef,
    channels: LLVMValueRef,
    sample_rate: LLVMValueRef,
) {
    out_args.push(ptr);
    out_args.push(frames);
    out_args.push(channels);
    out_args.push(sample_rate);
}

unsafe fn lower_array_as_mono_buffer_tuple(
    out_args: &mut Vec<LLVMValueRef>,
    i32_ty: LLVMTypeRef,
    float_ty: LLVMTypeRef,
    base: &str,
    len: usize,
    context: &str,
    lower_array_ptr: impl FnMut(&mut Vec<LLVMValueRef>) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    lower_array_as_mono_buffer_tuple_ext(
        out_args,
        i32_ty,
        float_ty,
        base,
        len,
        None,
        context,
        lower_array_ptr,
    )
}

unsafe fn lower_array_as_mono_buffer_tuple_ext(
    out_args: &mut Vec<LLVMValueRef>,
    i32_ty: LLVMTypeRef,
    float_ty: LLVMTypeRef,
    base: &str,
    len: usize,
    runtime_len: Option<LLVMValueRef>,
    context: &str,
    mut lower_array_ptr: impl FnMut(&mut Vec<LLVMValueRef>) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "array argument '{}' length {} exceeds i32 range while adapting to buffer tuple in {}",
            base, len, context
        )));
    }

    let mut ptr_only = Vec::with_capacity(1);
    lower_array_ptr(&mut ptr_only)?;
    let ptr = *ptr_only.first().ok_or_else(|| {
        Diagnostic::internal(format!(
            "failed to materialize array pointer for '{}' while adapting to buffer tuple in {}",
            base, context
        ))
    })?;
    let frames = runtime_len.unwrap_or_else(|| LLVMConstInt(i32_ty, len as u64, 0));
    push_buffer_tuple(
        out_args,
        ptr,
        frames,
        LLVMConstInt(i32_ty, 1, 0),
        LLVMConstReal(float_ty, 0.0),
    );
    Ok(())
}
