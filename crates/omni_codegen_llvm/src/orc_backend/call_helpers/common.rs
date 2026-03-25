use super::super::*;

pub(in crate::orc_backend) fn ensure_builtin_data_call_positional_arity(
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

pub(in crate::orc_backend) fn builtin_data_call_base_symbol<'a>(
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
        Expr::Var { name: base, .. } => Ok(base.as_str()),
        _ => Err(Diagnostic::internal(format!(
            "builtin '{name}' requires a array/buffer symbol variable as first argument in {context}"
        ))),
    }
}

pub(in crate::orc_backend) fn ensure_internal_buffer_2d_call_positional_arity(
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

pub(in crate::orc_backend) fn parse_array_len_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "len" {
        return None;
    }
    Some(base)
}

pub(in crate::orc_backend) fn parse_buffer_chans_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "chans" {
        return None;
    }
    Some(base)
}

pub(in crate::orc_backend) fn parse_buffer_samplerate_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "samplerate" {
        return None;
    }
    Some(base)
}

pub(in crate::orc_backend) fn parse_unsafe_read_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "unsafe_read" {
        return None;
    }
    Some(base)
}

pub(in crate::orc_backend) fn parse_unsafe_write_instance_base(name: &str) -> Option<&str> {
    let (base, method) = name.rsplit_once('.')?;
    if base.is_empty() || method != "unsafe_write" {
        return None;
    }
    Some(base)
}

pub(in crate::orc_backend) fn ensure_builtin_instance_call_no_args(
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

pub(in crate::orc_backend) fn checked_len_const_i32(
    len: usize,
    context: &str,
) -> Result<u64, Diagnostic> {
    if len > i32::MAX as usize {
        return Err(Diagnostic::internal(format!(
            "array length {len} exceeds i32 range in {context}"
        )));
    }
    Ok(len as u64)
}

pub(in crate::orc_backend) fn parse_proc_index_buffer_selector_args<'a>(
    arg_expr: &'a Expr,
    callee_name: &str,
    context: &str,
) -> Result<(&'a Expr, Vec<&'a Expr>), Diagnostic> {
    let Expr::UserCall {
        name,
        type_args,
        args,
        ..
    } = arg_expr
    else {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in {context}"
        )));
    };
    if name != PROC_INDEX_BUFFER_SELECT_SENTINEL {
        return Err(Diagnostic::internal(format!(
            "function '{callee_name}' buffer argument must be a buffer symbol variable in {context}"
        )));
    }
    if !type_args.is_empty() {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' does not support type arguments in {context}"
        )));
    }

    let mut index_expr = None::<&Expr>;
    let mut slot_exprs = Vec::<&Expr>::new();
    for arg in args {
        match arg.name.as_deref() {
            Some(PROC_INDEX_BASE_ARG) => {
                if !matches!(arg.expr, Expr::Var { .. }) {
                    return Err(Diagnostic::internal(format!(
                        "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' expects '{PROC_INDEX_BASE_ARG}' as identifier in {context}"
                    )));
                }
            }
            Some(PROC_INDEX_EXPR_ARG) => {
                index_expr = Some(&arg.expr);
            }
            Some(other) => {
                return Err(Diagnostic::internal(format!(
                    "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' has unsupported named argument '{other}' in {context}"
                )));
            }
            None => slot_exprs.push(&arg.expr),
        }
    }

    let index_expr = index_expr.ok_or_else(|| {
        Diagnostic::internal(format!(
            "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' is missing '{PROC_INDEX_EXPR_ARG}' in {context}"
        ))
    })?;
    if slot_exprs.is_empty() {
        return Err(Diagnostic::internal(format!(
            "internal builtin '{PROC_INDEX_BUFFER_SELECT_SENTINEL}' requires at least one slot buffer argument in {context}"
        )));
    }
    Ok((index_expr, slot_exprs))
}

pub(in crate::orc_backend) unsafe fn select_def_value_by_slot_index(
    ctx: &mut DefLoweringCtx<'_>,
    slot_index: LLVMValueRef,
    candidates: &[LLVMValueRef],
    select_name: &[u8],
    context: &str,
) -> Result<LLVMValueRef, Diagnostic> {
    let Some(first) = candidates.first().copied() else {
        return Err(Diagnostic::internal(format!(
            "missing selector candidates in {context}"
        )));
    };
    let elem_ty = LLVMTypeOf(first);
    if candidates
        .iter()
        .any(|candidate| LLVMTypeOf(*candidate) != elem_ty)
    {
        return Err(Diagnostic::internal(format!(
            "selector candidates have mismatched LLVM types in {context}"
        )));
    }
    let mut selected = first;
    for (slot_idx, candidate) in candidates.iter().enumerate().skip(1) {
        let is_match = LLVMBuildICmp(
            ctx.builder,
            llvm_sys::LLVMIntPredicate::LLVMIntEQ,
            slot_index,
            LLVMConstInt(ctx.i32_ty, slot_idx as u64, 0),
            b"idx_sel_match\0".as_ptr().cast(),
        );
        selected = LLVMBuildSelect(
            ctx.builder,
            is_match,
            *candidate,
            selected,
            select_name.as_ptr().cast(),
        );
    }
    debug_assert_eq!(LLVMTypeOf(selected), elem_ty);
    Ok(selected)
}

pub(in crate::orc_backend) fn local_data_alias_len(alias: &LocalArrayAlias) -> usize {
    match alias {
        LocalArrayAlias::Primitive { len, .. } | LocalArrayAlias::Struct { len, .. } => *len,
    }
}

pub(in crate::orc_backend) fn lookup_orc_data_symbol_len(
    ctx: &LoweringCtx<'_>,
    base: &str,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Option<usize> {
    if let Some(alias) = local_array_aliases.get(base) {
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
    if let Some(len) = ctx.array_len.get(base) {
        return Some(*len);
    }
    ctx.array_struct_len.get(base).copied()
}

pub(in crate::orc_backend) fn lookup_def_data_symbol_len(
    ctx: &DefLoweringCtx<'_>,
    base: &str,
) -> Option<usize> {
    if let Some(alias) = ctx.local_array_aliases.get(base) {
        return Some(local_data_alias_len(alias));
    }
    ctx.array_len.get(base).copied()
}

pub(in crate::orc_backend) fn is_orc_builtin_data_len_receiver(
    ctx: &LoweringCtx<'_>,
    base: &str,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> bool {
    ctx.array_len_values.contains_key(base)
        || lookup_orc_data_symbol_len(ctx, base, local_array_aliases).is_some()
        || ctx.buffer_index.contains_key(base)
}

pub(in crate::orc_backend) fn is_orc_builtin_buffer_chans_receiver(
    ctx: &LoweringCtx<'_>,
    base: &str,
) -> bool {
    ctx.buffer_index.contains_key(base)
}

pub(in crate::orc_backend) fn is_orc_builtin_unsafe_data_receiver(
    ctx: &LoweringCtx<'_>,
    base: &str,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> bool {
    local_array_aliases.contains_key(base)
        || ctx.input_arrays.contains_key(base)
        || ctx.param_arrays.contains_key(base)
        || ctx.output_arrays.contains_key(base)
        || ctx.array_len.contains_key(base)
        || ctx.array_struct_len.contains_key(base)
        || ctx.buffer_index.contains_key(base)
}

pub(in crate::orc_backend) fn is_def_builtin_data_len_receiver(
    ctx: &DefLoweringCtx<'_>,
    base: &str,
) -> bool {
    lookup_def_data_symbol_len(ctx, base).is_some() || ctx.buffer_params.contains_key(base)
}

pub(in crate::orc_backend) fn is_def_builtin_buffer_chans_receiver(
    ctx: &DefLoweringCtx<'_>,
    base: &str,
) -> bool {
    ctx.buffer_params.contains_key(base)
}

pub(in crate::orc_backend) fn is_def_builtin_unsafe_data_receiver(
    ctx: &DefLoweringCtx<'_>,
    base: &str,
) -> bool {
    ctx.local_array_aliases.contains_key(base)
        || ctx.array_len.contains_key(base)
        || ctx.buffer_params.contains_key(base)
}

pub(in crate::orc_backend) unsafe fn lower_orc_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
    local_array_aliases: &HashMap<String, LocalArrayAlias>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if let Some(len_val) = ctx.array_len_values.get(base).copied() {
        return Ok(OrcValue {
            value: len_val,
            ty: PrimitiveType::I32,
        });
    }
    if let Some(len) = lookup_orc_data_symbol_len(ctx, base, local_array_aliases) {
        let len_const = checked_len_const_i32(len, "ORC expression lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if ctx.buffer_index.contains_key(base) {
        return Ok(OrcValue {
            value: load_orc_buffer_frames_i32(ctx, base)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a array or buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

pub(in crate::orc_backend) unsafe fn lower_orc_buffer_chans_call(
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

pub(in crate::orc_backend) unsafe fn lower_orc_buffer_samplerate_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut LoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "ORC expression lowering")?;
    if ctx.buffer_index.contains_key(base) {
        return Ok(OrcValue {
            value: load_orc_buffer_samplerate_f32(ctx, base)?,
            ty: PrimitiveType::F32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a buffer symbol receiver in ORC expression lowering, got '{base}'"
    )))
}

pub(in crate::orc_backend) unsafe fn lower_def_data_len_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    if let Some(len_val) = ctx.array_len_values.get(base) {
        return Ok(OrcValue {
            value: *len_val,
            ty: PrimitiveType::I32,
        });
    }
    if let Some(len) = lookup_def_data_symbol_len(ctx, base) {
        let len_const = checked_len_const_i32(len, "def lowering")?;
        return Ok(OrcValue {
            value: LLVMConstInt(ctx.i32_ty, len_const, 0),
            ty: PrimitiveType::I32,
        });
    }
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        return Ok(OrcValue {
            value: load_def_buffer_frames_i32(ctx, base, &info)?,
            ty: PrimitiveType::I32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a array or buffer symbol receiver in def lowering, got '{base}'"
    )))
}

pub(in crate::orc_backend) unsafe fn lower_def_buffer_samplerate_call(
    method_name: &str,
    base: &str,
    args: &[CallArg],
    ctx: &mut DefLoweringCtx<'_>,
) -> Result<OrcValue, Diagnostic> {
    ensure_builtin_instance_call_no_args(method_name, args, "def lowering")?;
    if let Some(info) = ctx.buffer_params.get(base).cloned() {
        return Ok(OrcValue {
            value: load_def_buffer_samplerate_f32(ctx, base, &info)?,
            ty: PrimitiveType::F32,
        });
    }
    Err(Diagnostic::internal(format!(
        "builtin method '{method_name}' requires a buffer symbol receiver in def lowering, got '{base}'"
    )))
}

pub(in crate::orc_backend) fn builtin_constant_value_and_type(
    name: &str,
    sample_rate: f32,
    block_size: f32,
) -> Option<(PrimitiveType, f64)> {
    builtin_constant_typed_value(name, sample_rate, block_size)
        .map(|value| (value.ty(), value.as_f64()))
}

#[derive(Clone, Copy, Debug)]
pub(in crate::orc_backend) enum ConstDefaultValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    Bool(bool),
}

impl ConstDefaultValue {
    pub(in crate::orc_backend) fn ty(self) -> PrimitiveType {
        match self {
            Self::F32(_) => PrimitiveType::F32,
            Self::F64(_) => PrimitiveType::F64,
            Self::I32(_) => PrimitiveType::I32,
            Self::I64(_) => PrimitiveType::I64,
            Self::Bool(_) => PrimitiveType::Bool,
        }
    }

    pub(in crate::orc_backend) fn as_f64(self) -> f64 {
        match self {
            Self::F32(value) => value as f64,
            Self::F64(value) => value,
            Self::I32(value) => value as f64,
            Self::I64(value) => value as f64,
            Self::Bool(value) => {
                if value {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    fn as_i64_cast(self) -> i64 {
        match self {
            Self::F32(value) => value as i64,
            Self::F64(value) => value as i64,
            Self::I32(value) => value as i64,
            Self::I64(value) => value,
            Self::Bool(value) => {
                if value {
                    1
                } else {
                    0
                }
            }
        }
    }

    fn truthy(self) -> bool {
        match self {
            Self::F32(value) => value != 0.0,
            Self::F64(value) => value != 0.0,
            Self::I32(value) => value != 0,
            Self::I64(value) => value != 0,
            Self::Bool(value) => value,
        }
    }
}

fn builtin_constant_typed_value(
    name: &str,
    sample_rate: f32,
    block_size: f32,
) -> Option<ConstDefaultValue> {
    match name {
        "PI" | "pi" => Some(ConstDefaultValue::F64(std::f64::consts::PI)),
        "TWO_PI" | "TWOPI" | "two_pi" | "twopi" => {
            Some(ConstDefaultValue::F64(2.0 * std::f64::consts::PI))
        }
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => {
            Some(ConstDefaultValue::F32(sample_rate))
        }
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => {
            Some(ConstDefaultValue::I32(block_size as i32))
        }
        _ => None,
    }
}

fn merge_const_integer_types(lhs: PrimitiveType, rhs: PrimitiveType) -> Option<PrimitiveType> {
    use omni_frontend::PrimitiveType::*;
    match (lhs, rhs) {
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
        _ => None,
    }
}

pub(in crate::orc_backend) fn infer_const_default_expr_type(
    expr: &Expr,
) -> Result<PrimitiveType, Diagnostic> {
    match expr {
        Expr::Number { .. } => Ok(PrimitiveType::F64),
        Expr::Int { .. } => Ok(PrimitiveType::I64),
        Expr::Bool { .. } => Ok(PrimitiveType::Bool),
        Expr::Var { name, .. } => builtin_constant_value_and_type(name, 0.0, 0.0)
            .map(|(ty, _)| ty)
            .ok_or_else(|| {
                Diagnostic::internal(format!(
                    "default expression uses non-constant symbol '{name}' in codegen"
                ))
            }),
        Expr::Cast { to, .. } => Ok(*to),
        Expr::UnaryNot { .. } | Expr::Logical { .. } | Expr::Compare { .. } => {
            Ok(PrimitiveType::Bool)
        }
        Expr::UnaryBitNot { expr, .. } => {
            let inner_ty = infer_const_default_expr_type(expr)?;
            merge_const_integer_types(inner_ty, inner_ty).ok_or_else(|| {
                Diagnostic::internal(format!(
                    "default bitwise-not expression requires integer operand, got {:?}",
                    inner_ty
                ))
            })
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let lhs_ty = infer_const_default_expr_type(lhs)?;
            let rhs_ty = infer_const_default_expr_type(rhs)?;
            match op {
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => {
                    merge_const_integer_types(lhs_ty, rhs_ty).ok_or_else(|| {
                        Diagnostic::internal(format!(
                            "default bitwise expression requires integer operands, got {:?} and {:?}",
                            lhs_ty, rhs_ty
                        ))
                    })
                }
                _ => merge_numeric_primitive(lhs_ty, rhs_ty).ok_or_else(|| {
                    Diagnostic::internal(format!(
                        "default binary expression requires numeric operands, got {:?} and {:?}",
                        lhs_ty, rhs_ty
                    ))
                }),
            }
        }
        _ => Err(Diagnostic::internal(
            "default expression must be compile-time constant in codegen",
        )),
    }
}

pub(in crate::orc_backend) fn eval_const_default_expr_typed(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<ConstDefaultValue, Diagnostic> {
    match expr {
        Expr::Number { value: v, .. } => Ok(ConstDefaultValue::F64(*v)),
        Expr::Int { value: v, .. } => Ok(ConstDefaultValue::I64(*v)),
        Expr::Bool { value: v, .. } => Ok(ConstDefaultValue::Bool(*v)),
        Expr::Var { name, .. } => builtin_constant_typed_value(name, sample_rate, block_size)
            .ok_or_else(|| {
                Diagnostic::internal(format!(
                    "default expression uses non-constant symbol '{name}' in codegen"
                ))
            }),
        Expr::Cast { to, expr, .. } => {
            let value = eval_const_default_expr_typed(expr, sample_rate, block_size)?;
            Ok(match to {
                PrimitiveType::F32 => ConstDefaultValue::F32(value.as_f64() as f32),
                PrimitiveType::F64 => ConstDefaultValue::F64(value.as_f64()),
                PrimitiveType::I32 => ConstDefaultValue::I32(value.as_i64_cast() as i32),
                PrimitiveType::I64 => ConstDefaultValue::I64(value.as_i64_cast()),
                PrimitiveType::Bool => ConstDefaultValue::Bool(value.truthy()),
            })
        }
        Expr::UnaryNot { expr, .. } => {
            let value = eval_const_default_expr_typed(expr, sample_rate, block_size)?;
            Ok(ConstDefaultValue::Bool(!value.truthy()))
        }
        Expr::UnaryBitNot { expr, .. } => {
            let ty = infer_const_default_expr_type(expr)?;
            Ok(match ty {
                PrimitiveType::I32 => ConstDefaultValue::I32(
                    !(eval_const_default_expr_typed(expr, sample_rate, block_size)?.as_i64_cast()
                        as i32),
                ),
                PrimitiveType::I64 => ConstDefaultValue::I64(
                    !eval_const_default_expr_typed(expr, sample_rate, block_size)?.as_i64_cast(),
                ),
                _ => {
                    return Err(Diagnostic::internal(format!(
                        "default bitwise-not expression requires integer operand, got {:?}",
                        ty
                    )))
                }
            })
        }
        Expr::Logical { op, lhs, rhs, .. } => {
            let lhs_value = eval_const_default_expr_typed(lhs, sample_rate, block_size)?;
            match op {
                LogicalOp::And => {
                    if !lhs_value.truthy() {
                        Ok(ConstDefaultValue::Bool(false))
                    } else {
                        let rhs_value =
                            eval_const_default_expr_typed(rhs, sample_rate, block_size)?;
                        Ok(ConstDefaultValue::Bool(rhs_value.truthy()))
                    }
                }
                LogicalOp::Or => {
                    if lhs_value.truthy() {
                        Ok(ConstDefaultValue::Bool(true))
                    } else {
                        let rhs_value =
                            eval_const_default_expr_typed(rhs, sample_rate, block_size)?;
                        Ok(ConstDefaultValue::Bool(rhs_value.truthy()))
                    }
                }
            }
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            let result_ty = infer_const_default_expr_type(expr)?;
            let lhs_value = eval_const_default_expr_typed(lhs, sample_rate, block_size)?;
            let rhs_value = eval_const_default_expr_typed(rhs, sample_rate, block_size)?;
            match result_ty {
                PrimitiveType::F32 => {
                    let lhs_value = lhs_value.as_f64();
                    let rhs_value = rhs_value.as_f64();
                    let result = match op {
                        BinaryOp::Add => lhs_value + rhs_value,
                        BinaryOp::Sub => lhs_value - rhs_value,
                        BinaryOp::Mul => lhs_value * rhs_value,
                        BinaryOp::Div => lhs_value / rhs_value,
                        BinaryOp::Mod => lhs_value % rhs_value,
                        _ => unreachable!("bitwise expression must be integer"),
                    };
                    Ok(ConstDefaultValue::F32(result as f32))
                }
                PrimitiveType::F64 => {
                    let lhs_value = lhs_value.as_f64();
                    let rhs_value = rhs_value.as_f64();
                    let result = match op {
                        BinaryOp::Add => lhs_value + rhs_value,
                        BinaryOp::Sub => lhs_value - rhs_value,
                        BinaryOp::Mul => lhs_value * rhs_value,
                        BinaryOp::Div => lhs_value / rhs_value,
                        BinaryOp::Mod => lhs_value % rhs_value,
                        _ => unreachable!("bitwise expression must be integer"),
                    };
                    Ok(ConstDefaultValue::F64(result))
                }
                PrimitiveType::I32 => {
                    let lhs_value = lhs_value.as_i64_cast() as i32;
                    let rhs_value = rhs_value.as_i64_cast() as i32;
                    let result = match op {
                        BinaryOp::Add => lhs_value.wrapping_add(rhs_value),
                        BinaryOp::Sub => lhs_value.wrapping_sub(rhs_value),
                        BinaryOp::Mul => lhs_value.wrapping_mul(rhs_value),
                        BinaryOp::Div => lhs_value / rhs_value,
                        BinaryOp::Mod => lhs_value % rhs_value,
                        BinaryOp::BitAnd => lhs_value & rhs_value,
                        BinaryOp::BitOr => lhs_value | rhs_value,
                        BinaryOp::BitXor => lhs_value ^ rhs_value,
                        BinaryOp::ShiftLeft => lhs_value.wrapping_shl(rhs_value as u32),
                        BinaryOp::ShiftRight => lhs_value.wrapping_shr(rhs_value as u32),
                    };
                    Ok(ConstDefaultValue::I32(result))
                }
                PrimitiveType::I64 => {
                    let lhs_value = lhs_value.as_i64_cast();
                    let rhs_value = rhs_value.as_i64_cast();
                    let result = match op {
                        BinaryOp::Add => lhs_value.wrapping_add(rhs_value),
                        BinaryOp::Sub => lhs_value.wrapping_sub(rhs_value),
                        BinaryOp::Mul => lhs_value.wrapping_mul(rhs_value),
                        BinaryOp::Div => lhs_value / rhs_value,
                        BinaryOp::Mod => lhs_value % rhs_value,
                        BinaryOp::BitAnd => lhs_value & rhs_value,
                        BinaryOp::BitOr => lhs_value | rhs_value,
                        BinaryOp::BitXor => lhs_value ^ rhs_value,
                        BinaryOp::ShiftLeft => lhs_value.wrapping_shl(rhs_value as u32),
                        BinaryOp::ShiftRight => lhs_value.wrapping_shr(rhs_value as u32),
                    };
                    Ok(ConstDefaultValue::I64(result))
                }
                PrimitiveType::Bool => Err(Diagnostic::internal(
                    "default binary expression cannot evaluate to bool",
                )),
            }
        }
        Expr::Compare { op, lhs, rhs, .. } => {
            let lhs_ty = infer_const_default_expr_type(lhs)?;
            let rhs_ty = infer_const_default_expr_type(rhs)?;
            let lhs_value = eval_const_default_expr_typed(lhs, sample_rate, block_size)?;
            let rhs_value = eval_const_default_expr_typed(rhs, sample_rate, block_size)?;
            let pred = if merge_const_integer_types(lhs_ty, rhs_ty).is_some() {
                let lhs_value = lhs_value.as_i64_cast();
                let rhs_value = rhs_value.as_i64_cast();
                match op {
                    CmpOp::Eq => lhs_value == rhs_value,
                    CmpOp::Ne => lhs_value != rhs_value,
                    CmpOp::Lt => lhs_value < rhs_value,
                    CmpOp::Le => lhs_value <= rhs_value,
                    CmpOp::Gt => lhs_value > rhs_value,
                    CmpOp::Ge => lhs_value >= rhs_value,
                }
            } else {
                let lhs_value = lhs_value.as_f64();
                let rhs_value = rhs_value.as_f64();
                match op {
                    CmpOp::Eq => lhs_value == rhs_value,
                    CmpOp::Ne => lhs_value != rhs_value,
                    CmpOp::Lt => lhs_value < rhs_value,
                    CmpOp::Le => lhs_value <= rhs_value,
                    CmpOp::Gt => lhs_value > rhs_value,
                    CmpOp::Ge => lhs_value >= rhs_value,
                }
            };
            Ok(ConstDefaultValue::Bool(pred))
        }
        _ => Err(Diagnostic::internal(
            "default expression must be compile-time constant in codegen",
        )),
    }
}

pub(in crate::orc_backend) fn eval_const_data_size_expr(
    expr: &Expr,
    sample_rate: f32,
    block_size: f32,
) -> Result<usize, Diagnostic> {
    match eval_const_default_expr_typed(expr, sample_rate, block_size)? {
        ConstDefaultValue::I32(value) => {
            if value <= 0 {
                return Err(Diagnostic::internal(
                    "array size expression must be greater than zero",
                ));
            }
            Ok(value as usize)
        }
        ConstDefaultValue::I64(value) => {
            if value <= 0 {
                return Err(Diagnostic::internal(
                    "array size expression must be greater than zero",
                ));
            }
            usize::try_from(value)
                .map_err(|_| Diagnostic::internal("array size expression exceeds supported range"))
        }
        ConstDefaultValue::F32(value) => {
            let value = value as f64;
            let truncated = value.trunc();
            if (value - truncated).abs() > 1e-6 {
                return Err(Diagnostic::internal(
                    "array size expression must evaluate to an integer constant",
                ));
            }
            if truncated <= 0.0 {
                return Err(Diagnostic::internal(
                    "array size expression must be greater than zero",
                ));
            }
            if truncated > usize::MAX as f64 {
                return Err(Diagnostic::internal(
                    "array size expression exceeds supported range",
                ));
            }
            Ok(truncated as usize)
        }
        ConstDefaultValue::F64(value) => {
            if !value.is_finite() {
                return Err(Diagnostic::internal(
                    "array size expression must evaluate to a finite constant",
                ));
            }
            let truncated = value.trunc();
            if (value - truncated).abs() > 1e-6 {
                return Err(Diagnostic::internal(
                    "array size expression must evaluate to an integer constant",
                ));
            }
            if truncated <= 0.0 {
                return Err(Diagnostic::internal(
                    "array size expression must be greater than zero",
                ));
            }
            if truncated > usize::MAX as f64 {
                return Err(Diagnostic::internal(
                    "array size expression exceeds supported range",
                ));
            }
            Ok(truncated as usize)
        }
        ConstDefaultValue::Bool(_) => Err(Diagnostic::internal(
            "array size expression must evaluate to an integer constant",
        )),
    }
}
pub(in crate::orc_backend) fn resolve_call_args_codegen<'a>(
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
