use crate::internal_names::METHOD_RECEIVER_ARG;
use crate::*;

fn push_expr_error(errors: &mut Vec<Diagnostic>, expr: &Expr, message: impl Into<String>) {
    errors.push(Diagnostic::semantic_span(message, expr.loc()));
}

fn push_loc_error(errors: &mut Vec<Diagnostic>, loc: SourceLoc, message: impl Into<String>) {
    errors.push(Diagnostic::semantic_span(message, loc));
}

fn block_audio_input_name<'a>(name: &'a str, env: ExprEnv<'_>) -> Option<&'a str> {
    if env.scope != ScopeKind::Block
        || env.locals.contains(name)
        || env.local_aliases.contains_key(name)
        || env.local_array_aliases.contains_key(name)
    {
        return None;
    }
    if env.input_names.contains(name)
        || (name == "ins" && !env.input_names.is_empty())
        || env.input_names.iter().any(|input| {
            input
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('['))
        })
    {
        Some(name)
    } else {
        None
    }
}

fn push_block_audio_input_error(errors: &mut Vec<Diagnostic>, loc: SourceLoc, name: &str) {
    push_loc_error(
        errors,
        loc,
        format!(
            "audio input '{name}' can only be read in sample; move this read into the block's nested sample section"
        ),
    );
}

pub(crate) fn infer_call_argument_scalar_type(
    expr: &Expr,
    env: ExprEnv<'_>,
) -> Option<PrimitiveType> {
    let mut discarded = Vec::new();
    infer_expr_type_for_semantics_with_local_data_and_proc_arrays(
        expr,
        env.state_scalars,
        env.declared_symbols,
        Some(env.param_structs),
        env.local_aliases,
        env.local_array_aliases,
        env.locals,
        env.input_names,
        env.output_names,
        env.param_names,
        env.struct_instances,
        env.struct_defs,
        env.proc_array_roots,
        &mut discarded,
    )
}

pub(crate) fn dynamic_param_surface_value_name<'a>(
    expr: &'a Expr,
    env: ExprEnv<'_>,
) -> Option<&'a str> {
    match expr {
        Expr::Var { name, .. } => dynamic_param_surface_name(name, env),
        Expr::Slice { base, .. } => dynamic_param_surface_name(base, env),
        _ => None,
    }
}

pub(crate) fn dynamic_param_surface_name<'a>(name: &'a str, env: ExprEnv<'_>) -> Option<&'a str> {
    if env.locals.contains(name) || env.local_aliases.contains_key(name) {
        return None;
    }
    env.dynamic_param_arrays.contains(name).then_some(name)
}

pub(crate) fn io_surface_value_name<'a>(expr: &'a Expr, env: ExprEnv<'_>) -> Option<&'a str> {
    match expr {
        Expr::Var { name, .. } => io_surface_name(name, env),
        Expr::Slice { base, .. } => io_surface_name(base, env),
        _ => None,
    }
}

pub(crate) fn io_surface_name<'a>(name: &'a str, env: ExprEnv<'_>) -> Option<&'a str> {
    if env.locals.contains(name) || env.local_aliases.contains_key(name) {
        return None;
    }
    (env.io_surface_names.contains(name) || env.io_surface_array_names.contains(name))
        .then_some(name)
}

pub(crate) fn io_surface_array_name<'a>(name: &'a str, env: ExprEnv<'_>) -> Option<&'a str> {
    if env.locals.contains(name) || env.local_aliases.contains_key(name) {
        return None;
    }
    env.io_surface_array_names.contains(name).then_some(name)
}

pub(crate) fn push_io_surface_scope_error(
    errors: &mut Vec<Diagnostic>,
    loc: SourceLoc,
    name: &str,
) {
    push_loc_error(
        errors,
        loc,
        format!("I/O symbol '{name}' is only available in block or sample"),
    );
}

pub(crate) fn push_io_surface_value_error(
    errors: &mut Vec<Diagnostic>,
    loc: SourceLoc,
    name: &str,
) {
    push_loc_error(
        errors,
        loc,
        format!(
            "I/O array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
        ),
    );
}

pub(crate) fn push_dynamic_param_surface_value_error(
    errors: &mut Vec<Diagnostic>,
    loc: SourceLoc,
    name: &str,
) {
    push_loc_error(
        errors,
        loc,
        format!(
            "dynamic param array '{name}' is not a first-class value; use '{name}[i]' directly in block or sample"
        ),
    );
}

pub(crate) fn push_dynamic_param_index_scope_error(
    errors: &mut Vec<Diagnostic>,
    expr: &Expr,
    name: &str,
) {
    push_expr_error(
        errors,
        expr,
        format!("dynamic param indexing '{name}[...]' is only allowed in block or sample"),
    );
}

fn validate_block_bound_surface_plain_name(
    name: &str,
    loc: SourceLoc,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if let Some(surface) = io_surface_name(name, env) {
        if !env.io_surface_access_allowed {
            push_io_surface_scope_error(errors, loc, surface);
            return false;
        }
        if let Some(array_name) = io_surface_array_name(name, env) {
            push_io_surface_value_error(errors, loc, array_name);
            return false;
        }
        return true;
    }
    if let Some(surface) = dynamic_param_surface_name(name, env) {
        push_dynamic_param_surface_value_error(errors, loc, surface);
        return false;
    }
    true
}

pub(crate) fn validate_block_bound_surface_var_name(
    name: &str,
    loc: SourceLoc,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if let Some((base, field)) = split_field_path(name, errors) {
        let flat = format!("{base}.{field}");
        return validate_block_bound_surface_plain_name(&flat, loc, env, errors)
            && validate_block_bound_surface_plain_name(base, loc, env, errors);
    }
    validate_block_bound_surface_plain_name(name, loc, env, errors)
}

pub(crate) fn validate_block_bound_surface_expr(
    expr: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let mut ok = true;
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::Var { name, .. } => {
            ok &= validate_block_bound_surface_var_name(name, expr.loc(), env, errors);
        }
        Expr::Index { base, index, .. } => {
            if let Some(surface) = io_surface_name(base, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, expr.loc(), surface);
                    ok = false;
                }
                ok &= validate_block_bound_surface_expr(index, env, errors);
                return ok;
            }
            if let Some(surface) = dynamic_param_surface_name(base, env) {
                if !env.dynamic_param_indexing_allowed {
                    push_dynamic_param_index_scope_error(errors, expr, surface);
                    ok = false;
                }
                ok &= validate_block_bound_surface_expr(index, env, errors);
                return ok;
            }
            ok &= validate_block_bound_surface_expr(index, env, errors);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            if let Some(surface) = io_surface_name(base, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, expr.loc(), surface);
                    ok = false;
                } else if let Some(array_name) = io_surface_array_name(base, env) {
                    push_io_surface_value_error(errors, expr.loc(), array_name);
                    ok = false;
                }
            } else if let Some(surface) = dynamic_param_surface_name(base, env) {
                push_dynamic_param_surface_value_error(errors, expr.loc(), surface);
                ok = false;
            }
            for coordinate in [selector, channel, start, end].into_iter().flatten() {
                ok &= validate_block_bound_surface_expr(coordinate, env, errors);
            }
        }
        Expr::ArrayCtor { spec, init, .. } => {
            ok &= validate_block_bound_surface_expr(&spec.size, env, errors);
            if let Some(values) = init {
                for value in values {
                    ok &= validate_block_bound_surface_expr(value, env, errors);
                }
            }
        }
        Expr::ArrayLiteral { values, .. } | Expr::Tuple { values, .. } => {
            for value in values {
                ok &= validate_block_bound_surface_expr(value, env, errors);
            }
        }
        Expr::Compare { lhs, rhs, .. }
        | Expr::Logical { lhs, rhs, .. }
        | Expr::Binary { lhs, rhs, .. } => {
            ok &= validate_block_bound_surface_expr(lhs, env, errors);
            ok &= validate_block_bound_surface_expr(rhs, env, errors);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                ok &= validate_block_bound_surface_expr(arg, env, errors);
            }
        }
        Expr::UserCall { args, .. } => {
            for arg in args {
                ok &= validate_block_bound_surface_expr(&arg.expr, env, errors);
            }
        }
        Expr::Cast { expr: inner, .. }
        | Expr::UnaryNot { expr: inner, .. }
        | Expr::UnaryBitNot { expr: inner, .. } => {
            ok &= validate_block_bound_surface_expr(inner, env, errors);
        }
    }
    ok
}

pub(crate) fn validate_block_bound_surface_assign_target(
    target: &AssignTarget,
    loc: SourceLoc,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let mut ok = true;
    match target {
        AssignTarget::Var(name) => {
            ok &= validate_block_bound_surface_var_name(name, loc, env, errors);
        }
        AssignTarget::Index { base, .. } | AssignTarget::IndexedMember { base, .. } => {
            if let Some(surface) = io_surface_name(base, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, loc, surface);
                    ok = false;
                }
            } else if let Some(surface) = dynamic_param_surface_name(base, env) {
                if !env.dynamic_param_indexing_allowed {
                    push_loc_error(
                        errors,
                        loc,
                        format!(
                            "dynamic param indexing '{surface}[...]' is only allowed in block or sample"
                        ),
                    );
                    ok = false;
                }
            }
        }
        AssignTarget::Slice { base, .. } => {
            if let Some(surface) = io_surface_name(base, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, loc, surface);
                    ok = false;
                } else if let Some(array_name) = io_surface_array_name(base, env) {
                    push_io_surface_value_error(errors, loc, array_name);
                    ok = false;
                }
            } else if let Some(surface) = dynamic_param_surface_name(base, env) {
                push_dynamic_param_surface_value_error(errors, loc, surface);
                ok = false;
            }
        }
        AssignTarget::Tuple(names) => {
            for name in names.iter().filter_map(|target| target.binding()) {
                ok &= validate_block_bound_surface_var_name(name, loc, env, errors);
            }
        }
    }
    target
        .visit_selectors(|selector| ok &= validate_block_bound_surface_expr(selector, env, errors));
    ok
}

pub(crate) fn validate_expr(expr: &Expr, env: ExprEnv<'_>, errors: &mut Vec<Diagnostic>) {
    let mut pending = vec![expr];
    while let Some(expr) = pending.pop() {
        let start = pending.len();
        validate_expr_node(expr, env, errors, &mut pending);
        pending[start..].reverse();
    }
}

fn validate_expr_node<'a>(
    expr: &'a Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
    children: &mut Vec<&'a Expr>,
) {
    match expr {
        Expr::Number { .. } | Expr::Int { .. } | Expr::Bool { .. } => {}
        Expr::ArrayLiteral { values, .. } => {
            let nominal = values
                .first()
                .and_then(|value| infer_fixed_data_type(value, env));
            for value in values {
                let actual = infer_fixed_data_type(value, env);
                if let Some(expected @ DataType::Struct(_)) = nominal.as_ref() {
                    if actual.as_ref() != Some(expected) {
                        push_expr_error(
                            errors,
                            value,
                            format!(
                                "data array element {}",
                                data_type_mismatch(expected, actual.as_ref())
                            ),
                        );
                    }
                }
                children.push(value);
            }
            push_expr_error(
                errors,
                expr,
                "array literals are only allowed in typed array declarations and parameter defaults",
            );
        }
        Expr::Tuple { values, .. } => {
            for value in values {
                children.push(value);
            }
        }
        Expr::Var { name, .. } => {
            if is_builtin_constant_name(name) {
                return;
            }
            if !name.contains('.') && env.has_value_binding(name) {
                return;
            }
            if let Some(name) = block_audio_input_name(name, env) {
                push_block_audio_input_error(errors, expr.loc(), name);
                return;
            }
            if let Some((root, field)) = name.split_once('.') {
                if env.has_value_binding(root) {
                    if env.tuple_vars.contains_key(root) {
                        push_expr_error(
                            errors,
                            expr,
                            format!("tuple binding '{root}' has no field '{field}'"),
                        );
                        return;
                    }
                    let kind = if env.locals.contains(root) {
                        "loop variable"
                    } else {
                        "value binding"
                    };
                    push_expr_error(
                        errors,
                        expr,
                        format!("{kind} '{root}' is scalar and has no field '{field}'"),
                    );
                    return;
                }
            }
            if let Some((base, field)) = split_field_path(name, errors) {
                if let Some(struct_name) = env.struct_name(base) {
                    let owner_kind = if env.param_structs.contains_key(base) {
                        "parameter"
                    } else {
                        "instance"
                    };
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

                if is_struct_array_root(env.declared_symbols, base) {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "'{base}' is an array of structs and must be indexed before accessing field '{field}'"
                        ),
                    );
                    return;
                }

                let flat = format!("{base}.{field}");
                if let Some(name) = io_surface_name(&flat, env) {
                    if !env.io_surface_access_allowed {
                        push_io_surface_scope_error(errors, expr.loc(), name);
                        return;
                    }
                }
                if let Some(name) = dynamic_param_surface_name(&flat, env) {
                    push_dynamic_param_surface_value_error(errors, expr.loc(), name);
                    return;
                }
                if env.array_vars.contains_key(&flat) {
                    push_expr_error(
                        errors,
                        expr,
                        format!("array symbol '{flat}' must be indexed"),
                    );
                    return;
                }
                if env.outputs.contains(&flat) {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "cannot read output symbol '{flat}' owned by the current program/proc"
                        ),
                    );
                    return;
                }
                if let Some(name) = io_surface_array_name(&flat, env) {
                    push_io_surface_value_error(errors, expr.loc(), name);
                    return;
                }
                if !env.known_scalars.contains(&flat)
                    && !env.state_scalars.contains_key(&flat)
                    && !env.local_aliases.contains_key(&flat)
                    && !env.locals.contains(&flat)
                {
                    push_expr_error(
                        errors,
                        expr,
                        format!("unknown symbol '{flat}' in expression"),
                    );
                }
                return;
            }

            if env.struct_name(name).is_some() && env.param_structs.contains_key(name) {
                if name == "self" {
                    return;
                }
                push_expr_error(
                    errors,
                    expr,
                    format!("struct parameter '{}' must be accessed via fields", name),
                );
                return;
            }
            if env.struct_name(name).is_some() {
                push_expr_error(
                    errors,
                    expr,
                    format!("struct instance '{name}' must be accessed via fields"),
                );
                return;
            }
            if let Some(name) = io_surface_name(name, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, expr.loc(), name);
                    return;
                }
            }
            if env.output_arrays.contains(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "cannot read output array symbol '{name}' owned by the current program/proc"
                    ),
                );
                return;
            }
            if let Some(name) = dynamic_param_surface_name(name, env) {
                push_dynamic_param_surface_value_error(errors, expr.loc(), name);
                return;
            }
            if let Some(name) = io_surface_array_name(name, env) {
                push_io_surface_value_error(errors, expr.loc(), name);
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
            if env.outputs.contains(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("cannot read output symbol '{name}' owned by the current program/proc"),
                );
                return;
            }
            if !env.known_scalars.contains(name) && !env.locals.contains(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!("unknown symbol '{name}' in expression"),
                );
            }
        }
        Expr::Index { base, index, .. } => {
            let flattened_struct_array_leaf =
                split_simple_field_path(base).and_then(|(root, field)| {
                    env.struct_name(root).and_then(|struct_name| {
                        resolve_flattened_struct_array_leaf_type(
                            struct_name,
                            field,
                            env.struct_defs,
                        )
                    })
                });
            let lowered_state_field = split_simple_field_path(base)
                .map(|(_, field)| field)
                .filter(|field| {
                    env.array_vars.contains_key(*field)
                        || is_declared_data_array_symbol(env.declared_symbols, field)
                });
            let base_is_registered_array = env.array_vars.contains_key(base)
                || lowered_state_field.is_some()
                || flattened_struct_array_leaf.is_some();
            let lexical_root = base.split('.').next().unwrap_or(base);
            if env.has_scalar_binding(lexical_root) {
                let kind = if env.locals.contains(lexical_root) {
                    "loop variable"
                } else {
                    "value binding"
                };
                push_expr_error(
                    errors,
                    expr,
                    format!("{kind} '{lexical_root}' is scalar and cannot be indexed"),
                );
                children.push(index);
                return;
            }
            if lexical_root != base && env.tuple_vars.contains_key(lexical_root) {
                push_expr_error(
                    errors,
                    expr,
                    format!("tuple binding '{lexical_root}' has no fields"),
                );
                children.push(index);
                return;
            }
            if let Some(name) = block_audio_input_name(base, env) {
                push_block_audio_input_error(errors, expr.loc(), name);
                children.push(index);
                return;
            }
            // Struct-array field access is flattened to `root.field[index]` before
            // validation. Once that flattened array is registered, validate it as
            // an array below instead of interpreting the dotted storage name as a
            // direct field path through the containing struct.
            if !base_is_registered_array {
                if let Some((root, field)) = split_field_path(base, errors) {
                    if let Some(struct_name) = env.struct_name(root) {
                        let owner_kind = if env.param_structs.contains_key(root) {
                            "parameter"
                        } else {
                            "instance"
                        };
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
                        if !matches!(
                            field_decl.ty,
                            TypedFieldType::Array(_) | TypedFieldType::Tuple(_)
                        ) {
                            push_expr_error(
                                errors,
                                expr,
                                format!(
                                    "field '{}.{}' is not array or tuple and cannot be indexed",
                                    root, field
                                ),
                            );
                        }
                        if let TypedFieldType::Tuple(ref elem_tys) = field_decl.ty {
                            // Validate const index for tuple field
                            match index.as_ref() {
                                Expr::Int { value, .. } => {
                                    let idx = *value as usize;
                                    if idx >= elem_tys.len() {
                                        push_expr_error(
                                            errors,
                                            expr,
                                            format!(
                                                "tuple field '{}.{}' index {idx} out of bounds (has {} elements)",
                                                root, field, elem_tys.len()
                                            ),
                                        );
                                    }
                                }
                                _ => {
                                    push_expr_error(
                                        errors,
                                        expr,
                                        "tuple element index must be a compile-time integer constant",
                                    );
                                }
                            }
                        } else {
                            children.push(index);
                        }
                        return;
                    }
                    if is_struct_array_root(env.declared_symbols, root)
                        && !env.proc_array_roots.contains_key(root)
                    {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "'{root}' is an array of structs and must be indexed before accessing field '{field}'"
                            ),
                        );
                        return;
                    }
                }
            }
            if let Some(name) = io_surface_name(base, env) {
                if !env.io_surface_access_allowed {
                    push_io_surface_scope_error(errors, expr.loc(), name);
                    children.push(index);
                    return;
                }
            }
            if let Some(name) = dynamic_param_surface_name(base, env) {
                if !env.dynamic_param_indexing_allowed {
                    push_dynamic_param_index_scope_error(errors, expr, name);
                }
                children.push(index);
                return;
            }
            if matches!(base.as_str(), "outs" | "kouts") {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "cannot read output symbol '{}[i]' owned by the current program/proc",
                        base
                    ),
                );
                children.push(index);
                return;
            }
            if env.output_arrays.contains(base) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "cannot read output array symbol '{base}[...]' owned by the current program/proc"
                    ),
                );
                children.push(index);
                return;
            }
            if (match base.as_str() {
                "ins" => env.port_index_ins,
                _ => None,
            })
            .is_some()
            {
                if env.scope == ScopeKind::Init {
                    push_expr_error(
                        errors,
                        expr,
                        format!("'{base}[...]' is not allowed in init scope"),
                    );
                }
                children.push(index);
                return;
            }
            if matches!(base.as_str(), "ins" | "outs" | "kouts" | "params" | "kins") {
                let requirement = match base.as_str() {
                    "outs" => "explicit 'outs' block declaration with uniform types".to_owned(),
                    "kouts" => "explicit 'kouts' block declaration with uniform types".to_owned(),
                    "kins" => "top-level explicit 'kins' declaration with uniform types".to_owned(),
                    _ => format!("explicit '{base}' block declaration with uniform types"),
                };
                push_expr_error(
                    errors,
                    expr,
                    format!("'{base}[i]' requires an {requirement}"),
                );
                children.push(index);
                return;
            }
            if is_declared_buffer_array_info(env.declared_symbols, base) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "buffer collection element '{base}[...]' is a reference and is only valid as a buffer argument or method receiver"
                    ),
                );
                children.push(index);
                return;
            }
            if !base_is_registered_array
                && !has_declared_buffer_symbol_info(env.declared_symbols, base)
                && !is_declared_struct_array_root_symbol(env.declared_symbols, base)
                && !env.tuple_vars.contains_key(base)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!("indexed expression '{base}[...]' is not a array/buffer symbol"),
                );
            } else if let Some(&arity) = env.tuple_vars.get(base) {
                match index.as_ref() {
                    Expr::Int { value, .. } => {
                        let idx = *value as usize;
                        if idx >= arity {
                            push_expr_error(errors, expr, format!(
                                "tuple index {idx} is out of bounds for '{base}' with {arity} elements"
                            ));
                        }
                    }
                    _ => {
                        push_expr_error(
                            errors,
                            expr,
                            "tuple element index must be a compile-time integer constant",
                        );
                    }
                }
                return;
            } else if is_declared_multichannel_buffer_info(env.declared_symbols, base)
                && !is_declared_buffer_array_info(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "indexed expression '{base}[...]' uses mono form on a multichannel buffer; use '{base}[channel, frame]'"
                    ),
                );
            }
            children.push(index);
        }
        Expr::Slice {
            base,
            selector,
            channel,
            start,
            end,
            ..
        } => {
            let lexical_root = base.split('.').next().unwrap_or(base);
            if env.has_scalar_binding(lexical_root) {
                let kind = if env.locals.contains(lexical_root) {
                    "loop variable"
                } else {
                    "value binding"
                };
                push_expr_error(
                    errors,
                    expr,
                    format!("{kind} '{lexical_root}' is scalar and cannot be sliced"),
                );
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    children.push(coordinate);
                }
                return;
            }
            if env.tuple_vars.contains_key(lexical_root) {
                push_expr_error(
                    errors,
                    expr,
                    format!("tuple binding '{lexical_root}' cannot be sliced"),
                );
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    children.push(coordinate);
                }
                return;
            }
            if let Some(name) = block_audio_input_name(base, env) {
                push_block_audio_input_error(errors, expr.loc(), name);
                for coordinate in [selector, channel, start, end].into_iter().flatten() {
                    children.push(coordinate);
                }
                return;
            }
            if let Some((root, field)) = split_field_path(base, errors) {
                if let Some(struct_name) = env.struct_name(root) {
                    let owner_kind = if env.param_structs.contains_key(root) {
                        "parameter"
                    } else {
                        "instance"
                    };
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
                } else if is_struct_array_root(env.declared_symbols, root)
                    && !env.array_vars.contains_key(base)
                    && !env.proc_array_roots.contains_key(root)
                {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "'{root}' is an array of structs and must be indexed before accessing field '{field}'"
                        ),
                    );
                    return;
                }
            } else if let Some(name) =
                io_surface_name(base, env).filter(|_| !env.io_surface_access_allowed)
            {
                push_io_surface_scope_error(errors, expr.loc(), name);
            } else if env.output_arrays.contains(base) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "cannot read output array symbol '{base}[...]' owned by the current program/proc"
                    ),
                );
            } else if let Some(name) = dynamic_param_surface_name(base, env) {
                push_dynamic_param_surface_value_error(errors, expr.loc(), name);
            } else if let Some(name) = io_surface_array_name(base, env) {
                push_io_surface_value_error(errors, expr.loc(), name);
            } else if !env.array_vars.contains_key(base)
                && !env.local_array_aliases.contains_key(base)
                && !has_declared_buffer_symbol_info(env.declared_symbols, base)
                && !is_declared_struct_array_root_symbol(env.declared_symbols, base)
            {
                push_expr_error(
                    errors,
                    expr,
                    format!("slice expression '{base}[...]' is not an array/buffer symbol"),
                );
            } else if has_declared_buffer_symbol_info(env.declared_symbols, base) {
                let is_array = is_declared_buffer_array_info(env.declared_symbols, base);
                let is_multichannel =
                    is_declared_multichannel_buffer_info(env.declared_symbols, base);
                let is_collection_span = is_array && selector.is_none() && channel.is_none();
                if is_collection_span {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "buffer collection slice '{base}[...]' is a reference and is only valid as a buffer-collection argument"
                        ),
                    );
                }
                if !is_collection_span && selector.is_some() != is_array {
                    push_expr_error(
                        errors,
                        expr,
                        if is_array {
                            format!("buffer collection '{base}' must select a slot before slicing")
                        } else {
                            format!("buffer '{base}' is not a buffer collection")
                        },
                    );
                }
                if !is_collection_span && channel.is_some() != is_multichannel {
                    push_expr_error(
                        errors,
                        expr,
                        if is_multichannel {
                            format!(
                                "multichannel buffer slice '{base}[...]' requires '[channel, start:end]'"
                            )
                        } else {
                            format!("mono buffer slice '{base}[...]' does not take a channel")
                        },
                    );
                }
            } else if selector.is_some() || channel.is_some() {
                push_expr_error(
                    errors,
                    expr,
                    format!("array slice '{base}[...]' does not support buffer coordinates"),
                );
            }
            for coordinate in [
                selector.as_deref(),
                channel.as_deref(),
                start.as_deref(),
                end.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                children.push(coordinate);
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
                    children.push(value);
                }
            }
        }
        Expr::Cast { expr, .. } | Expr::UnaryNot { expr, .. } | Expr::UnaryBitNot { expr, .. } => {
            children.push(expr);
        }
        Expr::Logical { lhs, rhs, .. } => {
            children.push(lhs);
            children.push(rhs);
        }
        Expr::Compare { lhs, rhs, .. } => {
            children.push(lhs);
            children.push(rhs);
        }
        Expr::Call { func, args, .. } => {
            for arg in args {
                children.push(arg);
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
            if is_internal_buffer_2d_fn(name) {
                validate_internal_buffer_index_call(name, args, env, expr.loc(), errors);
                if name == WRITE_UNSAFE_FN {
                    push_expr_error(
                        errors,
                        expr,
                        "'write_unsafe' is a statement and cannot be used as a value",
                    );
                }
                return;
            }
            if validate_indexed_buffer_metadata_call(name, args, env, expr.loc(), errors) {
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
            if name == STRUCT_ARRAY_FIELD_INDEX_SENTINEL {
                let Some((base, index, field, field_index)) =
                    crate::array_structs::extract_safi_args(args)
                else {
                    push_expr_error(errors, expr, "malformed indexed struct-array field access");
                    return;
                };
                let struct_name = env
                    .struct_array_roots
                    .get(&base)
                    .map(|root| root.struct_name.as_str())
                    .or_else(|| {
                        env.local_array_aliases
                            .get(&base)
                            .and_then(|alias| alias.elem_struct.as_deref())
                    });
                let Some(struct_name) = struct_name else {
                    push_expr_error(errors, expr, format!("'{base}' is not an array of structs"));
                    return;
                };
                let Some(field_decl) = crate::declaration_coercion::resolve_struct_field_decl(
                    struct_name,
                    &field,
                    env.struct_defs,
                ) else {
                    push_expr_error(
                        errors,
                        expr,
                        format!("struct '{struct_name}' has no field '{field}'"),
                    );
                    return;
                };
                validate_expr(&index, env, errors);
                require_expr_numeric_type(
                    &index,
                    infer_call_argument_scalar_type(&index, env),
                    "struct-array index",
                    errors,
                );
                match &field_decl.ty {
                    TypedFieldType::Array(_) => {
                        validate_expr(&field_index, env, errors);
                        require_expr_numeric_type(
                            &field_index,
                            infer_call_argument_scalar_type(&field_index, env),
                            "struct field index",
                            errors,
                        );
                    }
                    TypedFieldType::Tuple(types) => {
                        validate_expr(&field_index, env, errors);
                        let Expr::Int { value, .. } = field_index else {
                            push_expr_error(
                                errors,
                                expr,
                                "tuple field index must be a compile-time integer constant",
                            );
                            return;
                        };
                        if usize::try_from(value)
                            .ok()
                            .is_none_or(|index| index >= types.len())
                        {
                            push_expr_error(
                                errors,
                                expr,
                                format!(
                                    "tuple field index {value} is out of bounds for '{base}[...].{field}' with {} elements",
                                    types.len()
                                ),
                            );
                        }
                    }
                    TypedFieldType::Scalar(_) | TypedFieldType::Struct => push_expr_error(
                        errors,
                        expr,
                        format!("field '{field}' of struct '{struct_name}' is not indexable"),
                    ),
                }
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
                if let Some(function) = name.strip_prefix(PROC_FIELD_SENTINEL_PREFIX) {
                    if env.fn_signatures.contains_key(function) {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "field selection on function result '{function}(...)' is not supported; bind the result first"
                            ),
                        );
                        return;
                    }
                }
                if let Some(base) = parse_array_len_instance_base(name) {
                    if is_builtin_len_receiver(base, env) {
                        validate_data_len_builtin_call(name, args, expr.loc(), errors);
                        return;
                    }
                }
                if let Some(base) = parse_buffer_chans_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_metadata_builtin_call(
                            name,
                            base,
                            args,
                            "chans",
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
                if let Some(base) = parse_buffer_bound_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_metadata_builtin_call(
                            name,
                            base,
                            args,
                            "bound",
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
                if let Some(base) = parse_buffer_samplerate_instance_base(name) {
                    if is_builtin_buffer_receiver(base, env) {
                        validate_buffer_metadata_builtin_call(
                            name,
                            base,
                            args,
                            "samplerate",
                            env,
                            expr.loc(),
                            errors,
                        );
                        return;
                    }
                }
            }
            if is_internal_proc_index_validation_call(name) {
                for (idx, arg) in args.iter().enumerate() {
                    if is_internal_proc_index_validation_arg(args, idx, arg.name.as_deref()) {
                        continue;
                    }
                    children.push(&arg.expr);
                }
                return;
            }
            if let Some(sig) = env.fn_signatures.get(name) {
                let display_name = sig.display_name.as_deref().unwrap_or(name);
                if sig.type_params.is_empty() {
                    if !type_args.is_empty() {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "function '{}' is not generic and cannot take type arguments",
                                display_name
                            ),
                        );
                    }
                } else if !type_args.is_empty() {
                    if type_args.len() != sig.type_params.len() {
                        push_expr_error(
                            errors,
                            expr,
                            format!(
                                "function '{}' expects {} type arguments, got {}",
                                display_name,
                                sig.type_params.len(),
                                type_args.len()
                            ),
                        );
                    }
                    for ta in type_args {
                        if matches!(ta, CallTypeArg::Primitive(PrimitiveType::Bool)) {
                            push_expr_error(
                                errors,
                                expr,
                                format!(
                                    "'bool' is not valid as a generic type argument for '{}'; use f32, f64, i32, or i64",
                                    display_name
                                ),
                            );
                        }
                    }
                }

                let forbid_self_named = sig.params.first().map(String::as_str) == Some("self");
                let mut binding_errors = Vec::new();
                let resolved = resolve_call_args_at(
                    args,
                    &sig.params,
                    &sig.defaults,
                    forbid_self_named,
                    false,
                    &format!("function '{display_name}' call"),
                    expr.loc(),
                    &mut binding_errors,
                );
                let call_binding_is_valid = binding_errors.is_empty();
                for diagnostic in binding_errors {
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                }
                for (idx, arg) in resolved.into_iter().enumerate() {
                    if let Some(arg) = arg {
                        let param_ty = sig.param_types.get(idx).and_then(|t| t.as_ref());
                        let param_readonly = sig
                            .params
                            .get(idx)
                            .is_some_and(|param| sig.readonly_data_params.contains(param));
                        if let Some(FnParamType::BufferArray { buffer, len }) = param_ty {
                            validate_buffer_array_param_call_arg(
                                display_name,
                                idx,
                                &sig.params,
                                buffer,
                                *len,
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            );
                            continue;
                        }
                        if let Some(FnParamType::Buffer(buffer_ty)) = param_ty {
                            validate_buffer_param_call_arg(
                                display_name,
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
                        if matches!(param_ty, Some(FnParamType::Struct(_)))
                            && validate_aggregate_unsafe_reference_arg(arg, env, errors)
                        {
                            continue;
                        }
                        if let Some(FnParamType::Struct(expected)) = param_ty {
                            let actual = infer_fixed_data_type(arg, env);
                            if let Some(DataType::Struct(actual)) = actual.as_ref() {
                                if env.struct_defs.contains_key(expected) && actual != expected {
                                    push_expr_error(
                                        errors,
                                        arg,
                                        format!(
                                            "argument requires struct '{expected}', got '{actual}'"
                                        ),
                                    );
                                }
                                validate_fixed_data_expr(arg, env, errors);
                                continue;
                            }
                            if env.struct_defs.contains_key(expected) {
                                if !is_internal_proc_helper_call(name)
                                    || !matches!(arg, Expr::Index { .. } | Expr::Var { .. })
                                {
                                    push_expr_error(
                                        errors,
                                        arg,
                                        format!(
                                            "function '{display_name}' argument '{}' {}",
                                            sig.params[idx],
                                            data_type_mismatch(
                                                &DataType::Struct(expected.clone()),
                                                actual.as_ref(),
                                            )
                                        ),
                                    );
                                    children.push(arg);
                                }
                                continue;
                            }
                        }
                        if is_function_array_param(param_ty) {
                            if reject_protected_array_pointer_call_arg(
                                display_name,
                                &sig.params[idx],
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            ) {
                                continue;
                            }
                            if reject_immutable_array_call_arg(
                                display_name,
                                &sig.params[idx],
                                param_readonly,
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            ) {
                                continue;
                            }
                            if let Some(param_ty) = param_ty {
                                validate_array_param_call_arg(
                                    display_name,
                                    &sig.params[idx],
                                    param_ty,
                                    arg,
                                    env,
                                    errors,
                                );
                            }
                            if matches!(arg, Expr::Slice { .. }) {
                                children.push(arg);
                            } else if let Expr::ArrayLiteral { values, .. } = arg {
                                for value in values {
                                    children.push(value);
                                }
                            } else if matches!(arg, Expr::ArrayCtor { .. } | Expr::UserCall { .. })
                            {
                                children.push(arg);
                            }
                            // Array params accept data-like args.
                            continue;
                        }
                        if matches!(param_ty, Some(FnParamType::BareBuffer)) {
                            // Bare buffer params accept data-like args.
                            continue;
                        }
                        if param_ty.is_none() && is_by_ref_call_arg_expr(arg, env) {
                            if reject_protected_array_pointer_call_arg(
                                display_name,
                                &sig.params[idx],
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            ) {
                                continue;
                            }
                            if reject_immutable_array_call_arg(
                                display_name,
                                &sig.params[idx],
                                param_readonly,
                                arg,
                                env,
                                expr.loc(),
                                errors,
                            ) {
                                continue;
                            }
                            continue;
                        }
                        if let Some(FnParamType::Tuple(expected)) = param_ty {
                            validate_tuple_param_call_arg(
                                display_name,
                                &sig.params[idx],
                                expected,
                                arg,
                                env,
                                errors,
                            );
                        }
                        if let Some(FnParamType::Primitive(expected)) = param_ty {
                            if infer_call_argument_tuple_types(arg, env).is_some()
                                || call_array_arg_info(arg, env).is_some()
                            {
                                push_expr_error(
                                    errors,
                                    arg,
                                    format!(
                                        "function '{display_name}' parameter '{}' expects a scalar value",
                                        sig.params[idx]
                                    ),
                                );
                            } else {
                                let actual = infer_call_argument_scalar_type(arg, env);
                                require_expr_assignable_type(
                                    arg,
                                    actual,
                                    *expected,
                                    &format!(
                                        "function '{display_name}' argument '{}'",
                                        sig.params[idx]
                                    ),
                                    errors,
                                );
                            }
                        }
                        children.push(arg);
                    } else if let Some(default) = sig.defaults.get(idx).and_then(|d| d.as_ref()) {
                        validate_default_expr(
                            default,
                            errors,
                            &format!("function '{display_name}' default '{}'", sig.params[idx]),
                        );
                        let param_ty = sig.param_types.get(idx).and_then(|ty| ty.as_ref());
                        match param_ty {
                            Some(FnParamType::Primitive(expected)) => {
                                let actual = infer_call_argument_scalar_type(default, env);
                                require_expr_assignable_type(
                                    default,
                                    actual,
                                    *expected,
                                    &format!(
                                        "function '{display_name}' default '{}'",
                                        sig.params[idx]
                                    ),
                                    errors,
                                );
                            }
                            Some(FnParamType::Tuple(expected)) => {
                                validate_tuple_param_call_arg(
                                    display_name,
                                    &sig.params[idx],
                                    expected,
                                    default,
                                    env,
                                    errors,
                                );
                            }
                            Some(param_ty) if is_function_array_param(Some(param_ty)) => {
                                validate_array_param_call_arg(
                                    display_name,
                                    &sig.params[idx],
                                    param_ty,
                                    default,
                                    env,
                                    errors,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                if sig.requires_call_specialization && call_binding_is_valid {
                    let diagnostic = Diagnostic::semantic_span(
                        format!(
                            "function '{display_name}' call does not provide concrete argument types required for specialization"
                        ),
                        expr.loc(),
                    );
                    if !errors.contains(&diagnostic) {
                        errors.push(diagnostic);
                    }
                }
                return;
            }

            if env.struct_defs.contains_key(name) {
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "struct constructors are only allowed as direct assignments or call arguments; found '{}' call in {}",
                        name, env.diagnostic_scope
                    ),
                );
                for arg in args {
                    children.push(&arg.expr);
                }
                return;
            }

            if env.proc_event_names.contains(name) {
                let has_indexed_proc_receiver = args.first().is_some_and(|arg| {
                    matches!(
                        &arg.expr,
                        Expr::Index { base, .. } if env.proc_array_roots.contains_key(base)
                    ) && matches!(arg.name.as_deref(), None | Some(METHOD_RECEIVER_ARG))
                });
                if has_indexed_proc_receiver {
                    for arg in args {
                        children.push(&arg.expr);
                    }
                    return;
                }
                push_expr_error(
                    errors,
                    expr,
                    format!(
                        "proc event '{name}' is receiver-only; unqualified calls never resolve to proc events. Use a proc-local def for shared internal logic, or call the event on a child proc instance"
                    ),
                );
                return;
            }

            push_expr_error(
                errors,
                expr,
                format!("unknown function '{name}' in expression"),
            );
        }
        Expr::Binary { lhs, rhs, .. } => {
            children.push(lhs);
            children.push(rhs);
        }
    }
}

pub(crate) fn validate_standalone_expr_statement(
    expr: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Expr::UserCall { name, args, .. } = expr {
        if name == WRITE_UNSAFE_FN {
            validate_internal_buffer_index_call(name, args, env, expr.loc(), errors);
            return;
        }
    }
    validate_expr(expr, env, errors);
}

fn validate_indexed_buffer_metadata_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(method) = name
        .strip_prefix(PROC_INDEX_CALL_SENTINEL)
        .and_then(|suffix| suffix.strip_prefix('.'))
    else {
        return false;
    };
    if !matches!(
        method,
        ARRAY_LEN_METHOD | BUFFER_BOUND_METHOD | BUFFER_CHANS_METHOD | BUFFER_SAMPLERATE_METHOD
    ) {
        return false;
    }
    let base = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_BASE_ARG));
    let selector = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(PROC_INDEX_EXPR_ARG));
    let valid_base = base.is_some_and(|arg| {
        matches!(&arg.expr, Expr::Var { name, .. } if is_declared_buffer_array_info(env.declared_symbols, name))
    });
    if !valid_base || selector.is_none() || args.len() != 2 {
        push_loc_error(
            errors,
            loc,
            format!("indexed buffer method '{method}' requires one buffer-array element"),
        );
        return true;
    }
    if let Some(selector) = selector {
        validate_expr(&selector.expr, env, errors);
    }
    true
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

fn is_internal_proc_index_validation_call(name: &str) -> bool {
    name == PROC_INDEX_CALL_SENTINEL
        || name
            .strip_prefix(PROC_FIELD_SENTINEL_PREFIX)
            .is_some_and(|raw| raw == PROC_INDEX_CALL_SENTINEL)
}

fn is_internal_proc_index_validation_arg(
    args: &[CallArg],
    idx: usize,
    arg_name: Option<&str>,
) -> bool {
    if matches!(
        arg_name,
        Some(PROC_INDEX_BASE_ARG)
            | Some(PROC_INDEX_EXPR_ARG)
            | Some(PROC_INDEX_UNCHECKED_ARG)
            | Some(PROC_FIELD_SENTINEL_ARG)
    ) {
        return true;
    }
    if arg_name.is_some() {
        return false;
    }
    args.iter()
        .take(idx)
        .filter(|arg| arg.name.is_none())
        .count()
        < 2
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

/// Returns true when `name` is an array-of-structs root – i.e. it is a DataArray
/// AND has dotted child entries (`name.field`) that are also DataArray.
/// Plain data arrays (e.g. `x: f32[4]`) only have the root entry, no children.
fn is_struct_array_root(declared_symbols: &DeclaredSymbolMap, name: &str) -> bool {
    if !is_declared_data_array_symbol(declared_symbols, name) {
        return false;
    }
    let prefix = format!("{name}.");
    declared_symbols.iter().any(|(k, info)| {
        k.starts_with(&prefix) && matches!(info, DeclaredSymbolInfo::DataArray { .. })
    })
}

fn is_builtin_len_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    let root = base.split('.').next().unwrap_or(base);
    !env.has_value_binding(root)
        && (call_array_symbol_info(base, env).is_some()
            || has_declared_buffer_symbol_info(env.declared_symbols, base))
}

fn is_builtin_buffer_receiver(base: &str, env: ExprEnv<'_>) -> bool {
    let root = base.split('.').next().unwrap_or(base);
    !env.has_value_binding(root) && has_declared_buffer_symbol_info(env.declared_symbols, base)
}

fn is_by_ref_call_arg_var(name: &str, env: ExprEnv<'_>) -> bool {
    let root = name.split('.').next().unwrap_or(name);
    if env.has_value_binding(root) {
        return false;
    }
    env.struct_name(name).is_some()
        || call_array_symbol_info(name, env).is_some()
        || protected_proc_view_arg_name(name, env).is_some()
        || has_declared_buffer_symbol_info(env.declared_symbols, name)
}

fn is_by_ref_call_arg_expr(expr: &Expr, env: ExprEnv<'_>) -> bool {
    match expr {
        Expr::Var { name, .. } => is_by_ref_call_arg_var(name, env),
        Expr::Slice { base, .. } => is_by_ref_call_arg_var(base, env),
        _ => false,
    }
}

fn immutable_array_alias_arg_name<'a>(expr: &'a Expr, env: ExprEnv<'_>) -> Option<&'a str> {
    let name = match expr {
        Expr::Var { name, .. } => name.as_str(),
        Expr::Slice { base, .. } => base.as_str(),
        _ => return None,
    };
    env.local_array_aliases
        .get(name)
        .filter(|alias| !alias.writable)
        .map(|_| name)
}

fn is_function_array_param(param_ty: Option<&FnParamType>) -> bool {
    matches!(
        param_ty,
        Some(FnParamType::Array(_))
            | Some(FnParamType::ArrayGeneric(_))
            | Some(FnParamType::SizedArray { .. })
    )
}

pub(crate) fn infer_call_argument_tuple_types(
    expr: &Expr,
    env: ExprEnv<'_>,
) -> Option<Vec<PrimitiveType>> {
    match expr {
        Expr::Tuple { values, .. } => values
            .iter()
            .map(|value| {
                let inferred = infer_call_argument_scalar_type(value, env);
                effective_untyped_assignment_type(value, inferred).or(inferred)
            })
            .collect(),
        Expr::Var { name, .. } => {
            if let Some(types) = tracked_local_tuple_types(name, env.tuple_vars, env.local_aliases)
            {
                return Some(types);
            }
            let lexical_root = name.split('.').next().unwrap_or(name);
            if env.has_value_binding(lexical_root) {
                return None;
            }
            let (root, field) = split_simple_field_path(name)?;
            let struct_name = env.struct_name(root)?;
            match &resolve_struct_field_decl(struct_name, field, env.struct_defs)?.ty {
                TypedFieldType::Tuple(types) => Some(types.clone()),
                TypedFieldType::Scalar(_) | TypedFieldType::Struct | TypedFieldType::Array(_) => {
                    None
                }
            }
        }
        Expr::UserCall { name, .. } => match env
            .fn_signatures
            .get(name)
            .and_then(|signature| signature.return_type.as_ref())
        {
            Some(ReturnType::Tuple(types)) => Some(types.clone()),
            Some(ReturnType::Scalar(_) | ReturnType::Data(_)) | None => None,
        },
        _ => None,
    }
}

fn validate_tuple_param_call_arg(
    function_name: &str,
    param_name: &str,
    expected: &[PrimitiveType],
    arg: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    let Some(actual) = infer_call_argument_tuple_types(arg, env) else {
        if is_definitely_scalar_call_arg(arg, env) || call_array_arg_info(arg, env).is_some() {
            push_expr_error(
                errors,
                arg,
                format!(
                    "function '{function_name}' parameter '{param_name}' expects a tuple value"
                ),
            );
        }
        return;
    };
    if actual.len() != expected.len() {
        push_expr_error(
            errors,
            arg,
            format!(
                "function '{function_name}' parameter '{param_name}' expects tuple arity {}, got {}",
                expected.len(),
                actual.len()
            ),
        );
        return;
    }

    if let Expr::Tuple { values, .. } = arg {
        for ((value, actual), expected) in values.iter().zip(&actual).zip(expected) {
            require_expr_assignable_type(
                value,
                Some(*actual),
                *expected,
                &format!("function '{function_name}' argument '{param_name}'"),
                errors,
            );
        }
        return;
    }

    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if actual == expected || can_implicitly_assign(*actual, *expected) {
            continue;
        }
        push_expr_error(
            errors,
            arg,
            format!(
                "function '{function_name}' parameter '{param_name}' tuple element {index} type mismatch: cannot assign {} to {}",
                actual.name(),
                expected.name()
            ),
        );
    }
}

#[derive(Clone, Debug)]
struct CallArrayArgInfo {
    elem: CallArrayArgElem,
    len: Option<usize>,
}

#[derive(Clone, Debug)]
enum CallArrayArgElem {
    Primitive(PrimitiveType),
    Nominal(String),
    /// Empty literals carry a length but no element constraint.
    Unknown,
    NominalUnknown,
}

fn call_array_value_elem(value: &Expr, env: ExprEnv<'_>) -> Option<CallArrayArgElem> {
    let nominal = match value {
        Expr::Var { name, .. } => env
            .struct_name(name)
            .map(str::to_owned)
            .map(CallArrayArgElem::Nominal),
        Expr::Index { base, .. } => {
            call_array_symbol_info(base, env).and_then(|info| match info.elem {
                CallArrayArgElem::Nominal(name) => Some(CallArrayArgElem::Nominal(name)),
                CallArrayArgElem::Primitive(_)
                | CallArrayArgElem::Unknown
                | CallArrayArgElem::NominalUnknown => None,
            })
        }
        Expr::UserCall { name, .. } if env.struct_defs.contains_key(name) => {
            Some(CallArrayArgElem::Nominal(name.clone()))
        }
        Expr::UserCall { name, .. } => env.fn_signatures.get(name).and_then(|signature| {
            match signature.return_type.as_ref()? {
                ReturnType::Data(DataType::Struct(name)) => {
                    Some(CallArrayArgElem::Nominal(name.clone()))
                }
                _ => None,
            }
        }),
        _ => None,
    };
    nominal.or_else(|| infer_call_argument_scalar_type(value, env).map(CallArrayArgElem::Primitive))
}

fn struct_field_array_info(name: &str, env: ExprEnv<'_>) -> Option<CallArrayArgInfo> {
    let (root, field) = split_simple_field_path(name)?;
    let struct_name = env.struct_name(root)?;
    let declaration = resolve_struct_field_decl(struct_name, field, env.struct_defs)?;
    let TypedFieldType::Array(len) = &declaration.ty else {
        return None;
    };
    Some(CallArrayArgInfo {
        elem: declaration
            .array_elem_struct
            .clone()
            .map(CallArrayArgElem::Nominal)
            .unwrap_or(CallArrayArgElem::Primitive(
                declaration.array_elem_ty.unwrap_or(PrimitiveType::F32),
            )),
        len: Some(*len),
    })
}

fn direct_array_symbol_info(name: &str, env: ExprEnv<'_>) -> Option<CallArrayArgInfo> {
    let lexical_root = name.split('.').next().unwrap_or(name);
    if env.has_value_binding(lexical_root) {
        return None;
    }
    if let Some(alias) = env.local_array_aliases.get(name) {
        let elem = alias
            .elem_struct
            .clone()
            .map(CallArrayArgElem::Nominal)
            .unwrap_or(CallArrayArgElem::Primitive(alias.elem_ty));
        return Some(CallArrayArgInfo {
            elem,
            len: alias.static_len,
        });
    }
    if env.struct_name(name).is_some() {
        return None;
    }
    if let Some(proc_array) = env.proc_array_roots.get(name) {
        return Some(CallArrayArgInfo {
            elem: CallArrayArgElem::Nominal(proc_array.proc_name.clone()),
            len: crate::def_semantics::const_positive_usize_for_call_type(&proc_array.size_expr),
        });
    }
    if let Some(struct_array) = env.struct_array_roots.get(name) {
        return Some(CallArrayArgInfo {
            elem: CallArrayArgElem::Nominal(struct_array.struct_name.clone()),
            len: struct_array.static_len,
        });
    }
    if is_struct_array_root(env.declared_symbols, name) {
        return Some(CallArrayArgInfo {
            elem: env
                .local_array_aliases
                .get(name)
                .and_then(|alias| alias.elem_struct.clone())
                .map(CallArrayArgElem::Nominal)
                .unwrap_or(CallArrayArgElem::NominalUnknown),
            len: env
                .local_array_aliases
                .get(name)
                .and_then(|alias| alias.static_len)
                .or_else(|| env.array_vars.get(name).copied()),
        });
    }
    if !env.array_vars.contains_key(name)
        && !env.output_arrays.contains(name)
        && !is_declared_struct_array_root_symbol(env.declared_symbols, name)
    {
        return None;
    }
    let elem = declared_symbol_scalar_type(env.declared_symbols, name)
        .map(CallArrayArgElem::Primitive)
        .or_else(|| {
            env.struct_name(name)
                .map(str::to_owned)
                .map(CallArrayArgElem::Nominal)
        })?;
    Some(CallArrayArgInfo {
        elem,
        len: env.array_vars.get(name).copied(),
    })
}

fn call_array_symbol_info(name: &str, env: ExprEnv<'_>) -> Option<CallArrayArgInfo> {
    struct_field_array_info(name, env)
        .or_else(|| direct_array_symbol_info(name, env))
        .or_else(|| {
            // Proc state rewriting names owned storage `self.field`, while its
            // semantic metadata remains keyed by the source-level field name.
            let (root, field) = split_simple_field_path(name)?;
            let typed_receiver = env
                .struct_name(root)
                .is_some_and(|name| env.struct_defs.contains_key(name));
            if root == "self" && !typed_receiver {
                direct_array_symbol_info(field, env)
            } else {
                None
            }
        })
}

pub(crate) fn array_data_struct_element_type(name: &str, env: ExprEnv<'_>) -> Option<String> {
    let CallArrayArgElem::Nominal(struct_name) = call_array_symbol_info(name, env)?.elem else {
        return None;
    };
    env.struct_defs
        .contains_key(&struct_name)
        .then_some(struct_name)
}

fn call_array_arg_info(expr: &Expr, env: ExprEnv<'_>) -> Option<CallArrayArgInfo> {
    match expr {
        Expr::UserCall { name, .. } => match env.fn_signatures.get(name)?.return_type.as_ref()? {
            ReturnType::Data(DataType::Array { element, len }) => Some(CallArrayArgInfo {
                elem: match element {
                    ArrayElemType::Primitive(ty) => CallArrayArgElem::Primitive(*ty),
                    ArrayElemType::Struct(name) => CallArrayArgElem::Nominal(name.clone()),
                },
                len: Some(*len),
            }),
            _ => None,
        },
        Expr::Var { name, .. } => call_array_symbol_info(name, env),
        Expr::Slice { base, .. } => call_array_symbol_info(base, env).map(|mut info| {
            info.len = None;
            info
        }),
        Expr::ArrayLiteral { values, .. } => {
            let elem = values
                .first()
                .and_then(|value| call_array_value_elem(value, env))
                .unwrap_or(CallArrayArgElem::Unknown);
            Some(CallArrayArgInfo {
                elem,
                len: Some(values.len()),
            })
        }
        Expr::ArrayCtor { spec, .. } => Some(CallArrayArgInfo {
            elem: match &spec.elem {
                ArrayElemType::Primitive(elem) => CallArrayArgElem::Primitive(*elem),
                ArrayElemType::Struct(name) => CallArrayArgElem::Nominal(name.clone()),
            },
            len: crate::def_semantics::const_positive_usize_for_call_type(&spec.size),
        }),
        _ => None,
    }
}

/// Storage declarations may use a proven slice length without promoting the
/// source binding to a fixed array (in particular, it remains non-returnable).
pub(crate) fn infer_fixed_initializer_type(expr: &Expr, env: ExprEnv<'_>) -> Option<DataType> {
    if let Some(data) = infer_fixed_data_type(expr, env) {
        return Some(data);
    }
    let (source, start, end) = match expr {
        Expr::Var { name, .. } => (name, None, None),
        Expr::Slice {
            base,
            selector: None,
            channel: None,
            start,
            end,
            ..
        } => (base, start.as_deref(), end.as_deref()),
        _ => return None,
    };
    let (element, len) = match infer_fixed_data_type(&Expr::var(source), env) {
        Some(DataType::Array { element, len }) => (element, Some(len)),
        _ => {
            let alias = env.local_array_aliases.get(source)?;
            (
                alias
                    .elem_struct
                    .as_ref()
                    .map(|name| ArrayElemType::Struct(name.clone()))
                    .unwrap_or(ArrayElemType::Primitive(alias.elem_ty)),
                alias.proven_len,
            )
        }
    };
    let len = crate::stmt_analysis::prove_static_slice_len(len, start, end)?;
    (len > 0).then_some(DataType::Array { element, len })
}

pub(crate) fn infer_fixed_data_type(expr: &Expr, env: ExprEnv<'_>) -> Option<DataType> {
    if let Expr::UserCall { name, args, .. } = expr {
        if name == STRUCT_ARRAY_FIELD_INDEX_SENTINEL {
            let (base, _, field, _) = crate::array_structs::extract_safi_args(args)?;
            let struct_name = array_data_struct_element_type(&base, env)?;
            return resolve_indexed_struct_field_data_type(&struct_name, &field, env.struct_defs);
        }
    }
    match expr {
        Expr::ArrayCtor { spec, .. } => {
            return Some(DataType::Array {
                element: spec.elem.clone(),
                len: crate::def_semantics::const_positive_usize_for_call_type(&spec.size)?,
            });
        }
        Expr::ArrayLiteral { values, .. } => {
            if let Some(DataType::Struct(name)) = values
                .first()
                .and_then(|value| infer_fixed_data_type(value, env))
            {
                return Some(DataType::Array {
                    element: ArrayElemType::Struct(name),
                    len: values.len(),
                });
            }
        }
        Expr::UserCall { name, .. } if env.struct_defs.contains_key(name) => {
            return Some(DataType::Struct(name.clone()))
        }
        Expr::UserCall { name, .. } => {
            return match env.fn_signatures.get(name)?.return_type.as_ref()? {
                ReturnType::Data(data) => Some(data.clone()),
                _ => None,
            }
        }
        Expr::Var { name, .. } => {
            if let Some(struct_name) = env.struct_name(name) {
                return Some(DataType::Struct(struct_name.to_owned()));
            }
            if let Some((root, path)) = name.split_once('.') {
                if let Some(struct_name) = env.struct_name(root) {
                    if let Some(field) =
                        resolve_struct_field_decl(struct_name, path, env.struct_defs)
                    {
                        match field.ty {
                            TypedFieldType::Struct => {
                                return field.struct_name.clone().map(DataType::Struct)
                            }
                            TypedFieldType::Array(len) => {
                                return Some(DataType::Array {
                                    element: field
                                        .array_elem_struct
                                        .as_ref()
                                        .map(|name| ArrayElemType::Struct(name.clone()))
                                        .unwrap_or(ArrayElemType::Primitive(
                                            field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                                        )),
                                    len,
                                })
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Expr::Index { base, .. } => {
            if let CallArrayArgElem::Nominal(name) = call_array_symbol_info(base, env)?.elem {
                return env
                    .struct_defs
                    .contains_key(&name)
                    .then_some(DataType::Struct(name));
            }
            return None;
        }
        Expr::Slice { .. } => return None,
        _ => {}
    }
    let info = call_array_arg_info(expr, env)?;
    Some(DataType::Array {
        element: match info.elem {
            CallArrayArgElem::Primitive(ty) => ArrayElemType::Primitive(ty),
            CallArrayArgElem::Nominal(name) => ArrayElemType::Struct(name),
            _ => return None,
        },
        len: info.len?,
    })
}

pub(crate) fn validate_primitive_array_values(
    values: &[Expr],
    element: PrimitiveType,
    len: usize,
    expression: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if values.len() != len {
        push_expr_error(
            errors,
            expression,
            format!(
                "fixed array initializer expects {len} elements, got {}",
                values.len()
            ),
        );
    }
    for value in values {
        validate_expr(value, env, errors);
        require_expr_assignable_type(
            value,
            infer_call_argument_scalar_type(value, env),
            element,
            "array initializer",
            errors,
        );
    }
}

pub(crate) fn infer_data_value_type(expr: &Expr, env: ExprEnv<'_>) -> Option<DataType> {
    infer_fixed_data_type(expr, env).or_else(|| {
        let source = indexed_read_source(expr)?;
        array_data_struct_element_type(source.base, env).map(DataType::Struct)
    })
}

pub(crate) fn reject_empty_slice_backing_literal(
    expression: &Expr,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if !matches!(expression, Expr::ArrayLiteral { values, .. } if values.is_empty()) {
        return false;
    }
    push_expr_error(
        errors,
        expression,
        "empty array literal cannot provide backing storage for a slice; slice an existing array to create an empty view",
    );
    true
}

/// Validate data in a storage/reference context without treating its root as a
/// scalar read. Selector and argument effects still receive ordinary checking.
pub(crate) fn validate_fixed_data_expr(
    expr: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::ArrayCtor {
            spec,
            init,
            init_is_value,
            ..
        } if matches!(spec.elem, ArrayElemType::Primitive(_)) => {
            let ArrayElemType::Primitive(element) = spec.elem else {
                unreachable!()
            };
            let Some(len) = crate::def_semantics::const_positive_usize_for_call_type(&spec.size)
            else {
                push_expr_error(errors, expr, "data array requires a positive fixed length");
                return;
            };
            if let Some(values) = init {
                if *init_is_value
                    && values.len() == 1
                    && infer_fixed_initializer_type(&values[0], env)
                        == Some(DataType::Array {
                            element: spec.elem.clone(),
                            len,
                        })
                {
                    validate_fixed_data_expr(&values[0], env, errors);
                } else {
                    let count = if *init_is_value && values.len() == 1 {
                        1
                    } else {
                        len
                    };
                    validate_primitive_array_values(values, element, count, expr, env, errors);
                }
            }
        }
        Expr::ArrayCtor {
            spec,
            init,
            init_is_value,
            ..
        } => {
            let ArrayElemType::Struct(name) = &spec.elem else {
                unreachable!()
            };
            if !env.struct_defs.contains_key(name) {
                push_expr_error(errors, expr, format!("unknown data struct '{name}'"));
                return;
            }
            let Some(len) = crate::def_semantics::const_positive_usize_for_call_type(&spec.size)
            else {
                push_expr_error(errors, expr, "data array requires a positive fixed length");
                return;
            };
            if let Some(values) = init {
                let expected = DataType::Struct(name.clone());
                let copy_source = (*init_is_value && values.len() == 1).then(|| &values[0]);
                let actual_array =
                    copy_source.and_then(|value| infer_fixed_initializer_type(value, env));
                let array_source = copy_source.filter(|value| {
                    call_array_arg_info(value, env).is_some_and(|info| {
                        matches!(info.elem, CallArrayArgElem::Nominal(actual) if actual == *name)
                    })
                });
                let expected_array = DataType::Array {
                    element: spec.elem.clone(),
                    len,
                };
                let is_copy = actual_array.as_ref() == Some(&expected_array);
                if let Some(source) = array_source.filter(|_| !is_copy) {
                    let message = match actual_array.as_ref() {
                        Some(actual) => format!(
                            "fixed data array initializer {}",
                            data_type_mismatch(&expected_array, Some(actual))
                        ),
                        None => format!(
                            "fixed data array initializer for '{name}[{len}]' requires a statically proven exact length"
                        ),
                    };
                    push_expr_error(errors, source, message);
                }
                if !is_copy && !(*init_is_value && values.len() == 1) && values.len() != len {
                    push_expr_error(
                        errors,
                        expr,
                        format!(
                            "data array initializer expects {len} elements, got {}",
                            values.len()
                        ),
                    );
                }
                for value in values {
                    let actual = infer_data_value_type(value, env);
                    if !is_copy && array_source.is_none() && actual.as_ref() != Some(&expected) {
                        push_expr_error(
                            errors,
                            value,
                            format!(
                                "data array element {}",
                                data_type_mismatch(&expected, actual.as_ref())
                            ),
                        );
                    }
                    validate_fixed_data_expr(value, env, errors);
                }
            }
        }
        Expr::ArrayLiteral { values, .. } => {
            let nominal = values
                .first()
                .and_then(|value| infer_data_value_type(value, env));
            for value in values {
                let actual = infer_data_value_type(value, env);
                if let Some(expected @ DataType::Struct(_)) = nominal.as_ref() {
                    if actual.as_ref() != Some(expected) {
                        push_expr_error(
                            errors,
                            value,
                            format!(
                                "data array element {}",
                                data_type_mismatch(expected, actual.as_ref())
                            ),
                        );
                    }
                }
                if actual.is_some() {
                    validate_fixed_data_expr(value, env, errors);
                } else {
                    validate_expr(value, env, errors);
                }
            }
        }
        Expr::Var { name, .. }
            if infer_fixed_data_type(expr, env).is_some()
                || call_array_arg_info(expr, env).is_some() =>
        {
            if dynamic_param_surface_name(name, env).is_some() {
                validate_expr(expr, env, errors);
            }
        }
        Expr::Index { index, .. } if infer_fixed_data_type(expr, env).is_some() => {
            validate_expr(index, env, errors)
        }
        Expr::UserCall { name, .. }
            if name == STRUCT_ARRAY_FIELD_INDEX_SENTINEL
                && infer_fixed_data_type(expr, env).is_some() =>
        {
            validate_expr(expr, env, errors)
        }
        Expr::UserCall { name, args, .. }
            if name == READ_UNSAFE_FN && infer_data_value_type(expr, env).is_some() =>
        {
            validate_unsafe_index_call(name, args, env, expr.loc(), true, errors)
        }
        Expr::UserCall {
            name,
            args,
            type_args,
            ..
        } if env.struct_defs.contains_key(name) => {
            if !type_args.is_empty() {
                push_expr_error(
                    errors,
                    expr,
                    format!("constructor '{name}' does not accept type arguments"),
                );
            }
            let fields = &env.struct_defs[name];
            let names = fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<Vec<_>>();
            let defaults = fields
                .iter()
                .map(|field| field.default.clone().or_else(|| Some(Expr::int(0))))
                .collect::<Vec<_>>();
            let resolved = resolve_call_args_at(
                args,
                &names,
                &defaults,
                false,
                false,
                &format!("constructor '{name}'"),
                expr.loc(),
                errors,
            );
            for (field, arg) in fields.iter().zip(resolved) {
                let Some(arg) = arg else {
                    continue;
                };
                match &field.ty {
                    TypedFieldType::Scalar(ty) => {
                        validate_expr(arg, env, errors);
                        require_expr_assignable_type(
                            arg,
                            infer_call_argument_scalar_type(arg, env),
                            *ty,
                            "constructor field",
                            errors,
                        );
                    }
                    TypedFieldType::Tuple(types) => {
                        validate_tuple_param_call_arg(name, &field.name, types, arg, env, errors)
                    }
                    TypedFieldType::Struct | TypedFieldType::Array(_) => {
                        if let (
                            TypedFieldType::Array(len),
                            Some(element),
                            Expr::ArrayLiteral { values, .. },
                        ) = (&field.ty, field.array_elem_ty, arg)
                        {
                            validate_primitive_array_values(
                                values, element, *len, arg, env, errors,
                            );
                            continue;
                        }
                        let expected = match field.ty {
                            TypedFieldType::Struct => {
                                field.struct_name.clone().map(DataType::Struct)
                            }
                            TypedFieldType::Array(len) => Some(DataType::Array {
                                element: field
                                    .array_elem_struct
                                    .as_ref()
                                    .map(|name| ArrayElemType::Struct(name.clone()))
                                    .unwrap_or(ArrayElemType::Primitive(
                                        field.array_elem_ty.unwrap_or(PrimitiveType::F32),
                                    )),
                                len,
                            }),
                            _ => unreachable!(),
                        };
                        let actual = infer_data_value_type(arg, env);
                        if actual != expected {
                            let mismatch = expected.as_ref().map_or_else(
                                || "has an unresolved declared data type".to_owned(),
                                |expected| data_type_mismatch(expected, actual.as_ref()),
                            );
                            push_expr_error(
                                errors,
                                arg,
                                format!("constructor field '{}' {mismatch}", field.name),
                            );
                        }
                        validate_fixed_data_expr(arg, env, errors);
                    }
                }
            }
        }
        _ => validate_expr(expr, env, errors),
    }
}

fn validate_array_param_call_arg(
    function_name: &str,
    param_name: &str,
    param_ty: &FnParamType,
    arg: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) {
    if matches!(
        param_ty,
        FnParamType::Array(_) | FnParamType::ArrayGeneric(_)
    ) && reject_empty_slice_backing_literal(arg, errors)
    {
        return;
    }
    let Some(actual) = call_array_arg_info(arg, env) else {
        if is_definitely_scalar_call_arg(arg, env)
            || infer_call_argument_tuple_types(arg, env).is_some()
        {
            push_expr_error(
                errors,
                arg,
                format!(
                    "function '{function_name}' parameter '{param_name}' expects an array value"
                ),
            );
        }
        return;
    };

    let (expected_elem, expected_len) = match param_ty {
        FnParamType::Array(Some(elem)) => (Some(CallArrayArgElem::Primitive(*elem)), None),
        FnParamType::ArrayGeneric(name) => (Some(CallArrayArgElem::Nominal(name.clone())), None),
        FnParamType::SizedArray {
            elem,
            generic_name,
            size,
        } => {
            let elem = elem
                .map(CallArrayArgElem::Primitive)
                .or_else(|| generic_name.clone().map(CallArrayArgElem::Nominal));
            (
                elem,
                crate::def_semantics::const_positive_usize_for_call_type(size),
            )
        }
        FnParamType::Array(None) => (None, None),
        _ => return,
    };

    let literal_elements_match = match (expected_elem.as_ref(), arg) {
        (Some(CallArrayArgElem::Primitive(expected)), Expr::ArrayLiteral { values, .. }) => {
            Some(values.iter().all(|value| {
                infer_call_argument_scalar_type(value, env)
                    .is_some_and(|actual| can_assign_expr_to_type(value, actual, *expected))
            }))
        }
        (Some(CallArrayArgElem::Nominal(expected)), Expr::ArrayLiteral { values, .. }) => {
            Some(values.iter().all(|value| {
                matches!(
                    call_array_value_elem(value, env),
                    Some(CallArrayArgElem::Nominal(actual)) if actual == *expected
                )
            }))
        }
        _ => None,
    };
    let elem_matches =
        literal_elements_match.unwrap_or_else(|| match (expected_elem.as_ref(), &actual.elem) {
            (None, _) => true,
            (Some(_), CallArrayArgElem::Unknown) => true,
            (Some(CallArrayArgElem::Primitive(expected)), CallArrayArgElem::Primitive(actual)) => {
                expected == actual
            }
            (Some(CallArrayArgElem::Nominal(expected)), CallArrayArgElem::Nominal(actual)) => {
                expected == actual
            }
            (Some(CallArrayArgElem::Nominal(_)), CallArrayArgElem::NominalUnknown) => true,
            _ => false,
        });
    if !elem_matches {
        let expected = match expected_elem.expect("mismatched typed array element") {
            CallArrayArgElem::Primitive(elem) => elem.name().to_owned(),
            CallArrayArgElem::Nominal(name) => name,
            CallArrayArgElem::Unknown => "unknown".to_owned(),
            CallArrayArgElem::NominalUnknown => "nominal".to_owned(),
        };
        let actual = match &actual.elem {
            CallArrayArgElem::Primitive(elem) => elem.name().to_owned(),
            CallArrayArgElem::Nominal(name) => name.clone(),
            CallArrayArgElem::Unknown => "unknown".to_owned(),
            CallArrayArgElem::NominalUnknown => "nominal".to_owned(),
        };
        push_expr_error(
            errors,
            arg,
            format!(
                "function '{function_name}' parameter '{param_name}' expects {expected} array elements, got {actual}"
            ),
        );
        return;
    }

    if let Some(expected) = expected_len {
        match actual.len {
            Some(actual) if actual == expected => {}
            Some(actual) => push_expr_error(
                errors,
                arg,
                format!(
                    "function '{function_name}' parameter '{param_name}' expects array length {expected}, got {actual}"
                ),
            ),
            None => push_expr_error(
                errors,
                arg,
                format!(
                    "function '{function_name}' parameter '{param_name}' expects fixed array length {expected}, but the argument length is not statically known"
                ),
            ),
        }
    }
}

fn is_definitely_scalar_call_arg(expr: &Expr, env: ExprEnv<'_>) -> bool {
    match expr {
        Expr::Number { .. }
        | Expr::Int { .. }
        | Expr::Bool { .. }
        | Expr::Compare { .. }
        | Expr::Call { .. }
        | Expr::Cast { .. }
        | Expr::UnaryNot { .. }
        | Expr::UnaryBitNot { .. }
        | Expr::Logical { .. }
        | Expr::Binary { .. } => true,
        Expr::Var { name, .. } => {
            env.locals.contains(name)
                || env.local_aliases.contains_key(name)
                || env.state_scalars.contains_key(name)
                || matches!(
                    env.declared_symbols.get(name),
                    Some(
                        DeclaredSymbolInfo::Input { .. }
                            | DeclaredSymbolInfo::Output { .. }
                            | DeclaredSymbolInfo::Param { .. }
                            | DeclaredSymbolInfo::FunctionReturn { .. }
                    )
                )
        }
        Expr::UserCall { name, .. } => env
            .fn_signatures
            .get(name)
            .and_then(|signature| signature.return_type.as_ref())
            .is_some_and(|return_type| matches!(return_type, ReturnType::Scalar(_))),
        Expr::Index { .. }
        | Expr::Slice { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::ArrayCtor { .. }
        | Expr::Tuple { .. } => false,
    }
}

fn protected_array_pointer_arg_name<'a>(expr: &'a Expr, env: ExprEnv<'_>) -> Option<&'a str> {
    let name = match expr {
        Expr::Var { name, .. } | Expr::Slice { base: name, .. } => name.as_str(),
        _ => return None,
    };
    protected_proc_view_arg_name(name, env)
        .or_else(|| env.output_arrays.contains(name).then_some(name))
}

fn protected_proc_view_arg_name<'a>(name: &'a str, env: ExprEnv<'_>) -> Option<&'a str> {
    let (receiver, field) = split_simple_field_path(name)?;
    let protected_field = matches!(field, "ins" | "outs" | "kouts" | "params" | "kins");
    if protected_field && env.proc_array_roots.contains_key(receiver) {
        Some(name)
    } else {
        None
    }
}

fn reject_protected_array_pointer_call_arg(
    fn_name: &str,
    param_name: &str,
    arg: &Expr,
    env: ExprEnv<'_>,
    call_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    if let Some(arg_name) = dynamic_param_surface_value_name(arg, env) {
        push_dynamic_param_surface_value_error(errors, arg.loc().or(call_loc), arg_name);
        return true;
    }
    if let Some(arg_name) = io_surface_value_name(arg, env) {
        if env.io_surface_access_allowed {
            push_io_surface_value_error(errors, arg.loc().or(call_loc), arg_name);
        } else {
            push_io_surface_scope_error(errors, arg.loc().or(call_loc), arg_name);
        }
        return true;
    }
    let Some(arg_name) = protected_array_pointer_arg_name(arg, env) else {
        return false;
    };
    push_loc_error(
        errors,
        arg.loc().or(call_loc),
        format!(
            "cannot pass protected array view '{arg_name}' to array parameter '{param_name}' of function '{fn_name}'"
        ),
    );
    true
}

fn reject_immutable_array_call_arg(
    fn_name: &str,
    param_name: &str,
    param_readonly: bool,
    arg: &Expr,
    env: ExprEnv<'_>,
    call_loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Some(alias_name) = immutable_array_alias_arg_name(arg, env) else {
        return false;
    };
    if param_readonly {
        return false;
    }
    push_loc_error(
        errors,
        arg.loc().or(call_loc),
        format!(
            "cannot pass immutable array alias '{alias_name}' to mutable array parameter '{param_name}' of function '{fn_name}'"
        ),
    );
    true
}

fn validate_data_len_builtin_call(
    name: &str,
    args: &[CallArg],
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
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
}

fn validate_buffer_metadata_builtin_call(
    name: &str,
    base: &str,
    args: &[CallArg],
    method: &str,
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
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
    if is_declared_buffer_array_info(env.declared_symbols, base) {
        push_loc_error(
            errors,
            loc,
            format!("buffer collection '{base}' must select a slot before calling '.{method}()'"),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_buffer_array_param_call_arg(
    fn_name: &str,
    param_idx: usize,
    param_names: &[String],
    expected: &BufferType,
    expected_len: usize,
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
            for choice in args.iter().filter(|arg| arg.name.is_none()) {
                validate_buffer_array_param_call_arg(
                    fn_name,
                    param_idx,
                    param_names,
                    expected,
                    expected_len,
                    &choice.expr,
                    env,
                    loc,
                    errors,
                );
            }
            return;
        }
    }
    let (base, start, end) = match arg {
        Expr::Var { name, .. } => (name.as_str(), None, None),
        Expr::Slice {
            base,
            selector: None,
            channel: None,
            start,
            end,
            ..
        } => (base.as_str(), start.as_deref(), end.as_deref()),
        _ => {
            push_loc_error(
                errors,
                arg.loc().or(loc),
                format!("{context} expects a buffer collection or static collection slice"),
            );
            validate_expr(arg, env, errors);
            return;
        }
    };
    let Some(DeclaredSymbolInfo::Buffer {
        array_len,
        is_array: true,
        ..
    }) = env.declared_symbols.get(base)
    else {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            format!("{context} expects a buffer collection, got '{base}'"),
        );
        return;
    };
    validate_buffer_symbol_for_param(&context, expected, base, env, arg.loc().or(loc), errors);

    let bound = |expr: Option<&Expr>, default: i64, label: &str, errors: &mut Vec<Diagnostic>| {
        expr.map_or(Some(default), |expr| {
            let value = crate::try_constant_index_i64(expr);
            if value.is_none() {
                push_loc_error(
                    errors,
                    expr.loc().or(loc),
                    format!("{context} requires a compile-time {label} bound"),
                );
            }
            value
        })
    };
    let Some(start) = bound(start, 0, "slice start", errors) else {
        return;
    };
    let Some(end) = bound(end, *array_len as i64, "slice end", errors) else {
        return;
    };
    if start < 0 || end < start || end > *array_len as i64 {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            format!("buffer collection slice '{base}[{start}:{end}]' is outside 0..{array_len}"),
        );
        return;
    }
    let actual_len = usize::try_from(end - start).unwrap_or(usize::MAX);
    if actual_len != expected_len {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            format!("{context} expects {expected_len} buffers, got {actual_len}"),
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
    if let Expr::Index { base, index, .. } = arg {
        if is_declared_buffer_array_info(env.declared_symbols, base) {
            validate_expr(index, env, errors);
            validate_buffer_symbol_for_param(
                &context,
                expected,
                base,
                env,
                arg.loc().or(loc),
                errors,
            );
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
    if is_declared_buffer_array_info(env.declared_symbols, symbol) {
        push_loc_error(
            errors,
            arg.loc().or(loc),
            format!("{context} requires one buffer; select a slot from collection '{symbol}'"),
        );
        return;
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
        // `f32[]` means an unspecified positive channel count. Mono and exact
        // multichannel buffers are therefore both valid arguments.
        BufferChannels::Dynamic => {}
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

fn validate_internal_buffer_index_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    errors: &mut Vec<Diagnostic>,
) {
    if matches!(name, READ_UNSAFE_FN | WRITE_UNSAFE_FN) {
        validate_unsafe_index_call(name, args, env, loc, false, errors);
        return;
    }
    let is_write = matches!(
        name,
        INTERNAL_BUFFER_WRITE2_FN | INTERNAL_BUFFER_WRITE3_FN | INTERNAL_BUFFER_WRITE_CHANNEL_FN
    );
    let is_three_dimensional = matches!(name, INTERNAL_BUFFER_READ3_FN | INTERNAL_BUFFER_WRITE3_FN);
    let expected_arity = 3 + usize::from(is_three_dimensional) + usize::from(is_write);
    let label = "internal builtin";
    if args.len() != expected_arity {
        push_loc_error(
            errors,
            loc,
            format!(
                "{} '{}' expects {} positional arguments, got {}",
                label,
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
                format!("{} '{}' does not support named arguments", label, name),
            );
        }
    }
    if let Some(first) = args.first() {
        match &first.expr {
            Expr::Var { name: base, .. } => {
                if !has_declared_buffer_symbol_info(env.declared_symbols, base) {
                    push_loc_error(
                        errors,
                        first.expr.loc().or(loc),
                        format!(
                            "{} '{}' first argument must be a declared buffer symbol, got '{}'",
                            label, name, base
                        ),
                    );
                } else {
                    let is_array = is_declared_buffer_array_info(env.declared_symbols, base);
                    let is_multichannel =
                        is_declared_multichannel_buffer_info(env.declared_symbols, base);
                    let is_channel_access = matches!(
                        name,
                        INTERNAL_BUFFER_READ_CHANNEL_FN | INTERNAL_BUFFER_WRITE_CHANNEL_FN
                    );
                    let is_collection_access =
                        matches!(name, INTERNAL_BUFFER_READ2_FN | INTERNAL_BUFFER_WRITE2_FN);
                    if is_three_dimensional && (!is_array || !is_multichannel) {
                        push_loc_error(
                            errors,
                            first.expr.loc().or(loc),
                            format!("{} '{}' requires a multichannel buffer array", label, name),
                        );
                    } else if is_collection_access && (!is_array || is_multichannel) {
                        push_loc_error(
                            errors,
                            first.expr.loc().or(loc),
                            format!(
                                "buffer access '{}' requires a mono buffer collection and the form '{}[slot][frame]'",
                                base, base
                            ),
                        );
                    } else if is_channel_access && (is_array || !is_multichannel) {
                        push_loc_error(
                            errors,
                            first.expr.loc().or(loc),
                            format!(
                                "multichannel buffer access '{}' requires the form '{}[channel, frame]'",
                                base, base
                            ),
                        );
                    }
                }
            }
            other => {
                validate_expr(other, env, errors);
                push_loc_error(
                    errors,
                    other.loc().or(loc),
                    format!(
                        "{} '{}' first argument must be a declared buffer symbol variable",
                        label, name
                    ),
                );
            }
        }
    }
    for argument in args.iter().skip(1) {
        validate_expr(&argument.expr, env, errors);
    }
}

fn validate_unsafe_index_call(
    name: &str,
    args: &[CallArg],
    env: ExprEnv<'_>,
    loc: SourceLoc,
    allow_aggregate_read: bool,
    errors: &mut Vec<Diagnostic>,
) {
    let is_write = name == WRITE_UNSAFE_FN;
    for argument in args {
        if argument.name.is_some() {
            push_loc_error(
                errors,
                loc,
                format!("'{name}' does not support named arguments"),
            );
        }
    }
    let Some(first) = args.first() else {
        push_loc_error(errors, loc, format!("'{name}' expects a storage variable"));
        return;
    };
    let storage = match &first.expr {
        Expr::Var { name: base, .. } => unsafe_storage_shape(base, env).map(|shape| (base, shape)),
        Expr::Index { base, index, .. } => {
            validate_expr(index, env, errors);
            let index_ty = infer_call_argument_scalar_type(index, env);
            require_expr_numeric_type(index, index_ty, &format!("'{name}' index argument"), errors);
            unsafe_selected_buffer_shape(base, env).map(|shape| (base, shape))
        }
        _ => None,
    };
    if let Some((base, shape)) = storage {
        let expected = 1 + shape.index_count + usize::from(is_write);
        if args.len() != expected {
            push_loc_error(
                errors,
                loc,
                format!(
                    "'{name}' expects {} index argument{}{}, got {} arguments",
                    shape.index_count,
                    if shape.index_count == 1 { "" } else { "s" },
                    if is_write { " and a value" } else { "" },
                    args.len()
                ),
            );
        }
        if is_write {
            if matches!(shape.element, UnsafeStorageElement::Resource) {
                push_loc_error(
                    errors,
                    first.expr.loc().or(loc),
                    format!("write_unsafe does not support resource array '{base}'"),
                );
            } else if !shape.writable {
                push_loc_error(
                    errors,
                    first.expr.loc().or(loc),
                    format!("write_unsafe storage '{base}' is read-only"),
                );
            }
        }
        if !is_write && !shape.readable {
            push_loc_error(
                errors,
                first.expr.loc().or(loc),
                format!("read_unsafe storage '{base}' is write-only"),
            );
        }
        if !is_write && shape.element.is_aggregate() && !allow_aggregate_read {
            push_loc_error(
                errors,
                loc,
                format!(
                    "aggregate read_unsafe from '{base}' is only valid in an alias or reference argument"
                ),
            );
        }
        for argument in args.iter().skip(1).take(shape.index_count) {
            let index_ty = infer_call_argument_scalar_type(&argument.expr, env);
            require_expr_numeric_type(
                &argument.expr,
                index_ty,
                &format!("'{name}' index argument"),
                errors,
            );
        }
        if is_write {
            if let Some(value) = args.get(1 + shape.index_count) {
                match &shape.element {
                    UnsafeStorageElement::Data(struct_name) => validate_expected_data(
                        &value.expr,
                        &DataType::Struct(struct_name.clone()),
                        "write_unsafe replacement",
                        env,
                        value.expr.loc(),
                        errors,
                    ),
                    UnsafeStorageElement::Primitive(Some(expected_ty)) => {
                        let actual_ty = infer_call_argument_scalar_type(&value.expr, env);
                        require_expr_assignable_type(
                            &value.expr,
                            actual_ty,
                            *expected_ty,
                            "'write_unsafe' value",
                            errors,
                        );
                    }
                    UnsafeStorageElement::Primitive(None) | UnsafeStorageElement::Resource => {}
                }
            }
        }
        let validated_data_value =
            is_write && matches!(shape.element, UnsafeStorageElement::Data(_));
        for (index, argument) in args.iter().enumerate().skip(1) {
            if validated_data_value && index == 1 + shape.index_count {
                continue;
            }
            validate_expr(&argument.expr, env, errors);
        }
    } else {
        push_loc_error(
            errors,
            first.expr.loc().or(loc),
            format!(
                "'{name}' first argument must be a collection, aggregate array, or buffer reference"
            ),
        );
        for argument in args.iter().skip(1) {
            validate_expr(&argument.expr, env, errors);
        }
    }
}

struct UnsafeStorageShape {
    index_count: usize,
    element: UnsafeStorageElement,
    readable: bool,
    writable: bool,
}

enum UnsafeStorageElement {
    Primitive(Option<PrimitiveType>),
    Data(String),
    Resource,
}

impl UnsafeStorageElement {
    fn is_aggregate(&self) -> bool {
        !matches!(self, Self::Primitive(_))
    }
}

fn unsafe_storage_shape(base: &str, env: ExprEnv<'_>) -> Option<UnsafeStorageShape> {
    if let Some(alias) = env.local_array_aliases.get(base) {
        let data_struct = (!env.proc_array_roots.contains_key(base))
            .then(|| {
                alias
                    .elem_struct
                    .as_ref()
                    .filter(|name| {
                        env.struct_defs.contains_key(*name) && !is_processor_struct_name(name, env)
                    })
                    .cloned()
            })
            .flatten();
        return Some(UnsafeStorageShape {
            index_count: 1,
            element: match data_struct {
                Some(name) => UnsafeStorageElement::Data(name),
                None if alias.elem_struct.is_some() => UnsafeStorageElement::Resource,
                None => UnsafeStorageElement::Primitive(Some(alias.elem_ty)),
            },
            readable: alias.elem_struct.is_some() || !env.output_arrays.contains(base),
            writable: alias.writable,
        });
    }
    if env.local_aliases.contains_key(base) {
        return None;
    }

    if let Some(DeclaredSymbolInfo::Buffer {
        elem_ty,
        channels,
        is_array,
        ..
    }) = env.declared_symbols.get(base)
    {
        let has_channel = matches!(channels, BufferChannelInfo::Dynamic)
            || matches!(channels, BufferChannelInfo::Static(count) if *count > 1);
        return Some(UnsafeStorageShape {
            index_count: 1 + usize::from(*is_array) + usize::from(has_channel),
            element: UnsafeStorageElement::Primitive(Some(*elem_ty)),
            readable: true,
            writable: true,
        });
    }

    if unsafe_aggregate_array(base, env) {
        let data_struct = (!env.proc_array_roots.contains_key(base))
            .then(|| array_data_struct_element_type(base, env))
            .flatten()
            .filter(|name| !is_processor_struct_name(name, env));
        return Some(UnsafeStorageShape {
            index_count: 1,
            writable: data_struct.is_some(),
            element: data_struct
                .map(UnsafeStorageElement::Data)
                .unwrap_or(UnsafeStorageElement::Resource),
            readable: true,
        });
    }

    if env.array_vars.contains_key(base) {
        return Some(UnsafeStorageShape {
            index_count: 1,
            element: UnsafeStorageElement::Primitive(declared_symbol_scalar_type(
                env.declared_symbols,
                base,
            )),
            readable: !env.output_arrays.contains(base),
            writable: !env.input_names.contains(base) && !env.param_names.contains(base),
        });
    }

    let (port, readable, writable) = match base {
        "ins" => (env.port_index_ins, true, false),
        "outs" => (
            env.port_index_outs
                .filter(|_| env.scope == ScopeKind::Sample),
            false,
            true,
        ),
        "kouts" => (
            env.port_index_outs
                .filter(|_| env.scope == ScopeKind::Block),
            false,
            true,
        ),
        "params" => (env.port_index_params, true, false),
        "kins" => (env.port_index_kins, true, false),
        _ => return None,
    };
    port.map(|port| UnsafeStorageShape {
        index_count: 1,
        element: UnsafeStorageElement::Primitive(Some(port.elem_ty)),
        readable,
        writable,
    })
}

fn validate_aggregate_unsafe_reference_arg(
    expr: &Expr,
    env: ExprEnv<'_>,
    errors: &mut Vec<Diagnostic>,
) -> bool {
    let Expr::UserCall { name, args, .. } = expr else {
        return false;
    };
    if name != READ_UNSAFE_FN {
        return false;
    }
    let Some(Expr::Var { name: base, .. }) = args.first().map(|arg| &arg.expr) else {
        return false;
    };
    if !unsafe_storage_shape(base, env).is_some_and(|shape| shape.element.is_aggregate()) {
        return false;
    }
    validate_unsafe_index_call(name, args, env, expr.loc(), true, errors);
    true
}

fn is_processor_struct_name(name: &str, env: ExprEnv<'_>) -> bool {
    env.fn_signatures
        .contains_key(&format!("{name}{PROC_INIT_FN_SUFFIX}"))
}

fn unsafe_aggregate_array(base: &str, env: ExprEnv<'_>) -> bool {
    if env.struct_array_roots.contains_key(base) || env.proc_array_roots.contains_key(base) {
        return true;
    }

    let field = if let Some((root, field)) = base.split_once('.') {
        let Some(struct_name) = env.struct_name(root) else {
            return false;
        };
        resolve_struct_field_decl(struct_name, field, env.struct_defs)
    } else {
        let Some(struct_name) = env.param_structs.get("self") else {
            return false;
        };
        resolve_struct_field_decl(struct_name, base, env.struct_defs)
    };

    field.is_some_and(|field| {
        matches!(field.ty, TypedFieldType::Array(_)) && field.array_elem_struct.is_some()
    })
}

fn unsafe_selected_buffer_shape(base: &str, env: ExprEnv<'_>) -> Option<UnsafeStorageShape> {
    let DeclaredSymbolInfo::Buffer {
        elem_ty,
        channels,
        is_array: true,
        ..
    } = env.declared_symbols.get(base)?
    else {
        return None;
    };
    let has_channel = matches!(channels, BufferChannelInfo::Dynamic)
        || matches!(channels, BufferChannelInfo::Static(count) if *count > 1);
    Some(UnsafeStorageShape {
        index_count: 1 + usize::from(has_channel),
        element: UnsafeStorageElement::Primitive(Some(*elem_ty)),
        readable: true,
        writable: true,
    })
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
            Some(PROC_INDEX_UNCHECKED_ARG) => {}
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
            | Some(PROC_INDEX_UNCHECKED_ARG)
            | Some(PROC_FIELD_SENTINEL_ARG) => {}
            _ => {
                if !is_by_ref_call_arg_expr(&arg.expr, env) {
                    validate_expr(&arg.expr, env, errors);
                }
            }
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
            Some(PROC_INDEX_UNCHECKED_ARG) => {}
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
