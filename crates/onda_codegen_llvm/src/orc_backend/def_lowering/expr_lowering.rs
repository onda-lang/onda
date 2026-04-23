use super::struct_helpers::{
    lower_proc_array_call_args_in_def, lower_struct_array_call_args_in_def,
};
use super::*;
use crate::orc_backend::lowering_common::{lower_scalar_expr_common, SharedScalarExprBackend};

struct DefUserCallSpecialCaseBackend<'a, 'ctx> {
    ctx: &'a mut DefLoweringCtx<'ctx>,
}

impl SharedUserCallSpecialCaseBackend for DefUserCallSpecialCaseBackend<'_, '_> {
    unsafe fn lower_buffer_read2_call(&mut self, args: &[CallArg]) -> Result<OrcValue, Diagnostic> {
        lower_def_buffer_read2_call(args, self.ctx, true)
    }

    unsafe fn lower_buffer_write2_call(
        &mut self,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_buffer_write2_call(args, self.ctx, true)
    }

    fn is_builtin_data_len_receiver(&self, base: &str) -> bool {
        is_def_builtin_data_len_receiver(self.ctx, base)
    }

    unsafe fn lower_data_len_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_data_len_call(method_name, base, args, self.ctx)
    }

    fn is_builtin_buffer_receiver(&self, base: &str) -> bool {
        is_def_builtin_buffer_chans_receiver(self.ctx, base)
    }

    unsafe fn lower_buffer_chans_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_buffer_chans_call(method_name, base, args, self.ctx)
    }

    unsafe fn lower_buffer_samplerate_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_buffer_samplerate_call(method_name, base, args, self.ctx)
    }

    fn is_builtin_unsafe_data_receiver(&self, base: &str) -> bool {
        is_def_builtin_unsafe_data_receiver(self.ctx, base)
    }

    unsafe fn lower_unsafe_data_read_call(
        &mut self,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_unsafe_data_read_call(args, self.ctx)
    }

    unsafe fn lower_unsafe_data_write_call(
        &mut self,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_unsafe_data_write_call(args, self.ctx)
    }
}

struct DefSharedExprBackend<'a, 'ctx> {
    ctx: &'a mut DefLoweringCtx<'ctx>,
}

impl SharedScalarExprBackend for DefSharedExprBackend<'_, '_> {
    unsafe fn lower_expr(&mut self, expr: &Expr) -> Result<OrcValue, Diagnostic> {
        lower_def_expr(expr, self.ctx)
    }

    unsafe fn lower_var(&mut self, name: &str) -> Result<OrcValue, Diagnostic> {
        if self.ctx.buffer_params.contains_key(name) {
            return Err(Diagnostic::internal(format!(
                "buffer symbol '{name}' must be indexed in def lowering"
            )));
        }
        let local = self.ctx.local_slots.get(name).ok_or_else(|| {
            Diagnostic::internal(format!("unknown local '{name}' in def lowering"))
        })?;
        let loaded = LLVMBuildLoad2(
            self.ctx.builder,
            llvm_ty_for_primitive(self.ctx.context, local.ty),
            local.ptr,
            b"def_load\0".as_ptr().cast(),
        );
        Ok(OrcValue {
            value: loaded,
            ty: local.ty,
        })
    }

    unsafe fn lower_index_expr(
        &mut self,
        base: &str,
        index: &Expr,
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_index_expr(base, index, self.ctx)
    }

    unsafe fn lower_user_call_expr(
        &mut self,
        name: &str,
        type_args: &[CallTypeArg],
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_user_call_expr(name, type_args, args, self.ctx)
    }

    unsafe fn cast_value_to(
        &mut self,
        value: OrcValue,
        to: PrimitiveType,
        name: &[u8],
    ) -> LLVMValueRef {
        cast_def_value_to(self.ctx, value, to, name)
    }

    unsafe fn lower_logical_expr(
        &mut self,
        op: LogicalOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<OrcValue, Diagnostic> {
        lower_def_logical_expr(op, lhs, rhs, self.ctx)
    }

    unsafe fn lower_builtin_call(
        &mut self,
        func: BuiltinFn,
        args: &[OrcValue],
    ) -> Result<OrcValue, Diagnostic> {
        lower_builtin_call_def(self.ctx, func, args)
    }

    fn builder(&self) -> LLVMBuilderRef {
        self.ctx.builder
    }

    fn context(&self) -> LLVMContextRef {
        self.ctx.context
    }

    fn i32_ty(&self) -> LLVMTypeRef {
        self.ctx.i32_ty
    }

    fn float_ty(&self) -> LLVMTypeRef {
        self.ctx.float_ty
    }

    fn fast_math_flags(&self) -> LLVMFastMathFlags {
        self.ctx.fast_math_flags
    }

    fn sample_rate(&self) -> f32 {
        self.ctx.sample_rate
    }

    fn block_size(&self) -> f32 {
        self.ctx.block_size
    }

    fn expr_context_label(&self) -> &'static str {
        "def lowering"
    }

    fn array_tuple_literal_error(&self) -> &'static str {
        "array/tuple literal is not a scalar expression in def lowering"
    }

    fn slice_error(&self) -> &'static str {
        "slice expressions are not yet supported in def scalar lowering"
    }

    fn array_ctor_error(&self) -> &'static str {
        "array constructor is not supported in def lowering"
    }

    fn cast_name(&self) -> &'static [u8] {
        b"def_cast\0"
    }

    fn bitnot_name(&self) -> &'static [u8] {
        b"def_bitnot\0"
    }

    fn bitnot_error_context(&self) -> &'static str {
        "def lowering"
    }
}

unsafe fn lower_def_index_expr(
    base: &str,
    index: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    if let Some(tuple_slot) = ctx.tuple_slots.get(base).cloned() {
        let const_idx = match index {
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

unsafe fn lower_def_user_call_expr(
    name: &str,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    let mut special_backend = DefUserCallSpecialCaseBackend { ctx };
    if let Some(result) = try_lower_user_call_special_case_common(&mut special_backend, name, args)
    {
        return result;
    }
    let shared = def_user_call_context(ctx);
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let mut lower_scalar_expr =
        |arg_expr: &Expr| unsafe { lower_def_expr(arg_expr, &mut *ctx_ptr) };
    let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_buffer_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
    };
    let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_array_arg_signature_in_def(&*ctx_ptr, arg_expr, callee_name)
    };
    let ctx_ptr: *mut DefLoweringCtx<'_> = ctx;
    let mut cast_scalar_arg = |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
        cast_def_value_to(&*ctx_ptr, value, target_ty, arg_name)
    };
    let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                arg_expr: &Expr,
                                struct_name: &str,
                                by_ref: bool| unsafe {
        lower_struct_call_args_in_def(
            &mut *ctx_ptr,
            arg_values,
            arg_expr,
            struct_name,
            name,
            by_ref,
        )
    };
    let mut lower_struct_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                      arg_expr: &Expr,
                                      struct_name: &str| unsafe {
        lower_struct_array_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_proc_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                    arg_expr: &Expr,
                                    struct_name: &str| unsafe {
        lower_proc_array_call_args_in_def(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
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
    let lowered = lower_user_call_common(
        shared,
        name,
        type_args,
        args,
        &mut lower_scalar_expr,
        &mut infer_buffer_arg_signature,
        &mut infer_array_arg_signature,
        "def lowering",
        b"def_call_arg\0",
        &mut cast_scalar_arg,
        &mut lower_struct_arg,
        &mut lower_struct_array_arg,
        &mut lower_proc_array_arg,
        &mut lower_array_arg,
        &mut lower_buffer_arg,
        b"def_call\0",
    )?;
    match &lowered.ret_ty {
        ReturnType::Scalar(scalar_ty) => {
            set_fast_math_for_primitive(lowered.call, *scalar_ty, ctx.fast_math_flags);
            Ok(OrcValue {
                value: lowered.call,
                ty: *scalar_ty,
            })
        }
        ReturnType::Tuple(_) => Err(Diagnostic::internal(format!(
            "tuple-returning call to '{}' is not a scalar expression in def lowering",
            name
        ))),
    }
}

pub(super) unsafe fn lower_def_expr(
    expr: &Expr,
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    let mut common_backend = DefSharedExprBackend { ctx };
    return lower_scalar_expr_common(&mut common_backend, expr);
}
