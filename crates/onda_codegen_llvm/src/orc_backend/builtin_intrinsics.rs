use super::*;

pub(super) fn builtin_arity(func: BuiltinFn) -> usize {
    func.arity()
}

pub(super) fn builtin_intrinsic_name(func: BuiltinFn, use_f64: bool) -> &'static str {
    match func {
        BuiltinFn::Sin => {
            if use_f64 {
                "llvm.sin.f64"
            } else {
                "llvm.sin.f32"
            }
        }
        BuiltinFn::Cos => {
            if use_f64 {
                "llvm.cos.f64"
            } else {
                "llvm.cos.f32"
            }
        }
        BuiltinFn::Tan => {
            if use_f64 {
                "tan"
            } else {
                "tanf"
            }
        }
        BuiltinFn::Tanh => {
            if use_f64 {
                "tanh"
            } else {
                "tanhf"
            }
        }
        BuiltinFn::Atan => {
            if use_f64 {
                "atan"
            } else {
                "atanf"
            }
        }
        BuiltinFn::Atan2 => {
            if use_f64 {
                "atan2"
            } else {
                "atan2f"
            }
        }
        BuiltinFn::Exp => {
            if use_f64 {
                "llvm.exp.f64"
            } else {
                "llvm.exp.f32"
            }
        }
        BuiltinFn::Log => {
            if use_f64 {
                "llvm.log.f64"
            } else {
                "llvm.log.f32"
            }
        }
        BuiltinFn::Sqrt => {
            if use_f64 {
                "llvm.sqrt.f64"
            } else {
                "llvm.sqrt.f32"
            }
        }
        BuiltinFn::Pow => {
            if use_f64 {
                "llvm.pow.f64"
            } else {
                "llvm.pow.f32"
            }
        }
        BuiltinFn::Abs => {
            if use_f64 {
                "llvm.fabs.f64"
            } else {
                "llvm.fabs.f32"
            }
        }
        BuiltinFn::Floor => {
            if use_f64 {
                "llvm.floor.f64"
            } else {
                "llvm.floor.f32"
            }
        }
        BuiltinFn::Ceil => {
            if use_f64 {
                "llvm.ceil.f64"
            } else {
                "llvm.ceil.f32"
            }
        }
        BuiltinFn::Round => {
            if use_f64 {
                "llvm.round.f64"
            } else {
                "llvm.round.f32"
            }
        }
        BuiltinFn::Trunc => {
            if use_f64 {
                "llvm.trunc.f64"
            } else {
                "llvm.trunc.f32"
            }
        }
        BuiltinFn::Min => {
            if use_f64 {
                "llvm.minimum.f64"
            } else {
                "llvm.minimum.f32"
            }
        }
        BuiltinFn::Max => {
            if use_f64 {
                "llvm.maximum.f64"
            } else {
                "llvm.maximum.f32"
            }
        }
        BuiltinFn::Fma => {
            if use_f64 {
                "llvm.fma.f64"
            } else {
                "llvm.fma.f32"
            }
        }
    }
}

pub(super) unsafe fn build_unary_f32_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_unary_f64_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_binary_f32_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_binary_f64_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_ternary_f32_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMFloatTypeInContext(context);
    let mut params = [float_ty, float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_ternary_f64_fn_type(
    context: LLVMContextRef,
) -> Result<LLVMTypeRef, Diagnostic> {
    let float_ty = LLVMDoubleTypeInContext(context);
    let mut params = [float_ty, float_ty, float_ty];
    Ok(LLVMFunctionType(
        float_ty,
        params.as_mut_ptr(),
        params.len() as u32,
        0,
    ))
}

pub(super) unsafe fn build_builtin_fn_type(
    context: LLVMContextRef,
    use_f64: bool,
    arity: usize,
) -> Result<LLVMTypeRef, Diagnostic> {
    match (use_f64, arity) {
        (false, 1) => build_unary_f32_fn_type(context),
        (true, 1) => build_unary_f64_fn_type(context),
        (false, 2) => build_binary_f32_fn_type(context),
        (true, 2) => build_binary_f64_fn_type(context),
        (false, 3) => build_ternary_f32_fn_type(context),
        (true, 3) => build_ternary_f64_fn_type(context),
        _ => Err(Diagnostic::internal(format!(
            "unsupported builtin intrinsic arity {arity}"
        ))),
    }
}

pub(super) unsafe fn lower_builtin_call_def(
    ctx: &mut DefLoweringCtx<'_>,
    func: BuiltinFn,
    args: &[OrcValue],
) -> Result<OrcValue, Diagnostic> {
    let expected_arity = builtin_arity(func);
    if args.len() != expected_arity {
        return Err(Diagnostic::internal(format!(
            "builtin intrinsic call has wrong arity: expected {expected_arity}, got {}",
            args.len()
        )));
    }

    if func == BuiltinFn::Abs && matches!(args[0].ty, PrimitiveType::I32 | PrimitiveType::I64) {
        let result_ty = args[0].ty;
        let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
        let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
        let mut params = [int_ty, bool_ty];
        let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
        let fn_name = if result_ty == PrimitiveType::I32 {
            "llvm.abs.i32"
        } else {
            "llvm.abs.i64"
        };
        let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
        let arg0 = cast_def_value_to(ctx, args[0], result_ty, b"def_abs_int_arg\0");
        let no_poison = LLVMConstInt(bool_ty, 0, 0);
        let mut lowered_args = [arg0, no_poison];
        return Ok(OrcValue {
            value: LLVMBuildCall2(
                ctx.builder,
                fn_ty,
                fn_ref,
                lowered_args.as_mut_ptr(),
                lowered_args.len() as u32,
                b"def_abs_int\0".as_ptr().cast(),
            ),
            ty: result_ty,
        });
    }

    if matches!(func, BuiltinFn::Min | BuiltinFn::Max) {
        if let Some(result_ty) = merge_numeric_primitive(args[0].ty, args[1].ty) {
            if matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64) {
                let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
                let mut params = [int_ty, int_ty];
                let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
                let fn_name = match (func, result_ty) {
                    (BuiltinFn::Min, PrimitiveType::I32) => "llvm.smin.i32",
                    (BuiltinFn::Min, PrimitiveType::I64) => "llvm.smin.i64",
                    (BuiltinFn::Max, PrimitiveType::I32) => "llvm.smax.i32",
                    (BuiltinFn::Max, PrimitiveType::I64) => "llvm.smax.i64",
                    _ => unreachable!("min/max integer lowering must use i32/i64"),
                };
                let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
                let arg0 = cast_def_value_to(ctx, args[0], result_ty, b"def_minmax_int_l\0");
                let arg1 = cast_def_value_to(ctx, args[1], result_ty, b"def_minmax_int_r\0");
                let mut lowered_args = [arg0, arg1];
                return Ok(OrcValue {
                    value: LLVMBuildCall2(
                        ctx.builder,
                        fn_ty,
                        fn_ref,
                        lowered_args.as_mut_ptr(),
                        lowered_args.len() as u32,
                        b"def_minmax_int\0".as_ptr().cast(),
                    ),
                    ty: result_ty,
                });
            }
        }
    }

    let use_f64 = args.iter().any(|v| v.ty == PrimitiveType::F64);
    let result_ty = if use_f64 {
        PrimitiveType::F64
    } else {
        PrimitiveType::F32
    };
    let fn_ty = build_builtin_fn_type(ctx.context, use_f64, expected_arity)?;
    let fn_ref = ensure_named_fn(ctx.module, builtin_intrinsic_name(func, use_f64), fn_ty)?;
    let mut lowered_args = Vec::with_capacity(args.len());
    for arg in args {
        lowered_args.push(cast_def_value_to(
            ctx,
            *arg,
            result_ty,
            b"def_builtin_arg\0",
        ));
    }
    let call = LLVMBuildCall2(
        ctx.builder,
        fn_ty,
        fn_ref,
        lowered_args.as_mut_ptr(),
        lowered_args.len() as u32,
        b"def_builtin_call\0".as_ptr().cast(),
    );
    set_fast_math_for_primitive(call, result_ty, ctx.fast_math_flags);
    Ok(OrcValue {
        value: call,
        ty: result_ty,
    })
}

pub(super) unsafe fn lower_builtin_call_orc(
    ctx: &mut LoweringCtx<'_>,
    func: BuiltinFn,
    args: &[OrcValue],
) -> Result<OrcValue, Diagnostic> {
    let expected_arity = builtin_arity(func);
    if args.len() != expected_arity {
        return Err(Diagnostic::internal(format!(
            "builtin intrinsic call has wrong arity: expected {expected_arity}, got {}",
            args.len()
        )));
    }

    if func == BuiltinFn::Abs && matches!(args[0].ty, PrimitiveType::I32 | PrimitiveType::I64) {
        let result_ty = args[0].ty;
        let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
        let bool_ty = llvm_ty_for_primitive(ctx.context, PrimitiveType::Bool);
        let mut params = [int_ty, bool_ty];
        let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
        let fn_name = if result_ty == PrimitiveType::I32 {
            "llvm.abs.i32"
        } else {
            "llvm.abs.i64"
        };
        let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
        let arg0 = cast_orc_value_to(ctx, args[0], result_ty, b"abs_int_arg\0");
        let no_poison = LLVMConstInt(bool_ty, 0, 0);
        let mut lowered_args = [arg0, no_poison];
        return Ok(OrcValue {
            value: LLVMBuildCall2(
                ctx.builder,
                fn_ty,
                fn_ref,
                lowered_args.as_mut_ptr(),
                lowered_args.len() as u32,
                b"abs_int\0".as_ptr().cast(),
            ),
            ty: result_ty,
        });
    }

    if matches!(func, BuiltinFn::Min | BuiltinFn::Max) {
        if let Some(result_ty) = merge_numeric_primitive(args[0].ty, args[1].ty) {
            if matches!(result_ty, PrimitiveType::I32 | PrimitiveType::I64) {
                let int_ty = llvm_ty_for_primitive(ctx.context, result_ty);
                let mut params = [int_ty, int_ty];
                let fn_ty = LLVMFunctionType(int_ty, params.as_mut_ptr(), params.len() as u32, 0);
                let fn_name = match (func, result_ty) {
                    (BuiltinFn::Min, PrimitiveType::I32) => "llvm.smin.i32",
                    (BuiltinFn::Min, PrimitiveType::I64) => "llvm.smin.i64",
                    (BuiltinFn::Max, PrimitiveType::I32) => "llvm.smax.i32",
                    (BuiltinFn::Max, PrimitiveType::I64) => "llvm.smax.i64",
                    _ => unreachable!("min/max integer lowering must use i32/i64"),
                };
                let fn_ref = ensure_named_fn(ctx.module, fn_name, fn_ty)?;
                let arg0 = cast_orc_value_to(ctx, args[0], result_ty, b"minmax_int_l\0");
                let arg1 = cast_orc_value_to(ctx, args[1], result_ty, b"minmax_int_r\0");
                let mut lowered_args = [arg0, arg1];
                return Ok(OrcValue {
                    value: LLVMBuildCall2(
                        ctx.builder,
                        fn_ty,
                        fn_ref,
                        lowered_args.as_mut_ptr(),
                        lowered_args.len() as u32,
                        b"minmax_int\0".as_ptr().cast(),
                    ),
                    ty: result_ty,
                });
            }
        }
    }

    let use_f64 = args.iter().any(|v| v.ty == PrimitiveType::F64);
    let result_ty = if use_f64 {
        PrimitiveType::F64
    } else {
        PrimitiveType::F32
    };
    let fn_ty = build_builtin_fn_type(ctx.context, use_f64, expected_arity)?;
    let fn_ref = ensure_named_fn(ctx.module, builtin_intrinsic_name(func, use_f64), fn_ty)?;
    let mut lowered_args = Vec::with_capacity(args.len());
    for arg in args {
        lowered_args.push(cast_orc_value_to(ctx, *arg, result_ty, b"builtin_arg\0"));
    }
    let call = LLVMBuildCall2(
        ctx.builder,
        fn_ty,
        fn_ref,
        lowered_args.as_mut_ptr(),
        lowered_args.len() as u32,
        b"builtin_call\0".as_ptr().cast(),
    );
    set_fast_math_for_primitive(call, result_ty, ctx.fast_math_flags);
    Ok(OrcValue {
        value: call,
        ty: result_ty,
    })
}

pub(super) unsafe fn ensure_named_fn(
    module: LLVMModuleRef,
    name: &str,
    fn_ty: LLVMTypeRef,
) -> Result<LLVMValueRef, Diagnostic> {
    let cname = CString::new(name).map_err(|_| {
        Diagnostic::internal(format!("invalid function name '{name}' for ORC lowering"))
    })?;
    let existing = LLVMGetNamedFunction(module, cname.as_ptr());
    if !existing.is_null() {
        return Ok(existing);
    }

    let added = LLVMAddFunction(module, cname.as_ptr(), fn_ty);
    if added.is_null() {
        return Err(Diagnostic::internal(format!(
            "failed to add function '{name}' for ORC lowering"
        )));
    }
    Ok(added)
}
