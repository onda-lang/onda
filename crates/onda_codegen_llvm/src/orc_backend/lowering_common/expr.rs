use super::*;

pub(in crate::orc_backend) trait SharedScalarExprBackend {
    unsafe fn lower_expr(&mut self, expr: &Expr) -> Result<OrcValue, Diagnostic>;
    unsafe fn lower_var(&mut self, name: &str) -> Result<OrcValue, Diagnostic>;
    unsafe fn lower_index_expr(&mut self, base: &str, index: &Expr)
        -> Result<OrcValue, Diagnostic>;
    unsafe fn lower_user_call_expr(
        &mut self,
        name: &str,
        type_args: &[CallTypeArg],
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic>;
    unsafe fn cast_value_to(
        &mut self,
        value: OrcValue,
        to: PrimitiveType,
        name: &[u8],
    ) -> LLVMValueRef;
    unsafe fn lower_logical_expr(
        &mut self,
        op: LogicalOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<OrcValue, Diagnostic>;
    unsafe fn lower_builtin_call(
        &mut self,
        func: BuiltinFn,
        args: &[OrcValue],
    ) -> Result<OrcValue, Diagnostic>;

    fn builder(&self) -> LLVMBuilderRef;
    fn context(&self) -> LLVMContextRef;
    fn i32_ty(&self) -> LLVMTypeRef;
    fn float_ty(&self) -> LLVMTypeRef;
    fn fast_math_flags(&self) -> LLVMFastMathFlags;
    fn sample_rate(&self) -> f32;
    fn block_size(&self) -> f32;

    fn expr_context_label(&self) -> &'static str;
    fn array_tuple_literal_error(&self) -> &'static str;
    fn slice_error(&self) -> &'static str;
    fn array_ctor_error(&self) -> &'static str;
    fn cast_name(&self) -> &'static [u8];
    fn bitnot_name(&self) -> &'static [u8];
    fn bitnot_error_context(&self) -> &'static str;
}

pub(in crate::orc_backend) unsafe fn lower_scalar_expr_common<B: SharedScalarExprBackend>(
    backend: &mut B,
    expr: &Expr,
) -> Result<OrcValue, Diagnostic> {
    if let Some(literal) = lower_literal_expr_common(
        expr,
        backend.context(),
        backend.i32_ty(),
        backend.float_ty(),
    ) {
        return Ok(literal);
    }

    match expr {
        Expr::ArrayLiteral { .. } | Expr::Tuple { .. } => {
            Err(Diagnostic::internal(backend.array_tuple_literal_error()))
        }
        Expr::Var { name, .. } => {
            if let Some((ty, value)) =
                builtin_constant_value_and_type(name, backend.sample_rate(), backend.block_size())
            {
                Ok(OrcValue {
                    value: llvm_const_from_typed_f64(
                        backend.context(),
                        backend.float_ty(),
                        ty,
                        value,
                    ),
                    ty,
                })
            } else {
                backend.lower_var(name)
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let left = backend.lower_expr(lhs)?;
            let right = backend.lower_expr(rhs)?;
            let builder = backend.builder();
            let fast_math_flags = backend.fast_math_flags();
            let context_label = backend.expr_context_label();
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| unsafe {
                backend.cast_value_to(value, to, name)
            };
            lower_binary_numeric_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                context_label,
            )
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            let left = backend.lower_expr(lhs)?;
            let right = backend.lower_expr(rhs)?;
            let builder = backend.builder();
            let fast_math_flags = backend.fast_math_flags();
            let context_label = backend.expr_context_label();
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| unsafe {
                backend.cast_value_to(value, to, name)
            };
            lower_compare_common(
                *op,
                left,
                right,
                builder,
                fast_math_flags,
                &mut cast_value,
                context_label,
            )
        }
        Expr::Cast { to, expr, .. } => {
            if let Expr::Number { value: n, .. } = expr.as_ref() {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_f64(
                        backend.context(),
                        backend.float_ty(),
                        *to,
                        *n,
                    ),
                    ty: *to,
                });
            }
            if let Expr::Int { value: n, .. } = expr.as_ref() {
                return Ok(OrcValue {
                    value: llvm_const_from_typed_i64(
                        backend.context(),
                        backend.float_ty(),
                        *to,
                        *n,
                    ),
                    ty: *to,
                });
            }
            let value = backend.lower_expr(expr)?;
            Ok(OrcValue {
                value: backend.cast_value_to(value, *to, backend.cast_name()),
                ty: *to,
            })
        }
        Expr::UnaryNot { expr, .. } => {
            let value = backend.lower_expr(expr)?;
            let builder = backend.builder();
            let context = backend.context();
            let mut cast_value = |value: OrcValue, to: PrimitiveType, name: &[u8]| unsafe {
                backend.cast_value_to(value, to, name)
            };
            Ok(lower_unary_not_common(
                value,
                builder,
                context,
                &mut cast_value,
            ))
        }
        Expr::UnaryBitNot { expr, .. } => {
            let value = backend.lower_expr(expr)?;
            match value.ty {
                PrimitiveType::I32 | PrimitiveType::I64 => Ok(OrcValue {
                    value: LLVMBuildNot(
                        backend.builder(),
                        value.value,
                        backend.bitnot_name().as_ptr().cast(),
                    ),
                    ty: value.ty,
                }),
                _ => Err(Diagnostic::internal(format!(
                    "bitwise not requires integer operand in {}, got {:?}",
                    backend.bitnot_error_context(),
                    value.ty
                ))),
            }
        }
        Expr::Logical { op, lhs, rhs, .. } => backend.lower_logical_expr(*op, lhs, rhs),
        Expr::Call { func, args, .. } => {
            let mut lowered = Vec::with_capacity(args.len());
            for arg in args {
                lowered.push(backend.lower_expr(arg)?);
            }
            backend.lower_builtin_call(*func, &lowered)
        }
        Expr::Index { base, index, .. } => backend.lower_index_expr(base, index),
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => backend.lower_user_call_expr(name, type_args, args),
        Expr::Slice { .. } => Err(Diagnostic::internal(backend.slice_error())),
        Expr::ArrayCtor { .. } => Err(Diagnostic::internal(backend.array_ctor_error())),
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => unreachable!(),
    }
}
