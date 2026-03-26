use super::struct_helpers::lower_struct_array_call_args_in_def;
use super::*;

pub(super) unsafe fn lower_def_expr(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    if let Some(literal) = lower_literal_expr_common(expr, ctx.context, ctx.i32_ty, ctx.float_ty) {
        return Ok(literal);
    }
    match expr {
        Expr::ArrayLiteral { .. } | Expr::Tuple { .. } => Err(Diagnostic::internal(
            "array/tuple literal is not a scalar expression in def lowering",
        )),
        Expr::Var { name, .. } => {
            if let Some((ty, value)) =
                builtin_constant_value_and_type(name, ctx.sample_rate, ctx.block_size)
            {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_f64(ctx.context, ctx.float_ty, ty, value),
                    ty,
                });
            }
            if ctx.buffer_params.contains_key(name) {
                return Err(Diagnostic::internal(format!(
                    "buffer symbol '{name}' must be indexed in def lowering"
                )));
            }
            let local = ctx.local_slots.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("unknown local '{name}' in def lowering"))
            })?;
            let loaded = LLVMBuildLoad2(
                ctx.builder,
                llvm_ty_for_primitive(ctx.context, local.ty),
                local.ptr,
                b"def_load\0".as_ptr().cast(),
            );
            Ok(OrcValue {
                value: loaded,
                ty: local.ty,
            })
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let builder = ctx.builder;
            let fast_math_flags = ctx.fast_math_flags;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_def_value_to(ctx, value, to, name)
            };
            lower_binary_numeric_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                "def lowering",
            )
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let builder = ctx.builder;
            let fast_math_flags = ctx.fast_math_flags;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_def_value_to(ctx, value, to, name)
            };
            lower_compare_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                "def lowering",
            )
        }
        Expr::Cast { to, expr, .. } => {
            // When casting a literal directly, emit it at target precision
            // to avoid f32 truncation of f64 constants (e.g. T(1.234...) where T=f64)
            // and f64-rounding of large i64 values (e.g. i64(2^53+1)).
            if let Expr::Number { value: n, .. } = expr.as_ref() {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_f64(ctx.context, ctx.float_ty, *to, *n),
                    ty: *to,
                });
            }
            if let Expr::Int { value: n, .. } = expr.as_ref() {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_i64(ctx.context, ctx.float_ty, *to, *n),
                    ty: *to,
                });
            }
            let value = lower_def_expr(expr, ctx)?;
            Ok(OrcValue {
                value: cast_def_value_to(ctx, value, *to, b"def_cast\0"),
                ty: *to,
            })
        }
        Expr::UnaryNot { expr, .. } => {
            let value = lower_def_expr(expr, ctx)?;
            let builder = ctx.builder;
            let context = ctx.context;
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| {
                cast_def_value_to(ctx, value, to, name)
            };
            Ok(lower_unary_not_common(
                value,
                builder,
                context,
                &mut cast_value,
            ))
        }
        Expr::UnaryBitNot { expr, .. } => {
            let value = lower_def_expr(expr, ctx)?;
            match value.ty {
                PrimitiveType::I32 | PrimitiveType::I64 => Ok(OrcValue {
                    value: LLVMBuildNot(ctx.builder, value.value, b"def_bitnot\0".as_ptr().cast()),
                    ty: value.ty,
                }),
                _ => Err(Diagnostic::internal(format!(
                    "bitwise not requires integer operand in def lowering, got {:?}",
                    value.ty
                ))),
            }
        }
        Expr::Logical { op, lhs, rhs, .. } => lower_def_logical_expr(*op, lhs, rhs, ctx),
        Expr::Call { func, args, .. } => {
            let mut lowered = Vec::with_capacity(args.len());
            for arg in args {
                lowered.push(lower_def_expr(arg, ctx)?);
            }
            lower_builtin_call_def(ctx, *func, &lowered)
        }
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if name == "__omni_buffer_read2" {
                return lower_def_buffer_read2_call(args, ctx, true);
            }
            if name == "__omni_buffer_write2" {
                return lower_def_buffer_write2_call(args, ctx, true);
            }
            if let Some(base) = parse_array_len_instance_base(name) {
                if is_def_builtin_data_len_receiver(ctx, base) {
                    return lower_def_data_len_call(name, base, args, ctx);
                }
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                if is_def_builtin_buffer_chans_receiver(ctx, base) {
                    return lower_def_buffer_chans_call(name, base, args, ctx);
                }
            }
            if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                if is_def_builtin_buffer_chans_receiver(ctx, base) {
                    return lower_def_buffer_samplerate_call(name, base, args, ctx);
                }
            }
            if let Some(base) = parse_unsafe_read_instance_base(name) {
                if is_def_builtin_unsafe_data_receiver(ctx, base) {
                    let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                    method_args.push(CallArg {
                        name: None,
                        expr: Expr::var(base.to_owned()),
                    });
                    method_args.extend(args.iter().cloned());
                    return lower_def_unsafe_data_read_call(&method_args, ctx);
                }
            }
            if let Some(base) = parse_unsafe_write_instance_base(name) {
                if is_def_builtin_unsafe_data_receiver(ctx, base) {
                    let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                    method_args.push(CallArg {
                        name: None,
                        expr: Expr::var(base.to_owned()),
                    });
                    method_args.extend(args.iter().cloned());
                    return lower_def_unsafe_data_write_call(&method_args, ctx);
                }
            }
            if name == "unsafe_read" {
                return lower_def_unsafe_data_read_call(args, ctx);
            }
            if name == "unsafe_write" {
                return lower_def_unsafe_data_write_call(args, ctx);
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
            let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
            let mut lower_scalar_expr =
                |arg_expr: &Expr| unsafe { lower_def_expr(arg_expr, &mut *ctx_ptr) };
            let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_buffer_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
            };
            let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
                infer_array_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
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
                "def lowering",
            )?;

            let mut arg_values = Vec::new();
            let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
            let mut cast_scalar_arg =
                |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
                    cast_def_value_to(&*ctx_ptr, value, target_ty, arg_name)
                };
            let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                        arg_expr: &Expr,
                                        struct_name: &str,
                                        by_ref: bool| {
                unsafe {
                    lower_struct_call_args_in_def(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                        by_ref,
                    )
                }
            };
            let mut lower_struct_array_arg =
                |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr, struct_name: &str| unsafe {
                    lower_struct_array_call_args_in_def(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        struct_name,
                        name,
                    )
                };
            let mut lower_array_arg =
                |arg_values: &mut Vec<LLVMValueRef>,
                 arg_expr: &Expr,
                 expected_elem_ty: Option<PrimitiveType>| unsafe {
                    lower_array_call_args_in_def(
                        &mut *ctx_ptr,
                        arg_values,
                        arg_expr,
                        name,
                        expected_elem_ty,
                    )
                };
            let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
                lower_buffer_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, name)
            };
            materialize_user_call_args_common(
                name,
                &prepared,
                &mut arg_values,
                b"def_call_arg\0",
                "def lowering",
                &mut cast_scalar_arg,
                &mut lower_struct_arg,
                &mut lower_struct_array_arg,
                &mut lower_array_arg,
                &mut lower_buffer_arg,
            )?;
            let call = LLVMBuildCall2(
                ctx.builder,
                prepared.fn_ty,
                prepared.fn_ref,
                arg_values.as_mut_ptr(),
                arg_values.len() as u32,
                b"def_call\0".as_ptr().cast(),
            );
            match &prepared.ret_ty {
                ReturnType::Scalar(scalar_ty) => {
                    set_fast_math_for_primitive(call, *scalar_ty, ctx.fast_math_flags);
                    Ok(OrcValue {
                        value: call,
                        ty: *scalar_ty,
                    })
                }
                ReturnType::Tuple(_) => {
                    // Tuple-returning calls are not scalar expressions.
                    // They must be consumed by destructuring assignment or
                    // tuple variable binding, handled in stmt_lowering.
                    Err(Diagnostic::internal(format!(
                        "tuple-returning call to '{}' is not a scalar expression in def lowering",
                        name
                    )))
                }
            }
        }
        Expr::Index { base, index, .. } => {
            // Check if base is a tuple variable — use extractvalue
            if let Some(tuple_slot) = ctx.tuple_slots.get(base).cloned() {
                let const_idx = match index.as_ref() {
                    Expr::Int { value, .. } => *value as usize,
                    _ => {
                        return Err(Diagnostic::internal(
                            "tuple element access requires a compile-time constant index",
                        ));
                    }
                };
                if const_idx >= tuple_slot.elem_tys.len() {
                    return Err(Diagnostic::internal(format!(
                        "tuple index {} out of bounds for tuple of length {}",
                        const_idx,
                        tuple_slot.elem_tys.len()
                    )));
                }
                let mut llvm_elem_tys: Vec<LLVMTypeRef> = tuple_slot
                    .elem_tys
                    .iter()
                    .map(|t| llvm_ty_for_primitive(ctx.context, *t))
                    .collect();
                let struct_ty = LLVMStructTypeInContext(
                    ctx.context,
                    llvm_elem_tys.as_mut_ptr(),
                    llvm_elem_tys.len() as u32,
                    0,
                );
                let loaded = LLVMBuildLoad2(
                    ctx.builder,
                    struct_ty,
                    tuple_slot.ptr,
                    b"tup_load\0".as_ptr().cast(),
                );
                let elem_val = LLVMBuildExtractValue(
                    ctx.builder,
                    loaded,
                    const_idx as u32,
                    b"tup_idx\0".as_ptr().cast(),
                );
                return Ok(OrcValue {
                    value: elem_val,
                    ty: tuple_slot.elem_tys[const_idx],
                });
            }
            let data = lower_def_data_element_ptr(ctx, base, index, true)?;
            Ok(OrcValue {
                value: LLVMBuildLoad2(
                    ctx.builder,
                    llvm_ty_for_primitive(ctx.context, data.elem_ty),
                    data.ptr,
                    b"def_data_load\0".as_ptr().cast(),
                ),
                ty: data.elem_ty,
            })
        }
        Expr::Slice { .. } => Err(Diagnostic::internal(
            "slice expressions are not yet supported in def scalar lowering",
        )),
        Expr::ArrayCtor { .. } => Err(Diagnostic::internal(
            "array constructor is not supported in def lowering",
        )),
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => unreachable!(),
    }
}
