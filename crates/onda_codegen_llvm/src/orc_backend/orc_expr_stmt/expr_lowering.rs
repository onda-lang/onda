use super::*;
use crate::orc_backend::lowering_common::{lower_scalar_expr_common, SharedScalarExprBackend};

struct OrcUserCallSpecialCaseBackend<'a, 'ctx> {
    ctx: &'a mut LoweringCtx<'ctx>,
    locals: &'a HashMap<String, OrcValue>,
    local_aliases: &'a HashMap<String, AliasSlot>,
    local_array_aliases: &'a HashMap<String, LocalArrayAlias>,
    local_tuples: &'a HashMap<String, Vec<PrimitiveType>>,
}

impl SharedUserCallSpecialCaseBackend for OrcUserCallSpecialCaseBackend<'_, '_> {
    unsafe fn lower_buffer_read2_call(
        &mut self,
        args: &[CallArg],
        clamp_index: bool,
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_buffer_read2_call(
            args,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
            clamp_index,
        )
    }

    unsafe fn lower_buffer_write2_call(
        &mut self,
        args: &[CallArg],
        clamp_index: bool,
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_buffer_write2_call(
            args,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
            clamp_index,
        )
    }

    fn is_builtin_data_len_receiver(&self, base: &str) -> bool {
        is_orc_builtin_data_len_receiver(self.ctx, base, self.local_array_aliases)
    }

    unsafe fn lower_data_len_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_data_len_call(method_name, base, args, self.ctx, self.local_array_aliases)
    }

    fn is_builtin_buffer_receiver(&self, base: &str) -> bool {
        is_orc_builtin_buffer_chans_receiver(self.ctx, base)
    }

    unsafe fn lower_buffer_chans_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_buffer_chans_call(method_name, base, args, self.ctx)
    }

    unsafe fn lower_buffer_samplerate_call(
        &mut self,
        method_name: &str,
        base: &str,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_buffer_samplerate_call(method_name, base, args, self.ctx)
    }

    fn is_builtin_unsafe_data_receiver(&self, base: &str) -> bool {
        is_orc_builtin_unsafe_data_receiver(self.ctx, base, self.local_array_aliases)
    }

    unsafe fn lower_unsafe_data_read_call(
        &mut self,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_unsafe_data_read_call(
            args,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_unsafe_data_write_call(
        &mut self,
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_unsafe_data_write_call(
            args,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }
}

struct OrcSharedExprBackend<'a, 'ctx> {
    ctx: &'a mut LoweringCtx<'ctx>,
    locals: &'a HashMap<String, OrcValue>,
    local_aliases: &'a HashMap<String, AliasSlot>,
    local_array_aliases: &'a HashMap<String, LocalArrayAlias>,
    local_tuples: &'a HashMap<String, Vec<PrimitiveType>>,
}

impl SharedScalarExprBackend for OrcSharedExprBackend<'_, '_> {
    unsafe fn lower_expr(&mut self, expr: &Expr) -> Result<OrcValue, Diagnostic> {
        lower_expr(
            expr,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_var(&mut self, name: &str) -> Result<OrcValue, Diagnostic> {
        if let Some(alias) = self.local_aliases.get(name) {
            return Ok(OrcValue {
                value: LLVMBuildLoad2(
                    self.ctx.builder,
                    llvm_ty_for_primitive(self.ctx.context, alias.ty),
                    alias.ptr,
                    b"alias_load\0".as_ptr().cast(),
                ),
                ty: alias.ty,
            });
        }
        if let Some(local) = self.locals.get(name) {
            return Ok(*local);
        }
        if let Some(byte_offset) = self.ctx.param_byte_offset.get(name).copied() {
            if byte_offset > i32::MAX as usize {
                return Err(Diagnostic::internal(
                    "parameter byte offset exceeds supported i32 index range in ORC lowering",
                ));
            }
            let param_ty = *self
                .ctx
                .param_types
                .get(name)
                .unwrap_or(&PrimitiveType::F32);
            let ptr = build_typed_ptr_from_byte_offset(
                self.ctx.builder,
                self.ctx.context,
                self.ctx.params_ptr,
                const_i32(self.ctx.i32_ty, byte_offset as i32),
                param_ty,
                b"param_ptr_i8\0",
                b"param_ptr_typed\0",
            );
            let raw = LLVMBuildLoad2(
                self.ctx.builder,
                llvm_ty_for_primitive(self.ctx.context, param_ty),
                ptr,
                b"param_load\0".as_ptr().cast(),
            );
            return Ok(OrcValue {
                value: raw,
                ty: param_ty,
            });
        }
        if let Some(slot) = self.ctx.state_slots.get(name) {
            return Ok(OrcValue {
                value: LLVMBuildLoad2(
                    self.ctx.builder,
                    llvm_ty_for_primitive(self.ctx.context, slot.ty),
                    slot.ptr,
                    b"state_load_expr\0".as_ptr().cast(),
                ),
                ty: slot.ty,
            });
        }
        if let Some(slot) = self.ctx.out_slots.get(name) {
            return Ok(OrcValue {
                value: LLVMBuildLoad2(
                    self.ctx.builder,
                    llvm_ty_for_primitive(self.ctx.context, slot.ty),
                    slot.ptr,
                    b"out_load_expr\0".as_ptr().cast(),
                ),
                ty: slot.ty,
            });
        }
        if let Some(ch) = self.ctx.input_index.get(name) {
            let in_ty = *self
                .ctx
                .input_types
                .get(name)
                .unwrap_or(&PrimitiveType::F32);
            let ch_v = LLVMConstInt(self.ctx.i32_ty, *ch as u64, 0);
            let in_ptr_ptr = build_ptr_offset(
                self.ctx.builder,
                self.ctx.float_ptr_ty,
                self.ctx.in_ptrs,
                ch_v,
                b"in_ch_ptr_ptr\0",
            );
            let in_ch_ptr = LLVMBuildLoad2(
                self.ctx.builder,
                self.ctx.float_ptr_ty,
                in_ptr_ptr,
                b"in_ch_ptr\0".as_ptr().cast(),
            );
            let in_ch_ptr_typed = LLVMBuildBitCast(
                self.ctx.builder,
                in_ch_ptr,
                LLVMPointerType(llvm_ty_for_primitive(self.ctx.context, in_ty), 0),
                b"in_ch_ptr_typed\0".as_ptr().cast(),
            );
            let ptr = build_f32_ptr_offset(
                self.ctx.builder,
                llvm_ty_for_primitive(self.ctx.context, in_ty),
                in_ch_ptr_typed,
                self.ctx.frame_idx,
                b"in_ptr\0",
            );
            let raw = LLVMBuildLoad2(
                self.ctx.builder,
                llvm_ty_for_primitive(self.ctx.context, in_ty),
                ptr,
                b"in_load\0".as_ptr().cast(),
            );
            if let Some(input_cache_ptr) = self.ctx.oversample_input_cache {
                let cached = load_cached_oversampled_input(
                    self.ctx.builder,
                    self.ctx.context,
                    input_cache_ptr,
                    ch_v,
                    in_ty,
                );
                return Ok(OrcValue {
                    value: cached,
                    ty: in_ty,
                });
            }
            return Ok(OrcValue {
                value: raw,
                ty: in_ty,
            });
        }
        if self.ctx.array_base_ptrs.contains_key(name)
            || self.ctx.array_struct_len.contains_key(name)
        {
            return Err(Diagnostic::internal(format!(
                "array symbol '{name}' must be indexed in ORC expression lowering"
            )));
        }
        if self.ctx.buffer_index.contains_key(name) {
            return Err(Diagnostic::internal(format!(
                "buffer symbol '{name}' must be indexed in ORC expression lowering"
            )));
        }
        if self.ctx.input_arrays.contains_key(name)
            || self.ctx.param_arrays.contains_key(name)
            || self.ctx.output_arrays.contains_key(name)
        {
            return Err(Diagnostic::internal(format!(
                "top-level array symbol '{name}' must be indexed in ORC expression lowering"
            )));
        }
        Err(Diagnostic::internal(format!(
            "unknown symbol '{name}' in ORC expression lowering"
        )))
    }

    unsafe fn lower_index_expr(
        &mut self,
        base: &str,
        index: &Expr,
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_index_expr(
            base,
            index,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_user_call_expr(
        &mut self,
        name: &str,
        type_args: &[CallTypeArg],
        args: &[CallArg],
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_user_call_expr(
            name,
            type_args,
            args,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn cast_value_to(
        &mut self,
        value: OrcValue,
        to: PrimitiveType,
        name: &[u8],
    ) -> LLVMValueRef {
        cast_orc_value_to(self.ctx, value, to, name)
    }

    unsafe fn lower_logical_expr(
        &mut self,
        op: LogicalOp,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<OrcValue, Diagnostic> {
        lower_orc_logical_expr(
            op,
            lhs,
            rhs,
            self.ctx,
            self.locals,
            self.local_aliases,
            self.local_array_aliases,
            self.local_tuples,
        )
    }

    unsafe fn lower_builtin_call(
        &mut self,
        func: BuiltinFn,
        args: &[OrcValue],
    ) -> Result<OrcValue, Diagnostic> {
        lower_builtin_call_orc(self.ctx, func, args)
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
        "ORC expression lowering"
    }

    fn array_tuple_literal_error(&self) -> &'static str {
        "array/tuple literal is not a scalar expression in ORC lowering"
    }

    fn slice_error(&self) -> &'static str {
        "slice expressions are not yet supported in ORC scalar lowering"
    }

    fn array_ctor_error(&self) -> &'static str {
        "array constructor is only valid as an init assignment value"
    }

    fn cast_name(&self) -> &'static [u8] {
        b"cast\0"
    }

    fn bitnot_name(&self) -> &'static [u8] {
        b"bitnot\0"
    }

    fn bitnot_error_context(&self) -> &'static str {
        "ORC expression lowering"
    }
}

unsafe fn lower_orc_index_expr(
    base: &str,
    index: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    if base == "ins" {
        if let Some(meta) = ctx.port_index_ins {
            return lower_port_index_ins_read(
                ctx,
                meta,
                index,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            );
        }
    }
    if base == "outs" || base == "kouts" {
        if let Some(meta) = ctx.port_index_outs {
            return lower_port_index_outs_read(
                ctx,
                meta,
                index,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            );
        }
    }
    if base == "params" || base == "kins" {
        if let Some(meta) = ctx.port_index_params {
            return lower_port_index_params_read(
                ctx,
                meta,
                index,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
            );
        }
    }
    if local_tuples.contains_key(base) {
        if let Expr::Int { value, .. } = index {
            let flat_name = format!("{base}.__{value}");
            if let Some(slot) = local_aliases.get(&flat_name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"tuple_local_load\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            return Err(Diagnostic::internal(format!(
                "local tuple element '{flat_name}' not found in local aliases"
            )));
        }
        return Err(Diagnostic::internal(
            "tuple element index must be a compile-time integer constant in ORC lowering",
        ));
    }
    if ctx.state_slots.contains_key(&format!("{base}.__0")) {
        if let Expr::Int { value, .. } = index {
            let flat_name = format!("{base}.__{value}");
            if let Some(slot) = ctx.state_slots.get(&flat_name) {
                return Ok(OrcValue {
                    value: LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"tuple_state_load\0".as_ptr().cast(),
                    ),
                    ty: slot.ty,
                });
            }
            return Err(Diagnostic::internal(format!(
                "tuple state element '{flat_name}' not found in state slots"
            )));
        }
        return Err(Diagnostic::internal(
            "tuple element index must be a compile-time integer constant in ORC lowering",
        ));
    }
    if ctx.buffer_index.contains_key(base) {
        let data = lower_buffer_element_ptr(
            ctx,
            base,
            index,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
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
            local_tuples,
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
            local_tuples,
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
            local_tuples,
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
        local_tuples,
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

unsafe fn lower_orc_user_call_expr(
    name: &str,
    type_args: &[CallTypeArg],
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    let mut special_backend = OrcUserCallSpecialCaseBackend {
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    };
    if let Some(result) = try_lower_user_call_special_case_common(&mut special_backend, name, args)
    {
        return result;
    }
    if ctx.struct_fields.contains_key(name) {
        return Err(Diagnostic::internal(format!(
            "struct constructor '{name}(...)' used in scalar expression lowering"
        )));
    }
    let shared = orc_user_call_context(ctx);
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let mut lower_scalar_expr = |arg_expr: &Expr| unsafe {
        lower_expr(
            arg_expr,
            &mut *ctx_ptr,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )
    };
    let mut infer_buffer_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_buffer_arg_signature_in_orc(&*ctx_ptr, local_array_aliases, arg_expr, callee_name)
    };
    let mut infer_array_arg_signature = |arg_expr: &Expr, callee_name: &str| unsafe {
        infer_array_arg_signature_in_orc(&*ctx_ptr, local_array_aliases, arg_expr, callee_name)
    };
    let ctx_ptr: *mut LoweringCtx<'_> = ctx;
    let mut cast_scalar_arg = |value: OrcValue, target_ty: PrimitiveType, arg_name: &[u8]| unsafe {
        cast_orc_value_to(&*ctx_ptr, value, target_ty, arg_name)
    };
    let mut lower_struct_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                arg_expr: &Expr,
                                struct_name: &str,
                                by_ref: bool| unsafe {
        lower_struct_call_args_in_orc(
            &mut *ctx_ptr,
            arg_values,
            arg_expr,
            struct_name,
            name,
            by_ref,
            locals,
            local_aliases,
            local_array_aliases,
            local_tuples,
        )
    };
    let mut lower_struct_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                      arg_expr: &Expr,
                                      struct_name: &str| unsafe {
        lower_struct_array_call_args_in_orc(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_proc_array_arg = |arg_values: &mut Vec<LLVMValueRef>,
                                    arg_expr: &Expr,
                                    struct_name: &str| unsafe {
        lower_proc_array_call_args_in_orc(&mut *ctx_ptr, arg_values, arg_expr, struct_name, name)
    };
    let mut lower_array_arg =
        |arg_values: &mut Vec<LLVMValueRef>,
         arg_expr: &Expr,
         expected_elem_ty: Option<PrimitiveType>| unsafe {
            lower_array_call_args_in_orc(
                &mut *ctx_ptr,
                locals,
                local_aliases,
                local_array_aliases,
                local_tuples,
                arg_values,
                arg_expr,
                name,
                expected_elem_ty,
            )
        };
    let mut lower_buffer_arg = |arg_values: &mut Vec<LLVMValueRef>, arg_expr: &Expr| unsafe {
        lower_buffer_call_args_in_orc(
            &mut *ctx_ptr,
            local_array_aliases,
            local_tuples,
            arg_values,
            arg_expr,
            name,
            locals,
            local_aliases,
        )
    };
    let lowered = lower_user_call_common(
        shared,
        name,
        type_args,
        args,
        &mut lower_scalar_expr,
        &mut infer_buffer_arg_signature,
        &mut infer_array_arg_signature,
        "ORC expression lowering",
        b"call_arg\0",
        &mut cast_scalar_arg,
        &mut lower_struct_arg,
        &mut lower_struct_array_arg,
        &mut lower_proc_array_arg,
        &mut lower_array_arg,
        &mut lower_buffer_arg,
        b"call\0",
    )?;
    match &lowered.ret_ty {
        ReturnType::Scalar(scalar_ty) => {
            set_fast_math_for_primitive(lowered.call, *scalar_ty, ctx.fast_math_flags);
            Ok(OrcValue {
                value: lowered.call,
                ty: *scalar_ty,
            })
        }
        ReturnType::Tuple(_) => Err(Diagnostic::internal(
            "tuple-returning call is not a scalar expression in orc expr/stmt lowering",
        )),
    }
}

pub(super) unsafe fn lower_expr(
    expr: &Expr,
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    let mut common_backend = OrcSharedExprBackend {
        ctx,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
    };
    lower_scalar_expr_common(&mut common_backend, expr)
}

/// Read `ins[i]`: load from `in_ptrs[clamped_i]` at `frame_idx`.
unsafe fn lower_port_index_ins_read(
    ctx: &mut LoweringCtx<'_>,
    meta: PortIndexMeta,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        meta.count,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
        true,
    )?;
    let in_ptr_ptr = build_ptr_offset(
        ctx.builder,
        ctx.float_ptr_ty,
        ctx.in_ptrs,
        idx,
        b"ins_ch_ptr_ptr\0",
    );
    let in_ch_ptr = LLVMBuildLoad2(
        ctx.builder,
        ctx.float_ptr_ty,
        in_ptr_ptr,
        b"ins_ch_ptr\0".as_ptr().cast(),
    );
    let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, meta.elem_ty);
    let in_ch_ptr_typed = LLVMBuildBitCast(
        ctx.builder,
        in_ch_ptr,
        LLVMPointerType(elem_llvm_ty, 0),
        b"ins_ch_ptr_typed\0".as_ptr().cast(),
    );
    let ptr = build_f32_ptr_offset(
        ctx.builder,
        elem_llvm_ty,
        in_ch_ptr_typed,
        ctx.frame_idx,
        b"ins_ptr\0",
    );
    let value = LLVMBuildLoad2(
        ctx.builder,
        elem_llvm_ty,
        ptr,
        b"ins_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value,
        ty: meta.elem_ty,
    })
}

/// Read `outs[i]`: load from `out_slots` via `out_ptrs[clamped_i]` at `frame_idx`.
/// We read back from the output buffer pointer (same as the write-back loop).
unsafe fn lower_port_index_outs_read(
    ctx: &mut LoweringCtx<'_>,
    meta: PortIndexMeta,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    let arr = ctx
        .out_slot_ptr_array
        .ok_or_else(|| Diagnostic::internal("outs[i] read requires out_slot_ptr_array"))?;
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        meta.count,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
        true,
    )?;
    let elem_llvm_ty = llvm_ty_for_primitive(ctx.context, meta.elem_ty);
    let slot_ptr_ty = LLVMPointerType(elem_llvm_ty, 0);
    let gep = LLVMBuildGEP2(
        ctx.builder,
        slot_ptr_ty,
        arr,
        [idx].as_mut_ptr(),
        1,
        b"outs_r_slot_gep\0".as_ptr().cast(),
    );
    let slot_ptr = LLVMBuildLoad2(
        ctx.builder,
        slot_ptr_ty,
        gep,
        b"outs_r_slot_ptr\0".as_ptr().cast(),
    );
    let value = LLVMBuildLoad2(
        ctx.builder,
        elem_llvm_ty,
        slot_ptr,
        b"outs_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value,
        ty: meta.elem_ty,
    })
}

/// Read `params[i]`: load from `params_ptr` at byte offset `i * sizeof(type)`.
unsafe fn lower_port_index_params_read(
    ctx: &mut LoweringCtx<'_>,
    meta: PortIndexMeta,
    index_expr: &Expr,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
    local_tuples: &HashMap<String, Vec<PrimitiveType>>,
) -> Result<OrcValue, Diagnostic> {
    let idx = lower_array_index_i32(
        ctx,
        index_expr,
        meta.count,
        locals,
        local_aliases,
        local_array_aliases,
        local_tuples,
        true,
    )?;
    let elem_bytes = primitive_type_bytes(meta.elem_ty) as u64;
    let byte_offset = LLVMBuildMul(
        ctx.builder,
        idx,
        LLVMConstInt(ctx.i32_ty, elem_bytes, 0),
        b"param_byte_offset\0".as_ptr().cast(),
    );
    let ptr = build_typed_ptr_from_byte_offset(
        ctx.builder,
        ctx.context,
        ctx.params_ptr,
        byte_offset,
        meta.elem_ty,
        b"param_dyn_ptr_i8\0",
        b"param_dyn_ptr\0",
    );
    let value = LLVMBuildLoad2(
        ctx.builder,
        llvm_ty_for_primitive(ctx.context, meta.elem_ty),
        ptr,
        b"params_load\0".as_ptr().cast(),
    );
    Ok(OrcValue {
        value,
        ty: meta.elem_ty,
    })
}
