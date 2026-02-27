use super::*;

pub(super) unsafe fn llvm_const_from_typed_f64(
    context: LLVMContextRef,
    float_ty: LLVMTypeRef,
    ty: PrimitiveType,
    value: f64,
) -> LLVMValueRef {
    match ty {
        PrimitiveType::F32 => LLVMConstReal(float_ty, value),
        PrimitiveType::F64 => {
            LLVMConstReal(llvm_ty_for_primitive(context, PrimitiveType::F64), value)
        }
        PrimitiveType::I32 => LLVMConstInt(
            llvm_ty_for_primitive(context, PrimitiveType::I32),
            (value as i32) as u64,
            1,
        ),
        PrimitiveType::I64 => LLVMConstInt(
            llvm_ty_for_primitive(context, PrimitiveType::I64),
            (value as i64) as u64,
            1,
        ),
        PrimitiveType::Bool => LLVMConstInt(
            llvm_ty_for_primitive(context, PrimitiveType::Bool),
            if value != 0.0 { 1 } else { 0 },
            0,
        ),
    }
}

pub(super) unsafe fn cast_value_to_common(
    value: OrcValue,
    to: PrimitiveType,
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    i32_ty: LLVMTypeRef,
    f32_ty: LLVMTypeRef,
    fast_math_flags: LLVMFastMathFlags,
    name: &[u8],
) -> LLVMValueRef {
    if value.ty == to {
        return value.value;
    }
    let from = value.ty;
    let i64_ty = LLVMInt64TypeInContext(context);
    let f64_ty = LLVMDoubleTypeInContext(context);

    match (from, to) {
        (PrimitiveType::F32, PrimitiveType::F64) => {
            LLVMBuildFPExt(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::F32) => {
            LLVMBuildFPTrunc(builder, value.value, f32_ty, name.as_ptr().cast())
        }

        (PrimitiveType::F32, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F32, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f32_ty, 0.0);
            build_fcmp_fast(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name,
                fast_math_flags,
            )
        }

        (PrimitiveType::F64, PrimitiveType::I32) => {
            LLVMBuildFPToSI(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::I64) => {
            LLVMBuildFPToSI(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::F64, PrimitiveType::Bool) => {
            let zero = LLVMConstReal(f64_ty, 0.0);
            build_fcmp_fast(
                builder,
                LLVMRealPredicate::LLVMRealONE,
                value.value,
                zero,
                name,
                fast_math_flags,
            )
        }

        (PrimitiveType::I32, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::I64) => {
            LLVMBuildSExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I32, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i32_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::I64, PrimitiveType::F32) => {
            LLVMBuildSIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::F64) => {
            LLVMBuildSIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::I32) => {
            LLVMBuildTrunc(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::I64, PrimitiveType::Bool) => {
            let zero = LLVMConstInt(i64_ty, 0, 0);
            LLVMBuildICmp(
                builder,
                LLVMIntPredicate::LLVMIntNE,
                value.value,
                zero,
                name.as_ptr().cast(),
            )
        }

        (PrimitiveType::Bool, PrimitiveType::F32) => {
            LLVMBuildUIToFP(builder, value.value, f32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::F64) => {
            LLVMBuildUIToFP(builder, value.value, f64_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I32) => {
            LLVMBuildZExt(builder, value.value, i32_ty, name.as_ptr().cast())
        }
        (PrimitiveType::Bool, PrimitiveType::I64) => {
            LLVMBuildZExt(builder, value.value, i64_ty, name.as_ptr().cast())
        }
        _ => value.value,
    }
}

pub(super) unsafe fn lower_literal_expr_common(
    expr: &Expr,
    context: LLVMContextRef,
    i32_ty: LLVMTypeRef,
    float_ty: LLVMTypeRef,
) -> Option<OrcValue> {
    match expr {
        Expr::Number(value) => Some(OrcValue {
            value: LLVMConstReal(float_ty, *value as f64),
            ty: PrimitiveType::F32,
        }),
        Expr::Int(value) => {
            if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 {
                Some(OrcValue {
                    value: const_i32(i32_ty, *value as i32),
                    ty: PrimitiveType::I32,
                })
            } else {
                let i64_ty = llvm_ty_for_primitive(context, PrimitiveType::I64);
                Some(OrcValue {
                    value: LLVMConstInt(i64_ty, *value as u64, 1),
                    ty: PrimitiveType::I64,
                })
            }
        }
        Expr::Bool(value) => {
            let bool_ty = llvm_ty_for_primitive(context, PrimitiveType::Bool);
            Some(OrcValue {
                value: LLVMConstInt(bool_ty, if *value { 1 } else { 0 }, 0),
                ty: PrimitiveType::Bool,
            })
        }
        _ => None,
    }
}

pub(super) struct PreparedUserCall<'a> {
    pub(super) param_names: &'a [String],
    pub(super) param_kinds: &'a [TypedFnParam],
    pub(super) param_by_ref: &'a [bool],
    pub(super) resolved: Vec<Option<&'a Expr>>,
    pub(super) scalar_values: Vec<OrcValue>,
    pub(super) scalar_types: Vec<PrimitiveType>,
    pub(super) fn_ref: LLVMValueRef,
    pub(super) fn_ty: LLVMTypeRef,
    pub(super) ret_ty: PrimitiveType,
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn prepare_user_call_common<'a>(
    name: &str,
    type_args: &[CallTypeArg],
    args: &'a [CallArg],
    module: LLVMModuleRef,
    context: LLVMContextRef,
    float_ty: LLVMTypeRef,
    sample_rate: f32,
    block_size: f32,
    fast_math: bool,
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    user_fn_param_names: &'a HashMap<String, Vec<String>>,
    user_fn_param_defaults: &'a HashMap<String, Vec<Option<Expr>>>,
    user_fn_param_kinds: &'a HashMap<String, Vec<TypedFnParam>>,
    user_fn_param_by_ref: &'a HashMap<String, Vec<bool>>,
    user_registry: *mut UserFnRegistry,
    lower_scalar_expr: &mut dyn FnMut(&Expr) -> Result<OrcValue, Diagnostic>,
    infer_buffer_arg_signature: &mut dyn FnMut(
        &Expr,
        &str,
    ) -> Result<
        (PrimitiveType, TypedBufferChannels),
        Diagnostic,
    >,
    call_context: &str,
) -> Result<PreparedUserCall<'a>, Diagnostic> {
    let param_names = user_fn_param_names
        .get(name)
        .ok_or_else(|| Diagnostic::internal(format!("missing parameter metadata for '{name}'")))?;
    let param_defaults = user_fn_param_defaults.get(name).ok_or_else(|| {
        Diagnostic::internal(format!("missing parameter default metadata for '{name}'"))
    })?;
    let param_kinds = user_fn_param_kinds
        .get(name)
        .ok_or_else(|| Diagnostic::internal(format!("missing param kind metadata for '{name}'")))?;
    let param_by_ref = user_fn_param_by_ref
        .get(name)
        .ok_or_else(|| Diagnostic::internal(format!("missing by-ref metadata for '{name}'")))?;
    if param_kinds.len() != param_names.len() || param_kinds.len() != param_defaults.len() {
        return Err(Diagnostic::internal(format!(
            "function '{name}' has inconsistent metadata sizes in {call_context}"
        )));
    }
    if param_by_ref.len() != param_kinds.len() {
        return Err(Diagnostic::internal(format!(
            "function '{name}' by-ref metadata length {} does not match param metadata {} in {call_context}",
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
        &format!("function '{name}' call in {call_context}"),
    )?;

    let mut scalar_values = Vec::new();
    let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
    for (idx, kind) in param_kinds.iter().enumerate() {
        let resolved_arg = resolved.get(idx).copied().flatten();
        match kind {
            TypedFnParam::Scalar => {
                let value = if let Some(arg_expr) = resolved_arg {
                    lower_scalar_expr(arg_expr)?
                } else {
                    let default_expr = param_defaults
                        .get(idx)
                        .and_then(|d| d.as_ref())
                        .ok_or_else(|| {
                            Diagnostic::internal(format!(
                                "function '{name}' missing required argument '{}' in {call_context}",
                                param_names[idx]
                            ))
                        })?;
                    let default_value =
                        eval_const_default_expr(default_expr, sample_rate, block_size)?;
                    let default_ty = infer_const_default_expr_type(default_expr)?;
                    OrcValue {
                        value: llvm_const_from_typed_f64(
                            context,
                            float_ty,
                            default_ty,
                            default_value,
                        ),
                        ty: default_ty,
                    }
                };
                scalar_values.push(value);
            }
            TypedFnParam::Buffer { elem_ty, channels } => {
                let resolved_ty = if let Some(arg_expr) = resolved_arg {
                    infer_buffer_arg_signature(arg_expr, name)?
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
        resolve_explicit_call_type_args_for_codegen(name, call_context, type_args)?;
    apply_explicit_generic_type_args_for_call(
        &*(user_registry as *const UserFnRegistry),
        name,
        &explicit_type_args,
        &mut scalar_types,
        call_context,
    )?;

    let (fn_ref, fn_ty, ret_ty) = ensure_user_fn_specialization(
        module,
        context,
        &mut *user_registry,
        struct_fields,
        sample_rate,
        block_size as usize,
        fast_math,
        name,
        &scalar_types,
        &buffer_types,
        &explicit_type_args,
    )?;

    Ok(PreparedUserCall {
        param_names,
        param_kinds,
        param_by_ref,
        resolved,
        scalar_values,
        scalar_types,
        fn_ref,
        fn_ty,
        ret_ty,
    })
}

pub(super) unsafe fn lower_binary_numeric_common(
    op: BinaryOp,
    left: OrcValue,
    right: OrcValue,
    builder: LLVMBuilderRef,
    fast_math_flags: LLVMFastMathFlags,
    cast_value: &mut dyn FnMut(OrcValue, PrimitiveType, &[u8]) -> LLVMValueRef,
    context: &str,
) -> Result<OrcValue, Diagnostic> {
    let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
        return Err(Diagnostic::internal(format!(
            "binary op requires numeric operands in {context}, got {:?} and {:?}",
            left.ty, right.ty
        )));
    };
    let left_v = cast_value(left, result_ty, b"bin_lhs_cast\0");
    let right_v = cast_value(right, result_ty, b"bin_rhs_cast\0");
    let value = match result_ty {
        PrimitiveType::F32 | PrimitiveType::F64 => match op {
            BinaryOp::Add => {
                build_fadd_fast(builder, left_v, right_v, b"bin_fadd\0", fast_math_flags)
            }
            BinaryOp::Sub => {
                build_fsub_fast(builder, left_v, right_v, b"bin_fsub\0", fast_math_flags)
            }
            BinaryOp::Mul => {
                build_fmul_fast(builder, left_v, right_v, b"bin_fmul\0", fast_math_flags)
            }
            BinaryOp::Div => {
                build_fdiv_fast(builder, left_v, right_v, b"bin_fdiv\0", fast_math_flags)
            }
            BinaryOp::Mod => {
                build_frem_fast(builder, left_v, right_v, b"bin_frem\0", fast_math_flags)
            }
        },
        PrimitiveType::I32 | PrimitiveType::I64 => match op {
            BinaryOp::Add => LLVMBuildAdd(builder, left_v, right_v, b"bin_iadd\0".as_ptr().cast()),
            BinaryOp::Sub => LLVMBuildSub(builder, left_v, right_v, b"bin_isub\0".as_ptr().cast()),
            BinaryOp::Mul => LLVMBuildMul(builder, left_v, right_v, b"bin_imul\0".as_ptr().cast()),
            BinaryOp::Div => LLVMBuildSDiv(builder, left_v, right_v, b"bin_idiv\0".as_ptr().cast()),
            BinaryOp::Mod => LLVMBuildSRem(builder, left_v, right_v, b"bin_irem\0".as_ptr().cast()),
        },
        PrimitiveType::Bool => {
            return Err(Diagnostic::internal(format!(
                "binary op does not support bool operands in {context}"
            )));
        }
    };
    Ok(OrcValue {
        value,
        ty: result_ty,
    })
}

pub(super) unsafe fn lower_compare_common(
    op: CmpOp,
    left: OrcValue,
    right: OrcValue,
    builder: LLVMBuilderRef,
    fast_math_flags: LLVMFastMathFlags,
    cast_value: &mut dyn FnMut(OrcValue, PrimitiveType, &[u8]) -> LLVMValueRef,
    context: &str,
) -> Result<OrcValue, Diagnostic> {
    let cmp = if left.ty == PrimitiveType::Bool && right.ty == PrimitiveType::Bool {
        let pred = match op {
            CmpOp::Eq => LLVMIntPredicate::LLVMIntEQ,
            CmpOp::Ne => LLVMIntPredicate::LLVMIntNE,
            _ => {
                return Err(Diagnostic::internal(format!(
                    "bool comparisons only support == and != in {context}"
                )));
            }
        };
        LLVMBuildICmp(
            builder,
            pred,
            left.value,
            right.value,
            b"cmp_bool\0".as_ptr().cast(),
        )
    } else {
        let Some(result_ty) = merge_numeric_primitive(left.ty, right.ty) else {
            return Err(Diagnostic::internal(format!(
                "comparison requires compatible operands in {context}, got {:?} and {:?}",
                left.ty, right.ty
            )));
        };
        let left_v = cast_value(left, result_ty, b"cmp_lhs_cast\0");
        let right_v = cast_value(right, result_ty, b"cmp_rhs_cast\0");
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
                build_fcmp_fast(builder, pred, left_v, right_v, b"cmp_f\0", fast_math_flags)
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
                LLVMBuildICmp(builder, pred, left_v, right_v, b"cmp_i\0".as_ptr().cast())
            }
            PrimitiveType::Bool => unreachable!(),
        }
    };
    Ok(OrcValue {
        value: cmp,
        ty: PrimitiveType::Bool,
    })
}

pub(super) unsafe fn lower_unary_not_common(
    value: OrcValue,
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    cast_value: &mut dyn FnMut(OrcValue, PrimitiveType, &[u8]) -> LLVMValueRef,
) -> OrcValue {
    let as_bool = cast_value(value, PrimitiveType::Bool, b"not_bool\0");
    let one = LLVMConstInt(llvm_ty_for_primitive(context, PrimitiveType::Bool), 1, 0);
    let not_v = LLVMBuildXor(builder, as_bool, one, b"not\0".as_ptr().cast());
    OrcValue {
        value: not_v,
        ty: PrimitiveType::Bool,
    }
}

pub(super) unsafe fn lower_logical_short_circuit_common<F>(
    op: LogicalOp,
    builder: LLVMBuilderRef,
    context: LLVMContextRef,
    fn_ref: LLVMValueRef,
    lhs_bool: LLVMValueRef,
    rhs_bb_name: &[u8],
    merge_bb_name: &[u8],
    phi_name: &[u8],
    diag_context: &str,
    mut lower_rhs_bool: F,
) -> Result<OrcValue, Diagnostic>
where
    F: FnMut() -> Result<LLVMValueRef, Diagnostic>,
{
    let pre_bb = LLVMGetInsertBlock(builder);
    if pre_bb.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to get insertion block for {diag_context}"
        )));
    }

    let rhs_bb = LLVMAppendBasicBlockInContext(context, fn_ref, rhs_bb_name.as_ptr().cast());
    let merge_bb = LLVMAppendBasicBlockInContext(context, fn_ref, merge_bb_name.as_ptr().cast());

    match op {
        LogicalOp::And => LLVMBuildCondBr(builder, lhs_bool, rhs_bb, merge_bb),
        LogicalOp::Or => LLVMBuildCondBr(builder, lhs_bool, merge_bb, rhs_bb),
    };

    LLVMPositionBuilderAtEnd(builder, rhs_bb);
    let rhs_bool = lower_rhs_bool()?;
    LLVMBuildBr(builder, merge_bb);
    let rhs_end_bb = LLVMGetInsertBlock(builder);
    if rhs_end_bb.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to get rhs block for {diag_context}"
        )));
    }

    LLVMPositionBuilderAtEnd(builder, merge_bb);
    let bool_ty = llvm_ty_for_primitive(context, PrimitiveType::Bool);
    let phi = LLVMBuildPhi(builder, bool_ty, phi_name.as_ptr().cast());
    let lhs_short = match op {
        LogicalOp::And => LLVMConstInt(bool_ty, 0, 0),
        LogicalOp::Or => LLVMConstInt(bool_ty, 1, 0),
    };
    let mut incoming_vals = [lhs_short, rhs_bool];
    let mut incoming_blocks = [pre_bb, rhs_end_bb];
    LLVMAddIncoming(
        phi,
        incoming_vals.as_mut_ptr(),
        incoming_blocks.as_mut_ptr(),
        incoming_vals.len() as u32,
    );
    Ok(OrcValue {
        value: phi,
        ty: PrimitiveType::Bool,
    })
}
