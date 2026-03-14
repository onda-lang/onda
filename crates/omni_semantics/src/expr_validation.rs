use crate::*;

fn push_expr_error(errors: &mut Vec<Diagnostic>, expr: &Expr, message: impl Into<String>) {
    errors.push(Diagnostic::semantic_span(message, expr.loc()));
}

fn push_loc_error(errors: &mut Vec<Diagnostic>, loc: SourceLoc, message: impl Into<String>) {
    errors.push(Diagnostic::semantic_span(message, loc));
}

fn init_buffer_runtime_message(what: &str) -> String {
    format!(
        "{what} is not allowed in init; buffer bindings are runtime-only and must be used in block, sample, or def scopes"
    )
}

pub(crate) fn validate_expr(expr: &Expr, env: ExprEnv<'_>, errors: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::ArrayLiteral { values, .. } => {
            for value in values {
                validate_expr(value, env, errors);
            }
            push_expr_error(
                errors,
                expr,
                "array literals are only allowed in typed array declarations and parameter defaults",
            );
        }
        Expr::Var { name, .. } => {
            if is_builtin_constant_name(name) {
                return;
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                if let Some((struct_name, owner_kind)) = env
                    .param_structs
                    .get(base)
                    .map(|s| (s.as_str(), "parameter"))
                    .or_else(|| {
                        env.struct_instances
                            .get(base)
                            .map(|s| (s.as_str(), "instance"))
                    })
                {
                    let Some(_fields) = env.struct_defs.get(struct_name) else {
                        push_expr_error(
                            errors,
                            expr,
                            format!("unknown struct type '{}'", struct_name),
                        );
                        return;
                    };
                    let Some(field_decl) =
                        resolve_struct_field_decl(struct_name, field, env.struct_defs)
                    else {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "struct {} '{}' (type '{}') has no field '{}'",
                                owner_kind, base, struct_name, field
                            ),
                        );
                        return;
                    };
                    if matches!(field_decl.ty, TypedFieldType::Array(_)) {
                        push_expr_error(
                            errors,
                            expr,
                            format!("array field '{}.{}' must be indexed", base, field),
                        );
                    }
                    return;
                }

                let flat = format!("{base}.{field}");
                if env.array_vars.contains_key(&flat) {
                    push_expr_error(
                        errors,
                        expr,
                        format!("array symbol '{flat}' must be indexed"),
                    );
                    return;
                }
                if !env.known_scalars.contains(&flat)
                    && !env.locals.contains(&flat)
                    && !env.outputs.contains(&flat)
                {
                    push_expr_error(
                        errors,
                        expr,
                        format!("unknown symbol '{flat}' in expression"),
                    );
                }
                return;
            }

            if env.param_structs.contains_key(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("struct parameter '{}' must be accessed via fields", name),
                );
                return;
            }
            if env.struct_instances.contains_key(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("struct instance '{name}' must be accessed via fields"),
                );
                return;
            }
            if env.array_vars.contains_key(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("array symbol '{name}' must be indexed"),
                );
                return;
            }
            if has_declared_buffer_symbol_info(env.declared_symbols, name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("buffer symbol '{name}' must be indexed"),
                );
                return;
            }
            if !env.known_scalars.contains(name)
                && !env.locals.contains(name)
                && !env.outputs.contains(name)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!("unknown symbol '{name}' in expression"),
                );
            }
        }
        Expr::Index { base, index, .. } => {
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some((struct_name, owner_kind)) = env
                    .param_structs
                    .get(root)
                    .map(|s| (s.as_str(), "parameter"))
                    .or_else(|| {
                        env.struct_instances
                            .get(root)
                            .map(|s| (s.as_str(), "instance"))
                    })
                {
                    let Some(_fields) = env.struct_defs.get(struct_name) else {
                        push_expr_error(
                            errors,
                            expr,
                            format!("unknown struct type '{}'", struct_name),
                        );
                        return;
                    };
                    let Some(field_decl) =
                        resolve_struct_field_decl(struct_name, field, env.struct_defs)
                    else {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "struct {} '{}' (type '{}') has no field '{}'",
                                owner_kind, root, struct_name, field
                            ),
                        );
                        return;
                    };
                    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "field '{}.{}' is not array and cannot be indexed",
                                root, field
                            ),
                        );
                    }
                    validate_expr(index, env, errors);
                    return;
                }
            }
            if env.scope == ScopeKind::Init
                && has_declared_buffer_symbol_info(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    init_buffer_runtime_message(&format!("buffer indexing '{}[...]'", base)),
                );
            }
            if !env.array_vars.contains_key(base)
                && !has_declared_buffer_symbol_info(env.declared_symbols, base)
                && !is_declared_struct_array_root_symbol(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!("indexed expression '{base}[...]' is not a array/buffer symbol"),
                );
            } else if is_declared_multichannel_buffer_info(env.declared_symbols, base) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "indexed expression '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                );
            }
            validate_expr(index, env, errors);
        }
        Expr::Slice {
            base, start, end, ..
        } => {
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some((struct_name, owner_kind)) = env
                    .param_structs
                    .get(root)
                    .map(|s| (s.as_str(), "parameter"))
                    .or_else(|| {
                        env.struct_instances
                            .get(root)
                            .map(|s| (s.as_str(), "instance"))
                    })
                {
                    let Some(_fields) = env.struct_defs.get(struct_name) else {
                        push_expr_error(
                            errors,
                            expr,
                            format!("unknown struct type '{}'", struct_name),
                        );
                        return;
                    };
                    let Some(field_decl) =
                        resolve_struct_field_decl(struct_name, field, env.struct_defs)
                    else {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "struct {} '{}' (type '{}') has no field '{}'",
                                owner_kind, root, struct_name, field
                            ),
                        );
                        return;
                    };
                    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "field '{}.{}' is not array and cannot be sliced",
                                root, field
                            ),
                        );
                    }
                }
            } else if env.scope == ScopeKind::Init
                && has_declared_buffer_symbol_info(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    init_buffer_runtime_message(&format!("buffer slicing '{}[...]'", base)),
                );
            } else if !env.array_vars.contains_key(base)
                && !has_declared_buffer_symbol_info(env.declared_symbols, base)
                && !is_declared_struct_array_root_symbol(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!("slice expression '{base}[...]' is not an array/buffer symbol"),
                );
            } else if is_declared_multichannel_buffer_info(env.declared_symbols, base) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "slice expression '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                );
            }
            if let Some(start) = start {
                validate_expr(start, env, errors);
            }
            if let Some(end) = end {
                validate_expr(end, env, errors);
            }
        }
        Expr::ArrayCtor { init, .. } => {
            if !env.allow_array_ctor {
                push_expr_error(
                    errors,
                    expr,
                    "array[...] constructor is only allowed in init assignments",
                );
            }
            if let Some(values) = init {
                for value in values {
                    validate_expr(value, env, errors);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            validate_expr(expr, env, errors);
        }
        Expr::Logical { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
        Expr::Compare { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
        Expr::Call { func, args, .. } => {
            for arg in args {
                validate_expr(arg, env, errors);
            }
            let expected = builtin_arity(*func);
            if args.len() != expected {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "builtin '{}' expects {expected} positional arguments, got {}",
                        builtin_name(*func),
                        args.len()
                    ),
                );
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
            ..
        } => {
            if is_builtin_unsafe_data_fn(name) {
                validate_unsafe_data_builtin_call(name, args, env, expr.loc(), errors);
                return;
            }
            if is_internal_buffer_2d_fn(name) {
                validate_internal_buffer_2d_call(name, args, env, expr.loc(), errors);
                return;
            }
            if name == PROC_INDEX_CALL_SENTINEL
                || name
                    .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
                    .map(|s| s == PROC_INDEX_CALL_SENTINEL)
                    .unwrap_or(false)
                || split_simple_field_path(name)
                    .map(|(base, _)| base == PROC_INDEX_CALL_SENTINEL)
                    .unwrap_or(false)
            {
                validate_internal_proc_index_call(name, args, env, expr.loc(), errors);
                return;
            }
            if name == PROC_INDEX_BUFFER_SELECT_SENTINEL {
                validate_internal_proc_index_buffer_select_call(
                    name,
                    args,
                    env,
                    expr.loc(),
                    errors,
                );
                return;
            }
            if !env.fn_signatures.contains_key(name) {
                if let Some(base) = parse_array_len_instance_base(name) {
                    if is_builtin_len_receiver(base, env) {
                        validate_data_len_builtin_call(name, base, args, env, expr.loc(), errors);
                        return;
                    }
                }
                if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_chans_builtin_call(
                            name,
                            base,
                            args,
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
                if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_samplerate_builtin_call(
                            name,
                            base,
                            args,
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
                if let Some(base) = parse_unsafe_read_instance_base(name) {
                    if is_builtin_unsafe_data_receiver(base, env) {
                        let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                        method_args.push(CallArg {
                            name: None,
                            expr: Expr::var(base.to_owned()),
                        });
                        method_args.extend(args.iter().cloned());
                        validate_unsafe_data_builtin_call(
                            "unsafe_read",
                            &method_args,
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
                if let Some(base) = parse_unsafe_write_instance_base(name) {
                    if is_builtin_unsafe_data_receiver(base, env) {
                        let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                        method_args.push(CallArg {
                            name: None,
                            expr: Expr::var(base.to_owned()),
                        });
                        method_args.extend(args.iter().cloned());
                        validate_unsafe_data_builtin_call(
                            "unsafe_write",
                            &method_args,
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
            }
            if let Some(sig) = env.fn_signatures.get(name) {
                if sig.type_params.is_empty() {
                    if !type_args.is_empty() {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "function '{}' is not generic and cannot take type arguments",
                                name
                            ),
                        );
                    }
                } else if !type_args.is_empty() && type_args.len() != sig.type_params.len() {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "function '{}' expects {} type arguments, got {}",
                            name,
                            sig.type_params.len(),
                            type_args.len()
                        ),
                    );
                }

                let forbid_self_named = sig.params.first().map(String::as_str) == Some("self");
                let resolved = resolve_call_args_at(
                    args,
                    &sig.params,
                    &sig.defaults,
                    forbid_self_named,
                    false,
                    &format!("function '{name}' call"),
                    expr.loc(),
                    errors,
                );
                for (idx, arg) in resolved.into_iter().enumerate() {
                    if let Some(arg) = arg {
                        let param_ty = sig.param_types.get(idx).and_then(|t| t.as_ref());
                        if let Some(FnParamType::Buffer(buffer_ty)) = param_ty {
                            validate_buffer_param_call_arg(
                                name,
                                idx,
                                &sig.params,
                                buffer_ty,
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            );
                            continue;
                        }
                        if matches!(
                            param_ty,
                            Some(FnParamType::Array(_))
                                | Some(FnParamType::ArrayGeneric(_))
                                | Some(FnParamType::BareBuffer)
                        ) {
                            if matches!(arg, Expr::Slice { .. }) {
                                validate_expr(arg, env, errors);
                            }
                            // Array and bare buffer params accept data-like args.
                            continue;
                        }
                        if param_ty.is_none() {
                            if let Expr::Var { name: v, .. } = arg {
                                if has_declared_buffer_symbol_info(env.declared_symbols, v) {
                                    continue;
                                }
                                if env.array_vars.contains_key(v) {
                                    continue;
                                }
                            }
                        }
                        if let Expr::Var { name: v, .. } = arg {
                            if env.struct_instances.contains_key(v)
                                || env.param_structs.contains_key(v)
                            {
                                continue;
                            }
                        }
                        if matches!(param_ty, Some(FnParamType::Struct(_)))
                            && is_internal_proc_helper_call(name)
                            && matches!(arg, Expr::Index { .. } | Expr::Var { .. })
                        {
                            continue;
                        }
                        validate_expr(arg, env, errors);
                    } else if let Some(default) = sig.defaults.get(idx).and_then(|d| d.as_ref()) {
                        validate_default_expr(
                            default,
                            errors,
                            &format!("function '{name}' default '{}'", sig.params[idx]),
                        );
                    }
                }
                return;
            }

            if env.struct_defs.contains_key(name) {
                let scope_name = match env.scope {
                    ScopeKind::Init => "init",
                    ScopeKind::Block => "block",
                    ScopeKind::Sample => "sample",
                    ScopeKind::Def => "def",
                };
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "struct constructors are only allowed as direct init assignments; found '{}' call in {scope_name}",
                        name
                    ),
                );
                for arg in args {
                    validate_expr(&arg.expr, env, errors);
                }
                return;
            }

            push_expr_error(
                errors,
                expr,
                format!("unknown function '{name}' in expression"),
            );
        }
        Expr::Binary { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
    }
}

fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    split_root_field_path(name)
}

fn is_internal_proc_helper_call(name: &str) -> bool {
    name.ends_with(PROC_INIT_FN_SUFFIX)
        || name.ends_with(PROC_BLOCK_PRE_FN_SUFFIX)
        || name.ends_with(PROC_BLOCK_POST_FN_SUFFIX)
        || name.ends_with(PROC_STEP_FN_SUFFIX)
        || name.contains(PROC_CALL_OUT_FN_PREFIX)
        || name.contains(PROC_EVENT_FN_PREFIX)
}

fn is_declared_struct_array_root_symbol(declared_symbols: &DeclaredSymbolMap, base: &str) -> bool {
    if is_declared_data_array_symbol(declared_symbols, base) {
        return true;
    }
    let prefix = format!("{base}.");
    declared_symbols.iter().any(|(name, info)| {
        name.starts_with(&prefix) && matches!(info, DeclaredSymbolInfo::DataArray { .. })
    })
}

fn is_builtin_len_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    if env.array_vars.contains_key(base)
        || has_declared_buffer_symbol_info(env.declared_symbols, base)
        || is_declared_struct_array_root_symbol(env.declared_symbols, base)
    {
        return true;
    }
    if let Some((root, field)) = split_simple_field_path(base) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(field_decl) = resolve_struct_field_decl(struct_name, field, env.struct_defs)
            {
                return matches!(field_decl.ty, TypedFieldType::Array(_));
            }
        }
    }
    false
}

fn is_builtin_buffer_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    has_declared_buffer_symbol_info(env.declared_symbols, base)
}

fn is_builtin_unsafe_data_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    if env.array_vars.contains_key(base)
        || has_declared_buffer_symbol_info(env.declared_symbols, base)
    {
        return true;
    }
    if let Some((root, field)) = split_simple_field_path(base) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(field_decl) = resolve_struct_field_decl(struct_name, field, env.struct_defs)
            {
                return matches!(field_decl.ty, TypedFieldType::Array(_))
                    && field_decl.array_elem_struct.is_none();
            }
        }
    }
    false
}

fn validate_data_len_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    if env.scope == ScopeKind::Init && has_declared_buffer_symbol_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            init_buffer_runtime_message(&format!("buffer method '{}.len()'", base)),
        );
    }
    if !args.is_empty() {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
        );
    }
    for arg in args {
        if arg.name.is_some() {
            push_loc_error(
                errors,
                loc,
                format!("builtin method '{}' does not support named arguments", name),
            );
        }
    }

    let before = errors.len();
    let is_data_symbol = if env.array_vars.contains_key(base) {
        true
    } else if has_declared_buffer_symbol_info(env.declared_symbols, base) {
        true
    } else if is_declared_struct_array_root_symbol(env.declared_symbols, base) {
        true
    } else if let Some((root, field)) = split_field_path(base, errors) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(field_decl) = resolve_struct_field_decl(struct_name, field, env.struct_defs)
            {
                match field_decl.ty {
                    TypedFieldType::Array(_) => true,
                    TypedFieldType::Struct => {
                        push_loc_error(
                            errors,
                            loc,
                            format!(
                                "builtin method '{}' requires a array symbol, but '{}.{}' is a nested struct",
                                name, root, field
                            ),
                        );
                        false
                    }
                    TypedFieldType::Scalar(_) => {
                        push_loc_error(
                            errors,
                            loc,
                            format!(
                                "builtin method '{}' requires a array symbol, but '{}.{}' is scalar",
                                name, root, field
                            ),
                        );
                        false
                    }
                }
            } else {
                if env.struct_defs.contains_key(struct_name) {
                    push_loc_error(
                        errors,
                        loc,
                        format!(
                            "struct instance '{}' (type '{}') has no field '{}'",
                            root, struct_name, field
                        ),
                    );
                } else {
                    push_loc_error(
                        errors,
                        loc,
                        format!("unknown struct type '{}'", struct_name),
                    );
                }
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !is_data_symbol && errors.len() == before {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' requires a array or buffer symbol receiver, got '{}'",
                name, base
            ),
        );
    }
}

fn validate_buffer_chans_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    if env.scope == ScopeKind::Init && has_declared_buffer_symbol_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            init_buffer_runtime_message(&format!("buffer method '{}.chans()'", base)),
        );
    }
    if !args.is_empty() {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
        );
    }
    for arg in args {
        if arg.name.is_some() {
            push_loc_error(
                errors,
                loc,
                format!("builtin method '{}' does not support named arguments", name),
            );
        }
    }
    if !has_declared_buffer_symbol_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' requires a buffer symbol receiver, got '{}'",
                name, base
            ),
        );
    }
}

fn validate_buffer_samplerate_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    if env.scope == ScopeKind::Init && has_declared_buffer_symbol_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            init_buffer_runtime_message(&format!("buffer method '{}.samplerate()'", base)),
        );
    }
    if !args.is_empty() {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
        );
    }
    for arg in args {
        if arg.name.is_some() {
            push_loc_error(
                errors,
                loc,
                format!("builtin method '{}' does not support named arguments", name),
            );
        }
    }
    if !has_declared_buffer_symbol_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin method '{}' requires a buffer symbol receiver, got '{}'",
                name, base
            ),
        );
    }
}

fn validate_buffer_param_call_arg(
    fn_name: &str,
    param_idx: usize,
    param_names: &[String],
    expected: &BufferType,
    arg: &Expr,
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let context = if let Some(param_name) = param_names.get(param_idx) {
        format!("function '{fn_name}' parameter '{param_name}'")
    } else {
        format!("function '{fn_name}' parameter #{param_idx}")
    };
    if let Expr::UserCall { name, args, .. } = arg {
        if name == PROC_INDEX_BUFFER_SELECT_SENTINEL {
            validate_internal_proc_index_buffer_select_call(name, args, env, loc, errors);
            for slot_expr in args.iter().filter(|a| a.name.is_none()).map(|a| &a.expr) {
                if let Expr::Var { name: symbol, .. } = slot_expr {
                    validate_buffer_symbol_for_param(
                        &context,
                        expected,
                        symbol,
                        env,
                        slot_expr.loc(),
                        errors,
                    );
                }
            }
            return;
        }
    }
    let Expr::Var { name: symbol, .. } = arg else {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            format!("{context} expects a buffer symbol argument"),
        );
        validate_expr(arg, env, errors);
        return;
    };
    if env.scope == ScopeKind::Init {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            init_buffer_runtime_message(&format!("buffer argument '{}' in {}", symbol, context)),
        );
    }
    validate_buffer_symbol_for_param(&context, expected, symbol, env, arg.loc().or(loc), errors);
}

fn validate_buffer_symbol_for_param(
    context: &str,
    expected: &BufferType,
    symbol: &str,
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    if !has_declared_buffer_symbol_info(env.declared_symbols, symbol) {
        push_loc_error(
            errors,
            loc,
            format!("{context} expects a buffer symbol argument, got '{symbol}'"),
        );
        return;
    }
    let expected_elem = match expected.elem {
        BufferElemType::Primitive(ty) => ty,
        BufferElemType::Generic(ref param_ty) => {
            push_loc_error(
                errors,
                loc,
                format!(
                    "{context} uses unresolved generic buffer element type '{}'",
                    param_ty
                ),
            );
            PrimitiveType::F32
        }
    };
    if !has_declared_buffer_elem_type_info(env.declared_symbols, symbol, expected_elem) {
        push_loc_error(
            errors,
            loc,
            format!(
                "{context} expects element type {:?}, but buffer '{}' has a different element type",
                expected_elem, symbol
            ),
        );
    }
    match &expected.channels {
        BufferChannels::Mono => {
            if is_declared_multichannel_buffer_info(env.declared_symbols, symbol) {
                push_loc_error(
                    errors,
                    loc,
                    format!(
                        "{context} expects mono buffer, but '{}' is multichannel",
                        symbol
                    ),
                );
            }
        }
        BufferChannels::Static(expr) => {
            let requested_channels = const_positive_usize(expr);
            if let Some(channels) = requested_channels {
                if channels <= 1 {
                    if is_declared_multichannel_buffer_info(env.declared_symbols, symbol) {
                        push_loc_error(
                            errors,
                            loc,
                            format!(
                                "{context} expects mono/static-1 buffer, but '{}' is multichannel",
                                symbol
                            ),
                        );
                    }
                    return;
                }
            }
            if !is_declared_multichannel_buffer_info(env.declared_symbols, symbol) {
                push_loc_error(
                    errors,
                    loc,
                    format!(
                        "{context} expects multichannel buffer, but '{}' is mono",
                        symbol
                    ),
                );
                return;
            }
            if let Some(channels) = requested_channels {
                if has_declared_dynamic_buffer_channels_info(env.declared_symbols, symbol) {
                    push_loc_error(
                        errors,
                        loc,
                        format!(
                            "{context} expects static {channels} channels, but '{}' is dynamic",
                            symbol
                        ),
                    );
                    return;
                }
                if let Some(actual) =
                    declared_static_buffer_channels_info(env.declared_symbols, symbol)
                {
                    if actual != channels {
                        push_loc_error(
                            errors,
                            loc,
                            format!(
                                "{context} expects {channels} channels, but '{}' has {actual}",
                                symbol
                            ),
                        );
                    }
                }
            }
        }
        BufferChannels::Dynamic => {
            if !is_declared_multichannel_buffer_info(env.declared_symbols, symbol) {
                push_loc_error(
                    errors,
                    loc,
                    format!(
                        "{context} expects multichannel dynamic buffer, but '{}' is mono",
                        symbol
                    ),
                );
            }
        }
    }
}

fn const_positive_usize(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int { value: v, .. } if *v > 0 => usize::try_from(*v).ok(),
        Expr::Number { value: v, .. } if *v > 0.0 && v.fract() == 0.0 => {
            usize::try_from(*v as i64).ok()
        }
        _ => None,
    }
}

fn validate_internal_buffer_2d_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == INTERNAL_BUFFER_READ2_FN {
        3
    } else {
        4
    };
    if args.len() != expected_arity {
        push_loc_error(
            errors,
            loc,
            format!(
                "internal builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
        );
    }
    for arg in args {
        if arg.name.is_some() {
            push_loc_error(
                errors,
                loc,
                format!(
                    "internal builtin '{}' does not support named arguments",
                    name
                ),
            );
        }
    }
    if let Some(first) = args.first() {
        match &first.expr {
            Expr::Var { name: base, .. } => {
                if env.scope == ScopeKind::Init
                    && has_declared_buffer_symbol_info(env.declared_symbols, base)
                {
                    push_loc_error(
                        errors,
                        first.expr.loc().or(loc),
                        init_buffer_runtime_message(&format!(
                            "buffer access '{}' in '{}'",
                            base, name
                        )),
                    );
                }
                if !has_declared_buffer_symbol_info(env.declared_symbols, base) {
                    push_loc_error(
                        errors,
                        first.expr.loc().or(loc),
                        format!(
                            "internal builtin '{}' first argument must be a declared buffer symbol, got '{}'",
                            name, base
                        ),
                    );
                } else if !is_declared_multichannel_buffer_info(env.declared_symbols, base) {
                    push_loc_error(
                        errors,
                        first.expr.loc().or(loc),
                        format!(
                            "internal builtin '{}' requires multichannel buffer indexing form, but '{}' is mono",
                            name, base
                        ),
                    );
                }
            }
            other => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!(
                        "internal builtin '{}' first argument must be a declared buffer symbol variable",
                        name
                    ),
                );
            }
        }
    }
    if let Some(ch_arg) = args.get(1) {
        validate_expr(&ch_arg.expr, env, errors);
    }
    if let Some(sample_arg) = args.get(2) {
        validate_expr(&sample_arg.expr, env, errors);
    }
    if name == INTERNAL_BUFFER_WRITE2_FN {
        if let Some(value_arg) = args.get(3) {
            validate_expr(&value_arg.expr, env, errors);
        }
    }
}

fn validate_internal_proc_index_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let mut base_expr = None::<&Expr>;
    let mut index_expr = None::<&Expr>;
    let mut field_expr = None::<&Expr>;
    let mut positional_base_index = None::<(&Expr, &Expr)>;
    if args.len() >= 2 && args[0].name.is_none() && args[1].name.is_none() {
        positional_base_index = Some((&args[0].expr, &args[1].expr));
    }
    for arg in args {
        match arg.name.as_deref() {
            Some(PROC_INDEX_BASE_ARG) => base_expr = Some(&arg.expr),
            Some(PROC_INDEX_EXPR_ARG) => index_expr = Some(&arg.expr),
            Some(PROC_FIELD_SENTINEL_ARG) => field_expr = Some(&arg.expr),
            _ => {}
        }
    }
    if base_expr.is_none() {
        if let Some((base, _)) = positional_base_index {
            base_expr = Some(base);
        }
    }
    if index_expr.is_none() {
        if let Some((_, index)) = positional_base_index {
            index_expr = Some(index);
        }
    }

    if base_expr.is_none() {
        push_loc_error(
            errors,
            loc,
            format!("internal builtin '{name}' is missing processor array base argument"),
        );
    }
    if index_expr.is_none() {
        push_loc_error(
            errors,
            loc,
            format!("internal builtin '{name}' is missing processor array index argument"),
        );
    }

    if let Some(base_expr) = base_expr {
        if !matches!(base_expr, Expr::Var { .. }) {
            validate_expr(base_expr, env, errors);
            push_loc_error(
                errors,
                base_expr.loc().or(loc),
                format!("internal builtin '{name}' expects processor array base as an identifier"),
            );
        }
    }
    if let Some(index_expr) = index_expr {
        validate_expr(index_expr, env, errors);
    }

    let expects_field = name
        .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
        .map(|s| s == PROC_INDEX_CALL_SENTINEL)
        .unwrap_or(false);
    if expects_field {
        match field_expr {
            Some(Expr::Var { .. }) => {}
            Some(other) => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!("internal builtin '{name}' expects field selector as identifier"),
                );
            }
            None => {
                push_loc_error(
                    errors,
                    loc,
                    format!("internal builtin '{name}' is missing field selector argument"),
                );
            }
        }
    }

    for (arg_idx, arg) in args.iter().enumerate() {
        if positional_base_index.is_some() && arg_idx < 2 {
            continue;
        }
        match arg.name.as_deref() {
            Some(PROC_INDEX_BASE_ARG)
            | Some(PROC_INDEX_EXPR_ARG)
            | Some(PROC_FIELD_SENTINEL_ARG) => {}
            _ => validate_expr(&arg.expr, env, errors),
        }
    }
}

fn validate_internal_proc_index_buffer_select_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let mut base_expr = None::<&Expr>;
    let mut index_expr = None::<&Expr>;
    let mut slot_exprs = Vec::<&Expr>::new();

    for arg in args {
        match arg.name.as_deref() {
            Some(PROC_INDEX_BASE_ARG) => base_expr = Some(&arg.expr),
            Some(PROC_INDEX_EXPR_ARG) => index_expr = Some(&arg.expr),
            Some(other) => {
                push_loc_error(
                    errors,
                    arg.expr.loc().or(loc),
                    format!(
                        "internal builtin '{}' does not support named argument '{}'",
                        name, other
                    ),
                );
            }
            None => slot_exprs.push(&arg.expr),
        }
    }

    if base_expr.is_none() {
        push_loc_error(
            errors,
            loc,
            format!("internal builtin '{name}' is missing processor array base argument"),
        );
    }
    if index_expr.is_none() {
        push_loc_error(
            errors,
            loc,
            format!("internal builtin '{name}' is missing processor array index argument"),
        );
    }
    if slot_exprs.is_empty() {
        push_loc_error(
            errors,
            loc,
            format!("internal builtin '{name}' requires at least one slot buffer argument"),
        );
    }

    if let Some(base_expr) = base_expr {
        match base_expr {
            Expr::Var { .. } => {}
            other => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!(
                        "internal builtin '{name}' expects processor array base as an identifier"
                    ),
                );
            }
        }
    }
    if let Some(index_expr) = index_expr {
        validate_expr(index_expr, env, errors);
    }

    for slot_expr in slot_exprs {
        match slot_expr {
            Expr::Var { name: symbol, .. } => {
                if !has_declared_buffer_symbol_info(env.declared_symbols, symbol) {
                    push_loc_error(
                        errors,
                        slot_expr.loc().or(loc),
                        format!(
                            "internal builtin '{}' slot arguments must be declared buffer symbols, got '{}'",
                            name, symbol
                        ),
                    );
                }
            }
            other => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!(
                        "internal builtin '{}' slot arguments must be buffer symbol variables",
                        name
                    ),
                );
            }
        }
    }
}

fn validate_unsafe_data_builtin_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == "unsafe_read" { 2 } else { 3 };
    if args.len() != expected_arity {
        push_loc_error(
            errors,
            loc,
            format!(
                "builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
        );
    }

    for arg in args {
        if arg.name.is_some() {
            push_loc_error(
                errors,
                arg.expr.loc().or(loc),
                format!("builtin '{}' does not support named arguments", name),
            );
        }
    }

    if let Some(first_arg) = args.first() {
        match &first_arg.expr {
            Expr::Var { name: base, .. } => {
                if env.scope == ScopeKind::Init
                    && has_declared_buffer_symbol_info(env.declared_symbols, base)
                {
                    push_loc_error(
                        errors,
                        first_arg.expr.loc().or(loc),
                        init_buffer_runtime_message(&format!(
                            "buffer access '{}' in builtin '{}'",
                            base, name
                        )),
                    );
                }
                let mut is_valid_primitive_data = false;

                if let Some((root, field)) = split_field_path(base, errors) {
                    if let Some(struct_name) = env.param_structs.get(root) {
                        let Some(_fields) = env.struct_defs.get(struct_name) else {
                            push_loc_error(
                                errors,
                                first_arg.expr.loc().or(loc),
                                format!("unknown struct type '{}'", struct_name),
                            );
                            return;
                        };
                        let Some(field_decl) =
                            resolve_struct_field_decl(struct_name, field, env.struct_defs)
                        else {
                            push_loc_error(
                                errors,
                                first_arg.expr.loc().or(loc),
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                            );
                            return;
                        };
                        match field_decl.ty {
                            TypedFieldType::Array(_) => {
                                if field_decl.array_elem_struct.is_some() {
                                    push_loc_error(
                                        errors,
                                        first_arg.expr.loc().or(loc),
                                        format!(
                                            "builtin '{}' does not support array[Struct, N] symbol '{}'",
                                            name, base
                                        ),
                                    );
                                } else {
                                    is_valid_primitive_data = true;
                                }
                            }
                            TypedFieldType::Scalar(_) => {
                                push_loc_error(
                                    errors,
                                    first_arg.expr.loc().or(loc),
                                    format!(
                                        "builtin '{}' expects a array symbol as first argument, but '{}.{}' is scalar",
                                        name, root, field
                                    ),
                                );
                            }
                            TypedFieldType::Struct => {
                                push_loc_error(
                                    errors,
                                    first_arg.expr.loc().or(loc),
                                    format!(
                                        "builtin '{}' expects a array symbol as first argument, but '{}.{}' is a nested struct",
                                        name, root, field
                                    ),
                                );
                            }
                        }
                    } else if env.array_vars.contains_key(base) {
                        is_valid_primitive_data = true;
                    }
                } else if env.array_vars.contains_key(base)
                    || has_declared_buffer_symbol_info(env.declared_symbols, base)
                {
                    is_valid_primitive_data = true;
                }

                if !is_valid_primitive_data {
                    push_loc_error(
                        errors,
                        first_arg.expr.loc().or(loc),
                        format!(
                            "builtin '{}' expects a primitive array or buffer symbol as first argument, got '{}'",
                            name, base
                        ),
                    );
                }
            }
            other => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!(
                        "builtin '{}' first argument must be a array symbol variable",
                        name
                    ),
                );
            }
        }
    }

    if let Some(index_arg) = args.get(1) {
        validate_expr(&index_arg.expr, env, errors);
    }
    if name == "unsafe_write" {
        if let Some(value_arg) = args.get(2) {
            validate_expr(&value_arg.expr, env, errors);
        }
    }
}
