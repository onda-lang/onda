use crate::*;

pub(crate) fn validate_expr(expr: &Expr, env: ExprEnv<'_>, errors: &mut Vec<Diagnostic>) {
    match expr {
        Expr::Number(_) | Expr::Int(_) | Expr::Bool(_) => {}
        Expr::ArrayLiteral(values) => {
            for value in values {
                validate_expr(value, env, errors);
            }
            errors.push(Diagnostic::semantic(
                "array literals are only allowed in typed array declarations and parameter defaults",
                0,
                0,
            ));
        }
        Expr::Var(name) => {
            if is_builtin_constant_name(name) {
                return;
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                if let Some(struct_name) = env.param_structs.get(base) {
                    let Some(fields) = env.struct_defs.get(struct_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown struct type '{}'", struct_name),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct parameter '{}' (type '{}') has no field '{}'",
                                base, struct_name, field
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    if matches!(field_decl.ty, TypedFieldType::Array(_)) {
                        errors.push(Diagnostic::semantic(
                            format!("array field '{}.{}' must be indexed", base, field),
                            0,
                            0,
                        ));
                    }
                    return;
                }

                let flat = format!("{base}.{field}");
                if env.array_vars.contains_key(&flat) {
                    errors.push(Diagnostic::semantic(
                        format!("array symbol '{flat}' must be indexed"),
                        0,
                        0,
                    ));
                    return;
                }
                if !env.known_scalars.contains(&flat)
                    && !env.locals.contains(&flat)
                    && !env.outputs.contains(&flat)
                {
                    errors.push(Diagnostic::semantic(
                        format!("unknown symbol '{flat}' in expression"),
                        0,
                        0,
                    ));
                }
                return;
            }

            if env.param_structs.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct parameter '{}' must be accessed via fields", name),
                    0,
                    0,
                ));
                return;
            }
            if env.struct_instances.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("struct instance '{name}' must be accessed via fields"),
                    0,
                    0,
                ));
                return;
            }
            if env.array_vars.contains_key(name) {
                errors.push(Diagnostic::semantic(
                    format!("array symbol '{name}' must be indexed"),
                    0,
                    0,
                ));
                return;
            }
            if has_declared_buffer_symbol(env.known_scalars, name) {
                errors.push(Diagnostic::semantic(
                    format!("buffer symbol '{name}' must be indexed"),
                    0,
                    0,
                ));
                return;
            }
            if !env.known_scalars.contains(name)
                && !env.locals.contains(name)
                && !env.outputs.contains(name)
            {
                errors.push(Diagnostic::semantic(
                    format!("unknown symbol '{name}' in expression"),
                    0,
                    0,
                ));
            }
        }
        Expr::Index { base, index } => {
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some(struct_name) = env.param_structs.get(root) {
                    let Some(fields) = env.struct_defs.get(struct_name) else {
                        errors.push(Diagnostic::semantic(
                            format!("unknown struct type '{}'", struct_name),
                            0,
                            0,
                        ));
                        return;
                    };
                    let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "struct parameter '{}' (type '{}') has no field '{}'",
                                root, struct_name, field
                            ),
                            0,
                            0,
                        ));
                        return;
                    };
                    if !matches!(field_decl.ty, TypedFieldType::Array(_)) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "field '{}.{}' is not array and cannot be indexed",
                                root, field
                            ),
                            0,
                            0,
                        ));
                    }
                    validate_expr(index, env, errors);
                    return;
                }
            }
            if !env.array_vars.contains_key(base)
                && !has_declared_buffer_symbol(env.known_scalars, base)
            {
                errors.push(Diagnostic::semantic(
                    format!("indexed expression '{base}[...]' is not a array/buffer symbol"),
                    0,
                    0,
                ));
            } else if is_declared_multichannel_buffer_symbol(env.known_scalars, base) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "indexed expression '{base}[...]' uses mono form on a multichannel buffer; use '{base}[ch][sample]'"
                    ),
                    0,
                    0,
                ));
            }
            validate_expr(index, env, errors);
        }
        Expr::ArrayCtor { init, .. } => {
            if !env.allow_array_ctor {
                errors.push(Diagnostic::semantic(
                    "array[...] constructor is only allowed in init assignments",
                    0,
                    0,
                ));
            }
            if let Some(values) = init {
                for value in values {
                    validate_expr(value, env, errors);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr } => {
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
        Expr::Call { func, args } => {
            for arg in args {
                validate_expr(arg, env, errors);
            }
            let expected = builtin_arity(*func);
            if args.len() != expected {
                errors.push(Diagnostic::semantic(
                    format!(
                        "builtin '{}' expects {expected} positional arguments, got {}",
                        builtin_name(*func),
                        args.len()
                    ),
                    0,
                    0,
                ));
            }
        }
        Expr::UserCall {
            name,
            type_args,
            args,
        } => {
            if is_builtin_unsafe_data_fn(name) {
                validate_unsafe_data_builtin_call(name, args, env, errors);
                return;
            }
            if is_internal_buffer_2d_fn(name) {
                validate_internal_buffer_2d_call(name, args, env, errors);
                return;
            }
            if !env.fn_signatures.contains_key(name) {
                if let Some(base) = parse_array_len_instance_base(name) {
                    if is_builtin_len_receiver(base, env) {
                        validate_data_len_builtin_call(name, base, args, env, errors);
                        return;
                    }
                }
                if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_chans_builtin_call(name, base, args, env, errors);
                        return;
                    }
                }
                if let Some(base) = parse_unsafe_read_instance_base(name) {
                    if is_builtin_unsafe_data_receiver(base, env) {
                        let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                        method_args.push(CallArg {
                            name: None,
                            expr: Expr::Var(base.to_owned()),
                        });
                        method_args.extend(args.iter().cloned());
                        validate_unsafe_data_builtin_call("unsafe_read", &method_args, env, errors);
                        return;
                    }
                }
                if let Some(base) = parse_unsafe_write_instance_base(name) {
                    if is_builtin_unsafe_data_receiver(base, env) {
                        let mut method_args = Vec::with_capacity(args.len().saturating_add(1));
                        method_args.push(CallArg {
                            name: None,
                            expr: Expr::Var(base.to_owned()),
                        });
                        method_args.extend(args.iter().cloned());
                        validate_unsafe_data_builtin_call("unsafe_write", &method_args, env, errors);
                        return;
                    }
                }
            }
            if let Some(sig) = env.fn_signatures.get(name) {
                if sig.type_params.is_empty() {
                    if !type_args.is_empty() {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "function '{}' is not generic and cannot take type arguments",
                                name
                            ),
                            0,
                            0,
                        ));
                    }
                } else if !type_args.is_empty() && type_args.len() != sig.type_params.len() {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "function '{}' expects {} type arguments, got {}",
                            name,
                            sig.type_params.len(),
                            type_args.len()
                        ),
                        0,
                        0,
                    ));
                }

                let forbid_self_named = sig.params.first().map(String::as_str) == Some("self");
                let resolved = resolve_call_args(
                    args,
                    &sig.params,
                    &sig.defaults,
                    forbid_self_named,
                    false,
                    &format!("function '{name}' call"),
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
                                errors,
                            );
                            continue;
                        }
                        if param_ty.is_none() {
                            if let Expr::Var(v) = arg {
                                if has_declared_buffer_symbol(env.known_scalars, v) {
                                    continue;
                                }
                                if env.array_vars.contains_key(v) {
                                    continue;
                                }
                            }
                        }
                        if let Expr::Var(v) = arg {
                            if env.struct_instances.contains_key(v)
                                || env.param_structs.contains_key(v)
                            {
                                continue;
                            }
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
                    ScopeKind::Sample => "sample",
                    ScopeKind::Def => "def",
                };
                errors.push(Diagnostic::semantic(
                    format!(
                        "struct constructors are only allowed as direct init assignments; found '{}' call in {scope_name}",
                        name
                    ),
                    0,
                    0,
                ));
                for arg in args {
                    validate_expr(&arg.expr, env, errors);
                }
                return;
            }

            errors.push(Diagnostic::semantic(
                format!("unknown function '{name}' in expression"),
                0,
                0,
            ));
        }
        Expr::Binary { lhs, rhs, .. } => {
            validate_expr(lhs, env, errors);
            validate_expr(rhs, env, errors);
        }
    }
}

fn split_simple_field_path(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((first, second))
}

fn is_builtin_len_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    if env.array_vars.contains_key(base) || has_declared_buffer_symbol(env.known_scalars, base) {
        return true;
    }
    if let Some((root, field)) = split_simple_field_path(base) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(fields) = env.struct_defs.get(struct_name) {
                if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                    return matches!(field_decl.ty, TypedFieldType::Array(_));
                }
            }
        }
    }
    false
}

fn is_builtin_buffer_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    has_declared_buffer_symbol(env.known_scalars, base)
}

fn is_builtin_unsafe_data_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    if env.array_vars.contains_key(base) || has_declared_buffer_symbol(env.known_scalars, base) {
        return true;
    }
    if let Some((root, field)) = split_simple_field_path(base) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(fields) = env.struct_defs.get(struct_name) {
                if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                    return matches!(field_decl.ty, TypedFieldType::Array(_))
                        && field_decl.array_elem_struct.is_none();
                }
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
    errors: &mut Vec<Diagnostic>,
) {
    if !args.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin method '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }

    let before = errors.len();
    let is_data_symbol = if env.array_vars.contains_key(base) {
        true
    } else if has_declared_buffer_symbol(env.known_scalars, base) {
        true
    } else if let Some((root, field)) = split_field_path(base, errors) {
        let struct_name = env
            .param_structs
            .get(root)
            .or_else(|| env.struct_instances.get(root));
        if let Some(struct_name) = struct_name {
            if let Some(fields) = env.struct_defs.get(struct_name) {
                if let Some(field_decl) = fields.iter().find(|f| f.name == field) {
                    match field_decl.ty {
                        TypedFieldType::Array(_) => true,
                        TypedFieldType::Scalar(_) => {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "builtin method '{}' requires a array symbol, but '{}.{}' is scalar",
                                    name, root, field
                                ),
                                0,
                                0,
                            ));
                            false
                        }
                    }
                } else {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "struct instance '{}' (type '{}') has no field '{}'",
                            root, struct_name, field
                        ),
                        0,
                        0,
                    ));
                    false
                }
            } else {
                errors.push(Diagnostic::semantic(
                    format!("unknown struct type '{}'", struct_name),
                    0,
                    0,
                ));
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if !is_data_symbol && errors.len() == before {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' requires a array or buffer symbol receiver, got '{}'",
                name, base
            ),
            0,
            0,
        ));
    }
}

fn validate_buffer_chans_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if !args.is_empty() {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' expects 0 arguments, got {}",
                name,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin method '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }
    if !has_declared_buffer_symbol(env.known_scalars, base) {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin method '{}' requires a buffer symbol receiver, got '{}'",
                name, base
            ),
            0,
            0,
        ));
    }
}

fn validate_buffer_param_call_arg(
    fn_name: &str,
    param_idx: usize,
    param_names: &[String],
    expected: &BufferType,
    arg: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let context = if let Some(param_name) = param_names.get(param_idx) {
        format!("function '{fn_name}' parameter '{param_name}'")
    } else {
        format!("function '{fn_name}' parameter #{param_idx}")
    };
    let Expr::Var(symbol) = arg else {
        errors.push(Diagnostic::semantic(
            format!("{context} expects a buffer symbol argument"),
            0,
            0,
        ));
        validate_expr(arg, env, errors);
        return;
    };
    if !has_declared_buffer_symbol(env.known_scalars, symbol) {
        errors.push(Diagnostic::semantic(
            format!("{context} expects a buffer symbol argument, got '{symbol}'"),
            0,
            0,
        ));
        return;
    }
    let expected_elem = match expected.elem {
        BufferElemType::Primitive(ty) => ty,
        BufferElemType::Generic(ref param_ty) => {
            errors.push(Diagnostic::semantic(
                format!(
                    "{context} uses unresolved generic buffer element type '{}'",
                    param_ty
                ),
                0,
                0,
            ));
            PrimitiveType::F32
        }
    };
    if !has_declared_buffer_elem_type(env.known_scalars, symbol, expected_elem) {
        errors.push(Diagnostic::semantic(
            format!(
                "{context} expects element type {:?}, but buffer '{}' has a different element type",
                expected_elem, symbol
            ),
            0,
            0,
        ));
    }
    match &expected.channels {
        BufferChannels::Mono => {
            if is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects mono buffer, but '{}' is multichannel",
                        symbol
                    ),
                    0,
                    0,
                ));
            }
        }
        BufferChannels::Static(expr) => {
            let requested_channels = const_positive_usize(expr);
            if let Some(channels) = requested_channels {
                if channels <= 1 {
                    if is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context} expects mono/static-1 buffer, but '{}' is multichannel",
                                symbol
                            ),
                            0,
                            0,
                        ));
                    }
                    return;
                }
            }
            if !is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects multichannel buffer, but '{}' is mono",
                        symbol
                    ),
                    0,
                    0,
                ));
                return;
            }
            if let Some(channels) = requested_channels {
                if has_declared_dynamic_buffer_channels(env.known_scalars, symbol) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "{context} expects static {channels} channels, but '{}' is dynamic",
                            symbol
                        ),
                        0,
                        0,
                    ));
                    return;
                }
                if let Some(actual) = declared_static_buffer_channels(env.known_scalars, symbol) {
                    if actual != channels {
                        errors.push(Diagnostic::semantic(
                            format!(
                                "{context} expects {channels} channels, but '{}' has {actual}",
                                symbol
                            ),
                            0,
                            0,
                        ));
                    }
                }
            }
        }
        BufferChannels::Dynamic => {
            if !is_declared_multichannel_buffer_symbol(env.known_scalars, symbol) {
                errors.push(Diagnostic::semantic(
                    format!(
                        "{context} expects multichannel dynamic buffer, but '{}' is mono",
                        symbol
                    ),
                    0,
                    0,
                ));
            }
        }
    }
}

fn const_positive_usize(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Int(v) if *v > 0 => usize::try_from(*v).ok(),
        Expr::Number(v) if *v > 0.0 && v.fract() == 0.0 => usize::try_from(*v as i64).ok(),
        _ => None,
    }
}

fn validate_internal_buffer_2d_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == INTERNAL_BUFFER_READ2_FN {
        3
    } else {
        4
    };
    if args.len() != expected_arity {
        errors.push(Diagnostic::semantic(
            format!(
                "internal builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
            0,
            0,
        ));
    }
    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!(
                    "internal builtin '{}' does not support named arguments",
                    name
                ),
                0,
                0,
            ));
        }
    }
    if let Some(first) = args.first() {
        match &first.expr {
            Expr::Var(base) => {
                if !has_declared_buffer_symbol(env.known_scalars, base) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "internal builtin '{}' first argument must be a declared buffer symbol, got '{}'",
                            name, base
                        ),
                        0,
                        0,
                    ));
                } else if !is_declared_multichannel_buffer_symbol(env.known_scalars, base) {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "internal builtin '{}' requires multichannel buffer indexing form, but '{}' is mono",
                            name, base
                        ),
                        0,
                        0,
                    ));
                }
            }
            other => {
                validate_expr(other, env, errors);
                errors.push(Diagnostic::semantic(
                    format!(
                        "internal builtin '{}' first argument must be a declared buffer symbol variable",
                        name
                    ),
                    0,
                    0,
                ));
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

fn validate_unsafe_data_builtin_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let expected_arity = if name == "unsafe_read" { 2 } else { 3 };
    if args.len() != expected_arity {
        errors.push(Diagnostic::semantic(
            format!(
                "builtin '{}' expects {} positional arguments, got {}",
                name,
                expected_arity,
                args.len()
            ),
            0,
            0,
        ));
    }

    for arg in args {
        if arg.name.is_some() {
            errors.push(Diagnostic::semantic(
                format!("builtin '{}' does not support named arguments", name),
                0,
                0,
            ));
        }
    }

    if let Some(first_arg) = args.first() {
        match &first_arg.expr {
            Expr::Var(base) => {
                let mut is_valid_primitive_data = false;

                if let Some((root, field)) = split_field_path(base, errors) {
                    if let Some(struct_name) = env.param_structs.get(root) {
                        let Some(fields) = env.struct_defs.get(struct_name) else {
                            errors.push(Diagnostic::semantic(
                                format!("unknown struct type '{}'", struct_name),
                                0,
                                0,
                            ));
                            return;
                        };
                        let Some(field_decl) = fields.iter().find(|f| f.name == field) else {
                            errors.push(Diagnostic::semantic(
                                format!(
                                    "struct parameter '{}' (type '{}') has no field '{}'",
                                    root, struct_name, field
                                ),
                                0,
                                0,
                            ));
                            return;
                        };
                        match field_decl.ty {
                            TypedFieldType::Array(_) => {
                                if field_decl.array_elem_struct.is_some() {
                                    errors.push(Diagnostic::semantic(
                                        format!(
                                            "builtin '{}' does not support array[Struct, N] symbol '{}'",
                                            name, base
                                        ),
                                        0,
                                        0,
                                    ));
                                } else {
                                    is_valid_primitive_data = true;
                                }
                            }
                            TypedFieldType::Scalar(_) => {
                                errors.push(Diagnostic::semantic(
                                    format!(
                                        "builtin '{}' expects a array symbol as first argument, but '{}.{}' is scalar",
                                        name, root, field
                                    ),
                                    0,
                                    0,
                                ));
                            }
                        }
                    } else if env.array_vars.contains_key(base) {
                        is_valid_primitive_data = true;
                    }
                } else if env.array_vars.contains_key(base)
                    || has_declared_buffer_symbol(env.known_scalars, base)
                {
                    is_valid_primitive_data = true;
                }

                if !is_valid_primitive_data {
                    errors.push(Diagnostic::semantic(
                        format!(
                            "builtin '{}' expects a primitive array or buffer symbol as first argument, got '{}'",
                            name, base
                        ),
                        0,
                        0,
                    ));
                }
            }
            other => {
                validate_expr(other, env, errors);
                errors.push(Diagnostic::semantic(
                    format!(
                        "builtin '{}' first argument must be a array symbol variable",
                        name
                    ),
                    0,
                    0,
                ));
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
