use super::*;

pub(super) unsafe fn lower_def_expr(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    match expr {
        Expr::Number(value) => Ok(OrcValue {
            value: LLVMConstReal(ctx.float_ty, *value as f64),
            ty: PrimitiveType::F32,
        }),
        Expr::ArrayLiteral(_) => Err(Diagnostic::internal(
            "array literal is not a scalar expression in def lowering",
        )),
        Expr::Int(value) => {
            if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 {
                Ok(OrcValue {
                    value: const_i32(ctx.i32_ty, *value as i32),
                    ty: PrimitiveType::I32,
                })
            } else {
                let i64_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::I64);
                Ok(OrcValue {
                    value: LLVMConstInt(i64_ty, *value as u64, 1),
                    ty: PrimitiveType::I64,
                })
            }
        }
        Expr::Bool(value) => {
            let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
            Ok(OrcValue {
                value: LLVMConstInt(bool_ty, if *value { 1 } else { 0 }, 0),
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Var(name) => {
            if let Some(value) = builtin_constant_value(name, ctx.sample_rate, ctx.block_size) {
                return Ok(OrcValue {
                    value: LLVMConstReal(ctx.float_ty, value as f64),
                    ty: PrimitiveType::F32,
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
        Expr::Binary { op, lhs, rhs } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                return Err(Diagnostic::internal(format!(
                    "binary op requires numeric operands, got {:?} and {:?}",
                    left.ty, right.ty
                )));
            };
            let left_v = cast_def_value_to(ctx, left, result_ty, b"def_bin_lhs_cast\0");
            let right_v = cast_def_value_to(ctx, right, result_ty, b"def_bin_rhs_cast\0");
            let value = match result_ty {
                PrimitiveType::F32 | PrimitiveType::F64 => match op {
                    BinaryOp::Add => build_fadd_fast(
                        ctx.builder,
                        left_v,
                        right_v,
                        b"def_fadd\0",
                        ctx.fast_math_flags,
                    ),
                    BinaryOp::Sub => build_fsub_fast(
                        ctx.builder,
                        left_v,
                        right_v,
                        b"def_fsub\0",
                        ctx.fast_math_flags,
                    ),
                    BinaryOp::Mul => build_fmul_fast(
                        ctx.builder,
                        left_v,
                        right_v,
                        b"def_fmul\0",
                        ctx.fast_math_flags,
                    ),
                    BinaryOp::Div => build_fdiv_fast(
                        ctx.builder,
                        left_v,
                        right_v,
                        b"def_fdiv\0",
                        ctx.fast_math_flags,
                    ),
                    BinaryOp::Mod => build_frem_fast(
                        ctx.builder,
                        left_v,
                        right_v,
                        b"def_frem\0",
                        ctx.fast_math_flags,
                    ),
                },
                PrimitiveType::I32 | PrimitiveType::I64 => match op {
                    BinaryOp::Add => {
                        LLVMBuildAdd(ctx.builder, left_v, right_v, b"def_iadd\0".as_ptr().cast())
                    }
                    BinaryOp::Sub => {
                        LLVMBuildSub(ctx.builder, left_v, right_v, b"def_isub\0".as_ptr().cast())
                    }
                    BinaryOp::Mul => {
                        LLVMBuildMul(ctx.builder, left_v, right_v, b"def_imul\0".as_ptr().cast())
                    }
                    BinaryOp::Div => {
                        LLVMBuildSDiv(ctx.builder, left_v, right_v, b"def_idiv\0".as_ptr().cast())
                    }
                    BinaryOp::Mod => {
                        LLVMBuildSRem(ctx.builder, left_v, right_v, b"def_irem\0".as_ptr().cast())
                    }
                },
                PrimitiveType::Bool => {
                    return Err(Diagnostic::internal(
                        "binary op does not support bool operands in def lowering",
                    ));
                }
            };
            Ok(OrcValue {
                value,
                ty: result_ty,
            })
        }
        Expr::Compare { op, lhs, rhs } => {
            let left = lower_def_expr(lhs, ctx)?;
            let right = lower_def_expr(rhs, ctx)?;
            let cmp = if left.ty == PrimitiveType::Bool && right.ty == PrimitiveType::Bool {
                let pred = match op {
                    CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                    CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                    _ => {
                        return Err(Diagnostic::internal(
                            "bool comparisons only support == and != in def lowering",
                        ));
                    }
                };
                LLVMBuildICmp(
                    ctx.builder,
                    pred,
                    left.value,
                    right.value,
                    b"def_icmp_bool\0".as_ptr().cast(),
                )
            } else {
                let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
                    return Err(Diagnostic::internal(format!(
                        "comparison requires compatible operands, got {:?} and {:?}",
                        left.ty, right.ty
                    )));
                };
                let left_v = cast_def_value_to(ctx, left, result_ty, b"def_cmp_lhs_cast\0");
                let right_v = cast_def_value_to(ctx, right, result_ty, b"def_cmp_rhs_cast\0");
                match result_ty {
                    PrimitiveType::F32 | PrimitiveType::F64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMRealPredicate::LLVMRealOEQ,
                            CmpOp::Ne => LLVMRealPredicate::LLVMRealONE,
                            CmpOp::Lt => LLVMRealPredicate::LLVMRealOLT,
                            CmpOp::Le => LLVMRealPredicate::LLVMRealOLE,
                            CmpOp::Gt => LLVMRealPredicate::LLVMRealOGT,
                            CmpOp::Ge => LLVMRealPredicate::LLVMRealOGE,
                        };
                        build_fcmp_fast(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"def_fcmp\0",
                            ctx.fast_math_flags,
                        )
                    }
                    PrimitiveType::I32 | PrimitiveType::I64 => {
                        let pred = match op {
                            CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
                            CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
                            CmpOp::Lt => LLVMIntPredicate::LLVMIntSLT,
                            CmpOp::Le => LLVMIntPredicate::LLVMIntSLE,
                            CmpOp::Gt => LLVMIntPredicate::LLVMIntSGT,
                            CmpOp::Ge => LLVMIntPredicate::LLVMIntSGE,
                        };
                        LLVMBuildICmp(
                            ctx.builder,
                            pred,
                            left_v,
                            right_v,
                            b"def_icmp\0".as_ptr().cast(),
                        )
                    }
                    PrimitiveType::Bool => unreachable!(),
                }
            };
            Ok(OrcValue {
                value: cmp,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Cast { to, expr } => {
            let value = lower_def_expr(expr, ctx)?;
            Ok(OrcValue {
                value: cast_def_value_to(ctx, value, *to, b"def_cast\0"),
                ty: *to,
            })
        }
        Expr::UnaryNot { expr } => {
            let value = lower_def_expr(expr, ctx)?;
            let as_bool = cast_def_value_to(ctx, value, PrimitiveType::Bool, b"def_not_bool\0");
            let one = LLVMConstInt(
                llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool),
                1,
                0,
            );
            let not_v = LLVMBuildXor(ctx.builder, as_bool, one, b"def_not\0".as_ptr().cast());
            Ok(OrcValue {
                value: not_v,
                ty: PrimitiveType::Bool,
            })
        }
        Expr::Logical { op, lhs, rhs } => lower_def_logical_expr(*op, lhs, rhs, ctx),
        Expr::Call { func, args } => {
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
        } => {
            if name == "__omni_buffer_read2" {
                return lower_def_buffer_read2_call(args, ctx, true);
            }
            if name == "__omni_buffer_write2" {
                return lower_def_buffer_write2_call(args, ctx, true);
            }
            if let Some(base) = parse_data_len_instance_base(name) {
                return lower_def_data_len_call(name, base, args, ctx);
            }
            if let Some(base) = parse_buffer_chans_instance_base(name) {
                return lower_def_buffer_chans_call(name, base, args, ctx);
            }
            if name == "unsafe_read" {
                return lower_def_unsafe_data_read_call(args, ctx);
            }
            if name == "unsafe_write" {
                return lower_def_unsafe_data_write_call(args, ctx);
            }
            let param_names = ctx.user_fn_param_names.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter metadata for '{name}'"))
            })?;
            let param_defaults = ctx.user_fn_param_defaults.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing parameter default metadata for '{name}'"))
            })?;
            let param_kinds = ctx.user_fn_param_kinds.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing param kind metadata for '{name}'"))
            })?;
            let param_by_ref = ctx.user_fn_param_by_ref.get(name).ok_or_else(|| {
                Diagnostic::internal(format!("missing by-ref metadata for '{name}'"))
            })?;
            if param_kinds.len() != param_names.len() || param_kinds.len() != param_defaults.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' has inconsistent metadata sizes in def lowering"
                )));
            }
            if param_by_ref.len() != param_kinds.len() {
                return Err(Diagnostic::internal(format!(
                    "function '{name}' by-ref metadata length {} does not match param metadata {} in def lowering",
                    param_by_ref.len(),
                    param_kinds.len()
                )));
            }
            let forbid_self_named = param_names.first().map(String::as_str) == Some("self");
            let resolved = resolve_call_args_codegen(
                args,
                param_names,
                param_defaults,
                forbid_self_named,
                &format!("function '{name}' call in def lowering"),
            )?;

            let mut scalar_values = Vec::new();
            let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let value = if let Some(arg_expr) = resolved_arg {
                            lower_def_expr(arg_expr, ctx)?
                        } else {
                            let default_expr = param_defaults
                                .get(idx)
                                .and_then(|d| d.as_ref())
                                .ok_or_else(|| {
                                    Diagnostic::internal(format!(
                                        "function '{name}' missing required argument '{}' in def lowering",
                                        param_names[idx]
                                    ))
                                })?;
                            let default_value = eval_const_default_expr(
                                default_expr,
                                ctx.sample_rate,
                                ctx.block_size,
                            )?;
                            OrcValue {
                                value: LLVMConstReal(ctx.float_ty, default_value as f64),
                                ty: PrimitiveType::F32,
                            }
                        };
                        scalar_values.push(value);
                    }
                    TypedFnParam::Buffer { elem_ty, channels } => {
                        let resolved_ty = if let Some(arg_expr) = resolved_arg {
                            infer_buffer_arg_signature_in_def(ctx, arg_expr, name)?
                        } else {
                            (*elem_ty, channels.clone())
                        };
                        buffer_types.push(resolved_ty);
                    }
                    TypedFnParam::Struct { .. } => {}
                }
            }
            let mut scalar_types = scalar_values.iter().map(|v| v.ty).collect::<Vec<_>>();
            let explicit_type_args =
                resolve_explicit_call_type_args_for_codegen(name, "def lowering", type_args)?;
            apply_explicit_generic_type_args_for_call(
                &*(ctx.user_registry as *const UserFnRegistry),
                name,
                &explicit_type_args,
                &mut scalar_types,
                "def lowering",
            )?;

            let (fn_ref, fn_ty, ret_ty) = ensure_user_fn_specialization(
                ctx.module,
                ctx.context,
                &mut *(ctx.user_registry as *mut UserFnRegistry),
                ctx.struct_fields,
                ctx.sample_rate,
                ctx.block_size as usize,
                ctx.fast_math_flags != LLVMFastMathNone,
                name,
                &scalar_types,
                &buffer_types,
                &explicit_type_args,
            )?;

            let mut arg_values = Vec::new();
            let mut scalar_idx = 0usize;
            for (idx, kind) in param_kinds.iter().enumerate() {
                let resolved_arg = resolved.get(idx).copied().flatten();
                match kind {
                    TypedFnParam::Scalar => {
                        let target_ty = scalar_types[scalar_idx];
                        let value = cast_def_value_to(
                            ctx,
                            scalar_values[scalar_idx],
                            target_ty,
                            b"def_call_arg\0",
                        );
                        scalar_idx += 1;
                        arg_values.push(value);
                    }
                    TypedFnParam::Struct { struct_name } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required struct argument '{}' in def lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_struct_call_args_in_def(
                            ctx,
                            &mut arg_values,
                            arg_expr,
                            struct_name,
                            name,
                            param_by_ref[idx],
                        )?;
                    }
                    TypedFnParam::Buffer { .. } => {
                        let arg_expr = resolved_arg.ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required buffer argument '{}' in def lowering",
                                param_names[idx]
                            ))
                        })?;
                        lower_buffer_call_args_in_def(ctx, &mut arg_values, arg_expr, name)?;
                    }
                }
            }
            let call = LLVMBuildCall2(
                ctx.builder,
                fn_ty,
                fn_ref,
                arg_values.as_mut_ptr(),
                arg_values.len() as u32,
                b"def_call\0".as_ptr().cast(),
            );
            set_fast_math_for_primitive(call, ret_ty, ctx.fast_math_flags);
            Ok(OrcValue {
                value: call,
                ty: ret_ty,
            })
        }
        Expr::Index { base, index } => {
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
        Expr::DataCtor { .. } => Err(Diagnostic::internal(
            "Data constructor is not supported in def lowering",
        )),
    }
}
