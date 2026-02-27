use super::*;

pub(super) fn mangle_user_fn_symbol(name: &str) -> Result<CString, Diagnostic> {
    CString::new(format!("omni_def_{name}"))
        .map_err(|_| Diagnostic::internal(format!("invalid function name '{name}'")))
}

pub(super) fn primitive_sig_code(ty: PrimitiveType) -> &'static str {
    match ty {
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Bool => "b1",
    }
}

pub(super) fn buffer_channel_sig_code(channels: &TypedBufferChannels) -> String {
    match channels {
        TypedBufferChannels::Mono => "m".to_owned(),
        TypedBufferChannels::Dynamic => "d".to_owned(),
        TypedBufferChannels::Static(ch) => format!("s{ch}"),
    }
}

pub(super) fn user_fn_mono_key(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
) -> String {
    if scalar_types.is_empty() && buffer_types.is_empty() && generic_type_args.is_empty() {
        return format!("{name}__mono");
    }
    let mut sig_parts = generic_type_args
        .iter()
        .map(|t| format!("gen_{}", primitive_sig_code(*t)))
        .collect::<Vec<_>>();
    sig_parts.extend(
        scalar_types
            .iter()
            .map(|t| primitive_sig_code(*t))
            .map(|s| s.to_owned())
            .collect::<Vec<_>>(),
    );
    sig_parts.extend(buffer_types.iter().map(|(elem_ty, channels)| {
        format!(
            "buf_{}_{}",
            primitive_sig_code(*elem_ty),
            buffer_channel_sig_code(channels)
        )
    }));
    let sig = sig_parts.join("_");
    format!("{name}__mono__{sig}")
}

pub(super) fn mangle_user_fn_symbol_mono(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
    sample_rate: f32,
    block_size: usize,
) -> Result<CString, Diagnostic> {
    let context_suffix = format!(
        "__sr_{:08x}__bs_{:08x}",
        sample_rate.to_bits(),
        (block_size as u32)
    );
    CString::new(format!(
        "omni_def_{}",
        user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args) + &context_suffix
    ))
    .map_err(|_| {
        Diagnostic::internal(format!(
            "invalid monomorphized function name for '{}'",
            name
        ))
    })
}

pub(super) fn default_scalar_signature(def: &TypedFunction) -> Vec<PrimitiveType> {
    def.param_kinds
        .iter()
        .filter_map(|k| match k {
            TypedFnParam::Scalar => Some(PrimitiveType::F32),
            TypedFnParam::Struct { .. } => None,
            TypedFnParam::Buffer { .. } => None,
        })
        .collect::<Vec<_>>()
}

pub(super) fn default_buffer_signature(
    def: &TypedFunction,
) -> Vec<(PrimitiveType, TypedBufferChannels)> {
    def.param_kinds
        .iter()
        .filter_map(|k| match k {
            TypedFnParam::Buffer { elem_ty, channels } => Some((*elem_ty, channels.clone())),
            _ => None,
        })
        .collect::<Vec<_>>()
}

pub(super) fn resolve_explicit_call_type_args_for_codegen(
    name: &str,
    context: &str,
    type_args: &[CallTypeArg],
) -> Result<Vec<PrimitiveType>, Diagnostic> {
    let mut out = Vec::<PrimitiveType>::with_capacity(type_args.len());
    for arg in type_args {
        match arg {
            CallTypeArg::Primitive(ty) => out.push(*ty),
            CallTypeArg::Generic(param) => {
                return Err(Diagnostic::internal(format!(
                    "function '{}' in {} has unresolved generic type argument '{}'; expected concrete primitive type",
                    name, context, param
                )));
            }
        }
    }
    Ok(out)
}

pub(super) fn apply_explicit_generic_type_args_for_call(
    registry: &UserFnRegistry,
    name: &str,
    explicit_type_args: &[PrimitiveType],
    scalar_types: &mut [PrimitiveType],
    context: &str,
) -> Result<(), Diagnostic> {
    let def = registry.defs.get(name).ok_or_else(|| {
        Diagnostic::internal(format!("unknown function '{}' in {}", name, context))
    })?;
    let expected = def.type_params.len();
    if expected == 0 {
        if !explicit_type_args.is_empty() {
            return Err(Diagnostic::internal(format!(
                "function '{}' in {} is not generic but call provided {} type args",
                name,
                context,
                explicit_type_args.len()
            )));
        }
        return Ok(());
    }
    if explicit_type_args.is_empty() {
        if expected > scalar_types.len() {
            return Err(Diagnostic::internal(format!(
                "function '{}' in {} has {} type params but only {} scalar params available for specialization",
                name,
                context,
                expected,
                scalar_types.len()
            )));
        }
        return Ok(());
    }
    if explicit_type_args.len() != expected {
        return Err(Diagnostic::internal(format!(
            "function '{}' in {} expects {} type args, got {}",
            name,
            context,
            expected,
            explicit_type_args.len()
        )));
    }
    if expected > scalar_types.len() {
        return Err(Diagnostic::internal(format!(
            "function '{}' in {} has {} type params but only {} scalar params available for specialization",
            name,
            context,
            expected,
            scalar_types.len()
        )));
    }
    for (idx, ty) in explicit_type_args.iter().enumerate() {
        scalar_types[idx] = *ty;
    }
    Ok(())
}

pub(super) fn collect_data_struct_bindings(
    struct_fields: &HashMap<String, Vec<TypedStructField>>,
    struct_name: &str,
    base: &str,
    len: usize,
    roots: &mut Vec<(String, String, usize)>,
    leaves: &mut Vec<(String, usize, PrimitiveType)>,
    stack: &mut Vec<String>,
) -> Result<(), Diagnostic> {
    if stack.iter().any(|s| s == struct_name) {
        return Err(Diagnostic::internal(format!(
            "recursive Data[Struct] layout encountered while lowering '{}'",
            struct_name
        )));
    }
    let fields = struct_fields.get(struct_name).ok_or_else(|| {
        Diagnostic::internal(format!(
            "unknown struct '{}' while collecting Data[Struct] bindings",
            struct_name
        ))
    })?;
    roots.push((base.to_owned(), struct_name.to_owned(), len));
    stack.push(struct_name.to_owned());
    for field in fields {
        let flat = format!("{base}.{}", field.name);
        match field.ty {
            TypedFieldType::Scalar(prim) => leaves.push((flat, len, prim)),
            TypedFieldType::Data(field_len) => {
                let nested_len = len.saturating_mul(field_len);
                if let Some(elem_struct) = &field.data_elem_struct {
                    collect_data_struct_bindings(
                        struct_fields,
                        elem_struct,
                        &flat,
                        nested_len,
                        roots,
                        leaves,
                        stack,
                    )?;
                } else {
                    leaves.push((
                        flat,
                        nested_len,
                        field.data_elem_ty.unwrap_or(PrimitiveType::F32),
                    ));
                }
            }
        }
    }
    stack.pop();
    Ok(())
}

pub(super) fn merge_inferred_def_return_types(
    lhs: PrimitiveType,
    rhs: PrimitiveType,
) -> Option<PrimitiveType> {
    use PrimitiveType::*;
    match (lhs, rhs) {
        (a, b) if a == b => Some(a),
        (Bool, _) | (_, Bool) => None,
        (F64, _) | (_, F64) => Some(F64),
        (F32, I64) | (I64, F32) => Some(F64),
        (F32, I32) | (I32, F32) => Some(F32),
        (F32, F32) => Some(F32),
        (I64, I32) | (I32, I64) | (I64, I64) => Some(I64),
        (I32, I32) => Some(I32),
    }
}

fn builtin_constant_symbol_type(name: &str) -> Option<PrimitiveType> {
    match name {
        "PI" | "TWO_PI" | "TWOPI" | "pi" | "two_pi" | "twopi" => Some(PrimitiveType::F64),
        "SAMPLE_RATE" | "SAMPLERATE" | "SR" | "sample_rate" | "samplerate" => {
            Some(PrimitiveType::F32)
        }
        "BLOCK_SIZE" | "BLOCKSIZE" | "BS" | "block_size" | "blocksize" => Some(PrimitiveType::I32),
        _ => None,
    }
}

pub(super) fn infer_specialized_expr_return_type(
    expr: &Expr,
    locals: &HashMap<String, PrimitiveType>,
    registry: &mut UserFnRegistry,
) -> Result<Option<PrimitiveType>, Diagnostic> {
    Ok(match expr {
        Expr::Number(_) => Some(PrimitiveType::F32),
        Expr::Int(v) => Some(if *v >= i32::MIN as i64 && *v <= i32::MAX as i64 {
            PrimitiveType::I32
        } else {
            PrimitiveType::I64
        }),
        Expr::Bool(_) => Some(PrimitiveType::Bool),
        Expr::ArrayLiteral(_) | Expr::DataCtor { .. } => None,
        Expr::Var(name) => builtin_constant_symbol_type(name).or_else(|| locals.get(name).copied()),
        Expr::Index { base, .. } => locals.get(base).copied().or(Some(PrimitiveType::F32)),
        Expr::Cast { to, .. } => Some(*to),
        Expr::UnaryNot { .. } | Expr::Compare { .. } | Expr::Logical { .. } => {
            Some(PrimitiveType::Bool)
        }
        Expr::Binary { lhs, rhs, .. } => {
            let l = infer_specialized_expr_return_type(lhs, locals, registry)?
                .unwrap_or(PrimitiveType::F32);
            let r = infer_specialized_expr_return_type(rhs, locals, registry)?
                .unwrap_or(PrimitiveType::F32);
            merge_inferred_def_return_types(l, r)
        }
        Expr::Call { func, args } => {
            let mut arg_tys = Vec::<PrimitiveType>::new();
            for arg in args {
                arg_tys.push(
                    infer_specialized_expr_return_type(arg, locals, registry)?
                        .unwrap_or(PrimitiveType::F32),
                );
            }
            match func {
                BuiltinFn::Abs => arg_tys.first().copied().or(Some(PrimitiveType::F32)),
                BuiltinFn::Min | BuiltinFn::Max => {
                    let lhs = arg_tys.first().copied().unwrap_or(PrimitiveType::F32);
                    let rhs = arg_tys.get(1).copied().unwrap_or(PrimitiveType::F32);
                    merge_inferred_def_return_types(lhs, rhs)
                }
                BuiltinFn::Pow => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
                _ => Some(if arg_tys.iter().any(|t| *t == PrimitiveType::F64) {
                    PrimitiveType::F64
                } else {
                    PrimitiveType::F32
                }),
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if parse_data_len_instance_base(name).is_some()
                || parse_buffer_chans_instance_base(name).is_some()
            {
                return Ok(Some(PrimitiveType::I32));
            }
            if matches!(
                name.as_str(),
                "__omni_buffer_read2" | "__omni_buffer_write2" | "unsafe_read" | "unsafe_write"
            ) {
                if let Some(CallArg {
                    expr: Expr::Var(base),
                    ..
                }) = args.first()
                {
                    if let Some(ty) = locals.get(base).copied() {
                        return Ok(Some(ty));
                    }
                }
                return Ok(Some(PrimitiveType::F32));
            }
            let Some(param_names) = registry.param_names.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let Some(param_defaults) = registry.param_defaults.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let Some(param_kinds) = registry.param_kinds.get(name).cloned() else {
                return Ok(Some(PrimitiveType::F32));
            };
            let forbid_self_named = param_names.first().map(String::as_str) == Some("self");
            let resolved = resolve_call_args_codegen(
                args,
                &param_names,
                &param_defaults,
                forbid_self_named,
                &format!("function '{name}' call in return type inference"),
            )
            .unwrap_or_else(|_| vec![None; param_names.len()]);
            let mut scalar_types = Vec::<PrimitiveType>::new();
            let mut buffer_types = Vec::<(PrimitiveType, TypedBufferChannels)>::new();
            for (idx, kind) in param_kinds.iter().enumerate() {
                match kind {
                    TypedFnParam::Scalar => {
                        let resolved_arg = resolved.get(idx).copied().flatten();
                        let ty = if let Some(arg_expr) = resolved_arg {
                            infer_specialized_expr_return_type(arg_expr, locals, registry)?
                                .unwrap_or(PrimitiveType::F32)
                        } else if let Some(default_expr) =
                            param_defaults.get(idx).and_then(|d| d.as_ref())
                        {
                            infer_specialized_expr_return_type(default_expr, locals, registry)?
                                .unwrap_or(PrimitiveType::F32)
                        } else {
                            PrimitiveType::F32
                        };
                        scalar_types.push(ty);
                    }
                    TypedFnParam::Buffer { elem_ty, channels } => {
                        buffer_types.push((*elem_ty, channels.clone()));
                    }
                    TypedFnParam::Struct { .. } => {}
                }
            }
            let explicit_type_args = resolve_explicit_call_type_args_for_codegen(
                name,
                "return type inference",
                type_args,
            )?;
            apply_explicit_generic_type_args_for_call(
                registry,
                name,
                &explicit_type_args,
                &mut scalar_types,
                "return type inference",
            )?;
            Some(infer_specialized_def_return_type(
                name,
                &scalar_types,
                &buffer_types,
                &explicit_type_args,
                registry,
            )?)
        }
    })
}

pub(super) fn infer_specialized_stmt_returns(
    stmts: &[Stmt],
    locals: &mut HashMap<String, PrimitiveType>,
    registry: &mut UserFnRegistry,
    out: &mut Vec<PrimitiveType>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                target: AssignTarget::Var(name),
                decl_ty,
                expr,
                ..
            } => {
                if name.contains('.') || matches!(expr, Expr::DataCtor { .. }) {
                    continue;
                }
                let inferred = infer_specialized_expr_return_type(expr, locals, registry)?
                    .unwrap_or(PrimitiveType::F32);
                let target_ty = (*decl_ty)
                    .or_else(|| locals.get(name).copied())
                    .unwrap_or(inferred);
                locals.entry(name.clone()).or_insert(target_ty);
            }
            Stmt::Assign {
                target: AssignTarget::Index { .. },
                ..
            }
            | Stmt::Expr { .. } => {}
            Stmt::Return { expr, .. } => {
                let ty = infer_specialized_expr_return_type(expr, locals, registry)?
                    .unwrap_or(PrimitiveType::F32);
                out.push(ty);
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                let mut then_locals = locals.clone();
                let mut else_locals = locals.clone();
                infer_specialized_stmt_returns(then_branch, &mut then_locals, registry, out)?;
                infer_specialized_stmt_returns(else_branch, &mut else_locals, registry, out)?;
                let mut merged = locals.clone();
                for (name, then_ty) in &then_locals {
                    if let Some(else_ty) = else_locals.get(name) {
                        if then_ty == else_ty {
                            merged.insert(name.clone(), *then_ty);
                        }
                    }
                }
                *locals = merged;
            }
            Stmt::For { var, body, .. } => {
                let mut loop_locals = locals.clone();
                loop_locals.insert(var.clone(), PrimitiveType::I32);
                infer_specialized_stmt_returns(body, &mut loop_locals, registry, out)?;
            }
            Stmt::While { body, .. } => {
                let mut loop_locals = locals.clone();
                infer_specialized_stmt_returns(body, &mut loop_locals, registry, out)?;
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn infer_specialized_def_return_type(
    name: &str,
    scalar_types: &[PrimitiveType],
    buffer_types: &[(PrimitiveType, TypedBufferChannels)],
    generic_type_args: &[PrimitiveType],
    registry: &mut UserFnRegistry,
) -> Result<PrimitiveType, Diagnostic> {
    let key = user_fn_mono_key(name, scalar_types, buffer_types, generic_type_args);
    if let Some(ret_ty) = registry.mono_return_tys.get(&key).copied() {
        return Ok(ret_ty);
    }
    if !registry.return_in_progress.insert(key.clone()) {
        return Ok(registry
            .base_return_tys
            .get(name)
            .copied()
            .unwrap_or(PrimitiveType::F32));
    }

    let out = (|| -> Result<PrimitiveType, Diagnostic> {
        let def = registry
            .defs
            .get(name)
            .ok_or_else(|| {
                Diagnostic::internal(format!("unknown function '{}' for return inference", name))
            })?
            .clone();

        let mut locals = HashMap::<String, PrimitiveType>::new();
        let mut scalar_idx = 0usize;
        for (param_name, kind) in def.params.iter().zip(def.param_kinds.iter()) {
            if matches!(kind, TypedFnParam::Scalar) {
                let param_ty = scalar_types
                    .get(scalar_idx)
                    .copied()
                    .unwrap_or(PrimitiveType::F32);
                scalar_idx += 1;
                locals.insert(param_name.clone(), param_ty);
            }
        }

        let mut returns = Vec::<PrimitiveType>::new();
        infer_specialized_stmt_returns(&def.body, &mut locals, registry, &mut returns)?;
        let mut it = returns.into_iter();
        let Some(mut ret_ty) = it.next() else {
            return Ok(registry
                .base_return_tys
                .get(name)
                .copied()
                .unwrap_or(PrimitiveType::F32));
        };
        for ty in it {
            let Some(merged) = merge_inferred_def_return_types(ret_ty, ty) else {
                return Ok(PrimitiveType::F32);
            };
            ret_ty = merged;
        }
        Ok(ret_ty)
    })();

    registry.return_in_progress.remove(&key);
    let ret_ty = out?;
    registry.mono_return_tys.insert(key, ret_ty);
    Ok(ret_ty)
}
