use super::*;

pub(super) fn ensure_builtin_data_call_positional_arity(
    args: &[CallArg],
    name: &str,
    expected: usize,
    context: &str,
) -> Result<(), Diagnostic> {
    if args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "builtin '{name}' expects {expected} positional arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "builtin '{name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

pub(super) fn builtin_data_call_base_symbol<'a>(
    args: &'a [CallArg],
    name: &str,
    context: &str,
) -> Result<&'a str, Diagnostic> {
    let first = args.first().ok_or_else(|| {
        Diagnostic::internal(format!(
            "builtin '{name}' missing first argument in {context}"
        ))
    })?;
    match &first.expr {
        Expr::Var(base) => Ok(base.as_str()),
        _ => Err(Diagnostic::internal(format!(
            "builtin '{name}' requires a Data/buffer symbol variable as first argument in {context}"
        ))),
    }
}

pub(super) fn ensure_internal_buffer_2d_call_positional_arity(
    args: &[CallArg],
    name: &str,
    expected: usize,
    context: &str,
) -> Result<(), Diagnostic> {
    if args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{name}' expects {expected} positional arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

pub(super) fn parse_data_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "len" {
        return None;
    }
    Some(base)
}

pub(super) fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "chans" {
        return None;
    }
    Some(base)
}

pub(super) fn ensure_builtin_instance_call_no_args(
    method_name: &str,
    args: &[CallArg],
    context: &str,
) -> Result<(), Diagnostic> {
    if !args.is_empty() {
        return Err(Diagnostic::internal(format!(
            "builtin method '{method_name}' expects 0 arguments in {context}, got {}",
            args.len()
        )));
    }
    if args.iter().any(|a| a.name.is_some()) {
        return Err(Diagnostic::internal(format!(
            "builtin method '{method_name}' does not support named arguments in {context}"
        )));
    }
    Ok(())
}

pub(super) fn checked_len_const_i32(len: usize, context: &str) -> Result<u64, Diagnostic> {
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "Data length {len} exceeds i32 range in {context}"
        )));
    }
    Ok(len as u64)
}

pub(super) fn local_data_alias_len(alias: &LocalDataAlias) -> usize {
    match alias {
        LocalDataAlias::Primitive { len, .. } | LocalDataAlias::Struct { len, .. } => *len,
    }
}

pub(super) fn lookup_orc_data_symbol_len(
    ctx: &LoweringCtx<'_>,
    base: &str,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Option<usize> {
    if let Some(alias) = local_data_aliases.get(base) {
        return Some(local_data_alias_len(alias));
    }
    if let Some(info) = ctx.input_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(info) = ctx.param_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(info) = ctx.output_arrays.get(base) {
        return Some(info.len);
    }
    if let Some(len) = ctx.data_len.get(base) {
        return Some(*len);
    }
    ctx.data_struct_len.get(base).copied()
}

pub(super) fn lookup_def_data_symbol_len(ctx: &DefLoweringCtx<'_>, base: &str) -> Option<usize> {
    if let Some(alias) = ctx.local_data_aliases.get(base) {
        return Some(local_data_alias_len(alias));
    }
    ctx.data_len.get(base).copied()
}

pub(super) unsafe fn lower_orc_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if let Some(len) = lookup_orc_data_symbol_len(ctx, base, local_data_aliases) {
        let len_const = checked_len_const_i32(len, "ORC expression lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        return Ok(OrcValue {
            value: load_orc_buffer_total_len_i32(ctx, base)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a Data or buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

pub(super) unsafe fn lower_orc_buffer_chans_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if ctx.buffer_index.contains_key(base) {
        let channels = if ctx.buffer_mono.contains(base) {
            LLVMConstInt(ctx.i32_ty, 1, 0)
        } else {
            load_orc_buffer_channels_i32(ctx, base)?
        };
        return Ok(OrcValue {
            value: channels,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

pub(super) unsafe fn lower_def_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    if let Some(len) = lookup_def_data_symbol_len(ctx, base) {
        let len_const = checked_len_const_i32(len, "def lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        return Ok(OrcValue {
            value: load_def_buffer_total_len_i32(ctx, base, &info)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a Data or buffer symbol receiver in def lowering, got '{base}'"
    )))
}

pub(super) unsafe fn lower_orc_buffer_read2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
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
        local_data_aliases,
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

pub(super) unsafe fn lower_orc_buffer_write2_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
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
        local_data_aliases,
        clamp_index,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"buf2_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

pub(super) unsafe fn lower_orc_unsafe_data_read_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
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
            local_data_aliases,
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
            local_data_aliases,
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
            local_data_aliases,
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
            local_data_aliases,
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
        local_data_aliases,
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

pub(super) unsafe fn lower_orc_unsafe_data_write_call(
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    locals: &HashMap<String, OrcValue>,
    local_aliases: &HashMap<String, AliasSlot>,
    local_data_aliases: &HashMap<String, LocalDataAlias>,
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
            local_data_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
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
            local_data_aliases,
            false,
        )?;
        let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
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
        local_data_aliases,
    )?;
    let value = lower_expr(value_expr, ctx, locals, local_aliases, local_data_aliases)?;
    let casted = cast_orc_value_to(ctx, value, data.elem_ty, b"unsafe_data_write_cast\0");
    LLVMBuildStore(ctx.builder, casted, data.ptr);
    Ok(OrcValue {
        value: casted,
        ty: data.elem_ty,
    })
}

pub(super) unsafe fn lower_def_unsafe_data_read_call(
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

pub(super) unsafe fn lower_def_unsafe_data_write_call(
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

pub(super) fn builtin_constant_value(name: &str, sample_rate: f32, block_size: f32) -> Option<f32> {
    match name {
        "PI" | "pi" => Some(std::f32::consts::PI),
        "TWO_PI" | "TWOPI" | "two_pi" | "twopi" => Some(2.0 * std::f32::consts::PI),
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => Some(sample_rate),
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => Some(block_size),
        _ => None,
    }
}

pub(super) fn eval_const_default_expr(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<f32, Diagnostic> {
    match expr {
        Expr::Number(v) => Ok(*v),
        Expr::Int(v) => Ok(*v as f32),
        Expr::Bool(v) => Ok(if *v { 1.0 } else { 0.0 }),
        Expr::Var(name) => builtin_constant_value(name, sample_rate, block_size).ok_or_else(|| {
            Diagnostic::internal(format!(
                "default expression uses non-constant symbol '{name}' in codegen"
            ))
        }),
        Expr::Cast { to, expr } => {
            let v = eval_const_default_expr(expr, sample_rate, block_size)?;
            let out = match to {
                PrimitiveType::F32 | PrimitiveType::F64 => v,
                PrimitiveType::I32 => (v as i32) as f32,
                PrimitiveType::I64 => (v as i64) as f32,
                PrimitiveType::Bool => {
                    if v != 0.0 {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            Ok(out)
        }
        Expr::UnaryNot { expr } => {
            let v = eval_const_default_expr(expr, sample_rate, block_size)?;
            Ok(if v == 0.0 { 1.0 } else { 0.0 })
        }
        Expr::Logical { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            match op {
                LogicalOp::And => {
                    if l == 0.0 {
                        Ok(0.0)
                    } else {
                        let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
                        Ok(if r != 0.0 { 1.0 } else { 0.0 })
                    }
                }
                LogicalOp::Or => {
                    if l != 0.0 {
                        Ok(1.0)
                    } else {
                        let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
                        Ok(if r != 0.0 { 1.0 } else { 0.0 })
                    }
                }
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
            let out = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Mul => l * r,
                BinaryOp::Div => l / r,
                BinaryOp::Mod => l % r,
            };
            Ok(out)
        }
        Expr::Compare { op, lhs, rhs } => {
            let l = eval_const_default_expr(lhs, sample_rate, block_size)?;
            let r = eval_const_default_expr(rhs, sample_rate, block_size)?;
            let pred = match op {
                CmpOp::Eq => l == r,
                CmpOp::Ne => l != r,
                CmpOp::Lt => l < r,
                CmpOp::Le => l <= r,
                CmpOp::Gt => l > r,
                CmpOp::Ge => l >= r,
            };
            Ok(if pred { 1.0 } else { 0.0 })
        }
        _ => Err(Diagnostic::internal(
            "default expression must be compile-time constant in codegen",
        )),
    }
}

pub(super) fn eval_const_data_size_expr(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<usize, Diagnostic> {
    let value = eval_const_default_expr(expr, sample_rate, block_size)?;
    if !value.is_finite() {
        return Err(Diagnostic::internal(
            "Data size expression must evaluate to a finite constant",
        ));
    }

    let truncated = value.trunc();
    if (value - truncated).abs() > 1e-6 {
        return Err(Diagnostic::internal(
            "Data size expression must evaluate to an integer constant",
        ));
    }
    if truncated <= 0.0 {
        return Err(Diagnostic::internal(
            "Data size expression must be greater than zero",
        ));
    }
    if truncated > usize::MAX as f32 {
        return Err(Diagnostic::internal(
            "Data size expression exceeds supported range",
        ));
    }

    Ok(truncated as usize)
}
pub(super) fn resolve_call_args_codegen<'a>(
    args: &'a [CallArg],
    param_names: &[String],
    param_defaults: &[Option<Expr>],
    forbid_self_named: bool,
    context: &str,
) -> Result<Vec<Option<&'a Expr>>, Diagnostic> {
    if param_names.len() != param_defaults.len() {
        return Err(Diagnostic::internal(format!(
            "{context}: inconsistent parameter/default metadata"
        )));
    }

    let mut resolved = vec![None; param_names.len()];
    let mut next_pos = 0usize;
    let mut named_seen = HashSet::new();
    let mut saw_named = false;

    for arg in args {
        if let Some(name) = &arg.name {
            saw_named = true;
            if forbid_self_named && name == "self" {
                return Err(Diagnostic::internal(format!(
                    "{context}: 'self' cannot be passed as named argument"
                )));
            }
            if !named_seen.insert(name.clone()) {
                return Err(Diagnostic::internal(format!(
                    "{context}: duplicate named argument '{name}'"
                )));
            }
            let Some(idx) = param_names.iter().position(|p| p == name) else {
                return Err(Diagnostic::internal(format!(
                    "{context}: unknown named argument '{name}'"
                )));
            };
            if resolved[idx].is_some() {
                return Err(Diagnostic::internal(format!(
                    "{context}: argument '{name}' provided more than once"
                )));
            }
            resolved[idx] = Some(&arg.expr);
        } else {
            if saw_named {
                return Err(Diagnostic::internal(format!(
                    "{context}: positional arguments must precede named arguments"
                )));
            }
            while next_pos < resolved.len() && resolved[next_pos].is_some() {
                next_pos += 1;
            }
            if next_pos >= resolved.len() {
                return Err(Diagnostic::internal(format!(
                    "{context}: too many positional arguments (expected at most {})",
                    param_names.len()
                )));
            }
            resolved[next_pos] = Some(&arg.expr);
            next_pos += 1;
        }
    }

    for (idx, arg) in resolved.iter().enumerate() {
        if arg.is_none() && param_defaults[idx].is_none() {
            return Err(Diagnostic::internal(format!(
                "{context}: missing required argument '{}'",
                param_names[idx]
            )));
        }
    }

    Ok(resolved)
}

pub(super) unsafe fn lower_struct_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    struct_name: &str,
    callee_name: &str,
    by_ref: bool,
) -> Result<(), Diagnostic> {
    let base = match arg_expr {
        Expr::Var(name) => name,
        _ => {
            return Err(Diagnostic::internal(format!(
                "function '{callee_name}' expects struct '{struct_name}' argument as a variable reference in ORC lowering"
            )));
        }
    };
    let fields = ctx.struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{struct_name}' in ORC call lowering for function '{callee_name}'"
        ))
    })?;
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(_) => {
                let slot = ctx.state_slots.get(&flat).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "missing state slot for struct field '{flat}' while calling '{callee_name}'"
                    ))
                })?;
                if by_ref {
                    out_args.push(slot.ptr);
                } else {
                    let value = LLVMBuildLoad2(
                        ctx.builder,
                        llvm_ty_for_primitive(ctx.context, slot.ty),
                        slot.ptr,
                        b"struct_arg_load\0".as_ptr().cast(),
                    );
                    out_args.push(value);
                }
            }
            TypedFieldType::Data(_) => {
                let mut symbols = Vec::<String>::new();
                if let Some(elem_struct) = &field.data_elem_struct {
                    let root_len = *ctx.data_struct_len.get(&flat).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing Data[Struct] length metadata for '{flat}' while lowering struct argument for '{callee_name}'"
                        ))
                    })?;
                    let mut roots = Vec::new();
                    let mut leaves = Vec::new();
                    collect_data_struct_bindings(
                        ctx.struct_fields,
                        elem_struct,
                        &flat,
                        root_len,
                        &mut roots,
                        &mut leaves,
                        &mut Vec::new(),
                    )?;
                    symbols.extend(leaves.into_iter().map(|(name, _, _)| name));
                } else {
                    symbols.push(flat.clone());
                }
                for symbol in symbols {
                    let data_base_ptr = *ctx.data_base_ptrs.get(&symbol).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "missing Data symbol '{symbol}' while lowering struct argument for '{callee_name}'"
                        ))
                    })?;
                    out_args.push(data_base_ptr);
                }
            }
        }
    }
    Ok(())
}

pub(super) unsafe fn load_orc_buffer_binding_tuple(
    ctx: &mut LoweringCtx<'_>,
    base: &str,
) -> Result<(LLVMValueRef, LLVMValueRef, LLVMValueRef), Diagnostic> {
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
    Ok((ptr, frames, channels))
}

pub(super) unsafe fn lower_buffer_call_args_in_orc(
    ctx: &mut LoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in ORC expression lowering"
        )));
    };
    let (ptr, frames, channels) = load_orc_buffer_binding_tuple(ctx, base)?;
    out_args.push(ptr);
    out_args.push(frames);
    out_args.push(channels);
    Ok(())
}

pub(super) fn infer_buffer_arg_signature_in_orc(
    ctx: &LoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in ORC expression lowering"
        )));
    };
    let elem_ty = *ctx.buffer_elem_types.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in ORC buffer signature inference"
        ))
    })?;
    let channels = ctx.buffer_channels.get(base).cloned().ok_or_else(|| {
        Diagnostic::internal(format!(
            "missing channel metadata for buffer symbol '{base}' in ORC buffer signature inference"
        ))
    })?;
    Ok((elem_ty, channels))
}

pub(super) unsafe fn lower_buffer_call_args_in_def(
    ctx: &mut DefLoweringCtx<'_>,
    out_args: &mut Vec<LLVMValueRef>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in def lowering"
        )));
    };
    let info = ctx.buffer_params.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def buffer call argument lowering"
        ))
    })?;
    out_args.push(info.ptr);
    out_args.push(info.frames);
    out_args.push(info.channels);
    Ok(())
}

pub(super) fn infer_buffer_arg_signature_in_def(
    ctx: &DefLoweringCtx<'_>,
    arg_expr: &Expr,
    callee_name: &str,
) -> Result<(PrimitiveType, TypedBufferChannels), Diagnostic> {
    let Expr::Var(base) = arg_expr else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in def lowering"
        )));
    };
    let info = ctx.buffer_params.get(base).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown buffer symbol '{base}' in def buffer signature inference"
        ))
    })?;
    Ok((info.elem_ty, info.declared_channels.clone()))
}
