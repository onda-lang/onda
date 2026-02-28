use super::*;

pub(super) unsafe fn lower_expr(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<OrcValue, Diagnostic> {
    if let Some(literal) = lower_literal_expr_common(expr, ctx.context, ctx.i32_ty, ctx.float_ty) {
        return Ok(literal);
    }
    match expr {
        Expr::ArrayLiteral(_) => Err(Diagnostic::internal(
            "array literal is not a scalar expression in ORC lowering",
        )),
        Expr::Var(name) => {
            if let Some((ty, value)) =
                builtin_constant_value_and_type(name, ctx.sample_rate, ctx.block_size)
            {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_f64(ctx.context, ctx.float_ty, ty, value),
                    ty,
                });
            }
            if let Some(alias) = local_aliases.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, alias.ty),
                        alias.ptr,
                        b"alias_load\0".as_ptr().cast(),
                    ),
                    ty: alias.ty,
                });
            }
            if let Some(local) = locals.get(name) {
                return Ok(*local);
            }
            if let Some(byte_offset) = ctx.param_byte_offset.get(name).copied() {
                if byte_offset > i32::MAX as usize {
                    return Err(Diagnostic::internal(
                        "parameter byte offset exceeds supported i32 index range in ORC lowering",
                    ));
                }
                let param_ty = *ctx.param_types.get(name).unwrap_or(&PrimitiveType::F32);
                let ptr = build_typed_ptr_from_byte_offset(
                    ctx.builder,
                    ctx.context,
                    ctx.params_ptr,
                    const_i32(ctx.i32_ty, byte_offset as i32),
                    param_ty,
                    b"param_ptr_i8\0",
                    b"param_ptr_typed\0",
                );
                let raw = LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, param_ty),
                    ptr,
                    b"param_load\0".as_ptr().cast(),
                );
                return Ok(OrcValue {
                    value: raw,
                    ty: param_ty,
                });
            }
            if let Some(slot) = ctx.state_slots.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"state_load_expr\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            if let Some(slot) = ctx.out_slots.get(name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"out_load_expr\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            if let Some(ch) = ctx.input_index.get(name) {
                let in_ty = *ctx.input_types.get(name).unwrap_or(&PrimitiveType::F32);
                let ch_v = LLVMConstInt(ctx.i32_ty, *ch as u64, 0);
                let in_ptr_ptr = build_ptr_offset(
                    ctx.builder,
                    ctx.float_ptr_ty,
                    ctx.in_ptrs,
                    ch_v,
                    b"in_ch_ptr_ptr\0",
                );
                let in_ch_ptr = LLVMBuildLoad2(
                    ctx.builder,
                    ctx.float_ptr_ty,
                    in_ptr_ptr,
                    b"in_ch_ptr\0".as_ptr().cast(),
                );
                let in_ch_ptr_typed = LLVMBuildBitCast(
                    ctx.builder,
                    in_ch_ptr,
                    LLVMPointerType(llvm_ty_for_primitive(ctx.context, in_ty), 0),
                    b"in_ch_ptr_typed\0".as_ptr().cast(),
                );
                let ptr = build_f32_ptr_offset(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, in_ty),
                    in_ch_ptr_typed,
                    ctx.frame_idx,
                    b"in_ptr\0",
                );
                let raw = LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, in_ty),
                    ptr,
                    b"in_load\0".as_ptr().cast(),
                );
                if ctx.oversample_factor > 1 {
                    if let Some(input_cache_ptr) = ctx.oversample_input_cache {
                        let cached_ptr = build_f32_ptr_offset(
                            ctx.builder,
                            ctx.float_ty,
                            input_cache_ptr,
                            ch_v,
                            b"os_cached_in_expr_ptr\0",
                        );
                        let cached_f32 = LLVMBuildLoad2(
                            ctx.builder,
                            ctx.float_ty,
                            cached_ptr,
                            b"os_cached_in_expr\0".as_ptr().cast(),
                        );
                        let value = cast_orc_value_to(
                            ctx,
                            OrcValue {
                                value: cached_f32,
                                ty: PrimitiveType::F32,
                            },
                            in_ty,
                            b"os_cached_in_expr_cast\0",
                        );
                        return Ok(OrcValue { value, ty: in_ty });
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
                            ch_v,
                            b"os_prev_in_expr_ptr\0",
                        );
                        let prev = LLVMBuildLoad2(
                            ctx.builder,
                            ctx.float_ty,
                            prev_ptr,
                            b"os_prev_in_expr\0".as_ptr().cast(),
                        );
                        let current_f32 = cast_orc_value_to(
                            ctx,
                            OrcValue {
                                value: raw,
                                ty: in_ty,
                            },
                            PrimitiveType::F32,
                            b"os_in_curr_f32\0",
                        );
                        let diff = build_fsub_fast(
                            ctx.builder,
                            current_f32,
                            prev,
                            b"os_in_diff\0",
                            ctx.fast_math_flags,
                        );
                        let scaled = build_fmul_fast(
                            ctx.builder,
                            diff,
                            alpha,
                            b"os_in_scaled\0",
                            ctx.fast_math_flags,
                        );
                        let interp = build_fadd_fast(
                            ctx.builder,
                            prev,
                            scaled,
                            b"os_in_interp\0",
                            ctx.fast_math_flags,
                        );
                        let value = cast_orc_value_to(
                            ctx,
                            OrcValue {
                                value: interp,
                                ty: PrimitiveType::F32,
                            },
                            in_ty,
                            b"os_in_interp_cast\0",
                        );
                        return Ok(OrcValue { value, ty: in_ty });
                    }
                }
                return Ok(OrcValue {
                    value: raw,
                    ty: in_ty,
                });
            }
            if ctx.array_base_ptrs.contains_key(name) || ctx.array_struct_len.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "array symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            if ctx.buffer_index.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "buffer symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            if ctx.input_arrays.contains_key(name)
                || ctx.param_arrays.contains_key(name)
                || ctx.output_arrays.contains_key(name)
            {
                return Err(Diagnostic::internal(format!(
                    "top-level array symbol '{name}' must be indexed in ORC expression lowering"
                )));
            }
            Err(Diagnostic::internal(format!(
                "unknown symbol '{name}' in ORC expression lowering"
            )))
        }
        Expr::Index { base, index } => {
            if ctx.buffer_index.contains_key(base) {
                let data = lower_buffer_element_ptr(
                    ctx,
                    base,
                    index,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                )?;
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, data.elem_ty),
                        data.ptr,
                        b"buf_load\0".as_ptr().cast(),
                    ),
                    ty: data.elem_ty,
                });
            }
            if let Some(info) = ctx.input_arrays.get(base).copied() {
                return lower_input_array_index_read(
                    ctx,
                    info,
                    index,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                );
            }
            if let Some(info) = ctx.param_arrays.get(base).copied() {
                return lower_param_array_index_read(
                    ctx,
                    base,
                    info,
                    index,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                );
            }
            if ctx.output_arrays.contains_key(base) {
                let data = lower_output_array_element_ptr(
                    ctx,
                    base,
                    index,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                )?;
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, data.elem_ty),
                        data.ptr,
                        b"out_arr_load\0".as_ptr().cast(),
                    ),
                    ty: data.elem_ty,
                });
            }
            let data = lower_data_element_ptr(
                ctx,
                base,
                index,
                locals,
                local_aliases,
                local_array_aliases,
            )?;
            Ok(OrcValue {
                value: LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, data.elem_ty),
                    data.ptr,
                    b"array_load\0".as_ptr().cast(),
                ),
                ty: data.elem_ty,
            })
        }
        Expr::ArrayCtor { .. } => Err(Diagnostic::internal(
            "array constructor is only valid as an init assignment value",
        )),
        Expr::Binary { op, lhs, rhs } => {
            let left = lower_expr(lhs, ctx, locals, local_aliases, local_array_aliases)?;
            let right = lower_expr(rhs, ctx, locals, local_aliases, local_array_aliases)?;
            let builder = ctx.builder;
            let fast_math_flags = ctx.fast_math_flags;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_orc_value_to(ctx, value, to, name)
            };
            lower_binary_numeric_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                "ORC expression lowering",
            )
        }
        Expr::Compare { op, lhs, rhs } => {
            let left = lower_expr(lhs, ctx, locals, local_aliases, local_array_aliases)?;
            let right = lower_expr(rhs, ctx, locals, local_aliases, local_array_aliases)?;
            let builder = ctx.builder;
            let fast_math_flags = ctx.fast_math_flags;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_orc_value_to(ctx, value, to, name)
            };
            lower_compare_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                "ORC expression lowering",
            )
        }
        Expr::Cast { to, expr } => {
            let value = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
            let casted = cast_orc_value_to(ctx, value, *to, b"cast\0");
            Ok(OrcValue {
                value: casted,
                ty: *to,
            })
        }
        Expr::UnaryNot { expr } => {
            let value = lower_expr(expr, ctx, locals, local_aliases, local_array_aliases)?;
            let builder = ctx.builder;
            let context = ctx.context;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_orc_value_to(ctx, value, to, name)
            };
            Ok(lower_unary_not_common(
                value,
                builder,
                context,
                &mut cast_value,
            ))
        }
        Expr::Logical { op, lhs, rhs } => lower_orc_logical_expr(
            *op,
            lhs,
            rhs,
            ctx,
            locals,
            local_aliases,
            local_array_aliases,
        ),
        Expr::Call { func, args } => {
            let mut lowered = Vec::with_capacity(args.len());
            for arg in args {
                lowered.push(lower_expr(
                    arg,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                )?);
            }
            lower_builtin_call_orc(ctx, *func, &lowered)
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if name == "__omni_buffer_read2" {
                return lower_orc_buffer_read2_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                );
            }
            if name == "__omni_buffer_write2" {
                return lower_orc_buffer_write2_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                    true,
                );
            }
            if let Some(base) = parse_array_len_instance_base(name) {
                if is_orc_builtin_data_len_receiver(ctx, base, local_array_aliases) {
                    return lower_orc_data_len_call(name, base, args, ctx, local_array_aliases);
                }
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                if is_orc_builtin_buffer_chans_receiver(ctx, base) {
                    return lower_orc_buffer_chans_call(name, base, args, ctx);
                }
            }
            if let Some(base) = parse_unsafe_read_instance_base(name) {
                if is_orc_builtin_unsafe_data_receiver(ctx, base, local_array_aliases) {
                    let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                    method_args.push(CallArg {
                        name: None,
                        expr: Expr::Var(base.to_owned()),
                    });
                    method_args.extend(args.iter().cloned());
                    return lower_orc_unsafe_data_read_call(
                        &method_args,
                        ctx,
                        locals,
                        local_aliases,
                        local_array_aliases,
                    );
                }
            }
            if let Some(base) = parse_unsafe_write_instance_base(name) {
                if is_orc_builtin_unsafe_data_receiver(ctx, base, local_array_aliases) {
                    let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                    method_args.push(CallArg {
                        name: None,
                        expr: Expr::Var(base.to_owned()),
                    });
                    method_args.extend(args.iter().cloned());
                    return lower_orc_unsafe_data_write_call(
                        &method_args,
                        ctx,
                        locals,
                        local_aliases,
                        local_array_aliases,
                    );
                }
            }
            if name == "unsafe_read" {
                return lower_orc_unsafe_data_read_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                );
            }
            if name == "unsafe_write" {
                return lower_orc_unsafe_data_write_call(
                    args,
                    ctx,
                    locals,
                    local_aliases,
                    local_array_aliases,
                );
            }
            if ctx.struct_fields.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "struct constructor '{name}(...)' used in scalar expression lowering"
                )));
            }
            let module = ctx.module;
            let context = ctx.context;
            let float_ty = ctx.float_ty;
            let sample_rate = ctx.sample_rate;
            let block_size = ctx.block_size;
            let fast_math = ctx.fast_math_flags != LLVMFastMathNone;
            let struct_fields = ctx.struct_fields;
            let user_fn_param_names = ctx.user_fn_param_names;
            let user_fn_param_defaults = ctx.user_fn_param_defaults;
            let user_fn_param_kinds = ctx.user_fn_param_kinds;
            let user_fn_param_by_ref = ctx.user_fn_param_by_ref;
            let user_registry = ctx.user_registry as *mut UserFnRegistry;
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            let mut lower_scalar_expr = |arg_expr: &Expr| unsafe {
                lower_expr(
                    arg_expr,
                    &mut *ctx_ptr,
                    locals,
                    local_aliases,
                    local_array_aliases,
                )
            };
            let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_buffer_arg_signature_in_orc(
                    &*ctx_ptr,
                    local_array_aliases,
                    arg_expr,
                    callee_name,
                )
            };
            let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_array_arg_signature_in_orc(
                    &*ctx_ptr,
                    local_array_aliases,
                    arg_expr,
                    callee_name,
                )
            };
            let prepared = prepare_user_call_common(
                name,
                type_args,
                args,
                module,
                context,
                float_ty,
                sample_rate,
                block_size,
                fast_math,
                struct_fields,
                user_fn_param_names,
                user_fn_param_defaults,
                user_fn_param_kinds,
                user_fn_param_by_ref,
                user_registry,
                &mut lower_scalar_expr,
                &mut infer_buffer_arg_signature,
                &mut infer_array_arg_signature,
                "ORC expression lowering",
            )?;

            let mut arg_values = Vec::new();
            let ctx_ptr: *mut LoweringCtx<'_> = ctx;
            let mut cast_scalar_arg =
                |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
                    cast_orc_value_to(&*ctx_ptr, value, target_ty, arg_name)
                };
            let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                        arg_expr: &Expr,
                                        struct_name: &str,
                                        by_ref: bool| {
                unsafe {
                    lower_struct_call_args_in_orc(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                        by_ref,
                    )
                }
            };
            let mut lower_array_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
                lower_array_call_args_in_orc(
                    &mut *ctx_ptr,
                    local_array_aliases,
                    arg_values,
                    arg_expr,
                    name,
                )
            };
            let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
                lower_buffer_call_args_in_orc(
                    &mut *ctx_ptr,
                    local_array_aliases,
                    arg_values,
                    arg_expr,
                    name,
                )
            };
            materialize_user_call_args_common(
                name,
                &prepared,
                &mut arg_values,
                b"call_arg\0",
                "ORC expression lowering",
                &mut cast_scalar_arg,
                &mut lower_struct_arg,
                &mut lower_array_arg,
                &mut lower_buffer_arg,
            )?;
            let call = LLVMBuildCall2(
                ctx.builder,
                prepared.fn_ty,
                prepared.fn_ref,
                arg_values.as_mut_ptr(),
                arg_values.len() as u32,
                b"call\0".as_ptr().cast(),
            );
            set_fast_math_for_primitive(call, prepared.ret_ty, ctx.fast_math_flags);
            Ok(OrcValue {
                value: call,
                ty: prepared.ret_ty,
            })
        }
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => unreachable!(),
    }
}
